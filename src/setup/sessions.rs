//! `codefast doctor sessions`: where the tokens of this project's sessions went, read from the
//! harness's own transcripts on this machine, and which habits cost the most.
//!
//! Most of what a session costs goes to a few habits that nothing shows: a manual compaction
//! run as routine, which reads the whole conversation again; coming back after the prompt cache
//! has expired, or switching models mid-task, after which the next request sends the
//! conversation again at the price of new input; a large start (instruction files, plugins,
//! tool lists) that goes with every request; thinking, which is billed as output. This report
//! puts numbers from the user's own sessions on each. Nothing is run, nothing is changed,
//! nothing leaves the machine and no language model is called.
//!
//! The unit is the weighted token: one uncached input token of the request's own model. A
//! cache read weighs 0.1, a cache write 1.25 (2 for the one-hour cache), an output token 5.
//! Dollars are not printed: prices differ by model and change.
//!
//! Two harnesses are read, each through a [`Transcripts`]: Claude Code and Codex. Codex
//! sessions are weighed by the same convention, although its providers price differently.

use super::harness::Ctx;
use crate::output::count;
use anyhow::{Result, bail};
use rayon::prelude::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// What one token weighs, in uncached input tokens of the same model.
pub const INPUT: f64 = 1.0;
pub const CACHE_READ: f64 = 0.1;
pub const CACHE_WRITE_5M: f64 = 1.25;
pub const CACHE_WRITE_1H: f64 = 2.0;
pub const OUTPUT: f64 = 5.0;
/// About how many bytes of a tool's output make one token.
const BYTES_PER_TOKEN: f64 = 3.5;
/// A request reads less than this share of the previous context from the cache: it started cold.
const COLD_SHARE: f64 = 0.2;
/// Below this many tokens of context a cold start is not worth naming.
const COLD_FROM: u64 = 20_000;
/// The one-hour cache, with five minutes to spare, and the five-minute cache.
const IDLE_HOUR_MS: i64 = 55 * 60 * 1000;
const IDLE_FIVE_MS: i64 = 5 * 60 * 1000;
/// A manual compaction of a conversation shorter than this is early.
const EARLY_COMPACTION: u64 = 100_000;
/// Context above this many tokens is carried at a cost a compaction could have saved
/// (`--carry N`).
pub const CARRY: u64 = 200_000;
/// What a compaction leaves when a session had none to learn it from.
const POST_TOKENS: u64 = 25_000;
/// A finding under this share of the total is left out.
const WORTH_NAMING: f64 = 0.01;

const USAGE: &str = "Where the tokens of this project's sessions went, read from the harnesses' own transcripts on
this machine (Claude Code and Codex, one report each), and which habits cost the most.
Nothing is run, changed or sent.

Usage: codefast doctor sessions [--days N] [--session ID] [--carry N] [--all] [--json]
  --days N       the sessions whose transcript changed in the last N days (default 14)
  --session ID   one session, named by the start of its id, with each finding broken down
  --carry N      the context, in tokens, above which carrying it is counted (default 200000)
  --all          every project with sessions of either harness: one line each, then the totals
  --json         the same numbers as JSON
";

// ----- what a transcript holds -----

/// The token counts of one request. A count the transcript does not hold is `None`: unknown,
/// which is not zero.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub input: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub write_5m: Option<u64>,
    pub write_1h: Option<u64>,
    pub output: Option<u64>,
    pub thinking: Option<u64>,
}

/// Weighted tokens of one request or of many, by what they paid for.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Split {
    pub cache_read: f64,
    pub cache_write: f64,
    pub input: f64,
    pub output: f64,
}

impl Split {
    pub fn total(&self) -> f64 {
        self.cache_read + self.cache_write + self.input + self.output
    }

    fn add(&mut self, o: &Split) {
        self.cache_read += o.cache_read;
        self.cache_write += o.cache_write;
        self.input += o.input;
        self.output += o.output;
    }
}

impl Usage {
    /// Everything the request sent: `input + cache_read + cache_write`.
    pub fn context(&self) -> Option<u64> {
        Some(self.input? + self.cache_read? + self.cache_write?)
    }

    /// What one token this request wrote to the cache weighs: 1.25 for the five-minute cache, 2
    /// for the one-hour cache, and 1.25 when the transcript does not say which.
    pub fn write_weight(&self) -> f64 {
        let (short, long) = (self.write_5m.unwrap_or(0), self.write_1h.unwrap_or(0));
        if short + long == 0 { CACHE_WRITE_5M } else { (short as f64 * CACHE_WRITE_5M + long as f64 * CACHE_WRITE_1H) / (short + long) as f64 }
    }

    pub fn split(&self) -> Split {
        let n = |v: Option<u64>| v.unwrap_or(0) as f64;
        let written = self.cache_write.unwrap_or(0);
        let (short, long) = (self.write_5m.unwrap_or(0), self.write_1h.unwrap_or(0));
        // What the split leaves out of the total is written at the five-minute price.
        let rest = written.saturating_sub(short + long);
        let cache_write = if short + long == 0 {
            written as f64 * CACHE_WRITE_5M
        } else {
            short as f64 * CACHE_WRITE_5M + long as f64 * CACHE_WRITE_1H + rest as f64 * CACHE_WRITE_5M
        };
        Split { cache_read: n(self.cache_read) * CACHE_READ, cache_write, input: n(self.input) * INPUT, output: n(self.output) * OUTPUT }
    }

    /// One request is streamed as several lines: each count is the largest any line gave.
    fn merge(&mut self, o: &Usage) {
        let max = |a: Option<u64>, b: Option<u64>| match (a, b) {
            (Some(x), Some(y)) => Some(x.max(y)),
            (x, y) => x.or(y),
        };
        self.input = max(self.input, o.input);
        self.cache_read = max(self.cache_read, o.cache_read);
        self.cache_write = max(self.cache_write, o.cache_write);
        self.write_5m = max(self.write_5m, o.write_5m);
        self.write_1h = max(self.write_1h, o.write_1h);
        self.output = max(self.output, o.output);
        self.thinking = max(self.thinking, o.thinking);
    }
}

/// One request to the model.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub model: String,
    /// Milliseconds since the epoch.
    pub at: Option<i64>,
    /// A sub-agent's request, not one of the main thread.
    pub sidechain: bool,
    pub usage: Usage,
    /// Why the harness says the cache missed (`message.diagnostics.cache_miss_reason.type`),
    /// in its own words.
    pub miss: Option<String>,
}

/// What a tool gave back, as the conversation carries it.
#[derive(Debug, Clone, Default)]
pub struct ToolResult {
    /// What it is summed under: the tool, `Bash WORD`, `MCP SERVER`.
    pub group: String,
    /// The file, for a read.
    pub path: Option<String>,
    /// What makes two results the same call: a read's file, a call's tool and arguments.
    /// `None` for results that are not compared.
    pub repeat: Option<String>,
    pub bytes: u64,
    pub hash: u64,
}

#[derive(Debug, Clone)]
pub enum Event {
    Request(Request),
    Compaction { at: Option<i64>, trigger: String, pre: Option<u64>, post: Option<u64> },
    Fallback { from: String, to: String },
    ToolResult(ToolResult),
}

/// One session, in the order its transcript was written.
#[derive(Debug, Clone, Default)]
pub struct Transcript {
    pub id: String,
    /// The directory the session ran in, as the transcript says.
    pub cwd: Option<String>,
    /// The main thread: requests (sub-agents' among them where the harness writes them inline),
    /// compactions, model fallbacks and tool results.
    pub events: Vec<Event>,
    /// Requests of sub-agents kept in transcripts of their own.
    pub subagents: Vec<Request>,
}

/// The harness a transcript comes from, where the report says something different for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    ClaudeCode,
    Codex,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::ClaudeCode => "Claude Code",
            Kind::Codex => "Codex",
        }
    }

    /// Whether the harness can keep a one-hour cache, so that an hour away is told apart from
    /// five minutes away. Codex leaves the cache to its provider, and providers differ: five
    /// minutes is the threshold for it.
    fn hour_cache(self) -> bool {
        self == Kind::ClaudeCode
    }
}

/// A harness whose session transcripts this report can read.
pub trait Transcripts: Sync {
    fn kind(&self) -> Kind;
    /// Where the transcripts of the project at `root` are read from, as the report names it.
    fn location(&self, root: &Path) -> PathBuf;
    /// The sessions of the project at `root` whose transcript changed since `since`.
    fn sessions(&self, root: &Path, since: SystemTime) -> Vec<PathBuf>;
    /// Every session whose transcript changed since `since`, by project.
    fn projects(&self, since: SystemTime) -> Vec<Vec<PathBuf>>;
    /// The id of the session in `file`, as `--session` matches it.
    fn id(&self, file: &Path) -> String;
    /// One session, read whole.
    fn read(&self, file: &Path) -> Transcript;
}

/// When a file last changed.
fn modified(p: &Path) -> SystemTime {
    std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
}

// ----- Claude Code -----

/// `$CLAUDE_CONFIG_DIR/projects/<slug>/<session>.jsonl`, one JSON object per line. Sub-agents
/// write theirs under `<session>/subagents/`, workflows one level further down.
pub struct ClaudeCode {
    pub config: PathBuf,
}

/// Milliseconds since the epoch of an RFC 3339 time: `2026-11-14T09:30:00.123Z`, or with an
/// offset instead of the `Z`.
pub fn parse_time(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |from: usize, to: usize| -> Option<i64> { s.get(from..to)?.parse().ok() };
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (year, month, day, hour, minute, second) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut i = 19;
    let mut ms = 0;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let digits: String = s[start..i].chars().chain("000".chars()).take(3).collect();
        ms = digits.parse().ok()?;
    }
    let offset = match b.get(i) {
        None | Some(b'Z' | b'z') => 0,
        Some(&sign @ (b'+' | b'-')) => {
            let o = (num(i + 1, i + 3)? * 60 + num(i + 4, i + 6)?) * 60;
            if sign == b'+' { o } else { -o }
        }
        _ => return None,
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some((days * 86_400 + hour * 3600 + minute * 60 + second - offset) * 1000 + ms)
}

/// The first word of a shell command: the program it runs. Commands that only set up the
/// shell (`cd DIR &&`, `export`, a variable assignment) are passed over, so are wrappers such
/// as `nice -n 19` and `timeout 60`, and a path is cut to its last part.
pub fn first_word(command: &str) -> String {
    const SETUP: [&str; 6] = ["cd", "pushd", "export", "set", "source", "."];
    const WRAPPERS: [&str; 8] = ["nice", "time", "env", "sudo", "command", "exec", "timeout", "nohup"];
    let mut skipping = false;
    let mut wrapped = false;
    for raw in command.split_whitespace() {
        let word = raw.trim_start_matches(['(', '{']);
        let ends = word.ends_with(';') || word.ends_with('&');
        let word = word.trim_end_matches([';', '&', ')']);
        if matches!(word, "" | "|" | "||") || word.chars().all(|c| matches!(c, '&' | '|' | ';')) {
            (skipping, wrapped) = (false, false);
            continue;
        }
        if skipping {
            skipping = !ends;
            continue;
        }
        let assignment = word.split_once('=').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        let flag = wrapped && (word.starts_with('-') || word.chars().all(|c| c.is_ascii_digit() || c == '.' || c == 's'));
        if assignment || flag {
            continue;
        }
        if SETUP.contains(&word) {
            skipping = !ends;
            continue;
        }
        if WRAPPERS.contains(&word) {
            wrapped = true;
            continue;
        }
        return word.rsplit('/').next().unwrap_or(word).to_string();
    }
    String::new()
}

