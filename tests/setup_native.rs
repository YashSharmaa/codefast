//! `codefast setup` through the harnesses' own commands.
//!
//! Claude Code and Codex are run for real when they are installed on this machine: linked into
//! the sandbox's `PATH`, with the sandbox as their home and configuration directory, never
//! logged in and never given a prompt. Their own `mcp get` / `mcp list` then confirm what setup
//! did. The other harnesses are stand-ins that record how they were called, which pins the
//! documented command lines and the fallback to the documented file.

#[path = "setup_support/mod.rs"]
mod sandbox;

use sandbox::*;
use serde_json::{Value, json};
use std::path::Path;

/// Run a harness program that was linked into the sandbox, in the sandbox.
fn harness(sb: &Sandbox, program: &str, args: &[&str], envs: &[(&str, &str)]) -> (bool, String) {
    let mut c = std::process::Command::new(sb.bin.join(program));
    c.env_clear().env("HOME", &sb.home).env("PATH", format!("{}:/usr/bin:/bin", sb.bin.display()));
    for (k, v) in envs {
        c.env(k, v);
    }
    let out = c.current_dir(&sb.project).args(args).stdin(std::process::Stdio::null()).output().unwrap();
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn claude_code_is_registered_by_claude_itself() {
    let sb = Sandbox::new("claude");
    if !sb.real("claude") {
        eprintln!("skipped: claude is not installed on this machine");
        return;
    }
    let config = sb.home("claude-config");
    let env = [("CLAUDE_CONFIG_DIR", config.to_str().unwrap())];
    let run = format!("run `claude mcp add --transport stdio --scope user codefast -- {BIN} mcp`");

    // The plan names the command and runs nothing.
    let plan = sb.setup_env(&["claude-code", "--with", "mcp"], &env);
    assert!(plan.ok().contains(&format!("  mcp    plan     {run}\n")), "{}", plan.stdout);
    assert!(!config.exists());

    let out = sb.setup_env(&["claude-code", "--with", "mcp", "--apply"], &env);
    assert!(out.ok().contains(&format!("  mcp    done     {run}\n")) && out.stdout.ends_with("1 changed, 0 already in place.\n"), "{}", out.stdout);
    assert!(!out.stdout.contains("instead"), "claude's own command did it: {}", out.stdout);
    let entry = json(&config.join(".claude.json"))["mcpServers"]["codefast"].clone();
    assert_eq!((&entry["type"], &entry["command"], &entry["args"]), (&json!("stdio"), &json!(BIN), &json!(["mcp"])), "{entry}");
    // Claude Code lists the server and, starting it, completes the MCP handshake with it. (The
    // health check has a deadline of its own, so a busy machine gets a second and third try.)
    let mut said = String::new();
    for _ in 0..3 {
        let (ok, text) = harness(&sb, "claude", &["mcp", "get", "codefast"], &env);
        assert!(ok && text.contains("Scope: User config") && text.contains(&format!("Command: {BIN}")) && text.contains("Args: mcp"), "{text}");
        said = text;
        if said.contains("Connected") {
            break;
        }
    }
    assert!(said.contains("Connected"), "claude's health check of `codefast mcp`: {said}");
    assert!(sb.setup_env(&["claude-code", "--with", "mcp", "--apply"], &env).ok().ends_with("0 changed, 1 already in place.\n"));

    // A registration of ours from an older install is replaced: remove, then add.
    assert!(harness(&sb, "claude", &["mcp", "remove", "codefast", "--scope", "user"], &env).0);
    assert!(harness(&sb, "claude", &["mcp", "add", "--scope", "user", "codefast", "--", "/old/place/codefast", "mcp"], &env).0);
    let out = sb.setup_env(&["claude-code", "--with", "mcp", "--apply"], &env);
    assert!(out.ok().contains(&format!("run `claude mcp remove codefast --scope user && claude mcp add --transport stdio --scope user codefast -- {BIN} mcp` (replaces `/old/place/codefast mcp`)")), "{}", out.stdout);
    assert_eq!(json(&config.join(".claude.json"))["mcpServers"]["codefast"]["command"], BIN);

    // Somebody else's server under the same name is left alone unless forced.
    assert!(harness(&sb, "claude", &["mcp", "remove", "codefast", "--scope", "user"], &env).0);
    assert!(harness(&sb, "claude", &["mcp", "add", "--scope", "user", "codefast", "--", "node", "/srv/other.js"], &env).0);
    let refused = sb.setup_env(&["claude-code", "--with", "mcp", "--apply"], &env);
    assert_eq!(refused.code, 1);
    assert!(refused.stdout.contains("has another server named codefast: `node /srv/other.js`"), "{}", refused.stdout);
    assert_eq!(json(&config.join(".claude.json"))["mcpServers"]["codefast"]["command"], "node");
    sb.setup_env(&["claude-code", "--with", "mcp", "--apply", "--force"], &env).ok();
    assert_eq!(json(&config.join(".claude.json"))["mcpServers"]["codefast"]["command"], BIN);

    // Project scope: claude writes the project's .mcp.json, with the portable command.
    let out = sb.setup_env(&["claude-code", "--with", "mcp", "--scope", "project", "--apply"], &env);
    assert!(out.ok().contains("  mcp    done     run `claude mcp add --transport stdio --scope project codefast -- codefast mcp`\n"), "{}", out.stdout);
    let project = json(&sb.project(".mcp.json"));
    assert_eq!((&project["mcpServers"]["codefast"]["command"], &project["mcpServers"]["codefast"]["args"]), (&json!("codefast"), &json!(["mcp"])), "{project}");
    // Nothing of this reached a real home: the sandbox's own is still what claude left there.
    assert!(!sb.home(".claude.json").exists() && !sb.home(".claude").exists(), "CLAUDE_CONFIG_DIR was honoured throughout");
}

#[test]
fn codex_is_registered_by_codex_itself() {
    let sb = Sandbox::new("codex");
    if !sb.real("codex") || !sb.real("node") {
        eprintln!("skipped: codex (or the node it runs on) is not installed on this machine");
        return;
    }
    let home = sb.home("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    let env = [("CODEX_HOME", home.to_str().unwrap())];
    write(&home.join("config.toml"), "model = \"gpt-5.5\"\n");

    let plan = sb.setup_env(&["codex", "--with", "mcp"], &env);
    assert!(plan.ok().contains(&format!("  mcp    plan     run `codex mcp add codefast -- {BIN} mcp`\n")), "{}", plan.stdout);
    assert_eq!(read(&home.join("config.toml")), "model = \"gpt-5.5\"\n");

    let out = sb.setup_env(&["codex", "--with", "mcp", "--apply"], &env);
    assert!(out.ok().ends_with("1 changed, 0 already in place.\n") && !out.stdout.contains("instead"), "{}", out.stdout);
    let config = read(&home.join("config.toml"));
    assert!(
        config.starts_with("model = \"gpt-5.5\"\n") && config.contains(&format!("[mcp_servers.codefast]\ncommand = \"{BIN}\"\nargs = [\"mcp\"]\n")),
        "{config}"
    );
    // Codex's own view of its configuration.
    let (ok, listed) = harness(&sb, "codex", &["mcp", "list", "--json"], &env);
    let servers: Value = serde_json::from_str(&listed).unwrap_or_else(|e| panic!("codex mcp list --json ({ok}): {e}: {listed}"));
    let ours = servers.as_array().unwrap().iter().find(|s| s["name"] == "codefast").expect("codex lists the server");
    assert_eq!(
        (&ours["enabled"], &ours["transport"]["type"], &ours["transport"]["command"], &ours["transport"]["args"]),
        (&json!(true), &json!("stdio"), &json!(BIN), &json!(["mcp"])),
        "{ours}"
    );
    assert!(sb.setup_env(&["codex", "--with", "mcp", "--apply"], &env).ok().ends_with("0 changed, 1 already in place.\n"));

    // `codex mcp add` replaces whatever carries the name without asking, so setup has to look
    // first: a server that is not ours stays.
    assert!(harness(&sb, "codex", &["mcp", "add", "codefast", "--", "node", "/srv/other.js"], &env).0);
    let refused = sb.setup_env(&["codex", "--with", "mcp", "--apply"], &env);
    assert_eq!(refused.code, 1, "{}", refused.stdout);
    assert!(refused.stdout.contains("config.toml has another server named codefast: `node /srv/other.js`"), "{}", refused.stdout);
    assert!(read(&home.join("config.toml")).contains("command = \"node\""));
    sb.setup_env(&["codex", "--with", "mcp", "--apply", "--force"], &env).ok();
    assert!(read(&home.join("config.toml")).contains(&format!("command = \"{BIN}\"")));
    // An older registration of ours is brought up to date.
    assert!(harness(&sb, "codex", &["mcp", "add", "codefast", "--", "/old/place/codefast", "mcp"], &env).0);
    let out = sb.setup_env(&["codex", "--with", "mcp", "--apply"], &env);
    assert!(out.ok().contains("(replaces `/old/place/codefast mcp`)"), "{}", out.stdout);
    let (_, got) = harness(&sb, "codex", &["mcp", "get", "codefast"], &env);
    assert!(got.contains(&format!("command: {BIN}")) && got.contains("args: mcp"), "{got}");
    assert!(!sb.home(".codex").exists(), "CODEX_HOME was honoured throughout");
}

/// A stand-in harness that only records its arguments leaves nothing behind.
const SILENT: &str = "exit 0";

#[test]
fn each_documented_command_line_is_what_gets_run() {
    let sb = Sandbox::new("argv");
    for name in ["gemini", "qwen", "copilot", "amp", "cline", "droid", "code"] {
        sb.fake(name, SILENT);
    }
    // In a dry run not one of them is started.
    let plan = sb.setup(&["gemini-cli", "qwen-code", "copilot-cli", "amp", "cline", "droid", "copilot", "--with", "mcp"]);
    assert!(plan.ok().contains(&format!("  mcp    plan     run `gemini mcp add --scope user codefast {BIN} mcp`\n")), "{}", plan.stdout);
    assert!(plan.stdout.contains(&format!("  mcp    plan     run `droid mcp add codefast '{BIN} mcp'`\n")), "{}", plan.stdout);
    for name in ["gemini", "qwen", "copilot", "amp", "cline", "droid", "code"] {
        assert_eq!(sb.calls(name), Vec::<Vec<String>>::new(), "{name}");
    }

    let out = sb.setup(&["gemini-cli", "qwen-code", "copilot-cli", "amp", "cline", "droid", "copilot", "--with", "mcp", "--apply"]);
    assert!(out.ok().ends_with("7 changed, 0 already in place.\n"), "{}", out.stdout);
    let call = |args: &[&str]| vec![args.iter().map(|a| a.to_string()).collect::<Vec<String>>()];
    assert_eq!(sb.calls("gemini"), call(&["mcp", "add", "--scope", "user", "codefast", BIN, "mcp"]));
    assert_eq!(sb.calls("qwen"), call(&["mcp", "add", "--scope", "user", "codefast", BIN, "mcp"]));
    assert_eq!(sb.calls("copilot"), call(&["mcp", "add", "codefast", "--", BIN, "mcp"]));
    assert_eq!(sb.calls("amp"), call(&["mcp", "add", "codefast", "--", BIN, "mcp"]));
    assert_eq!(sb.calls("cline"), call(&["mcp", "add", "--yes", "codefast", "--", BIN, "mcp"]));
    assert_eq!(sb.calls("droid"), call(&["mcp", "add", "codefast", &format!("{BIN} mcp")]));
    let code = sb.calls("code");
    assert_eq!((code.len(), code[0].len(), code[0][0].as_str()), (1, 2, "--add-mcp"));
    assert_eq!(serde_json::from_str::<Value>(&code[0][1]).unwrap(), json!({"name": "codefast", "type": "stdio", "command": BIN, "args": ["mcp"]}));

    // The stand-ins wrote nothing, so the documented file was written for each of them...
    assert_eq!(out.stdout.matches("the command did not register the server; wrote ").count(), 6, "{}", out.stdout);
    for file in [
        ".gemini/settings.json",
        ".qwen/settings.json",
        ".copilot/mcp-config.json",
        ".config/amp/settings.json",
        ".cline/data/settings/cline_mcp_settings.json",
        ".factory/mcp.json",
    ] {
        assert!(read(&sb.home(file)).contains(BIN), "{file}");
    }
    // ...except VS Code's profile, which only `code` knows how to find: its exit status counts.
    assert!(!sb.home("Library").exists() && !sb.home(".config/Code").exists());
    // Registered now, so a second run starts no command at all.
    let again = sb.setup(&["gemini-cli", "qwen-code", "copilot-cli", "amp", "cline", "droid", "--with", "mcp", "--apply"]);
    assert!(again.ok().ends_with("0 changed, 6 already in place.\n"), "{}", again.stdout);
    assert_eq!(sb.calls("gemini").len(), 1);
}

#[test]
fn a_command_that_registers_the_server_is_taken_at_its_word() {
    let sb = Sandbox::new("works");
    // A gemini that does what `gemini mcp add --scope S NAME COMMAND ARGS` documents.
    sb.fake(
        "gemini",
        "pwd > \"$HOME/gemini.cwd\"\nif [ \"$2\" = add ]; then dir=\"$HOME/.gemini\"; [ \"$4\" = project ] && dir=.gemini; mkdir -p \"$dir\"\n  printf '{\"theme\":\"Default\",\"mcpServers\":{\"%s\":{\"command\":\"%s\",\"args\":[\"%s\"]}}}' \"$5\" \"$6\" \"$7\" > \"$dir/settings.json\"; fi",
    );
    let out = sb.setup(&["gemini-cli", "--with", "mcp", "--apply"]);
    assert!(out.ok().ends_with("1 changed, 0 already in place.\n") && !out.stdout.contains("instead"), "{}", out.stdout);
    assert_eq!(
        json(&sb.home(".gemini/settings.json")),
        json!({"theme": "Default", "mcpServers": {"codefast": {"command": BIN, "args": ["mcp"]}}}),
        "the harness's own writing, untouched"
    );
    assert_eq!(Path::new(read(&sb.home("gemini.cwd")).trim()), sb.project, "harness commands run in the project");

    // Replacing an older entry of ours: remove first, then add.
    write(&sb.home(".gemini/settings.json"), "{\"mcpServers\":{\"codefast\":{\"command\":\"/old/codefast\",\"args\":[\"mcp\"]}}}");
    sb.setup(&["gemini-cli", "--with", "mcp", "--apply"]).ok();
    let calls = sb.calls("gemini");
    assert_eq!(calls[1], ["mcp", "remove", "--scope", "user", "codefast"]);
    assert_eq!(calls[2], ["mcp", "add", "--scope", "user", "codefast", BIN, "mcp"]);
    // Project scope goes through the same command with the portable name.
    sb.setup(&["gemini-cli", "--with", "mcp", "--scope", "project", "--apply"]).ok();
    assert_eq!(sb.calls("gemini").last().unwrap(), &["mcp", "add", "--scope", "project", "codefast", "codefast", "mcp"]);
    assert_eq!(json(&sb.project(".gemini/settings.json"))["mcpServers"]["codefast"]["command"], "codefast");
}

#[test]
fn a_command_that_fails_falls_back_to_the_documented_file() {
    let sb = Sandbox::new("fails");
    sb.fake("amp", "echo 'amp: you are not logged in' >&2\nexit 3");
    // One that waits for an answer gets none: there is no terminal behind setup.
    sb.fake("cline", "read answer || exit 9\nexit 0");
    let out = sb.setup(&["amp", "cline", "--with", "mcp", "--apply"]);
    assert!(out.ok().ends_with("2 changed, 0 already in place.\n"), "{}", out.stdout);
    assert!(out.stdout.contains("`amp mcp add` failed: amp: you are not logged in\n"), "{}", out.stdout);
    assert!(out.stdout.contains("the command did not register the server; wrote ~/.config/amp/settings.json instead\n"));
    assert!(
        out.stdout.contains("`cline mcp add` failed: no output\n") && out.stdout.contains("wrote ~/.cline/data/settings/cline_mcp_settings.json instead\n"),
        "{}",
        out.stdout
    );
    assert_eq!(json(&sb.home(".config/amp/settings.json")), json!({"amp.mcpServers": {"codefast": {"command": BIN, "args": ["mcp"]}}}));

    // Where there is no file to fall back to, the failure is the result.
    sb.fake("code", "echo 'code: unknown option --add-mcp' >&2\nexit 1");
    let out = sb.setup(&["copilot", "--with", "mcp", "--apply"]);
    assert_eq!(out.code, 1);
    assert!(
        out.stdout.contains("  mcp    failed   run `code --add-mcp ")
            && out.stdout.contains("`code --add-mcp")
            && out.stdout.contains("failed: code: unknown option --add-mcp\n"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.ends_with("0 changed, 0 already in place, 1 failed.\n") && out.stderr.contains("1 step not applied"), "{}{}", out.stdout, out.stderr);
}

#[test]
fn a_failed_command_never_costs_what_the_harness_wrote_meanwhile() {
    let sb = Sandbox::new("meanwhile");
    // Claude Code rewrites ~/.claude.json whenever it runs (start counters, the signed-in
    // account, per-project state). A `claude mcp add` that does so and then fails must not have
    // that work undone by the file written in its place.
    write(&sb.home(".claude.json"), "{\n  \"numStartups\": 1,\n  \"mcpServers\": {}\n}\n");
    sb.fake(
        "claude",
        "printf '{\\n  \"numStartups\": 2,\\n  \"oauthAccount\": {\"emailAddress\": \"written-by-the-command\"},\\n  \"mcpServers\": {\"theirs\": {\"command\": \"x\"}}\\n}\\n' > \"$HOME/.claude.json\"\necho 'claude: cannot add' >&2\nexit 1",
    );
    let out = sb.setup(&["claude-code", "--with", "mcp", "--apply"]);
    assert!(out.ok().contains("the command did not register the server; wrote ~/.claude.json instead\n"), "{}", out.stdout);
    let doc = json(&sb.home(".claude.json"));
    assert_eq!(doc["mcpServers"]["codefast"]["command"], BIN, "{doc}");
    assert_eq!(
        (&doc["numStartups"], &doc["oauthAccount"]["emailAddress"], &doc["mcpServers"]["theirs"]["command"]),
        (&json!(2), &json!("written-by-the-command"), &json!("x")),
        "what the command wrote before it failed is still there: {doc}"
    );
    // A command that leaves the file unreadable is not papered over.
    let sb = Sandbox::new("meanwhile-broken");
    sb.fake("claude", "printf '{\"mcpServers\": ' > \"$HOME/.claude.json\"\nexit 1");
    let out = sb.setup(&["claude-code", "--with", "mcp", "--apply"]);
    assert_eq!(out.code, 1, "{}", out.stdout);
    assert!(out.stdout.contains("  mcp    failed   "), "{}", out.stdout);
    assert_eq!(read(&sb.home(".claude.json")), "{\"mcpServers\": ", "left as the command left it");
}

#[test]
fn harnesses_whose_command_is_not_safely_scriptable_get_the_file() {
    let sb = Sandbox::new("file-only");
    // `opencode mcp add` is an interactive wizard in OpenCode 1.x; Cursor, Kilo Code, Zed, Kiro
    // and Crush have no command for it at all. Their programs being on PATH changes nothing.
    for name in ["opencode", "cursor", "cursor-agent", "kilo", "zed", "kiro", "crush", "goose"] {
        sb.fake(name, SILENT);
    }
    let out = sb.setup(&["--all", "--with", "mcp", "--apply"]);
    assert!(out.ok().ends_with("7 changed, 0 already in place.\n"), "{}", out.stdout);
    for name in ["opencode", "cursor", "cursor-agent", "kilo", "zed", "kiro", "crush", "goose"] {
        assert_eq!(sb.calls(name), Vec::<Vec<String>>::new(), "{name}");
    }
    for file in [
        ".config/opencode/opencode.json",
        ".cursor/mcp.json",
        ".config/kilo/kilo.jsonc",
        ".config/zed/settings.json",
        ".kiro/settings/mcp.json",
        ".config/crush/crushrc",
        ".config/goose/config.yaml",
    ] {
        assert!(read(&sb.home(file)).contains(BIN), "{file}");
    }
}

#[test]
fn a_command_that_leaves_something_running_does_not_hold_setup_up() {
    let sb = Sandbox::new("lingers");
    // Some harness commands start a daemon that keeps the terminal's streams open.
    sb.fake("amp", "echo registered\n( sleep 20 ) &\nexit 0");
    let started = std::time::Instant::now();
    let out = sb.setup(&["amp", "--with", "mcp", "--apply"]);
    assert!(out.ok().ends_with("1 changed, 0 already in place.\n"), "{}", out.stdout);
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "setup waited {:?} for a background process", started.elapsed());
}
