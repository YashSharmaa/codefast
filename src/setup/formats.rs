//! The configuration formats that are not JSON: the `config.toml` of Codex and of Mistral Vibe,
//! the `config.yaml` of Goose and of Hermes Agent, Crush's `crushrc`, Aider's `.aider.conf.yml`,
//! and the marked block in `AGENTS.md`.
//!
//! No general TOML or YAML parser is involved. Each reader recognises the one shape the harness
//! documents for an MCP server entry, each writer adds or replaces exactly those lines, and a
//! file whose entry is written some other way is reported as unreadable rather than guessed at.

use super::shell;
use anyhow::{Result, bail};

/// What a configuration entry launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub command: String,
    pub args: Vec<String>,
}

/// An entry as found in a configuration file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    Missing,
    Entry(Server),
    /// Present, but not in a form this code reads or rewrites.
    Unreadable,
}

fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// `text` followed by `block`, separated by one blank line.
fn append_block(text: &str, block: &str) -> String {
    let nl = newline(text);
    let mut out = text.to_string();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push_str(nl);
        }
        if !out.trim().is_empty() {
            out.push_str(nl);
        }
    }
    for line in block.lines() {
        out.push_str(line);
        out.push_str(nl);
    }
    out
}

/// Lines of `text` with their terminators.
fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

fn is_blank_or_comment(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#')
}

// ----- TOML (Codex) -----

fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One TOML string at the start of `s`; returns it and what follows.
fn toml_read_string(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix('\'') {
        let end = rest.find('\'')?;
        return Some((rest[..end].to_string(), &rest[end + 1..]));
    }
    let rest = s.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &rest[i + 1..])),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'u' => {
                    let hex: String = (0..4).filter_map(|_| chars.next().map(|(_, h)| h)).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                _ => return None,
            },
            c => out.push(c),
        }
    }
    None
}

fn toml_read_array(s: &str) -> Option<Vec<String>> {
    let mut rest = s.trim().strip_prefix('[')?.trim_start();
    let mut out = Vec::new();
    loop {
        if let Some(after) = rest.strip_prefix(']') {
            return is_blank_or_comment(after).then_some(out);
        }
        let (item, after) = toml_read_string(rest)?;
        out.push(item);
        rest = after.trim_start();
        rest = rest.strip_prefix(',').map(str::trim_start).unwrap_or(rest);
    }
}

/// `[a.b]` or `[a."b"]` as `a.b`; `None` for anything that is not a plain table header.
fn toml_header(line: &str) -> Option<String> {
    let t = line.trim();
    let t = t.split('#').next().unwrap_or(t).trim();
    if t.starts_with("[[") {
        return Some(String::new());
    }
    let inner = t.strip_prefix('[')?.strip_suffix(']')?;
    Some(inner.split('.').map(|part| part.trim().trim_matches(['"', '\''])).collect::<Vec<_>>().join("."))
}

/// Lines `[start, end)` of the `[mcp_servers.NAME]` table, header included.
fn toml_table(all: &[&str], name: &str) -> Option<(usize, usize)> {
    let want = format!("mcp_servers.{name}");
    let start = all.iter().position(|l| toml_header(l).as_deref() == Some(want.as_str()))?;
    let end = all[start + 1..].iter().position(|l| toml_header(l).is_some()).map(|p| start + 1 + p).unwrap_or(all.len());
    Some((start, end))
}

fn toml_key<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.trim_start().strip_prefix(key)?.trim_start();
    rest.strip_prefix('=').map(str::trim)
}

/// The `command` and `args` keys among the lines of one table.
fn toml_entry(body: &[&str]) -> Found {
    let (mut command, mut args) = (None, Some(Vec::new()));
    for line in body {
        if let Some(v) = toml_key(line, "command") {
            command = toml_read_string(v).filter(|(_, rest)| is_blank_or_comment(rest)).map(|(s, _)| s);
            if command.is_none() {
                return Found::Unreadable;
            }
        } else if let Some(v) = toml_key(line, "args") {
            args = toml_read_array(v);
        }
    }
    match (command, args) {
        (Some(command), Some(args)) => Found::Entry(Server { command, args }),
        _ => Found::Unreadable,
    }
}

/// The table `[start, end)` with its `command` and `args` lines set; any other key stays. An
/// `args` line the table lacks goes right under the header.
fn toml_replace(all: &[&str], start: usize, end: usize, server: &Server, nl: &str) -> String {
    let command = format!("command = {}", toml_string(&server.command));
    let args = format!("args = [{}]", server.args.iter().map(|a| toml_string(a)).collect::<Vec<_>>().join(", "));
    let mut out = String::new();
    for (i, line) in all.iter().enumerate() {
        let inside = i > start && i < end;
        if inside && toml_key(line, "command").is_some() {
            out.push_str(&format!("{command}{nl}"));
        } else if inside && toml_key(line, "args").is_some() {
            out.push_str(&format!("{args}{nl}"));
        } else {
            out.push_str(line);
        }
        if i == start {
            if !line.ends_with('\n') {
                out.push_str(nl);
            }
            if !all[start + 1..end].iter().any(|l| toml_key(l, "args").is_some()) {
                out.push_str(&format!("{args}{nl}"));
            }
        }
    }
    out
}

/// The `[mcp_servers.NAME]` entry of a Codex `config.toml`.
pub fn toml_get(text: &str, name: &str) -> Found {
    let all = lines(text);
    let Some((start, end)) = toml_table(&all, name) else {
        // The same server written inline (`mcp_servers.NAME = {...}`, `NAME.command = ...`).
        let dotted = format!("mcp_servers.{name}");
        let inline = all.iter().any(|l| {
            let t = l.trim_start();
            !t.starts_with('#') && !t.starts_with('[') && (t.starts_with(&dotted) || t.starts_with(&format!("mcp_servers.\"{name}\"")))
        });
        let under_parent = all.iter().position(|l| toml_header(l).as_deref() == Some("mcp_servers")).is_some_and(|p| {
            all[p + 1..]
                .iter()
                .take_while(|l| toml_header(l).is_none())
                .any(|l| l.trim_start().starts_with(name) && l.trim_start()[name.len()..].trim_start().starts_with(['=', '.']))
        });
        return if inline || under_parent { Found::Unreadable } else { Found::Missing };
    };
    toml_entry(&all[start + 1..end])
}

