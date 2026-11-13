//! `codefast hook EVENT`: what a harness's lifecycle hooks run.
//!
//! Four events, all of them optional accelerators that add context and never decide anything:
//!
//! * `session-start` makes sure the repository has an index (starting a detached build when it
//!   has none) and returns a one-line note;
//! * `user-prompt-submit` reads the prompt for symbols it names and, for at most two that the
//!   index knows without doubt, adds where each is defined, what it is for and how widely it
//!   is used: the lookup an agent would otherwise spend its first steps on;
//! * `post-tool-use` looks at a search the agent just ran (`grep`, `rg`, `find -name`, the
//!   harness's own search tool) and, when it was a search for one identifier that is a known
//!   symbol, adds where the symbol is defined and who calls it;
//!   after an edit it compares the changed files with their committed version and, when the
//!   edit removed, renamed or re-signatured something that other code still uses, lists those
//!   places (said once: an edit that leaves nothing behind adds nothing);
//! * `pre-tool-use` looks at a whole-file read of a long source file and adds one line saying
//!   how long it is and which command prints its outline.
//!
//! After a compaction (`session-start` with source `compact`) the note is instead the list of
//! definitions the uncommitted changes added, removed and altered: what the session did so far,
//! in a few lines, at the moment its own record of it was summarised away.
//!
//! The payload is the harness's hook input (Claude Code and Codex share the shape; Cursor and
//! Gemini CLI are recognised by their own field names). The reply is the harness's hook output
//! carrying `additionalContext`, or nothing. A hook never blocks or rewrites a tool call, prints
//! nothing on any error, and always leaves the process with status 0.
//!
//! # Wiring (what a hooks file should register)
//!
//! | Harness event | Matcher | Command |
//! |---|---|---|
//! | `SessionStart` (every source) | none | `codefast hook session-start` |
//! | `UserPromptSubmit` | none | `codefast hook user-prompt-submit` |
//! | `PostToolUse`, and `PostToolUseFailure` where it exists (a `grep` without matches exits 1) | `Bash\|Grep\|Glob\|Edit\|Write` | `codefast hook post-tool-use` |
//! | `PreToolUse` | `Read\|Bash` | `codefast hook pre-tool-use` |
//!
//! # Environment
//!
//! `CODEFAST_HOOKS=off` (or `0`, `false`, `no`) turns all three events off.
//! `CODEFAST_HOOK_BUDGET_MS` is how long `post-tool-use` may wait for the caller summary
//! (default 50; the definition line goes out either way). `--root`, `CODEFAST_ROOT`,
//! `CODEFAST_INDEX_DIR` and `CODEFAST_NO_REFRESH` mean what they mean for every other command:
//! with the last one set, an existing index is never refreshed behind the scenes.
//!
//! Any other tool, event or payload is answered with nothing, so a broader matcher only costs
//! the few milliseconds of a process start. Cursor (`postToolUse`, `sessionStart`; tools `Shell`,
//! `Grep`, `Read`) and Gemini CLI (`AfterTool`, `SessionStart`; tools `run_shell_command`,
//! `search_file_content`, `read_file`) payloads are understood too; neither has a channel for
//! context before a tool call, so `pre-tool-use` stays silent there.

use super::prompt::{self, Mention};
use super::quote;
use super::shell::{self, Dir, Ident, Search};
use crate::build::{self, BASE_SEGMENT, OVERLAY_SEGMENT};
use crate::fresh::{self, OpenOptions};
use crate::index::Segment;
use crate::output::count;
use crate::query::search::usage_line;
use crate::query::{def_line, split_kind_prefix};
use crate::resolve::{Class, Resolver, Target};
use crate::store::{DefId, FileId, Index};
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A whole-file read is worth a hint above this many lines.
const READ_LINES: u64 = 400;
/// About 150 tokens at 2.8 bytes per token (`output::est_tokens`).
const NOTE_BYTES: usize = 420;
/// A hook that has no answer by now gives none.
const HARD_DEADLINE: Duration = Duration::from_millis(700);
/// The caller summary is added only when it is ready this soon after the hook started; the
/// definition goes out either way. `CODEFAST_HOOK_BUDGET_MS` overrides it.
const USAGE_DEADLINE: Duration = Duration::from_millis(50);
/// A name mentioned in more files than this is not summarised at all: resolving every use of
/// `kfree` takes longer than any hook may, and the count of files says enough.
const USAGE_MAX_FILES: usize = 1000;
/// How long `session-start` waits for a first index before it reports "being built".
const BUILD_WAIT: Duration = Duration::from_millis(400);
/// Names with more definitions than this are too ambiguous to answer unasked.
const MAX_DEFS: usize = 48;
const MAX_ENTITIES: usize = 8;
/// Symbols of a prompt that get a card. Context nobody asked for pays only while it is
/// short and right: two cards, and none for a name the index cannot pin down.
const MAX_CARDS: usize = 2;
const CARD_BYTES: usize = 380;
/// A prompt is answered once per turn and an edit check has files to read: both may take longer
/// than a note after a search.
const SLOW_DEADLINE: Duration = Duration::from_millis(2500);
/// What an edit check may say in one go, and how many changed files it looks at.
const CHECK_BYTES: usize = 2600;
const CHECK_FILES: usize = 40;
/// The summary of the uncommitted changes after a compaction, in tokens.
const RECAP_TOKENS: usize = 350;

const LOCK_FILE: &str = "refresh.lock";
const LOCK_STALE: Duration = Duration::from_secs(600);
/// Touched whenever a hook asks for a refresh, so that a stale file asks at most this often.
const STAMP_FILE: &str = "refresh.stamp";
const REFRESH_EVERY: Duration = Duration::from_secs(20);
const SESSIONS_DIR: &str = "sessions";
/// Touched when a session was told what its harness configuration costs, so that the next
/// sessions of the week are not told again.
const AUDIT_STAMP: &str = "audit.stamp";
const AUDIT_EVERY: Duration = Duration::from_secs(7 * 24 * 3600);
const SESSION_FILE_MAX: u64 = 256 << 10;
const SESSION_KEEP: Duration = Duration::from_secs(14 * 24 * 3600);
const ANONYMOUS_KEEP: Duration = Duration::from_secs(4 * 3600);

const MAX_PAYLOAD_BYTES: usize = 64 << 20;
const MAX_TOOL_INPUT_BYTES: usize = 256 << 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Event {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    /// Internal: the detached worker `session-start` leaves behind.
    Refresh,
}

impl Event {
    fn parse(s: &str) -> Option<Event> {
        let key: String = s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect();
        Some(match key.as_str() {
            "sessionstart" => Event::SessionStart,
            "userpromptsubmit" => Event::UserPromptSubmit,
            "pretooluse" | "beforetool" => Event::PreToolUse,
            "posttooluse" | "posttoolusefailure" | "aftertool" => Event::PostToolUse,
            "indexrefresh" => Event::Refresh,
            _ => return None,
        })
    }

    /// The names a harness gives this event in `hook_event_name`, the canonical one first.
    fn names(self) -> &'static [&'static str] {
        match self {
            Event::SessionStart => &["SessionStart"],
            Event::UserPromptSubmit => &["UserPromptSubmit"],
            Event::PreToolUse => &["PreToolUse"],
            Event::PostToolUse => &["PostToolUse", "PostToolUseFailure", "AfterTool"],
            Event::Refresh => &[],
        }
    }
}

pub(super) fn disabled() -> bool {
    std::env::var("CODEFAST_HOOKS").is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "off" | "0" | "false" | "no" | "disabled"))
}

fn usage_deadline() -> Duration {
    match std::env::var("CODEFAST_HOOK_BUDGET_MS").ok().and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(ms) => Duration::from_millis(ms).min(HARD_DEADLINE),
        None => USAGE_DEADLINE,
    }
}

// ----- the payload -----

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// End of the JSON string that starts at `b[i] == b'"'`.
fn string_end(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i + 1;
    loop {
        j += memchr::memchr2(b'"', b'\\', b.get(j..)?)?;
        if b[j] == b'"' {
            return Some(j + 1);
        }
        j += 2;
    }
}

