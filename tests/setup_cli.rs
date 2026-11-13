//! `codefast setup` as a command: what it prints, when it writes, and what it refuses to touch.
//! Every run happens in a sandbox with an empty environment.

#[path = "setup_support/mod.rs"]
mod sandbox;

use sandbox::*;
use serde_json::{Value, json};

/// The command setup registers for a hook event: the program, the event, and a guard for the one
/// failure the program cannot handle itself, which is not being there.
fn hook_command(program: &str, sub: &str) -> String {
    if cfg!(windows) { format!("{program} hook {sub}") } else { format!("{program} hook {sub} 2>/dev/null || true") }
}

/// The entry setup writes for Cursor at user scope: the absolute path of the running binary.
fn cursor_entry() -> Value {
    json!({"type": "stdio", "command": BIN, "args": ["mcp"]})
}

#[test]
fn without_a_terminal_or_apply_nothing_is_written() {
    let sb = Sandbox::new("print");
    let before = sb.snapshot();
    // stdout is a pipe here, as it is when an agent runs the command: the plan is all there is.
    let out = sb.setup(&["cursor"]);
    let text = out.ok();
    assert!(text.starts_with("Cursor (cursor), user scope\n  mcp    plan     create ~/.cursor/mcp.json\n"), "{text}");
    for line in ["+ {", "+   \"mcpServers\": {", "+     \"codefast\": {", "+       \"type\": \"stdio\",", "+       \"args\": [\"mcp\"]"] {
        assert!(text.contains(&format!("                  {line}\n")), "{line}: {text}");
    }
    assert!(text.contains(&format!("+       \"command\": \"{BIN}\",")), "{text}");
    assert!(text.contains("  skill  plan     create ~/.agents/skills/codefast/SKILL.md ("), "{text}");
    assert!(text.ends_with("Dry run: nothing was changed. 2 changes planned, 0 already in place; run again with --apply to make them.\n"), "{text}");
    assert_eq!(sb.snapshot(), before, "a dry run creates nothing");
    assert_eq!(sb.setup(&["cursor", "--print"]).ok(), text, "--print is the same thing said out loud");

    let applied = sb.setup(&["cursor", "--apply"]);
    assert!(applied.ok().ends_with("2 changed, 0 already in place.\n"), "{}", applied.stdout);
    assert!(applied.stdout.contains("  mcp    done     create ~/.cursor/mcp.json\n"));
    assert_eq!(json(&sb.home(".cursor/mcp.json")), json!({"mcpServers": {"codefast": cursor_entry()}}));
    let skill = read(&sb.home(".agents/skills/codefast/SKILL.md"));
    assert!(skill.starts_with("---\nname: codefast\n") && skill.contains("codefast search"), "the shipped skill, whole");

    // Asked again, there is nothing left to do, and nothing is rewritten.
    let settled = sb.snapshot();
    let again = sb.setup(&["cursor", "--apply"]);
    assert!(
        again.ok().contains("  mcp    ok       ~/.cursor/mcp.json already runs `") && again.stdout.ends_with("0 changed, 2 already in place.\n"),
        "{}",
        again.stdout
    );
    assert_eq!(sb.snapshot(), settled);
    assert!(sb.setup(&["cursor"]).ok().ends_with("Nothing to change: 2 already in place.\n"));

    let both = sb.setup(&["cursor", "--print", "--apply"]);
    assert_eq!((both.code, both.stdout.as_str()), (1, ""));
    assert!(both.stderr.contains("--print and --apply contradict each other"), "{}", both.stderr);
}

#[test]
fn with_no_target_the_harnesses_found_here_are_planned_and_none_is_changed() {
    let sb = Sandbox::new("unnamed");
    let none = sb.setup(&[]);
    assert!(none.ok().starts_with("codefast setup: no target named and no known harness found on this machine.\n"), "{}", none.stdout);
    assert!(
        none.stdout.contains("  claude-code   Claude Code                not found\n") && none.stdout.contains("Usage: codefast setup [TARGET...] [OPTIONS]")
    );

    // A configuration directory or a program on PATH is how a harness is found.
    std::fs::create_dir_all(sb.home(".cursor")).unwrap();
    sb.fake("gemini", "exit 0");
    let before = sb.snapshot();
    let out = sb.setup(&[]);
    let text = out.ok();
    assert!(text.starts_with("codefast setup: no target named. Found on this machine: gemini-cli, cursor.\nThis is what `codefast setup --all` would do:\n\nGemini CLI (gemini-cli), user scope\n"), "{text}");
    assert!(text.contains("\nCursor (cursor), user scope\n"));
    assert!(text.ends_with("`codefast setup --all --apply` makes them.\n"), "{text}");
    // Even --apply needs a target to be named.
    assert_eq!(sb.setup(&["--apply"]).ok(), text);
    assert_eq!(sb.snapshot(), before);
    assert_eq!(sb.calls("gemini"), Vec::<Vec<String>>::new(), "no harness command runs in a dry run");

    let listed = sb.setup(&["--list"]);
    assert!(listed.ok().contains("  cursor        Cursor                     found (~/.cursor exists)\n"), "{}", listed.stdout);
    assert!(listed.stdout.contains("  gemini-cli    Gemini CLI                 found (") && listed.stdout.contains("/bin/gemini on PATH)\n"));
    assert!(listed.stdout.contains("  agents-md     ") && listed.stdout.contains("  mcp-json      "));

    // --all is exactly the found ones.
    let all = sb.setup(&["--all", "--with", "mcp", "--apply"]);
    assert!(all.ok().ends_with("2 changed, 0 already in place.\n"), "{}", all.stdout);
    assert!(sb.home(".cursor/mcp.json").exists() && sb.home(".gemini/settings.json").exists());
    assert!(!sb.home(".codex").exists() && !sb.home(".config").exists() && !sb.home(".claude.json").exists());
}