/// What a tool's results are summed under, and the file of a read.
fn group(tool: &str, input: &Value) -> (String, Option<String>) {
    if let Some(rest) = tool.strip_prefix("mcp__") {
        return (format!("MCP {}", rest.split("__").next().unwrap_or(rest)), None);
    }
    match tool {
        "Bash" => {
            let word = first_word(input["command"].as_str().unwrap_or(""));
            (if word.is_empty() { "Bash".to_string() } else { format!("Bash {word}") }, None)
        }
        "Read" => ("Read".to_string(), input["file_path"].as_str().map(str::to_string)),
        _ => (tool.to_string(), None),
    }
}

/// The text of a tool result: a string, or a list of blocks of which the text ones count.
/// Images are left out: they are not paid for by their bytes.
fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn usage(u: &Value) -> Usage {
    Usage {
        input: u["input_tokens"].as_u64(),
        cache_read: u["cache_read_input_tokens"].as_u64(),
        cache_write: u["cache_creation_input_tokens"].as_u64(),
        write_5m: u["cache_creation"]["ephemeral_5m_input_tokens"].as_u64(),
        write_1h: u["cache_creation"]["ephemeral_1h_input_tokens"].as_u64(),
        output: u["output_tokens"].as_u64(),
        thinking: u["output_tokens_details"]["thinking_tokens"].as_u64(),
    }
}

/// The last request of the main thread that a transcript records.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Last {
    /// What the next request will send before anything new: the last request's context and
    /// its reply.
    pub context: u64,
    /// Milliseconds since the epoch.
    pub at: i64,
    /// Whether the conversation is written to the one-hour cache, where the transcript says.
    pub one_hour: Option<bool>,
}

/// The last main-thread request of the transcript at `path`, found from the end of the file:
/// a transcript can be a hundred megabytes, and a hook has no time to read it.
pub fn last_request(path: &Path) -> Option<Last> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    for window in [1u64 << 19, 1 << 22, 1 << 25] {
        let from = len.saturating_sub(window);
        file.seek(SeekFrom::Start(from)).ok()?;
        let mut buf = Vec::new();
        file.by_ref().take(window).read_to_end(&mut buf).ok()?;
        // The window begins in the middle of a line unless it begins the file.
        let start = if from == 0 { 0 } else { memchr::memchr(b'\n', &buf).map_or(buf.len(), |i| i + 1) };
        let mut found: Option<Last> = None;
        for line in buf[start..].rsplit(|b| *b == b'\n') {
            if memchr::memmem::find(line, b"\"assistant\"").is_none() {
                continue;
            }
            let Ok(v) = serde_json::from_slice::<Value>(line) else { continue };
            if v["type"] != "assistant" || v["isSidechain"] == true || v["message"]["model"] == "<synthetic>" {
                continue;
            }
            let u = usage(&v["message"]["usage"]);
            let (Some(context), Some(at)) = (u.context(), v["timestamp"].as_str().and_then(parse_time)) else { continue };
            let written = (u.write_5m.unwrap_or(0), u.write_1h.unwrap_or(0));
            let which = (written.0 + written.1 > 0).then_some(written.1 > 0);
            match &mut found {
                // The newest request gives the size and the time. Which cache it writes to is
                // taken from the newest request that wrote anything.
                None => found = Some(Last { context: context + u.output.unwrap_or(0), at, one_hour: which }),
                Some(last) => last.one_hour = last.one_hour.or(which),
            }
            if found.is_some_and(|l| l.one_hour.is_some()) {
                break;
            }
        }
        if found.is_some() || from == 0 {
            return found;
        }
    }
    None
}

/// Which kind of line this is worth parsing as, decided without parsing it. Most of a large
/// transcript is lines of other kinds (requests as sent, attachments, snapshots).
fn wanted(line: &[u8]) -> bool {
    use memchr::memmem::find;
    [&b"\"type\":\"assistant\""[..], b"\"type\": \"assistant\"", b"\"type\":\"user\"", b"\"type\": \"user\"", b"compact_boundary", b"model_fallback"]
        .iter()
        .any(|n| find(line, n).is_some())
}

/// Read one transcript file into `t`. `sub`: a sub-agent's own file, of which only the
/// requests count.
fn read_jsonl(path: &Path, sub: bool, t: &mut Transcript) {
    let Ok(file) = std::fs::File::open(path) else { return };
    let mut reader = std::io::BufReader::with_capacity(1 << 16, file);
    let mut line: Vec<u8> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    // Tool calls by id: (tool, group, path).
    let mut calls: HashMap<String, (String, String, Option<String>)> = HashMap::new();
    let mut n = 0usize;
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        n += 1;
        if !wanted(&line) {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(&line) else { continue };
        let sidechain = sub || v["isSidechain"] == true;
        if t.cwd.is_none() {
            t.cwd = v["cwd"].as_str().map(str::to_string);
        }
        match v["type"].as_str() {
            Some("assistant") => {
                let m = &v["message"];
                let model = m["model"].as_str().unwrap_or("").to_string();
                if model == "<synthetic>" {
                    continue;
                }
                for block in m["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use") {
                    if let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) {
                        let (g, path) = group(name, &block["input"]);
                        calls.insert(id.to_string(), (name.to_string(), g, path));
                    }
                }
                let u = usage(&m["usage"]);
                let miss = m["diagnostics"]["cache_miss_reason"]["type"].as_str().map(str::to_string);
                // A line without an id is a request of its own.
                let id = m["id"].as_str().map(str::to_string).unwrap_or_else(|| format!("line {n}"));
                if let Some(&i) = seen.get(&id) {
                    if let Some(Event::Request(r)) = t.events.get_mut(i) {
                        r.usage.merge(&u);
                        if r.miss.is_none() {
                            r.miss = miss;
                        }
                    }
                    continue;
                }
                seen.insert(id, t.events.len());
                t.events.push(Event::Request(Request { model, at: v["timestamp"].as_str().and_then(parse_time), sidechain, usage: u, miss }));
            }
            Some("user") if !sidechain => {
                for block in v["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    let text = result_text(&block["content"]);
                    let (tool, g, path) =
                        block["tool_use_id"].as_str().and_then(|id| calls.get(id)).cloned().unwrap_or_else(|| ("?".to_string(), "other".to_string(), None));
                    let hash = xxhash_rust::xxh3::xxh3_64(text.as_bytes());
                    let repeat = if tool == "Read" { path.clone() } else { None };
                    t.events.push(Event::ToolResult(ToolResult { group: g, path, repeat, bytes: text.len() as u64, hash }));
                }
            }
            Some("system") if !sidechain => match v["subtype"].as_str() {
                Some("compact_boundary") => {
                    let meta = &v["compactMetadata"];
                    t.events.push(Event::Compaction {
                        at: v["timestamp"].as_str().and_then(parse_time),
                        trigger: meta["trigger"].as_str().unwrap_or("").to_string(),
                        pre: meta["preTokens"].as_u64(),
                        post: meta["postTokens"].as_u64(),
                    });
                }
                Some("model_fallback") => {
                    let name = |key: &str| v[key].as_str().unwrap_or("").to_string();
                    t.events.push(Event::Fallback { from: name("originalModel"), to: name("fallbackModel") });
                }
                _ => {}
            },
            _ => {}
        }
    }
}

/// Every `.jsonl` file below `dir`.
fn jsonl_below(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() && depth > 0 {
            jsonl_below(&p, depth - 1, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}

impl ClaudeCode {
    /// The transcripts of one project directory changed since `since`.
    fn in_dir(dir: &Path, since: SystemTime) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "jsonl") && modified(p) >= since)
            .collect();
        v.sort();
        v
    }
}

impl Transcripts for ClaudeCode {
    fn kind(&self) -> Kind {
        Kind::ClaudeCode
    }

    fn location(&self, root: &Path) -> PathBuf {
        self.config.join("projects").join(super::audit::project_slug(root))
    }

    fn sessions(&self, root: &Path, since: SystemTime) -> Vec<PathBuf> {
        ClaudeCode::in_dir(&self.location(root), since)
    }

    fn projects(&self, since: SystemTime) -> Vec<Vec<PathBuf>> {
        let mut dirs: Vec<PathBuf> =
            std::fs::read_dir(self.config.join("projects")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        dirs.iter().map(|d| ClaudeCode::in_dir(d, since)).filter(|v| !v.is_empty()).collect()
    }

    fn id(&self, file: &Path) -> String {
        file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }

    fn read(&self, file: &Path) -> Transcript {
        let mut t = Transcript { id: file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), ..Default::default() };
        read_jsonl(file, false, &mut t);
        let mut subs = Vec::new();
        jsonl_below(&file.with_extension("").join("subagents"), 4, &mut subs);
        subs.sort();
        for f in subs {
            let mut s = Transcript::default();
            read_jsonl(&f, true, &mut s);
            t.subagents.extend(s.events.into_iter().filter_map(|e| if let Event::Request(r) = e { Some(r) } else { None }));
        }
        t
    }
}

// ----- Codex -----

/// `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<time>-<uuid>.jsonl`, one JSON object per line
/// with `timestamp`, `type` and `payload`. A session belongs to the project its `session_meta`
/// names as `cwd`, or to the project that directory lies in.
pub struct Codex {
    pub home: PathBuf,
}

/// Tools that run a shell command: their results are summed by the program they ran.
const SHELLS: [&str; 6] = ["shell", "shell_command", "exec_command", "local_shell", "local_shell_call", "container.exec"];

/// The command line a Codex shell call runs: `["bash", "-lc", "cargo test"]` is `cargo test`.
fn codex_command(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            let words: Vec<&str> = parts.iter().filter_map(Value::as_str).collect();
            match words.as_slice() {
                [_, flag, script, ..] if matches!(*flag, "-lc" | "-c") => script.to_string(),
                _ => words.join(" "),
            }
        }
        _ => String::new(),
    }
}

/// What a Codex tool's results are summed under: `shell WORD` for a shell, `MCP SERVER` for a
/// tool of an MCP server, the tool's name otherwise.
fn codex_group(name: &str, args: &Value) -> String {
    if SHELLS.contains(&name) {
        let command = codex_command(if args["command"].is_null() { &args["cmd"] } else { &args["command"] });
        let word = first_word(&command);
        return if word.is_empty() { "shell".to_string() } else { format!("shell {word}") };
    }
    if let Some((server, _)) = name.strip_prefix("mcp__").unwrap_or(name).split_once("__") {
        return format!("MCP {server}");
    }
    // The `exec` tool is given a line of script that runs one of the tools above:
    // `text(await tools.exec_command({cmd:"rg -n foo src"}))`. What ran is the command in it.
    if name == "exec"
        && let Some(script) = args["input"].as_str()
        && let Some(word) = scripted_command(script)
    {
        return format!("shell {word}");
    }
    name.to_string()
}