/// End of the JSON value that starts at `b[i]`. Nothing is decoded and nothing recurses, so a
/// value of any size or depth costs one pass over its bytes.
fn value_end(b: &[u8], i: usize) -> Option<usize> {
    match *b.get(i)? {
        b'"' => string_end(b, i),
        b'{' | b'[' => {
            let (mut depth, mut j) = (0usize, i);
            while j < b.len() {
                match b[j] {
                    b'"' => {
                        j = string_end(b, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']') && !b[j].is_ascii_whitespace() {
                j += 1;
            }
            (j > i).then_some(j)
        }
    }
}

const WANTED: &[&str] =
    &["session_id", "conversation_id", "agent_id", "cwd", "workspace_roots", "hook_event_name", "tool_name", "tool_input", "tool_response", "tool_output", "cursor_version", "prompt", "source"];

/// The wanted members of the payload's top-level object, as raw JSON text. Everything else is
/// stepped over without being parsed. `None` unless the input is one JSON object and nothing
/// more.
fn members(input: &str) -> Option<Vec<(String, &str)>> {
    let b = input.as_bytes();
    let mut i = skip_ws(b, 0);
    if b.get(i) != Some(&b'{') {
        return None;
    }
    i = skip_ws(b, i + 1);
    let mut out: Vec<(String, &str)> = Vec::new();
    if b.get(i) != Some(&b'}') {
        loop {
            if b.get(i) != Some(&b'"') {
                return None;
            }
            let key_end = string_end(b, i)?;
            let key: String = serde_json::from_str(input.get(i..key_end)?).ok()?;
            i = skip_ws(b, key_end);
            if b.get(i) != Some(&b':') {
                return None;
            }
            i = skip_ws(b, i + 1);
            let end = value_end(b, i)?;
            if WANTED.contains(&key.as_str()) && !out.iter().any(|(k, _)| *k == key) {
                out.push((key, input.get(i..end)?));
            }
            i = skip_ws(b, end);
            match *b.get(i)? {
                b',' => i = skip_ws(b, i + 1),
                b'}' => break,
                _ => return None,
            }
        }
    }
    (skip_ws(b, i + 1) == b.len()).then_some(out)
}

/// Every string of a tool's output, one after the other. Harnesses differ in the shape (a
/// string, an object with `stdout`, a JSON document inside a string), not in what was printed.
fn output_text(raw: &str) -> Option<String> {
    fn walk(v: &Value, out: &mut String, top: bool) {
        match v {
            Value::String(s) => {
                if top && s.starts_with('{') {
                    if let Ok(inner) = serde_json::from_str::<Value>(s) {
                        return walk(&inner, out, false);
                    }
                }
                out.push_str(s);
                out.push('\n');
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out, false)),
            Value::Object(o) => o.values().for_each(|x| walk(x, out, false)),
            _ => {}
        }
    }
    if raw.len() > MAX_TOOL_INPUT_BYTES {
        return None;
    }
    let mut out = String::new();
    walk(&serde_json::from_str(raw).ok()?, &mut out, true);
    (!out.is_empty()).then_some(out)
}

#[derive(Default, Debug)]
struct Payload {
    session: Option<String>,
    /// Set when the hook fires inside a subagent, which has a context window of its own.
    agent: Option<String>,
    cwd: Option<String>,
    event_name: Option<String>,
    tool: Option<String>,
    input: Option<Value>,
    /// What the user typed (`UserPromptSubmit`).
    prompt: Option<String>,
    /// Why the session starts (`SessionStart`): `startup`, `resume`, `clear`, `compact`, `fork`.
    source: Option<String>,
    /// What the tool printed, when the payload carries it and it is not large.
    output: Option<String>,
    /// Cursor's hook protocol: flat, snake-case output.
    cursor: bool,
}

impl Payload {
    fn parse(input: &str) -> Option<Payload> {
        let mut p = Payload::default();
        let string = |raw: &str| serde_json::from_str::<String>(raw).ok().filter(|s| !s.is_empty());
        for (key, raw) in members(input)? {
            match key.as_str() {
                "session_id" => p.session = string(raw).or(p.session),
                "conversation_id" => p.session = p.session.or_else(|| string(raw)),
                "agent_id" => p.agent = string(raw),
                "cwd" => p.cwd = string(raw).or(p.cwd),
                "workspace_roots" => {
                    if p.cwd.is_none() && raw.len() <= MAX_TOOL_INPUT_BYTES {
                        p.cwd = serde_json::from_str::<Vec<String>>(raw).ok().and_then(|v| v.into_iter().next());
                    }
                }
                "hook_event_name" => p.event_name = string(raw),
                "prompt" => {
                    if raw.len() <= MAX_TOOL_INPUT_BYTES {
                        p.prompt = string(raw);
                    }
                }
                "source" => p.source = string(raw),
                "tool_name" => p.tool = string(raw),
                "tool_input" => {
                    if raw.len() <= MAX_TOOL_INPUT_BYTES {
                        p.input = serde_json::from_str(raw).ok();
                    }
                }
                "tool_response" | "tool_output" => p.output = output_text(raw).or(p.output),
                "cursor_version" => p.cursor = true,
                _ => {}
            }
        }
        if p.event_name.as_deref().is_some_and(|n| n.starts_with(|c: char| c.is_ascii_lowercase())) {
            p.cursor = true;
        }
        Some(p)
    }

    /// [`Payload::reply`], with a line for the person at the keyboard as well (`systemMessage`:
    /// shown by the harness, not sent to the model). Harnesses without that channel get the
    /// plain reply.
    fn reply_with(&self, event: Event, text: &str, to_user: Option<&str>) -> String {
        let plain = self.reply(event, text);
        let Some(msg) = to_user.filter(|_| !self.cursor && !plain.is_empty()) else { return plain };
        match serde_json::from_str::<Value>(&plain) {
            Ok(Value::Object(mut o)) => {
                o.insert("systemMessage".to_string(), Value::String(msg.to_string()));
                format!("{}\n", Value::Object(o))
            }
            _ => plain,
        }
    }

    /// The hook output that carries `text` to the model, in the harness's own schema; empty
    /// when the payload belongs to another event or the harness has no such channel.
    fn reply(&self, event: Event, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        let canonical = event.names()[0];
        let given = self.event_name.as_deref();
        if self.cursor {
            // Cursor reads `additional_context` after a tool call and at session start only.
            let fits = given.is_none_or(|n| n.eq_ignore_ascii_case(canonical));
            let has_channel = !matches!(event, Event::PreToolUse | Event::UserPromptSubmit);
            return if fits && has_channel { format!("{}\n", json!({"additional_context": text})) } else { String::new() };
        }
        let name = match given {
            None => canonical,
            Some(n) => match event.names().iter().find(|k| **k == n) {
                Some(k) => *k,
                None => return String::new(),
            },
        };
        format!("{}\n", json!({"hookSpecificOutput": {"hookEventName": name, "additionalContext": text}}))
    }
}

impl Payload {
    /// What "once per session" is counted against: the session, and within it each subagent,
    /// because a subagent has not seen what the main conversation was told.
    fn audience(&self) -> Option<String> {
        match (&self.session, &self.agent) {
            (Some(s), Some(a)) => Some(format!("{s}\u{0}{a}")),
            (s, _) => s.clone(),
        }
    }
}

/// What the agent's tool call was, as far as the hooks care.
#[derive(Debug, PartialEq)]
enum Call {
    Shell { command: String, dir: Option<String> },
    Search(Search),
    Read { path: String, whole: bool },
    /// A tool that writes files: the files it named.
    Edit { paths: Vec<String> },
}

impl Payload {
    fn call(&self) -> Option<Call> {
        let input = self.input.as_ref()?;
        let text = |keys: &[&str]| keys.iter().find_map(|k| input.get(*k).and_then(Value::as_str)).filter(|s| !s.is_empty()).map(str::to_string);
        let number = |key: &str| input.get(key).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
        match self.tool.as_deref()?.to_ascii_lowercase().as_str() {
            "bash" | "shell" | "run_shell_command" | "exec_command" | "local_shell" | "run_terminal_cmd" | "execute_command" => {
                let command = match input.get("command").or_else(|| input.get("cmd"))? {
                    Value::String(s) => s.clone(),
                    // An argument vector: rebuilt as a command line so that one reader serves both.
                    Value::Array(a) => a.iter().map(|x| x.as_str().map(quote)).collect::<Option<Vec<_>>>()?.join(" "),
                    _ => return None,
                };
                Some(Call::Shell { command, dir: text(&["working_directory", "workdir", "cwd", "directory", "dir_path"]) })
            }
            "grep" | "search_file_content" | "grep_search" => {
                let pattern = text(&["pattern", "query", "regex"])?;
                let paths = text(&["path", "dir_path"]).into_iter().collect();
                Some(Call::Search(Search { pattern, fixed: false, by_name: false, paths, dir: Dir::Start }))
            }
            "glob" | "glob_file_search" => {
                let pattern = text(&["pattern", "glob_pattern"])?;
                let paths = text(&["path", "dir_path", "target_directory"]).into_iter().collect();
                Some(Call::Search(Search { pattern, fixed: false, by_name: true, paths, dir: Dir::Start }))
            }
            "read" | "read_file" => {
                let path = text(&["file_path", "absolute_path", "path", "target_file"])?;
                let whole = number("offset").is_none_or(|o| o <= 1) && number("limit").is_none_or(|l| l > READ_LINES);
                Some(Call::Read { path, whole })
            }
            "edit" | "write" | "multiedit" | "replace" | "write_file" | "edit_file" | "create_file" | "str_replace_editor" | "str_replace_based_edit_tool" | "search_replace" => {
                Some(Call::Edit { paths: text(&["file_path", "absolute_path", "path", "target_file"]).into_iter().collect() })
            }
            // Codex: one patch, any number of files, each announced on a line of its own.
            "apply_patch" => {
                let patch = text(&["command", "patch", "input"])?;
                let mut paths = Vec::new();
                for line in patch.lines() {
                    for lead in ["*** Update File: ", "*** Add File: ", "*** Delete File: ", "*** Move to: "] {
                        if let Some(p) = line.strip_prefix(lead) {
                            paths.push(p.trim().to_string());
                        }
                    }
                }
                Some(Call::Edit { paths })
            }
            _ => None,
        }
    }
}

// ----- which repository -----

/// A configured value that can name a directory: not empty, and not a placeholder the harness
/// left unexpanded.
fn usable(value: std::ffi::OsString) -> Option<PathBuf> {
    let ok = {
        let s = value.to_string_lossy();
        !s.trim().is_empty() && !s.contains("${")
    };
    ok.then(|| PathBuf::from(value))
}

/// The value of `--root` on this process's command line, if it was given.
fn root_flag() -> Option<std::ffi::OsString> {
    let mut args = std::env::args_os().skip(1);
    while let Some(a) = args.next() {
        if a == "--" {
            break;
        }
        if a == "--root" {
            return args.next();
        }
        if let Some(v) = a.to_str().and_then(|s| s.strip_prefix("--root=")) {
            return Some(v.into());
        }
    }
    None
}

struct Place {
    /// Canonical repository root: where `codefast` run by the agent would resolve to.
    root: PathBuf,
    /// The root was named by `--root` or `CODEFAST_ROOT`, not derived from a directory.
    explicit: bool,
    index_dir: PathBuf,
}

impl Place {
    /// The repository a hook payload is about: `--root`, `CODEFAST_ROOT`, the work tree around
    /// the payload's `cwd` (or the harness's project directory), then what the command line
    /// resolved from the process's own working directory.
    fn find(passed: &Path, cwd: Option<&str>) -> Option<Place> {
        let (root, explicit) = match root_flag().and_then(usable).or_else(|| std::env::var_os("CODEFAST_ROOT").and_then(usable)) {
            Some(r) => (r, true),
            None => {
                let from_env = || ["CLAUDE_PROJECT_DIR", "CURSOR_PROJECT_DIR", "GEMINI_PROJECT_DIR"].iter().find_map(|k| std::env::var_os(k).and_then(usable));
                let dir = cwd.map(PathBuf::from).or_else(from_env).filter(|d| d.is_absolute() && d.is_dir());
                match dir {
                    Some(d) => (crate::mcp::work_tree_of(&d).unwrap_or(d), false),
                    None => (passed.to_path_buf(), false),
                }
            }
        };
        let root = root.canonicalize().ok()?;
        let index_dir = build::index_dir(&root);
        Some(Place { root, explicit, index_dir })
    }

    /// Whether an index may be built here unasked: a git work tree (or a directory that already
    /// carries an index, or one the user named), never a home directory or a filesystem root.
    fn indexable(&self) -> bool {
        let home = std::env::var_os("HOME").map(PathBuf::from).and_then(|h| h.canonicalize().ok());
        if self.root.parent().is_none() || home.as_deref() == Some(self.root.as_path()) {
            return false;
        }
        self.explicit || self.root.join(".git").exists() || self.root.join(build::INDEX_DIR).is_dir()
    }

    /// `path` relative to the root with `/` separators, when it lies inside the root.
    fn relative(&self, path: &Path) -> Option<String> {
        let real = path.canonicalize().ok()?;
        let rel = real.strip_prefix(&self.root).ok()?.to_str()?;
        Some(if std::path::MAIN_SEPARATOR == '/' { rel.to_string() } else { rel.replace(std::path::MAIN_SEPARATOR, "/") })
    }
}

/// Where a path written in a command points, given the directory the command ran in.
fn locate(base: &Path, dir: &Dir, path: &str) -> Option<PathBuf> {
    let path = match path.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME")?).join(rest),
        None => PathBuf::from(path),
    };
    if path.is_absolute() {
        return Some(path);
    }
    match dir {
        Dir::Start => Some(base.join(path)),
        Dir::At(d) => Some(base.join(d).join(path)),
        Dir::Unknown => None,
    }
}

/// Open the index as it is on disk: the base segment plus the overlay the last refresh left.
/// The working tree is not walked and nothing is written, so this costs two memory maps
/// whatever the repository size.
pub(super) fn open_at(root: &Path, index_dir: &Path) -> Option<Index> {
    let base = Segment::open(&index_dir.join(BASE_SEGMENT)).ok().filter(fresh::built_by_this_version)?;
    let files = base.n_files();
    let overlay = Segment::open(&index_dir.join(OVERLAY_SEGMENT)).ok().filter(|o| o.meta("base").is_some() && o.meta("base") == base.meta("build_id"));
    let mut dead: Vec<u32> = Vec::new();
    if let Some(bytes) = overlay.as_ref().and_then(|o| o.section(b"DEAD")) {
        if bytes.len() % 4 != 0 {
            return None;
        }
        dead = bytes.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
        if dead.iter().any(|&f| f >= files) {
            return None;
        }
    }
    Some(Index::assemble(root.to_path_buf(), base, overlay, &dead))
}

fn open_index(place: &Place) -> Option<Index> {
    open_at(&place.root, &place.index_dir)
}

fn mtime_ns(md: &std::fs::Metadata) -> i64 {
    md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

/// Whether the file on disk is the one the index describes. Line numbers are only ever
/// reported for files that are.
fn current(ix: &Index, file: FileId) -> bool {
    let rec = ix.file(file);
    std::fs::metadata(ix.root.join(ix.path(file))).is_ok_and(|md| md.len() == rec.size && mtime_ns(&md) == rec.mtime_ns)
}

// ----- what was already said in this session -----

struct Session {
    file: PathBuf,
}

impl Session {
    /// The state file of a session, kept in the index directory. The session id is hashed: it
    /// comes from the payload and must not choose a path.
    fn open(place: &Place, id: Option<&str>) -> Option<Session> {
        if !place.index_dir.is_dir() {
            return None;
        }
        let dir = place.index_dir.join(SESSIONS_DIR);
        let file = match id {
            Some(id) => dir.join(format!("{:016x}", xxhash_rust::xxh3::xxh3_64(id.as_bytes()))),
            None => {
                // Without an id, "this session" is the last few hours.
                let file = dir.join("anonymous");
                if age(&file).is_some_and(|a| a > ANONYMOUS_KEEP) {
                    let _ = std::fs::remove_file(&file);
                }
                file
            }
        };
        Some(Session { file })
    }

    /// The key a line of the state file records: the line itself, or what stands before the
    /// tab that separates it from the writer's mark.
    fn key_of(line: &str) -> &str {
        line.split_once('\t').map_or(line, |(key, _)| key)
    }

    fn seen(&self, key: &str) -> bool {
        match std::fs::metadata(&self.file) {
            // A session that has been told this much has been told enough.
            Ok(md) if md.len() > SESSION_FILE_MAX => true,
            Ok(_) => std::fs::read_to_string(&self.file).is_ok_and(|text| text.lines().any(|l| Self::key_of(l) == key)),
            Err(_) => false,
        }
    }

    /// Record `key`, and say whether this call was the one that recorded it. A harness runs the
    /// hooks of parallel tool calls at the same time, so several processes can find the key
    /// unrecorded at once ([`Session::seen`] is only a first look). Each of them appends a line
    /// with a mark of its own in one write, which the kernel places after everything written
    /// before it; whoever's line then stands first in the file is the one that may speak.
    ///
    /// False as well when the key cannot be recorded, in which case nothing may be said: a
    /// note that cannot be remembered would be repeated on every call.
    fn claim(&self, key: &str) -> bool {
        if key.is_empty() || key.contains(['\n', '\r', '\t']) {
            return false;
        }
        if std::fs::metadata(&self.file).is_ok_and(|md| md.len() > SESSION_FILE_MAX) {
            return false;
        }
        if let Some(dir) = self.file.parent()
            && std::fs::create_dir_all(dir).is_err()
        {
            return false;
        }
        // Unique among everything that can write here: the process, the moment, the call.
        static CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let mark = format!("{:x}.{nanos:x}.{:x}", std::process::id(), CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let appended =
            std::fs::OpenOptions::new().create(true).append(true).open(&self.file).and_then(|mut f| f.write_all(format!("{key}\t{mark}\n").as_bytes()));
        if appended.is_err() {
            return false;
        }
        let Ok(text) = std::fs::read_to_string(&self.file) else { return false };
        text.lines().find(|l| Self::key_of(l) == key).and_then(|l| l.split_once('\t')).is_some_and(|(_, first)| first == mark)
    }
}

fn age(path: &Path) -> Option<Duration> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(SystemTime::now().duration_since(modified).unwrap_or_default())
}

/// Forget sessions that ended long ago.
fn prune_sessions(place: &Place) {
    let Ok(entries) = std::fs::read_dir(place.index_dir.join(SESSIONS_DIR)) else { return };
    for entry in entries.flatten().take(2048) {
        if age(&entry.path()).is_some_and(|a| a > SESSION_KEEP) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

// ----- the command to suggest -----

/// How the agent's shell reaches this program: its bare name when that resolves on `PATH`,
/// else the path of the running executable.
fn cli() -> String {
    let name = if cfg!(windows) { "codefast.exe" } else { "codefast" };
    let on_path = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()));
    // Claude Code puts an enabled plugin's `bin/` on the PATH of its shell tool, though not
    // on the PATH of the hooks it runs. (Codex sets the same variable for compatibility,
    // next to its own `PLUGIN_ROOT`, and adds nothing to PATH.)
    let from_plugin = std::env::var_os("PLUGIN_ROOT").is_none()
        && std::env::var_os("CLAUDE_PLUGIN_ROOT").and_then(usable).is_some_and(|root| root.join("bin").join(name).is_file());
    if on_path || from_plugin {
        return "codefast".to_string();
    }
    match std::env::current_exe().ok().and_then(|p| p.to_str().map(quote)) {
        Some(exe) => exe,
        None => "codefast".to_string(),
    }
}

fn clip_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max.saturating_sub(3);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

// ----- session-start -----

fn index_counts(place: &Place) -> Option<(usize, usize)> {
    let seg = Segment::open(&place.index_dir.join(BASE_SEGMENT)).ok()?;
    let number = |key: &str| seg.meta(key).and_then(|v| v.parse::<usize>().ok());
    Some((number("files").unwrap_or(seg.n_files() as usize), number("defs").unwrap_or(seg.defs().len())))
}

fn lock_is_held(place: &Place) -> bool {
    age(&place.index_dir.join(LOCK_FILE)).is_some_and(|a| a < LOCK_STALE)
}

/// Start `codefast hook index-refresh` for this repository as a process of its own session,
/// with no ties to this one: the harness must not wait for it or stop it.
fn spawn_refresh(place: &Place) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--root").arg(&place.root).arg("hook").arg("index-refresh");
    cmd.current_dir(&place.root).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe extern "C" {
            fn setsid() -> i32;
        }
        // SAFETY: `setsid` is async-signal-safe and touches no memory of this process.
        unsafe {
            cmd.pre_exec(|| {
                setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        cmd.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
    }
    cmd.spawn().is_ok()
}

/// The detached worker: build the index when there is none, else bring it up to date with the
/// working tree. One worker per index at a time.
fn refresh(root: &Path) {
    let Ok(root) = root.canonicalize() else { return };
    // The same line `session-start` draws: never a home directory, never a filesystem root.
    let home = std::env::var_os("HOME").map(PathBuf::from).and_then(|h| h.canonicalize().ok());
    if root.parent().is_none() || home.as_deref() == Some(root.as_path()) {
        return;
    }
    let Ok(dir) = build::ensure_index_dir(&root) else { return };
    let lock = dir.join(LOCK_FILE);
    if age(&lock).is_some_and(|a| a >= LOCK_STALE) {
        let _ = std::fs::remove_file(&lock);
    }
    let Ok(mut held) = std::fs::OpenOptions::new().write(true).create_new(true).open(&lock) else { return };
    let _ = writeln!(held, "{}", std::process::id());
    drop(held);
    // A missing index is built either way; an existing one is only touched when allowed.
    let _ = fresh::open(&root, &OpenOptions { refresh: refresh_allowed(), quiet: true });
    let _ = std::fs::remove_file(&lock);
}

/// `CODEFAST_NO_REFRESH` asks every command to trust the index as it is; the hooks do too.
fn refresh_allowed() -> bool {
    std::env::var_os("CODEFAST_NO_REFRESH").is_none()
}

/// Ask for a background refresh because a hook met a file newer than the index. Rate-limited
/// through a stamp file, and skipped when the stamp cannot be written.
fn request_refresh(place: &Place) {
    let stamp = place.index_dir.join(STAMP_FILE);
    if !refresh_allowed() || lock_is_held(place) || age(&stamp).is_some_and(|a| a < REFRESH_EVERY) {
        return;
    }
    if std::fs::write(&stamp, b"").is_ok() {
        spawn_refresh(place);
    }
}

fn session_start(passed: &Path, payload: &Payload) -> String {
    let Some(place) = Place::find(passed, payload.cwd.as_deref()) else { return String::new() };
    let mut counts = index_counts(&place);
    if counts.is_none() {
        if !place.indexable() {
            return String::new();
        }
        if !lock_is_held(&place) && !spawn_refresh(&place) {
            return String::new();
        }
        // A small tree is indexed before this note is written.
        let until = Instant::now() + BUILD_WAIT;
        while counts.is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(15));
            counts = index_counts(&place);
        }
    } else {
        prune_sessions(&place);
        // Fold what changed since the last run into the index, out of the agent's way.
        if refresh_allowed() && !lock_is_held(&place) {
            spawn_refresh(&place);
        }
    }
    let cli = cli();
    // After a compaction the conversation has lost its own record of what it changed. The
    // work tree still has it: the definitions added, removed and altered so far.
    if payload.source.as_deref() == Some("compact") && counts.is_some() && refresh_allowed() {
        if let Ok(ix) = fresh::open(&place.root, &OpenOptions { refresh: true, quiet: true }) {
            let mut out = crate::output::Out::new(RECAP_TOKENS);
            if crate::query::changed::summary(&ix, &crate::query::Common::default(), &mut out) > 0 {
                let recap = out.finish();
                return payload.reply(Event::SessionStart, &format!("codefast: {}`{cli} check` lists uses left behind by these changes, `{cli} search NAME` a symbol with every place that uses it.", recap));
            }
        }
    }
    let state = match counts {
        Some((files, defs)) => format!("codefast has an index of this repository ({} files, {} symbols)", count(files), count(defs)),
        None => "codefast is indexing this repository in the background (first run; its commands wait until it is ready)".to_string(),
    };
    // One line. What the tool can do is in its skill and its tool list: said again here, on
    // every session, it would be paid for on every request and read on none.
    let note = format!("{state}: `{cli} search NAME` prints a symbol's definition, its body and every place that uses it in one call.");
    payload.reply_with(Event::SessionStart, &note, context_cost(&place, payload, &cli).as_deref())
}

/// Once a week per project, for the person and not for the model: what the harness's own
/// configuration puts into every request here without the project having a use for it. Only
/// for Claude Code (which says so in the environment of its hooks), at the start of a session.
fn context_cost(place: &Place, payload: &Payload, cli: &str) -> Option<String> {
    if std::env::var_os("CLAUDECODE").is_none() || payload.agent.is_some() || !matches!(payload.source.as_deref(), None | Some("startup")) {
        return None;
    }
    let stamp = place.index_dir.join(AUDIT_STAMP);
    if age(&stamp).is_some_and(|a| a < AUDIT_EVERY) || !place.index_dir.is_dir() {
        return None;
    }
    let note = super::audit::session_note(&place.root, cli)?;
    std::fs::write(&stamp, b"").ok()?;
    Some(format!("codefast: {note}."))
}

// ----- user-prompt-submit -----

/// The one entity a mention names, or nothing when the index leaves any doubt.
fn resolve_mention<'a>(ix: &'a Index, rs: &Resolver<'a>, m: &Mention, paths: &[String]) -> Option<Target> {
    let nid = ix.name_id(m.name.as_bytes())?;
    let mut defs: Vec<DefId> = ix.defs_named_id(nid).take(4 * MAX_DEFS + 1).collect();
    if defs.is_empty() || defs.len() > 4 * MAX_DEFS {
        return None;
    }
    if let Some(owner) = &m.owner {
        // A qualified name is taken at its word: the owner has to be the container, or (for
        // `module.function`) part of the path of the defining file.
        let held: Vec<DefId> = defs.iter().copied().filter(|&d| ix.def(d).parent_name() == Some(owner.as_bytes())).collect();
        defs = if held.is_empty() {
            let stem = format!("/{owner}.");
            let dir = format!("/{owner}/");
            defs.into_iter().filter(|&d| ix.def(d).parent().is_none() && (format!("/{}", ix.def(d).path()).contains(&stem) || format!("/{}", ix.def(d).path()).contains(&dir))).collect()
        } else {
            held
        };
    }
    let in_named_file: Vec<DefId> = defs.iter().copied().filter(|&d| paths.iter().any(|p| ix.def(d).path().ends_with(p.as_str()))).collect();
    let pinned = !in_named_file.is_empty();
    if pinned {
        defs = in_named_file;
    }
    if defs.len() > MAX_DEFS {
        return None;
    }
    let ents = rs.entities(&defs);
    let strong: Vec<&Target> = ents.iter().filter(|t| matches!(t.class, Class::Callable | Class::Type | Class::Value)).collect();
    let main = match (&ents[..], &strong[..]) {
        ([one], _) => one,
        // Several things share the name: only a name written as code, or pinned to a file, may
        // pick the one function or type among fields and locals of the same name.
        (_, [one]) if m.explicit || pinned => *one,
        _ => return None,
    };
    // A plain word of the prompt that happens to be a field name is not a mention of it.
    if !m.explicit && !pinned && !matches!(main.class, Class::Callable | Class::Type) {
        return None;
    }
    Some(main.clone())
}