#[test]
fn mistakes_on_the_command_line_are_named() {
    let sb = Sandbox::new("usage");
    for (args, needle) in [
        (vec!["emacs"], "unknown target `emacs`. Harnesses: claude-code, codex, gemini-cli, cursor, copilot,"),
        (vec!["--harness", "cursor,vim"], "unknown target `vim`"),
        (vec!["cursor", "--scope", "team"], "unknown scope `team`: use user or project"),
        (vec!["cursor", "--with", "mcp,lsp"], "unknown part `lsp`: use mcp, skill, hooks"),
        (vec!["cursor", "--frobnicate"], "unknown option `--frobnicate` (`codefast setup help` lists them)"),
        (vec!["cursor", "--scope"], "--scope needs user or project"),
    ] {
        let out = sb.setup(&args);
        assert_eq!((out.code, out.stdout.as_str()), (1, ""), "{args:?}");
        assert!(out.stderr.starts_with("codefast: ") && out.stderr.contains(needle), "{args:?}: {}", out.stderr);
    }
    let help = sb.setup(&["help"]);
    for word in
        ["--scope user|project", "--with LIST", "--print", "--apply", "--force", "--bin PATH", "agents-md", "mcp-json", "CLAUDE_CONFIG_DIR", "CODEX_HOME"]
    {
        assert!(help.ok().contains(word), "{word}");
    }
    // Names are forgiving about case and separators, and a harness named twice is done once.
    let out = sb.setup(&["Cursor", "--harness=CURSOR", "kilo_code", "--with", "mcp"]);
    assert_eq!(out.ok().matches("(cursor), user scope").count(), 1, "{}", out.stdout);
    assert!(out.stdout.contains("Kilo Code (kilo-code), user scope"));
}

#[test]
fn the_generic_snippet_is_printed_never_written() {
    let sb = Sandbox::new("snippet");
    let before = sb.snapshot();
    let out = sb.setup(&["mcp-json", "--apply"]);
    let text = out.ok();
    assert!(text.starts_with("Any MCP client (mcp-json)\n  mcp    print    add this to the client's MCP configuration:\n"), "{text}");
    // The indented lines are the snippet itself: valid JSON in the standard shape.
    let snippet: String = text.lines().skip(2).take_while(|l| l.starts_with("                  ")).map(|l| format!("{}\n", l.trim())).collect();
    assert_eq!(serde_json::from_str::<Value>(&snippet).unwrap(), json!({"mcpServers": {"codefast": {"command": BIN, "args": ["mcp"]}}}));
    assert!(
        text.contains("\"command\": \"") && text.find("\"command\"").unwrap() < text.find("\"args\"").unwrap(),
        "command before args, as a person writes it"
    );
    assert_eq!(sb.snapshot(), before);
    let named = sb.setup(&["mcp-json", "--bin", "codefast"]);
    assert!(named.ok().contains("\"command\": \"codefast\","), "{}", named.stdout);
}

#[test]
fn project_scope_writes_into_the_repository_with_a_portable_command() {
    let sb = Sandbox::new("project");
    let out = sb.setup(&["cursor", "claude-code", "codex", "--scope", "project", "--apply"]);
    assert!(
        out.ok().contains("  mcp    done     create .cursor/mcp.json\n")
            && out.stdout.contains("  mcp    done     create .mcp.json\n")
            && out.stdout.contains("  mcp    done     create .codex/config.toml\n"),
        "{}",
        out.stdout
    );
    // Files a team shares name the program, not one person's install path.
    assert_eq!(json(&sb.project(".cursor/mcp.json")), json!({"mcpServers": {"codefast": {"type": "stdio", "command": "codefast", "args": ["mcp"]}}}));
    assert_eq!(json(&sb.project(".mcp.json")), json!({"mcpServers": {"codefast": {"type": "stdio", "command": "codefast", "args": ["mcp"], "env": {}}}}));
    assert_eq!(read(&sb.project(".codex/config.toml")), "[mcp_servers.codefast]\ncommand = \"codefast\"\nargs = [\"mcp\"]\n");
    assert!(sb.project(".agents/skills/codefast/SKILL.md").exists() && sb.project(".claude/skills/codefast/SKILL.md").exists());
    assert_eq!(std::fs::read_dir(&sb.home).unwrap().count(), 0, "nothing outside the project");
    // The scope can be named from a subdirectory: the repository root is where files go.
    std::fs::create_dir_all(sb.project("src/deep")).unwrap();
    let mut c = std::process::Command::new(BIN);
    let from_below = c
        .env_clear()
        .env("HOME", &sb.home)
        .env("PATH", "/usr/bin:/bin")
        .current_dir(sb.project("src/deep"))
        .args(["setup", "zed", "--scope=project", "--apply"])
        .output()
        .unwrap();
    assert!(from_below.status.success());
    assert!(sb.project(".zed/settings.json").exists() && !sb.project("src/deep/.zed").exists());
    // --bin overrides the command at either scope.
    let out = sb.setup(&["kiro", "--scope", "project", "--bin", "/opt/tools/codefast", "--with", "mcp", "--apply"]);
    assert!(out.ok().contains("1 changed"));
    assert_eq!(json(&sb.project(".kiro/settings/mcp.json"))["mcpServers"]["codefast"]["command"], "/opt/tools/codefast");
}

