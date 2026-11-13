//! Model Context Protocol server over stdio (newline-delimited JSON-RPC 2.0).
//!
//! The surface is deliberately small: five read-only tools whose output is the same plain text
//! the CLI prints. Tool definitions never change while the process lives and contain nothing
//! volatile, so they stay in the client's prompt cache.
//!
//! The server is dual-era, selected per request by how the client opens:
//!
//! * a request whose `params._meta` carries `io.modelcontextprotocol/protocolVersion` is served
//!   statelessly under revision 2026-07-28: `server/discover`, `resultType` on every result,
//!   `ttlMs` / `cacheScope` on list results, the server identity in the result's `_meta`;
//! * `initialize` (and any request without that metadata) is served under the handshake
//!   revisions 2025-11-25, 2025-06-18, 2025-03-26 and 2024-11-05. An `initialize` that does
//!   carry a well-formed stateless envelope is a stateless request for a method that revision
//!   does not have.
//!
//! Only protocol messages are ever written to stdout, the server never sends a request of its
//! own, and it exits when stdin closes.
//!
//! Which repository a call addresses is decided in [`Workspace`]: the `root` argument, then the
//! `--root` flag, `CODEFAST_ROOT`, `CLAUDE_PROJECT_DIR`, then the working directory when it lies
//! inside a git work tree. Whatever the source, a directory inside a repository means that
//! repository, as it does for the command line.

use crate::fresh::{self, OpenOptions};
use crate::output::{Out, clip, count};
use crate::query::{self, Common};
use crate::resolve::Resolver;
use anyhow::Result;
use serde_json::{Map, Value, json};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// Stateless revisions, served from per-request `_meta`.
const MODERN_VERSIONS: &[&str] = &["2026-07-28"];
/// Handshake revisions, newest first; `initialize` negotiates among these.
const LEGACY_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";
const META_SUBSCRIPTION_ID: &str = "io.modelcontextprotocol/subscriptionId";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;
const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

/// The tool list only changes with the binary, so clients may keep it for an hour.
const LIST_TTL_MS: u64 = 3_600_000;
/// One message may not exceed this many bytes; a longer line is skipped and reported.
const MAX_MESSAGE_BYTES: usize = 16 << 20;
/// Open `subscriptions/listen` streams one process keeps track of.
const MAX_SUBSCRIPTIONS: usize = 1024;

const DEFAULT_BUDGET: usize = 1500;
const MAX_BUDGET: usize = 8000;

const TOOL_NAMES: &[&str] = &["search", "show", "refs", "path", "grep"];

/// Sent with every request of a session, so every word is paid for on every request: what the
/// server is, where to start, and the two things about its answers that are not obvious.
const INSTRUCTIONS: &str = "codefast answers questions about this repository from a structural index. `search` is the entry point: give it a symbol name, a path or free text (a bug report, a stack trace) and it returns the definition with its body and every place that uses it, or the files the text points to. Use it before grep and file reads for anything about a symbol. Answers give `path:line` locations and say when they are incomplete. `refs` and `grep` leave out test code unless `tests` is true.";

/// Appended to the instructions when the process was started outside a repository.
const INSTRUCTIONS_NEED_ROOT: &str = " This server was started outside a repository: pass `root` (the absolute path of the repository) with every call.";

/// What to do about a missing root; ends every "no repository root" error.
const HOW_TO_NAME_A_ROOT: &str = "Pass `root` (the absolute path of the repository) with this call; or start the server with `--root DIR`, set CODEFAST_ROOT, or run it from inside a git work tree";

