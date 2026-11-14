//! `codefast doctor`: what is installed, what state the index is in, and which harnesses on
//! this machine have codefast registered. It only reads: no file is created or changed, the
//! index included, and none of the harnesses' own commands are run.

use super::formats::{self, Found, Server};
use super::harness::{self, Ctx, HARNESSES, Harness, Mcp, NAME, Scope};
use super::{Planned, RULES, SKILL};
use crate::build::{self, BASE_SEGMENT, OVERLAY_SEGMENT};
use crate::index::Segment;
use crate::output::count;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const USAGE: &str = "Check the installation: the binary, the index of this repository, and each harness found
on this machine. Nothing is created or changed.

Usage: codefast doctor [--json]
       codefast doctor context    what the harness loads into every request of a session in
                                  this project (plugins, skills, sub-agents, instruction files),
                                  what each costs, and how to switch off for this project only
                                  the parts nothing in it matches
       codefast doctor sessions   where the tokens of this project's sessions went, read from
                                  the harness's transcripts on this machine, and which habits
                                  cost the most (`codefast doctor sessions --help`)
";

/// `YYYY-MM-DD HH:MM:SS UTC` for seconds since the epoch.
pub(super) fn utc(secs: u64) -> String {
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    // Civil date from a day count (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn ago(secs: u64) -> String {
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => plural(secs / 60, "minute"),
        3600..=86_399 => plural(secs / 3600, "hour"),
        _ => plural(secs / 86_400, "day"),
    }
}

fn size(bytes: u64) -> String {
    if bytes >= 1 << 20 { format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64) } else { format!("{} KB", bytes.div_ceil(1024)) }
}

fn binary(ctx: &Ctx) -> Value {
    let real = ctx.exe.canonicalize().unwrap_or_else(|_| ctx.exe.clone());
    let on_path = ctx.which(NAME);
    let same = on_path.as_ref().is_some_and(|p| p.canonicalize().is_ok_and(|c| c == real));
    // A plugin's launcher or the npm wrapper under that name is how the binary is reached, not
    // a second program in front of it.
    let via = on_path.as_ref().filter(|_| !same).and_then(|p| harness::wrapper_kind(p));
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "path": ctx.exe.display().to_string(),
        "on_path": on_path.map(|p| p.display().to_string()),
        "on_path_is_this_binary": same,
        "on_path_via": via,
        "hooks_enabled": !super::hook::disabled(),
    })
}

/// How the working tree differs from what the index describes: changed, new and deleted files.
fn drift(root: &Path, index_dir: &Path) -> Option<(usize, usize, usize)> {
    let ix = super::hook::open_at(root, index_dir)?;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let (mut changed, mut new, mut seen) = (0usize, 0usize, 0usize);
    for file in crate::walk::walk(root, threads) {
        match ix.file_by_path(&file.rel) {
            Some(id) => {
                seen += 1;
                let rec = ix.file(id);
                if rec.size != file.size || rec.mtime_ns != file.mtime_ns {
                    changed += 1;
                }
            }
            None => new += 1,
        }
    }
    Some((changed, new, ix.files().count().saturating_sub(seen)))
}

fn index(ctx: &Ctx) -> Value {
    let dir = build::index_dir(&ctx.root);
    let base = dir.join(BASE_SEGMENT);
    let mut out = json!({"root": ctx.root.display().to_string(), "dir": dir.display().to_string(), "exists": false});
    let Ok(seg) = Segment::open(&base) else {
        if base.exists() {
            out["error"] = json!("the index file cannot be read by this version; `codefast index` rebuilds it");
        }
        return out;
    };
    let number = |key: &str| seg.meta(key).and_then(|v| v.parse::<u64>().ok());
    let bytes = [BASE_SEGMENT, OVERLAY_SEGMENT].iter().filter_map(|f| std::fs::metadata(dir.join(f)).ok()).map(|m| m.len()).sum::<u64>();
    let built = std::fs::metadata(&base).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs());
    out["exists"] = json!(true);
    out["bytes"] = json!(bytes);
    out["files"] = json!(number("files").unwrap_or(seg.n_files() as u64));
    out["symbols"] = json!(number("defs").unwrap_or(seg.defs().len() as u64));
    out["references"] = json!(number("refs"));
    out["lines"] = json!(number("lines"));
    out["built_by"] = json!(seg.meta("tool"));
    out["built_unix"] = json!(built);
    out["built_at"] = json!(built.map(utc));
    drop(seg);
    if let Some((changed, new, deleted)) = drift(&ctx.root, &dir) {
        out["fresh"] = json!(changed + new + deleted == 0);
        out["changed_files"] = json!(changed);
        out["new_files"] = json!(new);
        out["deleted_files"] = json!(deleted);
    }
    out
}