/// `called from 19 sites in 13 files and 9 sites in 6 test files`, or nothing.
fn spread(ix: &Index, rs: &Resolver, t: &Target) -> Option<String> {
    let p = t.primary(ix);
    if p.flags() & crate::facts::dflag::TEST != 0 || ix.is_test_file(p.file()) {
        return None;
    }
    let files = ix.name_id(&t.name).map(|n| ix.files_with(n).count()).unwrap_or(0);
    if files > USAGE_MAX_FILES {
        return Some(format!("named in {} files", count(files)));
    }
    // A function is used where it is called or handed on. Anything else (a type, a variable,
    // a macro) is used wherever it is named.
    let named: Vec<crate::store::RefV> = if t.class == Class::Callable {
        let (edges, _) = crate::query::refs::caller_edges(rs, t);
        edges.iter().filter(|e| !matches!(e.via, crate::query::refs::Via::Slot(..))).map(|e| e.r).collect()
    } else {
        let mut v = Vec::new();
        rs.refs_to(t, |r, conf| {
            if conf != crate::resolve::Conf::Name && !matches!(r.kind(), crate::facts::RefKind::Import) {
                v.push(r);
            }
        });
        v
    };
    let (tests, code): (Vec<_>, Vec<_>) = named.into_iter().partition(|r| ix.is_test_file(r.file));
    let nfiles = |v: &[crate::store::RefV]| v.iter().map(|r| r.file).collect::<std::collections::HashSet<_>>().len();
    let verb = if t.class == Class::Callable { "called from" } else { "used at" };
    let part = |v: &[crate::store::RefV], what: &str| format!("{} site{} in {} {what}file{}", count(v.len()), if v.len() == 1 { "" } else { "s" }, count(nfiles(v)), if nfiles(v) == 1 { "" } else { "s" });
    match (code.is_empty(), tests.is_empty()) {
        // No use found is said of a function, where it is a fact worth having (dead code, or
        // reached in a way the index does not see). Of anything else it would be a guess.
        (true, true) if t.class == Class::Callable => Some("no call of it found in the indexed code".to_string()),
        (true, true) => None,
        (false, true) => Some(format!("{verb} {}", part(&code, ""))),
        (true, false) => Some(format!("{verb} {}", part(&tests, "test "))),
        (false, false) => Some(format!("{verb} {} and {}", part(&code, ""), part(&tests, "test "))),
    }
}

