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
//! The unit is the weighted token: one uncached input token of the request's own model, each
//! kind of token weighed as that model's published price weighs it ([`weights`], from the
//! table in this file). Dollars are not printed: prices change.
//!
//! Two harnesses are read, each through a [`Transcripts`]: Claude Code and Codex. Codex
//! sessions are weighed by the default weights, since its providers price differently.

use super::harness::Ctx;
use crate::output::count;
use anyhow::{Result, bail};
use rayon::prelude::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// What one token weighs, in uncached input tokens of the same model, for a model the price
/// table below does not name. [`weights`] gives them for a model.
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
/// What a compaction writes and leaves, where a session has none of its own to measure. The
/// summary is written as output by a request that reads the conversation, and the first request
/// after it sends as new what the conversation now is: the summary, and the files and lists the
/// harness puts back. Measured on 31 compactions of long Claude Code 2.1 sessions in November
/// 2026: summaries of 12,000 to 35,000 characters (24,000 in the middle), and 28,000 to 110,000
/// tokens sent as new after them (57,000 in the middle).
const SUMMARY_TOKENS: u64 = 6_000;
const RESENT_TOKENS: u64 = 50_000;
/// The start of a session where it is not known: what stays in the cache across a compaction.
const START_TOKENS: u64 = 25_000;
/// The hooks speak of a long conversation from where a compaction would pay for itself within
/// this many requests ([`carry_from`]).
const PAYBACK: f64 = 15.0;
/// The lengths at which an earlier compaction is tried for a session (`--carry N` tries one).
const WINDOWS: [u64; 7] = [150_000, 200_000, 300_000, 400_000, 500_000, 600_000, 800_000];
/// Every compaction loses some of the conversation, so the length the report speaks of is the
/// longest one that gains at least this share of what the best one gains.
const KEEP: f64 = 0.8;
/// A finding under this share of the total is left out.
const WORTH_NAMING: f64 = 0.01;

// ----- prices -----

/// What each kind of token of one model weighs, as a multiple of that model's own base price
/// of an uncached input token.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weights {
    pub cache_read: f64,
    pub write_5m: f64,
    pub write_1h: f64,
    pub output: f64,
    pub tier: Option<Tier>,
}

/// A price that steps with the length of the prompt: a request whose prompt (all of its input,
/// cache reads and cache writes included) is over `over` tokens pays every kind of token
/// `times` as much. Exactly `over` is still the lower price. Each request is priced on its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tier {
    pub over: u64,
    pub times: f64,
}

/// The weights of a model the table does not name: older Claude models, and every Codex model,
/// whose providers price differently.
pub const DEFAULT_WEIGHTS: Weights = Weights { cache_read: CACHE_READ, write_5m: CACHE_WRITE_5M, write_1h: CACHE_WRITE_1H, output: OUTPUT, tier: None };

/// When the table was read, and from where.
pub const PRICES_READ: &str = "2026-11-15";

/// The models whose prices differ from the defaults: (family, cache read, tier). Read from
/// platform.claude.com/docs/en/about-claude/pricing on 2026-11-15. Output is 5 times the base
/// input price for every current model, a cache write 1.25 times for the five-minute cache and 2
/// times for the one-hour cache. Cache reads ("cache hits and refreshes") are 0.1 times by
/// default. Every model not named here has its whole window at one price.
const PRICES: [(&str, f64, Option<Tier>); 7] = [
    ("claude-opus-5-5", 0.05, None),
    ("claude-sonnet-5-5", 0.05, None),
    ("claude-fable-5-2", 0.025, None),
    ("claude-fable-5-1", 0.025, None),
    ("claude-mythos-5-2", 0.025, None),
    ("claude-mythos-5-1", 0.025, None),
    ("claude-haiku-5-5", 0.1, Some(Tier { over: 100_000, times: 5.0 })),
];

/// The weights of a model, by its id as a transcript or a provider writes it:
/// `claude-opus-5-5`, `claude-opus-5-5-20261101`, `anthropic/claude-opus-5-5`,
/// `us.anthropic.claude-opus-5-5-v1:0`.
pub fn weights(model: &str) -> Weights {
    let id = model.to_ascii_lowercase();
    let Some(at) = id.find("claude-") else { return DEFAULT_WEIGHTS };
    for (family, cache_read, tier) in PRICES {
        // `claude-opus-5-5` names `claude-opus-5-5-20261101`, not a `claude-opus-5-50`.
        if let Some(rest) = id[at..].strip_prefix(family)
            && !rest.starts_with(|c: char| c.is_ascii_digit())
        {
            return Weights { cache_read, tier, ..DEFAULT_WEIGHTS };
        }
    }
    DEFAULT_WEIGHTS
}

impl Weights {
    /// How many times the base prices a request with a prompt of `prompt` tokens pays.
    pub fn times(&self, prompt: Option<u64>) -> f64 {
        match (self.tier, prompt) {
            (Some(t), Some(p)) if p > t.over => t.times,
            _ => 1.0,
        }
    }
}

// ----- what a compaction costs -----

/// What a compaction writes and leaves: the tokens of its summary, what the first request after
/// it sends as new, and the context of that request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub summary: u64,
    pub resent: u64,
    pub left: u64,
}

impl Shape {
    /// Where there is no compaction to measure, after a start of `start` tokens.
    pub fn typical(start: Option<u64>) -> Shape {
        Shape { summary: SUMMARY_TOKENS, resent: RESENT_TOKENS, left: start.unwrap_or(START_TOKENS) + RESENT_TOKENS }
    }

    /// What compacting a conversation of `pre` tokens costs at the weights `w` (an estimate):
    /// one request reads the conversation from the cache and writes the summary, and the next
    /// sends what is left as new. `resend`: what a token sent as new weighs.
    pub fn cost(&self, w: &Weights, pre: u64, resend: f64) -> f64 {
        w.times(Some(pre)) * (w.cache_read * pre as f64 + w.output * self.summary as f64) + w.times(Some(self.left)) * resend * self.resent as f64
    }

    /// What each request after a compaction at `context` saves by reading less.
    pub fn saves(&self, w: &Weights, context: u64) -> f64 {
        w.cache_read * (w.times(Some(context)) * context as f64 - w.times(Some(self.left)) * self.left as f64)
    }

    /// After how many requests a compaction at `context` has paid for itself by the reads it
    /// saves. `None` where it saves nothing.
    pub fn pays_after(&self, w: &Weights, context: u64, resend: f64) -> Option<f64> {
        let saves = self.saves(w, context);
        (saves > 0.0).then(|| self.cost(w, context, resend) / saves)
    }
}

/// The length from which a hook speaks of carrying a conversation: where the price of the model
/// steps up, or else where a compaction of the usual shape would pay for itself within
/// [`PAYBACK`] requests. With a one-hour cache that is 170,000 tokens at the default prices,
/// 270,000 where a cache read is 0.05 and 450,000 where it is 0.025.
pub fn carry_from(w: &Weights, resend: f64) -> u64 {
    if let Some(tier) = w.tier {
        return tier.over;
    }
    let shape = Shape::typical(None);
    // `cost(c) = PAYBACK * saves(c)`, solved for c.
    let fixed = w.output * shape.summary as f64 + resend * shape.resent as f64;
    let c = (fixed + PAYBACK * w.cache_read * shape.left as f64) / (w.cache_read * (PAYBACK - 1.0));
    (c / 10_000.0).round() as u64 * 10_000
}

const USAGE: &str = "Where the tokens of this project's sessions went, read from the harnesses' own transcripts on
this machine (Claude Code and Codex, one report each), and which habits cost the most.
Nothing is run, changed or sent.

