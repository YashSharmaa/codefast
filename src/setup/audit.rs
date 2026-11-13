//! `codefast doctor context`: what a harness puts into every request before the first prompt,
//! and how to switch off, for this project only, the parts the project shows no use for.
//!
//! Every enabled plugin, skill, sub-agent and instruction file costs tokens on every request of
//! every session, used or not: a skill's name and description, an agent's description, an
//! instruction file in full. A harness set up for many kinds of work carries all of it into a
//! project that needs a fraction. This report reads the harness's own files (nothing is run and
//! nothing is changed), estimates what each part costs, sets it against what the project
//! contains, and prints the command that turns a part off for this project alone.
//!
//! The numbers are estimates from the text on disk at about four characters per token. Claude
//! Code's own `/context` and `claude plugin details NAME` are the exact figures: on a
//! configuration of 7 plugins, 43 skills and 3 loaded files the total here was 4% above the one
//! `/context` gave (2026-11-13, Claude Code 2.1.323). Codex has no such command, so its figures
//! rest on its documentation of what is loaded.
//!
//! Two harnesses are read: Claude Code and Codex.

use super::harness::{Ctx, env_path};
use crate::output::count;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// What the listing of one skill may hold of its description (Claude Code's
/// `skillListingMaxDescChars` default).
const LISTED_CHARS: usize = 1536;
/// Of the auto memory index, what is loaded at the start of a conversation.
const MEMORY_BYTES: usize = 25 * 1024;
const MEMORY_LINES: usize = 200;
/// A part is worth a line of its own from this many tokens on.
const WORTH_NAMING: usize = 150;

/// About how many tokens a text is. Words cost by their length, digits and punctuation by
/// their number. The weights are set against what Claude Code's `/context` reports for the same
/// texts (a 3,620-character instruction file of names, numbers and URLs: 1.4k tokens, a skill
/// description of 552 characters: about 190), and land within a few percent of both.
pub fn tokens(text: &str) -> usize {
    let mut n = 0.0f64;
    let (mut letters, mut digits) = (0usize, 0usize);
    let flush = |n: &mut f64, letters: &mut usize, digits: &mut usize| {
        if *letters > 0 {
            *n += (*letters as f64 / 3.0).max(1.0);
        }
        if *digits > 0 {
            *n += *digits as f64;
        }
        *letters = 0;
        *digits = 0;
    };
    for c in text.chars() {
        if c.is_alphabetic() {
            if digits > 0 {
                flush(&mut n, &mut letters, &mut digits);
            }
            letters += 1;
        } else if c.is_ascii_digit() {
            if letters > 0 {
                flush(&mut n, &mut letters, &mut digits);
            }
            digits += 1;
        } else {
            flush(&mut n, &mut letters, &mut digits);
            if !c.is_whitespace() {
                n += 1.2;
            }
        }
    }
    flush(&mut n, &mut letters, &mut digits);
    n.ceil() as usize
}

/// One thing that is loaded into every request.
#[derive(Debug, Clone)]
pub struct Part {
    /// `plugin`, `skill`, `agent`, `instructions`, `memory`.
    pub kind: &'static str,
    pub name: String,
    /// Where it comes from: a plugin id, `user` or `project`.
    pub source: String,
    /// Estimated tokens it adds to every request.
    pub tokens: usize,
    /// What it is about, in its own words (for a plugin: the names of its skills).
    pub about: String,
    /// The command or setting that switches it off for this project. Empty when there is none.
    pub off: String,
    /// Why it belongs in this project: a word of the project's own that it names (a language, a
    /// dependency), `general` for a part about software work as such, empty for parts that are
    /// not judged (instruction files). `None`: nothing in the project matches.
    pub matches: Option<String>,
    /// How many skills, commands and agents it lists.
    pub listed: usize,
}

#[derive(Debug, Default)]
pub struct Audit {
    pub harness: &'static str,
    pub parts: Vec<Part>,
    /// MCP servers by where they are declared. Their tools are deferred by default, so only
    /// names and instructions load: counted, not costed.
    pub servers: Vec<(String, String)>,
    pub profile: Vec<String>,
    /// What could not be looked at, said in the report.
    pub notes: Vec<String>,
}

impl Audit {
    pub fn total(&self) -> usize {
        self.parts.iter().map(|p| p.tokens).sum()
    }

    /// Parts that nothing in the project matches and that can be switched off, heaviest first.
    pub fn unmatched(&self) -> Vec<&Part> {
        let mut v: Vec<&Part> = self.parts.iter().filter(|p| p.matches.is_none() && !p.off.is_empty() && matches!(p.kind, "plugin" | "skill" | "agent")).collect();
        v.sort_by_key(|p| std::cmp::Reverse(p.tokens));
        v
    }

    /// Instruction files whose text another one repeats word for word: (the copy, the first).
    pub fn repeated(&self) -> Vec<(&Part, &Part)> {
        let files: Vec<&Part> = self.parts.iter().filter(|p| p.kind == "instructions").collect();
        let mut out = Vec::new();
        for (i, p) in files.iter().enumerate() {
            if let Some(first) = files[..i].iter().find(|q| q.about == p.about && !p.about.is_empty()) {
                out.push((*p, *first));
            }
        }
        out
    }
}