/// The rules block in the project's instruction files.
fn rules(ctx: &Ctx) -> Value {
    let mut out = serde_json::Map::new();
    for name in ["AGENTS.md", "CLAUDE.md", "GEMINI.md"] {
        let Ok(text) = std::fs::read_to_string(ctx.project(name)) else { continue };
        let state = if !formats::block_present(&text) {
            if text.contains("@AGENTS.md") { "imports AGENTS.md" } else { "no block" }
        } else if formats::block_set(&text, RULES).is_ok_and(|same| same == text) {
            "current"
        } else {
            "outdated"
        };
        out.insert(name.to_string(), json!(state));
    }
    Value::Object(out)
}

fn mcp_state(ctx: &Ctx, h: &Harness, scope: Scope) -> Option<Value> {
    let want = Server { command: ctx.command(scope), args: vec!["mcp".to_string()] };
    let target = h.mcp(ctx, scope, &want);
    let file = match &target {
        Mcp::Json { file, .. } | Mcp::Toml { file, .. } | Mcp::Yaml { file, .. } | Mcp::CrushRc { file } => file.clone(),
        Mcp::Manual(_) | Mcp::Unavailable(_) => return None,
    };
    let mut out = json!({"file": file.display().to_string()});
    match super::existing(&target, &Planned::default()) {
        Ok(Found::Missing) => out["state"] = json!("missing"),
        Ok(Found::Unreadable) => out["state"] = json!("unreadable"),
        Err(why) => {
            out["state"] = json!("unreadable");
            out["error"] = json!(why);
        }
        Ok(Found::Entry(server)) => {
            out["state"] = json!(if harness::is_codefast(&server) { "registered" } else { "other" });
            // A registration is only as good as the command it names.
            let command = Path::new(&server.command);
            let runs = if command.components().count() > 1 { command.is_file() } else { ctx.which(&server.command).is_some() };
            out["command"] = json!(super::command_line(&server));
            out["command_found"] = json!(runs);
        }
    }
    Some(out)
}

fn skill_state(ctx: &Ctx, h: &Harness, scope: Scope) -> Option<Value> {
    let file = h.skills(ctx, scope)?.join(NAME).join("SKILL.md");
    let state = match std::fs::read_to_string(&file) {
        Ok(text) if text == SKILL => "current",
        Ok(text) if super::is_our_skill(&text) => "outdated",
        Ok(_) => "other",
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "missing",
        Err(_) => "unreadable",
    };
    Some(json!({"file": file.display().to_string(), "state": state}))
}

fn hooks_state(ctx: &Ctx, h: &Harness, scope: Scope) -> Option<Value> {
    let hooks = h.hooks(ctx, scope)?;
    let registered = std::fs::read_to_string(&hooks.file).ok().and_then(|t| super::json_edit::parse(&t).ok()).map(|doc| {
        hooks.events.iter().filter(|(event, ..)| doc["hooks"][*event].as_array().is_some_and(|entries| entries.iter().any(super::is_our_hook))).count()
    });
    let state = match registered {
        Some(n) if n == hooks.events.len() => "registered",
        Some(0) | None => "missing",
        Some(_) => "partial",
    };
    Some(json!({"file": hooks.file.display().to_string(), "state": state}))
}

fn harness_report(ctx: &Ctx, h: &Harness, found: &str) -> Value {
    let mut out = json!({"id": h.id, "name": h.name, "found": found, "plugin": h.plugin(ctx)});
    let mut registered = out["plugin"].is_string();
    // The MCP server or the hooks registered once more next to a plugin that brings them: the
    // harness then starts two servers with the same tools and runs every hook twice.
    let mut twice: Vec<&str> = Vec::new();
    for scope in [Scope::User, Scope::Project] {
        let mut parts = serde_json::Map::new();
        for (key, value) in [("mcp", mcp_state(ctx, h, scope)), ("skill", skill_state(ctx, h, scope)), ("hooks", hooks_state(ctx, h, scope))] {
            if let Some(v) = value {
                registered |= key != "hooks" && matches!(v["state"].as_str(), Some("registered" | "current" | "outdated"));
                let doubles = key != "skill" && h.plugin_parts().contains(&key) && matches!(v["state"].as_str(), Some("registered" | "partial"));
                if doubles && !twice.contains(&key) {
                    twice.push(key);
                }
                parts.insert(key.to_string(), v);
            }
        }
        out[scope.word()] = Value::Object(parts);
    }
    out["registered_twice"] = json!(if out["plugin"].is_string() { twice } else { Vec::new() });
    // The plugin's own server is all there is, and this harness starts it in the plugin's
    // directory: it works, but only when the model names the repository with every call.
    let own_server = ["user", "project"].iter().any(|scope| out[*scope]["mcp"]["state"] == "registered");
    out["plugin_server_needs_root"] = json!(out["plugin"].is_string() && h.plugin_server_needs_root() && !own_server);
    if h.id == "aider" {
        let conf = std::fs::read_to_string(ctx.project(".aider.conf.yml")).unwrap_or_default();
        let block = std::fs::read_to_string(ctx.project("AGENTS.md")).is_ok_and(|t| formats::block_present(&t));
        registered = formats::aider_reads(&conf, "AGENTS.md") && block;
    }
    out["registered"] = json!(registered);
    out
}