fn prompt_cards(place: Place, found: prompt::Mentions, session_id: Option<String>, cli: String, tx: mpsc::Sender<String>) {
    let Some(ix) = open_index(&place) else { return };
    let Some(session) = Session::open(&place, session_id.as_deref()) else { return };
    let rs = Resolver::new(&ix);
    let mut docs = crate::query::card::Docs::new(&ix);
    // Names written as code first, tests last: a failing test is where the agent starts
    // anyway, the function it is about is what it has to find.
    let mut order: Vec<&Mention> = found.symbols.iter().collect();
    order.sort_by_key(|m| (!m.explicit, m.name.starts_with("test") || m.name.starts_with("Test")));
    let mut sent = 0;
    let mut renamed = false;
    let mut seen: Vec<DefId> = Vec::new();
    for m in order {
        if sent >= MAX_CARDS {
            break;
        }
        let Some(target) = resolve_mention(&ix, &rs, m, &found.paths) else { continue };
        let def = target.primary(&ix);
        if seen.contains(&def.id) {
            continue;
        }
        // Exact or silent: a file edited since it was indexed may have moved its lines.
        if !current(&ix, def.file()) {
            request_refresh(&place);
            continue;
        }
        seen.push(def.id);
        // A test the prompt names together with its file: the agent knows where it is.
        let test = def.flags() & crate::facts::dflag::TEST != 0 || ix.is_test_file(def.file());
        if test && found.paths.iter().any(|p| def.path().ends_with(p.as_str())) {
            continue;
        }
        let name = def.qualified();
        if session.seen(&format!("p:{name}")) {
            continue;
        }
        let mut card = format!("{name} is defined at {}:{} {}", def.path(), def.rec.line, clip_bytes(&crate::query::sig_of(&def), 130));
        if let Some(doc) = docs.line(&def, 100) {
            card.push_str(&format!("\n  {doc}"));
        }
        // A prompt that asks for a rename of a function gets the command that does it, once,
        // in the place of the pointer to the list of callers.
        let rename = found.rename && target.class == Class::Callable && !renamed;
        if let Some(s) = spread(&ix, &rs, &target) {
            let verb = if target.class == Class::Callable { "callers" } else { "refs" };
            let all = if rename || s.starts_with("no call") || s.starts_with("named in") { String::new() } else { format!(" (`{cli} {verb} {name}` lists every one)") };
            card.push_str(&format!("\n  {s}{all}"));
        }
        let mut room = CARD_BYTES;
        if rename {
            renamed = true;
            let line = format!("\n  `{cli} rename {name} NEW_NAME` renames it at its definition and at every call the index is sure of, and lists the rest");
            card = clip_bytes(&card, CARD_BYTES);
            room += line.len();
            card.push_str(&line);
        }
        // The same symbol is not explained again when the agent greps for it.
        if !session.claim(&format!("p:{name}")) {
            continue;
        }
        session.claim(&format!("s:{}", String::from_utf8_lossy(&target.name)));
        if tx.send(clip_bytes(&card, room)).is_err() {
            return;
        }
        sent += 1;
    }
}