fn tools() -> Value {
    let root = json!({"type": "string", "description": "Repository path, only when the server asks for it."});
    let budget = json!({"type": "integer", "description": "Answer size in tokens (default 1500, max 8000)."});
    let path = json!({"type": "string", "description": "Only paths containing this text, or a glob such as drivers/net/**."});
    let tests = json!({"type": "boolean", "description": "Include test code (skipped by default)."});
    let hints = |title: &str| json!({"title": title, "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false});
    json!([
        {
            "name": "search",
            "description": "Entry point for code questions. Give a symbol name (`vfs_read`, `Owner.method`), a file path or `path:line`, or free text (a bug report, an error message, a stack trace). Returns the definition with its body and every place that calls or uses it, or for free text the files and functions it most likely means.",
            "inputSchema": {"type": "object", "properties": {
                "queries": {"type": "array", "items": {"type": "string"}, "description": "One or more queries, answered in turn."},
                "path": path, "budget": budget, "root": root
            }, "required": ["queries"]},
            "annotations": hints("Search code"),
            "_meta": {"anthropic/alwaysLoad": true}
        },
        {
            "name": "show",
            "description": "Print source precisely: a symbol's full body, a file's outline (signatures with line ranges), `path:line` (the enclosing symbol), or `path:start-end`. Cheaper than reading a whole file. Set `outline` for signatures only, `full` to lift the length cap. Set `context` before editing a function: adds its type, callee signatures, call sites and tests.",
            "inputSchema": {"type": "object", "properties": {
                "targets": {"type": "array", "items": {"type": "string"}, "description": "Symbols, files, `path:line` or `path:start-end`."},
                "outline": {"type": "boolean"}, "full": {"type": "boolean"}, "context": {"type": "boolean"}, "budget": budget, "root": root
            }, "required": ["targets"]},
            "annotations": hints("Show source")
        },
        {
            "name": "refs",
            "description": "Who uses a symbol. mode `refs`: every reference grouped by kind (calls, reads, writes, type uses) with source lines, plus text-only mentions. mode `callers`: call sites including dispatch through callback tables and function pointers; `depth` > 1 gives a caller tree. mode `callees`: what the function calls and where each callee is defined.",
            "inputSchema": {"type": "object", "properties": {
                "targets": {"type": "array", "items": {"type": "string"}, "description": "Symbols (`name`, `Owner.name`) or `path:line`."},
                "mode": {"type": "string", "enum": ["refs", "callers", "callees"], "description": "Default refs."},
                "depth": {"type": "integer", "description": "Caller tree depth (callers mode), default 1, max 8."},
                "tests": tests, "path": path, "budget": budget, "root": root
            }, "required": ["targets"]},
            "annotations": hints("References and call graph")
        },
        {
            "name": "path",
            "description": "Shortest call chain from one function to another, with the call site of every hop. Follows direct calls and dispatch through callback tables. Use it for \"how does execution get from A to B\".",
            "inputSchema": {"type": "object", "properties": {
                "from": {"type": "string"}, "to": {"type": "string"}, "budget": budget, "root": root
            }, "required": ["from", "to"]},
            "annotations": hints("Call path")
        },
        {
            "name": "grep",
            "description": "Regex search over all text in the repository, answered from a word index. Every match shows its line and the enclosing function. Use for strings, log messages and patterns; use `search` for symbols.",
            "inputSchema": {"type": "object", "properties": {
                "pattern": {"type": "string", "description": "Rust regex syntax."},
                "ignore_case": {"type": "boolean"}, "word": {"type": "boolean"}, "fixed": {"type": "boolean", "description": "Treat the pattern as a literal string."},
                "files_only": {"type": "boolean"}, "tests": tests, "path": path, "budget": budget, "root": root
            }, "required": ["pattern"]},
            "annotations": hints("Search text")
        }
    ])
}

// ----- tool arguments -----

/// A list of strings given as an array or as one string; `Err` names the offending key.
fn string_list(args: &Value, key: &str) -> Result<Vec<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(s)) => Ok(vec![s.clone()]),
        Some(Value::Array(a)) => {
            a.iter().map(|x| x.as_str().map(str::to_string).ok_or_else(|| anyhow::anyhow!("`{key}` must be an array of strings"))).collect()
        }
        Some(_) => anyhow::bail!("`{key}` must be an array of strings"),
    }
}

/// The required list argument of a tool, also accepted under its singular name.
fn required_list(args: &Value, key: &str, singular: &str) -> Result<Vec<String>> {
    let mut v = string_list(args, key)?;
    v.extend(string_list(args, singular)?);
    v.retain(|s| !s.trim().is_empty());
    if v.is_empty() {
        anyhow::bail!("`{key}` is required: an array with at least one string");
    }
    Ok(v)
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    match args.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Ok(s),
        Some(Value::String(_)) | None | Some(Value::Null) => anyhow::bail!("`{key}` is required"),
        Some(_) => anyhow::bail!("`{key}` must be a string"),
    }
}

/// Models sometimes send numbers and booleans as strings; accept both spellings.
fn integer(args: &Value, key: &str) -> Result<Option<u64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => match n.as_u64().or_else(|| n.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64)) {
            Some(v) => Ok(Some(v)),
            None => anyhow::bail!("`{key}` must be a non-negative integer"),
        },
        Some(Value::String(s)) => match s.trim().parse::<u64>() {
            Ok(v) => Ok(Some(v)),
            Err(_) => anyhow::bail!("`{key}` must be a non-negative integer"),
        },
        Some(_) => anyhow::bail!("`{key}` must be a non-negative integer"),
    }
}

fn flag(args: &Value, key: &str) -> Result<bool> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(Value::String(s)) if s == "true" || s == "false" => Ok(s == "true"),
        Some(_) => anyhow::bail!("`{key}` must be true or false"),
    }
}

/// The `budget` of a call in tokens, within the bounds the tool definitions state.
fn budget_of(args: &Value) -> Result<usize> {
    Ok(integer(args, "budget")?.map(|b| b.clamp(100, MAX_BUDGET as u64) as usize).unwrap_or(DEFAULT_BUDGET))
}