/// Whether the `[mcp_servers.NAME]` table holds more than `command` and `args`: other keys, or
/// sub-tables such as `[mcp_servers.NAME.env]`.
pub fn toml_has_more(text: &str, name: &str) -> bool {
    let all = lines(text);
    let Some((start, end)) = toml_table(&all, name) else { return false };
    let own = all[start + 1..end].iter().any(|l| !is_blank_or_comment(l) && toml_key(l, "command").is_none() && toml_key(l, "args").is_none());
    let sub = format!("mcp_servers.{name}.");
    own || all.iter().any(|l| toml_header(l).is_some_and(|h| h.starts_with(&sub)))
}

/// Add or update the `[mcp_servers.NAME]` table. Other keys of an existing table are kept.
pub fn toml_set(text: &str, name: &str, server: &Server) -> Result<String> {
    let nl = newline(text);
    let command = format!("command = {}", toml_string(&server.command));
    let args = format!("args = [{}]", server.args.iter().map(|a| toml_string(a)).collect::<Vec<_>>().join(", "));
    let new = match toml_get(text, name) {
        Found::Unreadable => bail!("the existing mcp_servers.{name} entry is written in a form that is not rewritten automatically"),
        Found::Missing => append_block(text, &format!("[mcp_servers.{name}]\n{command}\n{args}")),
        Found::Entry(_) => {
            let all = lines(text);
            let (start, end) = toml_table(&all, name).expect("found above");
            toml_replace(&all, start, end, server, nl)
        }
    };
    if toml_get(&new, name) != Found::Entry(server.clone()) {
        bail!("the edit could not be verified; the file was left unchanged");
    }
    Ok(new)
}

// ----- TOML (Mistral Vibe) -----

/// Whether a line is the header `[[mcp_servers]]`.
fn vibe_header(line: &str) -> bool {
    let t = line.trim();
    t.split('#').next().unwrap_or(t).trim().replace(' ', "") == "[[mcp_servers]]"
}

/// Lines `[start, end)` of the `[[mcp_servers]]` table whose `name` is NAME, header included.
fn vibe_table(all: &[&str], name: &str) -> Option<(usize, usize)> {
    (0..all.len()).filter(|&i| vibe_header(all[i])).find_map(|start| {
        let end = all[start + 1..].iter().position(|l| toml_header(l).is_some()).map(|p| start + 1 + p).unwrap_or(all.len());
        let named =
            all[start + 1..end].iter().any(|l| toml_key(l, "name").and_then(toml_read_string).is_some_and(|(n, rest)| n == name && is_blank_or_comment(rest)));
        named.then_some((start, end))
    })
}

/// The `[[mcp_servers]]` entry named NAME of a Vibe `config.toml`.
pub fn vibe_get(text: &str, name: &str) -> Found {
    let all = lines(text);
    match vibe_table(&all, name) {
        Some((start, end)) => toml_entry(&all[start + 1..end]),
        // The servers written as one inline array (`mcp_servers = [{ name = ... }]`).
        None if all.iter().any(|l| toml_key(l, "mcp_servers").is_some() && text.contains(&format!("\"{name}\""))) => Found::Unreadable,
        None => Found::Missing,
    }
}

/// Whether the entry NAME holds more than `name`, `transport`, `command` and `args`: other
/// keys, or a sub-table such as `[mcp_servers.env]` right after it.
pub fn vibe_has_more(text: &str, name: &str) -> bool {
    let all = lines(text);
    let Some((start, end)) = vibe_table(&all, name) else { return false };
    let own = ["name", "transport", "command", "args"];
    let other = all[start + 1..end].iter().any(|l| !is_blank_or_comment(l) && own.iter().all(|k| toml_key(l, k).is_none()));
    other || all.get(end).and_then(|l| toml_header(l)).is_some_and(|h| h.starts_with("mcp_servers."))
}

/// Add or update the `[[mcp_servers]]` entry named NAME. Other keys of an existing entry are kept.
pub fn vibe_set(text: &str, name: &str, server: &Server) -> Result<String> {
    let nl = newline(text);
    let new = match vibe_get(text, name) {
        Found::Unreadable => bail!("the existing mcp_servers entry `{name}` is written in a form that is not rewritten automatically"),
        Found::Missing => append_block(
            text,
            &format!(
                "[[mcp_servers]]\nname = {}\ntransport = \"stdio\"\ncommand = {}\nargs = [{}]",
                toml_string(name),
                toml_string(&server.command),
                server.args.iter().map(|a| toml_string(a)).collect::<Vec<_>>().join(", ")
            ),
        ),
        Found::Entry(_) => {
            let all = lines(text);
            let (start, end) = vibe_table(&all, name).expect("found above");
            toml_replace(&all, start, end, server, nl)
        }
    };
    if vibe_get(&new, name) != Found::Entry(server.clone()) {
        bail!("the edit could not be verified; the file was left unchanged");
    }
    Ok(new)
}

// ----- YAML (Goose) -----

fn yaml_scalar(s: &str) -> String {
    let plain = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_./-".contains(c)) && !s.starts_with('-');
    if plain { s.to_string() } else { serde_json::Value::String(s.to_string()).to_string() }
}