#[test]
fn an_entry_setup_did_not_write_is_never_replaced_without_force() {
    let sb = Sandbox::new("foreign");
    let file = sb.home(".cursor/mcp.json");
    let foreign = "{\n  \"mcpServers\": {\n    \"codefast\": {\"command\": \"node\", \"args\": [\"/srv/other-codefast.js\"]},\n    \"github\": {\"command\": \"gh-mcp\"}\n  }\n}\n";
    write(&file, foreign);
    for args in [vec!["cursor", "--with", "mcp"], vec!["cursor", "--with", "mcp", "--apply"]] {
        let out = sb.setup(&args);
        assert!(out.stdout.contains("  mcp    refused  ~/.cursor/mcp.json has another server named codefast: `node /srv/other-codefast.js`\n                  left as it is; --force replaces it\n"), "{args:?}: {}", out.stdout);
        assert_eq!(read(&file), foreign, "{args:?}");
    }
    // Applying and being refused is a failure the caller can see; a dry run is only a report.
    assert_eq!(sb.setup(&["cursor", "--with", "mcp"]).code, 0);
    let refused = sb.setup(&["cursor", "--with", "mcp", "--apply"]);
    assert_eq!(refused.code, 1);
    assert!(
        refused.stdout.ends_with("0 changed, 0 already in place, 1 refused (see --force).\n")
            && refused.stderr.contains("setup did not complete: 1 step not applied"),
        "{}{}",
        refused.stdout,
        refused.stderr
    );

    let forced = sb.setup(&["cursor", "--with", "mcp", "--apply", "--force"]);
    assert!(forced.ok().contains("  mcp    done     edit ~/.cursor/mcp.json (replaces `node /srv/other-codefast.js`)\n"), "{}", forced.stdout);
    assert!(
        forced.stdout.contains("- \"codefast\": {\"command\": \"node\", \"args\": [\"/srv/other-codefast.js\"]},")
            || forced.stdout.contains("-     \"codefast\": {\"command\": \"node\""),
        "the diff shows what went: {}",
        forced.stdout
    );
    let doc = json(&file);
    assert_eq!(doc["mcpServers"]["codefast"], cursor_entry());
    assert_eq!(doc["mcpServers"]["github"], json!({"command": "gh-mcp"}), "the neighbours stay");

    // An entry of ours that points somewhere else (an old install) is simply brought up to date.
    write(&file, "{\"mcpServers\": {\"codefast\": {\"command\": \"/old/place/codefast\", \"args\": [\"mcp\"]}}}\n");
    let moved = sb.setup(&["cursor", "--with", "mcp", "--apply"]);
    assert!(moved.ok().contains("(replaces `/old/place/codefast mcp`)"), "{}", moved.stdout);
    assert_eq!(json(&file)["mcpServers"]["codefast"], cursor_entry());

    // The same for a skill directory that holds something else.
    let skill = sb.home(".agents/skills/codefast/SKILL.md");
    write(&skill, "---\nname: something-else\ndescription: not ours\n---\n# other\n");
    let out = sb.setup(&["cursor", "--with", "skill", "--apply"]);
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("  skill  refused  ~/.agents/skills/codefast/SKILL.md is another skill\n"), "{}", out.stdout);
    assert!(read(&skill).contains("something-else"));
    // An older copy of our own skill is updated.
    write(&skill, "---\nname: codefast\ndescription: an older text\n---\n# codefast\nold\n");
    let out = sb.setup(&["cursor", "--with", "skill", "--apply"]);
    assert!(out.ok().contains("  skill  done     edit ~/.agents/skills/codefast/SKILL.md\n"), "{}", out.stdout);
    assert!(read(&skill).contains("codefast search"));
}

