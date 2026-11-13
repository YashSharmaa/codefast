//! Harness integration: `codefast setup`, `codefast doctor` and `codefast hook`.
//!
//! `setup` registers this program with coding harnesses: the MCP server entry, the skill that
//! tells an agent to run the `codefast` command, optionally the lifecycle hooks, and the short
//! rules block in `AGENTS.md`. It runs the harness's own command where there is one and edits
//! the documented configuration file otherwise. It plans first and shows the plan as a diff;
//! nothing is written unless it was asked to apply, and an entry it did not write is never
//! replaced without `--force`.

mod doctor;
mod formats;
mod harness;
mod hook;
mod json_edit;
mod prompt;
mod shell;

use anyhow::{Result, bail};
use formats::{Found, Server};
use harness::{Ctx, HARNESSES, Harness, Mcp, NAME, Scope};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The agent-facing guide, as shipped in `skills/codefast/SKILL.md`.
const SKILL: &str = include_str!("../../skills/codefast/SKILL.md");

/// What `setup agents-md` keeps in a project's instruction files: when to reach for which
/// command, and when not to.
const RULES: &str = "<!-- codefast:begin -->
## Code navigation: codefast

This repository is indexed by `codefast`. Use it first to find or understand code: one command usually replaces a grep and several file reads.

- before changing a function, read it with its type, what it calls, who calls it and its tests: `codefast context NAME`
- where a symbol is defined, with its body and callers: `codefast search NAME`
- one function or type instead of a whole file: `codefast show NAME`; a file's outline: `codefast show PATH`
- every call site with its source line (renames, signature changes): `codefast callers NAME`
- what a function calls: `codefast callees NAME`; every use of a symbol: `codefast refs NAME`
- how execution gets from one function to another: `codefast path FROM TO`
- which function prints a message: `codefast grep -F \"text\"`
- where to start for a bug report or stack trace: `codefast locate \"report text\"`

Answers give `path:line` locations to open directly. Plain grep is still right for exact strings in configuration and documentation.
<!-- codefast:end -->
";

const USAGE: &str = "Register codefast with coding harnesses.

Usage: codefast setup [TARGET...] [OPTIONS]       (`codefast setup help` prints this text)

Targets (also given as --harness ID; several are fine):
  a harness id      claude-code, codex, gemini-cli, cursor, copilot, ... (--list shows them all)
  agents-md         the short rules block in AGENTS.md (and CLAUDE.md / GEMINI.md where they exist)
  mcp-json          print the standard mcpServers snippet for any other MCP client
  --all             every harness found on this machine

Options:
  --scope user|project   the user's own configuration (default) or files inside this repository
  --with LIST            what to register: mcp, skill, hooks (default: mcp,skill)
  --hooks                the same as adding hooks to --with (Claude Code and Codex)
  --print                show what would be done and change nothing; this is the default
                         when no target is named or stdout is not a terminal
  --apply                make the changes even when stdout is not a terminal
  --force                replace an entry named codefast that setup did not write
  --bin PATH             the command to register (default: this binary; `codefast` at project scope)
  --file PATH            with agents-md: the file to keep the block in (repeatable)
  --list                 list the known harnesses and whether each is found here

Each harness's own configuration directory variable is honoured (CLAUDE_CONFIG_DIR, CODEX_HOME,
GEMINI_CLI_HOME, COPILOT_HOME, XDG_CONFIG_HOME, CLINE_MCP_SETTINGS_PATH, CRUSH_GLOBAL_CONFIG).
`codefast doctor` shows what is registered.
";

/// `arg` as one word of a POSIX shell command line.
pub(crate) fn quote(arg: &str) -> String {
    if !arg.is_empty() && arg.bytes().all(|b| b.is_ascii_alphanumeric() || b"_./+-@%,:=".contains(&b)) {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

// ----- options -----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Part {
    Mcp,
    Skill,
    Hooks,
}

#[derive(Debug, Default)]
struct Options {
    targets: Vec<String>,
    all: bool,
    scope: Option<Scope>,
    print: bool,
    apply: bool,
    force: bool,
    with: Option<Vec<Part>>,
    hooks: bool,
    bin: Option<String>,
    files: Vec<String>,
    list: bool,
    help: bool,
}

impl Options {
    fn parse(args: &[String]) -> Result<Options> {
        let mut o = Options::default();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let (flag, inline) = match arg.split_once('=') {
                Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
                _ => (arg.as_str(), None),
            };
            let mut value = |what: &str| -> Result<String> {
                match inline.clone().or_else(|| it.next().cloned()) {
                    Some(v) if !v.is_empty() => Ok(v),
                    _ => bail!("{flag} needs {what}"),
                }
            };
            match flag {
                "--harness" | "-H" => o.targets.extend(value("a harness id")?.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty())),
                "--all" => o.all = true,
                "--scope" => {
                    o.scope = Some(match value("user or project")?.as_str() {
                        "user" | "global" => Scope::User,
                        "project" | "workspace" | "local" => Scope::Project,
                        other => bail!("unknown scope `{other}`: use user or project"),
                    })
                }
                "--user" => o.scope = Some(Scope::User),
                "--project" => o.scope = Some(Scope::Project),
                "--print" | "--dry-run" | "-n" => o.print = true,
                "--apply" | "--yes" | "-y" => o.apply = true,
                "--force" | "-f" => o.force = true,
                "--hooks" => o.hooks = true,
                "--with" => {
                    let mut parts = Vec::new();
                    for word in value("a list of mcp, skill, hooks")?.split(',') {
                        parts.push(match word.trim() {
                            "mcp" => Part::Mcp,
                            "skill" | "skills" => Part::Skill,
                            "hooks" | "hook" => Part::Hooks,
                            other => bail!("unknown part `{other}`: use mcp, skill, hooks"),
                        });
                    }
                    o.with = Some(parts);
                }
                "--bin" | "--command" => o.bin = Some(value("a path or a command name")?),
                "--file" => o.files.push(value("a file name")?),
                "--list" => o.list = true,
                "--help" | "-h" | "help" => o.help = true,
                word if word.starts_with('-') => bail!("unknown option `{word}` (`codefast setup help` lists them)"),
                word => o.targets.push(word.to_string()),
            }
        }
        if o.print && o.apply {
            bail!("--print and --apply contradict each other");
        }
        Ok(o)
    }

    fn parts(&self) -> Vec<Part> {
        let mut parts = self.with.clone().unwrap_or_else(|| vec![Part::Mcp, Part::Skill]);
        if self.hooks && !parts.contains(&Part::Hooks) {
            parts.push(Part::Hooks);
        }
        parts
    }
}