/// The program a line of script hands to a shell tool, where the line is one such call with a
/// literal command: `tools.exec_command({cmd:"sed -n '1,40p' a.go"})` gives `sed`.
fn scripted_command(script: &str) -> Option<String> {
    let call = SHELLS.iter().filter_map(|t| script.find(&format!("tools.{t}(")).map(|i| &script[i..])).next()?;
    let at = ["cmd:", "command:", "\"cmd\":", "\"command\":"].iter().filter_map(|k| call.find(k).map(|i| i + k.len())).min()?;
    let rest = call[at..].trim_start().trim_start_matches('[').trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\'' | '`'))?;
    let word = first_word(rest[1..].split(quote).next()?);
    (!word.is_empty()).then_some(word)
}

/// The text a Codex tool output puts into the conversation: a string, a list of parts, or an
/// object with `output` or `content`.
fn codex_output(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str().or(p.as_str())).collect::<Vec<_>>().join("\n"),
        Value::Object(_) if !v["output"].is_null() => codex_output(&v["output"]),
        Value::Object(_) => codex_output(&v["content"]),
        _ => String::new(),
    }
}

/// One request's counts from `last_token_usage`. `input_tokens` holds everything sent, the
/// cached part and the part written to the cache included. `reasoning_output_tokens` is taken
/// as a part of `output_tokens`, as the Responses API counts reasoning. `None` when the counts
/// are all zero: that is not a request.
fn codex_usage(u: &Value) -> Option<Usage> {
    let n = |k: &str| u[k].as_u64();
    let all = ["input_tokens", "cached_input_tokens", "cache_write_input_tokens", "output_tokens", "reasoning_output_tokens"];
    if !u.is_object() || all.iter().all(|k| n(k).unwrap_or(0) == 0) {
        return None;
    }
    let (cached, written) = (n("cached_input_tokens"), n("cache_write_input_tokens").unwrap_or(0));
    Some(Usage {
        input: n("input_tokens").map(|i| i.saturating_sub(cached.unwrap_or(0) + written)),
        cache_read: cached,
        cache_write: Some(written),
        write_5m: None,
        write_1h: None,
        output: n("output_tokens"),
        thinking: n("reasoning_output_tokens"),
    })
}

/// Lines worth parsing in a rollout.
fn codex_wanted(line: &[u8]) -> bool {
    use memchr::memmem::find;
    [&b"session_meta"[..], b"turn_context", b"token_count", b"response_item", b"compact"].iter().any(|n| find(line, n).is_some())
}

impl Codex {
    /// Every rollout changed since `since`.
    fn rollouts(&self, since: SystemTime) -> Vec<PathBuf> {
        let mut v = Vec::new();
        jsonl_below(&self.home.join("sessions"), 3, &mut v);
        v.retain(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rollout-")) && modified(p) >= since);
        v.sort();
        v
    }

    /// The directory a rollout's session ran in, from its `session_meta` near the top.
    fn cwd(file: &Path) -> Option<PathBuf> {
        let reader = std::io::BufReader::new(std::fs::File::open(file).ok()?);
        for line in reader.lines().take(20) {
            let line = line.ok()?;
            if !line.contains("session_meta") {
                continue;
            }
            let v: Value = serde_json::from_str(&line).ok()?;
            if v["type"] == "session_meta" {
                let cwd = PathBuf::from(v["payload"]["cwd"].as_str()?);
                return Some(cwd.canonicalize().unwrap_or(cwd));
            }
        }
        None
    }
}

impl Transcripts for Codex {
    fn kind(&self) -> Kind {
        Kind::Codex
    }

    fn location(&self, _root: &Path) -> PathBuf {
        self.home.join("sessions")
    }

    fn sessions(&self, root: &Path, since: SystemTime) -> Vec<PathBuf> {
        self.rollouts(since).into_iter().filter(|f| Codex::cwd(f).is_some_and(|c| c.starts_with(root))).collect()
    }

    fn projects(&self, since: SystemTime) -> Vec<Vec<PathBuf>> {
        let mut by: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        for f in self.rollouts(since) {
            if let Some(cwd) = Codex::cwd(&f) {
                by.entry(cwd).or_default().push(f);
            }
        }
        by.into_values().collect()
    }

    fn id(&self, file: &Path) -> String {
        // `rollout-2026-11-14T14-33-46-<uuid>`: the uuid.
        let stem = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let rest = stem.strip_prefix("rollout-").unwrap_or(&stem);
        match rest.get(19..20) {
            Some("-") => rest[20..].to_string(),
            _ => rest.to_string(),
        }
    }

    fn read(&self, file: &Path) -> Transcript {
        let mut t = Transcript { id: self.id(file), ..Default::default() };
        let Ok(f) = std::fs::File::open(file) else { return t };
        let mut reader = std::io::BufReader::with_capacity(1 << 16, f);
        let mut line: Vec<u8> = Vec::new();
        let mut model = String::new();
        // The running total of the last `token_count`: a report repeated with the same total is
        // the same request.
        let mut total = Value::Null;
        // Calls by id: (tool, group, what makes two calls the same).
        let mut calls: HashMap<String, (String, String, String)> = HashMap::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if !codex_wanted(&line) {
                continue;
            }
            let Ok(v) = serde_json::from_slice::<Value>(&line) else { continue };
            let at = v["timestamp"].as_str().and_then(parse_time);
            let p = &v["payload"];
            let (kind, inner) = (v["type"].as_str().unwrap_or(""), p["type"].as_str().unwrap_or(""));
            match (kind, inner) {
                ("session_meta", _) => t.cwd = t.cwd.take().or_else(|| p["cwd"].as_str().map(str::to_string)),
                ("turn_context", _) => {
                    if let Some(m) = p["model"].as_str().or(p["collaboration_mode"]["settings"]["model"].as_str()) {
                        model = m.to_string();
                    }
                }
                ("event_msg", "token_count") => {
                    let info = &p["info"];
                    let Some(usage) = codex_usage(&info["last_token_usage"]) else { continue };
                    if !info["total_token_usage"].is_null() && info["total_token_usage"] == total {
                        continue;
                    }
                    total = info["total_token_usage"].clone();
                    t.events.push(Event::Request(Request { model: model.clone(), at, sidechain: false, usage, miss: None }));
                }
                ("response_item", "function_call" | "custom_tool_call" | "local_shell_call") => {
                    let id = p["call_id"].as_str().or(p["id"].as_str()).unwrap_or("").to_string();
                    let (name, args) = match inner {
                        "local_shell_call" => ("local_shell_call".to_string(), json!({"command": p["action"]["command"]})),
                        "custom_tool_call" => (p["name"].as_str().unwrap_or("?").to_string(), json!({"input": p["input"]})),
                        _ => {
                            let raw = p["arguments"].as_str().unwrap_or("");
                            (p["name"].as_str().unwrap_or("?").to_string(), serde_json::from_str(raw).unwrap_or_else(|_| json!({"arguments": raw})))
                        }
                    };
                    let group = codex_group(&name, &args);
                    let key = format!("{name} {args}");
                    calls.insert(id, (name, group, key));
                }
                ("response_item", o) if o.ends_with("_output") => {
                    let text = codex_output(&p["output"]);
                    let (_, group, key) =
                        p["call_id"].as_str().and_then(|id| calls.get(id)).cloned().unwrap_or_else(|| ("?".to_string(), "other".to_string(), String::new()));
                    let hash = xxhash_rust::xxh3::xxh3_64(text.as_bytes());
                    let repeat = (!key.is_empty()).then_some(key);
                    t.events.push(Event::ToolResult(ToolResult { group, path: None, repeat, bytes: text.len() as u64, hash }));
                }
                // The rollout may say it twice (an event and an item): once is a boundary.
                (k, i) if (k.contains("compact") || i.contains("compact")) && !matches!(t.events.last(), Some(Event::Compaction { .. })) => {
                    t.events.push(Event::Compaction { at, trigger: p["trigger"].as_str().unwrap_or("").to_string(), pre: None, post: None });
                }
                _ => {}
            }
        }
        // A compaction's sizes are not recorded: the context of the request before it and of
        // the one after stand in for them (an estimate).
        for i in 0..t.events.len() {
            if let Event::Compaction { .. } = t.events[i] {
                let before = t.events[..i].iter().rev().find_map(|e| if let Event::Request(r) = e { r.usage.context() } else { None });
                let after = t.events[i + 1..].iter().find_map(|e| if let Event::Request(r) = e { r.usage.context() } else { None });
                if let Event::Compaction { pre, post, .. } = &mut t.events[i] {
                    *pre = pre.or(before);
                    *post = post.or(after);
                }
            }
        }
        t
    }
}

// ----- the arithmetic -----

/// What a tool result costs while the conversation carries it: written to the cache by the
/// first request after it, read from the cache by each one after that (`n` requests in all).
pub fn carried(bytes: u64, n: usize) -> f64 {
    bytes as f64 / BYTES_PER_TOKEN * (CACHE_WRITE_5M + CACHE_READ * n.saturating_sub(1) as f64)
}

/// The estimated cost of a compaction: the conversation read once, the summary written once.
pub fn compaction_cost(pre: Option<u64>, post: Option<u64>) -> f64 {
    CACHE_READ * pre.unwrap_or(0) as f64 + CACHE_WRITE_5M * post.unwrap_or(0) as f64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cause {
    ModelSwitch,
    IdleHour,
    IdleFive,
    Other,
}

impl Cause {
    const ALL: [Cause; 4] = [Cause::ModelSwitch, Cause::IdleHour, Cause::IdleFive, Cause::Other];

    pub fn words(self) -> &'static str {
        match self {
            Cause::ModelSwitch => "model switch",
            Cause::IdleHour => "idle for more than an hour",
            Cause::IdleFive => "idle for more than five minutes",
            Cause::Other => "other",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Cause::ModelSwitch => "model_switch",
            Cause::IdleHour => "idle_hour",
            Cause::IdleFive => "idle_five_minutes",
            Cause::Other => "other",
        }
    }
}

/// Why request `cur` found the cache cold after `prev`: first match wins. `hour`: the harness
/// can keep a one-hour cache, so an hour away is a cause of its own.
pub fn cause(prev: &Request, cur: &Request, hour: bool) -> Cause {
    let gap = match (prev.at, cur.at) {
        (Some(a), Some(b)) => b - a,
        _ => 0,
    };
    if prev.model != cur.model {
        Cause::ModelSwitch
    } else if hour && gap >= IDLE_HOUR_MS {
        Cause::IdleHour
    } else if gap >= IDLE_FIVE_MS {
        Cause::IdleFive
    } else {
        Cause::Other
    }
}

/// The weighted tokens a cold restart cost more than a warm one would have: `None` when `cur`
/// read enough of `prev`'s context from the cache, or when a count is not recorded.
///
/// `uncached`: what was sent again without being written to the cache counts too, at the
/// price of uncached input. A Codex provider that reports no cache writes bills a cold
/// request so. For Claude Code the cache writes alone are counted.
pub fn cold_cost(prev: &Usage, cur: &Usage, uncached: bool) -> Option<f64> {
    let before = prev.context()?;
    let read = cur.cache_read?;
    if before <= COLD_FROM || read as f64 >= COLD_SHARE * before as f64 {
        return None;
    }
    let written = before.min(cur.cache_write.unwrap_or(0));
    let mut cost = (cur.write_weight() - CACHE_READ) * written as f64;
    if uncached {
        cost += (INPUT - CACHE_READ) * (before - written).min(cur.input.unwrap_or(0)) as f64;
    }
    Some(cost)
}