fn yaml_unquote(s: &str) -> Option<String> {
    let s = s.trim();
    if s.starts_with('"') {
        return serde_json::from_str::<String>(s).ok();
    }
    if let Some(inner) = s.strip_prefix('\'') {
        return inner.strip_suffix('\'').map(|x| x.replace("''", "'"));
    }
    // A plain scalar ends at a comment.
    let s = s.split(" #").next().unwrap_or(s).trim();
    (!s.is_empty() && !s.starts_with(['{', '[', '&', '*', '!', '|', '>'])).then(|| s.to_string())
}

fn yaml_flow_list(s: &str) -> Option<Vec<String>> {
    let inner = s.trim().strip_prefix('[')?.strip_suffix(']')?;
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }
    inner.split(',').map(yaml_unquote).collect()
}

/// `key:` at exactly `indent`, returning what follows the colon.
fn yaml_key<'a>(line: &'a str, indent: usize, key: &str) -> Option<&'a str> {
    if indent_of(line) != indent || is_blank_or_comment(line) {
        return None;
    }
    let rest = line[indent..].strip_prefix(key)?;
    let rest = rest.strip_prefix(':')?;
    (rest.is_empty() || rest.starts_with([' ', '\t', '\r', '\n'])).then(|| rest.trim())
}

/// The two YAML files that keep servers in a map under one top-level key: Goose's
/// `config.yaml` (`extensions:`, the program under `cmd`) and Hermes Agent's `config.yaml`
/// (`mcp_servers:`, the program under `command`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YamlStyle {
    Goose,
    Hermes,
}

impl YamlStyle {
    fn top(self) -> &'static str {
        match self {
            YamlStyle::Goose => "extensions",
            YamlStyle::Hermes => "mcp_servers",
        }
    }

    fn cmd(self) -> &'static str {
        match self {
            YamlStyle::Goose => "cmd",
            YamlStyle::Hermes => "command",
        }
    }
}

/// Lines `(start, end)` below the top-level key, and the indent of its children.
fn yaml_map(all: &[&str], top: &str) -> Option<(usize, usize, usize, bool)> {
    let at = all.iter().position(|l| yaml_key(l, 0, top).is_some())?;
    let inline = !yaml_key(all[at], 0, top).unwrap_or("").is_empty();
    let end = all[at + 1..].iter().position(|l| !is_blank_or_comment(l) && indent_of(l) == 0).map(|p| at + 1 + p).unwrap_or(all.len());
    let child = all[at + 1..end].iter().find(|l| !is_blank_or_comment(l)).map(|l| indent_of(l)).unwrap_or(2);
    Some((at, end, child, inline))
}

/// Lines `[start, end)` of the entry NAME inside the map.
fn yaml_entry(all: &[&str], top: &str, name: &str) -> Option<(usize, usize, usize)> {
    let (at, end, child, _) = yaml_map(all, top)?;
    let start = (at + 1..end).find(|&i| yaml_key(all[i], child, name).is_some())?;
    let mut stop = (start + 1..end).find(|&i| !is_blank_or_comment(all[i]) && indent_of(all[i]) <= child).unwrap_or(end);
    // Blank lines and comments after the entry's last line belong to what follows it.
    while stop > start + 1 && is_blank_or_comment(all[stop - 1]) {
        stop -= 1;
    }
    Some((start, stop, child))
}

/// The server NAME of a YAML file in `style`.
pub fn yaml_get(text: &str, style: YamlStyle, name: &str) -> Found {
    let (top, cmd) = (style.top(), style.cmd());
    let all = lines(text);
    let Some((start, stop, child)) = yaml_entry(&all, top, name) else {
        return match yaml_map(&all, top) {
            // `extensions: {...}` on one line may hold anything.
            Some((at, _, _, true)) if yaml_key(all[at], 0, top) != Some("{}") => Found::Unreadable,
            _ => Found::Missing,
        };
    };
    let body = &all[start + 1..stop];
    let Some(inner) = body.iter().find(|l| !is_blank_or_comment(l)).map(|l| indent_of(l)) else { return Found::Unreadable };
    if inner <= child {
        return Found::Unreadable;
    }
    let (mut command, mut args) = (None, Some(Vec::new()));
    for (i, line) in body.iter().enumerate() {
        if let Some(v) = yaml_key(line, inner, cmd) {
            command = yaml_unquote(v);
        } else if let Some(v) = yaml_key(line, inner, "args") {
            args = if v.is_empty() {
                // A block list: `- item` lines at the key's indent or deeper.
                body[i + 1..]
                    .iter()
                    .take_while(|l| is_blank_or_comment(l) || l.trim_start().starts_with("- ") || l.trim() == "-")
                    .filter(|l| !is_blank_or_comment(l))
                    .map(|l| yaml_unquote(l.trim_start().trim_start_matches('-')))
                    .collect()
            } else {
                yaml_flow_list(v)
            };
        }
    }
    match (command, args) {
        (Some(command), Some(args)) => Found::Entry(Server { command, args }),
        _ => Found::Unreadable,
    }
}