#[test]
fn bringing_our_own_entry_up_to_date_keeps_what_its_owner_set_in_it() {
    let sb = Sandbox::new("keep");
    // An entry of ours from an older install. Cline writes the tools a user approved into it;
    // people add a root, a timeout, or switch the server off for a while. Moving the command to
    // the new binary must not reset any of that.
    let cline = sb.home(".cline/data/settings/cline_mcp_settings.json");
    write(
        &cline,
        "{\n  \"mcpServers\": {\n    \"codefast\": {\n      \"command\": \"/old/place/codefast\",\n      \"args\": [\"mcp\"],\n      \"disabled\": true,\n      \"autoApprove\": [\"search\", \"show\"], // approved in the UI\n      \"env\": {\"CODEFAST_ROOT\": \"/work/repo\"},\n      \"timeout\": 120\n    }\n  }\n}\n",
    );
    let goose = sb.home(".config/goose/config.yaml");
    let goose_before = "extensions:\n  codefast:\n    name: codefast\n    type: stdio\n    cmd: /old/place/codefast\n    args:\n    - mcp\n    enabled: false\n    envs:\n      CODEFAST_ROOT: /work/repo\n    timeout: 900\n  other:\n    type: builtin\nGOOSE_MODEL: x\n";
    write(&goose, goose_before);
    // Claude Code's own `mcp remove` + `mcp add` would drop the environment of the old entry:
    // an entry that carries settings is edited where it stands instead.
    let claude = sb.home(".claude.json");
    write(
        &claude,
        "{\n  \"mcpServers\": {\n    \"codefast\": {\n      \"type\": \"stdio\",\n      \"command\": \"/old/place/codefast\",\n      \"args\": [\"mcp\"],\n      \"env\": {\"CODEFAST_ROOT\": \"/work/repo\"}\n    }\n  }\n}\n",
    );
    sb.fake("claude", "exit 0");
    let out = sb.setup(&["cline", "goose", "claude-code", "--with", "mcp", "--apply"]);
    assert!(out.ok().ends_with("3 changed, 0 already in place.\n"), "{}", out.stdout);
    assert_eq!(out.stdout.matches("(replaces `/old/place/codefast mcp`)").count(), 3, "{}", out.stdout);
    assert_eq!(
        read(&cline),
        format!(
            "{{\n  \"mcpServers\": {{\n    \"codefast\": {{\n      \"command\": \"{BIN}\",\n      \"args\": [\"mcp\"],\n      \"disabled\": true,\n      \"autoApprove\": [\"search\", \"show\"], // approved in the UI\n      \"env\": {{\"CODEFAST_ROOT\": \"/work/repo\"}},\n      \"timeout\": 120\n    }}\n  }}\n}}\n"
        ),
        "only the command moved"
    );
    assert_eq!(read(&goose), goose_before.replace("/old/place/codefast", BIN), "only the command moved");
    assert_eq!(json(&claude)["mcpServers"]["codefast"], json!({"type": "stdio", "command": BIN, "args": ["mcp"], "env": {"CODEFAST_ROOT": "/work/repo"}}));
    assert_eq!(sb.calls("claude"), Vec::<Vec<String>>::new(), "no remove-and-add for an entry with settings of its own");
    // Up to date now, settings and all.
    assert!(sb.setup(&["cline", "goose", "claude-code", "--with", "mcp", "--apply"]).ok().ends_with("0 changed, 3 already in place.\n"));

    // Somebody else's server under the name is another matter: forced, it is replaced whole.
    write(
        &cline,
        "{\"mcpServers\": {\"codefast\": {\"command\": \"node\", \"args\": [\"/srv/x.js\"], \"env\": {\"TOKEN\": \"t\"}, \"autoApprove\": [\"deploy\"]}}}",
    );
    sb.setup(&["cline", "--with", "mcp", "--apply", "--force"]).ok();
    assert_eq!(json(&cline)["mcpServers"]["codefast"], json!({"command": BIN, "args": ["mcp"], "disabled": false, "autoApprove": []}));
    // Arguments the owner added after `mcp` (a pinned root) are theirs as well, in every format.
    write(&goose, "extensions:\n  codefast:\n    cmd: /old/place/codefast\n    args: [mcp, --root, /work/repo]\n    type: stdio\n    timeout: 900\n");
    write(
        &sb.home(".codex/config.toml"),
        "[mcp_servers.codefast]\ncommand = \"/old/place/codefast\"\nargs = [\"mcp\", \"--root\", \"/work/repo\"]\nstartup_timeout_sec = 20\n",
    );
    write(&sb.home(".config/crush/crushrc"), "mcp add codefast --command /old/place/codefast --args mcp --args --root --args /work/repo\n");
    write(
        &sb.home(".cursor/mcp.json"),
        "{\"mcpServers\": {\"codefast\": {\"command\": \"/old/place/codefast\", \"args\": [\"mcp\", \"--root\", \"/work/repo\"]}}}",
    );
    write(
        &sb.home(".config/opencode/opencode.json"),
        "{\"mcp\": {\"codefast\": {\"type\": \"local\", \"command\": [\"/old/place/codefast\", \"mcp\", \"--root\", \"/work/repo\"], \"enabled\": false}}}",
    );
    let out = sb.setup(&["goose", "codex", "crush", "cursor", "opencode", "--with", "mcp", "--apply"]);
    assert!(out.ok().ends_with("5 changed, 0 already in place.\n"), "{}", out.stdout);
    assert_eq!(read(&goose), format!("extensions:\n  codefast:\n    cmd: {BIN}\n    args: [mcp, --root, /work/repo]\n    type: stdio\n    timeout: 900\n"));
    assert_eq!(
        read(&sb.home(".codex/config.toml")),
        format!("[mcp_servers.codefast]\ncommand = \"{BIN}\"\nargs = [\"mcp\", \"--root\", \"/work/repo\"]\nstartup_timeout_sec = 20\n")
    );
    assert_eq!(read(&sb.home(".config/crush/crushrc")), format!("mcp add codefast --command {BIN} --args mcp --args --root --args /work/repo\n"));
    // (What an entry written today has and the old one lacks is added at its end: Cursor's `type`.)
    assert_eq!(
        read(&sb.home(".cursor/mcp.json")),
        format!("{{\"mcpServers\": {{\"codefast\": {{\"command\": \"{BIN}\", \"args\": [\"mcp\", \"--root\", \"/work/repo\"], \"type\": \"stdio\"}}}}}}")
    );
    assert_eq!(
        json(&sb.home(".config/opencode/opencode.json"))["mcp"]["codefast"],
        json!({"type": "local", "command": [BIN, "mcp", "--root", "/work/repo"], "enabled": false})
    );
    assert!(sb.setup(&["goose", "codex", "crush", "cursor", "opencode", "--with", "mcp", "--apply"]).ok().ends_with("0 changed, 5 already in place.\n"));
}