// ----- reading -----

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The front matter of a Markdown file: its top-level `key: value` pairs. Block scalars
/// (`|`, `>`) and values continued on indented lines are joined with spaces.
pub fn front_matter(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return out;
    }
    let mut key: Option<String> = None;
    for line in lines {
        if line.trim_end() == "---" {
            break;
        }
        let indented = line.starts_with([' ', '\t']);
        if !indented {
            if let Some((k, v)) = line.split_once(':') {
                if !k.is_empty() && k.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
                    let v = v.trim();
                    let v = if matches!(v, "|" | ">" | "|-" | ">-" | "|+" | ">+") { "" } else { v };
                    let v = v.strip_prefix('"').and_then(|x| x.strip_suffix('"')).or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\''))).unwrap_or(v);
                    out.insert(k.to_string(), v.to_string());
                    key = Some(k.to_string());
                    continue;
                }
            }
            key = None;
        } else if let Some(k) = &key {
            let e = out.entry(k.clone()).or_default();
            if !e.is_empty() {
                e.push(' ');
            }
            e.push_str(line.trim());
        }
    }
    out
}

/// What one skill, command or agent file adds to the listing: its name and what the harness
/// shows of its description. `None` when the model is not shown it at all.
fn listed(path: &Path, fallback: &str) -> Option<(String, usize, String)> {
    // (its name, the tokens its listing costs, its description)
    let text = std::fs::read_to_string(path).ok()?;
    let fm = front_matter(&text);
    if fm.get("disable-model-invocation").is_some_and(|v| v == "true") {
        return None;
    }
    let name = fm.get("name").filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| fallback.to_string());
    let desc = format!("{} {}", fm.get("description").map(String::as_str).unwrap_or(""), fm.get("when_to_use").map(String::as_str).unwrap_or(""));
    let desc = desc.trim().to_string();
    let shown: String = desc.chars().take(LISTED_CHARS).collect();
    Some((name.clone(), tokens(&name) + tokens(&shown) + 4, desc))
}

fn dirs(path: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    v.sort();
    v
}