// ----- the plan -----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Status {
    /// Already as it should be.
    Ok,
    /// A change that has not been made yet.
    Plan,
    Done,
    /// Not applicable here (no such scope, no such feature).
    Skip,
    /// An entry this program did not write stands in the way; `--force` replaces it.
    Refused,
    /// Needs a person.
    Manual,
    /// Something to read, nothing to do.
    Info,
    Failed,
}

impl Status {
    fn word(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Plan => "plan",
            Status::Done => "done",
            Status::Skip => "skip",
            Status::Refused => "refused",
            Status::Manual => "manual",
            Status::Info => "print",
            Status::Failed => "failed",
        }
    }
}

// A handful of these exist per run; boxing the larger variant would buy nothing.
#[allow(clippy::large_enum_variant)]
enum Act {
    None,
    Write {
        file: PathBuf,
        new: String,
    },
    /// The harness's own commands, checked afterwards against its configuration; when they do
    /// not register the server and the file may be written, the file is edited instead, as the
    /// commands left it.
    Run {
        commands: Vec<(PathBuf, Vec<String>)>,
        target: Mcp,
        want: Server,
        fallback: Option<PathBuf>,
        /// Whether the harness's file is known well enough to judge the commands by it. Where
        /// it is not (VS Code keeps one per profile), their exit status has to do.
        checkable: bool,
    },
}

struct Step {
    part: &'static str,
    status: Status,
    title: String,
    detail: Vec<String>,
    act: Act,
}

impl Step {
    fn new(part: &'static str, status: Status, title: impl Into<String>) -> Step {
        Step { part, status, title: title.into(), detail: Vec::new(), act: Act::None }
    }
}

struct Section {
    title: String,
    steps: Vec<Step>,
    notes: Vec<String>,
}

/// Files as they will be once the steps planned so far have run. Two harnesses that share a
/// file (`.mcp.json`, `~/.agents/skills`) then plan one change between them, not two.
#[derive(Default)]
struct Planned {
    files: HashMap<PathBuf, String>,
}

impl Planned {
    /// For a file an earlier step of this run already takes care of: a note saying so.
    fn shared(&self, file: &Path) -> &'static str {
        if self.files.contains_key(file) { " (see above)" } else { "" }
    }

    /// The text of `file`: `None` when it does not exist, an error when it cannot be read.
    fn read(&self, file: &Path) -> Result<Option<String>> {
        if let Some(text) = self.files.get(file) {
            return Ok(Some(text.clone()));
        }
        match std::fs::read(file) {
            Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| anyhow::anyhow!("is not UTF-8 text")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(anyhow::anyhow!("cannot be read: {e}")),
        }
    }
}

/// How an existing entry relates to the one wanted.
#[derive(Debug, PartialEq)]
enum State {
    Missing,
    Same,
    /// An entry of this program that launches something else (an older path).
    Ours(Server),
    Foreign(Server),
    Unreadable(String),
}

/// The entry named codefast in the harness's configuration, as it is now.
fn existing(target: &Mcp, plan: &Planned) -> Result<Found, String> {
    let file = match target {
        Mcp::Json { file, .. } | Mcp::Toml { file, .. } | Mcp::Yaml { file } | Mcp::CrushRc { file } => file,
        Mcp::Manual(_) | Mcp::Unavailable(_) => return Ok(Found::Missing),
    };
    match plan.read(file) {
        Ok(None) => Ok(Found::Missing),
        Ok(Some(text)) => found_in(target, &text),
        Err(e) => Err(e.to_string()),
    }
}

/// The entry named codefast in the text of the harness's configuration file.
fn found_in(target: &Mcp, text: &str) -> Result<Found, String> {
    match target {
        Mcp::Json { path, .. } => match json_edit::get(text, &path.iter().map(String::as_str).collect::<Vec<_>>()) {
            Ok(None) => Ok(Found::Missing),
            Ok(Some(entry)) => Ok(harness::server_of(&entry).map(Found::Entry).unwrap_or(Found::Unreadable)),
            Err(e) => Err(e.to_string()),
        },
        Mcp::Toml { .. } => Ok(formats::toml_get(text, NAME)),
        Mcp::Yaml { .. } => Ok(formats::goose_get(text, NAME)),
        Mcp::CrushRc { .. } => Ok(formats::crush_get(text, NAME)),
        Mcp::Manual(_) | Mcp::Unavailable(_) => Ok(Found::Missing),
    }
}