/// Add or replace the stdio server NAME in a YAML file in `style`. With `in_place`, an entry
/// that is already there keeps everything but its command (`cmd` in Goose): the caller says so
/// for an entry of this program that only has to follow the binary to a new path, and whose
/// arguments are unchanged.
pub fn yaml_set(text: &str, style: YamlStyle, name: &str, server: &Server, description: &str, in_place: bool) -> Result<String> {
    let (top, cmd) = (style.top(), style.cmd());
    let nl = newline(text);
    let all = lines(text);
    let block = |child: usize| {
        let (a, b) = (" ".repeat(child), " ".repeat(child + 2));
        let args = server.args.iter().map(|x| yaml_scalar(x)).collect::<Vec<_>>().join(", ");
        let lines = match style {
            YamlStyle::Goose => vec![
                format!("{a}{name}:"),
                format!("{b}name: {name}"),
                format!("{b}type: stdio"),
                format!("{b}cmd: {}", yaml_scalar(&server.command)),
                format!("{b}args: [{args}]"),
                format!("{b}enabled: true"),
                format!("{b}envs: {{}}"),
                format!("{b}timeout: 300"),
                format!("{b}description: {}", yaml_scalar(description)),
            ],
            YamlStyle::Hermes => vec![format!("{a}{name}:"), format!("{b}command: {}", yaml_scalar(&server.command)), format!("{b}args: [{args}]")],
        };
        lines.iter().map(|l| format!("{l}{nl}")).collect::<String>()
    };
    let new = match (yaml_get(text, style, name), yaml_map(&all, top)) {
        (Found::Unreadable, _) => bail!("the existing `{name}` entry under {top} is written in a form that is not rewritten automatically"),
        (Found::Entry(found), _) if in_place && found.args == server.args => {
            let (start, stop, _) = yaml_entry(&all, top, name).expect("found above");
            let inner = all[start + 1..stop].iter().find(|l| !is_blank_or_comment(l)).map(|l| indent_of(l)).unwrap_or(0);
            let mut out = String::with_capacity(text.len() + 64);
            for (i, line) in all.iter().enumerate() {
                if i > start && i < stop && yaml_key(line, inner, cmd).is_some() {
                    out.push_str(&format!("{}{cmd}: {}{}", &line[..inner], yaml_scalar(&server.command), if line.ends_with('\n') { nl } else { "" }));
                } else {
                    out.push_str(line);
                }
            }
            out
        }
        (Found::Entry(_), _) => {
            let (start, stop, child) = yaml_entry(&all, top, name).expect("found above");
            format!("{}{}{}", all[..start].concat(), block(child), all[stop..].concat())
        }
        (Found::Missing, Some((at, _, child, inline))) => {
            let head = if inline { format!("{top}:{nl}") } else { ensure_newline(all[at], nl) };
            format!("{}{}{}{}", all[..at].concat(), head, block(child), all[at + 1..].concat())
        }
        (Found::Missing, None) => append_block(text, &format!("{top}:{nl}{}", block(2))),
    };
    if yaml_get(&new, style, name) != Found::Entry(server.clone()) {
        bail!("the edit could not be verified; the file was left unchanged");
    }
    Ok(new)
}

fn ensure_newline(line: &str, nl: &str) -> String {
    if line.ends_with('\n') { line.to_string() } else { format!("{line}{nl}") }
}

// ----- crushrc (Crush) -----

/// The `mcp add NAME ...` line of a `crushrc`, if any: its index and what it launches.
fn crush_line(all: &[&str], name: &str) -> Option<(usize, Option<Server>)> {
    all.iter().enumerate().find_map(|(i, line)| {
        let words = shell::words(line)?;
        let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        if texts.len() < 3 || texts[0] != "mcp" || texts[1] != "add" || texts[2] != name {
            return None;
        }
        let (mut command, mut args, mut ok) = (None, Vec::new(), !words.iter().any(|w| w.dynamic));
        let mut k = 3;
        while k < texts.len() {
            match (texts[k], texts.get(k + 1)) {
                ("--command", Some(v)) => command = Some(v.to_string()),
                ("--args", Some(v)) => args.push(v.to_string()),
                // Anything else (`--env K V`, `--timeout N`) is the user's own addition.
                _ => ok = false,
            }
            k += 2;
        }
        Some((i, command.filter(|_| ok).map(|command| Server { command, args })))
    })
}

/// The `mcp add NAME` line of a `crushrc`.
pub fn crush_get(text: &str, name: &str) -> Found {
    match crush_line(&lines(text), name) {
        None => Found::Missing,
        Some((_, Some(server))) => Found::Entry(server),
        Some((_, None)) => Found::Unreadable,
    }
}

/// Add or replace the `mcp add NAME --command ... --args ...` line of a `crushrc`.
pub fn crush_set(text: &str, name: &str, server: &Server) -> Result<String> {
    let nl = newline(text);
    let mut line = format!("mcp add {name} --command {}", super::quote(&server.command));
    for arg in &server.args {
        line.push_str(&format!(" --args {}", super::quote(arg)));
    }
    let all = lines(text);
    let new = match crush_line(&all, name) {
        Some((_, None)) => bail!("the existing `mcp add {name}` line carries options that are not rewritten automatically"),
        Some((i, Some(_))) => format!("{}{line}{nl}{}", all[..i].concat(), all[i + 1..].concat()),
        None => {
            let mut out = text.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push_str(nl);
            }
            format!("{out}{line}{nl}")
        }
    };
    if crush_get(&new, name) != Found::Entry(server.clone()) {
        bail!("the edit could not be verified; the file was left unchanged");
    }
    Ok(new)
}

// ----- .aider.conf.yml -----

/// The files an `.aider.conf.yml` reads at startup, and how `read:` is written.
fn aider_read(all: &[&str]) -> Option<(usize, usize, Vec<String>, bool)> {
    let at = all.iter().position(|l| yaml_key(l, 0, "read").is_some())?;
    let value = yaml_key(all[at], 0, "read").unwrap_or("");
    if value.is_empty() {
        let end = (at + 1..all.len()).find(|&i| !is_blank_or_comment(all[i]) && !all[i].trim_start().starts_with('-')).unwrap_or(all.len());
        let items = all[at + 1..end].iter().filter(|l| !is_blank_or_comment(l)).filter_map(|l| yaml_unquote(l.trim_start().trim_start_matches('-'))).collect();
        return Some((at, end, items, true));
    }
    let items = match yaml_flow_list(value) {
        Some(items) => items,
        None => vec![yaml_unquote(value)?],
    };
    Some((at, at + 1, items, false))
}

pub fn aider_reads(text: &str, file: &str) -> bool {
    aider_read(&lines(text)).is_some_and(|(_, _, items, _)| items.iter().any(|i| i == file))
}