fn user_prompt(passed: &Path, payload: &Payload, started: Instant) -> String {
    let Some(text) = payload.prompt.as_deref() else { return String::new() };
    let found = prompt::mentions(text);
    if found.symbols.is_empty() {
        return String::new();
    }
    let Some(place) = Place::find(passed, payload.cwd.as_deref()) else { return String::new() };
    let session_id = payload.audience();
    let (tx, rx) = mpsc::channel();
    let cli = cli();
    let worker = std::thread::Builder::new().stack_size(16 << 20).spawn(move || prompt_cards(place, found, session_id, cli, tx));
    if worker.is_err() {
        return String::new();
    }
    // Whatever is ready when the time is up goes out: a card that is late is not waited for.
    let mut cards: Vec<String> = Vec::new();
    while let Ok(card) = rx.recv_timeout(SLOW_DEADLINE.saturating_sub(started.elapsed())) {
        cards.push(card);
    }
    if cards.is_empty() {
        return String::new();
    }
    payload.reply(Event::UserPromptSubmit, &format!("codefast: {}", cards.join("\n")))
}

// ----- post-tool-use -----

/// The first stage of an answer: the definition line.
struct Head {
    name: String,
    /// `path:line` of the definition.
    at: String,
    line: String,
    /// The search printed the line the definition starts on.
    shown: bool,
    /// Test code: what the agent is looking at when it searches for it.
    test: bool,
    /// The first sentence of its documentation.
    doc: Option<String>,
    callable: bool,
    /// Files whose text mentions the name.
    files: usize,
    /// Other entities that share the name.
    others: usize,
    session: Session,
}

enum Stage {
    Head(Head),
    Usage(Option<String>),
}

/// Directory (root-relative, with a trailing `/`) or file the search was limited to.
fn search_scope(place: &Place, base: &Path, search: &Search) -> Option<String> {
    let [only] = &search.paths[..] else { return None };
    let path = locate(base, &search.dir, only)?;
    let rel = place.relative(&path)?;
    if rel.is_empty() {
        return None;
    }
    Some(if path.is_dir() { format!("{rel}/") } else { rel })
}

fn pick<'a>(ix: &Index, rs: &Resolver<'a>, id: &Ident, scope: Option<&str>) -> Option<(Target, usize)> {
    let nid = ix.name_id(id.name.as_bytes())?;
    let mut defs: Vec<DefId> = ix.defs_named_id(nid).take(MAX_DEFS + 1).collect();
    if defs.is_empty() || defs.len() > MAX_DEFS {
        return None;
    }
    let mut narrow = |keep: &dyn Fn(DefId) -> bool| {
        let kept: Vec<DefId> = defs.iter().copied().filter(|&d| keep(d)).collect();
        if !kept.is_empty() {
            defs = kept;
        }
    };
    if let Some(kinds) = id.kind.and_then(|k| split_kind_prefix(&format!("{k} {}", id.name)).0) {
        narrow(&|d| kinds.contains(&ix.def(d).kind()));
    }
    if let Some(scope) = scope {
        narrow(&|d| ix.def(d).path().starts_with(scope));
    }
    let ents = rs.entities(&defs);
    if ents.is_empty() || ents.len() > MAX_ENTITIES {
        return None;
    }
    let strong: Vec<&Target> = ents.iter().filter(|t| matches!(t.class, Class::Callable | Class::Type | Class::Value)).collect();
    let main = if strong.len() == 1 { strong[0] } else { &ents[0] };
    Some((main.clone(), ents.len() - 1))
}

/// Whether a tool's output contains the line a definition starts on: the search covered the
/// file that defines it, and the agent has read where it is.
fn shows_definition(ix: &Index, def: &crate::store::DefV, output: &str) -> bool {
    let Some(src) = ix.source(def.file()) else { return false };
    let starts = crate::query::show::line_starts(&src);
    let line = String::from_utf8_lossy(crate::query::show::line_text(&src, &starts, def.rec.line)).trim().to_string();
    line.len() >= 6 && output.contains(&line)
}