fn collect(ctx: &Ctx) -> Value {
    let mut found = Vec::new();
    let mut absent = Vec::new();
    for h in HARNESSES {
        match h.found(ctx) {
            Some(why) => found.push(harness_report(ctx, h, &why)),
            None => absent.push(h.id),
        }
    }
    json!({"binary": binary(ctx), "index": index(ctx), "rules": rules(ctx), "harnesses": found, "not_found": absent})
}

fn text(ctx: &Ctx, r: &Value) -> String {
    let show = |path: &Value| path.as_str().map(|p| ctx.show(Path::new(p))).unwrap_or_default();
    let mut out = String::new();
    let b = &r["binary"];
    out.push_str(&format!("codefast {}\n  binary    {}\n", b["version"].as_str().unwrap_or("?"), b["path"].as_str().unwrap_or("?")));
    out.push_str(&match (b["on_path"].as_str(), b["on_path_is_this_binary"] == true, b["on_path_via"].as_str()) {
        (Some(_), true, _) => "  on PATH   yes\n".to_string(),
        (Some(script), false, Some("launcher")) => format!("  on PATH   yes, through {script} (a plugin's launcher, which finds and runs the binary)\n"),
        (Some(script), false, Some(_)) => format!("  on PATH   yes, through {script} (the npm wrapper, which runs the binary)\n"),
        (Some(other), false, None) => format!("  on PATH   another binary comes first: {other}\n"),
        (None, ..) => "  on PATH   no: agents run `codefast` by name, so put its directory on PATH\n".to_string(),
    });
    if b["hooks_enabled"] == false {
        out.push_str("  hooks     turned off by CODEFAST_HOOKS\n");
    }

    let ix = &r["index"];
    out.push_str(&format!("\nindex of {}\n", show(&ix["root"])));
    if ix["exists"] == true {
        let n = |key: &str| count(ix[key].as_u64().unwrap_or(0) as usize);
        out.push_str(&format!("  location  {} ({})\n", show(&ix["dir"]), size(ix["bytes"].as_u64().unwrap_or(0))));
        if let Some(built) = ix["built_unix"].as_u64() {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(built);
            out.push_str(&format!("  built     {} ({}) by codefast {}\n", utc(built), ago(now.saturating_sub(built)), ix["built_by"].as_str().unwrap_or("?")));
        }
        out.push_str(&format!("  contents  {} files, {} symbols, {} references\n", n("files"), n("symbols"), n("references")));
        out.push_str(&match ix["fresh"].as_bool() {
            Some(true) => "  state     current\n".to_string(),
            Some(false) => format!(
                "  state     behind the working tree: {} changed, {} new, {} deleted files; the next codefast command folds them in\n",
                n("changed_files"),
                n("new_files"),
                n("deleted_files")
            ),
            None => "  state     could not be compared with the working tree\n".to_string(),
        });
    } else if let Some(e) = ix["error"].as_str() {
        out.push_str(&format!("  state     {e}\n"));
    } else {
        out.push_str("  state     no index yet: it is built on first use, or by `codefast index`\n");
    }

    out.push_str("\nproject instructions\n");
    match r["rules"].as_object().filter(|m| !m.is_empty()) {
        Some(files) => {
            for (name, state) in files {
                let state = match state.as_str().unwrap_or("") {
                    "current" => "codefast block is current".to_string(),
                    "outdated" => "codefast block is outdated: `codefast setup agents-md --apply` updates it".to_string(),
                    "no block" => "no codefast block: `codefast setup agents-md --apply` adds one".to_string(),
                    other => other.to_string(),
                };
                out.push_str(&format!("  {name:<9} {state}\n"));
            }
        }
        None => out.push_str("  no AGENTS.md, CLAUDE.md or GEMINI.md here: `codefast setup agents-md --apply` writes AGENTS.md\n"),
    }

    out.push_str("\nharnesses found on this machine\n");
    let harnesses = r["harnesses"].as_array().cloned().unwrap_or_default();
    if harnesses.is_empty() {
        out.push_str("  none (`codefast setup --list` names the ones setup knows)\n");
    }
    for h in &harnesses {
        let id = h["id"].as_str().unwrap_or("?");
        let verdict = if h["registered"] == true { "registered".to_string() } else { format!("not registered: codefast setup {id} --apply") };
        out.push_str(&format!("  {id:<13} {verdict}\n"));
        if let Some(plugin) = h["plugin"].as_str() {
            out.push_str(&format!("      plugin  {plugin}\n"));
            let twice: Vec<&str> = h["registered_twice"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            if !twice.is_empty() {
                let what = twice.iter().map(|part| if *part == "mcp" { "the MCP server" } else { "the hooks" }).collect::<Vec<_>>().join(" and ");
                let them = if twice == ["mcp"] { "it" } else { "them" };
                out.push_str(&format!("      twice   the plugin brings {what}, and the files below register {them} again: remove one of the two\n"));
            }
            if h["plugin_server_needs_root"] == true {
                out.push_str(&format!(
                    "      mcp     the plugin's server is started in the plugin's directory and needs `root` with every call: `codefast setup {id} --apply` registers one that starts in the project\n"
                ));
            }
        }
        for scope in ["user", "project"] {
            for part in ["mcp", "skill", "hooks"] {
                let p = &h[scope][part];
                let line = match (part, p["state"].as_str().unwrap_or("")) {
                    ("mcp", "registered") if p["command_found"] == false => {
                        format!("{} runs `{}`, which does not exist", show(&p["file"]), p["command"].as_str().unwrap_or(""))
                    }
                    ("mcp", "registered") => format!("{} runs `{}`", show(&p["file"]), p["command"].as_str().unwrap_or("")),
                    ("mcp", "other") => format!("{} has another server named {NAME}: `{}`", show(&p["file"]), p["command"].as_str().unwrap_or("")),
                    (_, "unreadable") => format!("{} cannot be read", show(&p["file"])),
                    ("skill", "current") => show(&p["file"]),
                    ("skill", "outdated") => format!("{} is an older copy: `codefast setup {id} --apply` updates it", show(&p["file"])),
                    ("skill", "other") => format!("{} is another skill", show(&p["file"])),
                    ("hooks", "registered") => show(&p["file"]),
                    ("hooks", "partial") => format!("{} registers only some of the hooks", show(&p["file"])),
                    _ => continue,
                };
                out.push_str(&format!("      {part:<7} {line}\n"));
            }
        }
    }
    let absent: Vec<&str> = r["not_found"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if !absent.is_empty() {
        out.push_str(&format!("\nnot found: {}\n", absent.join(", ")));
    }
    out
}

pub(super) fn run(root: &Path, args: &[String]) -> Result<String> {
    if args.first().map(String::as_str) == Some("context") {
        return Ok(super::audit::run(root, &args[1..]));
    }
    if args.first().map(String::as_str) == Some("sessions") {
        return super::sessions::run(root, &args[1..]);
    }
    let mut as_json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => as_json = true,
            "--help" | "-h" | "help" => return Ok(USAGE.to_string()),
            other => bail!("unknown option `{other}` (usage: codefast doctor [--json])"),
        }
    }
    let ctx = Ctx::new(root);
    let report = collect(&ctx);
    if as_json { Ok(format!("{}\n", serde_json::to_string_pretty(&report)?)) } else { Ok(text(&ctx, &report)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_sizes_read_well() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00:00 UTC");
        assert_eq!(utc(1_794_535_631), "2026-11-13 02:07:11 UTC");
        assert_eq!(utc(4_102_444_799), "2099-12-31 23:59:59 UTC");
        assert_eq!(
            (ago(5).as_str(), ago(60).as_str(), ago(7_300).as_str(), ago(86_400 * 3).as_str()),
            ("just now", "1 minute ago", "2 hours ago", "3 days ago")
        );
        assert_eq!((size(10).as_str(), size(2048).as_str(), size(5 << 20).as_str()), ("1 KB", "2 KB", "5.0 MB"));
    }
}