/// Make `.aider.conf.yml` read `file` in every session.
pub fn aider_add_read(text: &str, file: &str) -> Result<String> {
    if aider_reads(text, file) {
        return Ok(text.to_string());
    }
    let nl = newline(text);
    let all = lines(text);
    let new = match aider_read(&all) {
        None => {
            if all.iter().any(|l| l.trim_start().starts_with("read:") && indent_of(l) == 0) {
                bail!("the `read:` setting is written in a form that is not rewritten automatically");
            }
            append_block(text, &format!("read: [{}]", yaml_scalar(file))).replace("\n\n\n", "\n\n")
        }
        Some((at, end, _, true)) => {
            // A block list: one more item, indented like the last one.
            let last = all[at + 1..end].iter().rev().find(|l| !is_blank_or_comment(l));
            let indent = last.map(|l| indent_of(l)).unwrap_or(2);
            let insert = at + 1 + all[at + 1..end].iter().rposition(|l| !is_blank_or_comment(l)).map(|p| p + 1).unwrap_or(0);
            format!(
                "{}{}{}- {}{nl}{}",
                all[..insert].concat(),
                if all[..insert].concat().ends_with('\n') { "" } else { nl },
                " ".repeat(indent),
                yaml_scalar(file),
                all[insert..].concat()
            )
        }
        Some((at, _, mut items, false)) => {
            items.push(file.to_string());
            let list = items.iter().map(|i| yaml_scalar(i)).collect::<Vec<_>>().join(", ");
            format!("{}read: [{list}]{nl}{}", all[..at].concat(), all[at + 1..].concat())
        }
    };
    if !aider_reads(&new, file) {
        bail!("the edit could not be verified; the file was left unchanged");
    }
    Ok(new)
}

// ----- the marked block in AGENTS.md -----

pub const BLOCK_BEGIN: &str = "<!-- codefast:begin -->";
pub const BLOCK_END: &str = "<!-- codefast:end -->";

/// Insert `block` (which carries its own markers) into a Markdown file, or replace the block
/// that is already there. Everything outside the markers is kept.
pub fn block_set(text: &str, block: &str) -> Result<String> {
    let nl = newline(text);
    let block: String = block.trim_end().lines().map(|l| format!("{l}{nl}")).collect();
    match (text.matches(BLOCK_BEGIN).count(), text.matches(BLOCK_END).count()) {
        (0, 0) => Ok(append_block(text, &block)),
        (1, 1) => {
            let start = text.find(BLOCK_BEGIN).unwrap_or(0);
            let end = text.find(BLOCK_END).unwrap_or(0);
            if end < start {
                bail!("the codefast markers are out of order");
            }
            let mut after = end + BLOCK_END.len();
            after += text[after..].find('\n').map(|p| p + 1).unwrap_or(text.len() - after);
            Ok(format!("{}{}{}", &text[..start], block, &text[after..]))
        }
        _ => bail!("the file has unpaired or repeated codefast markers; fix them by hand first"),
    }
}

/// Whether the file carries a codefast block at all.
pub fn block_present(text: &str) -> bool {
    text.contains(BLOCK_BEGIN) && text.contains(BLOCK_END)
}

// ----- a diff to show -----