/// Whether the harness's own reason for a cache miss names another cause than `cause`. `None`
/// when its reason does not tell which cause it is (`previous_message_not_found`,
/// `unavailable` and names this report does not know).
pub fn disagrees(reason: &str, cause: Cause) -> Option<bool> {
    let theirs = match reason {
        "model_changed" => Cause::ModelSwitch,
        r if r.contains("ttl") || r.contains("expir") => Cause::IdleHour,
        "messages_changed" | "tools_changed" | "system_changed" => Cause::Other,
        _ => return None,
    };
    let idle = |c: Cause| matches!(c, Cause::IdleHour | Cause::IdleFive);
    Some(!(theirs == cause || idle(theirs) && idle(cause)))
}

#[derive(Debug, Clone)]
pub struct Cold {
    pub session: String,
    pub at: Option<i64>,
    pub cause: Cause,
    /// A model fallback the harness recorded explains the switch.
    pub fallback: bool,
    pub before: u64,
    pub gap_ms: Option<i64>,
    pub cost: f64,
    /// The harness's own reason for the miss, when it recorded one.
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Compaction {
    pub session: String,
    pub at: Option<i64>,
    pub manual: bool,
    pub pre: Option<u64>,
    pub post: Option<u64>,
    pub cost: f64,
}

impl Compaction {
    pub fn early(&self) -> bool {
        self.manual && self.pre.is_some_and(|p| p < EARLY_COMPACTION)
    }
}

/// The requests between two compactions.
#[derive(Debug, Clone)]
pub struct Stretch {
    pub session: String,
    /// The context of its first request.
    pub first: u64,
    pub requests: usize,
    /// `first` read again by every later request.
    pub cost: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Carried {
    pub session: String,
    pub group: String,
    pub path: Option<String>,
    pub bytes: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Group {
    pub results: usize,
    pub bytes: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Thinking {
    pub requests: usize,
    /// Requests whose transcript records thinking tokens.
    pub recorded: usize,
    pub thinking: u64,
    /// The output tokens of those requests.
    pub output: u64,
}

/// The numbers of one or more sessions.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub kind: Kind,
    pub sessions: usize,
    pub requests: usize,
    pub main: Split,
    pub sub_requests: usize,
    pub sub: Split,
    /// Main-thread requests whose context is not recorded.
    pub unknown_context: usize,
    pub stretches: Vec<Stretch>,
    pub cold: Vec<Cold>,
    pub compactions: Vec<Compaction>,
    pub thinking: Thinking,
    pub models: BTreeMap<String, (usize, f64)>,
    pub switches: usize,
    pub groups: BTreeMap<String, Group>,
    /// The three heaviest tool results.
    pub largest: Vec<Carried>,
    pub repeats: Vec<Carried>,
    pub per_session: Vec<Session>,
    pub cwd: Option<String>,
    pub carry: Carry,
}

/// One session in a few numbers.
#[derive(Debug, Clone, Default)]
pub struct Session {
    pub id: String,
    pub weighted: f64,
    /// Main-thread requests.
    pub requests: usize,
    pub first: Option<i64>,
    pub last: Option<i64>,
}

/// Context above a threshold, read again by every request that carries it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Carry {
    pub threshold: u64,
    /// Main-thread requests with a known context, and those above the threshold.
    pub requests: usize,
    pub above: usize,
    pub peak: u64,
    /// `0.1 *` the context above the threshold, summed over the requests.
    pub cost: f64,
    /// The compactions that keeping the context at the threshold would have taken, by
    /// [`sawtooth`].
    pub compactions: u64,
    /// What those compactions would have cost (an estimate).
    pub instead: f64,
    /// The reads they would have saved: `0.1 *` the context each request would not have
    /// carried (an estimate).
    pub saved: f64,
}

/// Compacting at `threshold` as a sawtooth: once a stretch has passed the threshold, each
/// compaction brings the context down to `post`, and it grows again as it did. For a context
/// `c`: the context the request would have had, and how many compactions it took to get there.
pub fn sawtooth(c: u64, threshold: u64, post: u64) -> (u64, u64) {
    let period = threshold.saturating_sub(post).max(1);
    if c <= post {
        return (c, 0);
    }
    (post + (c - post) % period, (c - post) / period)
}

/// The median of what the compactions of a session left, or [`POST_TOKENS`] without one.
pub fn median_post(mut posts: Vec<u64>) -> u64 {
    if posts.is_empty() {
        return POST_TOKENS;
    }
    posts.sort_unstable();
    let n = posts.len();
    if n % 2 == 1 { posts[n / 2] } else { (posts[n / 2 - 1] + posts[n / 2]) / 2 }
}

/// What one compaction at `threshold` would cost: the conversation read once, `post` written.
pub fn compaction_at(threshold: u64, post: u64) -> f64 {
    compaction_cost(Some(threshold), Some(post))
}

impl Stats {
    pub fn new(kind: Kind, carry: u64) -> Stats {
        Stats { kind, carry: Carry { threshold: carry, ..Default::default() }, ..Default::default() }
    }

    pub fn total(&self) -> f64 {
        self.main.total() + self.sub.total()
    }

    fn count(&mut self, r: &Request) -> Split {
        let split = r.usage.split();
        let m = self.models.entry(r.model.clone()).or_default();
        m.0 += 1;
        m.1 += split.total();
        self.thinking.requests += 1;
        if let (Some(t), Some(o)) = (r.usage.thinking, r.usage.output) {
            self.thinking.recorded += 1;
            self.thinking.thinking += t;
            self.thinking.output += o;
        }
        split
    }

    fn sub_request(&mut self, r: &Request) {
        let split = self.count(r);
        self.sub_requests += 1;
        self.sub.add(&split);
    }

    pub fn start(&self) -> f64 {
        self.stretches.iter().map(|s| s.cost).sum()
    }

    /// The cold restarts for which the harness's own reason names another cause.
    pub fn harness_disagrees(&self) -> usize {
        self.cold.iter().filter(|c| c.reason.as_deref().and_then(|r| disagrees(r, c.cause)) == Some(true)).count()
    }

    pub fn cold_cost(&self) -> f64 {
        self.cold.iter().map(|c| c.cost).sum()
    }

    pub fn compaction_cost(&self, manual: bool) -> f64 {
        self.compactions.iter().filter(|c| c.manual == manual).map(|c| c.cost).sum()
    }

    pub fn thinking_cost(&self) -> f64 {
        OUTPUT * self.thinking.thinking as f64
    }

    pub fn repeat_cost(&self) -> f64 {
        self.repeats.iter().map(|r| r.cost).sum()
    }

    /// The findings with their weighted cost, largest first, without those under 1% of the total.
    /// What a result given back twice is called: a read in Claude Code, a call's output in Codex.
    pub fn repeat_key(&self) -> &'static str {
        match self.kind {
            Kind::ClaudeCode => "repeated_reads",
            Kind::Codex => "repeated_outputs",
        }
    }

    pub fn findings(&self) -> Vec<(&'static str, f64)> {
        let mut v = vec![
            ("start", self.start()),
            ("cold_restarts", self.cold_cost()),
            ("thinking", self.thinking_cost()),
            ("manual_compactions", self.compaction_cost(true)),
            ("automatic_compactions", self.compaction_cost(false)),
            (self.repeat_key(), self.repeat_cost()),
            ("carried_context", if self.carry.above > 0 { self.carry.cost } else { 0.0 }),
        ];
        let total = self.total();
        v.retain(|(_, cost)| total > 0.0 && *cost >= WORTH_NAMING * total);
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        v
    }

    fn keep_largest(&mut self, c: Carried) {
        self.largest.push(c);
        self.largest.sort_by(|a, b| b.cost.total_cmp(&a.cost));
        self.largest.truncate(3);
    }

    fn close(&mut self, session: &str, first: Option<u64>, k: usize, open: &mut Vec<(ToolResult, usize, bool)>) {
        if let (Some(first), true) = (first, k > 0) {
            self.stretches.push(Stretch { session: session.to_string(), first, requests: k, cost: CACHE_READ * first as f64 * (k - 1) as f64 });
        }
        for (r, at, repeat) in open.drain(..) {
            let cost = carried(r.bytes, k - at);
            let g = self.groups.entry(r.group.clone()).or_default();
            g.results += 1;
            g.bytes += r.bytes;
            g.cost += cost;
            let c = Carried { session: session.to_string(), group: r.group, path: r.path, bytes: r.bytes, cost };
            if repeat {
                self.repeats.push(c.clone());
            }
            if self.largest.len() < 3 || self.largest.last().is_some_and(|l| l.cost < cost) {
                self.keep_largest(c);
            }
        }
    }

    /// Add one session.
    pub fn add(&mut self, t: &Transcript) {
        self.sessions += 1;
        if self.cwd.is_none() {
            self.cwd = t.cwd.clone();
        }
        let before = self.total();
        let fallbacks: HashSet<(&str, &str)> =
            t.events.iter().filter_map(|e| if let Event::Fallback { from, to } = e { Some((from.as_str(), to.as_str())) } else { None }).collect();
        let mut prev: Option<&Request> = None;
        let mut compacted = false;
        let (mut first, mut k) = (None, 0usize);
        let (mut requests, mut first_at, mut last_at) = (0usize, None, None);
        let mut open: Vec<(ToolResult, usize, bool)> = Vec::new();
        let mut reads: HashSet<(String, u64)> = HashSet::new();
        let threshold = self.carry.threshold;
        // What a compaction leaves, never more than half the threshold.
        let post = median_post(t.events.iter().filter_map(|e| if let Event::Compaction { post, .. } = e { *post } else { None }).collect()).min(threshold / 2);
        // Whether the stretch has passed the threshold, and the compactions counted in it.
        let (mut reached, mut wraps) = (false, 0u64);
        for e in &t.events {
            match e {
                Event::Request(r) if r.sidechain => self.sub_request(r),
                Event::Request(r) => {
                    let split = self.count(r);
                    self.requests += 1;
                    requests += 1;
                    self.main.add(&split);
                    match r.usage.context() {
                        None => self.unknown_context += 1,
                        Some(c) => {
                            self.carry.requests += 1;
                            self.carry.peak = self.carry.peak.max(c);
                            if c > threshold {
                                self.carry.above += 1;
                                self.carry.cost += CACHE_READ * (c - threshold) as f64;
                                reached = true;
                            }
                            if reached {
                                let (would, n) = sawtooth(c, threshold, post);
                                if n > wraps {
                                    self.carry.compactions += n - wraps;
                                    self.carry.instead += (n - wraps) as f64 * compaction_at(threshold, post);
                                    wraps = n;
                                }
                                self.carry.saved += CACHE_READ * c.saturating_sub(would) as f64;
                            }
                        }
                    }
                    first_at = first_at.or(r.at);
                    last_at = r.at.or(last_at);
                    if let Some(p) = prev {
                        if p.model != r.model {
                            self.switches += 1;
                        }
                        if let Some(cost) = cold_cost(&p.usage, &r.usage, self.kind == Kind::Codex).filter(|_| !compacted) {
                            let why = cause(p, r, self.kind.hour_cache());
                            self.cold.push(Cold {
                                session: t.id.clone(),
                                at: r.at,
                                cause: why,
                                fallback: why == Cause::ModelSwitch && fallbacks.contains(&(p.model.as_str(), r.model.as_str())),
                                before: p.usage.context().unwrap_or(0),
                                gap_ms: p.at.zip(r.at).map(|(a, b)| b - a),
                                cost,
                                reason: r.miss.clone(),
                            });
                        }
                    }
                    if k == 0 {
                        first = r.usage.context();
                    }
                    k += 1;
                    compacted = false;
                    prev = Some(r);
                }
                Event::Compaction { at, trigger, pre, post } => {
                    self.close(&t.id, first, k, &mut open);
                    (first, k, reached, wraps) = (None, 0, false, 0);
                    reads.clear();
                    compacted = true;
                    self.compactions.push(Compaction {
                        session: t.id.clone(),
                        at: *at,
                        manual: trigger == "manual",
                        pre: *pre,
                        post: *post,
                        cost: compaction_cost(*pre, *post),
                    });
                }
                Event::Fallback { .. } => {}
                Event::ToolResult(r) => {
                    let repeat = r.repeat.as_ref().is_some_and(|k| !reads.insert((k.clone(), r.hash)));
                    open.push((r.clone(), k, repeat));
                }
            }
        }
        self.close(&t.id, first, k, &mut open);
        for r in &t.subagents {
            self.sub_request(r);
        }
        self.per_session.push(Session { id: t.id.clone(), weighted: self.total() - before, requests, first: first_at, last: last_at });
    }
}

// ----- the report -----

/// Weighted tokens, short: `18.4M`, `52k`, `730`.
pub fn short(x: f64) -> String {
    let x = if x > 0.0 { x } else { 0.0 };
    if x >= 999_500.0 {
        format!("{:.1}M", x / 1e6)
    } else if x >= 999.5 {
        format!("{:.0}k", x / 1e3)
    } else {
        format!("{x:.0}")
    }
}

/// A share as a whole percentage, `<1%` for a small one that is not zero.
pub fn share(part: f64, total: f64) -> String {
    if total <= 0.0 {
        return "0%".to_string();
    }
    let p = part / total * 100.0;
    if p > 0.0 && p < 0.5 { "<1%".to_string() } else { format!("{p:.0}%") }
}

fn size(bytes: u64) -> String {
    if bytes >= 1 << 20 { format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64) } else { format!("{} KB", bytes.div_ceil(1024)) }
}