/// Hold an answer to its budget. The queries count what they print, but they also repeat the
/// question ("no symbol named `…`"), and an argument can be of any length. The bound is what
/// `budget` tokens come to at the 2.8 bytes per token the budget is estimated with, with room
/// for the header and footer lines that are never dropped, and it stays under the 40,000
/// characters at which the strictest client starts to cut answers on its own.
fn bounded(mut text: String, budget: usize) -> String {
    let limit = (budget * 5 + 2048).min(40_000);
    if text.len() <= limit {
        return text;
    }
    let mut cut = limit - 200;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let dropped = text.len() - cut;
    text.truncate(cut);
    text.push_str(&format!(
        "\n… answer cut: {} more bytes did not fit the budget of {budget} tokens. This happens when a very long argument is repeated in the answer: shorten it.\n",
        count(dropped)
    ));
    text
}

/// Run one tool against the repository at `root` and return its text.
pub fn call_tool(root: &Path, name: &str, args: &Value) -> Result<String> {
    // Validate the arguments before opening the index, so that a malformed call is answered at once.
    let budget = budget_of(args)?;
    let common = Common { paths: string_list(args, "path")?, budget, tests: flag(args, "tests")?, ..Default::default() };
    enum Call {
        Search(Vec<String>),
        Show(Vec<String>, query::show::ShowOpts),
        Refs(Vec<String>),
        Context(Vec<String>),
        Callers(Vec<String>, u32),
        Callees(Vec<String>),
        Path(String, String),
        Grep(String, query::grep::GrepOpts),
    }
    let call = match name {
        "search" => Call::Search(required_list(args, "queries", "query")?),
        "show" => {
            let opts = query::show::ShowOpts { full: flag(args, "full")?, outline: flag(args, "outline")?, ..Default::default() };
            let targets = required_list(args, "targets", "target")?;
            if flag(args, "context")? { Call::Context(targets) } else { Call::Show(targets, opts) }
        }
        "refs" => {
            let targets = required_list(args, "targets", "target")?;
            let mode = match args.get("mode") {
                None | Some(Value::Null) => "refs",
                Some(Value::String(m)) => m.as_str(),
                Some(_) => anyhow::bail!("`mode` must be one of refs, callers, callees"),
            };
            match mode {
                "refs" => Call::Refs(targets),
                "callers" => Call::Callers(targets, integer(args, "depth")?.unwrap_or(1).clamp(1, 8) as u32),
                "callees" => Call::Callees(targets),
                other => anyhow::bail!("unknown mode `{}`: use refs, callers or callees", clip(other, 40)),
            }
        }
        "path" => Call::Path(required_str(args, "from")?.to_string(), required_str(args, "to")?.to_string()),
        "grep" => {
            let opts = query::grep::GrepOpts {
                ignore_case: flag(args, "ignore_case")?,
                word: flag(args, "word")?,
                fixed: flag(args, "fixed")?,
                files_only: flag(args, "files_only")?,
                ..Default::default()
            };
            Call::Grep(required_str(args, "pattern")?.to_string(), opts)
        }
        other => anyhow::bail!("unknown tool `{other}`"),
    };

    let ix = fresh::open(root, &OpenOptions { quiet: true, ..Default::default() })?;
    let rs = Resolver::new(&ix);
    let mut out = Out::new(budget);
    match call {
        Call::Search(queries) => {
            // Two wordings of one question get one answer: the second is not printed again.
            let mut answers: Vec<String> = Vec::new();
            for q in &queries {
                let mut one = Out::new(out.remaining().min(budget));
                query::search::search(&ix, &rs, q, &common, &query::search::SearchOpts::default(), &mut one);
                let text = one.finish();
                if answers.contains(&text) {
                    continue;
                }
                if !answers.is_empty() {
                    out.push("");
                }
                out.push(text.trim_end());
                answers.push(text);
            }
        }
        Call::Show(targets, opts) => query::show::show(&ix, &rs, &targets, &common, &opts, &mut out),
        Call::Context(targets) => query::context::context(&ix, &rs, &targets, &common, &mut out),
        Call::Refs(targets) => query::refs::refs(&ix, &rs, &targets, &common, &query::refs::RefsOpts::default(), &mut out),
        Call::Callers(targets, depth) => query::refs::callers(&ix, &rs, &targets, &common, depth, &mut out),
        Call::Callees(targets) => query::refs::callees(&ix, &rs, &targets, &common, false, &mut out),
        Call::Path(from, to) => query::refs::path(&ix, &rs, &from, &to, &common, 12, &mut out),
        Call::Grep(pattern, opts) => {
            query::grep::grep(&ix, &pattern, &common, &opts, &mut out);
            // The one failure a query reports as text: a pattern that does not compile is a
            // mistake of the caller, and the result has to say so.
            let text = out.finish();
            if text.starts_with("invalid pattern:") {
                anyhow::bail!("{}", text.trim_end());
            }
            return Ok(bounded(text, budget));
        }
    }
    Ok(bounded(out.finish(), budget))
}

// ----- workspace root -----