fn markdown(path: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(path).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() && depth > 0 {
            out.extend(markdown(&p, depth - 1));
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// The skills, commands and agents under one root (a plugin, or a configuration directory).
fn components(root: &Path) -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    for d in dirs(&root.join("skills")) {
        let name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        out.extend(listed(&d.join("SKILL.md"), &name));
    }
    for sub in ["commands", "agents"] {
        for f in markdown(&root.join(sub), 3) {
            let name = f.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.extend(listed(&f, &name));
        }
    }
    out
}

// ----- the project -----

/// Words that say what a project is made of: its languages and the names of its dependencies.
/// Files that git ignores are not part of it (vendored trees, build output, benchmark data).
pub fn profile(root: &Path) -> BTreeSet<String> {
    let mut words: BTreeSet<String> = BTreeSet::new();
    let mut add = |w: &str| {
        let w = w.trim().to_ascii_lowercase();
        if w.len() >= 2 {
            words.insert(w);
        }
    };
    let walk = ignore::WalkBuilder::new(root).max_depth(Some(4)).hidden(true).git_ignore(true).git_exclude(true).parents(true).require_git(false).build();
    for entry in walk.flatten().take(30_000) {
        let p = entry.path();
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let lang: &[&str] = match p.extension().and_then(|x| x.to_str()).unwrap_or("") {
            "rs" => &["rust", "cargo"],
            "py" | "pyi" => &["python"],
            "ts" | "mts" | "cts" => &["typescript"],
            "tsx" => &["typescript", "react", "frontend"],
            "jsx" => &["javascript", "react", "frontend"],
            "js" | "mjs" | "cjs" => &["javascript"],
            "go" => &["golang"],
            "java" => &["java"],
            "kt" | "kts" => &["kotlin"],
            "swift" => &["swift", "ios"],
            "cs" => &["csharp", "dotnet"],
            "rb" => &["ruby"],
            "php" => &["php"],
            "cc" | "cpp" | "cxx" | "hpp" => &["cpp", "c++"],
            "html" | "css" | "scss" => &["html", "css", "frontend"],
            "vue" => &["vue", "frontend"],
            "svelte" => &["svelte", "frontend"],
            "sql" => &["sql", "database"],
            "tf" => &["terraform"],
            "ipynb" => &["jupyter", "notebook"],
            _ => &[],
        };
        for l in lang {
            add(l);
        }
        match name.as_str() {
            "Dockerfile" => add("docker"),
            "package.json" => {
                if let Some(v) = read_json(p) {
                    for key in ["dependencies", "devDependencies", "peerDependencies"] {
                        for dep in v.get(key).and_then(Value::as_object).into_iter().flatten().map(|(k, _)| k) {
                            add(dep.rsplit('/').next().unwrap_or(dep));
                            add(dep.trim_start_matches('@').split('/').next().unwrap_or(dep));
                        }
                    }
                }
            }
            "Cargo.toml" | "pyproject.toml" | "requirements.txt" | "go.mod" | "Gemfile" | "composer.json" => {
                for line in std::fs::read_to_string(p).unwrap_or_default().lines().take(400) {
                    let l = line.trim().trim_start_matches(['"', '\'', '-', ' ']).trim_start_matches("gem ").trim_start_matches(['"', '\'']);
                    let dep: String = l.chars().take_while(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/')).collect();
                    if !dep.is_empty() {
                        add(dep.rsplit('/').next().unwrap_or(&dep));
                    }
                }
            }
            _ => {}
        }
    }
    words
}

/// Words too common to tie a part to a project: a dependency called `time` or `color` does not
/// make a video skill belong here.
const PLAIN: [&str; 64] = [
    "test", "tests", "time", "path", "log", "http", "json", "core", "util", "utils", "types", "node", "web", "build", "cli", "app", "api", "data", "file", "files", "code", "name", "version",
    "docs", "common", "color", "colors", "error", "errors", "documentation", "config", "text", "table", "image", "images", "string", "format", "parse", "parser", "server", "client", "model",
    "models", "schema", "date", "math", "list", "map", "set", "queue", "process", "shell", "style", "styles", "theme", "icon", "font", "chart", "edition", "license", "description", "features",
    "default", "require",
];

/// What marks a part as being about software work as such, and so at home in any repository.
const GENERAL: [&str; 22] = [
    "code review", "review the", "refactor", "debug", "commit", "pull request", "unit test", "test suite", "lint", "codebase", "source code", "repositor", "security review", "simplif",
    "changed code", "diff", "bug", "settings.json", "claude api", "anthropic sdk", "workflow", "slash command",
];

/// Why a part belongs in a project with this profile: a word of the project that the part
/// names, or that its own name is; `general` when it is about software work as such.
fn fit(profile: &BTreeSet<String>, name: &str, text: &str) -> Option<String> {
    let lower = format!("{name} {text}").to_ascii_lowercase();
    let words: BTreeSet<&str> = lower.split(|c: char| !(c.is_alphanumeric() || c == '+' || c == '#')).filter(|w| w.len() >= 3).collect();
    if let Some(w) = profile.iter().find(|p| p.len() >= 3 && !PLAIN.contains(&p.as_str()) && words.contains(p.as_str())) {
        return Some(w.clone());
    }
    GENERAL.iter().any(|g| lower.contains(g)).then(|| "general".to_string())
}

// ----- Claude Code -----

fn claude_dir(ctx: &Ctx) -> PathBuf {
    env_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| ctx.home(".claude"))
}

/// `enabledPlugins` as the user, project and local settings leave it (the later file wins).
fn enabled_plugins(ctx: &Ctx, cfg: &Path) -> BTreeMap<String, bool> {
    let mut out = BTreeMap::new();
    for file in [cfg.join("settings.json"), ctx.project(".claude/settings.json"), ctx.project(".claude/settings.local.json")] {
        if let Some(map) = read_json(&file).as_ref().and_then(|v| v.get("enabledPlugins")).and_then(Value::as_object) {
            for (id, on) in map {
                if let Some(b) = on.as_bool() {
                    out.insert(id.clone(), b);
                }
            }
        }
    }
    out
}

/// Skills the settings hide from the model (`skillOverrides`), later files winning.
fn hidden_skills(ctx: &Ctx, cfg: &Path) -> BTreeSet<String> {
    let mut state: BTreeMap<String, bool> = BTreeMap::new();
    for file in [cfg.join("settings.json"), ctx.project(".claude/settings.json"), ctx.project(".claude/settings.local.json")] {
        if let Some(map) = read_json(&file).as_ref().and_then(|v| v.get("skillOverrides")).and_then(Value::as_object) {
            for (name, v) in map {
                state.insert(name.clone(), matches!(v.as_str(), Some("off" | "user-invocable-only")));
            }
        }
    }
    state.into_iter().filter(|(_, off)| *off).map(|(n, _)| n).collect()
}

/// Instruction files in the order the harness loads them, with where each comes from.
fn instruction_files(ctx: &Ctx, cfg: &Path) -> Vec<(PathBuf, &'static str)> {
    let mut out = vec![(cfg.join("CLAUDE.md"), "user")];
    out.extend(markdown(&cfg.join("rules"), 3).into_iter().map(|p| (p, "user")));
    // The project's own, and those of every directory above it. A directory above the project
    // that holds another harness account's `.claude/CLAUDE.md` counts here too: the harness
    // reads it as an instruction file of a parent directory.
    let mut dir = Some(ctx.root.as_path());
    while let Some(d) = dir {
        for rel in ["CLAUDE.md", ".claude/CLAUDE.md", "CLAUDE.local.md"] {
            let p = d.join(rel);
            if p.is_file() && !out.iter().any(|(q, _)| *q == p) {
                out.push((p, if d == ctx.root { "project" } else { "parent directory" }));
            }
        }
        // Nothing above the home directory is anybody's instruction file.
        if ctx.home.as_deref() == Some(d) {
            break;
        }
        dir = d.parent();
    }
    out.extend(markdown(&ctx.project(".claude/rules"), 4).into_iter().map(|p| (p, "project")));
    out.retain(|(p, _)| p.is_file());
    // A loaded file brings in what it imports (`@AGENTS.md`, `@docs/rules.md`, `@~/notes.md`).
    // `AGENTS.md` is not read of its own accord: `/context` of Claude Code 2.1.323 does not list
    // it for a project that has one and no `CLAUDE.md`.
    let mut i = 0;
    while i < out.len() && out.len() < 64 {
        let (path, source) = out[i].clone();
        i += 1;
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let mut fenced = false;
        for line in text.lines() {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
            }
            if fenced {
                continue;
            }
            for word in line.split_whitespace() {
                let Some(target) = word.strip_prefix('@').map(|t| t.trim_end_matches(|c: char| matches!(c, '.' | ',' | ')' | ':' | ';'))) else { continue };
                if target.is_empty() || !target.contains('.') && !target.contains('/') {
                    continue;
                }
                let to = match target.strip_prefix("~/") {
                    Some(rest) => ctx.home(rest),
                    None => path.parent().unwrap_or(Path::new("")).join(target),
                };
                if to.is_file() && !out.iter().any(|(q, _)| *q == to) {
                    out.push((to, source));
                }
            }
        }
    }
    out
}