#[test]
fn a_settings_file_keeps_its_comments_order_and_formatting() {
    let sb = Sandbox::new("jsonc");
    let file = sb.home(".config/zed/settings.json");
    let settings = "// Zed settings\n//\n// For information on how to configure Zed, see the documentation.\n{\n  \"vim_mode\": true, // muscle memory\n  \"ui_font_size\": 16,\n  /* servers the assistant may use */\n  \"context_servers\": {\n    \"postgres\": {\n      \"command\": \"pg-mcp\",\n      \"args\": [],\n    },\n  },\n  \"theme\": \"One Dark\",\n}\n";
    write(&file, settings);
    let plan = sb.setup(&["zed"]);
    // The plan is the diff: the added lines between the two that surround them, nothing else.
    let shown = [
        "@@ line 12 @@".to_string(),
        "      },".to_string(),
        "+     \"codefast\": {".to_string(),
        format!("+       \"command\": \"{BIN}\","),
        "+       \"args\": [\"mcp\"],".to_string(),
        "+       \"env\": {}".to_string(),
        "+     }".to_string(),
        "    },".to_string(),
    ];
    let detail: String = shown.iter().map(|l| format!("{}{l}\n", " ".repeat(18))).collect();
    let expect = format!(
        "Zed (zed), user scope\n  mcp    plan     edit ~/.config/zed/settings.json\n{detail}\nDry run: nothing was changed. 1 change planned, 0 already in place; run again with --apply to make them.\n"
    );
    assert_eq!(plan.ok(), expect);
    sb.setup(&["zed", "--apply"]).ok();
    let after = read(&file);
    let added = format!("    \"codefast\": {{\n      \"command\": \"{BIN}\",\n      \"args\": [\"mcp\"],\n      \"env\": {{}}\n    }}\n");
    assert_eq!(after, settings.replace("    },\n  },\n", &format!("    }},\n{added}  }},\n")), "everything else byte for byte");
    assert!(sb.setup(&["zed", "--apply"]).ok().ends_with("0 changed, 1 already in place.\n"));
    assert_eq!(read(&file), after);
}

#[test]
fn a_file_setup_cannot_read_safely_is_left_to_its_owner() {
    let sb = Sandbox::new("broken");
    let cases = [
        (".cursor/mcp.json", "{\"mcpServers\": {\"x\": ", "cursor", "not valid JSON"),
        (".cursor/mcp.json", "[1, 2, 3]\n", "cursor", "top level of the file is not a JSON object"),
        (".cursor/mcp.json", "{\"mcpServers\": [\"a\"]}\n", "cursor", "`mcpServers` is not an object"),
        (
            ".cursor/mcp.json",
            "{\"mcpServers\": {\"codefast\": \"just a string\"}}\n",
            "cursor",
            "holds a codefast entry in a form that is not rewritten automatically",
        ),
        (".codex/config.toml", "mcp_servers.codefast = { command = \"x\" }\n", "codex", "holds a codefast entry in a form that is not rewritten automatically"),
        (".config/goose/config.yaml", "extensions: {codefast: {cmd: x}}\n", "goose", "holds a codefast entry in a form that is not rewritten automatically"),
        (
            ".config/crush/crushrc",
            "mcp add codefast --command \"$HOME/bin/codefast\" --args mcp\n",
            "crush",
            "holds a codefast entry in a form that is not rewritten automatically",
        ),
    ];
    for (rel, content, harness, why) in cases {
        let file = sb.home(rel);
        write(&file, content);
        let out = sb.setup(&[harness, "--with", "mcp", "--apply", "--force"]);
        assert_eq!(out.code, 0, "{rel}: a file to fix by hand is not a failure of setup: {}", out.stderr);
        assert!(out.stdout.contains("  mcp    manual   ") && out.stdout.contains(why), "{rel}: {}", out.stdout);
        assert!(out.stdout.ends_with("0 changed, 0 already in place, 1 left to do by hand.\n"), "{}", out.stdout);
        assert_eq!(read(&file), content, "{rel} untouched");
        std::fs::remove_file(&file).unwrap();
    }
    // Bytes that are not text at all.
    std::fs::write(sb.home(".cursor/mcp.json"), [0xff, 0xfe, 0x00, 0x01]).unwrap();
    let out = sb.setup(&["cursor", "--with", "mcp", "--apply"]);
    assert!(out.stdout.contains("  mcp    manual   ~/.cursor/mcp.json is not UTF-8 text\n"), "{}", out.stdout);
    assert_eq!(std::fs::read(sb.home(".cursor/mcp.json")).unwrap(), [0xff, 0xfe, 0x00, 0x01]);
}

#[cfg(unix)]
#[test]
fn a_linked_settings_file_stays_a_link_and_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new("symlink");
    let real = sb.home("dotfiles/cursor-mcp.json");
    write(&real, "{\n  \"mcpServers\": {}\n}\n");
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::create_dir_all(sb.home(".cursor")).unwrap();
    std::os::unix::fs::symlink(&real, sb.home(".cursor/mcp.json")).unwrap();
    sb.setup(&["cursor", "--with", "mcp", "--apply"]).ok();
    assert!(std::fs::symlink_metadata(sb.home(".cursor/mcp.json")).unwrap().file_type().is_symlink(), "people keep their settings in a dotfiles repository");
    assert_eq!(json(&real)["mcpServers"]["codefast"], cursor_entry());
    assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::read_dir(sb.home("dotfiles")).unwrap().count(), 1, "no temporary file is left behind");
    // A directory that cannot be written is reported, and the run fails.
    let locked = sb.home(".config/zed");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let out = sb.setup(&["zed", "--apply"]);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.code, 1);
    assert!(out.stdout.contains("  mcp    failed   create ~/.config/zed/settings.json\n") && out.stdout.contains("cannot write"), "{}", out.stdout);
    assert!(out.stdout.ends_with("0 changed, 0 already in place, 1 failed.\n"), "{}", out.stdout);
}