/// A configured value that can name a directory: not empty and not a placeholder the harness
/// left unexpanded (`${workspaceFolder}`, `${CLAUDE_PROJECT_DIR}`).
fn usable(value: &std::ffi::OsStr) -> Option<PathBuf> {
    let s = value.to_string_lossy();
    if s.trim().is_empty() || s.contains("${") {
        return None;
    }
    Some(PathBuf::from(value))
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

/// Nearest directory at or above `dir` that is a git work tree or already holds an index.
pub(crate) fn work_tree_of(dir: &Path) -> Option<PathBuf> {
    let mut d = Some(dir);
    while let Some(p) = d {
        if p.join(".git").exists() || p.join(crate::build::INDEX_DIR).is_dir() {
            return Some(p.to_path_buf());
        }
        d = p.parent();
    }
    None
}

/// Directories a harness installed this plugin into. A server started there (the Agent Plugins
/// default working directory) must not mistake the plugin's own checkout for the user's project.
fn plugin_roots() -> Vec<PathBuf> {
    ["CODEFAST_PLUGIN_ROOT", "PLUGIN_ROOT", "CLAUDE_PLUGIN_ROOT", "CURSOR_PLUGIN_ROOT", "COPILOT_PLUGIN_ROOT", "DROID_PLUGIN_ROOT"]
        .iter()
        .filter_map(std::env::var_os)
        .filter_map(|v| usable(&v))
        .filter_map(|p| p.canonicalize().ok())
        .collect()
}

/// The user's home directory, if the environment names one that exists.
fn home_dir() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"].iter().filter_map(std::env::var_os).filter_map(|v| usable(&v)).find_map(|p| p.canonicalize().ok())
}

/// Whether `dir` is the home directory or one of the directories above it. Indexing such a
/// place means walking everything the user owns, so it is never done on a harness's say-so.
fn holds_home(dir: &Path) -> bool {
    home_dir().is_some_and(|home| home.starts_with(dir))
}

/// How this process learns which repository a tool call is about.
struct Workspace {
    /// The root to use when a call passes none, or what is missing.
    default: std::result::Result<PathBuf, String>,
    /// The default root is not the directory the session was started in but a directory above
    /// it, so the paths in answers do not resolve from where the agent stands. The instructions
    /// then say what they are relative to.
    above_session: bool,
}

impl Workspace {
    fn missing(why: String) -> Workspace {
        Workspace { default: Err(format!("no repository root: {why}. {HOW_TO_NAME_A_ROOT}.")), above_session: false }
    }

    /// Resolution order after the `root` tool argument: the `--root` flag, `CODEFAST_ROOT`,
    /// `CLAUDE_PROJECT_DIR`, then the working directory when it is inside a git work tree.
    /// `passed` is what the caller of [`serve`] resolved; the command line only substitutes the
    /// working directory for a missing flag, which is not good enough here.
    fn detect(passed: &Path) -> Workspace {
        let env = |k: &str| std::env::var_os(k).and_then(|v| usable(&v));
        let cwd = std::env::current_dir().ok();
        if let Some(r) = root_flag().and_then(|v| usable(&v)) {
            return Workspace::configured(r, "--root");
        }
        // A library caller may hand over a root that is neither the flag nor the environment.
        let cli_fallback = cwd.as_deref().map(|c| work_tree_of(c).unwrap_or_else(|| c.to_path_buf()));
        let from_env = env("CODEFAST_ROOT");
        if let Some(p) = usable(passed.as_os_str())
            && Some(&p) != cli_fallback.as_ref()
            && Some(&p) != from_env.as_ref()
            && p != Path::new(".")
        {
            return Workspace::configured(p, "the caller");
        }
        if let Some(r) = from_env {
            return Workspace::configured(r, "CODEFAST_ROOT");
        }
        if let Some(r) = env("CLAUDE_PROJECT_DIR") {
            return Workspace::configured(r, "CLAUDE_PROJECT_DIR");
        }
        let Some(cwd) = cwd else {
            return Workspace::missing("the working directory is not readable".to_string());
        };
        match work_tree_of(&cwd) {
            Some(top) => {
                let real = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
                if plugin_roots().iter().any(|p| real.starts_with(p)) {
                    Workspace::missing(format!("the server was started in its own plugin directory ({}), not in a project", cwd.display()))
                } else {
                    let above_session = top != cwd;
                    Workspace { default: Ok(top), above_session }
                }
            }
            None => Workspace::missing(format!("the working directory {} is not inside a git work tree", cwd.display())),
        }
    }