Usage: codefast doctor sessions [--days N] [--session ID] [--carry N] [--all] [--json]
  --days N       the sessions whose transcript changed in the last N days (default 14)
  --session ID   one session, named by the start of its id, with each finding broken down
  --carry N      the length, in tokens, at which an earlier compaction is tried (default:
                 several from 150000 to 800000, of which the report names the longest that
                 gains nearly as much as the best, and for a model whose price steps up
                 the length of that step)
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

    /// What one token this request wrote to the cache weighs at the default prices: 1.25 for
    /// the five-minute cache, 2 for the one-hour cache, and 1.25 when the transcript does not say.
    #[cfg(test)]
    pub fn write_weight(&self) -> f64 {
        self.write_weight_in(&DEFAULT_WEIGHTS)
    }

    /// The same at the weights `w`, before any step in price.
    pub fn write_weight_in(&self, w: &Weights) -> f64 {
        let (short, long) = (self.write_5m.unwrap_or(0), self.write_1h.unwrap_or(0));
        if short + long == 0 { w.write_5m } else { (short as f64 * w.write_5m + long as f64 * w.write_1h) / (short + long) as f64 }
    }

    /// The request in weighted tokens at the default prices.
    #[cfg(test)]
    pub fn split(&self) -> Split {
        self.weigh(&DEFAULT_WEIGHTS)
    }

    /// The request in weighted tokens at the weights of its model, stepped up where its prompt
    /// is over the model's threshold.
    pub fn weigh(&self, w: &Weights) -> Split {
        let n = |v: Option<u64>| v.unwrap_or(0) as f64;
        let times = w.times(self.context());
        let written = self.cache_write.unwrap_or(0);
        let (short, long) = (self.write_5m.unwrap_or(0), self.write_1h.unwrap_or(0));
        // What the split leaves out of the total is written at the five-minute price.
        let rest = written.saturating_sub(short + long);
        let cache_write =
            if short + long == 0 { written as f64 * w.write_5m } else { short as f64 * w.write_5m + long as f64 * w.write_1h + rest as f64 * w.write_5m };
        Split {
            cache_read: times * n(self.cache_read) * w.cache_read,
            cache_write: times * cache_write,
            input: times * n(self.input) * INPUT,
            output: times * n(self.output) * w.output,
        }
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
    /// `post`: the context of the first request after it, where there is one. `summary`: the
    /// bytes of the summary it wrote.
    Compaction {
        at: Option<i64>,
        trigger: String,
        pre: Option<u64>,
        post: Option<u64>,
        summary: Option<u64>,
    },
    Fallback {
        from: String,
        to: String,
    },
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
    /// Sub-agents with a transcript of their own: the requests and compactions of each, in
    /// order.
    pub agents: Vec<Vec<Event>>,
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
#[derive(Debug, Clone, PartialEq)]
pub struct Last {
    /// What the next request will send before anything new: the last request's context and
    /// its reply.
    pub context: u64,
    /// Milliseconds since the epoch.
    pub at: i64,
    /// Whether the conversation is written to the one-hour cache, where the transcript says.
    pub one_hour: Option<bool>,
    /// The model of that request, as the transcript writes it: [`weights`] gives its prices.
    pub model: String,
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
                None => {
                    let model = v["message"]["model"].as_str().unwrap_or("").to_string();
                    found = Some(Last { context: context + u.output.unwrap_or(0), at, one_hour: which, model })
                }
                Some(last) => last.one_hour = last.one_hour.or(which),
            }
            if found.as_ref().is_some_and(|l| l.one_hour.is_some()) {
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
        // A sub-agent's line written into the main transcript. In a sub-agent's own transcript
        // every line is the agent's, whose conversation it is.
        let sidechain = !sub && v["isSidechain"] == true;
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
                if v["isCompactSummary"] == true
                    && let Some(Event::Compaction { summary, .. }) = t.events.last_mut()
                {
                    *summary = summary.or(Some(result_text(&v["message"]["content"]).len() as u64));
                }
                // Tool results are followed in the main thread only.
                for block in v["message"]["content"].as_array().into_iter().flatten().filter(|b| !sub && b["type"] == "tool_result") {
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
                        summary: None,
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
            measure_compactions(&mut s);
            if s.events.iter().any(|e| matches!(e, Event::Request(_))) {
                t.agents.push(s.events);
            }
        }
        measure_compactions(&mut t);
        t
    }
}

/// The sizes either side of each compaction, from the main-thread requests around it: the
/// context of the last one before it where the harness did not record it, and the context of
/// the first one after it, which is what the compaction left.
fn measure_compactions(t: &mut Transcript) {
    let context = |e: &Event| match e {
        Event::Request(r) if !r.sidechain => r.usage.context(),
        _ => None,
    };
    for i in 0..t.events.len() {
        if let Event::Compaction { .. } = t.events[i] {
            let before = t.events[..i].iter().rev().find_map(context);
            // Up to the next compaction: a session that ends with one has no request after it.
            let after = t.events[i + 1..].iter().take_while(|e| !matches!(e, Event::Compaction { .. })).find_map(context);
            if let Event::Compaction { pre, post, .. } = &mut t.events[i] {
                *pre = pre.or(before);
                *post = after.or(*post);
            }
        }
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
                (k, i) if k.contains("compact") || i.contains("compact") => {
                    let summary = p["message"].as_str().map(|m| m.len() as u64);
                    match t.events.last_mut() {
                        // The rollout may say it twice (an event and an item): once is a boundary.
                        Some(Event::Compaction { summary: known, .. }) => *known = known.or(summary),
                        _ => t.events.push(Event::Compaction { at, trigger: p["trigger"].as_str().unwrap_or("").to_string(), pre: None, post: None, summary }),
                    }
                }
                _ => {}
            }
        }
        // A compaction's sizes are not recorded: the context of the request before it and of
        // the one after stand in for them.
        measure_compactions(&mut t);
        t
    }
}

// ----- the arithmetic -----

/// What a tool result costs while the conversation carries it: written to the cache by the
/// first request after it, read from the cache by each one after that (`n` requests in all).
#[cfg(test)]
pub fn carried(bytes: u64, n: usize) -> f64 {
    bytes as f64 / BYTES_PER_TOKEN * (CACHE_WRITE_5M + CACHE_READ * n.saturating_sub(1) as f64)
}

/// What a token sent as new weighs in a session, before any step in price: its cache writes
/// and its uncached input, each at the weight of its kind.
pub fn resend_weight(main: &[&Request], w: &Weights) -> f64 {
    let (mut tokens, mut weighted) = (0.0, 0.0);
    for r in main {
        let (written, input) = (r.usage.cache_write.unwrap_or(0) as f64, r.usage.input.unwrap_or(0) as f64);
        tokens += written + input;
        weighted += written * r.usage.write_weight_in(w) + input * INPUT;
    }
    if tokens > 0.0 { weighted / tokens } else { w.write_5m }
}

/// The tokens of a compaction's summary: by its bytes where the transcript has it, and else
/// the usual size, or an eighth of a conversation too short for that.
fn summary_tokens(bytes: Option<u64>, pre: Option<u64>) -> u64 {
    match bytes {
        Some(b) => (b as f64 / BYTES_PER_TOKEN) as u64,
        None => SUMMARY_TOKENS.min(pre.unwrap_or(0) / 8),
    }
}