#[test]
fn two_harnesses_that_share_a_file_plan_one_change() {
    let sb = Sandbox::new("shared");
    // Claude Code, Copilot CLI and Deep Agents Code all read the project's .mcp.json; Codex,
    // Cursor and Goose all read ~/.agents/skills.
    let plan = sb.setup(&["claude-code", "copilot-cli", "deepagents", "--scope", "project", "--with", "mcp"]);
    assert_eq!(plan.ok().matches("plan     create .mcp.json").count(), 1, "{}", plan.stdout);
    assert_eq!(plan.stdout.matches("ok       .mcp.json already runs `codefast mcp` (see above)").count(), 2, "{}", plan.stdout);
    assert!(plan.stdout.contains("Dry run: nothing was changed. 1 change planned, 2 already in place;"));
    let plan = sb.setup(&["codex", "cursor", "goose", "--with", "skill"]);
    assert_eq!(plan.ok().matches("plan     create ~/.agents/skills/codefast/SKILL.md").count(), 1);
    assert_eq!(plan.stdout.matches("ok       ~/.agents/skills/codefast/SKILL.md is current (see above)").count(), 2);
    let applied = sb.setup(&["codex", "cursor", "goose", "--with", "skill", "--apply"]);
    assert!(applied.ok().ends_with("1 changed, 2 already in place.\n"), "{}", applied.stdout);
}

#[test]
fn hooks_are_registered_on_request_next_to_the_users_own() {
    let sb = Sandbox::new("hooks");
    let settings = sb.home(".claude/settings.json");
    let own = "{\n  \"model\": \"opus\",\n  \"hooks\": {\n    \"PostToolUse\": [\n      {\n        \"matcher\": \"Edit|Write\",\n        \"hooks\": [{\"type\": \"command\", \"command\": \"prettier --write\"}]\n      }\n    ]\n  }\n}\n";
    write(&settings, own);
    // Not unless asked.
    assert!(!sb.setup(&["claude-code", "codex"]).ok().lines().any(|l| l.starts_with("  hooks ")));
    let out = sb.setup(&["claude-code", "codex", "--with", "hooks", "--apply"]);
    assert!(
        out.ok().contains("  hooks  done     edit ~/.claude/settings.json\n") && out.stdout.contains("  hooks  done     create ~/.codex/hooks.json ("),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("Codex runs new hooks only after you review and trust them once"));
    let doc = json(&settings);
    assert_eq!(doc["model"], "opus");
    let handler = |sub: &str, timeout: u64| json!([{"type": "command", "command": hook_command(BIN, sub), "timeout": timeout}]);
    assert_eq!(doc["hooks"]["SessionStart"], json!([{"hooks": handler("session-start", 10)}]));
    assert_eq!(doc["hooks"]["PreToolUse"], json!([{"matcher": "Read|Bash", "hooks": handler("pre-tool-use", 5)}]));
    assert_eq!(doc["hooks"]["PostToolUseFailure"], json!([{"matcher": "Bash|Grep|Glob", "hooks": handler("post-tool-use", 5)}]));
    let post = doc["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(post.len(), 2);
    assert_eq!(
        post[0],
        json!({"matcher": "Edit|Write", "hooks": [{"type": "command", "command": "prettier --write"}]}),
        "the user's own hook, first and unchanged"
    );
    assert_eq!(post[1], json!({"matcher": "Bash|Grep|Glob", "hooks": handler("post-tool-use", 5)}));
    assert!(read(&settings).starts_with("{\n  \"model\": \"opus\",\n  \"hooks\": {\n    \"PostToolUse\": [\n      {\n        \"matcher\": \"Edit|Write\",\n        \"hooks\": [{\"type\": \"command\", \"command\": \"prettier --write\"}]\n      },\n"));
    // Codex: its shell is the only tool to match, and there is no failure event.
    let codex = json(&sb.home(".codex/hooks.json"));
    assert_eq!(codex["hooks"].as_object().unwrap().keys().collect::<Vec<_>>(), ["PostToolUse", "PreToolUse", "SessionStart"]);
    assert_eq!(codex["hooks"]["PostToolUse"], json!([{"matcher": "Bash", "hooks": handler("post-tool-use", 5)}]));

    // Again: nothing. With another binary: the entries are replaced, not doubled.
    let again = sb.setup(&["claude-code", "codex", "--with", "hooks", "--apply"]);
    assert!(again.ok().ends_with("0 changed, 2 already in place.\n"), "{}", again.stdout);
    sb.setup(&["claude-code", "--with", "hooks", "--bin", "/opt/code fast/codefast", "--apply"]).ok();
    let doc = json(&settings);
    assert_eq!(doc["hooks"]["PostToolUse"].as_array().unwrap().len(), 2);
    assert_eq!(
        doc["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        hook_command("'/opt/code fast/codefast'", "session-start"),
        "a path with a space is one shell word"
    );
    // Harnesses whose hooks setup does not write say so.
    let other = sb.setup(&["cursor", "--with", "hooks"]);
    assert!(
        other.ok().contains("  hooks  skip     setup registers hooks for Claude Code and Codex; for Cursor they come with the plugin, if at all\n"),
        "{}",
        other.stdout
    );
    // And the registered command is the one the hook tests exercise.
    let ran = sb.run(&["hook", "post-tool-use"], &[]);
    assert_eq!((ran.code, ran.stdout.as_str(), ran.stderr.as_str()), (0, "", ""));
}

#[test]
fn hooks_that_are_not_ours_survive_setup() {
    let sb = Sandbox::new("hooks-foreign");
    let settings = sb.home(".claude/settings.json");
    // A user who merged the codefast hook into a group of their own, and one whose own hook
    // merely mentions the words: neither group is setup's to rewrite.
    let shared = json!({"matcher": "Bash", "hooks": [
        {"type": "command", "command": "/Users/me/bin/audit-log.sh"},
        {"type": "command", "command": "codefast hook post-tool-use 2>/dev/null || true", "timeout": 5}
    ]});
    let lookalike = json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "cd ~/src/codefast && ./scripts/guard.sh hook pre"}]});
    let own = json!({"hooks": {"PostToolUse": [shared], "PreToolUse": [lookalike]}});
    write(&settings, &format!("{}\n", serde_json::to_string_pretty(&own).unwrap()));

    let out = sb.setup(&["claude-code", "--with", "hooks", "--apply"]);
    assert!(out.ok().contains("  hooks  done     edit ~/.claude/settings.json\n"), "{}", out.stdout);
    assert!(!out.stdout.contains("audit-log.sh") && !out.stdout.contains("guard.sh"), "their lines are not part of the change: {}", out.stdout);
    let doc = json(&settings);
    let handler = |sub: &str, timeout: u64| json!([{"type": "command", "command": hook_command(BIN, sub), "timeout": timeout}]);
    assert_eq!(doc["hooks"]["PostToolUse"], json!([shared]), "the shared group is left exactly as it was, and it already runs the hook");
    assert_eq!(doc["hooks"]["PreToolUse"], json!([lookalike, {"matcher": "Read|Bash", "hooks": handler("pre-tool-use", 5)}]));
    assert_eq!(doc["hooks"]["SessionStart"], json!([{"hooks": handler("session-start", 10)}]));
    assert_eq!(doc["hooks"]["PostToolUseFailure"], json!([{"matcher": "Bash|Grep|Glob", "hooks": handler("post-tool-use", 5)}]));
    // doctor counts the shared group as a registration, and a second run has nothing to add.
    let report: Value = serde_json::from_str(sb.run(&["doctor", "--json"], &[]).ok()).unwrap();
    let claude = report["harnesses"].as_array().unwrap().iter().find(|h| h["id"] == "claude-code").unwrap();
    assert_eq!(claude["user"]["hooks"]["state"], "registered", "{claude}");
    let before = read(&settings);
    assert!(sb.setup(&["claude-code", "--with", "hooks", "--apply"]).ok().ends_with("0 changed, 1 already in place.\n"));
    assert_eq!(read(&settings), before);

    // The plugin's own form, a quoted path and an environment prefix are all recognised as ours
    // and brought up to date in place; nothing is doubled.
    let ours = json!({"hooks": {
        "SessionStart": [{"hooks": [{"type": "command", "command": "\"${CLAUDE_PLUGIN_ROOT}/bin/codefast\" hook session-start"}]}],
        "PreToolUse": [{"matcher": "Read", "hooks": [{"type": "command", "command": "'/old place/codefast' hook pre-tool-use"}]}],
        "PostToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "CODEFAST_HOOK_BUDGET_MS=20 /opt/bin/codefast hook post-tool-use"}]}]
    }});
    write(&settings, &ours.to_string());
    sb.setup(&["claude-code", "--with", "hooks", "--apply"]).ok();
    let doc = json(&settings);
    for (event, sub, timeout) in [("SessionStart", "session-start", 10), ("PreToolUse", "pre-tool-use", 5), ("PostToolUse", "post-tool-use", 5)] {
        let entries = doc["hooks"][event].as_array().unwrap();
        assert_eq!((entries.len(), &entries[0]["hooks"]), (1, &handler(sub, timeout)), "{event}: {doc}");
    }
}

