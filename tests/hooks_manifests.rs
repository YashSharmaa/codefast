//! The hooks files this repository ships, held against the two things they must agree with:
//! the `codefast hook` command they run, and what `codefast setup --hooks` registers for the
//! same harness.
//!
//! * `hooks/claude-code.json` is loaded by Claude Code from the plugin
//!   (`.claude-plugin/plugin.json` names it) and runs the launcher in the plugin's `bin/`;
//! * `hooks/codex.json` is a file to merge into a Codex configuration layer, because Codex
//!   loads no hooks from this plugin. It is what `setup codex --hooks --scope project` writes.
//!
//! The files are read from the repository, not restated here.
#![cfg(unix)]

#[path = "packaging/payloads/mod.rs"]
mod payloads;
#[path = "setup_support/mod.rs"]
mod sandbox;
#[path = "packaging/support.rs"]
mod support;

use payloads::{in_session, indexed, payload};
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use support::{BIN, Dir, scratch};

fn repository(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn shipped(rel: &str) -> Value {
    let text = std::fs::read_to_string(repository(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{rel} is not JSON: {e}"))
}

/// One registered handler: the event, the group's matcher, the command line and its timeout.
#[derive(Debug, Clone, PartialEq)]
struct Wired {
    event: String,
    matcher: Option<String>,
    command: String,
    timeout: Value,
}

impl Wired {
    /// The `codefast hook` subcommand the command line ends in, before the guard that keeps a
    /// missing program quiet.
    fn sub(&self) -> &str {
        let run = self.command.strip_suffix(" 2>/dev/null || true").unwrap_or(&self.command);
        let (before, sub) = run.rsplit_once(' ').expect("a subcommand");
        assert!(before.ends_with(" hook"), "{}", self.command);
        sub
    }
}

/// Every handler under the `hooks` member of a hooks file or a settings file, in order.
fn wiring(doc: &Value) -> Vec<Wired> {
    let mut out = Vec::new();
    for (event, groups) in doc["hooks"].as_object().expect("a hooks object") {
        for group in groups.as_array().expect("matcher groups") {
            for handler in group["hooks"].as_array().expect("handlers") {
                assert_eq!(handler["type"], "command", "{handler}");
                out.push(Wired {
                    event: event.clone(),
                    matcher: group.get("matcher").map(|m| m.as_str().unwrap().to_string()),
                    command: handler["command"].as_str().unwrap().to_string(),
                    timeout: handler["timeout"].clone(),
                });
            }
        }
    }
    out
}

/// The wiring without the program that is run: what two registrations for the same harness
/// must have in common.
fn shape(wired: &[Wired]) -> Vec<(String, Option<String>, String, Value)> {
    wired.iter().map(|w| (w.event.clone(), w.matcher.clone(), w.sub().to_string(), w.timeout.clone())).collect()
}

/// The recorded payload a harness sends for `event`, for a tool the group's matcher lets through.
fn payload_for(harness: &str, wired: &Wired, repo: &Path, home: &Path) -> String {
    let name = match (harness, wired.event.as_str()) {
        ("claude", "SessionStart") => "claude-session-start",
        ("claude", "PreToolUse") => "claude-pre-read",
        ("claude", "PostToolUse") => "claude-post-bash-grep",
        ("claude", "PostToolUseFailure") => "claude-post-failure-bash-grep",
        ("codex", "SessionStart") => "codex-session-start",
        ("codex", "PreToolUse") => "codex-pre-bash-cat",
        ("codex", "PostToolUse") => "codex-post-bash-rg",
        other => panic!("no recorded payload for {other:?}"),
    };
    let text = payload(name, repo, home);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["hook_event_name"], wired.event.as_str(), "{name} is a payload of that event");
    // The harness only runs the handler when the matcher lets the tool through: an exact name
    // or a `|` list in Claude Code, a regular expression in Codex. The shipped matchers are
    // plain lists, which read the same either way.
    if let Some(matcher) = &wired.matcher {
        let tool = v["tool_name"].as_str().expect("a tool payload");
        assert!(matcher.split('|').any(|m| m == tool), "{} {matcher} does not let {tool} through", wired.event);
    } else {
        assert!(v.get("tool_name").is_none(), "{}: a tool event needs a matcher", wired.event);
    }
    text
}

/// Run a registered command line the way a harness does: `sh -c LINE` in the session's
/// directory, the payload on stdin. Returns (exit status, stdout, stderr).
fn fire(line: &str, cwd: &Path, home: &Path, envs: &[(&str, &str)], stdin: &str) -> (Option<i32>, String, String) {
    let mut c = Command::new("/bin/sh");
    c.arg("-c").arg(line).current_dir(cwd).env_clear();
    c.env("HOME", home).env("PATH", "/usr/bin:/bin").env("CODEFAST_NO_DOWNLOAD", "1").env("CODEFAST_HOOK_BUDGET_MS", "5000");
    for (k, v) in envs {
        c.env(k, v);
    }
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("sh -c");
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// The reply must be the one object both harnesses accept, under the name of the event the
/// handler is registered for. Returns the context it carries.
fn context(reply: &(Option<i32>, String, String), event: &str, what: &str) -> String {
    assert_eq!((reply.0, reply.2.as_str()), (Some(0), ""), "{what}");
    let v: Value = serde_json::from_str(&reply.1).unwrap_or_else(|e| panic!("{what}: stdout is not JSON ({e}): {:?}", reply.1));
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], event, "{what}: {v}");
    assert_eq!(v.as_object().unwrap().len(), 1, "{what}: {v}");
    v["hookSpecificOutput"]["additionalContext"].as_str().unwrap_or_else(|| panic!("{what}: {v}")).to_string()
}

/// An installed plugin as Claude Code keeps it, in a directory with a space in its name.
fn installed_plugin() -> Dir {
    let dir = scratch("plug in");
    for rel in [".claude-plugin/plugin.json", "hooks/claude-code.json", "bin/codefast"] {
        let to = dir.join(rel);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(repository(rel), &to).unwrap_or_else(|e| panic!("{rel}: {e}"));
    }
    dir
}

#[test]
fn the_plugin_hooks_run_the_launcher_and_answer_under_the_event_they_are_registered_for() {
    let manifest = shipped(".claude-plugin/plugin.json");
    assert_eq!(manifest["hooks"], "./hooks/claude-code.json", "the plugin manifest names the hooks file");
    let wired = wiring(&shipped("hooks/claude-code.json"));
    assert_eq!(wired.iter().map(|w| w.event.as_str()).collect::<Vec<_>>(), ["PostToolUse", "PostToolUseFailure", "PreToolUse", "SessionStart"]);

    let (repo, home) = indexed("plugin-hooks");
    let plugin = installed_plugin();
    // The launcher finds the binary in its cache, as it does after the first download.
    let slot = home.join(".cache/codefast").join(env!("CARGO_PKG_VERSION"));
    std::fs::create_dir_all(&slot).unwrap();
    std::os::unix::fs::symlink(BIN, slot.join("codefast")).unwrap();
    let envs = [("CLAUDE_PLUGIN_ROOT", plugin.to_str().unwrap()), ("CLAUDE_PROJECT_DIR", repo.str())];
    for (n, w) in wired.iter().enumerate() {
        assert_eq!(w.command, format!("\"${{CLAUDE_PLUGIN_ROOT}}/bin/codefast\" hook {}", w.sub()), "the launcher in the plugin, quoted");
        // A session of its own each time, so that nothing was said before.
        let input = in_session(&payload_for("claude", w, &repo.dir, &home), &format!("plugin-hooks-{n}"));
        let reply = fire(&w.command, &repo.dir, &home, &envs, &input);
        let said = context(&reply, &w.event, &w.command);
        // Inside the plugin the agent's shell has the launcher on PATH: the bare name is right.
        assert!(said.contains("`codefast "), "{said}");
        // Turned off, and with no binary to be found, the same line is silent and still succeeds.
        let off = fire(&w.command, &repo.dir, &home, &[envs[0], envs[1], ("CODEFAST_HOOKS", "off")], &input);
        assert_eq!(off, (Some(0), String::new(), String::new()), "CODEFAST_HOOKS=off: {}", w.command);
        let nowhere = scratch("empty-home");
        let missing = fire(&w.command, &repo.dir, &nowhere, &envs, &input);
        assert_eq!(missing, (Some(0), String::new(), String::new()), "no binary anywhere: {}", w.command);
        assert!(!nowhere.join(".cache").exists(), "nothing was fetched");
    }
}

#[test]
fn the_codex_hooks_file_is_what_setup_writes_for_a_project() {
    let file = shipped("hooks/codex.json");
    let sb = sandbox::Sandbox::new("codex-hooks-file");
    sb.setup(&["codex", "--scope", "project", "--with", "hooks", "--apply"]).ok();
    let written = sandbox::json(&sb.project(".codex/hooks.json"));
    assert_eq!(written["hooks"], file["hooks"], "hooks/codex.json and `codefast setup codex --hooks --scope project` have drifted apart");
    assert_eq!(file.as_object().unwrap().keys().collect::<Vec<_>>(), ["description", "hooks"]);
    // Merged by hand first and registered by setup later, it is brought up to date in place:
    // never a second copy of a hook.
    sandbox::write(&sb.home(".codex/hooks.json"), &json!({"hooks": file["hooks"]}).to_string());
    sb.setup(&["codex", "--with", "hooks", "--apply"]).ok();
    let merged = wiring(&sandbox::json(&sb.home(".codex/hooks.json")));
    assert_eq!(shape(&merged), shape(&wiring(&file)), "{merged:?}");
    assert!(merged.iter().all(|w| w.command.starts_with(sandbox::BIN)), "the user's own file runs the binary by its path: {merged:?}");

    // Each line, run as Codex runs it with codefast on PATH, answers under its event.
    let (repo, home) = indexed("codex-hooks");
    let bin_dir = Path::new(BIN).parent().unwrap();
    let path = format!("{}:/usr/bin:/bin", bin_dir.display());
    for (n, w) in wiring(&file).iter().enumerate() {
        assert_eq!(w.command, format!("codefast hook {} 2>/dev/null || true", w.sub()), "codefast from PATH, quiet where it is missing");
        let input = in_session(&payload_for("codex", w, &repo.dir, &home), &format!("codex-hooks-{n}"));
        let reply = fire(&w.command, &repo.dir, &home, &[("PATH", path.as_str())], &input);
        context(&reply, &w.event, &w.command);
        // On a machine without codefast the guard keeps the line quiet and successful.
        let missing = fire(&w.command, &repo.dir, &home, &[], &input);
        assert_eq!(missing, (Some(0), String::new(), String::new()), "codefast not installed: {}", w.command);
    }
}

#[test]
fn setup_registers_for_claude_code_what_the_plugin_brings() {
    // Without the plugin, `setup claude-code --hooks` writes the same events, matchers and
    // timeouts into settings.json that the plugin's hooks file carries; only the program differs.
    let sb = sandbox::Sandbox::new("claude-hooks-file");
    sb.setup(&["claude-code", "--with", "hooks", "--apply"]).ok();
    let registered = wiring(&sandbox::json(&sb.home(".claude/settings.json")));
    let plugin = wiring(&shipped("hooks/claude-code.json"));
    let sorted = |mut v: Vec<(String, Option<String>, String, Value)>| {
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    assert_eq!(sorted(shape(&registered)), sorted(shape(&plugin)), "setup and hooks/claude-code.json have drifted apart");
    assert!(registered.iter().all(|w| w.command == format!("{} hook {} 2>/dev/null || true", sandbox::BIN, w.sub())), "{registered:?}");
}