/// What a compaction writes and leaves in a session: the middle of its own compactions, and
/// the usual shape for what none of them recorded.
pub fn shape_of(events: &[Event]) -> Shape {
    let main = |e: &Event| match e {
        Event::Request(r) if !r.sidechain => Some(r.usage),
        _ => None,
    };
    // The start of a session that was resumed is the whole conversation: not what stays.
    let usual = Shape::typical(events.iter().find_map(main).and_then(|u| u.context()).map(|c| c.min(100_000)));
    let (mut summaries, mut resents, mut lefts) = (Vec::new(), Vec::new(), Vec::new());
    for (i, e) in events.iter().enumerate() {
        let Event::Compaction { summary, pre, .. } = e else { continue };
        if summary.is_some() {
            summaries.push(summary_tokens(*summary, *pre));
        }
        if let Some(u) = events[i + 1..].iter().take_while(|e| !matches!(e, Event::Compaction { .. })).find_map(main) {
            lefts.extend(u.context());
            resents.push(u.cache_write.unwrap_or(0) + u.input.unwrap_or(0));
        }
    }
    Shape { summary: median(summaries).unwrap_or(usual.summary), resent: median(resents).unwrap_or(usual.resent), left: median(lefts).unwrap_or(usual.left) }
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
#[cfg(test)]
pub fn cold_cost(prev: &Usage, cur: &Usage, uncached: bool) -> Option<f64> {
    cold_cost_in(prev, cur, uncached, &DEFAULT_WEIGHTS)
}

/// The same at the weights of `cur`'s model, at the price its prompt pays.
pub fn cold_cost_in(prev: &Usage, cur: &Usage, uncached: bool, w: &Weights) -> Option<f64> {
    let before = prev.context()?;
    let read = cur.cache_read?;
    if before <= COLD_FROM || read as f64 >= COLD_SHARE * before as f64 {
        return None;
    }
    let times = w.times(cur.context());
    let written = before.min(cur.cache_write.unwrap_or(0));
    let mut cost = (cur.write_weight_in(w) - w.cache_read) * written as f64;
    if uncached {
        cost += (INPUT - w.cache_read) * (before - written).min(cur.input.unwrap_or(0)) as f64;
    }
    Some(times * cost)
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
    /// The context of the first request after it.
    pub post: Option<u64>,
    /// The tokens of the summary it wrote, and what the first request after it sent as new.
    pub summary: u64,
    pub resent: Option<u64>,
    /// The conversation read and the summary written by a request the transcript does not
    /// record (an estimate), and what the next request sent as new (measured).
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
    /// The thinking tokens in weighted tokens, at the output price of each request's model.
    pub weighted: f64,
}

/// Requests whose prompt was over the length where their model's price steps up.
#[derive(Debug, Clone, Default)]
pub struct Step {
    pub requests: usize,
    /// What they cost, and what they would have cost at the lower price.
    pub cost: f64,
    pub lower: f64,
    /// The models, and the length where the price steps.
    pub models: BTreeSet<String>,
    pub over: u64,
    /// Compacting at that length instead, in the sessions of those models.
    pub window: Window,
}

/// Sub-agents with a transcript of their own, each looked at as the conversation it is.
#[derive(Debug, Clone, Default)]
pub struct Agents {
    pub count: usize,
    /// Those that wrote to the five-minute cache and never to the one-hour one.
    pub five_minutes: usize,
    /// Their requests that found the cache cold.
    pub cold: Vec<Cold>,
    /// The context they carried, and what compacting it earlier would have done.
    pub carry: Carry,
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
    pub step: Step,
    pub agents: Agents,
    /// `--carry`, when it was given.
    pub carry_given: Option<u64>,
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

/// Compacting at one length instead of where the sessions did.
#[derive(Debug, Clone, Default)]
pub struct Window {
    pub threshold: u64,
    /// The main-thread requests with a known context of the sessions it was tried on, those
    /// that carried more than it, and what reading the part above it cost.
    pub requests: usize,
    pub above: usize,
    pub cost: f64,
    /// The compactions more that keeping the context under it would have taken, by
    /// [`sawtooth`], and what they would have cost (an estimate).
    pub compactions: u64,
    pub instead: f64,
    /// What they would have saved: each request without the context it would not have carried,
    /// at the price of the length it would then have had (an estimate).
    pub saved: f64,
    /// Where a compaction leaves no less than this length: the most one left.
    pub left_above: u64,
}

impl Window {
    /// What compacting at this length would have saved, less what the compactions cost.
    pub fn gain(&self) -> f64 {
        self.saved - self.instead
    }

    /// One request that carried `c` tokens and cost `base` before any step in price (`times`):
    /// what it read above the length, and what it would have cost after compactions of `shape`
    /// at the length. `run`: whether the stretch has passed the length, and the compactions
    /// counted in it.
    #[allow(clippy::too_many_arguments)]
    fn request(&mut self, c: u64, base: f64, times: f64, w: &Weights, shape: &Shape, resend: f64, run: &mut (bool, u64)) {
        self.requests += 1;
        if c > self.threshold {
            self.above += 1;
            self.cost += times * w.cache_read * (c - self.threshold) as f64;
            run.0 = true;
        }
        if !run.0 {
            return;
        }
        if shape.left >= self.threshold {
            // A compaction that leaves this much does not bring the conversation under it.
            self.left_above = self.left_above.max(shape.left);
            return;
        }
        let (would, n) = sawtooth(c, self.threshold, shape.left);
        if n > run.1 {
            self.compactions += n - run.1;
            self.instead += (n - run.1) as f64 * shape.cost(w, self.threshold, resend);
            run.1 = n;
        }
        // The same request with less to read, at the price of the length it would then have had.
        self.saved += times * base - w.times(Some(would)) * (base - w.cache_read * (c - would) as f64);
    }

    fn merge(&mut self, o: &Window) {
        self.threshold = self.threshold.max(o.threshold);
        self.requests += o.requests;
        self.above += o.above;
        self.cost += o.cost;
        self.compactions += o.compactions;
        self.instead += o.instead;
        self.saved += o.saved;
        self.left_above = self.left_above.max(o.left_above);
    }
}

/// The context the main thread carried, and what compacting it earlier would have done.
#[derive(Debug, Clone, Default)]
pub struct Carry {
    /// The largest context of a main-thread request.
    pub peak: u64,
    /// The lengths tried, shortest first. The sessions of a model whose price steps are not
    /// among these unless `--carry` was given: [`Step::window`] has them.
    pub windows: Vec<Window>,
}

impl Carry {
    fn window(&mut self, threshold: u64) -> &mut Window {
        let i = match self.windows.binary_search_by_key(&threshold, |w| w.threshold) {
            Ok(i) => i,
            Err(i) => {
                self.windows.insert(i, Window { threshold, ..Default::default() });
                i
            }
        };
        &mut self.windows[i]
    }

    /// The length with the largest gain, where one gains anything.
    pub fn cheapest(&self) -> Option<&Window> {
        self.windows.iter().filter(|w| w.gain() > 0.0).max_by(|a, b| a.gain().total_cmp(&b.gain()))
    }

    /// The length the report speaks of: the longest one that gains at least [`KEEP`] of what
    /// the best gains. Where none gains, the one length that was asked for, if any request
    /// carried more.
    pub fn featured(&self) -> Option<&Window> {
        match self.cheapest() {
            Some(best) => self.windows.iter().rev().find(|w| w.gain() >= KEEP * best.gain()),
            None => self.windows.first().filter(|w| self.windows.len() == 1 && w.above > 0),
        }
    }

    /// What the finding is ranked by: what the length spoken of would have gained, or else
    /// what reading the part above it cost.
    pub fn cost(&self) -> f64 {
        self.featured().map_or(0.0, |w| if w.gain() > 0.0 { w.gain() } else { w.cost })
    }
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

/// The middle of some numbers.
pub fn median(mut v: Vec<u64>) -> Option<u64> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let n = v.len();
    Some(if n % 2 == 1 { v[n / 2] } else { (v[n / 2 - 1] + v[n / 2]) / 2 })
}

/// A tool result the conversation carries: how many requests have carried it so far and what
/// that cost, and whether it repeats an earlier one.
struct Open {
    result: ToolResult,
    seen: usize,
    cost: f64,
    repeat: bool,
}

impl Stats {
    /// `carry`: the threshold of `--carry`, when given.
    pub fn new(kind: Kind, carry: Option<u64>) -> Stats {
        Stats { kind, carry_given: carry, ..Default::default() }
    }

    /// The weights of a model in this harness. Codex providers price differently, so every Codex
    /// model is weighed by the defaults.
    pub fn weights(&self, model: &str) -> Weights {
        match self.kind {
            Kind::ClaudeCode => weights(model),
            Kind::Codex => DEFAULT_WEIGHTS,
        }
    }

    pub fn total(&self) -> f64 {
        self.main.total() + self.sub.total()
    }

    fn count(&mut self, r: &Request) -> Split {
        let w = self.weights(&r.model);
        let split = r.usage.weigh(&w);
        let m = self.models.entry(r.model.clone()).or_default();
        m.0 += 1;
        m.1 += split.total();
        let times = w.times(r.usage.context());
        self.thinking.requests += 1;
        if let (Some(t), Some(o)) = (r.usage.thinking, r.usage.output) {
            self.thinking.recorded += 1;
            self.thinking.thinking += t;
            self.thinking.output += o;
            self.thinking.weighted += times * w.output * t as f64;
        }
        if let (Some(tier), true) = (w.tier, times > 1.0) {
            self.step.requests += 1;
            self.step.cost += split.total();
            self.step.lower += split.total() / times;
            self.step.models.insert(r.model.clone());
            self.step.over = tier.over;
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
        self.thinking.weighted
    }

    pub fn repeat_cost(&self) -> f64 {
        self.repeats.iter().map(|r| r.cost).sum()
    }

    /// What the step in price cost: the requests over it, less what they would have cost below.
    pub fn step_cost(&self) -> f64 {
        self.step.cost - self.step.lower
    }

    /// What a result given back twice is called: a read in Claude Code, a call's output in Codex.
    pub fn repeat_key(&self) -> &'static str {
        match self.kind {
            Kind::ClaudeCode => "repeated_reads",
            Kind::Codex => "repeated_outputs",
        }
    }

    /// The findings with their weighted cost, largest first, without those under 1% of the total.
    pub fn findings(&self) -> Vec<(&'static str, f64)> {
        let mut v = vec![
            ("start", self.start()),
            ("cold_restarts", self.cold_cost()),
            ("thinking", self.thinking_cost()),
            ("manual_compactions", self.compaction_cost(true)),
            ("automatic_compactions", self.compaction_cost(false)),
            (self.repeat_key(), self.repeat_cost()),
            ("carried_context", self.carry.cost()),
            ("agent_cold_restarts", self.agents.cold.iter().map(|c| c.cost).sum()),
            ("agent_carried_context", self.agents.carry.cost()),
            ("price_step", if self.step.requests > 0 { self.step_cost() } else { 0.0 }),
        ];
        let total = self.total();
        v.retain(|(_, cost)| total > 0.0 && *cost >= WORTH_NAMING * total);
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        v
    }

    /// Add the numbers of `o`, of the same harness, for a total.
    pub fn merge(&mut self, o: &Stats) {
        self.sessions += o.sessions;
        self.requests += o.requests;
        self.main.add(&o.main);
        self.sub.add(&o.sub);
        self.sub_requests += o.sub_requests;
        self.unknown_context += o.unknown_context;
        self.stretches.extend(o.stretches.iter().cloned());
        self.cold.extend(o.cold.iter().cloned());
        self.compactions.extend(o.compactions.iter().cloned());
        self.repeats.extend(o.repeats.iter().cloned());
        self.thinking.requests += o.thinking.requests;
        self.thinking.recorded += o.thinking.recorded;
        self.thinking.thinking += o.thinking.thinking;
        self.thinking.output += o.thinking.output;
        self.thinking.weighted += o.thinking.weighted;
        for (m, (n, w)) in &o.models {
            let e = self.models.entry(m.clone()).or_default();
            e.0 += n;
            e.1 += w;
        }
        self.carry.peak = self.carry.peak.max(o.carry.peak);
        for w in &o.carry.windows {
            self.carry.window(w.threshold).merge(w);
        }
        self.step.window.merge(&o.step.window);
        self.agents.count += o.agents.count;
        self.agents.five_minutes += o.agents.five_minutes;
        self.agents.cold.extend(o.agents.cold.iter().cloned());
        self.agents.carry.peak = self.agents.carry.peak.max(o.agents.carry.peak);
        for w in &o.agents.carry.windows {
            self.agents.carry.window(w.threshold).merge(w);
        }
        self.step.requests += o.step.requests;
        self.step.cost += o.step.cost;
        self.step.lower += o.step.lower;
        self.step.models.extend(o.step.models.iter().cloned());
        self.step.over = self.step.over.max(o.step.over);
    }

    fn keep_largest(&mut self, c: Carried) {
        self.largest.push(c);
        self.largest.sort_by(|a, b| b.cost.total_cmp(&a.cost));
        self.largest.truncate(3);
    }

    /// The end of a stretch: its start, and the tool results it carried. `w`: the weights of its
    /// last request, for a result no request carried.
    fn close(&mut self, session: &str, first: Option<u64>, k: usize, start: f64, open: &mut Vec<Open>, w: &Weights) {
        if let (Some(first), true) = (first, k > 0) {
            self.stretches.push(Stretch { session: session.to_string(), first, requests: k, cost: start });
        }
        for o in open.drain(..) {
            // A result after the last request of the stretch is written once all the same.
            let cost = if o.seen == 0 { o.result.bytes as f64 / BYTES_PER_TOKEN * w.write_5m } else { o.cost };
            let r = o.result;
            let g = self.groups.entry(r.group.clone()).or_default();
            g.results += 1;
            g.bytes += r.bytes;
            g.cost += cost;
            let c = Carried { session: session.to_string(), group: r.group, path: r.path, bytes: r.bytes, cost };
            if o.repeat {
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
        let main: Vec<&Request> = t.events.iter().filter_map(|e| if let Event::Request(r) = e { (!r.sidechain).then_some(r) } else { None }).collect();
        // The model of most of the session's requests: a session of a model whose price steps
        // is measured against that step, unless `--carry` says otherwise.
        let mut by_model: BTreeMap<&str, usize> = BTreeMap::new();
        for r in &main {
            *by_model.entry(r.model.as_str()).or_default() += 1;
        }
        let model = by_model.iter().max_by_key(|(_, n)| **n).map(|(m, _)| *m).unwrap_or("");
        let resend = resend_weight(&main, &self.weights(model));
        // What a compaction writes and leaves in this session.
        let shape = shape_of(&t.events);
        // The lengths an earlier compaction is tried at: the one asked for, or the step of a
        // model whose price steps (which goes with that finding), or the usual ones.
        let step = self.weights(model).tier.filter(|_| self.carry_given.is_none());
        let tried: Vec<u64> = match (self.carry_given, step) {
            (Some(n), _) => vec![n],
            (None, Some(tier)) => vec![tier.over],
            (None, None) => WINDOWS.to_vec(),
        };
        // For each: whether the stretch has passed it, and the compactions counted in it.
        let mut runs = vec![(false, 0u64); tried.len()];
        let mut prev: Option<&Request> = None;
        let mut compacted = false;
        let (mut first, mut k, mut start) = (None, 0usize, 0.0f64);
        let (mut requests, mut first_at, mut last_at) = (0usize, None, None);
        let mut open: Vec<Open> = Vec::new();
        let mut reads: HashSet<(String, u64)> = HashSet::new();
        let mut last_w = self.weights(model);
        for e in &t.events {
            match e {
                Event::Request(r) if r.sidechain => self.sub_request(r),
                Event::Request(r) => {
                    let w = self.weights(&r.model);
                    last_w = w;
                    let times = w.times(r.usage.context());
                    let split = self.count(r);
                    self.requests += 1;
                    requests += 1;
                    self.main.add(&split);
                    if compacted && let Some(c) = self.compactions.last_mut() {
                        // What the compaction left is sent as new by the first request after it.
                        c.resent = Some(r.usage.cache_write.unwrap_or(0) + r.usage.input.unwrap_or(0));
                        c.cost += split.cache_write + split.input;
                    }
                    match r.usage.context() {
                        None => self.unknown_context += 1,
                        Some(c) => {
                            self.carry.peak = self.carry.peak.max(c);
                            let base = split.total() / times;
                            for (threshold, run) in tried.iter().zip(runs.iter_mut()) {
                                let window = if step.is_some() { &mut self.step.window } else { self.carry.window(*threshold) };
                                window.threshold = *threshold;
                                window.request(c, base, times, &w, &shape, resend, run);
                            }
                        }
                    }
                    first_at = first_at.or(r.at);
                    last_at = r.at.or(last_at);
                    if let Some(p) = prev {
                        if p.model != r.model {
                            self.switches += 1;
                        }
                        if let Some(cost) = cold_cost_in(&p.usage, &r.usage, self.kind == Kind::Codex, &w).filter(|_| !compacted) {
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
                    } else if let Some(f) = first {
                        start += times * w.cache_read * f as f64;
                    }
                    // Each tool result is written to the cache by the first request after it,
                    // and read from it by every one after that.
                    for o in open.iter_mut() {
                        let weight = if o.seen == 0 { w.write_5m } else { w.cache_read };
                        o.cost += o.result.bytes as f64 / BYTES_PER_TOKEN * weight * times;
                        o.seen += 1;
                    }
                    k += 1;
                    compacted = false;
                    prev = Some(r);
                }
                Event::Compaction { at, trigger, pre, post, summary } => {
                    self.close(&t.id, first, k, start, &mut open, &last_w);
                    (first, k, start) = (None, 0, 0.0);
                    runs.fill((false, 0));
                    reads.clear();
                    compacted = true;
                    let tokens = summary_tokens(*summary, *pre);
                    self.compactions.push(Compaction {
                        session: t.id.clone(),
                        at: *at,
                        manual: trigger == "manual",
                        pre: *pre,
                        post: *post,
                        summary: tokens,
                        resent: None,
                        cost: last_w.times(*pre) * (last_w.cache_read * pre.unwrap_or(0) as f64 + last_w.output * tokens as f64),
                    });
                }
                Event::Fallback { .. } => {}
                Event::ToolResult(r) => {
                    let repeat = r.repeat.as_ref().is_some_and(|k| !reads.insert((k.clone(), r.hash)));
                    open.push(Open { result: r.clone(), seen: 0, cost: 0.0, repeat });
                }
            }
        }
        self.close(&t.id, first, k, start, &mut open, &last_w);
        for agent in &t.agents {
            self.agent(&t.id, agent);
        }
        self.per_session.push(Session { id: t.id.clone(), weighted: self.total() - before, requests, first: first_at, last: last_at });
    }

    /// One sub-agent with a transcript of its own. Its requests count as the sub-agents' do,
    /// and its conversation is looked at as the main thread's is: for requests that found the
    /// cache cold, and for what an earlier compaction would have done.
    fn agent(&mut self, session: &str, events: &[Event]) {
        let own: Vec<&Request> = events.iter().filter_map(|e| if let Event::Request(r) = e { Some(r) } else { None }).collect();
        let mut by_model: BTreeMap<&str, usize> = BTreeMap::new();
        for r in &own {
            *by_model.entry(r.model.as_str()).or_default() += 1;
        }
        let model = by_model.iter().max_by_key(|(_, n)| **n).map(|(m, _)| *m).unwrap_or("");
        let resend = resend_weight(&own, &self.weights(model));
        let shape = shape_of(events);
        let step = self.weights(model).tier.filter(|_| self.carry_given.is_none());
        let tried: Vec<u64> = match (self.carry_given, step) {
            (Some(n), _) => vec![n],
            (None, Some(tier)) => vec![tier.over],
            (None, None) => WINDOWS.to_vec(),
        };
        let mut runs = vec![(false, 0u64); tried.len()];
        self.agents.count += 1;
        // Which cache it writes to decides how long it may wait.
        let written = |pick: fn(&Usage) -> Option<u64>| own.iter().map(|r| pick(&r.usage).unwrap_or(0)).sum::<u64>();
        let hour = written(|u| u.write_1h) > 0;
        if !hour && written(|u| u.write_5m) > 0 {
            self.agents.five_minutes += 1;
        }
        let mut prev: Option<&Request> = None;
        let mut compacted = false;
        for e in events {
            match e {
                Event::Request(r) => {
                    let w = self.weights(&r.model);
                    let times = w.times(r.usage.context());
                    let split = self.count(r);
                    self.sub_requests += 1;
                    self.sub.add(&split);
                    if let Some(c) = r.usage.context() {
                        self.agents.carry.peak = self.agents.carry.peak.max(c);
                        let base = split.total() / times;
                        for (threshold, run) in tried.iter().zip(runs.iter_mut()) {
                            let window = if step.is_some() { &mut self.step.window } else { self.agents.carry.window(*threshold) };
                            window.threshold = *threshold;
                            window.request(c, base, times, &w, &shape, resend, run);
                        }
                    }
                    if let Some(p) = prev
                        && let Some(cost) = cold_cost_in(&p.usage, &r.usage, self.kind == Kind::Codex, &w).filter(|_| !compacted)
                    {
                        self.agents.cold.push(Cold {
                            session: session.to_string(),
                            at: r.at,
                            cause: cause(p, r, hour),
                            fallback: false,
                            before: p.usage.context().unwrap_or(0),
                            gap_ms: p.at.zip(r.at).map(|(a, b)| b - a),
                            cost,
                            reason: r.miss.clone(),
                        });
                    }
                    compacted = false;
                    prev = Some(r);
                }
                Event::Compaction { .. } => {
                    runs.fill((false, 0));
                    compacted = true;
                }
                Event::Fallback { .. } | Event::ToolResult(_) => {}
            }
        }
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

/// What compacting at one length instead would have done, in a line.
fn compacting(w: &Window) -> String {
    let t = short(w.threshold as f64);
    let n = format!("{} more compaction{} for about {}", count(w.compactions as usize), if w.compactions == 1 { "" } else { "s" }, short(w.instead));
    if w.compactions == 0 && w.left_above > 0 {
        format!(
            "A compaction leaves about {} tokens here, which is no shorter: a fresh session at a natural break is what brings a conversation under {t}.",
            short(w.left_above as f64)
        )
    } else if w.gain() > 0.0 {
        format!(
            "Compacting at {t} instead: {n}, and about {} less to pay for after them, so about {} less in all (an estimate).",
            short(w.saved),
            short(w.gain())
        )
    } else {
        format!("Compacting at {t} instead: {n}, which would not have paid (an estimate).")
    }
}

/// Cold requests by cause: `1 model switch (707k), 2 idle for more than an hour (1.4M)`.
fn by_cause(cold: &[Cold]) -> String {
    let mut parts = Vec::new();
    for c in Cause::ALL {
        let of: Vec<&Cold> = cold.iter().filter(|x| x.cause == c).collect();
        if of.is_empty() {
            continue;
        }
        let fell = of.iter().filter(|x| x.fallback).count();
        let note = if fell > 0 { format!(", {fell} after a model fallback") } else { String::new() };
        // Several switches, and several requests that were idle.
        let words = if c == Cause::ModelSwitch && of.len() > 1 { "model switches" } else { c.words() };
        parts.push(format!("{} {words} ({}{note})", of.len(), short(of.iter().map(|x| x.cost).sum())));
    }
    parts.join(", ")
}

/// One cold request, for the breakdown of a session.
fn cold_item(x: &Cold) -> String {
    let fell = if x.fallback { " (a model fallback)" } else { "" };
    let theirs = x.reason.as_deref().map(|r| format!(" (harness: {r})")).unwrap_or_default();
    format!("  {}  {}{fell}{theirs} after {}, {} tokens sent again: {}", when(x.at), x.cause.words(), minutes(x.gap_ms), short(x.before as f64), short(x.cost))
}

/// The context some conversations carried and what compacting it earlier would have done:
/// the main thread's, or the sub-agents'.
fn carried_finding(carry: &Carry, title: &str, of: &str, advice: &str, push: &mut dyn FnMut(bool, &str)) {
    let Some(w) = carry.featured() else { return };
    let t = short(w.threshold as f64);
    let read = if w.gain() > 0.0 { String::new() } else { format!(" Reading the part above {t} again cost {}.", short(w.cost)) };
    push(
        true,
        &format!(
            "{title}: {} of {} {of} ({}) carried more than {t} tokens, at most {}.{read}",
            count(w.above),
            count(w.requests),
            share(w.above as f64, w.requests as f64),
            short(carry.peak as f64)
        ),
    );
    push(false, &compacting(w));
    if w.gain() > 0.0 {
        // Where a shorter length gains a tenth more, at the price of more compactions.
        if let Some(best) = carry.cheapest().filter(|b| b.threshold != w.threshold && w.gain() < 0.9 * b.gain()) {
            push(
                false,
                &format!(
                    "At {}: {} more compactions, about {} less in all.",
                    short(best.threshold as f64),
                    count(best.compactions as usize),
                    short(best.gain())
                ),
            );
        }
        push(false, advice);
    }
}

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
            let n = s.cold.len();
            let other = s.harness_disagrees();
            let clause = if other * 4 > n { format!(", and the harness names another cause for {other} of them") } else { String::new() };
            push(
                true,
                &format!(
                    "Cold restarts: {n} request{} sent the conversation again at the price of new input: {}{clause}.",
                    if n == 1 { "" } else { "s" },
                    by_cause(&s.cold)
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
                s.cold.iter().take(ITEMS).for_each(|x| push(false, &cold_item(x)));
            }
        }
        "agent_cold_restarts" => {
            let cold = &s.agents.cold;
            let n = cold.len();
            push(
                true,
                &format!(
                    "Cold restarts in sub-agents: {n} request{} of sub-agents sent the agent's conversation again at the price of new input: {}.",
                    if n == 1 { "" } else { "s" },
                    by_cause(cold)
                ),
            );
            if s.agents.five_minutes > 0 && cold.iter().any(|x| x.cause == Cause::IdleFive) {
                push(
                    false,
                    "A sub-agent's prompt cache lasts five minutes here: one that waits longer on a command pays for its whole conversation again when it comes back.",
                );
                push(false, "Long builds and test runs cost less run by the main thread, or in the background with a look every few minutes.");
            }
            if detail {
                // The dearest first: a session can have hundreds of agents.
                let mut dearest: Vec<&Cold> = cold.iter().collect();
                dearest.sort_by(|a, b| b.cost.total_cmp(&a.cost));
                dearest.iter().take(ITEMS).for_each(|x| push(false, &cold_item(x)));
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
            push(
                true,
                &format!(
                    "{what}: {}, at {} tokens{early}. Each read the conversation, wrote a summary, and had the next request send what was left as new (an estimate).",
                    of.len(),
                    list(&at, 6)
                ),
            );
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
        "carried_context" => carried_finding(
            &s.carry,
            "Carried context",
            "requests",
            "Start a fresh session at a natural break, or compact earlier, when the context stays far above what the next step needs.",
            &mut push,
        ),
        "agent_carried_context" => carried_finding(
            &s.agents.carry,
            "Carried context in sub-agents",
            "requests of sub-agents",
            "A long job costs less given to several agents in turn, each told what the last one did, than to one agent that carries it all.",
            &mut push,
        ),
        "price_step" => {
            let st = &s.step;
            let models = st.models.iter().cloned().collect::<Vec<_>>().join(", ");
            let over = short(st.over as f64);
            push(
                true,
                &format!(
                    "Price step: {} request{} of {models} had a prompt over {over} tokens and cost {}, where the lower price gives {}: the step cost {}.",
                    count(st.requests),
                    if st.requests == 1 { "" } else { "s" },
                    short(st.cost),
                    short(st.lower),
                    short(s.step_cost())
                ),
            );
            push(false, &format!("Keep a conversation with this model under {over} tokens, by starting a fresh session or compacting before it gets there."));
            if st.window.above > 0 {
                push(false, &compacting(&st.window));
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

/// The weights of the models of a report, models with the same weights together:
/// `claude-fable-5-2: a cache read 0.025, a cache write 1.25 (2 for one hour), output 5`.
fn weights_said(s: &Stats) -> String {
    let mut groups: Vec<(Weights, Vec<&str>)> = Vec::new();
    for m in s.models.keys() {
        let w = s.weights(m);
        match groups.iter_mut().find(|(g, _)| *g == w) {
            Some((_, names)) => names.push(m),
            None => groups.push((w, vec![m])),
        }
    }
    groups
        .iter()
        .map(|(w, names)| {
            let tier = w.tier.map(|t| format!(", and {} times each over a prompt of {} tokens", t.times, short(t.over as f64))).unwrap_or_default();
            format!(
                "{}: a cache read {}, a cache write {} ({} for one hour), output {}{tier}",
                names.join(", "),
                w.cache_read,
                w.write_5m,
                w.write_1h,
                w.output
            )
        })
        .collect::<Vec<_>>()
        .join(". ")
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
            "(Weights follow each model's published prices of {PRICES_READ}, in its own uncached input tokens: {}. Output and thinking are the counts the transcript recorded. Read from {} on this machine.)\n",
            weights_said(s),
            ctx.show(dir)
        ),
        Kind::Codex => format!(
            "(Weighted by the defaults, since Codex providers price differently: in the model's own uncached input tokens, a cache read 0.1, a cache write 1.25, output 5. Output and thinking are the counts the transcript recorded, thinking as a part of output. Read from {} on this machine.)\n",
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
        let agents = match s.agents.count {
            0 => String::new(),
            1 => ", 1 agent with a transcript of its own".to_string(),
            n => format!(", {} agents with transcripts of their own", count(n)),
        };
        out.push_str(&format!(
            "  sub-agents  {:>7}  {} requests ({}){agents}: cache reads {}, cache writes {}, output {}\n",
            short(s.sub.total()),
            count(s.sub_requests),
            share(s.sub.total(), total),
            short(s.sub.cache_read),
            short(s.sub.cache_write),
            short(s.sub.output)
        ));
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
    let by_cause = |cold: &[Cold]| -> Vec<Value> {
        Cause::ALL
            .iter()
            .map(|c| {
                let of: Vec<&Cold> = cold.iter().filter(|x| x.cause == *c).collect();
                json!({"cause": c.key(), "count": of.len(), "weighted": round(of.iter().map(|x| x.cost).sum()), "after_fallback": of.iter().filter(|x| x.fallback).count()})
            })
            .collect()
    };
    let causes = by_cause(&s.cold);
    let compactions = |manual: bool| {
        let of: Vec<&Compaction> = s.compactions.iter().filter(|c| c.manual == manual).collect();
        let cost: f64 = of.iter().map(|c| c.cost).sum();
        json!({
            "count": of.len(),
            "early": of.iter().filter(|c| c.early()).count(),
            "pre_tokens": of.iter().map(|c| c.pre).collect::<Vec<_>>(),
            "post_tokens": of.iter().map(|c| c.post).collect::<Vec<_>>(),
            "summary_tokens": of.iter().map(|c| c.summary).collect::<Vec<_>>(),
            "resent_tokens": of.iter().map(|c| c.resent).collect::<Vec<_>>(),
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
    let window = |w: &Window| {
        json!({
            "threshold": w.threshold,
            "requests": w.requests,
            "requests_above": w.above,
            "share_of_requests": ratio(w.above as f64, w.requests as f64),
            "weighted": round(w.cost),
            "compactions_instead": w.compactions,
            "compactions_weighted_estimate": round(w.instead),
            "saved_reads_estimate": round(w.saved),
            "difference_estimate": w.gain().round() + 0.0,
            "a_compaction_leaves_above_threshold": if w.left_above > 0 { json!(w.left_above) } else { Value::Null },
        })
    };
    // The length spoken of, with every length tried beside it. Where none is spoken of, the
    // one asked for, or the first.
    let carry_json = |carry: &Carry| {
        let mut v = window(carry.featured().or(carry.windows.first()).unwrap_or(&Window { threshold: s.carry_given.unwrap_or(0), ..Default::default() }));
        v["peak_context"] = json!(carry.peak);
        v["share"] = ratio(carry.cost(), total);
        v["spoken_of"] = json!(carry.featured().is_some());
        v["lengths_tried"] = carry.windows.iter().map(window).collect();
        v
    };
    let carried_json = carry_json(&s.carry);
    let agents_cold: f64 = s.agents.cold.iter().map(|c| c.cost).sum();
    let mut out = json!({
        "sessions": s.sessions,
        "requests": s.requests,
        "weighted": split(&s.main),
        "total_weighted": round(total),
        "subagents": {
            "requests": s.sub_requests,
            "weighted": split(&s.sub),
            "share": ratio(s.sub.total(), total),
            "agents_with_transcripts": s.agents.count,
            "agents_on_the_five_minute_cache": s.agents.five_minutes,
            "cold_restarts": {"count": s.agents.cold.len(), "weighted": round(agents_cold), "share": ratio(agents_cold, total), "by_cause": by_cause(&s.agents.cold)},
            "carried_context": carry_json(&s.agents.carry),
        },
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
        "carried_context": carried_json,
        "price_step": {
            "requests": s.step.requests,
            "models": s.step.models,
            "over_tokens": if s.step.requests > 0 { json!(s.step.over) } else { Value::Null },
            "weighted": round(s.step.cost),
            "weighted_at_lower_price": round(s.step.lower),
            "step_weighted": round(s.step_cost()),
            "share": ratio(s.step_cost(), total),
            "compacting_at_the_step": if s.step.window.requests > 0 { window(&s.step.window) } else { Value::Null },
        },
        "weights": s.models.keys().map(|m| {
            let w = s.weights(m);
            (m.clone(), json!({"cache_read": w.cache_read, "cache_write_5m": w.write_5m, "cache_write_1h": w.write_1h, "output": w.output, "tier": w.tier.map(|t| json!({"over_tokens": t.over, "times": t.times}))}))
        }).collect::<serde_json::Map<String, Value>>(),
        "prices_read": PRICES_READ,
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
            .map(|c| json!({"session": c.session, "at": c.at.map(|m| when(Some(m))), "trigger": if c.manual { "manual" } else { "auto" }, "pre_tokens": c.pre, "post_tokens": c.post, "summary_tokens": c.summary, "resent_tokens": c.resent, "early": c.early(), "weighted_estimate": round(c.cost)}))
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
    /// `--carry`, when given.
    carry: Option<u64>,
    session: Option<String>,
    all: bool,
    json: bool,
}

fn options(args: &[String]) -> Result<Option<Options>> {
    let mut o = Options { days: 14, carry: None, session: None, all: false, json: false };
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
                o.carry = Some(v.parse().ok().filter(|n| *n > 0).ok_or_else(|| anyhow::anyhow!("--carry takes a number of tokens above 0, not `{v}`"))?);
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

fn stats(source: &dyn Transcripts, files: &[PathBuf], carry: Option<u64>) -> Stats {
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
            sum.merge(&s);
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
        // A compaction: one request reads the conversation and writes the summary, and the
        // next sends what is left as new.
        let shape = Shape { summary: 2_000, resent: 10_000, left: 30_000 };
        assert_eq!(shape.cost(&DEFAULT_WEIGHTS, 80_000, 2.0), 8_000.0 + 10_000.0 + 20_000.0);
        // Each request after it reads 50,000 tokens less: paid back in 7.6 requests.
        assert_eq!(shape.saves(&DEFAULT_WEIGHTS, 80_000), 5_000.0);
        assert!((shape.pays_after(&DEFAULT_WEIGHTS, 80_000, 2.0).unwrap() - 7.6).abs() < 1e-9);
        assert_eq!(shape.pays_after(&DEFAULT_WEIGHTS, 30_000, 2.0), None);
        // Where the price steps, a prompt over the step pays five times for everything.
        let haiku = weights("claude-haiku-5-5");
        assert_eq!(shape.cost(&haiku, 120_000, 2.0), 5.0 * (12_000.0 + 10_000.0) + 20_000.0);
        assert!((shape.saves(&haiku, 120_000) - 0.1 * (600_000.0 - 30_000.0)).abs() < 1e-6);
        // From where a hook speaks of carrying: where a compaction of the usual shape pays for
        // itself within fifteen requests, which follows the price of a cache read, or where the
        // price steps.
        assert_eq!(Shape::typical(None), Shape { summary: 6_000, resent: 50_000, left: 75_000 });
        assert_eq!(carry_from(&DEFAULT_WEIGHTS, 2.0), 170_000);
        assert_eq!(carry_from(&DEFAULT_WEIGHTS, 1.25), 150_000);
        assert_eq!(carry_from(&weights("claude-opus-5-5"), 2.0), 270_000);
        assert_eq!(carry_from(&weights("claude-fable-5-2"), 2.0), 450_000);
        assert_eq!(carry_from(&haiku, 2.0), 100_000);
        let at = |w: &Weights| Shape::typical(None).pays_after(w, carry_from(w, 2.0), 2.0).unwrap();
        assert!((at(&DEFAULT_WEIGHTS) - 15.0).abs() < 0.6 && (at(&weights("claude-fable-5-2")) - 15.0).abs() < 0.6);
        assert_eq!((median(vec![]), median(vec![9, 1, 5]), median(vec![4, 10])), (None, Some(5), Some(7)));
        assert_eq!((summary_tokens(Some(7_000), None), summary_tokens(None, Some(1_000_000)), summary_tokens(None, Some(16_000))), (2_000, 6_000, 2_000));
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
                Event::Compaction { at: None, trigger: "manual".into(), pre: Some(40_000), post: Some(5_010), summary: Some(7_000) },
                // Right after a compaction a cold cache is expected.
                req("a", 71, u(10, 0, 5_000, 100)),
                req("a", 72, u(10, 5_000, 100, 100)),
            ],
            ..Default::default()
        };
        let mut s = Stats::new(Kind::ClaudeCode, Some(200_000));
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
        // The compaction: 40,000 tokens read and a summary of 2,000 written, by a request the
        // transcript does not have, and 5,010 sent as new by the one after it.
        let c = &s.compactions[0];
        assert!(c.early());
        assert_eq!((c.summary, c.resent), (2_000, Some(5_010)));
        assert!((c.cost - (4_000.0 + 10_000.0 + 1.25 * 5_000.0 + 10.0)).abs() < 1e-6);
        // What this session's own compaction wrote and left is what another would.
        assert_eq!(shape_of(&t.events), Shape { summary: 2_000, resent: 5_010, left: 5_010 });
    }

    #[test]
    fn context_carried_above_the_threshold() {
        // A start of 25k, the context climbing from 150k to 600k in ten requests, each reading
        // the one before, and twenty more requests at 600k.
        let mut events = vec![req("a", 0, u(0, 0, 25_000, 10))];
        let mut before = 25_000;
        for i in 0..10u64 {
            let context = 150_000 + 50_000 * i;
            events.push(req("a", 1 + i as i64, u(0, before, context - before, 10)));
            before = context;
        }
        for i in 0..20 {
            events.push(req("a", 11 + i, u(0, 600_000, 0, 10)));
        }
        let t = Transcript { id: "s".into(), events, ..Default::default() };
        // No compaction to measure: the usual shape after this start.
        assert_eq!(shape_of(&t.events), Shape { summary: 6_000, resent: 50_000, left: 75_000 });
        let mut s = Stats::new(Kind::ClaudeCode, Some(200_000));
        s.add(&t);
        let c = s.carry.windows[0].clone();
        // Compacted at 200k down to 75k, the context repeats every 125k: requests of 250k to
        // 600k would have carried 125k, 175k, 100k, 150k, 75k, 125k, 175k and 100k. It wraps at
        // 250k, 350k, 450k and 600k.
        let would = [125, 175, 100, 150, 75, 125, 175, 100];
        for (i, w) in would.iter().enumerate() {
            assert_eq!(sawtooth(250_000 + 50_000 * i as u64, 200_000, 75_000).0, w * 1000);
        }
        assert_eq!((sawtooth(150_000, 200_000, 75_000), sawtooth(60_000, 200_000, 75_000)), ((150_000, 0), (60_000, 0)));
        let saved: u64 = would.iter().enumerate().map(|(i, w)| 250_000 + 50_000 * i as u64 - w * 1000).sum::<u64>() + 20 * 500_000;
        assert_eq!((c.requests, c.above, s.carry.peak, c.compactions, c.left_above), (31, 28, 600_000, 4, 0));
        assert!((c.cost - 0.1 * (50_000.0 * 36.0 + 20.0 * 400_000.0)).abs() < 1e-6);
        assert!((c.saved - 0.1 * saved as f64).abs() < 1e-3);
        // Each compaction: 200k read and a summary of 6k written, then 50k sent as new at the
        // 1.25 this session writes to its cache at.
        assert!((c.instead - 4.0 * (20_000.0 + 30_000.0 + 62_500.0)).abs() < 1e-6);
        assert!(s.cold.is_empty());
        // What the finding is ranked by is what the compactions would have gained.
        assert!((s.carry.cost() - (c.saved - c.instead)).abs() < 1e-6);
        let mut out = String::new();
        finding(&mut out, &s, "carried_context", s.carry.cost(), &Ctx::new(Path::new("/")), false);
        assert!(out.contains("Carried context: 28 of 31 requests (90%) carried more than 200k tokens, at most 600k.\n"), "{out}");
        assert!(out.contains("Compacting at 200k instead: 4 more compactions for about 450k, and about 1.2M less to pay for after them, so about 78"), "{out}");
        assert!(out.contains("Start a fresh session at a natural break"), "{out}");
        // Without the twenty requests at 600k the four compactions cost more than they save.
        let mut climb = Stats::new(Kind::ClaudeCode, Some(200_000));
        climb.add(&Transcript { id: "c".into(), events: t.events[..11].to_vec(), ..Default::default() });
        assert_eq!((climb.carry.windows[0].above, climb.carry.windows[0].compactions), (8, 4));
        // It is ranked by what reading the part above the length cost, and says so.
        assert!((climb.carry.cost() - 180_000.0).abs() < 1e-6);
        let mut out = String::new();
        finding(&mut out, &climb, "carried_context", climb.carry.cost(), &Ctx::new(Path::new("/")), false);
        assert!(out.contains("carried more than 200k tokens, at most 600k. Reading the part above 200k again cost 180k.\n"), "{out}");
        assert!(out.contains("Compacting at 200k instead: 4 more compactions for about 450k, which would not have paid (an estimate).\n"), "{out}");
        assert!(!out.contains("Start a fresh session"), "{out}");

        // A session with a compaction of its own, which wrote a summary of 10k tokens and left
        // 300k, of which the next request sent 280k as new: that is the shape of another one.
        let mut big = vec![
            req("a", 0, u(0, 0, 25_000, 10)),
            req("a", 1, u(0, 25_000, 415_000, 10)),
            Event::Compaction { at: None, trigger: "auto".into(), pre: Some(440_000), post: Some(300_000), summary: Some(35_000) },
            req("a", 2, u(0, 20_000, 280_000, 10)),
        ];
        let mut before = 300_000;
        for i in 1..=6u64 {
            let context = 300_000 + 50_000 * i;
            big.push(req("a", 2 + i as i64, u(0, before, context - before, 10)));
            before = context;
        }
        let big = Transcript { id: "b".into(), events: big, ..Default::default() };
        assert_eq!(shape_of(&big.events), Shape { summary: 10_000, resent: 280_000, left: 300_000 });
        let mut b = Stats::new(Kind::ClaudeCode, Some(450_000));
        b.add(&big);
        // What it cost: 440k read, 10k written as output, 280k sent as new.
        assert!((b.compactions[0].cost - (44_000.0 + 50_000.0 + 350_000.0)).abs() < 1e-6);
        assert_eq!(b.compactions[0].resent, Some(280_000));
        // Compacting at 450k down to 300k: 500k, 550k and 600k would have been 350k, 400k and
        // 300k, with two compactions that cost far more than the reads they save.
        let w = b.carry.windows[0].clone();
        assert_eq!((w.above, w.compactions), (3, 2));
        assert!((w.saved - 0.1 * (150_000.0 + 150_000.0 + 300_000.0)).abs() < 1e-3);
        assert!((w.instead - 2.0 * (45_000.0 + 50_000.0 + 350_000.0)).abs() < 1e-6);
        let mut out = String::new();
        finding(&mut out, &b, "carried_context", b.carry.cost(), &Ctx::new(Path::new("/")), false);
        assert!(out.contains("Compacting at 450k instead: 2 more compactions for about 890k, which would not have paid (an estimate).\n"), "{out}");
        // A threshold under what a compaction leaves: no compaction brings the conversation there.
        let mut under = Stats::new(Kind::ClaudeCode, Some(250_000));
        under.add(&big);
        assert_eq!((under.carry.windows[0].compactions, under.carry.windows[0].left_above), (0, 300_000));
        let mut out = String::new();
        finding(&mut out, &under, "carried_context", under.carry.cost(), &Ctx::new(Path::new("/")), false);
        assert!(
            out.contains("A compaction leaves about 300k tokens here, which is no shorter: a fresh session at a natural break is what brings a conversation under 250k.\n"),
            "{out}"
        );
        let mut low = Stats::new(Kind::ClaudeCode, Some(700_000));
        low.add(&t);
        assert_eq!((low.carry.windows[0].above, low.carry.cost()), (0, 0.0));
        assert!(low.carry.featured().is_none() && !low.findings().iter().any(|(k, _)| *k == "carried_context"));

        // No length asked for: several are tried. Compacting at 150k, 200k, 300k, 400k and
        // 500k would have taken 7, 4, 2, 1 and 1 compactions and gained 567.5k, 787.5k, 835k,
        // 647.5k and 792.5k. No request carried more than 600k.
        let mut all = Stats::new(Kind::ClaudeCode, None);
        all.add(&t);
        let tried: Vec<(u64, u64, f64)> = all.carry.windows.iter().map(|w| (w.threshold / 1000, w.compactions, w.gain().round())).collect();
        assert_eq!(
            tried,
            [(150, 7, 567_500.0), (200, 4, 787_500.0), (300, 2, 835_000.0), (400, 1, 647_500.0), (500, 1, 792_500.0), (600, 0, 0.0), (800, 0, 0.0)]
        );
        // The best is 300k. The report speaks of the longest that gains four fifths as much,
        // which takes one compaction where 300k takes two, and names the best beside it.
        assert_eq!((all.carry.cheapest().unwrap().threshold, all.carry.featured().unwrap().threshold), (300_000, 500_000));
        assert!((all.carry.cost() - 792_500.0).abs() < 1e-3);
        let mut out = String::new();
        finding(&mut out, &all, "carried_context", all.carry.cost(), &Ctx::new(Path::new("/")), false);
        assert!(out.contains("Carried context: 22 of 31 requests (71%) carried more than 500k tokens, at most 600k.\n"), "{out}");
        assert!(out.contains("Compacting at 500k instead: 1 more compaction for about 14"), "{out}");
        // The best is named beside it only where it gains a tenth more.
        assert!(!out.contains("At 300k"), "{out}");
        let mut carry = Carry { peak: 900_000, windows: Vec::new() };
        *carry.window(200_000) = Window { threshold: 200_000, requests: 10, above: 9, saved: 10_000.0, compactions: 6, ..Default::default() };
        *carry.window(400_000) = Window { threshold: 400_000, requests: 10, above: 5, saved: 8_500.0, compactions: 2, ..Default::default() };
        let mut lines = Vec::new();
        carried_finding(&carry, "Carried context", "requests", "Advice.", &mut |_, text| lines.push(text.to_string()));
        assert_eq!(lines[0], "Carried context: 5 of 10 requests (50%) carried more than 400k tokens, at most 900k.");
        assert_eq!((lines[2].as_str(), lines[3].as_str()), ("At 200k: 6 more compactions, about 10k less in all.", "Advice."));
        let cold = |cause: Cause| Cold { session: String::new(), at: None, cause, fallback: false, before: 0, gap_ms: None, cost: 1_000.0, reason: None };
        assert_eq!(
            by_cause(&[cold(Cause::ModelSwitch), cold(Cause::IdleFive), cold(Cause::ModelSwitch)]),
            "2 model switches (2k), 1 idle for more than five minutes (1k)"
        );
        assert_eq!(by_cause(&[cold(Cause::ModelSwitch)]), "1 model switch (1k)");
        // A session of a model whose price steps is tried at that step, with that finding.
        let mut stepped = Stats::new(Kind::ClaudeCode, None);
        let haiku = |minute: i64, usage: Usage| req("claude-haiku-5-5", minute, usage);
        stepped.add(&Transcript { id: "h".into(), events: vec![haiku(0, u(0, 0, 20_000, 10)), haiku(1, u(0, 20_000, 100_000, 10))], ..Default::default() });
        assert!(stepped.carry.windows.is_empty());
        assert_eq!((stepped.step.window.threshold, stepped.step.window.requests, stepped.step.window.above), (100_000, 2, 1));
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
    fn weights_by_model() {
        for id in ["claude-fable-5-2", "claude-fable-5-1", "claude-mythos-5-2", "claude-mythos-5-1-20261001"] {
            assert_eq!(weights(id).cache_read, 0.025, "{id}");
        }
        for id in ["claude-opus-5-5", "claude-sonnet-5-5-20261101", "anthropic/claude-sonnet-5-5", "us.anthropic.claude-opus-5-5-v1:0", "claude-opus-5-5[1m]"] {
            assert_eq!(weights(id), Weights { cache_read: 0.05, ..DEFAULT_WEIGHTS }, "{id}");
        }
        let haiku = weights("anthropic/claude-haiku-5-5@20261001");
        assert_eq!((haiku.cache_read, haiku.tier), (0.1, Some(Tier { over: 100_000, times: 5.0 })));
        assert_eq!((haiku.times(Some(100_000)), haiku.times(Some(100_001)), haiku.times(None)), (1.0, 5.0, 1.0));
        // Older models, a family that only begins the same, and models of other providers.
        for id in ["claude-opus-5", "claude-haiku-4-5", "claude-opus-5-50", "gpt-6", ""] {
            assert_eq!(weights(id), DEFAULT_WEIGHTS, "{id}");
        }
        assert_eq!((DEFAULT_WEIGHTS.cache_read, DEFAULT_WEIGHTS.write_5m, DEFAULT_WEIGHTS.write_1h, DEFAULT_WEIGHTS.output), (0.1, 1.25, 2.0, 5.0));
        // A request over the step pays every kind five times.
        let over = Usage { input: Some(1_000), cache_read: Some(100_000), cache_write: Some(1_000), output: Some(100), ..Default::default() };
        assert_eq!(over.weigh(&haiku).total(), 5.0 * (1_000.0 + 10_000.0 + 1_250.0 + 500.0));
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
