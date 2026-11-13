//! Where and how `codefast setup` registers the MCP server and the skill for each harness.
//!
//! One row per harness and scope: the file, the place inside it, and the entry, each as that
//! harness documents it (`docs/research/harness-plugin-formats.md` sections 2 and 5, re-read
//! against the vendors' pages on 2026-11-13). No harness program is on `PATH` in these tests,
//! so this is the "write the documented file" route; `setup_native.rs` covers the commands.

#[path = "setup_support/mod.rs"]
mod sandbox;

use sandbox::*;
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Clone, Copy)]
enum At {
    Home(&'static str),
    Project(&'static str),
}

impl At {
    fn path(self, sb: &Sandbox) -> PathBuf {
        match self {
            At::Home(rel) => sb.home(rel),
            At::Project(rel) => sb.project(rel),
        }
    }
}

fn plain(command: &str) -> Value {
    json!({"command": command, "args": ["mcp"]})
}

fn stdio(command: &str) -> Value {
    json!({"type": "stdio", "command": command, "args": ["mcp"]})
}

fn local(command: &str) -> Value {
    json!({"type": "local", "command": [command, "mcp"], "enabled": true})
}

/// (harness, scope, file, JSON pointer of the entry, the entry for a given command)
type Row = (&'static str, &'static str, At, &'static str, fn(&str) -> Value);

const SERVERS: &str = "/mcpServers/codefast";

fn rows() -> Vec<Row> {
    vec![
        ("claude-code", "user", At::Home(".claude.json"), SERVERS, |c| json!({"type": "stdio", "command": c, "args": ["mcp"], "env": {}})),
        ("claude-code", "project", At::Project(".mcp.json"), SERVERS, |c| json!({"type": "stdio", "command": c, "args": ["mcp"], "env": {}})),
        ("gemini-cli", "user", At::Home(".gemini/settings.json"), SERVERS, plain),
        ("gemini-cli", "project", At::Project(".gemini/settings.json"), SERVERS, plain),
        ("qwen-code", "user", At::Home(".qwen/settings.json"), SERVERS, plain),
        ("qwen-code", "project", At::Project(".qwen/settings.json"), SERVERS, plain),
        ("cursor", "user", At::Home(".cursor/mcp.json"), SERVERS, stdio),
        ("cursor", "project", At::Project(".cursor/mcp.json"), SERVERS, stdio),
        ("copilot", "project", At::Project(".vscode/mcp.json"), "/servers/codefast", stdio),
        ("copilot-cli", "user", At::Home(".copilot/mcp-config.json"), SERVERS, |c| json!({"type": "local", "command": c, "args": ["mcp"], "tools": ["*"]})),
        ("copilot-cli", "project", At::Project(".mcp.json"), SERVERS, stdio),
        ("opencode", "user", At::Home(".config/opencode/opencode.json"), "/mcp/codefast", local),
        ("opencode", "project", At::Project("opencode.json"), "/mcp/codefast", local),
        ("kilo-code", "user", At::Home(".config/kilo/kilo.jsonc"), "/mcp/codefast", local),
        ("kilo-code", "project", At::Project(".kilo/kilo.jsonc"), "/mcp/codefast", local),
        ("amp", "user", At::Home(".config/amp/settings.json"), "/amp.mcpServers/codefast", plain),
        ("amp", "project", At::Project(".amp/settings.json"), "/amp.mcpServers/codefast", plain),
        (
            "cline",
            "user",
            At::Home(".cline/data/settings/cline_mcp_settings.json"),
            SERVERS,
            |c| json!({"command": c, "args": ["mcp"], "disabled": false, "autoApprove": []}),
        ),
        ("droid", "user", At::Home(".factory/mcp.json"), SERVERS, |c| json!({"type": "stdio", "command": c, "args": ["mcp"], "disabled": false})),
        ("droid", "project", At::Project(".factory/mcp.json"), SERVERS, |c| json!({"type": "stdio", "command": c, "args": ["mcp"], "disabled": false})),
        ("zed", "user", At::Home(".config/zed/settings.json"), "/context_servers/codefast", |c| json!({"command": c, "args": ["mcp"], "env": {}})),
        ("zed", "project", At::Project(".zed/settings.json"), "/context_servers/codefast", |c| json!({"command": c, "args": ["mcp"], "env": {}})),
        ("kiro", "user", At::Home(".kiro/settings/mcp.json"), SERVERS, |c| json!({"command": c, "args": ["mcp"], "disabled": false})),
        ("kiro", "project", At::Project(".kiro/settings/mcp.json"), SERVERS, |c| json!({"command": c, "args": ["mcp"], "disabled": false})),
        ("junie", "user", At::Home(".junie/mcp/mcp.json"), SERVERS, plain),
        ("junie", "project", At::Project(".junie/mcp/mcp.json"), SERVERS, plain),
        ("deepagents", "user", At::Home(".deepagents/.mcp.json"), SERVERS, plain),
        ("deepagents", "project", At::Project(".mcp.json"), SERVERS, stdio),
    ]
}

/// Regular files in the sandbox's home and project, relative to the sandbox.
fn files(sb: &Sandbox) -> Vec<String> {
    sb.snapshot()
        .into_iter()
        .filter(|(_, size, _)| *size > 0)
        .map(|(path, _, _)| path)
        .filter(|p| p.starts_with("home/") || p.starts_with("project/"))
        .collect()
}

#[test]
fn every_json_harness_gets_its_documented_file_place_and_entry() {
    for (id, scope, at, pointer, entry) in rows() {
        let what = format!("{id} at {scope} scope");
        let sb = Sandbox::new("row");
        let command = if scope == "user" { BIN } else { "codefast" };
        let out = sb.setup(&[id, "--scope", scope, "--with", "mcp", "--apply"]);
        assert!(out.ok().ends_with("1 changed, 0 already in place.\n"), "{what}: {}", out.stdout);
        let file = at.path(&sb);
        let doc = json(&file);
        assert_eq!(doc.pointer(pointer), Some(&entry(command)), "{what}: {doc}");
        // The entry and the objects that hold it are all there is in the new file...
        let depth = pointer.matches('/').count();
        let mut level = &doc;
        for _ in 0..depth {
            let object = level.as_object().unwrap();
            assert_eq!(object.len(), 1, "{what}: {doc}");
            level = object.values().next().unwrap();
        }
        // ...and that file is all that was written.
        let rel = file.strip_prefix(sb.home.parent().unwrap()).unwrap().display().to_string();
        assert_eq!(files(&sb), [rel], "{what}");
        // Idempotent, and the dry run agrees.
        assert!(sb.setup(&[id, "--scope", scope, "--with", "mcp", "--apply"]).ok().ends_with("0 changed, 1 already in place.\n"), "{what}");
        assert!(sb.setup(&[id, "--scope", scope, "--with", "mcp"]).ok().ends_with("Nothing to change: 1 already in place.\n"), "{what}");
        // doctor reads the same place back.
        if scope == "user" {
            let report: Value = serde_json::from_str(sb.run(&["doctor", "--json"], &[]).ok()).unwrap();
            let seen =
                report["harnesses"].as_array().unwrap().iter().find(|h| h["id"] == id).unwrap_or_else(|| panic!("{what}: doctor does not list it: {report}"));
            assert_eq!(
                (&seen["registered"], &seen["user"]["mcp"]["state"], &seen["user"]["mcp"]["command_found"]),
                (&json!(true), &json!("registered"), &json!(true)),
                "{what}: {seen}"
            );
            assert_eq!(seen["user"]["mcp"]["file"], file.to_str().unwrap(), "{what}");
        }
    }
}

#[test]
fn the_formats_that_are_not_json() {
    let sb = Sandbox::new("text");
    let out = sb.setup(&["codex", "goose", "crush", "--with", "mcp", "--apply"]);
    assert!(out.ok().ends_with("3 changed, 0 already in place.\n"), "{}", out.stdout);
    assert_eq!(read(&sb.home(".codex/config.toml")), format!("[mcp_servers.codefast]\ncommand = \"{BIN}\"\nargs = [\"mcp\"]\n"));
    assert_eq!(
        read(&sb.home(".config/goose/config.yaml")),
        format!(
            "extensions:\n  codefast:\n    name: codefast\n    type: stdio\n    cmd: {BIN}\n    args: [mcp]\n    enabled: true\n    envs: {{}}\n    timeout: 300\n    description: \"Code search, references and call graphs from a prebuilt index\"\n"
        )
    );
    assert_eq!(read(&sb.home(".config/crush/crushrc")), format!("mcp add codefast --command {BIN} --args mcp\n"));
    assert!(sb.setup(&["codex", "goose", "crush", "--with", "mcp", "--apply"]).ok().ends_with("0 changed, 3 already in place.\n"));

    // Existing content stays; the entry lands where the format wants it.
    write(&sb.home(".codex/config.toml"), "model = \"gpt-5.5\"\n\n[features]\nhooks = true\n");
    write(
        &sb.home(".config/goose/config.yaml"),
        "GOOSE_PROVIDER: anthropic\nextensions:\n  developer:\n    bundled: true\n    enabled: true\n    type: builtin\n",
    );
    write(&sb.home(".config/crush/crushrc"), "# crush\nlsp add go --command gopls\n");
    sb.setup(&["codex", "goose", "crush", "--with", "mcp", "--apply"]).ok();
    assert_eq!(
        read(&sb.home(".codex/config.toml")),
        format!("model = \"gpt-5.5\"\n\n[features]\nhooks = true\n\n[mcp_servers.codefast]\ncommand = \"{BIN}\"\nargs = [\"mcp\"]\n")
    );
    let goose = read(&sb.home(".config/goose/config.yaml"));
    assert!(
        goose.starts_with("GOOSE_PROVIDER: anthropic\nextensions:\n  codefast:\n    name: codefast\n")
            && goose.ends_with("  developer:\n    bundled: true\n    enabled: true\n    type: builtin\n"),
        "{goose}"
    );
    assert_eq!(read(&sb.home(".config/crush/crushrc")), format!("# crush\nlsp add go --command gopls\nmcp add codefast --command {BIN} --args mcp\n"));

    // Project scope, where the harness has one.
    let out = sb.setup(&["codex", "crush", "goose", "cline", "--scope", "project", "--with", "mcp", "--apply"]);
    assert!(out.ok().ends_with("2 changed, 0 already in place.\n"), "{}", out.stdout);
    assert_eq!(read(&sb.project(".codex/config.toml")), "[mcp_servers.codefast]\ncommand = \"codefast\"\nargs = [\"mcp\"]\n");
    assert_eq!(read(&sb.project(".crushrc")), "mcp add codefast --command codefast --args mcp\n");
    assert!(out.stdout.contains("  mcp    skip     Goose keeps extensions in the user's config.yaml only; use --scope user\n"), "{}", out.stdout);
    assert!(out.stdout.contains("  mcp    skip     Cline keeps MCP servers in the user's cline_mcp_settings.json only; use --scope user\n"));
}

#[test]
fn an_existing_file_decides_the_variant() {
    // OpenCode: a .jsonc file is edited as it is; a file in the V2 layout keeps that layout.
    let sb = Sandbox::new("variants");
    write(
        &sb.home(".config/opencode/opencode.jsonc"),
        "{\n  // my models\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"mcp\": {\n    \"servers\": {\n      \"context7\": {\"type\": \"remote\", \"url\": \"https://mcp.context7.com/mcp\"},\n    },\n  },\n}\n",
    );
    sb.setup(&["opencode", "--with", "mcp", "--apply"]).ok();
    assert!(!sb.home(".config/opencode/opencode.json").exists());
    let text = read(&sb.home(".config/opencode/opencode.jsonc"));
    assert!(text.starts_with("{\n  // my models\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"mcp\": {\n    \"servers\": {\n      \"context7\": {\"type\": \"remote\", \"url\": \"https://mcp.context7.com/mcp\"},\n      \"codefast\": {\n        \"type\": \"local\",\n        \"command\": ["), "{text}");
    assert!(text.ends_with("      }\n    },\n  },\n}\n") && !text.contains("\"enabled\""), "V2 entries have no `enabled`: {text}");
    // doctor finds it under the same layout.
    let report: Value = serde_json::from_str(sb.run(&["doctor", "--json"], &[]).ok()).unwrap();
    let opencode = report["harnesses"].as_array().unwrap().iter().find(|h| h["id"] == "opencode").unwrap();
    assert_eq!((&opencode["user"]["mcp"]["state"], &opencode["user"]["mcp"]["command"]), (&json!("registered"), &json!(format!("{BIN} mcp"))), "{opencode}");
    assert!(sb.setup(&["opencode", "--with", "mcp", "--apply"]).ok().ends_with("0 changed, 1 already in place.\n"));
    // Kilo Code: a kilo.json at the project root is where the project's servers go.
    write(&sb.project("kilo.json"), "{\n  \"mcp\": {}\n}\n");
    sb.setup(&["kilo-code", "--scope", "project", "--with", "mcp", "--apply"]).ok();
    assert!(!sb.project(".kilo").exists());
    assert_eq!(json(&sb.project("kilo.json")), json!({"mcp": {"codefast": {"type": "local", "command": ["codefast", "mcp"], "enabled": true}}}));
    // Crush: its older JSON file is used only where there is no crushrc.
    write(&sb.home(".config/crush/crush.json"), "{\n  \"lsp\": {}\n}\n");
    sb.setup(&["crush", "--with", "mcp", "--apply"]).ok();
    assert!(!sb.home(".config/crush/crushrc").exists());
    assert_eq!(json(&sb.home(".config/crush/crush.json"))["mcp"]["codefast"], stdio(BIN));
    write(&sb.project("crushrc"), "# project crush\n");
    sb.setup(&["crush", "--scope", "project", "--with", "mcp", "--apply"]).ok();
    assert_eq!(read(&sb.project("crushrc")), "# project crush\nmcp add codefast --command codefast --args mcp\n");
    assert!(!sb.project(".crushrc").exists());
}

#[test]
fn each_harness_configuration_variable_moves_its_files() {
    let sb = Sandbox::new("env");
    let other = sb.home.parent().unwrap().join("elsewhere");
    let o = |rel: &str| other.join(rel);
    let s = |rel: &str| other.join(rel).display().to_string();
    let cases: Vec<(&str, &str, String, PathBuf)> = vec![
        ("claude-code", "CLAUDE_CONFIG_DIR", s("claude"), o("claude/.claude.json")),
        ("codex", "CODEX_HOME", s("codex"), o("codex/config.toml")),
        ("gemini-cli", "GEMINI_CLI_HOME", s("gemini-home"), o("gemini-home/.gemini/settings.json")),
        ("copilot-cli", "COPILOT_HOME", s("copilot"), o("copilot/mcp-config.json")),
        ("cline", "CLINE_MCP_SETTINGS_PATH", s("cline/servers.json"), o("cline/servers.json")),
        ("crush", "CRUSH_GLOBAL_CONFIG", s("crush/myrc"), o("crush/myrc")),
        ("opencode", "XDG_CONFIG_HOME", s("xdg"), o("xdg/opencode/opencode.json")),
        ("kilo-code", "XDG_CONFIG_HOME", s("xdg"), o("xdg/kilo/kilo.jsonc")),
        ("goose", "XDG_CONFIG_HOME", s("xdg"), o("xdg/goose/config.yaml")),
        ("amp", "XDG_CONFIG_HOME", s("xdg"), o("xdg/amp/settings.json")),
    ];
    for (id, var, value, file) in &cases {
        let out = sb.setup_env(&[id, "--with", "mcp", "--apply"], &[(var, value)]);
        assert!(out.ok().ends_with("1 changed, 0 already in place.\n"), "{id} with {var}: {}", out.stdout);
        assert!(read(file).contains(BIN), "{id}: {}", file.display());
    }
    assert_eq!(std::fs::read_dir(&sb.home).unwrap().count(), 0, "with the variables set, the default places stay empty");
    // The skill and the hooks follow the same variables.
    let out = sb.setup_env(&["claude-code", "codex", "--with", "skill,hooks", "--apply"], &[("CLAUDE_CONFIG_DIR", &s("claude")), ("CODEX_HOME", &s("codex"))]);
    out.ok();
    assert!(o("claude/skills/codefast/SKILL.md").exists() && o("claude/settings.json").exists() && o("codex/hooks.json").exists());
    assert!(sb.home(".agents/skills/codefast/SKILL.md").exists(), "Codex reads skills from ~/.agents/skills, which CODEX_HOME does not move");
    // A placeholder a harness left unexpanded is not a directory name.
    let out = sb.setup_env(&["claude-code", "--with", "mcp"], &[("CLAUDE_CONFIG_DIR", "${CLAUDE_CONFIG_DIR}")]);
    assert!(out.ok().contains("create ~/.claude.json"), "{}", out.stdout);
}

#[test]
fn zed_settings_are_where_zed_reads_them_on_this_platform() {
    // Zed (crates/paths/src/paths.rs) follows XDG_CONFIG_HOME on Linux and FreeBSD only; on
    // macOS its settings are in ~/.config/zed whatever that variable says.
    let sb = Sandbox::new("zed");
    let xdg = sb.home.parent().unwrap().join("xdg");
    let out = sb.setup_env(&["zed", "--with", "mcp", "--apply"], &[("XDG_CONFIG_HOME", xdg.to_str().unwrap())]);
    assert!(out.ok().ends_with("1 changed, 0 already in place.\n"), "{}", out.stdout);
    let (read_by_zed, not_read) = if cfg!(target_os = "macos") {
        (sb.home(".config/zed/settings.json"), xdg.join("zed/settings.json"))
    } else {
        (xdg.join("zed/settings.json"), sb.home(".config/zed/settings.json"))
    };
    assert!(read(&read_by_zed).contains(BIN), "{}", read_by_zed.display());
    assert!(!not_read.exists(), "{}", not_read.display());
    // doctor looks in the same place.
    let report: Value = serde_json::from_str(sb.run(&["doctor", "--json"], &[("XDG_CONFIG_HOME", xdg.to_str().unwrap())]).ok()).unwrap();
    let zed = report["harnesses"].as_array().unwrap().iter().find(|h| h["id"] == "zed").expect("doctor lists zed");
    assert_eq!((&zed["user"]["mcp"]["state"], zed["user"]["mcp"]["file"].as_str()), (&json!("registered"), read_by_zed.to_str()), "{zed}");
}

#[test]
fn the_skill_goes_where_each_harness_looks_for_skills() {
    let shared = [
        "codex",
        "gemini-cli",
        "cursor",
        "copilot",
        "copilot-cli",
        "opencode",
        "goose",
        "amp",
        "cline",
        "kilo-code",
        "crush",
        "deepagents",
        "pi",
        "devin",
        "antigravity",
        "dsh",
    ];
    let own = [
        ("claude-code", ".claude/skills", ".claude/skills"),
        ("qwen-code", ".qwen/skills", ".qwen/skills"),
        ("junie", ".junie/skills", ".junie/skills"),
        ("droid", ".factory/skills", ".agents/skills"),
    ];
    let skill = include_str!("../skills/codefast/SKILL.md");
    for id in shared {
        let sb = Sandbox::new("skill");
        sb.setup(&[id, "--with", "skill", "--apply"]).ok();
        assert_eq!(read(&sb.home(".agents/skills/codefast/SKILL.md")), skill, "{id}");
        sb.setup(&[id, "--with", "skill", "--scope", "project", "--apply"]).ok();
        assert_eq!(read(&sb.project(".agents/skills/codefast/SKILL.md")), skill, "{id}");
    }
    for (id, user, project) in own {
        let sb = Sandbox::new("skill");
        sb.setup(&[id, "--with", "skill", "--apply"]).ok();
        sb.setup(&[id, "--with", "skill", "--scope", "project", "--apply"]).ok();
        assert_eq!(read(&sb.home(&format!("{user}/codefast/SKILL.md"))), skill, "{id}");
        assert_eq!(read(&sb.project(&format!("{project}/codefast/SKILL.md"))), skill, "{id}");
        assert!(!sb.home(".agents").exists(), "{id} does not read ~/.agents/skills");
    }
    // Harnesses without skills of their own get no skill step at all.
    for id in ["zed", "kiro"] {
        let sb = Sandbox::new("skill");
        assert!(!sb.setup(&[id]).ok().contains("  skill "), "{id}");
    }
    // The skill is the open format: the frontmatter keys every client accepts, and the name of its folder.
    let front: Vec<&str> = skill.lines().skip(1).take_while(|l| *l != "---").collect();
    assert!(front.contains(&"name: codefast") && front.iter().any(|l| l.starts_with("description: ")), "{front:?}");
}

#[test]
fn harnesses_without_a_file_to_write_get_exact_instructions() {
    let sb = Sandbox::new("manual");
    let out = sb.setup(&["dsh", "devin", "antigravity", "pi", "--apply"]);
    let text = out.ok();
    assert!(
        text.contains(&format!(
            "  mcp    manual   add an MCP row in Settings -> MCP, or to the profile's cordis.patch.yml: {{id: mcp-codefast, name: '@deepseek-ai/dsh-mcp-client', serverName: codefast, transport: stdio, command: {BIN}, args: [mcp]}}\n"
        )),
        "{text}"
    );
    assert!(text.contains("  mcp    manual   add this server to ~/.config/devin/mcp_config.json (stdio; the file's schema is not published"), "{text}");
    assert!(
        text.contains("  mcp    manual   Antigravity takes MCP servers from plugins: agy plugin install https://github.com/YashSharmaa/codefast\n"),
        "{text}"
    );
    assert!(text.contains("  mcp    skip     Pi has no MCP client: the skill, which runs the codefast command, is the integration\n"), "{text}");
    assert!(text.contains("  note            as a package: pi install git:github.com/YashSharmaa/codefast\n"), "{text}");
    // What can be done is done: the skill all four read.
    assert!(text.ends_with("1 changed, 3 already in place, 3 left to do by hand.\n"), "{text}");
    assert!(sb.home(".agents/skills/codefast/SKILL.md").exists());
    // VS Code's user profile is VS Code's to write: without `code` on PATH the command is printed.
    let out = sb.setup(&["copilot", "--with", "mcp", "--apply"]);
    assert!(
        out.ok().contains("  mcp    manual   `code` is not on PATH; run it yourself:\n                  code --add-mcp '{\"args\":[\"mcp\"],\"command\":\""),
        "{}",
        out.stdout
    );
    assert!(!sb.home("Library").exists() && !sb.home(".config/Code").exists());
}

#[test]
fn aider_gets_the_conventions_file() {
    let sb = Sandbox::new("aider");
    write(&sb.project(".aider.conf.yml"), "model: sonnet\nread:\n  - CONVENTIONS.md\nauto-commits: false\n");
    let out = sb.setup(&["aider", "--apply"]);
    assert!(out.ok().starts_with("Aider (aider), this project\n  rules  done     create AGENTS.md (add the codefast block)\n"), "{}", out.stdout);
    assert!(
        out.stdout.contains("  rules  done     edit .aider.conf.yml (read AGENTS.md in every session)\n")
            && out.stdout.ends_with("2 changed, 0 already in place.\n"),
        "{}",
        out.stdout
    );
    assert_eq!(read(&sb.project(".aider.conf.yml")), "model: sonnet\nread:\n  - CONVENTIONS.md\n  - AGENTS.md\nauto-commits: false\n");
    let agents = read(&sb.project("AGENTS.md"));
    assert!(agents.starts_with("<!-- codefast:begin -->\n## Code navigation: codefast\n") && agents.ends_with("<!-- codefast:end -->\n"));
    assert!(sb.setup(&["aider", "--apply"]).ok().ends_with("0 changed, 2 already in place.\n"));
    assert_eq!(std::fs::read_dir(&sb.home).unwrap().count(), 0);
}