fn when(ms: Option<i64>) -> String {
    ms.filter(|m| *m >= 0).map(|m| super::doctor::utc((m / 1000) as u64)).unwrap_or_else(|| "at an unrecorded time".to_string())
}

fn minutes(ms: Option<i64>) -> String {
    match ms {
        Some(ms) if ms >= 3_600_000 => format!("{:.1} hours", ms as f64 / 3_600_000.0),
        Some(ms) => format!("{} min", ms / 60_000),
        None => "unknown gap".to_string(),
    }
}

/// A path as the report shows it: below the project relative to it.
fn shown(path: &str, ctx: &Ctx) -> String {
    ctx.show(Path::new(path))
}

fn list(items: &[String], max: usize) -> String {
    if items.len() > max { format!("{}, and {} more", items[..max].join(", "), items.len() - max) } else { items.join(", ") }
}

/// How many items of a breakdown are shown.
const ITEMS: usize = 10;

fn finding(out: &mut String, s: &Stats, key: &str, cost: f64, ctx: &Ctx, detail: bool) {
    let total = s.total();
    let head = format!("  {:>6} {:>4}  ", short(cost), share(cost, total));
    let pad = " ".repeat(15);
    let mut push = |first: bool, text: &str| {
        out.push_str(if first { &head } else { &pad });
        out.push_str(text);
        out.push('\n');
    };
    match key {
        "start" => {
            let n = s.stretches.len();
            let avg = s.stretches.iter().map(|x| x.first).sum::<u64>() / n.max(1) as u64;
            let again: usize = s.stretches.iter().map(|x| x.requests.saturating_sub(1)).sum();
            let stretches = if n == 1 { "the start of the conversation".to_string() } else { format!("the start of {n} stretches between compactions") };
            push(
                true,
                &format!("The start, {} tokens on average at {stretches}, was read again from the cache by {} requests.", short(avg as f64), count(again)),
            );
            push(false, "`codefast doctor context` shows what the start holds and what can be switched off for this project.");
            if detail {
                for x in s.stretches.iter().take(ITEMS) {
                    push(false, &format!("  {} tokens, {} requests: {}", short(x.first as f64), count(x.requests), short(x.cost)));
                }
            }
        }
        "cold_restarts" => {
            let mut parts = Vec::new();
            for c in Cause::ALL {
                let of: Vec<&Cold> = s.cold.iter().filter(|x| x.cause == c).collect();
                if of.is_empty() {
                    continue;
                }
                let fell = of.iter().filter(|x| x.fallback).count();
                let note = if fell > 0 { format!(", {fell} after a model fallback") } else { String::new() };
                parts.push(format!("{} {} ({}{note})", of.len(), c.words(), short(of.iter().map(|x| x.cost).sum())));
            }
            let n = s.cold.len();
            let other = s.harness_disagrees();
            let clause = if other * 4 > n { format!(", and the harness names another cause for {other} of them") } else { String::new() };
            push(
                true,
                &format!(
                    "Cold restarts: {n} request{} sent the conversation again at the price of new input: {}{clause}.",
                    if n == 1 { "" } else { "s" },
                    parts.join(", ")
                ),
            );
            let mut todo = Vec::new();
            if s.cold.iter().any(|x| matches!(x.cause, Cause::IdleHour | Cause::IdleFive)) {
                todo.push("Come back within the cache lifetime or start fresh.");
            }
            if s.cold.iter().any(|x| x.cause == Cause::ModelSwitch) {
                todo.push("Finish a job on one model.");
            }
            if !todo.is_empty() {
                push(false, &todo.join(" "));
            }
            if detail {
                for x in s.cold.iter().take(ITEMS) {
                    let fell = if x.fallback { " (a model fallback)" } else { "" };
                    let theirs = x.reason.as_deref().map(|r| format!(" (harness: {r})")).unwrap_or_default();
                    push(
                        false,
                        &format!(
                            "  {}  {}{fell}{theirs} after {}, {} tokens sent again: {}",
                            when(x.at),
                            x.cause.words(),
                            minutes(x.gap_ms),
                            short(x.before as f64),
                            short(x.cost)
                        ),
                    );
                }
            }
        }
        "thinking" => {
            let t = &s.thinking;
            let partly =
                if t.recorded < t.requests { format!(" (recorded on {} of {} requests)", count(t.recorded), count(t.requests)) } else { String::new() };
            push(
                true,
                &format!(
                    "Thinking: {} of {} output tokens ({}){partly}, {}.",
                    short(t.thinking as f64),
                    short(t.output as f64),
                    share(t.thinking as f64, t.output as f64),
                    if s.kind == Kind::Codex { "counted as output, at 5 times the weight of input" } else { "billed as output at 5 times the input price" }
                ),
            );
            push(false, "Lower effort for routine work.");
        }
        "manual_compactions" | "automatic_compactions" => {
            let manual = key == "manual_compactions";
            let of: Vec<&Compaction> = s.compactions.iter().filter(|c| c.manual == manual).collect();
            let at: Vec<String> = of.iter().map(|c| c.pre.map(|p| short(p as f64)).unwrap_or_else(|| "?".to_string())).collect();
            let early = of.iter().filter(|c| c.early()).count();
            let early = if early > 0 { format!(", {early} of them early (under 100k)") } else { String::new() };
            // Codex does not record what started a compaction, nor its sizes: those are the
            // context of the requests either side of it.
            let what = match (manual, s.kind) {
                (true, _) => "Manual compactions",
                (false, Kind::ClaudeCode) => "Automatic compactions",
                (false, Kind::Codex) => "Compactions",
            };
            push(true, &format!("{what}: {}, at {} tokens{early}. Each read the conversation and wrote a summary (an estimate).", of.len(), list(&at, 6)));
            push(
                false,
                if manual { "`/clear` when a job is done instead of a compaction." } else { "`/clear` when a job is done, before the conversation fills up." },
            );
            if detail {
                for c in of.iter().take(ITEMS) {
                    let pre = c.pre.map(|p| short(p as f64)).unwrap_or_else(|| "not recorded".to_string());
                    let post = c.post.map(|p| short(p as f64)).unwrap_or_else(|| "not recorded".to_string());
                    push(false, &format!("  {}  {pre} tokens down to {post}{}: {}", when(c.at), if c.early() { ", early" } else { "" }, short(c.cost)));
                }
            }
        }
        "carried_context" => {
            let c = &s.carry;
            let t = short(c.threshold as f64);
            push(
                true,
                &format!(
                    "Carried context: {} of {} requests ({}) carried more than {t} tokens, at most {}. Reading the part above {t} again cost {}.",
                    count(c.above),
                    count(c.requests),
                    share(c.above as f64, c.requests as f64),
                    short(c.peak as f64),
                    short(c.cost)
                ),
            );
            let gain = c.saved - c.instead;
            let n = format!("{} compaction{}, about {}", c.compactions, if c.compactions == 1 { "" } else { "s" }, short(c.instead));
            if gain > 0.0 {
                push(false, &format!("Compacting at {t} instead: {n}, so about {} less (an estimate).", short(gain)));
                push(false, "Start a fresh session at a natural break, or compact earlier, when the context stays far above what the next step needs.");
            } else {
                push(false, &format!("Compacting at {t} instead: {n}, which would not have paid (an estimate)."));
            }
        }
        "repeated_reads" | "repeated_outputs" => {
            let bytes: u64 = s.repeats.iter().map(|r| r.bytes).sum();
            let n = s.repeats.len();
            let plural = if n == 1 { "" } else { "s" };
            if key == "repeated_reads" {
                push(
                    true,
                    &format!("Repeated reads: {n} read{plural} gave back a file word for word as an earlier read of the same stretch did ({}).", size(bytes)),
                );
            } else {
                push(
                    true,
                    &format!(
                        "Repeated outputs: {n} call{plural} gave back word for word what the same call with the same arguments gave earlier in the stretch ({}).",
                        size(bytes)
                    ),
                );
            }
            push(false, "The first copy is still in the conversation: read again only what changed.");
            if detail {
                let mut by: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
                for r in &s.repeats {
                    let e = by.entry(r.path.as_deref().unwrap_or(&r.group)).or_default();
                    e.0 += 1;
                    e.1 += r.bytes;
                }
                let mut by: Vec<_> = by.into_iter().collect();
                by.sort_by_key(|(_, (_, b))| std::cmp::Reverse(*b));
                for (path, (n, b)) in by.into_iter().take(ITEMS) {
                    push(false, &format!("  {}  {n} more time{}, {}", shown(path, ctx), if n == 1 { "" } else { "s" }, size(b)));
                }
            }
        }
        _ => {}
    }
}