/// The skills Claude Code keeps in step with the signed-in claude.ai account: one directory per
/// organisation and account under `skills/synced`, of which the signed-in one is loaded.
fn synced_skills(ctx: &Ctx, cfg: &Path) -> Vec<PathBuf> {
    let state = read_json(&cfg.join(".claude.json")).or_else(|| read_json(&ctx.home(".claude.json")));
    let id = |key: &str| state.as_ref().and_then(|s| s.get("oauthAccount")).and_then(|o| o.get(key)).and_then(Value::as_str).map(str::to_string);
    let (Some(org), Some(account)) = (id("organizationUuid"), id("accountUuid")) else { return Vec::new() };
    dirs(&cfg.join("skills/synced").join(format!("{org}_{account}")))
}

pub fn claude_code(ctx: &Ctx) -> Audit {
    let cfg = claude_dir(ctx);
    let profile = profile(&ctx.root);
    let mut a = Audit { harness: "Claude Code", profile: profile.iter().cloned().collect(), ..Default::default() };
    // Plugins: what each lists of its skills, commands and agents.
    let enabled = enabled_plugins(ctx, &cfg);
    let installed = read_json(&cfg.join("plugins/installed_plugins.json"));
    for (id, installs) in installed.as_ref().and_then(|v| v.get("plugins")).and_then(Value::as_object).into_iter().flatten() {
        if enabled.get(id) == Some(&false) {
            continue;
        }
        let Some(path) = installs.as_array().and_then(|l| l.last()).and_then(|i| i.get("installPath")).and_then(Value::as_str) else { continue };
        let root = Path::new(path);
        let parts = components(root);
        let manifest = read_json(&root.join(".claude-plugin/plugin.json"));
        let inline = manifest.as_ref().and_then(|m| m.get("mcpServers")).and_then(Value::as_object).map(|m| m.len()).unwrap_or(0);
        let file = read_json(&root.join(".mcp.json")).as_ref().and_then(|m| m.get("mcpServers").or(Some(m))).and_then(Value::as_object).map(|m| m.len()).unwrap_or(0);
        for _ in 0..inline + file {
            a.servers.push((id.clone(), "plugin".to_string()));
        }
        if parts.is_empty() {
            continue;
        }
        let names: Vec<&str> = parts.iter().map(|p| p.0.as_str()).collect();
        let text = format!("{} {}", names.join(" "), parts.iter().map(|p| p.2.as_str()).collect::<Vec<_>>().join(" "));
        let shown: Vec<&str> = names.iter().take(4).copied().collect();
        let about = if names.len() > shown.len() { format!("{}, +{}", shown.join(", "), names.len() - shown.len()) } else { shown.join(", ") };
        let short = id.split('@').next().unwrap_or(id);
        a.parts.push(Part {
            kind: "plugin",
            name: id.clone(),
            source: "plugin".to_string(),
            tokens: parts.iter().map(|p| p.1).sum(),
            about,
            off: format!("claude plugin disable {id} --scope local"),
            matches: fit(&profile, short, &text),
            listed: parts.len(),
        });
    }
    // Skills and agents of the user and of the project.
    let hidden = hidden_skills(ctx, &cfg);
    for (root, source) in [(cfg.clone(), "user"), (ctx.project(".claude"), "project")] {
        for d in dirs(&root.join("skills")) {
            // A plugin kept in the skills directory is a plugin, and is counted there.
            if d.join(".claude-plugin/plugin.json").is_file() {
                continue;
            }
            let dir_name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Some((name, tokens, desc)) = listed(&d.join("SKILL.md"), &dir_name).filter(|(n, ..)| !hidden.contains(n)) {
                let off = format!("\"skillOverrides\": {{\"{name}\": \"off\"}}");
                a.parts.push(Part { kind: "skill", matches: fit(&profile, &name, &desc), about: crate::output::clip(&desc, 70), name, source: source.to_string(), tokens, off, listed: 1 });
            }
        }
        for f in markdown(&root.join("agents"), 3) {
            let stem = f.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Some((name, tokens, desc)) = listed(&f, &stem) {
                let off = format!("\"permissions\": {{\"deny\": [\"Agent({name})\"]}}");
                a.parts.push(Part { kind: "agent", matches: fit(&profile, &name, &desc), about: crate::output::clip(&desc, 70), name, source: source.to_string(), tokens, off, listed: 1 });
            }
        }
    }
    for d in synced_skills(ctx, &cfg) {
        let dir_name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some((name, tokens, desc)) = listed(&d.join("SKILL.md"), &dir_name).filter(|(n, ..)| !hidden.contains(n)) {
            a.parts.push(Part { kind: "synced", matches: fit(&profile, &name, &desc), about: crate::output::clip(&desc, 70), name, source: "claude.ai".to_string(), tokens, off: String::new(), listed: 1 });
        }
    }
    // Instruction files are loaded whole. `about` holds a hash of the text here, which is how
    // one that repeats another is recognised.
    for (path, source) in instruction_files(ctx, &cfg) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let off = if source == "project" { String::new() } else { format!("\"claudeMdExcludes\": [\"{}\"]", path.display()) };
        a.parts.push(Part {
            kind: "instructions",
            name: ctx.show(&path),
            source: source.to_string(),
            tokens: tokens(&text),
            about: format!("{:016x}", xxhash_rust::xxh3::xxh3_64(text.trim().as_bytes())),
            off,
            matches: Some(String::new()),
            listed: 0,
        });
    }
    // The index of the automatic memory of this project.
    let slug: String = ctx.root.to_string_lossy().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    let memory = cfg.join("projects").join(slug).join("memory/MEMORY.md");
    if let Ok(text) = std::fs::read_to_string(&memory) {
        let mut head: String = text.lines().take(MEMORY_LINES).collect::<Vec<_>>().join("\n");
        head.truncate(head.len().min(MEMORY_BYTES));
        a.parts.push(Part { kind: "memory", name: ctx.show(&memory), source: "user".to_string(), tokens: tokens(&head), about: String::new(), off: String::new(), matches: Some(String::new()), listed: 0 });
    }
    // MCP servers: named here, costed by the harness (their tools are deferred by default).
    let state = read_json(&cfg.join(".claude.json")).or_else(|| read_json(&ctx.home(".claude.json")));
    if let Some(state) = &state {
        for name in state.get("mcpServers").and_then(Value::as_object).into_iter().flatten().map(|(k, _)| k) {
            a.servers.push((name.clone(), "user".to_string()));
        }
        let here = state.get("projects").and_then(|p| p.get(ctx.root.to_string_lossy().as_ref()));
        for name in here.and_then(|p| p.get("mcpServers")).and_then(Value::as_object).into_iter().flatten().map(|(k, _)| k) {
            a.servers.push((name.clone(), "local".to_string()));
        }
    }
    for name in read_json(&ctx.project(".mcp.json")).as_ref().and_then(|v| v.get("mcpServers")).and_then(Value::as_object).into_iter().flatten().map(|(k, _)| k) {
        a.servers.push((name.clone(), "project".to_string()));
    }
    a
}