fn answer_search(place: Place, base: PathBuf, searches: Vec<Search>, output: Option<String>, session_id: Option<String>, tx: mpsc::Sender<Stage>) {
    let Some(ix) = open_index(&place) else { return };
    let Some(session) = Session::open(&place, session_id.as_deref()) else { return };
    let rs = Resolver::new(&ix);
    for search in &searches {
        let Some(id) = shell::identifier(search) else { continue };
        if session.seen(&format!("s:{}", id.name)) {
            continue;
        }
        let scope = search_scope(&place, &base, search);
        let Some((target, others)) = pick(&ix, &rs, &id, scope.as_deref()) else { continue };
        let def = target.primary(&ix);
        // Exact or silent: a file edited since it was indexed may have moved its lines.
        if !current(&ix, def.file()) {
            request_refresh(&place);
            continue;
        }
        let files = ix.name_id(id.name.as_bytes()).map(|n| ix.files_with(n).count()).unwrap_or(0);
        let doc = crate::query::card::Docs::new(&ix).line(&def, 110);
        let test = def.flags() & crate::facts::dflag::TEST != 0 || ix.is_test_file(def.file());
        let shown = output.as_deref().is_some_and(|o| shows_definition(&ix, &def, o));
        let at = format!("{}:{}", def.path(), def.rec.line);
        let head = Head { name: id.name.clone(), at, line: def_line(&def), shown, test, doc, callable: target.class == Class::Callable, files, others, session };
        if tx.send(Stage::Head(head)).is_err() {
            return;
        }
        // Of the lines about its use, the one a text search cannot give comes first: where it
        // is registered or passed as a callback. The names of its callers otherwise.
        let usage = if files > USAGE_MAX_FILES {
            None
        } else {
            let lines = usage_line(&ix, &rs, &target, 4);
            lines.iter().find(|l| l.starts_with("registered") || l.starts_with("passed as a callback")).or(lines.first()).cloned()
        };
        let _ = tx.send(Stage::Usage(usage));
        return;
    }
}

fn symbol_note(head: &Head, usage: Option<&str>, cli: &str) -> String {
    let name = &head.name;
    // After the definition, best first: how it is used; without that (a type, a field, or a
    // symbol used too widely to summarise in time) where to get it; at the least, the count.
    let seconds: Vec<String> = match usage {
        Some(u) => vec![clip_bytes(u, 120)],
        None => {
            let files = format!("mentioned in {} file{}", count(head.files), if head.files == 1 { "" } else { "s" });
            let (verb, what) = if head.callable { ("callers", "its call sites") } else { ("refs", "its uses") };
            vec![format!("{files}; `{cli} {verb} {name}` lists {what}"), files]
        }
    };
    // What it is for, when its author said so: the line that saves opening the body.
    let doc = head.doc.as_deref().map(|d| format!("\n{d}")).unwrap_or_default();
    let others = match head.others {
        0 => String::new(),
        n => format!("\n{n} other symbol{} named {name}: `{cli} find {name}`", if n == 1 { " is" } else { "s are" }),
    };
    let lead = format!("codefast: {name} is defined at ");
    // The definition is the answer. What stands around it is dropped, piece by piece, before
    // the definition is cut: the other symbols of the name, then the sentence, then the command
    // on the usage line (which is what goes when this program is not on PATH and its own path
    // is very long). Nothing here tells the reader what to run next: a note states what is.
    let enough = head.line.len().min(100);
    for (doc, others) in [(doc.as_str(), others.as_str()), (doc.as_str(), ""), ("", "")] {
        for second in &seconds {
            let room = NOTE_BYTES.saturating_sub(lead.len() + doc.len() + second.len() + others.len() + 1);
            if room >= enough {
                return format!("{lead}{}{doc}\n{second}{others}", clip_bytes(&head.line, room));
            }
        }
    }
    clip_bytes(&format!("{lead}{}", head.line), NOTE_BYTES)
}

/// What the edits left behind, for the files a tool call wrote (all changed files when it did
/// not name them). Every finding is said once per session.
fn edit_findings(place: Place, base: Option<PathBuf>, paths: Vec<String>, session_id: Option<String>, tx: mpsc::Sender<String>) {
    if !refresh_allowed() {
        return;
    }
    let Some(session) = Session::open(&place, session_id.as_deref()) else { return };
    // The rest of the code is compared as it is now, so the index is brought up to date first.
    let Ok(ix) = fresh::open(&place.root, &OpenOptions { refresh: true, quiet: true }) else { return };
    let rs = Resolver::new(&ix);
    let mut files: Vec<String> = Vec::new();
    for p in &paths {
        let path = PathBuf::from(p);
        let abs = match (path.is_absolute(), &base) {
            (true, _) => path,
            (false, Some(b)) => b.join(path),
            _ => continue,
        };
        if let Some(rel) = place.relative(&abs) {
            files.push(rel);
        }
    }
    if paths.is_empty() {
        files = crate::query::changed::dirty_files(&place.root).into_iter().filter(|f| crate::lang::Lang::from_path(f).has_extractor()).take(CHECK_FILES).collect();
    }
    let mut said = 0usize;
    let mut held = 0usize;
    for rel in &files {
        for f in crate::query::changed::check_file(&ix, &rs, &place.root, rel, 500) {
            if session.seen(&f.key) {
                continue;
            }
            // The list of places is not printed twice: a command that showed it a few minutes
            // ago (the agent asked before it edited) is what "above" refers to.
            let text = if crate::recent::was_listed(&place.index_dir, &f.symbol, Duration::from_secs(900)) { f.brief } else { f.text };
            if said + text.len() > CHECK_BYTES && said > 0 {
                held += 1;
                continue;
            }
            if !session.claim(&f.key) {
                continue;
            }
            said += text.len();
            if tx.send(text).is_err() {
                return;
            }
        }
    }
    if held > 0 {
        let _ = tx.send(format!("(+{held} more findings: `{} check` lists them)", cli()));
    }
}

fn after_edit(passed: &Path, payload: &Payload, paths: Vec<String>, dir: Option<String>, started: Instant) -> String {
    let Some(place) = Place::find(passed, payload.cwd.as_deref()) else { return String::new() };
    let base = dir.or_else(|| payload.cwd.clone()).map(PathBuf::from).filter(|d| d.is_absolute());
    let session_id = payload.audience();
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::Builder::new().stack_size(16 << 20).spawn(move || edit_findings(place, base, paths, session_id, tx));
    if worker.is_err() {
        return String::new();
    }
    let mut found: Vec<String> = Vec::new();
    while let Ok(text) = rx.recv_timeout(SLOW_DEADLINE.saturating_sub(started.elapsed())) {
        found.push(text);
    }
    if found.is_empty() {
        return String::new();
    }
    payload.reply(Event::PostToolUse, &format!("codefast: {}", found.join("\n\n")))
}

fn post_tool_use(passed: &Path, payload: &Payload, started: Instant) -> String {
    let (searches, dir) = match payload.call() {
        Some(Call::Shell { command, dir }) => {
            let searches = shell::analyse(&command).searches;
            // A command that searched gets the note on what it searched for. One that may have
            // written files gets the check of what the edits left behind.
            if !searches.iter().any(|s| shell::identifier(s).is_some()) && shell::may_write(&command) {
                return after_edit(passed, payload, Vec::new(), dir, started);
            }
            (searches, dir)
        }
        Some(Call::Search(s)) => (vec![s], None),
        Some(Call::Edit { paths }) => return after_edit(passed, payload, paths, None, started),
        _ => return String::new(),
    };
    if !searches.iter().any(|s| shell::identifier(s).is_some()) {
        return String::new();
    }
    let Some(place) = Place::find(passed, payload.cwd.as_deref()) else { return String::new() };
    let base = dir.or_else(|| payload.cwd.clone()).map(PathBuf::from).filter(|d| d.is_absolute()).unwrap_or_else(|| place.root.clone());
    let session_id = payload.audience();
    let (tx, rx) = mpsc::channel();
    let output = payload.output.clone();
    let worker = std::thread::Builder::new().stack_size(16 << 20).spawn(move || answer_search(place, base, searches, output, session_id, tx));
    if worker.is_err() {
        return String::new();
    }
    let left = |limit: Duration| limit.saturating_sub(started.elapsed());
    let Ok(Stage::Head(head)) = rx.recv_timeout(left(HARD_DEADLINE)) else { return String::new() };
    let budget = usage_deadline();
    let usage = match rx.recv_timeout(left(budget)) {
        Ok(Stage::Usage(u)) if !budget.is_zero() => u,
        _ => None,
    };
    // What the agent has is not said again. When the search printed the definition, or the
    // symbol is a test (which the agent is looking at), a note is worth its tokens only for
    // what a text search does not print: what the function is for, who calls it, where it is
    // registered.
    let note = if head.shown || head.test {
        let news = usage.as_deref().filter(|u| !u.contains("no callers found"));
        let doc = head.doc.as_deref().filter(|_| !head.test);
        let lead = format!("codefast: {} ({})", head.name, head.at);
        clip_bytes(
            &match (doc, news) {
                (None, None) => return String::new(),
                (Some(d), Some(u)) => format!("{lead}: {d}\n{u}"),
                (Some(d), None) => format!("{lead}: {d}"),
                (None, Some(u)) => format!("{lead} {u}"),
            },
            NOTE_BYTES,
        )
    } else {
        symbol_note(&head, usage.as_deref(), &cli())
    };
    // Last of all, and only for one of the hooks that got this far for the same symbol.
    if !head.session.claim(&format!("s:{}", head.name)) {
        return String::new();
    }
    payload.reply(Event::PostToolUse, &note)
}

// ----- pre-tool-use -----

fn size_text(bytes: u64) -> String {
    if bytes >= 1 << 20 { format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64) } else { format!("{} KB", bytes.div_ceil(1024).max(1)) }
}

