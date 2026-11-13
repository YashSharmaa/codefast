//! The MCP start command of every manifest this repository ships, run the way that harness
//! runs it: through the launcher in `bin/`, from the directory and with the environment the
//! harness gives a plugin's server. Each must end in the same five tools, answering for the
//! user's repository and never for the plugin's own directory.
//!
//! The manifests are read from the repository, not restated here, so a manifest that stops
//! matching the launcher or the server fails one of these tests.
#![cfg(unix)]

#[path = "packaging/support.rs"]
mod support;

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use support::*;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const TOOLS: [&str; 5] = ["search", "show", "refs", "path", "grep"];

fn repository(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn manifest(rel: &str) -> Value {
    let text = std::fs::read_to_string(repository(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{rel} is not JSON: {e}"))
}

/// An installed plugin as a harness keeps it: the manifests and the launcher in a directory of
/// their own (one with a space in its name), and no binary next to them.
fn installed_plugin() -> Dir {
    let dir = scratch("plug in");
    for rel in ["plugin.json", "mcp.json", "gemini-extension.json", ".claude-plugin/plugin.json", "bin/codefast"] {
        let to = dir.join(rel);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(repository(rel), &to).unwrap_or_else(|e| panic!("{rel}: {e}"));
    }
    dir
}

/// A home directory whose launcher cache holds the binary under test, where the launcher looks
/// for the release it belongs to: `~/.cache/codefast/<version>/codefast`. The version is the
/// crate's, so a launcher that names another release does not find it.
fn home_with_cached_binary() -> Dir {
    let home = scratch("home");
    let slot = home.join(".cache/codefast").join(VERSION);
    std::fs::create_dir_all(&slot).unwrap();
    std::os::unix::fs::symlink(BIN, slot.join("codefast")).unwrap();
    home
}

/// `program args` with nothing in its environment but what a harness passes on. The launcher
/// may never reach for the network from a test, whatever the state of the cache.
fn harness_command(program: &Path, args: &[String], cwd: &Path, home: &Path, envs: &[(&str, &str)]) -> Command {
    let mut c = Command::new(program);
    c.args(args).current_dir(cwd).env_clear();
    c.env("HOME", home).env("PATH", "/usr/bin:/bin").env("TERM", "dumb").env("CODEFAST_NO_DOWNLOAD", "1");
    for (k, v) in envs {
        c.env(k, v);
    }
    c
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().expect("an array of strings").iter().map(|s| s.as_str().expect("a string").to_string()).collect()
}

fn tool_names(list: &Value) -> Vec<String> {
    list["result"]["tools"].as_array().unwrap_or_else(|| panic!("no tools in {list}")).iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
}

#[test]
fn every_manifest_carries_the_version_of_the_binary_and_the_launcher() {
    for rel in ["plugin.json", ".claude-plugin/plugin.json", "gemini-extension.json", "server.json", "npm/codefast/package.json"] {
        assert_eq!(manifest(rel)["version"], VERSION, "{rel}");
    }
    assert_eq!(manifest("server.json")["packages"][0]["version"], VERSION);
    let launcher = std::fs::read_to_string(repository("bin/codefast")).unwrap();
    assert!(launcher.lines().any(|l| l == format!("version={VERSION}")), "bin/codefast downloads the release of the crate's version");
    // And the server says the same of itself.
    let repo = Repo::new("version");
    let mut s = Session::in_repo(&repo);
    assert_eq!(s.modern("server/discover", Value::Null)["result"]["_meta"][META_SERVER], json!({"name": "codefast", "version": VERSION}));
}

#[test]
fn the_claude_code_manifest_starts_the_server_for_the_project() {
    let (plugin, home, repo) = (installed_plugin(), home_with_cached_binary(), Repo::new("claude-manifest"));
    let entry = &manifest(".claude-plugin/plugin.json")["mcpServers"]["codefast"];
    // Claude Code fills in its variables, exports them to the server and starts it in the
    // directory of the session: here one below the top of the repository.
    let session_dir = repo.path("src");
    let fill = |s: &str| s.replace("${CLAUDE_PLUGIN_ROOT}", plugin.to_str().unwrap()).replace("${CLAUDE_PROJECT_DIR}", session_dir.to_str().unwrap());
    let program = PathBuf::from(fill(entry["command"].as_str().unwrap()));
    assert_eq!(program, plugin.join("bin/codefast"), "the manifest starts the launcher");
    let args: Vec<String> = strings(&entry["args"]).iter().map(|a| fill(a)).collect();
    let env: Vec<(String, String)> = entry["env"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), fill(v.as_str().unwrap()))).collect();
    let data = plugin.join("data");
    let mut envs: Vec<(&str, &str)> = vec![
        ("CLAUDE_PLUGIN_ROOT", plugin.to_str().unwrap()),
        ("CLAUDE_PLUGIN_DATA", data.to_str().unwrap()),
        ("CLAUDE_PROJECT_DIR", session_dir.to_str().unwrap()),
    ];
    envs.extend(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));

    let mut s = Session::spawn(harness_command(&program, &args, &session_dir, &home, &envs), home);
    // Claude Code 2.1 opens with the stateless revision.
    let discover = s.modern("server/discover", Value::Null);
    assert_eq!(discover["result"]["_meta"][META_SERVER]["version"], VERSION, "{discover}");
    assert_eq!(tool_names(&s.modern("tools/list", Value::Null)), TOOLS);
    // The session's directory is inside the repository, so the whole repository answers.
    let found = s.call("search", json!({"queries": [repo.marker]}));
    assert_eq!(found["isError"], false, "{found}");
    assert!(text(&found).starts_with("lib/marker.c:1-4"), "{}", text(&found));
    assert!(repo.path(".codefast").is_dir() && !repo.path("src/.codefast").exists(), "one index, at the top");
    let end = s.finish();
    assert_eq!((end.code, end.stderr.as_str()), (Some(0), ""));
}