// ----- Codex -----

fn codex_dir(ctx: &Ctx) -> PathBuf {
    env_path("CODEX_HOME").unwrap_or_else(|| ctx.home(".codex"))
}

/// A dotted TOML key as its parts: `plugins."a.b@c".enabled` is `plugins`, `a.b@c`, `enabled`.
fn toml_key(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => quote = Some(c),
            (None, '.') => parts.push(std::mem::take(&mut cur).trim().to_string()),
            (None, c) => cur.push(c),
        }
    }
    parts.push(cur.trim().to_string());
    parts
}

/// The `key = value` lines of a TOML file, each with the table it stands in. It reads what this
/// audit needs (switches, names, one-line strings) and nothing else: an entry of an array of
/// tables gets a number as the last part of its table, and a value that spans lines is skipped.
fn toml_pairs(text: &str) -> Vec<(Vec<String>, String, String)> {
    let mut table: Vec<String> = Vec::new();
    let mut entries = 0usize;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let array = line.starts_with("[[");
            let inner = line.trim_start_matches('[');
            let inner = inner.rfind(']').map_or(inner, |i| &inner[..i]).trim_end_matches(']');
            table = toml_key(inner);
            if array {
                entries += 1;
                table.push(format!("#{entries}"));
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let mut key = toml_key(k.trim());
        let Some(last) = key.pop() else { continue };
        let v = v.trim();
        let value = match v.chars().next() {
            Some(q @ ('"' | '\'')) => match v[1..].find(q) {
                Some(end) => v[1..1 + end].to_string(),
                None => continue,
            },
            _ => v.split('#').next().unwrap_or("").trim().to_string(),
        };
        let mut at = table.clone();
        at.extend(key);
        out.push((at, last, value));
    }
    out
}