/// The report of one project or one session.
fn text(s: &Stats, ctx: &Ctx, dir: &Path, days: u64, session: Option<&str>) -> String {
    let mut out = String::new();
    let harness = s.kind.name();
    let total = s.total();
    match session {
        Some(id) => {
            let Session { requests, first: from, last: to, .. } = s.per_session.first().cloned().unwrap_or_default();
            out.push_str(&format!(
                "{harness} session {id}: {} main-thread requests from {} to {}, {} weighted tokens.\n",
                count(requests),
                when(from),
                when(to),
                short(total)
            ));
        }
        None => out.push_str(&format!(
            "{harness} sessions of this project in the last {days} days: {} session{}, {} main-thread requests, {} weighted tokens.\n",
            count(s.sessions),
            if s.sessions == 1 { "" } else { "s" },
            count(s.requests),
            short(total)
        )),
    }
    out.push_str(&match s.kind {
        Kind::ClaudeCode => format!(
            "(A weighted token is one uncached input token of the request's model: a cache read weighs 0.1, a cache write 1.25, or 2 for the one-hour cache, an output token 5. Output and thinking are the counts the transcript recorded, the largest of the lines of a request. Read from {} on this machine.)\n",
            ctx.show(dir)
        ),
        Kind::Codex => format!(
            "(Weighted as for Claude Code, although Codex providers price differently: one uncached input token of the request's model, a cache read 0.1, a cache write 1.25, an output token 5. Output and thinking are the counts the transcript recorded, thinking as a part of output. Read from {} on this machine.)\n",
            ctx.show(dir)
        ),
    });
    let m = &s.main;
    out.push_str(&format!(
        "  main thread {:>7}  cache reads {} ({}), cache writes {} ({}), uncached input {} ({}), output {} ({})\n",
        short(m.total()),
        short(m.cache_read),
        share(m.cache_read, total),
        short(m.cache_write),
        share(m.cache_write, total),
        short(m.input),
        share(m.input, total),
        short(m.output),
        share(m.output, total)
    ));
    if s.sub_requests > 0 {
        out.push_str(&format!("  sub-agents  {:>7}  {} requests ({})\n", short(s.sub.total()), count(s.sub_requests), share(s.sub.total(), total)));
    }
    let mut models: Vec<(&String, &(usize, f64))> = s.models.iter().collect();
    models.sort_by(|a, b| b.1.1.total_cmp(&a.1.1));
    let names: Vec<String> = models.iter().map(|(name, (n, w))| format!("{name} {} requests {} ({})", count(*n), short(*w), share(*w, total))).collect();
    let switches = match s.switches {
        0 => String::new(),
        1 => ". 1 switch in the main thread".to_string(),
        n => format!(". {n} switches in the main thread"),
    };
    out.push_str(&format!("  models       {}{switches}\n", list(&names, 3)));

    let findings = s.findings();
    if !findings.is_empty() {
        out.push_str(&format!("\nWhere it went, largest first, as a share of the {} in all:\n", short(total)));
        for (key, cost) in &findings {
            finding(&mut out, s, key, *cost, ctx, session.is_some());
        }
    }
    if !s.groups.is_empty() {
        let mut groups: Vec<(&String, &Group)> = s.groups.iter().collect();
        groups.sort_by(|a, b| b.1.cost.total_cmp(&a.1.cost));
        out.push_str("\nWhat tool results weighed, from the request after each to the next compaction:\n");
        let line: Vec<String> = groups.iter().take(5).map(|(g, x)| format!("{g} {} ({})", short(x.cost), share(x.cost, total))).collect();
        out.push_str(&format!("  {}\n", line.join(", ")));
        let largest: Vec<String> = s
            .largest
            .iter()
            .map(|c| {
                let path = c.path.as_deref().map(|p| format!(" {}", shown(p, ctx))).unwrap_or_default();
                format!("{}{path} ({}, {})", c.group, size(c.bytes), short(c.cost))
            })
            .collect();
        out.push_str(&format!("  largest: {}\n", largest.join(", ")));
    }
    let mut unrecorded = Vec::new();
    if s.thinking.recorded == 0 && s.thinking.requests > 0 {
        unrecorded.push("thinking tokens".to_string());
    }
    if s.unknown_context > 0 {
        unrecorded.push(format!("token counts of {} main-thread requests, left out of the cold restarts", count(s.unknown_context)));
    }
    if !unrecorded.is_empty() {
        out.push_str(&format!("Not recorded in these transcripts: {}.\n", unrecorded.join(", and ")));
    }
    if session.is_none() && s.sessions > 1 {
        let mut by: Vec<&Session> = s.per_session.iter().collect();
        by.sort_by(|a, b| b.weighted.total_cmp(&a.weighted));
        let top: Vec<String> = by.iter().take(3).map(|x| format!("{} {}", x.id.get(..8).unwrap_or(&x.id), short(x.weighted))).collect();
        out.push_str(&format!("Largest sessions: {}. `codefast doctor sessions --session ID` breaks one down.\n", top.join(", ")));
    }
    out
}

fn round(x: f64) -> u64 {
    if x > 0.0 { x.round() as u64 } else { 0 }
}

fn ratio(part: f64, total: f64) -> Value {
    // `+ 0.0` turns the -0.0 that an empty sum of floats starts from into 0.
    if total > 0.0 { json!((part / total * 10_000.0).round() / 10_000.0 + 0.0) } else { Value::Null }
}

fn to_json(s: &Stats, ctx: &Ctx, detail: bool) -> Value {
    let total = s.total();
    let split = |x: &Split| json!({"total": round(x.total()), "cache_read": round(x.cache_read), "cache_write": round(x.cache_write), "input": round(x.input), "output": round(x.output)});
    let causes: Vec<Value> = Cause::ALL
        .iter()
        .map(|c| {
            let of: Vec<&Cold> = s.cold.iter().filter(|x| x.cause == *c).collect();
            json!({"cause": c.key(), "count": of.len(), "weighted": round(of.iter().map(|x| x.cost).sum()), "after_fallback": of.iter().filter(|x| x.fallback).count()})
        })
        .collect();
    let compactions = |manual: bool| {
        let of: Vec<&Compaction> = s.compactions.iter().filter(|c| c.manual == manual).collect();
        let cost: f64 = of.iter().map(|c| c.cost).sum();
        json!({
            "count": of.len(),
            "early": of.iter().filter(|c| c.early()).count(),
            "pre_tokens": of.iter().map(|c| c.pre).collect::<Vec<_>>(),
            "weighted_estimate": round(cost),
            "share": ratio(cost, total),
        })
    };
    let t = &s.thinking;
    let thinking = if t.recorded == 0 {
        json!({"requests": t.requests, "recorded_requests": 0, "thinking_tokens": null, "output_tokens": null, "share_of_output": null, "weighted": null, "share": null})
    } else {
        json!({
            "requests": t.requests,
            "recorded_requests": t.recorded,
            "thinking_tokens": t.thinking,
            "output_tokens": t.output,
            "share_of_output": ratio(t.thinking as f64, t.output as f64),
            "weighted": round(s.thinking_cost()),
            "share": ratio(s.thinking_cost(), total),
        })
    };
    let mut groups: Vec<(&String, &Group)> = s.groups.iter().collect();
    groups.sort_by(|a, b| b.1.cost.total_cmp(&a.1.cost));
    let carried = |c: &Carried| json!({"session": c.session, "tool": c.group, "path": c.path.as_deref().map(|p| shown(p, ctx)), "bytes": c.bytes, "weighted": round(c.cost)});
    let mut out = json!({
        "sessions": s.sessions,
        "requests": s.requests,
        "weighted": split(&s.main),
        "total_weighted": round(total),
        "subagents": {"requests": s.sub_requests, "weighted": split(&s.sub), "share": ratio(s.sub.total(), total)},
        "start": {
            "weighted": round(s.start()),
            "share": ratio(s.start(), total),
            "stretches": s.stretches.len(),
            "first_context_tokens": s.stretches.iter().map(|x| x.first).collect::<Vec<_>>(),
        },
        "cold_restarts": {"count": s.cold.len(), "weighted": round(s.cold_cost()), "share": ratio(s.cold_cost(), total), "by_cause": causes, "harness_names_another_cause": s.harness_disagrees()},
        "compactions": {"manual": compactions(true), "auto": compactions(false)},
        "thinking": thinking,
        "models": s.models.iter().map(|(m, (n, w))| json!({"model": m, "requests": n, "weighted": round(*w)})).collect::<Vec<_>>(),
        "model_switches": s.switches,
        "tool_results": {
            "groups": groups.iter().map(|(g, x)| json!({"tool": g, "results": x.results, "bytes": x.bytes, "weighted": round(x.cost)})).collect::<Vec<_>>(),
            "largest": s.largest.iter().map(carried).collect::<Vec<_>>(),
        },
        "carried_context": {
            "threshold": s.carry.threshold,
            "requests": s.carry.requests,
            "requests_above": s.carry.above,
            "share_of_requests": ratio(s.carry.above as f64, s.carry.requests as f64),
            "peak_context": s.carry.peak,
            "weighted": round(s.carry.cost),
            "share": ratio(s.carry.cost, total),
            "compactions_instead": s.carry.compactions,
            "compactions_weighted_estimate": round(s.carry.instead),
            "saved_reads_estimate": round(s.carry.saved),
            "difference_estimate": (s.carry.saved - s.carry.instead).round() + 0.0,
        },
        "unknown_context_requests": s.unknown_context,
        "findings": s.findings().iter().map(|(k, c)| json!({"finding": k, "weighted": round(*c), "share": ratio(*c, total)})).collect::<Vec<_>>(),
        "by_session": s.per_session.iter().map(|x| json!({"session": x.id, "requests": x.requests, "weighted": round(x.weighted)})).collect::<Vec<_>>(),
    });
    out[s.repeat_key()] = json!({"count": s.repeats.len(), "bytes": s.repeats.iter().map(|r| r.bytes).sum::<u64>(), "weighted": round(s.repeat_cost()), "share": ratio(s.repeat_cost(), total)});
    if detail {
        out["cold_restarts"]["items"] = s
            .cold
            .iter()
            .map(|c| json!({"session": c.session, "at": c.at.map(|m| when(Some(m))), "cause": c.cause.key(), "after_fallback": c.fallback, "gap_ms": c.gap_ms, "context_before": c.before, "weighted": round(c.cost), "harness_reason": c.reason}))
            .collect();
        out["compactions"]["items"] = s
            .compactions
            .iter()
            .map(|c| json!({"session": c.session, "at": c.at.map(|m| when(Some(m))), "trigger": if c.manual { "manual" } else { "auto" }, "pre_tokens": c.pre, "post_tokens": c.post, "early": c.early(), "weighted_estimate": round(c.cost)}))
            .collect();
        out[s.repeat_key()]["items"] = s.repeats.iter().map(carried).collect();
        out["start"]["items"] = s
            .stretches
            .iter()
            .map(|x| json!({"session": x.session, "first_context_tokens": x.first, "requests": x.requests, "weighted": round(x.cost)}))
            .collect();
    }
    out
}

// ----- the command -----

struct Options {
    days: u64,
    carry: u64,
    session: Option<String>,
    all: bool,
    json: bool,
}