#[test]
fn the_agent_plugins_manifest_starts_the_server_in_the_plugin_and_takes_the_root_per_call() {
    let entry = &manifest("mcp.json")["mcpServers"]["codefast"];
    assert_eq!((&entry["type"], &entry["command"]), (&json!("stdio"), &json!("./bin/codefast")), "a plugin-relative command without placeholders");
    // Agent Plugins clients run the command from the plugin's root, and Codex keeps the plugin
    // as the git checkout it cloned: the working directory is a work tree, of the wrong
    // repository. Codex 0.164.0 gives the server HOME, PATH, TMPDIR, TERM and its own two
    // variables; a client that sets none of them leaves only the launcher to say where the
    // plugin is.
    for names_the_plugin in [true, false] {
        let (plugin, home, repo) = (installed_plugin(), home_with_cached_binary(), Repo::new("agent-plugins-manifest"));
        std::fs::create_dir_all(plugin.join(".git")).unwrap();
        let data = plugin.join("data");
        let mut envs = vec![("TMPDIR", home.to_str().unwrap())];
        if names_the_plugin {
            envs.extend([("PLUGIN_ROOT", plugin.to_str().unwrap()), ("PLUGIN_DATA", data.to_str().unwrap())]);
        }
        let program = plugin.join(entry["command"].as_str().unwrap());
        let mut s = Session::spawn(harness_command(&program, &strings(&entry["args"]), &plugin, &home, &envs), scratch("kept"));
        // Codex 0.164.0 opens with the handshake.
        let hello = s.legacy(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "codex-mcp-client", "version": "0.164.0"}}),
        );
        assert_eq!(hello["result"]["serverInfo"]["version"], VERSION, "{hello}");
        let told = hello["result"]["instructions"].as_str().unwrap();
        assert!(told.ends_with("pass `root` (the absolute path of the repository) with every call."), "{told}");
        s.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        assert_eq!(tool_names(&s.legacy("tools/list", Value::Null)), TOOLS);

        // Without a root the plugin's own checkout is never taken for the project...
        let lost = s.legacy("tools/call", json!({"name": "search", "arguments": {"queries": [repo.marker]}}));
        assert_eq!(lost["result"]["isError"], true, "PLUGIN_ROOT set: {names_the_plugin}: {lost}");
        let why = text(&lost["result"]);
        assert!(
            why.contains("the server was started in its own plugin directory") && why.contains("Pass `root`"),
            "PLUGIN_ROOT set: {names_the_plugin}: {why}"
        );
        // ...and with one the call is answered from the repository.
        let found = s.legacy("tools/call", json!({"name": "search", "arguments": {"queries": [repo.marker], "root": repo.str()}}));
        assert_eq!(found["result"]["isError"], false, "{found}");
        assert!(text(&found["result"]).starts_with("lib/marker.c:1-4"), "{}", text(&found["result"]));
        assert!(!plugin.join(".codefast").exists(), "the plugin directory is never indexed");
        let end = s.finish();
        assert_eq!((end.code, end.stderr.as_str()), (Some(0), ""));
    }
}

#[test]
fn the_gemini_extension_manifest_names_the_workspace_on_the_command_line() {
    let (plugin, home, repo) = (installed_plugin(), home_with_cached_binary(), Repo::new("gemini-manifest"));
    let entry = &manifest("gemini-extension.json")["mcpServers"]["codefast"];
    let fill = |s: &str| s.replace("${extensionPath}", plugin.to_str().unwrap()).replace("${workspacePath}", repo.str()).replace("${/}", "/");
    let program = PathBuf::from(fill(entry["command"].as_str().unwrap()));
    assert_eq!(program, plugin.join("bin/codefast"), "the manifest starts the launcher");
    let args: Vec<String> = strings(&entry["args"]).iter().map(|a| fill(a)).collect();
    assert_eq!(args, ["mcp", "--root", repo.str()]);
    // Wherever the extension's server is started, the flag decides.
    let elsewhere = scratch("elsewhere");
    let mut s = Session::spawn(harness_command(&program, &args, &elsewhere, &home, &[]), home);
    s.legacy("initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "gemini-cli", "version": "0"}}));
    s.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    assert_eq!(tool_names(&s.legacy("tools/list", Value::Null)), TOOLS);
    let found = s.legacy("tools/call", json!({"name": "search", "arguments": {"queries": [repo.marker]}}));
    assert_eq!(found["result"]["isError"], false, "{found}");
    assert!(text(&found["result"]).starts_with("lib/marker.c:1-4"), "{}", text(&found["result"]));
    assert_eq!(s.finish().code, Some(0));
}

#[test]
fn a_plugin_without_a_binary_fails_at_once_and_says_why() {
    // The server cannot start without the binary; what matters is that the launcher says so on
    // stderr, writes nothing where the protocol goes, and does not hang the harness.
    let (plugin, home) = (installed_plugin(), scratch("home"));
    let out = harness_command(&plugin.join("bin/codefast"), &["mcp".to_string()], &plugin, &home, &[]).stdin(std::process::Stdio::null()).output().unwrap();
    assert_eq!((out.status.code(), out.stdout.as_slice()), (Some(127), &b""[..]), "{}", String::from_utf8_lossy(&out.stderr));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("codefast: the codefast binary is not installed"), "{said}");
    assert!(!home.join(".cache").exists(), "nothing was fetched or cached");
}