pub fn codex(ctx: &Ctx) -> Audit {
    let cfg = codex_dir(ctx);
    let mut a = Audit { harness: "Codex", ..Default::default() };
    if !cfg.is_dir() {
        return a;
    }
    let profile = profile(&ctx.root);
    a.profile = profile.iter().cloned().collect();
    // The user's file, then the project's: where both set a switch, the closer one counts.
    let mut pairs: Vec<(Vec<String>, String, String)> = Vec::new();
    for file in [cfg.join("config.toml"), ctx.project(".codex/config.toml")] {
        match std::fs::read_to_string(&file) {
            Ok(text) => pairs.extend(toml_pairs(&text)),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                a.notes.push(format!("{} could not be read: the plugins and MCP servers it switches on are not counted.", ctx.show(&file)));
            }
            Err(_) => {}
        }
    }
    let switch = |table: &[&str]| pairs.iter().rev().find(|(t, k, _)| k == "enabled" && t.iter().map(String::as_str).eq(table.iter().copied())).map(|(_, _, v)| v == "true");
    // Plugins: the ones the configuration names and has not switched off. What each costs is
    // the listing of its skills.
    let ids: BTreeSet<String> = pairs.iter().filter(|(t, ..)| t.len() >= 2 && t[0] == "plugins" && t[1] != "_default").map(|(t, ..)| t[1].clone()).collect();
    for id in ids {
        if switch(&["plugins", &id]) == Some(false) {
            continue;
        }
        let Some((name, market)) = id.split_once('@') else { continue };
        // `cache/<marketplace>/<plugin>/<version>/`: the newest version is the one in use.
        let Some(root) = dirs(&cfg.join("plugins/cache").join(market).join(name)).pop() else { continue };
        let servers = [root.join(".mcp.json"), root.join("mcp.json")].iter().find_map(|f| read_json(f)).as_ref().and_then(|m| m.get("mcpServers").or(Some(m))).and_then(Value::as_object).map(|m| m.len()).unwrap_or(0);
        for _ in 0..servers {
            a.servers.push((id.clone(), "plugin".to_string()));
        }
        let mut parts = Vec::new();
        for d in dirs(&root.join("skills")) {
            let dir_name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            parts.extend(listed(&d.join("SKILL.md"), &dir_name));
        }
        if parts.is_empty() {
            continue;
        }
        let names: Vec<&str> = parts.iter().map(|p| p.0.as_str()).collect();
        let text = format!("{} {}", names.join(" "), parts.iter().map(|p| p.2.as_str()).collect::<Vec<_>>().join(" "));
        let shown: Vec<&str> = names.iter().take(4).copied().collect();
        let about = if names.len() > shown.len() { format!("{}, +{}", shown.join(", "), names.len() - shown.len()) } else { shown.join(", ") };
        a.parts.push(Part {
            kind: "plugin",
            name: id.clone(),
            source: "plugin".to_string(),
            tokens: parts.iter().map(|p| p.1).sum(),
            about,
            off: format!("[plugins.\"{id}\"]\n    enabled = false"),
            matches: fit(&profile, name, &text),
            listed: parts.len(),
        });
    }
    // Skills: the listing holds each one's name, description and file path.
    let hidden: BTreeSet<String> = {
        let entry = |t: &Vec<String>| t.len() == 3 && t[0] == "skills" && t[1] == "config";
        let off: BTreeSet<&Vec<String>> = pairs.iter().filter(|(t, k, v)| entry(t) && k == "enabled" && v == "false").map(|(t, ..)| t).collect();
        pairs.iter().filter(|(t, k, _)| off.contains(t) && (k == "name" || k == "path")).map(|(_, _, v)| v.clone()).collect()
    };
    for (dir, source) in [(ctx.home(".agents/skills"), "user"), (ctx.project(".agents/skills"), "project")] {
        for d in dirs(&dir) {
            let file = d.join("SKILL.md");
            let dir_name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Some((name, listing, desc)) = listed(&file, &dir_name) else { continue };
            if hidden.contains(&name) || hidden.contains(file.to_string_lossy().as_ref()) {
                continue;
            }
            let off = format!("[[skills.config]]\n    name = \"{name}\"\n    enabled = false");
            a.parts.push(Part {
                kind: "skill",
                matches: fit(&profile, &name, &desc),
                about: crate::output::clip(&desc, 70),
                name,
                source: source.to_string(),
                tokens: listing + tokens(&file.to_string_lossy()),
                off,
                listed: 1,
            });
        }
    }
    // Sub-agents: a description each, shown when Codex chooses one.
    for (dir, source) in [(cfg.join("agents"), "user"), (ctx.project(".codex/agents"), "project")] {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
        files.sort();
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            let top = toml_pairs(&text);
            let get = |key: &str| top.iter().find(|(t, k, _)| t.is_empty() && k == key).map(|(_, _, v)| v.clone());
            let Some(desc) = get("description") else { continue };
            let name = get("name").unwrap_or_else(|| f.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
            a.parts.push(Part { kind: "agent", matches: fit(&profile, &name, &desc), about: crate::output::clip(&desc, 70), tokens: tokens(&name) + tokens(&desc) + 4, name, source: source.to_string(), off: String::new(), listed: 1 });
        }
    }
    // Instruction files: the user's, then the project's. In each place an override file takes
    // the place of the plain one, and together they are cut at `project_doc_max_bytes`.
    let limit: usize = pairs.iter().rev().find(|(t, k, _)| t.is_empty() && k == "project_doc_max_bytes").and_then(|(_, _, v)| v.parse().ok()).unwrap_or(32 * 1024);
    let mut room = limit;
    for (dir, source) in [(cfg.clone(), "user"), (ctx.root.clone(), "project")] {
        let Some(path) = ["AGENTS.override.md", "AGENTS.md"].iter().map(|n| dir.join(n)).find(|p| p.is_file()) else { continue };
        let Ok(mut text) = std::fs::read_to_string(&path) else { continue };
        let mut cut = text.len().min(room);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        room -= cut;
        if text.is_empty() {
            continue;
        }
        a.parts.push(Part {
            kind: "instructions",
            name: ctx.show(&path),
            source: source.to_string(),
            tokens: tokens(&text),
            about: format!("{:016x}", xxhash_rust::xxh3::xxh3_64(text.trim().as_bytes())),
            off: String::new(),
            matches: Some(String::new()),
            listed: 0,
        });
    }
    // MCP servers the configuration declares and has not switched off.
    let servers: BTreeSet<String> = pairs.iter().filter(|(t, ..)| t.len() >= 2 && t[0] == "mcp_servers").map(|(t, ..)| t[1].clone()).collect();
    for id in servers {
        if switch(&["mcp_servers", &id]) != Some(false) {
            a.servers.push((id, "config".to_string()));
        }
    }
    a
}

// ----- the report -----

fn tok(n: usize) -> String {
    format!("{} tok", count(n))
}