#[cfg(unix)]
#[test]
fn registered_hooks_stay_silent_where_codefast_is_not_installed() {
    // Project-scope hooks are shared through the repository. On a teammate's machine without
    // codefast (and on one's own after an uninstall) the harness still runs every one of them:
    // each must end with status 0 and say nothing, or the harness reports a hook error on
    // every tool call.
    let sb = Sandbox::new("hooks-absent");
    sb.setup(&["claude-code", "codex", "--scope", "project", "--with", "hooks", "--apply"]).ok();
    sb.setup(&["claude-code", "codex", "--with", "hooks", "--bin", "/nonexistent/place/codefast", "--apply"]).ok();
    let mut ran = 0;
    for file in [sb.project(".claude/settings.json"), sb.project(".codex/hooks.json"), sb.home(".claude/settings.json"), sb.home(".codex/hooks.json")] {
        let doc = json(&file);
        for (event, groups) in doc["hooks"].as_object().unwrap() {
            let command = groups[0]["hooks"][0]["command"].as_str().unwrap();
            // As the harnesses run a command hook: through `sh -c`, payload on stdin.
            let mut child = std::process::Command::new("/bin/sh")
                .args(["-c", command])
                .env_clear()
                .env("HOME", &sb.home)
                .env("PATH", "/usr/bin:/bin")
                .current_dir(&sb.project)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            {
                use std::io::Write;
                let _ = child.stdin.take().unwrap().write_all(b"{\"hook_event_name\": \"PreToolUse\"}");
            }
            let out = child.wait_with_output().unwrap();
            assert_eq!(
                (out.status.code(), String::from_utf8_lossy(&out.stdout).as_ref(), String::from_utf8_lossy(&out.stderr).as_ref()),
                (Some(0), "", ""),
                "{}: {event}: `{command}`",
                file.display()
            );
            ran += 1;
        }
    }
    assert_eq!(ran, 4 + 3 + 4 + 3);
    // With the program there, the same commands are the hooks: the guard changes nothing.
    let command = json(&sb.project(".codex/hooks.json"))["hooks"]["SessionStart"][0]["hooks"][0]["command"].as_str().unwrap().to_string();
    let bin_dir = std::path::Path::new(BIN).parent().unwrap();
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", &command])
        .env_clear()
        .env("HOME", &sb.home)
        .env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display()))
        .env("CODEFAST_NO_REFRESH", "1")
        .current_dir(&sb.project)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stderr).as_ref()), (Some(0), ""));
}