fn state(target: &Mcp, want: &Server, plan: &Planned) -> State {
    match existing(target, plan) {
        Err(why) => State::Unreadable(why),
        Ok(Found::Missing) => State::Missing,
        Ok(Found::Unreadable) => State::Unreadable("holds a codefast entry in a form that is not rewritten automatically".to_string()),
        Ok(Found::Entry(s)) if s == *want => State::Same,
        // The wanted program with arguments its owner added after `mcp` (a pinned `--root`) is in place too.
        Ok(Found::Entry(s)) if harness::is_codefast(&s) && s.command == want.command => State::Same,
        Ok(Found::Entry(s)) if harness::is_codefast(&s) => State::Ours(s),
        Ok(Found::Entry(s)) => State::Foreign(s),
    }
}

/// The entry of ours that `old` holds, if it holds one: the one case where the file is brought
/// up to date rather than given a new entry.
fn ours_in(target: &Mcp, old: &str) -> Option<Server> {
    match found_in(target, old) {
        Ok(Found::Entry(s)) if harness::is_codefast(&s) => Some(s),
        _ => None,
    }
}

/// Whether the entry of ours in `old` carries more than how to launch the server: settings its
/// owner (or the harness on their behalf) put there, such as an environment, a timeout, approved
/// tools, or a switch that turns the server off. A harness's own remove-and-add would drop them.
fn carries_settings(target: &Mcp, old: &str, want: &Server) -> bool {
    let Some(ours) = ours_in(target, old) else { return false };
    if ours.args != want.args {
        return true;
    }
    match target {
        Mcp::Json { path, entry, .. } => {
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            let found = json_edit::get(old, &path).ok().flatten();
            found.is_some_and(|found| harness::moved(&found, &want.command) != *entry)
        }
        Mcp::Toml { .. } => formats::toml_has_more(old, NAME),
        _ => false,
    }
}

/// The file with the entry set. An entry of ours that is already there is moved to the wanted
/// command where it stands: the arguments after `mcp` and every other setting in it stay as
/// they are. Anything else (no entry yet, or somebody else's that `--force` gives up) gets the
/// entry as this program writes it.
fn rewritten(target: &Mcp, old: &str, want: &Server) -> Result<String> {
    let ours = ours_in(target, old);
    let launch = match &ours {
        Some(s) => Server { command: want.command.clone(), args: s.args.clone() },
        None => want.clone(),
    };
    match target {
        Mcp::Json { path, entry, .. } => {
            let mut path: Vec<&str> = path.iter().map(String::as_str).collect();
            match (ours.is_some(), json_edit::get(old, &path)?) {
                (true, Some(found)) => {
                    // The member that names the program is the one that changes...
                    let key = if found.get("command").is_some() { "command" } else { "cmd" };
                    let value = harness::moved(&found, &want.command)[key].clone();
                    path.push(key);
                    let mut text = json_edit::set(old, &path, &value)?;
                    // ...and what an entry written today has and this one lacks is added.
                    // Nothing is taken out of it.
                    if let (Some(today), Some(have)) = (entry.as_object(), found.as_object()) {
                        for (member, value) in today.iter().filter(|(m, _)| !have.contains_key(*m) && !matches!(m.as_str(), "command" | "cmd" | "args")) {
                            *path.last_mut().expect("the entry's path") = member.as_str();
                            text = json_edit::set(&text, &path, value)?;
                        }
                    }
                    Ok(text)
                }
                _ => json_edit::set(old, &path, entry),
            }
        }
        Mcp::Toml { .. } => formats::toml_set(old, NAME, &launch),
        Mcp::Yaml { .. } => formats::goose_set(old, NAME, &launch, harness::DESCRIPTION, ours.is_some()),
        Mcp::CrushRc { .. } => formats::crush_set(old, NAME, &launch),
        Mcp::Manual(_) | Mcp::Unavailable(_) => bail!("nothing to write"),
    }
}

fn command_line(server: &Server) -> String {
    std::iter::once(server.command.as_str()).chain(server.args.iter().map(String::as_str)).map(quote).collect::<Vec<_>>().join(" ")
}

/// The lines of a change to show: a diff for an edit, the content for a short new file.
fn change_lines(old: Option<&str>, new: &str) -> Vec<String> {
    let mut lines = match old {
        Some(old) => formats::diff(old, new, 1),
        None if new.lines().count() <= 16 => formats::diff("", new, 0),
        None => Vec::new(),
    };
    if lines.len() > 40 {
        let more = lines.len() - 36;
        lines.truncate(36);
        lines.push(format!("… {more} more changed lines"));
    }
    lines
}

fn plan_write(part: &'static str, ctx: &Ctx, plan: &mut Planned, file: &Path, old: Option<&str>, new: String, what: &str) -> Step {
    let verb = if old.is_some() { "edit" } else { "create" };
    let mut step = Step::new(part, Status::Plan, format!("{verb} {}{what}", ctx.show(file)));
    step.detail = change_lines(old, &new);
    if old.is_none() && step.detail.is_empty() {
        step.title.push_str(&format!(" ({} lines)", new.lines().count()));
    }
    plan.files.insert(file.to_path_buf(), new.clone());
    step.act = Act::Write { file: file.to_path_buf(), new };
    step
}