fn options(args: &[String]) -> Result<Option<Options>> {
    let mut o = Options { days: 14, carry: CARRY, session: None, all: false, json: false };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut value = |name: &str| {
            inline.clone().or_else(|| it.next().cloned()).ok_or_else(|| anyhow::anyhow!("{name} needs a value (usage: codefast doctor sessions --help)"))
        };
        match key {
            "--days" => {
                let v = value("--days")?;
                o.days = v.parse().ok().filter(|d| *d > 0).ok_or_else(|| anyhow::anyhow!("--days takes a whole number of days above 0, not `{v}`"))?;
            }
            "--session" => o.session = Some(value("--session")?),
            "--carry" => {
                let v = value("--carry")?;
                o.carry = v.parse().ok().filter(|n| *n > 0).ok_or_else(|| anyhow::anyhow!("--carry takes a number of tokens above 0, not `{v}`"))?;
            }
            "--all" => o.all = true,
            "--json" => o.json = true,
            "--help" | "-h" | "help" => return Ok(None),
            other => bail!("unknown option `{other}` (usage: codefast doctor sessions [--days N] [--session ID] [--carry N] [--all] [--json])"),
        }
    }
    if o.all && o.session.is_some() {
        bail!("--session names a session of this project: it does not go with --all");
    }
    Ok(Some(o))
}

/// The harnesses read, Claude Code first.
fn sources(ctx: &Ctx) -> Vec<Box<dyn Transcripts>> {
    let codex = super::harness::env_path("CODEX_HOME").unwrap_or_else(|| ctx.home(".codex"));
    vec![Box::new(ClaudeCode { config: super::audit::claude_dir(ctx) }), Box::new(Codex { home: codex })]
}

fn since(days: u64) -> SystemTime {
    SystemTime::now().checked_sub(Duration::from_secs(days * 86_400)).unwrap_or(SystemTime::UNIX_EPOCH)
}

fn stats(source: &dyn Transcripts, files: &[PathBuf], carry: u64) -> Stats {
    let transcripts: Vec<Transcript> = files.par_iter().map(|f| source.read(f)).collect();
    let mut s = Stats::new(source.kind(), carry);
    for t in &transcripts {
        s.add(t);
    }
    s
}

fn findings_json(s: &Stats) -> Vec<Value> {
    s.findings().iter().map(|(k, c)| json!({"finding": k, "weighted": round(*c), "share": ratio(*c, s.total())})).collect()
}

fn all_projects(ctx: &Ctx, o: &Options) -> Result<String> {
    let from = since(o.days);
    let mut rows: Vec<(String, Stats)> = Vec::new();
    let mut sums: Vec<(usize, Stats)> = Vec::new();
    for source in sources(ctx) {
        let mut sum = Stats::new(source.kind(), o.carry);
        let mut projects = 0;
        for files in source.projects(from) {
            let s = stats(source.as_ref(), &files, o.carry);
            // Named from the home directory, wherever the command runs.
            let name = s
                .cwd
                .as_deref()
                .map(|c| match ctx.home.as_ref().and_then(|h| Path::new(c).strip_prefix(h).ok()) {
                    Some(rel) => format!("~/{}", rel.display()),
                    None => c.to_string(),
                })
                .unwrap_or_else(|| "?".to_string());
            sum.sessions += s.sessions;
            sum.requests += s.requests;
            sum.main.add(&s.main);
            sum.sub.add(&s.sub);
            sum.sub_requests += s.sub_requests;
            sum.stretches.extend(s.stretches.iter().cloned());
            sum.cold.extend(s.cold.iter().cloned());
            sum.compactions.extend(s.compactions.iter().cloned());
            sum.repeats.extend(s.repeats.iter().cloned());
            sum.thinking.requests += s.thinking.requests;
            sum.thinking.recorded += s.thinking.recorded;
            sum.thinking.thinking += s.thinking.thinking;
            sum.thinking.output += s.thinking.output;
            sum.carry.requests += s.carry.requests;
            sum.carry.above += s.carry.above;
            sum.carry.peak = sum.carry.peak.max(s.carry.peak);
            sum.carry.cost += s.carry.cost;
            sum.carry.compactions += s.carry.compactions;
            sum.carry.instead += s.carry.instead;
            sum.carry.saved += s.carry.saved;
            projects += 1;
            rows.push((name, s));
        }
        if projects > 0 {
            sums.push((projects, sum));
        }
    }
    rows.sort_by(|a, b| b.1.total().total_cmp(&a.1.total()));
    let largest = |s: &Stats| s.findings().first().map(|(k, c)| format!(". Largest: {} ({})", k.replace('_', " "), share(*c, s.total()))).unwrap_or_default();
    if o.json {
        let projects: Vec<Value> = rows
            .iter()
            .map(|(name, s)| json!({"harness": s.kind.name(), "project": name, "sessions": s.sessions, "requests": s.requests, "weighted": round(s.total()), "findings": findings_json(s)}))
            .collect();
        let totals: Vec<Value> = sums
            .iter()
            .map(|(n, s)| json!({"harness": s.kind.name(), "projects": n, "sessions": s.sessions, "requests": s.requests, "weighted": round(s.total()), "findings": findings_json(s)}))
            .collect();
        return Ok(format!("{}\n", serde_json::to_string_pretty(&json!({"days": o.days, "projects": projects, "totals": totals}))?));
    }
    if rows.is_empty() {
        let places: Vec<String> =
            sources(ctx).iter().map(|s| ctx.show(&s.location(Path::new("/")).parent().map(Path::to_path_buf).unwrap_or_default())).collect();
        return Ok(format!("No Claude Code or Codex session in the last {} days under {}.\n", o.days, places.join(" or ")));
    }
    let width = rows.iter().map(|(_, s)| s.kind.name().len()).max().unwrap_or(0);
    let mut out = format!("Sessions in the last {} days, by project, largest first:\n", o.days);
    for (name, s) in rows.iter().take(20) {
        out.push_str(&format!(
            "  {:>7}  {:<width$}  {name}: {} sessions, {} main-thread requests{}\n",
            short(s.total()),
            s.kind.name(),
            count(s.sessions),
            count(s.requests),
            largest(s)
        ));
    }
    if rows.len() > 20 {
        out.push_str(&format!("  … {} more projects\n", rows.len() - 20));
    }
    for (n, sum) in &sums {
        out.push_str(&format!(
            "  {:>7}  {:<width$}  all {n} projects: {} sessions, {} main-thread requests{}\n",
            short(sum.total()),
            sum.kind.name(),
            count(sum.sessions),
            count(sum.requests),
            largest(sum)
        ));
    }
    out.push_str("`codefast doctor sessions` in a project shows its findings.\n");
    Ok(out)
}