    /// A root named by the flag or the environment. Harnesses fill these in with the directory
    /// a session was started in (`CLAUDE_PROJECT_DIR`, `--root ${workspacePath}`), which is not
    /// always the top of the repository and is sometimes the home directory:
    ///
    /// * inside a git work tree or an indexed directory it means that tree, exactly as the
    ///   command line run from there resolves it, so that a subdirectory never gets an index of
    ///   its own (one that would hide the rest of the repository, also from the command line);
    /// * outside any, it is a project without git and is served as it is;
    /// * unless it is the home directory or lies above it, which nobody means to index.
    fn configured(given: PathBuf, source: &str) -> Workspace {
        // A directory that does not exist is reported, by name, by each call that needs it.
        let Ok(real) = given.canonicalize() else {
            return Workspace { default: Ok(given), above_session: false };
        };
        let dir = match real.parent() {
            Some(parent) if real.is_file() => parent.to_path_buf(),
            _ => real,
        };
        match work_tree_of(&dir) {
            // A tree that starts at or above the home directory is only meant when it was named
            // itself: a project below a home directory kept in git is still its own project.
            Some(top) if top == dir || !holds_home(&top) => {
                let above_session = top != dir;
                Workspace { default: Ok(top), above_session }
            }
            _ if holds_home(&dir) => {
                let what = if home_dir().as_deref() == Some(dir.as_path()) { "the home directory" } else { "a directory above the home directory" };
                Workspace::missing(format!(
                    "{source} names {}, {what}, which is not a repository and is not indexed as one (run `codefast index` there once if that is what you want)",
                    dir.display()
                ))
            }
            _ => Workspace { default: Ok(dir), above_session: false },
        }
    }

    /// The repository a call addresses: its `root` argument, else the default.
    fn resolve(&self, args: &Value) -> std::result::Result<PathBuf, String> {
        match args.get("root") {
            None | Some(Value::Null) => self.default.clone(),
            Some(Value::String(s)) if s.trim().is_empty() => self.default.clone(),
            Some(Value::String(s)) => root_argument(s.trim()),
            Some(_) => Err("`root` must be a string: the absolute path of the repository".to_string()),
        }
    }
}