/// The lines that differ between two versions of a file that were changed in one place, with
/// `context` unchanged lines around them: ` ` kept, `-` removed, `+` added.
pub fn diff(old: &str, new: &str, context: usize) -> Vec<String> {
    if old == new {
        return Vec::new();
    }
    let (a, b): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let mut head = 0;
    while head < a.len() && head < b.len() && a[head] == b[head] {
        head += 1;
    }
    let mut tail = 0;
    while tail < a.len() - head && tail < b.len() - head && a[a.len() - 1 - tail] == b[b.len() - 1 - tail] {
        tail += 1;
    }
    let mut out = Vec::new();
    let from = head.saturating_sub(context);
    if from > 0 {
        out.push(format!("@@ line {} @@", from + 1));
    }
    out.extend(a[from..head].iter().map(|l| format!("  {l}")));
    out.extend(a[head..a.len() - tail].iter().map(|l| format!("- {l}")));
    out.extend(b[head..b.len() - tail].iter().map(|l| format!("+ {l}")));
    out.extend(a[a.len() - tail..].iter().take(context).map(|l| format!("  {l}")));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(command: &str) -> Server {
        Server { command: command.to_string(), args: vec!["mcp".to_string()] }
    }

    #[test]
    fn codex_toml_tables() {
        let s = server("/usr/local/bin/codefast");
        // What `codex mcp add` itself writes.
        let written = "[mcp_servers.codefast]\ncommand = \"/usr/local/bin/codefast\"\nargs = [\"mcp\"]\n";
        assert_eq!(toml_set("", "codefast", &s).unwrap(), written);
        assert_eq!(toml_get(written, "codefast"), Found::Entry(s.clone()));
        assert_eq!(toml_set(written, "codefast", &s).unwrap(), written);

        let config = "model = \"gpt-5.5\"\n\n[mcp_servers.other]\ncommand = 'npx'\nargs = ['-y', \"pkg\"] # two\n\n[features]\nx = true\n";
        assert_eq!(toml_get(config, "codefast"), Found::Missing);
        assert_eq!(toml_get(config, "other"), Found::Entry(Server { command: "npx".into(), args: vec!["-y".into(), "pkg".into()] }));
        let added = toml_set(config, "codefast", &s).unwrap();
        assert_eq!(added, format!("{config}\n{written}"));

        // An existing table keeps its other keys and its sub-tables.
        let old = "[mcp_servers.codefast]\nstartup_timeout_sec = 20\ncommand = \"/old/codefast\"\n\n[mcp_servers.codefast.tools.search]\noutput_token_limit = 30000\n";
        assert_eq!(toml_get(old, "codefast"), Found::Entry(Server { command: "/old/codefast".into(), args: vec![] }));
        assert_eq!(
            toml_set(old, "codefast", &s).unwrap(),
            "[mcp_servers.codefast]\nargs = [\"mcp\"]\nstartup_timeout_sec = 20\ncommand = \"/usr/local/bin/codefast\"\n\n[mcp_servers.codefast.tools.search]\noutput_token_limit = 30000\n"
        );
        assert!(toml_has_more(old, "codefast") && !toml_has_more(written, "codefast") && !toml_has_more(config, "codefast"));
        assert!(toml_has_more("[mcp_servers.codefast]\ncommand = \"x\"\nargs = [\"mcp\"]\n\n[mcp_servers.codefast.env]\nA = \"b\"\n", "codefast"));
        // Quoting survives a path with spaces, quotes and backslashes.
        let odd = server("C:\\Program Files\\code \"fast\"\\codefast.exe");
        assert_eq!(toml_get(&toml_set("", "codefast", &odd).unwrap(), "codefast"), Found::Entry(odd));
        // Other ways to write the same table are recognised and left alone.
        for other in [
            "mcp_servers.codefast = { command = \"x\" }\n",
            "[mcp_servers]\ncodefast = { command = \"x\" }\n",
            "[mcp_servers]\ncodefast.command = \"x\"\n",
            "[mcp_servers.codefast]\ncommand = [\"x\"]\n",
            "[mcp_servers.codefast]\ncommand = \"x\"\nargs = [\n  \"mcp\"\n]\n",
        ] {
            assert_eq!(toml_get(other, "codefast"), Found::Unreadable, "{other}");
            assert!(toml_set(other, "codefast", &s).is_err());
        }
        assert_eq!(toml_get("[mcp_servers.\"codefast\"]\ncommand = \"x\"\n", "codefast"), Found::Entry(Server { command: "x".into(), args: vec![] }));
        assert_eq!(toml_get("# [mcp_servers.codefast]\n[mcp_servers.codefast2]\ncommand = \"x\"\n", "codefast"), Found::Missing);
    }

    #[test]
    fn goose_yaml_extensions() {
        let s = server("/usr/local/bin/codefast");
        let fresh = yaml_set("", YamlStyle::Goose, "codefast", &s, "Code search", false).unwrap();
        assert_eq!(
            fresh,
            "extensions:\n  codefast:\n    name: codefast\n    type: stdio\n    cmd: /usr/local/bin/codefast\n    args: [mcp]\n    enabled: true\n    envs: {}\n    timeout: 300\n    description: \"Code search\"\n"
        );
        assert_eq!(yaml_get(&fresh, YamlStyle::Goose, "codefast"), Found::Entry(s.clone()));
        assert_eq!(yaml_set(&fresh, YamlStyle::Goose, "codefast", &s, "Code search", false).unwrap(), fresh);

        // As Goose writes its own file: block lists, keys in any order, other settings around.
        let config = "GOOSE_PROVIDER: anthropic\nextensions:\n  developer:\n    bundled: true\n    enabled: true\n    name: developer\n    timeout: 300\n    type: builtin\n  github:\n    args:\n    - -y\n    - '@modelcontextprotocol/server-github'\n    cmd: npx\n    type: stdio\nGOOSE_MODEL: claude-opus-5\n";
        assert_eq!(
            yaml_get(config, YamlStyle::Goose, "github"),
            Found::Entry(Server { command: "npx".into(), args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()] })
        );
        assert_eq!(yaml_get(config, YamlStyle::Goose, "codefast"), Found::Missing);
        let added = yaml_set(config, YamlStyle::Goose, "codefast", &s, "Code search", false).unwrap();
        assert!(added.starts_with("GOOSE_PROVIDER: anthropic\nextensions:\n  codefast:\n    name: codefast\n"), "{added}");
        assert!(
            added.ends_with(
                "  github:\n    args:\n    - -y\n    - '@modelcontextprotocol/server-github'\n    cmd: npx\n    type: stdio\nGOOSE_MODEL: claude-opus-5\n"
            ),
            "{added}"
        );
        assert_eq!(yaml_get(&added, YamlStyle::Goose, "github"), yaml_get(config, YamlStyle::Goose, "github"));
        // Replacing touches only our block.
        let moved = yaml_set(&added, YamlStyle::Goose, "codefast", &server("codefast"), "Code search", false).unwrap();
        assert_eq!(moved.replace("cmd: codefast", "cmd: /usr/local/bin/codefast"), added);
        // No extensions yet, or an empty map.
        assert_eq!(
            yaml_set("GOOSE_MODEL: x\n", YamlStyle::Goose, "codefast", &s, "d", false).unwrap().lines().take(3).collect::<Vec<_>>(),
            ["GOOSE_MODEL: x", "", "extensions:"]
        );
        assert!(yaml_set("extensions: {}\nGOOSE_MODEL: x\n", YamlStyle::Goose, "codefast", &s, "d", false).unwrap().starts_with("extensions:\n  codefast:\n"));
        // Shapes that are not rewritten.
        for other in
            ["extensions: {codefast: {cmd: x}}\n", "extensions:\n  codefast: {cmd: x, args: [mcp]}\n", "extensions:\n  codefast:\n    cmd: |\n      x\n"]
        {
            assert_eq!(yaml_get(other, YamlStyle::Goose, "codefast"), Found::Unreadable, "{other}");
            assert!(yaml_set(other, YamlStyle::Goose, "codefast", &s, "d", false).is_err());
        }
        // An entry that only follows its binary keeps every other line, comments included.
        let lived = "extensions:\n  codefast:\n    # pinned\n    args:\n    - mcp\n    cmd: /old/codefast\n    enabled: false\n    envs:\n      CODEFAST_ROOT: /r\n    timeout: 900\n  z:\n    type: builtin\n";
        assert_eq!(yaml_set(lived, YamlStyle::Goose, "codefast", &s, "d", true).unwrap(), lived.replace("/old/codefast", "/usr/local/bin/codefast"));
        // Not when the arguments change too: then the entry is written anew.
        let other = Server { command: "/usr/local/bin/codefast".into(), args: vec!["mcp".into(), "--root".into(), "/r".into()] };
        assert!(yaml_set(lived, YamlStyle::Goose, "codefast", &other, "d", true).unwrap().contains("    args: [mcp, \"--root\", /r]\n    enabled: true\n"));
        // A command that needs quoting round-trips.
        let odd = server("/Users/a b/it's: here #1/codefast");
        assert_eq!(yaml_get(&yaml_set("", YamlStyle::Goose, "codefast", &odd, "d", false).unwrap(), YamlStyle::Goose, "codefast"), Found::Entry(odd));
    }

    #[test]
    fn vibe_toml_array_of_tables() {
        let s = server("/usr/local/bin/codefast");
        // The shape the Vibe documentation gives: one `[[mcp_servers]]` table per server.
        let written = "[[mcp_servers]]\nname = \"codefast\"\ntransport = \"stdio\"\ncommand = \"/usr/local/bin/codefast\"\nargs = [\"mcp\"]\n";
        assert_eq!(vibe_set("", "codefast", &s).unwrap(), written);
        assert_eq!(vibe_get(written, "codefast"), Found::Entry(s.clone()));
        assert_eq!(vibe_set(written, "codefast", &s).unwrap(), written);
        assert!(!vibe_has_more(written, "codefast"));

        let config = "# my vibe\nactive_model = \"devstral\"\n\n[[mcp_servers]]\nname = \"linear\"\ntransport = \"http\"\nurl = \"https://mcp.linear.app/mcp\"\n\n[mcp_servers.auth]\ntype = \"oauth\"\n";
        assert_eq!(vibe_get(config, "codefast"), Found::Missing);
        assert_eq!(vibe_set(config, "codefast", &s).unwrap(), format!("{config}\n{written}"));
        // An entry of ours keeps its other keys and what follows it.
        let old = "[[mcp_servers]]\nname = 'codefast'\ntransport = \"stdio\"\ncommand = \"/old/codefast\" # mine\nargs = [\"mcp\"]\nstartup_timeout_sec = 30\n\n[[mcp_servers]]\nname = \"x\"\ncommand = \"/old/codefast\"\n";
        assert_eq!(vibe_get(old, "codefast"), Found::Entry(Server { command: "/old/codefast".into(), args: vec!["mcp".into()] }));
        assert!(vibe_has_more(old, "codefast"));
        assert_eq!(vibe_set(old, "codefast", &s).unwrap(), old.replacen("\"/old/codefast\" # mine", "\"/usr/local/bin/codefast\"", 1));
        assert!(vibe_has_more("[[mcp_servers]]\nname = \"codefast\"\ncommand = \"x\"\n[mcp_servers.env]\nA = \"b\"\n", "codefast"));
        // Written some other way: left alone.
        for other in ["mcp_servers = [{ name = \"codefast\", command = \"x\" }]\n", "[[mcp_servers]]\nname = \"codefast\"\ncommand = [\"x\"]\n"] {
            assert_eq!(vibe_get(other, "codefast"), Found::Unreadable, "{other}");
            assert!(vibe_set(other, "codefast", &s).is_err());
        }
        assert_eq!(vibe_get("# [[mcp_servers]]\n# name = \"codefast\"\n[[mcp_servers]]\nname = \"codefast2\"\ncommand = \"x\"\n", "codefast"), Found::Missing);
    }

    #[test]
    fn hermes_yaml_servers() {
        let s = server("/usr/local/bin/codefast");
        let fresh = yaml_set("", YamlStyle::Hermes, "codefast", &s, "d", false).unwrap();
        assert_eq!(fresh, "mcp_servers:\n  codefast:\n    command: /usr/local/bin/codefast\n    args: [mcp]\n");
        assert_eq!(yaml_get(&fresh, YamlStyle::Hermes, "codefast"), Found::Entry(s.clone()));
        assert_eq!(yaml_get(&fresh, YamlStyle::Goose, "codefast"), Found::Missing, "a Goose reader does not see it");
        // As the Hermes documentation writes servers: quoted strings, a flow list, other settings around.
        let config = "model:\n  default: hermes-4\nmcp_servers:\n  # docs\n  filesystem:\n    command: \"npx\"\n    args: [\"-y\", \"@modelcontextprotocol/server-filesystem\", \"/home/user/projects\"]\nterminal:\n  backend: local\n";
        assert_eq!(
            yaml_get(config, YamlStyle::Hermes, "filesystem"),
            Found::Entry(Server {
                command: "npx".into(),
                args: vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into(), "/home/user/projects".into()]
            })
        );
        let added = yaml_set(config, YamlStyle::Hermes, "codefast", &s, "d", false).unwrap();
        assert_eq!(added, config.replace("mcp_servers:\n", "mcp_servers:\n  codefast:\n    command: /usr/local/bin/codefast\n    args: [mcp]\n"));
        assert_eq!(yaml_set(&added, YamlStyle::Hermes, "codefast", &s, "d", false).unwrap(), added);
        // An entry of ours that follows its binary keeps its other keys and comments.
        let lived = "mcp_servers:\n  codefast:\n    # pinned\n    command: /old/codefast\n    args: [mcp]\n    env:\n      CODEFAST_ROOT: /r\n";
        assert_eq!(yaml_set(lived, YamlStyle::Hermes, "codefast", &s, "d", true).unwrap(), lived.replace("/old/codefast", "/usr/local/bin/codefast"));
        assert!(yaml_set("mcp_servers: {}\n", YamlStyle::Hermes, "codefast", &s, "d", false).unwrap().starts_with("mcp_servers:\n  codefast:\n"));
        assert_eq!(yaml_get("mcp_servers: {codefast: {command: x}}\n", YamlStyle::Hermes, "codefast"), Found::Unreadable);
    }

    #[test]
    fn crushrc_lines() {
        let s = server("/usr/local/bin/codefast");
        let line = "mcp add codefast --command /usr/local/bin/codefast --args mcp\n";
        assert_eq!(crush_set("", "codefast", &s).unwrap(), line);
        let rc = "# my crush\nlsp add go --command gopls\nmcp add linear --type http --url https://mcp.linear.app/mcp\n";
        assert_eq!(crush_get(rc, "codefast"), Found::Missing);
        let added = crush_set(rc, "codefast", &s).unwrap();
        assert_eq!(added, format!("{rc}{line}"));
        assert_eq!(crush_get(&added, "codefast"), Found::Entry(s.clone()));
        assert_eq!(crush_set(&added, "codefast", &s).unwrap(), added);
        let moved = crush_set(&added, "codefast", &server("/opt/code fast/codefast")).unwrap();
        assert_eq!(moved, format!("{rc}mcp add codefast --command '/opt/code fast/codefast' --args mcp\n"));
        assert_eq!(crush_get(&moved, "codefast"), Found::Entry(server("/opt/code fast/codefast")));
        // A line with the user's own options, or built from variables, is not rewritten.
        for other in ["mcp add codefast --command codefast --args mcp --timeout 10\n", "mcp add codefast --command \"$HOME/bin/codefast\" --args mcp\n"] {
            assert_eq!(crush_get(other, "codefast"), Found::Unreadable, "{other}");
            assert!(crush_set(other, "codefast", &s).is_err());
        }
        assert_eq!(crush_get("# mcp add codefast --command x\nmcp add codefast2 --command x\n", "codefast"), Found::Missing);
    }

    #[test]
    fn aider_read_setting() {
        assert_eq!(aider_add_read("", "AGENTS.md").unwrap(), "read: [AGENTS.md]\n");
        assert_eq!(aider_add_read("model: sonnet\n", "AGENTS.md").unwrap(), "model: sonnet\n\nread: [AGENTS.md]\n");
        assert_eq!(aider_add_read("read: CONVENTIONS.md\nmodel: x\n", "AGENTS.md").unwrap(), "read: [CONVENTIONS.md, AGENTS.md]\nmodel: x\n");
        assert_eq!(aider_add_read("read: [a.md, b.md]\n", "AGENTS.md").unwrap(), "read: [a.md, b.md, AGENTS.md]\n");
        assert_eq!(aider_add_read("read:\n  - a.md\n  - b.md\nmodel: x\n", "AGENTS.md").unwrap(), "read:\n  - a.md\n  - b.md\n  - AGENTS.md\nmodel: x\n");
        assert_eq!(aider_add_read("read:\n- a.md\n", "AGENTS.md").unwrap(), "read:\n- a.md\n- AGENTS.md\n");
        // The commented sample line of the stock file is not a setting.
        assert_eq!(aider_add_read("#read: xxx\n", "AGENTS.md").unwrap(), "#read: xxx\n\nread: [AGENTS.md]\n");
        for already in ["read: AGENTS.md\n", "read: [x, AGENTS.md]\n", "read:\n  - AGENTS.md\n", "read: \"AGENTS.md\"\n"] {
            assert!(aider_reads(already, "AGENTS.md"), "{already}");
            assert_eq!(aider_add_read(already, "AGENTS.md").unwrap(), already);
        }
    }

    #[test]
    fn the_marked_block() {
        let block = format!("{BLOCK_BEGIN}\n## codefast\ntext\n{BLOCK_END}\n");
        assert_eq!(block_set("", &block).unwrap(), block);
        let doc = "# Project\n\nRules.\n";
        let with = block_set(doc, &block).unwrap();
        assert_eq!(with, format!("{doc}\n{block}"));
        assert_eq!(block_set(&with, &block).unwrap(), with, "idempotent");
        // An update replaces the block where it stands and nothing else.
        let moved = format!("# Project\n\n{block}\nMore rules.\n");
        let newer = format!("{BLOCK_BEGIN}\n## codefast\nnew text\n{BLOCK_END}\n");
        assert_eq!(block_set(&moved, &newer).unwrap(), format!("# Project\n\n{newer}\nMore rules.\n"));
        assert!(block_present(&with) && !block_present(doc));
        // Damaged markers are for a person to repair.
        for bad in [format!("{BLOCK_BEGIN}\nhalf\n"), format!("{BLOCK_END}\n{BLOCK_BEGIN}\n"), format!("{block}{block}")] {
            assert!(block_set(&bad, &block).is_err(), "{bad}");
        }
        // No trailing newline, and Windows line endings.
        assert_eq!(block_set("# P", &block).unwrap(), format!("# P\n\n{block}"));
        assert_eq!(block_set("# P\r\n", &block).unwrap(), format!("# P\r\n\r\n{}", block.replace('\n', "\r\n")));
    }

    #[test]
    fn diffs_show_the_changed_lines_in_context() {
        assert_eq!(diff("a\nb\n", "a\nb\n", 2), [] as [&str; 0]);
        assert_eq!(diff("", "a\nb\n", 2), ["+ a", "+ b"]);
        assert_eq!(diff("1\n2\n3\n4\n5\n6\n", "1\n2\n3\nnew\n4\n5\n6\n", 1), ["@@ line 3 @@", "  3", "+ new", "  4"]);
        assert_eq!(diff("1\n2\n3\n", "1\ntwo\n3\n", 2), ["  1", "- 2", "+ two", "  3"]);
        assert_eq!(diff("1\n2\n", "1\n", 0), ["@@ line 2 @@", "- 2"]);
    }
}