pub fn report(a: &Audit) -> String {
    let mut out = String::new();
    let line = |out: &mut String, s: &str| {
        out.push_str(s);
        out.push('\n');
    };
    let codex = a.harness == "Codex";
    if a.parts.is_empty() && a.servers.is_empty() {
        line(&mut out, &format!("{}: nothing of its configuration was found on this machine.", a.harness));
        for n in &a.notes {
            line(&mut out, n);
        }
        return out;
    }
    let sum = |kind: &str| a.parts.iter().filter(|p| p.kind == kind).map(|p| p.tokens).sum::<usize>();
    let n = |kind: &str| a.parts.iter().filter(|p| p.kind == kind).count();
    line(&mut out, &format!("{}: about {} tokens of your configuration go into every request of a session in this project.", a.harness, count(a.total())));
    if codex {
        line(&mut out, "(Estimated from the files. Codex has no command that shows these figures, and it shortens the list of skills where it passes 2% of the model's context window.)");
    } else {
        line(&mut out, "(Estimated from the files. The harness's own figures: `/context` in a session, `claude plugin details NAME`.)");
    }
    for n in &a.notes {
        line(&mut out, n);
    }
    let listed: usize = a.parts.iter().filter(|p| p.kind == "plugin").map(|p| p.listed).sum();
    for (kind, label) in [("plugin", "plugins"), ("skill", "skills of your own"), ("synced", "skills synced from your claude.ai account"), ("agent", "sub-agents"), ("instructions", "instruction files, loaded in full"), ("memory", "memory index")] {
        if n(kind) == 0 {
            continue;
        }
        let extra = match kind {
            "plugin" if codex => format!(", listing {} skills", count(listed)),
            "plugin" => format!(", listing {} skills, commands and agents", count(listed)),
            _ => String::new(),
        };
        line(&mut out, &format!("  {:>10}  {} {label}{extra}", tok(sum(kind)), n(kind)));
    }
    if !a.servers.is_empty() {
        line(&mut out, &format!("  {:>10}  {} MCP servers declared (their tools are deferred by default: names and instructions load)", "", a.servers.len()));
    }
    let langs: Vec<&str> = a.profile.iter().map(String::as_str).filter(|w| LANGUAGES.contains(w)).collect();
    if !langs.is_empty() {
        line(&mut out, &format!("This project: {}, and {} dependency names.", langs.join(", "), count(a.profile.len() - langs.len())));
    }

    // 1. The same instructions twice.
    let twice = a.repeated();
    if !twice.is_empty() {
        line(&mut out, "");
        for (copy, first) in &twice {
            line(&mut out, &format!("Loaded twice, word for word: {} ({}) repeats {} ({}), about {} tokens per request.", copy.name, copy.source, first.name, first.source, count(copy.tokens)));
            if !copy.off.is_empty() {
                line(&mut out, &format!("  To load it once here, add to .claude/settings.local.json: {}", copy.off));
            }
        }
    }
    // 2. What nothing in the project matches.
    let loose = a.unmatched();
    let saved: usize = loose.iter().map(|p| p.tokens).sum();
    if saved >= WORTH_NAMING {
        line(&mut out, "");
        line(&mut out, &format!("Nothing in this project's files matches {} parts, about {} tokens per request, heaviest first:", loose.len(), count(saved)));
        for p in loose.iter().take(20) {
            line(&mut out, &format!("  {:>10}  {} {}  ({})", tok(p.tokens), p.kind, p.name, p.about));
        }
        if loose.len() > 20 {
            line(&mut out, &format!("  … {} more", loose.len() - 20));
        }
        let plugins: Vec<&str> = loose.iter().filter(|p| p.kind == "plugin").map(|p| p.name.as_str()).collect();
        let skills: Vec<String> = loose.iter().filter(|p| p.kind == "skill").map(|p| format!("\"{}\": \"off\"", p.name)).collect();
        let agents: Vec<String> = loose.iter().filter(|p| p.kind == "agent").map(|p| format!("\"Agent({})\"", p.name)).collect();
        if codex {
            // Codex documents a switch per project for a plugin, and only a general one for a skill.
            if !plugins.is_empty() {
                line(&mut out, "To switch a plugin off for this project only, in .codex/config.toml of the project (Codex reads it once the project is trusted):");
                for id in plugins.iter().take(12) {
                    line(&mut out, &format!("    [plugins.\"{id}\"]\n    enabled = false"));
                }
            }
            let names: Vec<&str> = loose.iter().filter(|p| p.kind == "skill").map(|p| p.name.as_str()).collect();
            if let Some(first) = names.first() {
                line(&mut out, "To switch a skill off, in ~/.codex/config.toml (for every project: Codex documents no switch per project for a skill), one entry each:");
                line(&mut out, &format!("    [[skills.config]]\n    name = \"{first}\"\n    enabled = false"));
                if names.len() > 1 {
                    line(&mut out, &format!("  and the same for: {}", names[1..].join(", ")));
                }
            }
        } else {
            line(&mut out, "To switch them off for this project only (they stay installed, and it applies from the next session):");
            for id in plugins.iter().take(12) {
                line(&mut out, &format!("  claude plugin disable {id} --scope local"));
            }
            if !skills.is_empty() {
                line(&mut out, &format!("  in .claude/settings.local.json: \"skillOverrides\": {{{}}}", skills.join(", ")));
            }
            if !agents.is_empty() {
                line(&mut out, &format!("  in .claude/settings.local.json: \"permissions\": {{\"deny\": [{}]}}", agents.join(", ")));
            }
        }
        line(&mut out, "A match is a word: a language or a dependency of this project that a part names, or that it is about software work as such. Read the list before acting on it.");
    }
    // 3. Everything else, by weight.
    let mut heavy: Vec<&Part> = a.parts.iter().filter(|p| p.tokens >= WORTH_NAMING && !(p.matches.is_none() && !p.off.is_empty())).collect();
    heavy.sort_by_key(|p| std::cmp::Reverse(p.tokens));
    if !heavy.is_empty() {
        line(&mut out, "");
        line(&mut out, "The rest, heaviest first:");
        for p in heavy.iter().take(16) {
            let why = match (p.kind, p.matches.as_deref()) {
                ("instructions", _) => format!("  ({})", p.source),
                // It comes with the account, whatever the project: nothing here switches it off.
                ("synced", _) => "  (from your claude.ai account)".to_string(),
                (_, Some("general")) => "  (about software work in general)".to_string(),
                (_, Some(w)) if !w.is_empty() => format!("  (this project has: {w})"),
                _ => String::new(),
            };
            let about = if p.kind == "instructions" || p.about.is_empty() { String::new() } else { format!("  {}", p.about) };
            let kind = if p.kind == "synced" { "synced skill" } else { p.kind };
            line(&mut out, &format!("  {:>10}  {kind} {}{about}{why}", tok(p.tokens), p.name));
        }
    }
    out
}