fn read_hint(place: &Place, path: &Path, session_id: Option<&str>) -> Option<String> {
    let rel = place.relative(path)?;
    if rel.is_empty() || rel.chars().any(char::is_control) {
        return None;
    }
    let ix = open_index(place)?;
    let file = ix.file_by_path(&rel)?;
    let rec = ix.file(file);
    // Without symbols there is no outline to offer.
    if rec.def_count < 2 {
        return None;
    }
    let md = std::fs::metadata(path).ok()?;
    let lines = if md.len() == rec.size && mtime_ns(&md) == rec.mtime_ns {
        rec.nlines as u64
    } else if md.len() <= 16 << 20 {
        crate::lang::count_lines(&std::fs::read(path).ok()?) as u64
    } else {
        return None;
    };
    if lines <= READ_LINES {
        return None;
    }
    let session = Session::open(place, session_id)?;
    let key = format!("f:{rel}");
    if session.seen(&key) || !session.claim(&key) {
        return None;
    }
    Some(read_note(&rel, lines, md.len(), rec.def_count as usize, &cli()))
}

/// The one line about a long file: its size and the command that prints its outline. The
/// pointer to single symbols is dropped when the command is so long that the line would
/// outgrow the budget of a note.
fn read_note(rel: &str, lines: u64, bytes: u64, symbols: usize, cli: &str) -> String {
    let outline = format!(
        "{rel} has {} lines ({}): `{cli} show {}` prints its outline ({} symbols with line ranges)",
        count(lines as usize),
        size_text(bytes),
        quote(rel),
        count(symbols)
    );
    let body = format!(", and `{cli} show NAME` one symbol's body.");
    if outline.len() + body.len() <= NOTE_BYTES { format!("{outline}{body}") } else { format!("{outline}.") }
}

fn pre_tool_use(passed: &Path, payload: &Payload, started: Instant) -> String {
    let cwd = payload.cwd.as_deref().map(PathBuf::from).filter(|d| d.is_absolute());
    let path: PathBuf = match payload.call() {
        Some(Call::Read { path, whole: true }) => {
            let p = PathBuf::from(path);
            match (p.is_absolute(), &cwd) {
                (true, _) => p,
                (false, Some(cwd)) => cwd.join(p),
                _ => return String::new(),
            }
        }
        Some(Call::Shell { command, dir }) => {
            let reads = shell::analyse(&command).reads;
            let [read] = &reads[..] else { return String::new() };
            let Some(base) = dir.map(PathBuf::from).filter(|d| d.is_absolute()).or(cwd) else { return String::new() };
            match locate(&base, &read.dir, &read.path) {
                Some(p) => p,
                None => return String::new(),
            }
        }
        _ => return String::new(),
    };
    let Some(place) = Place::find(passed, payload.cwd.as_deref()) else { return String::new() };
    let session_id = payload.audience();
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::Builder::new().spawn(move || {
        let _ = tx.send(read_hint(&place, &path, session_id.as_deref()));
    });
    if worker.is_err() {
        return String::new();
    }
    match rx.recv_timeout(HARD_DEADLINE.saturating_sub(started.elapsed())) {
        Ok(Some(hint)) => payload.reply(Event::PreToolUse, &hint),
        _ => String::new(),
    }
}