#[test]
fn the_rules_block_is_added_updated_in_place_and_otherwise_left_alone() {
    // No instruction file yet: AGENTS.md is created with the block and nothing else.
    let sb = Sandbox::new("rules");
    let out = sb.setup(&["agents-md", "--apply"]);
    assert_eq!(
        out.ok().lines().take(2).collect::<Vec<_>>(),
        ["Project instructions (agents-md)", "  rules  done     create AGENTS.md (add the codefast block)"],
        "{}",
        out.stdout
    );
    assert!(out.stdout.ends_with("1 changed, 0 already in place.\n"));
    let block = read(&sb.project("AGENTS.md"));
    assert!(block.starts_with("<!-- codefast:begin -->\n## Code navigation: codefast\n") && block.ends_with("<!-- codefast:end -->\n"), "{block}");
    // A few hundred tokens at most, said calmly, and every command in it exists.
    assert!(block.len() < 1300, "{} bytes", block.len());
    let said = block.replace("<!--", "");
    for shout in ["MUST", "ALWAYS", "NEVER", "IMPORTANT", "CRITICAL", "!"] {
        assert!(!said.contains(shout), "{shout}");
    }
    let commands: Vec<&str> = block.split("`codefast ").skip(1).filter_map(|rest| rest.split([' ', '`']).next()).collect();
    assert_eq!(commands, ["search", "show", "show", "callers", "callees", "refs", "path", "grep", "locate"]);
    for command in &commands {
        assert_eq!(sb.run(&[command, "--help"], &[]).code, 0, "codefast {command}");
    }
    // Asked again: nothing to do, nothing rewritten.
    let settled = sb.snapshot();
    assert!(sb.setup(&["agents-md", "--apply"]).ok().ends_with("0 changed, 1 already in place.\n"));
    assert!(sb.setup(&["agents-md"]).ok().ends_with("Nothing to change: 1 already in place.\n"));
    assert_eq!(sb.snapshot(), settled);

    // Files that exist keep their text; the block goes after it. A file that imports AGENTS.md
    // already gets the block through it.
    let sb = Sandbox::new("rules");
    write(&sb.project("AGENTS.md"), "# Rules\n\nUse tabs.\n");
    write(&sb.project("CLAUDE.md"), "# Notes for Claude");
    write(&sb.project("GEMINI.md"), "@AGENTS.md\n\nBe brief.\n");
    let out = sb.setup(&["agents-md", "--apply"]);
    assert!(
        out.ok().contains("  rules  done     edit AGENTS.md (add the codefast block)\n")
            && out.stdout.contains("  rules  done     edit CLAUDE.md (add the codefast block)\n"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("  rules  ok       GEMINI.md imports AGENTS.md\n") && out.stdout.ends_with("2 changed, 1 already in place.\n"),
        "{}",
        out.stdout
    );
    assert_eq!(read(&sb.project("AGENTS.md")), format!("# Rules\n\nUse tabs.\n\n{block}"));
    assert_eq!(read(&sb.project("CLAUDE.md")), format!("# Notes for Claude\n\n{block}"));
    assert_eq!(read(&sb.project("GEMINI.md")), "@AGENTS.md\n\nBe brief.\n");

    // A block from an older release, or one somebody edited, is replaced where it stands.
    let older = block.replace("Plain grep is still the right tool", "Use grep");
    assert_ne!(older, block);
    write(&sb.project("AGENTS.md"), &format!("# Rules\n\n{older}\n## Style\n\nUse tabs.\n"));
    let out = sb.setup(&["agents-md", "--apply"]);
    assert!(out.ok().contains("  rules  done     edit AGENTS.md (update the codefast block)\n"), "{}", out.stdout);
    assert!(
        out.stdout.contains("                  - Answers give") && out.stdout.contains("                  + Answers give"),
        "the diff shows the line that changed: {}",
        out.stdout
    );
    assert_eq!(read(&sb.project("AGENTS.md")), format!("# Rules\n\n{block}\n## Style\n\nUse tabs.\n"));

    // Markers that no longer pair up are for a person to repair.
    let damaged = "# Rules\n\n<!-- codefast:begin -->\nhalf a block\n";
    write(&sb.project("AGENTS.md"), damaged);
    let out = sb.setup(&["agents-md", "--apply"]);
    assert_eq!(out.code, 0);
    assert!(out.stdout.contains("  rules  manual   AGENTS.md: the file has unpaired or repeated codefast markers; fix them by hand first\n"), "{}", out.stdout);
    assert_eq!(read(&sb.project("AGENTS.md")), damaged);

    // --file names the files to use instead.
    let sb = Sandbox::new("rules");
    let out = sb.setup(&["agents-md", "--file", "docs/AI.md", "--file=CLAUDE.md", "--apply"]);
    assert!(out.ok().ends_with("2 changed, 0 already in place.\n"), "{}", out.stdout);
    assert_eq!((read(&sb.project("docs/AI.md")), read(&sb.project("CLAUDE.md"))), (block.clone(), block.clone()));
    assert!(!sb.project("AGENTS.md").exists());
    assert_eq!(std::fs::read_dir(&sb.home).unwrap().count(), 0, "project instructions never touch the home directory");
}