/// The path of a `file://` URI: the part after the scheme (and an empty or `localhost`
/// authority), with percent escapes decoded. `None` when `s` is not such a URI.
fn file_uri_path(s: &str) -> Option<String> {
    let rest = s.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").filter(|r| r.starts_with('/')).unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
        match (bytes[i], bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(hi), Some(lo)) => {
                out.push(hi * 16 + lo);
                i += 3;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// Check a `root` given by the model. It must name a place inside a git work tree or an indexed
/// directory, so that a mistaken path can never start an index of a home directory.
fn root_argument(s: &str) -> std::result::Result<PathBuf, String> {
    let from_uri = file_uri_path(s);
    let s = from_uri.as_deref().unwrap_or(s);
    let p = match s.strip_prefix("~/") {
        Some(rest) => match std::env::var_os("HOME") {
            Some(h) => PathBuf::from(h).join(rest),
            None => PathBuf::from(s),
        },
        None => PathBuf::from(s),
    };
    if !p.is_absolute() {
        return Err(format!("`root` must be an absolute path, got `{}`", clip(s, 200)));
    }
    let dir = if p.is_file() { p.parent().unwrap_or(&p).to_path_buf() } else { p };
    if !dir.is_dir() {
        return Err(format!("`root` is not a directory: {}", clip(&dir.to_string_lossy(), 400)));
    }
    // Resolve `..` and links first: the search for the enclosing tree walks up by name.
    let dir = dir.canonicalize().unwrap_or(dir);
    work_tree_of(&dir).ok_or_else(|| {
        format!(
            "`root` {} is not inside a git work tree and has no codefast index. Pass the repository's root directory, or run `codefast index` there once.",
            dir.display()
        )
    })
}

// ----- JSON-RPC plumbing -----

struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError { code, message: message.into(), data: None }
    }

    fn not_supported(what: &str) -> RpcError {
        RpcError {
            code: METHOD_NOT_FOUND,
            message: format!("{what} not supported"),
            data: Some(json!({"reason": format!("codefast does not offer the {} capability", what.to_lowercase())})),
        }
    }

    /// The error response; `id` is left out when the request's id could not be read.
    fn response(self, id: Option<&Value>) -> Value {
        let mut error = Map::new();
        error.insert("code".into(), json!(self.code));
        error.insert("message".into(), json!(self.message));
        if let Some(d) = self.data {
            error.insert("data".into(), d);
        }
        let mut msg = Map::new();
        msg.insert("jsonrpc".into(), json!("2.0"));
        if let Some(id) = id {
            msg.insert("id".into(), id.clone());
        }
        msg.insert("error".into(), Value::Object(error));
        Value::Object(msg)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Era {
    /// Stateless, per-request metadata (2026-07-28).
    Modern,
    /// `initialize` handshake (2025-11-25 and earlier).
    Legacy,
}

fn server_info() -> Value {
    json!({"name": "codefast", "version": env!("CARGO_PKG_VERSION")})
}

fn capabilities() -> Value {
    json!({"tools": {"listChanged": false}})
}

/// Wrap a result body for the era: modern results carry `resultType` and the server identity.
fn result(era: Era, id: &Value, mut body: Map<String, Value>) -> Value {
    if era == Era::Modern {
        body.insert("resultType".into(), json!("complete"));
        let meta = body.entry("_meta").or_insert_with(|| json!({}));
        if let Some(m) = meta.as_object_mut() {
            m.insert(META_SERVER_INFO.into(), server_info());
        }
    }
    json!({"jsonrpc": "2.0", "id": id, "result": body})
}

fn object(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn tool_result(text: String, is_error: bool) -> Map<String, Value> {
    object(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
}

struct Server<W: Write> {
    out: W,
    workspace: Workspace,
    /// Ids of open `subscriptions/listen` streams, closed gracefully at shutdown.
    subscriptions: Vec<Value>,
    /// Set when the client stopped reading; the loop ends.
    closed: bool,
}

impl<W: Write> Server<W> {
    fn send(&mut self, msg: &Value) {
        // `to_string` escapes every control character, so a message is always exactly one line.
        let mut line = msg.to_string();
        line.push('\n');
        if self.out.write_all(line.as_bytes()).and_then(|_| self.out.flush()).is_err() {
            self.closed = true;
        }
    }

    fn instructions(&self) -> String {
        match &self.workspace.default {
            Err(_) => format!("{INSTRUCTIONS}{INSTRUCTIONS_NEED_ROOT}"),
            Ok(root) if self.workspace.above_session => format!("{INSTRUCTIONS} Paths in answers are relative to the repository root, {}.", root.display()),
            Ok(_) => INSTRUCTIONS.to_string(),
        }
    }

    /// Whether the instructions carry a path of this machine; such a result is not for sharing.
    fn instructions_are_local(&self) -> bool {
        self.workspace.default.is_ok() && self.workspace.above_session
    }

    /// Handle one decoded line: a message, or (handshake revision 2025-03-26) a batch of them.
    fn handle_line(&mut self, msg: Value) {
        match msg {
            Value::Array(batch) => {
                if batch.is_empty() {
                    self.send(&RpcError::new(INVALID_REQUEST, "Invalid request: empty batch").response(None));
                    return;
                }
                let replies: Vec<Value> = batch.into_iter().filter_map(|m| self.handle_message(m)).collect();
                if !replies.is_empty() {
                    self.send(&Value::Array(replies));
                }
            }
            other => {
                if let Some(reply) = self.handle_message(other) {
                    self.send(&reply);
                }
            }
        }
    }

    /// Handle one JSON-RPC message; the return value is the response, if one is due.
    fn handle_message(&mut self, msg: Value) -> Option<Value> {
        let Value::Object(msg) = msg else {
            return Some(RpcError::new(INVALID_REQUEST, "Invalid request: a message must be a JSON object").response(None));
        };
        // Ids are strings or numbers; anything else cannot be echoed back.
        let id = match msg.get("id") {
            None => None,
            Some(v) if v.is_string() || v.is_number() => Some(v.clone()),
            Some(_) => return Some(RpcError::new(INVALID_REQUEST, "Invalid request: `id` must be a string or a number").response(None)),
        };
        if msg.get("jsonrpc").is_some_and(|v| v != "2.0") {
            return Some(RpcError::new(INVALID_REQUEST, "Invalid request: `jsonrpc` must be \"2.0\"").response(id.as_ref()));
        }
        let Some(method) = msg.get("method").and_then(|m| m.as_str()) else {
            // A response to a request this server never sent is dropped; anything else is malformed.
            if msg.contains_key("result") || msg.contains_key("error") {
                return None;
            }
            return Some(RpcError::new(INVALID_REQUEST, "Invalid request: `method` is missing").response(id.as_ref()));
        };
        let empty = Map::new();
        let params = match msg.get("params") {
            None | Some(Value::Null) => &empty,
            Some(Value::Object(p)) => p,
            Some(_) => {
                // Notifications are never answered, not even malformed ones.
                return id.map(|id| RpcError::new(INVALID_PARAMS, "Invalid params: `params` must be an object").response(Some(&id)));
            }
        };
        let Some(id) = id else {
            self.notification(method, params);
            return None;
        };
        match self.request(method, params, &id) {
            Ok(reply) => reply,
            Err(e) => Some(e.response(Some(&id))),
        }
    }

    fn notification(&mut self, method: &str, params: &Map<String, Value>) {
        // The only notification with an effect: cancelling a `subscriptions/listen` stream.
        // Requests are answered in order, so any other cancellation arrives after the fact.
        if method == "notifications/cancelled"
            && let Some(target) = params.get("requestId")
        {
            self.subscriptions.retain(|s| s != target);
        }
    }

    fn request(&mut self, method: &str, params: &Map<String, Value>, id: &Value) -> std::result::Result<Option<Value>, RpcError> {
        let meta = params.get("_meta").and_then(|m| m.as_object());
        let has_capabilities = meta.and_then(|m| m.get(META_CLIENT_CAPABILITIES)).is_some_and(|c| c.is_object());
        if method == "initialize" {
            // The legacy handshake, unless the request carries a well-formed envelope of a
            // stateless revision: that makes it a request of that revision, in which no such
            // method exists. Revisions are ISO dates, so they compare as text.
            let stateless = meta.and_then(|m| m.get(META_PROTOCOL_VERSION)).and_then(|v| v.as_str()).is_some_and(|v| v >= MODERN_VERSIONS[0]);
            if !(stateless && has_capabilities) {
                return Ok(Some(self.initialize(params, id)));
            }
        }
        let era = match meta.and_then(|m| m.get(META_PROTOCOL_VERSION)) {
            Some(version) => {
                let requested = version
                    .as_str()
                    .ok_or_else(|| RpcError::new(INVALID_PARAMS, format!("Invalid params: _meta[\"{META_PROTOCOL_VERSION}\"] must be a string")))?;
                if !MODERN_VERSIONS.contains(&requested) {
                    return Err(RpcError {
                        code: UNSUPPORTED_PROTOCOL_VERSION,
                        message: "Unsupported protocol version".to_string(),
                        data: Some(json!({"supported": MODERN_VERSIONS, "requested": clip(requested, 100)})),
                    });
                }
                if !has_capabilities {
                    return Err(RpcError::new(INVALID_PARAMS, format!("Invalid params: _meta[\"{META_CLIENT_CAPABILITIES}\"] is required")));
                }
                Era::Modern
            }
            None => Era::Legacy,
        };
        let body = match (method, era) {
            ("server/discover", Era::Modern) => object(json!({
                "supportedVersions": MODERN_VERSIONS,
                "capabilities": capabilities(),
                "instructions": self.instructions(),
                "ttlMs": LIST_TTL_MS,
                "cacheScope": if self.instructions_are_local() { "private" } else { "public" }
            })),
            ("server/discover", Era::Legacy) => {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    format!("Invalid params: server/discover needs _meta[\"{META_PROTOCOL_VERSION}\"] (supported: {})", MODERN_VERSIONS.join(", ")),
                ));
            }
            ("tools/list", _) => {
                if params.get("cursor").is_some_and(|c| !c.is_null()) {
                    return Err(RpcError::new(INVALID_PARAMS, "Invalid cursor: the tool list has a single page"));
                }
                let mut body = object(json!({"tools": tools()}));
                if era == Era::Modern {
                    body.insert("ttlMs".into(), json!(LIST_TTL_MS));
                    body.insert("cacheScope".into(), json!("public"));
                }
                body
            }
            ("tools/call", _) => self.tools_call(params)?,
            ("subscriptions/listen", Era::Modern) => {
                if !params.get("notifications").is_some_and(|n| n.is_object()) {
                    return Err(RpcError::new(INVALID_PARAMS, "Invalid params: `notifications` is required"));
                }
                // An id opens one stream however often it is sent, and the number of streams
                // is bounded, so that no client can grow this process without limit.
                if !self.subscriptions.contains(id) {
                    if self.subscriptions.len() >= MAX_SUBSCRIPTIONS {
                        return Err(RpcError::new(INTERNAL_ERROR, "Subscription limit reached"));
                    }
                    self.subscriptions.push(id.clone());
                }
                // Nothing here ever changes, so the acknowledged filter is empty. The stream stays
                // open, as the pattern requires, until the client cancels it or stdin closes.
                self.send(&json!({"jsonrpc": "2.0", "method": "notifications/subscriptions/acknowledged", "params": {"_meta": {META_SUBSCRIPTION_ID: id}, "notifications": {}}}));
                return Ok(None);
            }
            ("ping", Era::Legacy) => Map::new(),
            // Older clients ask for these without checking the advertised capabilities.
            ("resources/list", Era::Legacy) => object(json!({"resources": []})),
            ("resources/templates/list", Era::Legacy) => object(json!({"resourceTemplates": []})),
            ("prompts/list", Era::Legacy) => object(json!({"prompts": []})),
            (m, _) if m.starts_with("resources/") => return Err(RpcError::not_supported("Resources")),
            (m, _) if m.starts_with("prompts/") => return Err(RpcError::not_supported("Prompts")),
            (m, _) => return Err(RpcError::new(METHOD_NOT_FOUND, format!("Method not found: {}", clip(m, 120)))),
        };
        Ok(Some(result(era, id, body)))
    }

    /// The handshake of the legacy revisions. The reply names the requested revision when this
    /// server speaks it, and otherwise the newest handshake revision it does speak.
    fn initialize(&mut self, params: &Map<String, Value>, id: &Value) -> Value {
        let requested = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
        let version = if LEGACY_VERSIONS.contains(&requested) { requested } else { LEGACY_VERSIONS[0] };
        let body = object(json!({
            "protocolVersion": version,
            "capabilities": capabilities(),
            "serverInfo": server_info(),
            "instructions": self.instructions()
        }));
        result(Era::Legacy, id, body)
    }

    /// `tools/call`. Protocol errors (unknown tool, malformed request) are JSON-RPC errors;
    /// everything the model can correct comes back as a result with `isError: true`.
    fn tools_call(&mut self, params: &Map<String, Value>) -> std::result::Result<Map<String, Value>, RpcError> {
        let name = params.get("name").and_then(|n| n.as_str()).ok_or_else(|| RpcError::new(INVALID_PARAMS, "Invalid params: `name` is required"))?;
        if !TOOL_NAMES.contains(&name) {
            return Err(RpcError::new(INVALID_PARAMS, format!("Unknown tool: {}", clip(name, 120))));
        }
        let args = match params.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(a @ Value::Object(_)) => a.clone(),
            Some(_) => return Err(RpcError::new(INVALID_PARAMS, format!("Invalid arguments for tool {name}: `arguments` must be an object"))),
        };
        // An error repeats what was wrong, so it is held to the budget like any other answer.
        let failed = |why: String| tool_result(bounded(format!("codefast: {why}"), budget_of(&args).unwrap_or(DEFAULT_BUDGET)), true);
        let root = match self.workspace.resolve(&args) {
            Ok(r) => r,
            Err(why) => return Ok(failed(why)),
        };
        // A panic inside a query must not take the server down where the build can unwind.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call_tool(&root, name, &args)));
        Ok(match outcome {
            Ok(Ok(text)) if text.is_empty() => tool_result("no results".to_string(), false),
            Ok(Ok(text)) => tool_result(text, false),
            Ok(Err(e)) => failed(format!("{e:#}")),
            Err(_) => failed(format!("internal error while running `{name}`; try a narrower query")),
        })
    }

    /// End every open subscription with its completion result.
    fn shutdown(&mut self) {
        for id in std::mem::take(&mut self.subscriptions) {
            let body = object(json!({"_meta": {META_SUBSCRIPTION_ID: id}}));
            let reply = result(Era::Modern, &id, body);
            self.send(&reply);
        }
    }
}

enum Line {
    Message,
    /// Longer than [`MAX_MESSAGE_BYTES`]; consumed up to its newline and dropped.
    Oversized,
    Eof,
}

/// Read one newline-terminated message into `buf` without ever holding more than the cap.
fn read_line(r: &mut impl BufRead, buf: &mut Vec<u8>) -> std::io::Result<Line> {
    buf.clear();
    let mut oversized = false;
    let mut any = false;
    loop {
        let avail = match r.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if avail.is_empty() {
            return Ok(match (any, oversized) {
                (false, _) => Line::Eof,
                (true, true) => Line::Oversized,
                (true, false) => Line::Message,
            });
        }
        any = true;
        let (chunk, used, done) = match memchr::memchr(b'\n', avail) {
            Some(i) => (&avail[..i], i + 1, true),
            None => (avail, avail.len(), false),
        };
        if !oversized {
            if buf.len() + chunk.len() > MAX_MESSAGE_BYTES {
                oversized = true;
                buf.clear();
            } else {
                buf.extend_from_slice(chunk);
            }
        }
        r.consume(used);
        if done {
            return Ok(if oversized { Line::Oversized } else { Line::Message });
        }
    }
}

/// The stream protocol messages go to. On Unix the real stdout is moved to a private descriptor
/// and descriptor 1 is pointed at stderr, so that a stray print anywhere in the process can
/// never reach the client as a malformed message.
#[cfg(unix)]
fn protocol_stream() -> Box<dyn Write> {
    use std::os::fd::{AsFd, AsRawFd};
    unsafe extern "C" {
        fn dup2(src: i32, dst: i32) -> i32;
    }
    match std::io::stdout().as_fd().try_clone_to_owned() {
        Ok(private) => {
            // SAFETY: dup2 on two descriptors this process owns; it has no memory effects.
            unsafe { dup2(std::io::stderr().as_raw_fd(), std::io::stdout().as_raw_fd()) };
            Box::new(std::fs::File::from(private))
        }
        Err(_) => Box::new(std::io::stdout()),
    }
}

#[cfg(not(unix))]
fn protocol_stream() -> Box<dyn Write> {
    Box::new(std::io::stdout())
}

/// Serve MCP on stdin / stdout until the client closes the stream. `root` is the repository
/// the command line resolved; see [`Workspace::detect`] for how it is weighed.
pub fn serve(root: PathBuf) -> Result<()> {
    let mut server = Server { out: protocol_stream(), workspace: Workspace::detect(&root), subscriptions: Vec::new(), closed: false };
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    while !server.closed {
        match read_line(&mut input, &mut buf) {
            Ok(Line::Eof) | Err(_) => break,
            Ok(Line::Oversized) => {
                let limit = MAX_MESSAGE_BYTES >> 20;
                server.send(&RpcError::new(INVALID_REQUEST, format!("Invalid request: message longer than {limit} MiB")).response(None));
            }
            Ok(Line::Message) => {
                if buf.iter().all(|b| b.is_ascii_whitespace()) {
                    continue;
                }
                match serde_json::from_slice::<Value>(&buf) {
                    Ok(msg) => server.handle_line(msg),
                    Err(_) => server.send(&RpcError::new(PARSE_ERROR, "Parse error: invalid JSON").response(None)),
                }
            }
        }
    }
    server.shutdown();
    Ok(())
}