/// Handle one hook invocation; the result is what goes to stdout.
pub(super) fn run(passed: &Path, event: &str, input: &str) -> String {
    let started = Instant::now();
    let Some(event) = Event::parse(event) else { return String::new() };
    if event == Event::Refresh {
        refresh(passed);
        return String::new();
    }
    if disabled() || input.len() > MAX_PAYLOAD_BYTES {
        return String::new();
    }
    // Anything that is not a JSON object is not a hook payload (input that is not UTF-8 arrives
    // here as an empty string).
    let Some(payload) = Payload::parse(input) else { return String::new() };
    // A payload of another event, or a harness with no way to take context here: nothing to do.
    if payload.reply(event, "x").is_empty() {
        return String::new();
    }
    match event {
        Event::SessionStart => session_start(passed, &payload),
        Event::UserPromptSubmit => user_prompt(passed, &payload, started),
        Event::PostToolUse => post_tool_use(passed, &payload, started),
        Event::PreToolUse => pre_tool_use(passed, &payload, started),
        Event::Refresh => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_names_in_every_spelling() {
        for (given, want) in [
            ("session-start", Some(Event::SessionStart)),
            ("SessionStart", Some(Event::SessionStart)),
            ("sessionStart", Some(Event::SessionStart)),
            ("pre-tool-use", Some(Event::PreToolUse)),
            ("PreToolUse", Some(Event::PreToolUse)),
            ("post_tool_use", Some(Event::PostToolUse)),
            ("PostToolUseFailure", Some(Event::PostToolUse)),
            ("AfterTool", Some(Event::PostToolUse)),
            ("stop", None),
            ("", None),
            ("../../etc", None),
        ] {
            assert_eq!(Event::parse(given), want, "{given}");
        }
    }

    #[test]
    fn the_payload_scanner_steps_over_what_it_does_not_need() {
        let big = "x".repeat(200_000);
        let input = format!(
            r#" {{"tool_response": {{"stdout": "{big}", "nested": [[[{{"a": "}}]]\"]"}}]]]}}, "session_id": "s1", "cwd": "/w", "extra": [1, 2.5e3, true, null], "hook_event_name": "PostToolUse", "tool_name": "Bash", "tool_input": {{"command": "rg foo"}}}} "#
        );
        let p = Payload::parse(&input).unwrap();
        assert_eq!((p.session.as_deref(), p.cwd.as_deref(), p.tool.as_deref()), (Some("s1"), Some("/w"), Some("Bash")));
        assert_eq!(p.call(), Some(Call::Shell { command: "rg foo".into(), dir: None }));
        assert!(!p.cursor);
        for bad in [
            "",
            "[]",
            "{",
            "{\"a\"}",
            "{\"a\": }",
            "{\"a\": 1,",
            "{\"a\": 1,}",
            "{\"a\": \"unterminated}",
            "null",
            "{\"a\": [1, 2}",
            "{'a': 1}",
            "{\"a\": 1} trailing",
            "{}{}",
        ] {
            assert!(members(bad).is_none(), "{bad}");
        }
        for good in ["{}", " { } \n", "{\"a\": 1}", "{\"a\":{\"b\":[1,{\"c\":\"}\"}]},\"d\":null}\n"] {
            assert!(members(good).is_some(), "{good}");
        }
        // A deeply nested value neither recurses nor allocates.
        let deep = format!("{{\"x\": {}{}, \"tool_name\": \"Read\"}}", "[".repeat(500_000), "]".repeat(500_000));
        assert_eq!(Payload::parse(&deep).unwrap().tool.as_deref(), Some("Read"));
    }

    #[test]
    fn tool_calls_of_each_harness() {
        let call = |json: &str| Payload::parse(json).unwrap().call();
        // Claude Code
        assert_eq!(
            call(r#"{"tool_name": "Grep", "tool_input": {"pattern": "vfs_read", "path": "/r/fs", "output_mode": "content"}}"#),
            Some(Call::Search(Search { pattern: "vfs_read".into(), fixed: false, by_name: false, paths: vec!["/r/fs".into()], dir: Dir::Start }))
        );
        assert_eq!(call(r#"{"tool_name": "Read", "tool_input": {"file_path": "/r/a.c"}}"#), Some(Call::Read { path: "/r/a.c".into(), whole: true }));
        assert_eq!(
            call(r#"{"tool_name": "Read", "tool_input": {"file_path": "/r/a.c", "offset": 300, "limit": 80}}"#),
            Some(Call::Read { path: "/r/a.c".into(), whole: false })
        );
        assert_eq!(
            call(r#"{"tool_name": "Read", "tool_input": {"file_path": "/r/a.c", "limit": 2000}}"#),
            Some(Call::Read { path: "/r/a.c".into(), whole: true })
        );
        assert_eq!(
            call(r#"{"tool_name": "Read", "tool_input": {"file_path": "/r/a.c", "limit": 100}}"#),
            Some(Call::Read { path: "/r/a.c".into(), whole: false })
        );
        // Codex: a command line, or an argument vector
        assert_eq!(
            call(r#"{"tool_name": "Bash", "tool_input": {"command": ["bash", "-lc", "rg -n 'a b' ."]}}"#),
            Some(Call::Shell { command: "bash -lc 'rg -n '\\''a b'\\'' .'".into(), dir: None })
        );
        // Cursor and Gemini CLI
        assert_eq!(
            call(r#"{"tool_name": "Shell", "tool_input": {"command": "rg x", "working_directory": "/p"}}"#),
            Some(Call::Shell { command: "rg x".into(), dir: Some("/p".into()) })
        );
        assert!(matches!(call(r#"{"tool_name": "run_shell_command", "tool_input": {"command": "rg x"}}"#), Some(Call::Shell { .. })));
        // Tools that write files: the files they named
        assert_eq!(call(r#"{"tool_name": "Write", "tool_input": {"file_path": "/r/a.c", "content": "x"}}"#), Some(Call::Edit { paths: vec!["/r/a.c".into()] }));
        assert_eq!(
            call(r#"{"tool_name": "Edit", "tool_input": {"file_path": "/r/a.c", "old_string": "a", "new_string": "b"}}"#),
            Some(Call::Edit { paths: vec!["/r/a.c".into()] })
        );
        assert_eq!(
            call(r#"{"tool_name": "apply_patch", "tool_input": {"command": "*** Begin Patch\n*** Update File: src/a.c\n@@\n-x\n+y\n*** Add File: src/b.c\n+z\n*** End Patch"}}"#),
            Some(Call::Edit { paths: vec!["src/a.c".into(), "src/b.c".into()] })
        );
        // Tools the hooks have nothing to say about
        for other in [
            r#"{"tool_name": "WebFetch", "tool_input": {"url": "https://example.com"}}"#,
            r#"{"tool_name": "Bash", "tool_input": {"command": 7}}"#,
            r#"{"tool_name": "Bash"}"#,
            r#"{"tool_input": {"command": "rg x"}}"#,
        ] {
            assert_eq!(call(other), None, "{other}");
        }
    }

    #[test]
    fn replies_follow_the_harness_that_asked() {
        let payload = |json: &str| Payload::parse(json).unwrap();
        let nested = |reply: &str| serde_json::from_str::<Value>(reply).unwrap()["hookSpecificOutput"].clone();
        // Claude Code and Codex: the event name is echoed.
        let p = payload(r#"{"hook_event_name": "PostToolUse"}"#);
        assert_eq!(nested(&p.reply(Event::PostToolUse, "note")), json!({"hookEventName": "PostToolUse", "additionalContext": "note"}));
        let p = payload(r#"{"hook_event_name": "PostToolUseFailure"}"#);
        assert_eq!(nested(&p.reply(Event::PostToolUse, "note"))["hookEventName"], "PostToolUseFailure");
        assert_eq!(nested(&payload("{}").reply(Event::PreToolUse, "note"))["hookEventName"], "PreToolUse");
        // A payload of another event is never answered.
        assert_eq!(payload(r#"{"hook_event_name": "Stop"}"#).reply(Event::PostToolUse, "note"), "");
        assert_eq!(payload(r#"{"hook_event_name": "PreToolUse"}"#).reply(Event::PostToolUse, "note"), "");
        // Gemini CLI has no context channel before a tool call.
        assert_eq!(nested(&payload(r#"{"hook_event_name": "AfterTool"}"#).reply(Event::PostToolUse, "note"))["hookEventName"], "AfterTool");
        assert_eq!(payload(r#"{"hook_event_name": "BeforeTool"}"#).reply(Event::PreToolUse, "note"), "");
        // Cursor: flat output, and none before a tool call.
        let p = payload(r#"{"hook_event_name": "postToolUse", "cursor_version": "3.1"}"#);
        assert_eq!(serde_json::from_str::<Value>(&p.reply(Event::PostToolUse, "note")).unwrap(), json!({"additional_context": "note"}));
        assert_eq!(payload(r#"{"hook_event_name": "preToolUse", "cursor_version": "3.1"}"#).reply(Event::PreToolUse, "note"), "");
        // Nothing to say is nothing printed, and every reply is one line.
        assert_eq!(p.reply(Event::PostToolUse, ""), "");
        assert_eq!(payload("{}").reply(Event::SessionStart, "two\nlines").matches('\n').count(), 1);
    }

    #[test]
    fn notes_stay_inside_their_budget() {
        let session = Session { file: PathBuf::from("/nonexistent/codefast-test") };
        let long = format!("drivers/gpu/drm/some/long/path/file.c:1234 static int {}", "a_long_parameter_name, ".repeat(40));
        let head = Head { name: "a_function_with_quite_a_long_name".into(), at: "a.c:1".into(), line: long, shown: false, test: false, doc: None, callable: true, files: 1234, others: 3, session };
        let usage = format!("called by: {}", "another_caller_name, ".repeat(30));
        for usage in [Some(usage.as_str()), None] {
            let note = symbol_note(&head, usage, "codefast");
            assert!(note.len() <= NOTE_BYTES, "{} bytes: {note}", note.len());
            assert!(crate::output::est_tokens(&note) <= 150);
            assert!(
                note.starts_with("codefast: a_function_with_quite_a_long_name is defined at drivers/gpu/drm/some/long/path/file.c:1234 static int "),
                "{note}"
            );
            // The definition, how it is used, and that three other symbols share the name.
            assert!(note.ends_with("3 other symbols are named a_function_with_quite_a_long_name: `codefast find a_function_with_quite_a_long_name`"), "{note}");
            assert!(note.lines().count() == 3, "{note}");
        }
        // A command as long as an absolute path leaves less room, and never a longer note: the
        // pointers go first, the definition keeps at least a hundred bytes.
        let lead =
            "codefast: a_function_with_quite_a_long_name is defined at drivers/gpu/drm/some/long/path/file.c:1234 static int a_long_parameter_name, a_long_";
        for (repeat, mentions) in [(1usize, 2usize), (3, 1), (6, 1), (12, 0), (40, 0)] {
            let cli = "/a/very/long/install/prefix".repeat(repeat);
            for usage in [Some(usage.as_str()), None] {
                let note = symbol_note(&head, usage, &cli);
                assert!(note.len() <= NOTE_BYTES && note.lines().count() >= 2, "{} bytes with a {}-byte command: {note}", note.len(), cli.len());
                assert!(note.starts_with(lead), "{note}");
                if usage.is_none() {
                    assert_eq!(note.matches(cli.as_str()).count(), mentions, "{note}");
                }
            }
        }
        let short = Head {
            name: "f_x".into(),
            at: "a.c:1".into(),
            line: "a.c:1 int f_x(void)".into(),
            shown: false, test: false,
            doc: None,
            callable: false,
            files: 1,
            others: 0,
            session: Session { file: PathBuf::from("/nonexistent/x") },
        };
        assert_eq!(
            symbol_note(&short, None, "codefast"),
            "codefast: f_x is defined at a.c:1 int f_x(void)\nmentioned in 1 file; `codefast refs f_x` lists its uses"
        );
        // The sentence its author wrote stands between the definition and its use.
        let documented = Head { doc: Some("Fold x into the running sum".into()), session: Session { file: PathBuf::from("/nonexistent/y") }, name: "f_x".into(), at: "a.c:1".into(), line: "a.c:1 int f_x(void)".into(), shown: false, test: false, callable: true, files: 1, others: 0 };
        assert_eq!(
            symbol_note(&documented, Some("called by: main"), "codefast"),
            "codefast: f_x is defined at a.c:1 int f_x(void)\nFold x into the running sum\ncalled by: main"
        );
        assert_eq!(symbol_note(&short, None, &"/p".repeat(250)), "codefast: f_x is defined at a.c:1 int f_x(void)\nmentioned in 1 file");
        // The read hint keeps the outline command whatever its length, and drops the rest first.
        assert_eq!(
            read_note("src/big.c", 2250, 21_000, 450, "codefast"),
            "src/big.c has 2,250 lines (21 KB): `codefast show src/big.c` prints its outline (450 symbols with line ranges), and `codefast show NAME` one symbol's body."
        );
        let long = read_note("src/big.c", 2250, 21_000, 450, &"/p".repeat(100));
        assert!(long.ends_with(" show src/big.c` prints its outline (450 symbols with line ranges).") && long.len() < NOTE_BYTES, "{long}");
        assert_eq!(clip_bytes("héllo wörld", 8), "héll…");
    }

    #[test]
    fn a_key_is_claimed_once() {
        let dir = std::env::temp_dir().join(format!("codefast-claim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let session = Session { file: dir.join("sessions").join("0123456789abcdef") };
        assert!(!session.seen("s:vfs_read"));
        assert!(session.claim("s:vfs_read"), "the first claim wins, and creates the file");
        assert!(session.seen("s:vfs_read") && !session.seen("s:vfs") && !session.seen("s:vfs_read2"));
        assert!(!session.claim("s:vfs_read"), "the second one loses");
        assert!(session.claim("f:fs/read_write.c") && !session.claim("f:fs/read_write.c"));
        // A state file written by an earlier release has bare keys: they count.
        std::fs::write(&session.file, "s:old_name\nf:a b/c.c\n").unwrap();
        assert!(session.seen("s:old_name") && session.seen("f:a b/c.c") && !session.claim("s:old_name") && !session.claim("f:a b/c.c"));
        assert!(session.claim("s:new_name"));
        // Keys that could not be read back as one line are never recorded.
        for bad in ["", "s:a\nb", "s:a\tb", "s:a\rb"] {
            assert!(!session.claim(bad), "{bad:?}");
        }
        // Many threads at once: exactly one gets it.
        let session = std::sync::Arc::new(session);
        let winners: usize = (0..16)
            .map(|_| {
                let s = session.clone();
                std::thread::spawn(move || usize::from(s.claim("s:contended")))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .sum();
        assert_eq!(winners, 1);
        // A session that has been told too much is told nothing more.
        std::fs::write(&session.file, "x".repeat(SESSION_FILE_MAX as usize + 1)).unwrap();
        assert!(session.seen("s:anything") && !session.claim("s:anything"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(size_text(900), "1 KB");
        assert_eq!(size_text(48_000), "47 KB");
        assert_eq!(size_text(3 << 20), "3.0 MB");
        assert_eq!(quote("src/big.c"), "src/big.c");
        assert_eq!(quote("dir with space/it's.c"), "'dir with space/it'\\''s.c'");
    }
}