pub fn run(root: &Path, args: &[String]) -> Result<String> {
    let Some(o) = options(args)? else { return Ok(USAGE.to_string()) };
    let ctx = Ctx::new(root);
    if o.all {
        return all_projects(&ctx, &o);
    }
    let sources = sources(&ctx);
    // The sessions of each harness: those of the window, or the one `--session` names.
    let mut chosen: Vec<Vec<PathBuf>> = sources.iter().map(|src| src.sessions(&ctx.root, since(o.days))).collect();
    if let Some(prefix) = &o.session {
        let mut found: Vec<(usize, PathBuf)> = Vec::new();
        for (i, src) in sources.iter().enumerate() {
            found.extend(src.sessions(&ctx.root, SystemTime::UNIX_EPOCH).into_iter().filter(|f| src.id(f).starts_with(prefix.as_str())).map(|f| (i, f)));
        }
        match found.len() {
            0 => bail!("no Claude Code or Codex session of this project starts with `{prefix}`"),
            1 => {
                chosen = vec![Vec::new(); sources.len()];
                let (i, f) = found.remove(0);
                chosen[i].push(f);
            }
            n => bail!("{n} sessions start with `{prefix}`: {}", found.iter().map(|(i, f)| sources[*i].id(f)).collect::<Vec<_>>().join(", ")),
        }
    }
    let all: Vec<(&dyn Transcripts, Stats)> = sources.iter().zip(&chosen).map(|(src, files)| (src.as_ref(), stats(src.as_ref(), files, o.carry))).collect();
    if o.json {
        let harnesses: Vec<Value> = all
            .iter()
            .filter(|(_, s)| o.session.is_none() || s.sessions > 0)
            .map(|(src, s)| {
                let mut v = to_json(s, &ctx, o.session.is_some());
                v["harness"] = json!(s.kind.name());
                v["transcripts"] = json!(src.location(&ctx.root).display().to_string());
                v
            })
            .collect();
        let days = if o.session.is_some() { Value::Null } else { json!(o.days) };
        return Ok(format!("{}\n", serde_json::to_string_pretty(&json!({"project": ctx.root.display().to_string(), "days": days, "harnesses": harnesses}))?));
    }
    let reports: Vec<String> = all
        .iter()
        .filter(|(_, s)| s.sessions > 0)
        .map(|(src, s)| {
            let id = o.session.as_ref().and_then(|_| s.per_session.first()).map(|p| p.id.clone());
            text(s, &ctx, &src.location(&ctx.root), o.days, id.as_deref())
        })
        .collect();
    if reports.is_empty() {
        let places: Vec<String> = all.iter().map(|(src, _)| ctx.show(&src.location(&ctx.root))).collect();
        return Ok(format!(
            "No Claude Code or Codex session of this project in the last {} days. Transcripts are read from {}.\n",
            o.days,
            places.join(" and ")
        ));
    }
    Ok(reports.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(input: u64, read: u64, write: u64, output: u64) -> Usage {
        Usage { input: Some(input), cache_read: Some(read), cache_write: Some(write), output: Some(output), ..Default::default() }
    }

    fn req(model: &str, minute: i64, usage: Usage) -> Event {
        Event::Request(Request { model: model.to_string(), at: Some(minute * 60_000), sidechain: false, usage, miss: None })
    }

    #[test]
    fn weights_of_a_request() {
        let mut x = u(10, 1000, 400, 20);
        let s = x.split();
        assert_eq!((s.input, s.cache_read, s.cache_write, s.output), (10.0, 100.0, 500.0, 100.0));
        assert_eq!(x.context(), Some(1410));
        // The one-hour cache costs more to write, and the split decides the weight.
        x.write_5m = Some(100);
        x.write_1h = Some(300);
        assert_eq!(x.split().cache_write, 100.0 * 1.25 + 300.0 * 2.0);
        assert!((x.write_weight() - (125.0 + 600.0) / 400.0).abs() < 1e-9);
        assert_eq!(u(0, 0, 0, 0).write_weight(), 1.25);
        // An unknown count is unknown, not zero.
        assert_eq!(Usage { input: Some(1), cache_read: None, cache_write: Some(1), ..Default::default() }.context(), None);
    }

    #[test]
    fn carried_and_estimated_costs() {
        assert!((carried(3500, 1) - 1250.0).abs() < 1e-9);
        assert!((carried(3500, 11) - (1000.0 * (1.25 + 1.0))).abs() < 1e-9);
        assert!((carried(3500, 0) - 1250.0).abs() < 1e-9);
        assert_eq!(compaction_cost(Some(80_000), Some(10_000)), 8_000.0 + 12_500.0);
    }

    #[test]
    fn a_cold_restart_and_its_cause() {
        let prev = Request { model: "a".into(), at: Some(0), usage: u(1, 50_000, 0, 10), ..Default::default() };
        let warm = u(1, 50_000, 300, 10);
        assert_eq!(cold_cost(&prev.usage, &warm, false), None);
        let cold = u(1, 2_000, 60_000, 10);
        assert_eq!(cold_cost(&prev.usage, &cold, false), Some((1.25 - 0.1) * 50_001.0));
        // Sent again as uncached input, with no cache write: 0.9 a token where it is counted.
        let uncached = u(40_000, 0, 0, 10);
        assert_eq!(cold_cost(&prev.usage, &uncached, false), Some(0.0));
        assert!((cold_cost(&prev.usage, &uncached, true).unwrap() - 0.9 * 40_000.0).abs() < 1e-6);
        // A small conversation is not worth naming, and an unknown read is not a cold one.
        assert_eq!(cold_cost(&u(1, 10_000, 0, 1), &u(1, 0, 10_000, 1), false), None);
        assert_eq!(cold_cost(&prev.usage, &Usage { cache_read: None, ..cold }, false), None);
        let at = |m: &str, minutes: i64| Request { model: m.into(), at: Some(minutes * 60_000), ..Default::default() };
        assert_eq!(cause(&prev, &at("b", 90), true), Cause::ModelSwitch);
        assert_eq!(cause(&prev, &at("a", 55), true), Cause::IdleHour);
        assert_eq!(cause(&prev, &at("a", 54), true), Cause::IdleFive);
        assert_eq!(cause(&prev, &at("a", 5), true), Cause::IdleFive);
        assert_eq!(cause(&prev, &at("a", 4), true), Cause::Other);
        // Without a one-hour cache an hour away is five minutes away and more.
        assert_eq!(cause(&prev, &at("a", 90), false), Cause::IdleFive);
    }

    #[test]
    fn a_session_in_numbers() {
        let read = |path: &str, hash: u64| {
            Event::ToolResult(ToolResult { group: "Read".into(), path: Some(path.into()), repeat: Some(path.into()), bytes: 3500, hash })
        };
        let t = Transcript {
            id: "s".into(),
            events: vec![
                req("a", 0, u(10, 0, 30_000, 100)),
                read("/p/x.rs", 1),
                req("a", 1, u(10, 30_000, 1_000, 100)),
                read("/p/x.rs", 1),
                req("a", 2, u(10, 31_000, 1_000, 100)),
                // An hour away: the cache is cold.
                req("a", 70, u(10, 0, 32_000, 100)),
                Event::Compaction { at: None, trigger: "manual".into(), pre: Some(40_000), post: Some(5_000) },
                // Right after a compaction a cold cache is expected.
                req("a", 71, u(10, 0, 5_000, 100)),
                req("a", 72, u(10, 5_000, 100, 100)),
            ],
            ..Default::default()
        };
        let mut s = Stats::new(Kind::ClaudeCode, CARRY);
        s.add(&t);
        assert_eq!((s.sessions, s.requests, s.cold.len()), (1, 6, 1));
        assert_eq!(s.cold[0].cause, Cause::IdleHour);
        assert!((s.cold[0].cost - 1.15 * 32_000.0).abs() < 1e-6);
        // Two stretches: 30,010 tokens read again by 3 requests, then 5,010 by 1.
        assert_eq!(s.stretches.len(), 2);
        assert!((s.start() - (0.1 * 30_010.0 * 3.0 + 0.1 * 5_010.0)).abs() < 1e-6);
        // The second read of the same file is a repeat, carried by 2 requests.
        assert_eq!(s.repeats.len(), 1);
        assert!((s.repeats[0].cost - carried(3500, 2)).abs() < 1e-9);
        assert_eq!(s.groups["Read"].results, 2);
        assert!(s.compactions[0].early());
    }

    #[test]
    fn context_carried_above_the_threshold() {
        // Context climbing from 150k to 600k in ten requests, each reading the one before.
        let mut events = Vec::new();
        let mut before = 0;
        for i in 0..10u64 {
            let context = 150_000 + 50_000 * i;
            events.push(req("a", i as i64, u(0, before, context - before, 10)));
            before = context;
        }
        let t = Transcript { id: "s".into(), events, ..Default::default() };
        let mut s = Stats::new(Kind::ClaudeCode, CARRY);
        s.add(&t);
        let c = s.carry;
        // Compacted at 200k down to 25k, the context repeats every 175k: requests of 250k to
        // 600k would have carried 75k, 125k, 175k, 50k, 100k, 150k, 25k and 75k. It wraps at
        // 250k, 400k and 550k.
        let would = [75, 125, 175, 50, 100, 150, 25, 75];
        for (i, w) in would.iter().enumerate() {
            assert_eq!(sawtooth(250_000 + 50_000 * i as u64, 200_000, 25_000).0, w * 1000);
        }
        assert_eq!(sawtooth(150_000, 200_000, 25_000), (150_000, 0));
        let saved: u64 = would.iter().enumerate().map(|(i, w)| 250_000 + 50_000 * i as u64 - w * 1000).sum();
        assert_eq!(saved, 2_625_000);
        assert_eq!((c.requests, c.above, c.peak, c.compactions), (10, 8, 600_000, 3));
        assert!((c.cost - 0.1 * 50_000.0 * 36.0).abs() < 1e-6);
        assert!((c.saved - 0.1 * saved as f64).abs() < 1e-6);
        assert!((c.instead - 3.0 * (20_000.0 + 1.25 * 25_000.0)).abs() < 1e-6);
        assert!(s.cold.is_empty());
        // A compaction that left a lot, more than half the threshold: compacting at 450k
        // brings the context down to 225k only, and would not have paid.
        let mut big = vec![Event::Compaction { at: None, trigger: "auto".into(), pre: Some(500_000), post: Some(300_000) }];
        big.extend(t.events.iter().cloned());
        let mut b = Stats::new(Kind::ClaudeCode, 450_000);
        b.add(&Transcript { id: "b".into(), events: big, ..Default::default() });
        assert_eq!((b.carry.above, b.carry.compactions), (3, 1));
        assert!((b.carry.saved - 0.1 * 3.0 * 225_000.0).abs() < 1e-6);
        assert!((b.carry.instead - (45_000.0 + 1.25 * 225_000.0)).abs() < 1e-6);
        let mut out = String::new();
        finding(&mut out, &b, "carried_context", b.carry.cost, &Ctx::new(Path::new("/")), false);
        assert!(out.contains("Compacting at 450k instead: 1 compaction, about 326k, which would not have paid (an estimate).\n"), "{out}");
        assert!(!out.contains("Start a fresh session"), "{out}");
        let mut low = Stats::new(Kind::ClaudeCode, 700_000);
        low.add(&t);
        assert_eq!((low.carry.above, low.carry.cost), (0, 0.0));
        assert!(!low.findings().iter().any(|(k, _)| *k == "carried_context"));
        assert_eq!((median_post(vec![]), median_post(vec![9, 1, 5]), median_post(vec![4, 10])), (25_000, 5, 7));
    }

    #[test]
    fn the_harness_reason_set_against_ours() {
        assert_eq!(disagrees("model_changed", Cause::ModelSwitch), Some(false));
        assert_eq!(disagrees("model_changed", Cause::IdleHour), Some(true));
        assert_eq!(disagrees("messages_changed", Cause::IdleFive), Some(true));
        assert_eq!(disagrees("tools_changed", Cause::Other), Some(false));
        assert_eq!(disagrees("cache_ttl_expired", Cause::IdleFive), Some(false));
        assert_eq!(disagrees("previous_message_not_found", Cause::IdleHour), None);
        assert_eq!(disagrees("unavailable", Cause::Other), None);
    }

    #[test]
    fn codex_counts_and_tools() {
        let u = codex_usage(
            &json!({"input_tokens": 1000, "cached_input_tokens": 700, "cache_write_input_tokens": 200, "output_tokens": 50, "reasoning_output_tokens": 30}),
        )
        .unwrap();
        assert_eq!((u.input, u.cache_read, u.cache_write, u.output, u.thinking), (Some(100), Some(700), Some(200), Some(50), Some(30)));
        assert_eq!(u.context(), Some(1000));
        // A provider that reports no cache writes, and a report of nothing.
        let v = codex_usage(&json!({"input_tokens": 1000, "cached_input_tokens": 900, "output_tokens": 5})).unwrap();
        assert_eq!((v.input, v.cache_write, v.thinking), (Some(100), Some(0), None));
        assert!(codex_usage(&json!({"input_tokens": 0, "cached_input_tokens": 0, "output_tokens": 0, "reasoning_output_tokens": 0})).is_none());
        assert_eq!(codex_group("shell", &json!({"command": ["bash", "-lc", "cd x && cargo test"]})), "shell cargo");
        assert_eq!(codex_group("exec_command", &json!({"cmd": "rg -n foo"})), "shell rg");
        assert_eq!(codex_group("local_shell_call", &json!({"command": ["git", "status"]})), "shell git");
        assert_eq!(codex_group("mcp__codefast__context", &Value::Null), "MCP codefast");
        assert_eq!(codex_group("apply_patch", &Value::Null), "apply_patch");
        assert_eq!(codex_group("exec", &json!({"input": "text(await tools.exec_command({cmd:\"rg -n 'Add\\(' syft\"}))"})), "shell rg");
        assert_eq!(codex_group("exec", &json!({"input": "const r = await tools.shell({command: ['bash', '-lc', 'x']}); text(r)"})), "shell bash");
        assert_eq!(codex_group("exec", &json!({"input": "text(1 + 1)"})), "exec");
        assert_eq!(codex_output(&json!({"output": "abc", "metadata": {}})), "abc");
        assert_eq!(codex_output(&json!([{"type": "input_text", "text": "a"}, {"type": "input_text", "text": "b"}])), "a\nb");
        let c = Codex { home: PathBuf::new() };
        assert_eq!(c.id(Path::new("/s/rollout-2026-11-14T14-33-46-01a1d94c-6cbb-7f10.jsonl")), "01a1d94c-6cbb-7f10");
    }

    #[test]
    fn times_and_words() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-11-13T02:07:11.250Z"), Some(1_794_535_631_250));
        assert_eq!(parse_time("2026-11-13T04:07:11+02:00"), Some(1_794_535_631_000));
        assert_eq!(parse_time("2000-02-29T00:00:00.5Z"), Some(951_782_400_500));
        assert_eq!(parse_time("yesterday"), None);
        assert_eq!(first_word("git status"), "git");
        assert_eq!(first_word("cd /x/y && cargo test -q"), "cargo");
        assert_eq!(first_word("RUST_LOG=1 /usr/bin/rg foo"), "rg");
        assert_eq!(first_word("(cd /x && CARGO_BUILD_JOBS=2 nice -n 19 cargo build)"), "cargo");
        assert_eq!(first_word("W=/a; cd $W && sed -n 1p f"), "sed");
        assert_eq!(first_word("export A=1; timeout 60 python3 x.py | head"), "python3");
        assert_eq!(first_word("cd \"/a b\""), "");
        assert_eq!(group("mcp__codefast__context", &Value::Null).0, "MCP codefast");
        assert_eq!(group("Bash", &json!({"command": "ls -la"})).0, "Bash ls");
        assert_eq!((short(18_400_000.0), short(52_300.0), short(730.0)), ("18.4M".to_string(), "52k".to_string(), "730".to_string()));
        assert_eq!((share(1.0, 400.0), share(0.0, 5.0), share(1.0, 0.0)), ("<1%".to_string(), "0%".to_string(), "0%".to_string()));
    }
}