fn plan_mcp(ctx: &Ctx, h: &Harness, scope: Scope, want: &Server, force: bool, plan: &mut Planned) -> Step {
    let target = h.mcp(ctx, scope, want);
    let (file, native, writable) = match &target {
        Mcp::Unavailable(why) => return Step::new("mcp", Status::Skip, why.clone()),
        Mcp::Manual(text) => return Step::new("mcp", Status::Manual, text.clone()),
        Mcp::Json { file, native, writable, .. } => (file.clone(), native.clone(), *writable),
        Mcp::Toml { file, native } => (file.clone(), native.clone(), true),
        Mcp::Yaml { file } | Mcp::CrushRc { file } => (file.clone(), None, true),
    };
    let shown = ctx.show(&file);
    let current = state(&target, want, plan);
    match &current {
        State::Same => return Step::new("mcp", Status::Ok, format!("{shown} already runs `{}`{}", command_line(want), plan.shared(&file))),
        State::Unreadable(why) => {
            let mut step = Step::new("mcp", Status::Manual, format!("{shown} {why}"));
            step.detail.push(format!("add the server by hand: {}", command_line(want)));
            return step;
        }
        State::Foreign(other) if !force => {
            let mut step = Step::new("mcp", Status::Refused, format!("{shown} has another server named {NAME}: `{}`", command_line(other)));
            step.detail.push("left as it is; --force replaces it".to_string());
            return step;
        }
        _ => {}
    }
    let old = match plan.read(&file) {
        Ok(old) => old,
        Err(e) => return Step::new("mcp", Status::Manual, format!("{shown} {e}")),
    };
    let new = rewritten(&target, old.as_deref().unwrap_or(""), want);
    // The harness's own command, unless it would cost the owner the settings of an entry that is
    // only being moved to a new binary: that entry is edited where it stands.
    let in_place = writable && carries_settings(&target, old.as_deref().unwrap_or(""), want);
    let program = native.as_ref().filter(|_| !in_place).and_then(|n| ctx.which(n.program).map(|p| (p, n)));
    let replaced = match &current {
        State::Ours(s) | State::Foreign(s) => format!(" (replaces `{}`)", command_line(s)),
        _ => String::new(),
    };
    match (program, new) {
        (Some((path, n)), new) => {
            let mut commands = Vec::new();
            if let (Some(remove), true) = (&n.remove, current != State::Missing) {
                commands.push((path.clone(), remove.clone()));
            }
            commands.push((path, n.add.clone()));
            let shown_command = commands
                .iter()
                .map(|(_, args)| format!("{} {}", n.program, args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")))
                .collect::<Vec<_>>()
                .join(" && ");
            let mut step = Step::new("mcp", Status::Plan, format!("run `{shown_command}`{replaced}"));
            step.detail.push(format!("which writes {shown}:"));
            let fallback = match new {
                Ok(new) => {
                    step.detail.extend(change_lines(old.as_deref(), &new));
                    plan.files.insert(file.clone(), new);
                    writable.then(|| file.clone())
                }
                Err(_) => None,
            };
            step.act = Act::Run { commands, target, want: want.clone(), fallback, checkable: writable };
            step
        }
        (None, Ok(new)) if writable => plan_write("mcp", ctx, plan, &file, old.as_deref(), new, &replaced),
        (None, Ok(_)) => {
            let n = native.clone().expect("a file that is not writable has a command");
            let mut step = Step::new("mcp", Status::Manual, format!("`{}` is not on PATH; run it yourself:", n.program));
            step.detail.push(format!("{} {}", n.program, n.add.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")));
            step
        }
        (None, Err(e)) => {
            let mut step = Step::new("mcp", Status::Manual, format!("{shown}: {e}"));
            step.detail.push(format!("add the server by hand: {}", command_line(want)));
            step
        }
    }
}

/// Whether a `SKILL.md` is a copy of ours (of any version).
fn is_our_skill(text: &str) -> bool {
    text.lines().skip(1).take_while(|l| l.trim() != "---").any(|l| l.trim() == format!("name: {NAME}"))
}

fn plan_skill(ctx: &Ctx, h: &Harness, scope: Scope, force: bool, plan: &mut Planned) -> Option<Step> {
    let file = h.skills(ctx, scope)?.join(NAME).join("SKILL.md");
    let shown = ctx.show(&file);
    Some(match plan.read(&file) {
        Err(e) => Step::new("skill", Status::Manual, format!("{shown} {e}")),
        Ok(Some(old)) if old == SKILL => Step::new("skill", Status::Ok, format!("{shown} is current{}", plan.shared(&file))),
        Ok(Some(old)) if !is_our_skill(&old) && !force => {
            let mut step = Step::new("skill", Status::Refused, format!("{shown} is another skill"));
            step.detail.push("left as it is; --force replaces it".to_string());
            step
        }
        Ok(old) => plan_write("skill", ctx, plan, &file, old.as_deref(), SKILL.to_string(), ""),
    })
}

/// The event a hook handler hands to this program, when its command is `codefast hook EVENT`:
/// the first word is this program, wherever it lives and however it is quoted, and the second
/// is `hook`. A command that merely mentions the two words is somebody else's.
fn our_hook_event(handler: &Value) -> Option<String> {
    let words = shell::leading_words(handler["command"].as_str()?);
    let [program, hook, event, ..] = &words[..] else { return None };
    let file = program.text.rsplit(['/', '\\']).next().unwrap_or("");
    (matches!(file, "codefast" | "codefast.exe" | "codefast.cmd") && hook.text == "hook" && !hook.dynamic).then(|| event.text.clone())
}

/// How many of a matcher group's handlers are ours, and how many it has.
fn our_share(entry: &Value) -> (usize, usize) {
    let handlers = entry["hooks"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    (handlers.iter().filter(|h| our_hook_event(h).is_some()).count(), handlers.len())
}

/// Whether a matcher group runs one of our hooks, alone or next to its owner's.
fn is_our_hook(entry: &Value) -> bool {
    our_share(entry).0 > 0
}

/// Whether a matcher group is ours to rewrite: it runs our hooks and nothing else.
fn is_only_our_hook(entry: &Value) -> bool {
    matches!(our_share(entry), (ours, all) if ours > 0 && ours == all)
}

/// The command line a hooks file runs for one event. `codefast hook` itself always ends with
/// status 0 and an empty stderr; the guard is for the one failure it cannot handle, which is not
/// being there: a hooks file shared through a repository reaches machines without the program,
/// and a harness reports a hook that fails to start on every tool call it was meant for.
/// Harnesses hand the line to `sh -c` (Git Bash on Windows). Claude Code falls back to
/// PowerShell on Windows without Git Bash, which would not take the guard, so it is left out
/// there.
fn hook_command(bin: &str, sub: &str) -> String {
    let run = format!("{} hook {sub}", quote(bin));
    if cfg!(windows) { run } else { format!("{run} 2>/dev/null || true") }
}

fn plan_hooks(ctx: &Ctx, h: &Harness, scope: Scope, bin: &str, plan: &mut Planned) -> Step {
    let Some(hooks) = h.hooks(ctx, scope) else {
        return Step::new(
            "hooks",
            Status::Skip,
            format!("setup registers hooks for Claude Code and Codex; for {} they come with the plugin, if at all", h.name),
        );
    };
    let shown = ctx.show(&hooks.file);
    let old = match plan.read(&hooks.file) {
        Ok(old) => old,
        Err(e) => return Step::new("hooks", Status::Manual, format!("{shown} {e}")),
    };
    let mut new = old.clone().unwrap_or_default();
    for (event, matcher, sub, timeout) in &hooks.events {
        let mut entry = json!({"hooks": [{"type": "command", "command": hook_command(bin, sub), "timeout": timeout}]});
        if let Some(m) = matcher {
            entry["matcher"] = json!(m);
        }
        // A group that runs this hook next to its owner's own handlers is the owner's
        // arrangement: it already does the job, and rewriting it would take their handlers out.
        let groups = match json_edit::get(&new, &["hooks", event]) {
            Ok(groups) => groups.unwrap_or(Value::Null),
            Err(e) => return Step::new("hooks", Status::Manual, format!("{shown}: {e}")),
        };
        let groups = groups.as_array().map(Vec::as_slice).unwrap_or(&[]);
        let runs_it = |g: &&Value| g["hooks"].as_array().is_some_and(|hs| hs.iter().any(|h| our_hook_event(h).as_deref() == Some(*sub)));
        if !groups.iter().any(is_only_our_hook) && groups.iter().any(|g| !is_only_our_hook(g) && runs_it(&g)) {
            continue;
        }
        new = match json_edit::upsert_item(&new, &["hooks", event], &entry, &is_only_our_hook) {
            Ok(text) => text,
            Err(e) => return Step::new("hooks", Status::Manual, format!("{shown}: {e}")),
        };
    }
    let mut step = if old.as_deref() == Some(new.as_str()) {
        Step::new("hooks", Status::Ok, format!("{shown} already registers the hooks"))
    } else {
        plan_write("hooks", ctx, plan, &hooks.file, old.as_deref(), new, "")
    };
    if let (Some(note), Status::Plan) = (hooks.note, step.status) {
        step.detail.push(note.to_string());
    }
    step
}

/// The rules block in one instruction file.
fn plan_rules_file(ctx: &Ctx, file: &Path, plan: &mut Planned) -> Step {
    let shown = ctx.show(file);
    let old = match plan.read(file) {
        Ok(old) => old,
        Err(e) => return Step::new("rules", Status::Manual, format!("{shown} {e}")),
    };
    match formats::block_set(old.as_deref().unwrap_or(""), RULES) {
        Err(e) => Step::new("rules", Status::Manual, format!("{shown}: {e}")),
        Ok(new) if old.as_deref() == Some(new.as_str()) => Step::new("rules", Status::Ok, format!("{shown} has the current block{}", plan.shared(file))),
        Ok(new) => {
            let what = if old.as_deref().is_some_and(formats::block_present) { " (update the codefast block)" } else { " (add the codefast block)" };
            plan_write("rules", ctx, plan, file, old.as_deref(), new, what)
        }
    }
}

/// The instruction files of the project that should carry the block: `AGENTS.md` always, and
/// `CLAUDE.md` / `GEMINI.md` where they exist and do not import `AGENTS.md` themselves.
fn plan_rules(ctx: &Ctx, files: &[String], plan: &mut Planned) -> Vec<Step> {
    if !files.is_empty() {
        return files.iter().map(|f| plan_rules_file(ctx, &ctx.project(f), plan)).collect();
    }
    let mut steps = vec![plan_rules_file(ctx, &ctx.project("AGENTS.md"), plan)];
    for name in ["CLAUDE.md", "GEMINI.md"] {
        let file = ctx.project(name);
        match plan.read(&file) {
            Ok(Some(text)) if text.contains("@AGENTS.md") => steps.push(Step::new("rules", Status::Ok, format!("{name} imports AGENTS.md"))),
            Ok(Some(_)) => steps.push(plan_rules_file(ctx, &file, plan)),
            _ => {}
        }
    }
    steps
}

/// Aider reads conventions from the files named under `read:` in `.aider.conf.yml`.
fn plan_aider(ctx: &Ctx, plan: &mut Planned) -> Vec<Step> {
    let mut steps = vec![plan_rules_file(ctx, &ctx.project("AGENTS.md"), plan)];
    let file = ctx.project(".aider.conf.yml");
    let shown = ctx.show(&file);
    steps.push(match plan.read(&file) {
        Err(e) => Step::new("rules", Status::Manual, format!("{shown} {e}")),
        Ok(old) => match formats::aider_add_read(old.as_deref().unwrap_or(""), "AGENTS.md") {
            Err(e) => Step::new("rules", Status::Manual, format!("{shown}: {e}")),
            Ok(new) if old.as_deref() == Some(new.as_str()) => Step::new("rules", Status::Ok, format!("{shown} already reads AGENTS.md")),
            Ok(new) => plan_write("rules", ctx, plan, &file, old.as_deref(), new, " (read AGENTS.md in every session)"),
        },
    });
    steps
}

fn plan_harness(ctx: &Ctx, h: &Harness, opts: &Options, plan: &mut Planned) -> Section {
    let scope = opts.scope.unwrap_or(Scope::User);
    let bin = opts.bin.clone().unwrap_or_else(|| ctx.command(scope));
    let want = Server { command: bin.clone(), args: vec!["mcp".to_string()] };
    let mut steps = Vec::new();
    let mut notes = h.notes(scope);
    let mut title = format!("{} ({}), {} scope", h.name, h.id, scope.word());
    if h.id == "aider" {
        title = format!("{} ({}), this project", h.name, h.id);
        steps = plan_aider(ctx, plan);
    } else {
        // What an installed plugin brings is not registered a second time: the harness would
        // start two servers with the same tools and run every hook twice.
        let plugin = h.plugin(ctx);
        let brought: &[&str] = if plugin.is_some() { h.plugin_parts() } else { &[] };
        if let Some(plugin) = &plugin {
            let names: Vec<&str> = [("skill", "the skill"), ("mcp", "the MCP server"), ("hooks", "the hooks")]
                .iter()
                .filter(|(part, _)| brought.contains(part))
                .map(|(_, name)| *name)
                .collect();
            let list = match &names[..] {
                [rest @ .., last] if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
                _ => names.join(""),
            };
            let verb = if names.len() == 1 { "comes" } else { "come" };
            steps.push(Step::new("plugin", Status::Ok, format!("{plugin} is installed: {list} {verb} with it")));
            // The pointer to the plugin route is for those who have not taken it.
            notes.retain(|note| !note.starts_with("as a plugin"));
            if h.plugin_server_needs_root() && opts.parts().contains(&Part::Mcp) {
                notes.push(format!(
                    "{} uses the server registered here in place of the plugin's own, which it starts in the plugin's directory and which needs `root` with every call",
                    h.name
                ));
            }
        }
        for part in opts.parts() {
            match part {
                Part::Mcp if !brought.contains(&"mcp") => steps.push(plan_mcp(ctx, h, scope, &want, opts.force, plan)),
                Part::Skill if !brought.contains(&"skill") => steps.extend(plan_skill(ctx, h, scope, opts.force, plan)),
                Part::Hooks if !brought.contains(&"hooks") => steps.push(plan_hooks(ctx, h, scope, &bin, plan)),
                _ => {}
            }
        }
    }
    Section { title, steps, notes }
}

fn mcp_json_section(ctx: &Ctx, opts: &Options) -> Section {
    let command = opts.bin.clone().unwrap_or_else(|| ctx.command(opts.scope.unwrap_or(Scope::User)));
    let snippet = json!({"mcpServers": {NAME: {"command": command, "args": ["mcp"]}}});
    let mut step = Step::new("mcp", Status::Info, "add this to the client's MCP configuration:");
    step.detail = json_edit::pretty_text(&snippet).lines().map(str::to_string).collect();
    Section {
        title: "Any MCP client (mcp-json)".to_string(),
        steps: vec![step],
        notes: vec![
            "the server speaks MCP over stdio; clients that do not start it inside the repository can pass `--root DIR` or set CODEFAST_ROOT".to_string(),
        ],
    }
}

// ----- applying -----

/// Replace `file` with `text` in one step. A symbolic link stays a link: its target is replaced.
fn write_file(file: &Path, text: &str) -> Result<()> {
    let target = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let dir = target.parent().map(Path::to_path_buf).unwrap_or_default();
    std::fs::create_dir_all(&dir).map_err(|e| anyhow::anyhow!("cannot create {}: {e}", dir.display()))?;
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(".{name}.codefast-{}", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        std::fs::write(&tmp, text)?;
        if let Ok(md) = std::fs::metadata(&target) {
            std::fs::set_permissions(&tmp, md.permissions())?;
        }
        std::fs::rename(&tmp, &target)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        bail!("cannot write {}: {e}", target.display());
    }
    Ok(())
}

/// Run one of a harness's commands to completion, without a terminal and with a time limit.
/// Returns whether it succeeded and what it said.
fn run_command(program: &Path, args: &[String], cwd: &Path) -> (bool, String) {
    let mut child = match std::process::Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return (false, format!("cannot start {}: {e}", program.display())),
    };
    // Both streams are drained as they fill, into one channel, so the command never blocks on a
    // full pipe and this side never waits on a pipe that a background process keeps open.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let pipes: [Option<Box<dyn Read + Send>>; 2] = [child.stdout.take().map(|p| Box::new(p) as _), child.stderr.take().map(|p| Box::new(p) as _)];
    for mut pipe in pipes.into_iter().flatten() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    // What the command wrote before it ended is there by now; a moment more for the last of it.
    let mut bytes = Vec::new();
    let grace = Instant::now() + Duration::from_millis(300);
    while let Ok(chunk) = rx.recv_timeout(grace.saturating_duration_since(Instant::now())) {
        bytes.extend(chunk);
    }
    match status {
        Some(s) => (s.success(), String::from_utf8_lossy(&bytes).trim().to_string()),
        None => (false, "no answer within 60 seconds".to_string()),
    }
}

fn apply(ctx: &Ctx, step: &mut Step) {
    match std::mem::replace(&mut step.act, Act::None) {
        Act::None => {}
        Act::Write { file, new } => match write_file(&file, &new) {
            Ok(()) => step.status = Status::Done,
            Err(e) => {
                step.status = Status::Failed;
                step.detail.push(e.to_string());
            }
        },
        Act::Run { commands, target, want, fallback, checkable } => {
            let mut said = Vec::new();
            for (program, args) in &commands {
                let (ok, text) = run_command(program, args, &ctx.root);
                if !ok {
                    let name = program.file_name().and_then(|n| n.to_str()).unwrap_or("command");
                    said.push(format!(
                        "`{name} {}` failed: {}",
                        args.iter().take(2).cloned().collect::<Vec<_>>().join(" "),
                        text.lines().last().unwrap_or("no output")
                    ));
                }
            }
            // What counts is what the harness's configuration says afterwards.
            if state(&target, &want, &Planned::default()) == State::Same || (!checkable && said.is_empty()) {
                step.status = Status::Done;
                return;
            }
            step.detail.extend(said);
            match fallback {
                // The edit is made to the file as it is now, not as it was when the plan was
                // drawn up: a harness command rewrites its own state in there (Claude Code keeps
                // the account and every project's settings in `.claude.json`) even when it fails.
                Some(file) => {
                    let written = Planned::default()
                        .read(&file)
                        .and_then(|now| rewritten(&target, now.as_deref().unwrap_or(""), &want))
                        .and_then(|new| write_file(&file, &new));
                    match written {
                        Ok(()) => {
                            step.status = Status::Done;
                            step.detail.push(format!("the command did not register the server; wrote {} instead", ctx.show(&file)));
                        }
                        Err(e) => {
                            step.status = Status::Failed;
                            step.detail.push(format!("the command did not register the server, and {} could not be edited instead: {e}", ctx.show(&file)));
                        }
                    }
                }
                None => {
                    step.status = Status::Failed;
                    step.detail.push("the command did not register the server".to_string());
                }
            }
        }
    }
}

// ----- the report -----

fn render(sections: &[Section], out: &mut String) {
    for section in sections {
        out.push_str(&format!("{}\n", section.title));
        for step in &section.steps {
            out.push_str(&format!("  {:<6} {:<8} {}\n", step.part, step.status.word(), step.title));
            for line in &step.detail {
                out.push_str(&format!("                  {line}\n"));
            }
        }
        for note in &section.notes {
            out.push_str(&format!("  note            {note}\n"));
        }
        out.push('\n');
    }
}

fn list(ctx: &Ctx) -> String {
    let mut out = String::from("Harnesses codefast setup knows:\n");
    for h in HARNESSES {
        let found = h.found(ctx).map(|why| format!("found ({why})")).unwrap_or_else(|| "not found".to_string());
        out.push_str(&format!("  {:<13} {:<26} {found}\n", h.id, h.name));
    }
    out.push_str("Other targets:\n  agents-md     the rules block in AGENTS.md / CLAUDE.md / GEMINI.md of this project\n  mcp-json      the standard mcpServers snippet for any other MCP client\n");
    out
}

/// A setup target: a harness, or one of the two that are not harnesses.
enum Target {
    Harness(&'static Harness),
    AgentsMd,
    McpJson,
}

fn targets(ctx: &Ctx, opts: &Options) -> Result<(Vec<Target>, bool)> {
    let mut out: Vec<Target> = Vec::new();
    for word in &opts.targets {
        let key = word.to_ascii_lowercase().replace('_', "-");
        match key.as_str() {
            "agents-md" | "agents.md" | "rules" => out.push(Target::AgentsMd),
            "mcp-json" | "mcp" | "generic" => out.push(Target::McpJson),
            "all" => out.extend(HARNESSES.iter().filter(|h| h.found(ctx).is_some()).map(Target::Harness)),
            _ => match harness::lookup(&key) {
                Some(h) if !out.iter().any(|t| matches!(t, Target::Harness(x) if x.id == h.id)) => out.push(Target::Harness(h)),
                Some(_) => {}
                None => bail!(
                    "unknown target `{word}`. Harnesses: {}; other targets: agents-md, mcp-json",
                    HARNESSES.iter().map(|h| h.id).collect::<Vec<_>>().join(", ")
                ),
            },
        }
    }
    let named = !out.is_empty() || opts.all;
    if opts.all || out.is_empty() {
        for h in HARNESSES.iter().filter(|h| h.found(ctx).is_some()) {
            if !out.iter().any(|t| matches!(t, Target::Harness(x) if x.id == h.id)) {
                out.push(Target::Harness(h));
            }
        }
    }
    Ok((out, named))
}

/// Register codefast with one or more coding harnesses. `args` are the raw CLI arguments after
/// `setup` (for example `--harness claude-code --scope project --print`).
pub fn setup(root: &Path, args: &[String]) -> Result<String> {
    let opts = Options::parse(args)?;
    if opts.help {
        return Ok(USAGE.to_string());
    }
    let ctx = Ctx::new(root);
    if opts.list {
        return Ok(list(&ctx));
    }
    let (targets, named) = targets(&ctx, &opts)?;
    // Changing a person's configuration takes a named target and either a terminal or --apply.
    let applying = named && !opts.print && (opts.apply || std::io::stdout().is_terminal());
    let mut out = String::new();
    if !named {
        let found: Vec<&str> = targets.iter().filter_map(|t| if let Target::Harness(h) = t { Some(h.id) } else { None }).collect();
        if found.is_empty() {
            return Ok(format!("codefast setup: no target named and no known harness found on this machine.\n\n{}\n{USAGE}", list(&ctx)));
        }
        out.push_str(&format!(
            "codefast setup: no target named. Found on this machine: {}.\nThis is what `codefast setup --all` would do:\n\n",
            found.join(", ")
        ));
    }

    let mut plan = Planned::default();
    let mut sections: Vec<Section> = Vec::new();
    for target in &targets {
        sections.push(match target {
            Target::Harness(h) => plan_harness(&ctx, h, &opts, &mut plan),
            Target::AgentsMd => {
                Section { title: "Project instructions (agents-md)".to_string(), steps: plan_rules(&ctx, &opts.files, &mut plan), notes: Vec::new() }
            }
            Target::McpJson => mcp_json_section(&ctx, &opts),
        });
    }
    if applying {
        for step in sections.iter_mut().flat_map(|s| s.steps.iter_mut()) {
            apply(&ctx, step);
        }
    }
    render(&sections, &mut out);

    let count = |status: Status| sections.iter().flat_map(|s| &s.steps).filter(|s| s.status == status).count();
    let (planned, done, ok, refused, manual, failed) =
        (count(Status::Plan), count(Status::Done), count(Status::Ok), count(Status::Refused), count(Status::Manual), count(Status::Failed));
    let mut extra = String::new();
    for (n, what) in [(refused, "refused (see --force)"), (manual, "left to do by hand"), (failed, "failed")] {
        if n > 0 {
            extra.push_str(&format!(", {n} {what}"));
        }
    }
    if applying {
        out.push_str(&format!("{done} changed, {ok} already in place{extra}.\n"));
    } else if planned == 0 {
        out.push_str(&format!("Nothing to change: {ok} already in place{extra}.\n"));
    } else {
        let how = if named { "run again with --apply to make them" } else { "`codefast setup --all --apply` makes them" };
        out.push_str(&format!(
            "Dry run: nothing was changed. {planned} change{} planned, {ok} already in place{extra}; {how}.\n",
            if planned == 1 { "" } else { "s" }
        ));
    }
    if applying && failed + refused > 0 {
        // The report is the result; the error only carries the exit status.
        print!("{out}");
        let _ = std::io::stdout().flush();
        bail!("setup did not complete: {} step{} not applied", failed + refused, if failed + refused == 1 { "" } else { "s" });
    }
    Ok(out)
}

/// Check the installation: binary on PATH, index health, and each harness registration.
pub fn doctor(root: &Path, args: &[String]) -> Result<String> {
    doctor::run(root, args)
}

/// Handle a harness hook event. `event` is the event name as passed on the command line
/// (`session-start`, `pre-tool-use`, `post-tool-use`); the hook's JSON payload is `input`.
/// Returns the text to print on stdout (possibly empty).
///
/// A hook must never disturb the harness that runs it: there is no error to return, nothing is
/// written to stderr, and a panic anywhere below ends the process quietly with status 0 (the
/// release profile aborts on panic, so unwinding cannot be relied on).
pub fn hook(root: &Path, event: &str, input: &str) -> Result<String> {
    #[cfg(not(test))]
    std::panic::set_hook(Box::new(|_| std::process::exit(0)));
    Ok(hook::run(root, event, input))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(args: &[&str]) -> Result<Options> {
        Options::parse(&args.iter().map(|a| a.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn options_are_read_in_every_spelling() {
        let o = opts(&[
            "--harness",
            "claude-code,cursor",
            "--harness=codex",
            "zed",
            "--scope",
            "project",
            "--apply",
            "--force",
            "--with=mcp,hooks",
            "--bin",
            "/x/codefast",
        ])
        .unwrap();
        assert_eq!(o.targets, ["claude-code", "cursor", "codex", "zed"]);
        assert_eq!((o.scope, o.apply, o.force, o.print), (Some(Scope::Project), true, true, false));
        assert_eq!(o.parts(), [Part::Mcp, Part::Hooks]);
        assert_eq!(o.bin.as_deref(), Some("/x/codefast"));
        assert_eq!(opts(&[]).unwrap().parts(), [Part::Mcp, Part::Skill]);
        assert_eq!(opts(&["--hooks"]).unwrap().parts(), [Part::Mcp, Part::Skill, Part::Hooks]);
        assert_eq!(opts(&["--with", "skill", "--hooks"]).unwrap().parts(), [Part::Skill, Part::Hooks]);
        assert!(opts(&["--all", "--print"]).unwrap().all);
        assert_eq!(opts(&["agents-md", "--file", "docs/AI.md", "--file=CLAUDE.md"]).unwrap().files, ["docs/AI.md", "CLAUDE.md"]);
        for bad in [
            vec!["--scope"],
            vec!["--scope", "team"],
            vec!["--with", "mcp,lsp"],
            vec!["--frobnicate"],
            vec!["--print", "--apply"],
            vec!["--harness"],
            vec!["--bin="],
        ] {
            assert!(opts(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_rules_block_is_short_and_calm() {
        assert!(RULES.starts_with(formats::BLOCK_BEGIN) && RULES.trim_end().ends_with(formats::BLOCK_END));
        assert!(crate::output::est_tokens(RULES) < 450, "{} tokens", crate::output::est_tokens(RULES));
        // Triggers, not mandates: nothing shouted, nothing forbidden.
        for word in RULES.split(|c: char| !c.is_ascii_alphabetic()) {
            let placeholder = matches!(word, "NAME" | "PATH" | "FROM");
            assert!(word.len() < 4 || placeholder || word.chars().any(|c| c.is_ascii_lowercase()), "{word}");
        }
        for shout in ["must", "always", "never", "important"] {
            assert!(!RULES.to_ascii_lowercase().contains(shout), "{shout}");
        }
        for command in ["search", "show", "callers", "callees", "refs", "path", "grep", "locate"] {
            assert!(RULES.contains(&format!("`codefast {command} ")), "{command}");
        }
        assert!(is_our_skill(SKILL) && !is_our_skill("---\nname: other\n---\n# x\n"));
    }
}