const LANGUAGES: [&str; 24] = [
    "rust", "python", "typescript", "javascript", "golang", "java", "kotlin", "swift", "csharp", "ruby", "php", "cpp", "html", "css", "sql", "react", "vue", "svelte", "docker", "terraform",
    "jupyter", "dotnet", "ios", "frontend",
];

/// One line for the start of a session, when enough of the configuration has no place in the
/// project to be worth a look. `None` otherwise.
pub fn note(a: &Audit, cli: &str) -> Option<String> {
    let loose = a.unmatched();
    let unmatched: usize = loose.iter().map(|p| p.tokens).sum();
    let twice: usize = a.repeated().iter().map(|(c, _)| c.tokens).sum();
    if unmatched + twice < 1500 {
        return None;
    }
    let mut what: Vec<String> = Vec::new();
    if unmatched >= WORTH_NAMING {
        let names: Vec<&str> = loose.iter().take(3).map(|p| p.name.split('@').next().unwrap_or(&p.name)).collect();
        let more = if loose.len() > names.len() { format!(", +{}", loose.len() - names.len()) } else { String::new() };
        what.push(format!("about {} tokens from plugins and skills nothing in this project matches ({}{more})", count(unmatched), names.join(", ")));
    }
    if twice >= WORTH_NAMING {
        what.push(format!("about {} from an instruction file that is loaded twice", count(twice)));
    }
    Some(format!("every request of this session carries {}: `{cli} doctor context` lists them with the setting that switches each off for this project", what.join(" and ")))
}

pub fn run(root: &Path, _args: &[String]) -> String {
    let ctx = Ctx::new(root);
    let audits = [claude_code(&ctx), codex(&ctx)];
    let found: Vec<String> = audits.iter().filter(|a| !a.parts.is_empty() || !a.servers.is_empty() || !a.notes.is_empty()).map(report).collect();
    if found.is_empty() {
        return "Nothing of the configuration of Claude Code or Codex was found on this machine.\n".to_string();
    }
    found.join("\n")
}

/// The line for the start of a Claude Code session in `root`, if there is something to say.
pub fn session_note(root: &Path, cli: &str) -> Option<String> {
    note(&claude_code(&Ctx::new(root)), cli)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_of_a_skill() {
        let fm = front_matter("---\nname: deploy\ndescription: >-\n  Ship the app\n  to production.\nwhen_to_use: \"after tests pass\"\nallowed-tools: Bash\n---\n# Body\nname: not this\n");
        assert_eq!(fm["name"], "deploy");
        assert_eq!(fm["description"], "Ship the app to production.");
        assert_eq!(fm["when_to_use"], "after tests pass");
        assert!(front_matter("# no front matter\nname: x\n").is_empty());
    }

    #[test]
    fn a_part_fits_by_a_word_of_the_project_or_by_being_general() {
        let profile: BTreeSet<String> = ["rust", "cargo", "serde", "time", "color"].iter().map(|s| s.to_string()).collect();
        assert_eq!(fit(&profile, "rust-helper", "Helps with lifetimes and Cargo features").as_deref(), Some("cargo"));
        assert_eq!(fit(&profile, "timeline", "Render a video, one frame at a time, in full color"), None);
        assert_eq!(fit(&profile, "crusty", "rustic furniture catalogue"), None);
        assert_eq!(fit(&profile, "reviewer", "Reviews a pull request for bugs").as_deref(), Some("general"));
        let node: BTreeSet<String> = ["typescript", "remotion"].iter().map(|s| s.to_string()).collect();
        assert_eq!(fit(&node, "remotion-captions", "Display captions").as_deref(), Some("remotion"));
    }

    #[test]
    fn tokens_follow_the_kind_of_text() {
        let prose = "Use this skill whenever the user wants to create a chart or a dashboard from tabular data.";
        let ids = "818594764980-bq71un8dhv21bilb4kg75i2bnlu2t8s5.apps.googleusercontent.com https://x.io/a?b=1";
        let per_token = |t: &str| t.len() as f64 / tokens(t) as f64;
        assert!((2.6..3.6).contains(&per_token(prose)), "{}", tokens(prose));
        assert!((1.2..2.2).contains(&per_token(ids)), "{}", tokens(ids));
    }
}
