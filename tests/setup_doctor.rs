//! `codefast doctor`: the binary, the index of the repository, and each harness found on the
//! machine. It reports, and it never writes: not a configuration file, not the index.

#[path = "setup_support/mod.rs"]
mod sandbox;

use sandbox::*;
use serde_json::{Value, json};

fn report(sb: &Sandbox, envs: &[(&str, &str)]) -> Value {
    let out = sb.run(&["doctor", "--json"], envs);
    serde_json::from_str(out.ok()).unwrap_or_else(|e| panic!("doctor --json is not JSON ({e}): {}", out.stdout))
}

fn text(sb: &Sandbox, envs: &[(&str, &str)]) -> String {
    sb.run(&["doctor"], envs).ok().to_string()
}

fn harness<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["harnesses"].as_array().unwrap().iter().find(|h| h["id"] == id).unwrap_or_else(|| panic!("{id} is not among the harnesses found: {report}"))
}

/// A few C files to index.
fn sources(sb: &Sandbox) {
    write(&sb.project("src/io.c"), "int dev_read(int fd)\n{\n    return fd;\n}\n\nint dev_write(int fd)\n{\n    return dev_read(fd) + 1;\n}\n");
    write(&sb.project("src/main.c"), "int dev_write(int fd);\n\nint main(void)\n{\n    return dev_write(0);\n}\n");
    write(&sb.project("README.md"), "# project\n");
}

#[test]
fn doctor_creates_and_changes_nothing() {
    let sb = Sandbox::new("readonly");
    sources(&sb);
    // No index, nothing registered: nothing appears, least of all an index.
    let before = sb.snapshot();
    let plain = text(&sb, &[]);
    report(&sb, &[]);
    assert_eq!(sb.snapshot(), before);
    assert!(!sb.project(".codefast").exists());
    assert!(plain.contains("  state     no index yet: it is built on first use, or by `codefast index`\n"), "{plain}");

    // With an index that is behind the working tree, registrations, hooks and a rules block.
    sb.run(&["index"], &[]).ok();
    sb.setup(&["cursor", "codex", "claude-code", "agents-md", "--hooks", "--apply"]).ok();
    write(&sb.project("src/io.c"), "int dev_read(int fd)\n{\n    return fd + 2;\n}\n");
    write(&sb.project("src/new.c"), "int fresh(void)\n{\n    return 1;\n}\n");
    let before = sb.snapshot();
    let behind = text(&sb, &[]);
    let json = report(&sb, &[]);
    assert_eq!(sb.snapshot(), before, "no overlay, no session state, no lock, no rewritten configuration");
    assert!(behind.contains("behind the working tree") && json["index"]["fresh"] == false, "{behind}");
    let listed: Vec<String> = std::fs::read_dir(sb.project(".codefast")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(listed.iter().all(|f| f == "base.cfseg" || f == ".gitignore"), "{listed:?}");
}

#[test]
fn the_index_section_follows_the_working_tree() {
    let sb = Sandbox::new("index");
    sources(&sb);
    let none = report(&sb, &[]);
    assert_eq!(none["index"], json!({"root": sb.project.to_str().unwrap(), "dir": sb.project(".codefast").to_str().unwrap(), "exists": false}));

    sb.run(&["index"], &[]).ok();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let ix = report(&sb, &[])["index"].clone();
    assert_eq!(
        (&ix["exists"], &ix["fresh"], &ix["files"], &ix["built_by"]),
        (&json!(true), &json!(true), &json!(3), &json!(env!("CARGO_PKG_VERSION"))),
        "{ix}"
    );
    assert_eq!((&ix["changed_files"], &ix["new_files"], &ix["deleted_files"]), (&json!(0), &json!(0), &json!(0)));
    assert!(ix["symbols"].as_u64().unwrap() >= 3 && ix["references"].as_u64().unwrap() >= 2 && ix["bytes"].as_u64().unwrap() > 0, "{ix}");
    let built = ix["built_unix"].as_u64().unwrap();
    assert!(built <= now + 1 && now - built.min(now) < 120, "built {built}, now {now}");
    let stamp = ix["built_at"].as_str().unwrap();
    assert!(stamp.len() == 23 && stamp.ends_with(" UTC") && stamp.as_bytes()[4] == b'-' && stamp.as_bytes()[13] == b':', "{stamp}");
    let said = text(&sb, &[]);
    for line in [
        format!("index of {}\n", sb.project.display()),
        "  location  .codefast (".to_string(),
        format!("  built     {stamp} (just now) by codefast {}\n", env!("CARGO_PKG_VERSION")),
        "  contents  3 files, ".to_string(),
        "  state     current\n".to_string(),
    ] {
        assert!(said.contains(&line), "{line:?} in {said}");
    }

    // One file edited, one added, one removed.
    write(&sb.project("src/io.c"), "int dev_read(int fd)\n{\n    return fd + 40;\n}\n");
    write(&sb.project("src/extra.c"), "int extra(void)\n{\n    return 4;\n}\n");
    std::fs::remove_file(sb.project("README.md")).unwrap();
    let ix = report(&sb, &[])["index"].clone();
    assert_eq!((&ix["fresh"], &ix["changed_files"], &ix["new_files"], &ix["deleted_files"]), (&json!(false), &json!(1), &json!(1), &json!(1)), "{ix}");
    assert!(text(&sb, &[]).contains("  state     behind the working tree: 1 changed, 1 new, 1 deleted files; the next codefast command folds them in\n"));
    // Which is true: any query brings the index up to date, and doctor then says so.
    sb.run(&["find", "extra"], &[]).ok();
    let ix = report(&sb, &[])["index"].clone();
    assert_eq!(ix["fresh"], true, "{ix}");

    // An index kept elsewhere is found through the same variable the other commands use.
    let other = sb.home("indexes/project");
    let moved = [("CODEFAST_INDEX_DIR", other.to_str().unwrap())];
    assert_eq!(report(&sb, &moved)["index"]["exists"], false);
    sb.run(&["index"], &moved).ok();
    let ix = report(&sb, &moved)["index"].clone();
    assert_eq!((&ix["exists"], &ix["dir"], &ix["fresh"]), (&json!(true), &json!(other.to_str().unwrap()), &json!(true)), "{ix}");
    // A file that is not an index is reported, not replaced.
    write(&other.join("base.cfseg"), "not an index");
    let broken = report(&sb, &moved)["index"].clone();
    assert_eq!(broken["exists"], false);
    assert!(broken["error"].as_str().unwrap().contains("cannot be read by this version"), "{broken}");
    assert_eq!(read(&other.join("base.cfseg")), "not an index");
}

#[cfg(unix)]
#[test]
fn the_binary_section_says_whether_agents_can_run_it() {
    let sb = Sandbox::new("binary");
    let r = report(&sb, &[]);
    assert_eq!(
        r["binary"],
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "path": BIN,
            "on_path": null,
            "on_path_is_this_binary": false,
            "on_path_via": null,
            "hooks_enabled": true
        })
    );
    let said = text(&sb, &[]);
    assert!(
        said.starts_with(&format!(
            "codefast {}\n  binary    {BIN}\n  on PATH   no: agents run `codefast` by name, so put its directory on PATH\n",
            env!("CARGO_PKG_VERSION")
        )),
        "{said}"
    );

    // Installed the usual way: a link on PATH to wherever the binary really is.
    let link = sb.bin.join("codefast");
    std::os::unix::fs::symlink(BIN, &link).unwrap();
    let r = report(&sb, &[]);
    assert_eq!((&r["binary"]["on_path"], &r["binary"]["on_path_is_this_binary"]), (&json!(link.to_str().unwrap()), &json!(true)));
    assert!(text(&sb, &[]).contains("  on PATH   yes\n"));
    // That link is then the command setup registers for the user: it survives an upgrade.
    let plan = sb.setup(&["cursor", "--with", "mcp"]);
    assert!(plan.ok().contains(&format!("\"command\": \"{}\",", link.display())), "{}", plan.stdout);

    // Another program of the same name first on PATH is worth knowing about.
    std::fs::remove_file(&link).unwrap();
    sb.fake("codefast", "exit 0");
    let r = report(&sb, &[]);
    assert_eq!(r["binary"]["on_path_is_this_binary"], false);
    assert!(text(&sb, &[]).contains(&format!("  on PATH   another binary comes first: {}\n", link.display())));
    let off = [("CODEFAST_HOOKS", "off")];
    assert_eq!(report(&sb, &off)["binary"]["hooks_enabled"], false);
    assert!(text(&sb, &off).contains("  hooks     turned off by CODEFAST_HOOKS\n"));
}

/// The scripts this repository ships under the name `codefast` are not "another binary": a
/// plugin's launcher (Claude Code puts it on the `PATH` of its shell tool) and the npm wrapper
/// both find the binary and run it, and doctor is usually started through one of them.
#[cfg(unix)]
#[test]
fn a_launcher_or_wrapper_on_path_is_not_another_binary() {
    use std::os::unix::fs::PermissionsExt;
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (shipped, via, said) in [
        ("bin/codefast", "launcher", "a plugin's launcher, which finds and runs the binary"),
        ("npm/codefast/bin/codefast.js", "npm", "the npm wrapper, which runs the binary"),
    ] {
        let sb = Sandbox::new(&format!("via-{via}"));
        // As an install leaves it: the script itself for a plugin, a link named codefast for npm.
        let on_path = sb.bin.join("codefast");
        if via == "launcher" {
            std::fs::copy(repo.join(shipped), &on_path).unwrap();
            std::fs::set_permissions(&on_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        } else {
            std::os::unix::fs::symlink(repo.join(shipped), &on_path).unwrap();
        }
        let r = report(&sb, &[]);
        assert_eq!(
            (&r["binary"]["on_path"], &r["binary"]["on_path_is_this_binary"], &r["binary"]["on_path_via"]),
            (&json!(on_path.to_str().unwrap()), &json!(false), &json!(via)),
            "{}",
            r["binary"]
        );
        let text = text(&sb, &[]);
        assert!(text.contains(&format!("  on PATH   yes, through {} ({said})\n", on_path.display())), "{text}");
        assert!(!text.contains("another binary"), "{text}");
    }
}

#[test]
fn each_harness_found_is_reported_with_what_is_registered() {
    let sb = Sandbox::new("harnesses");
    // Nothing installed, nothing found.
    let r = report(&sb, &[]);
    assert_eq!(r["harnesses"], json!([]));
    assert_eq!(r["not_found"].as_array().unwrap().len(), 32);
    assert!(text(&sb, &[]).contains("harnesses found on this machine\n  none (`codefast setup --list` names the ones setup knows)\n"));

    // Found but not registered: the command that would register it.
    std::fs::create_dir_all(sb.home(".cursor")).unwrap();
    sb.fake("codex", "exit 0");
    let said = text(&sb, &[]);
    assert!(
        said.contains("  cursor        not registered: codefast setup cursor --apply\n")
            && said.contains("  codex         not registered: codefast setup codex --apply\n"),
        "{said}"
    );
    assert!(said.contains("\nnot found: claude-code, gemini-cli, copilot, "), "{said}");
    let r = report(&sb, &[]);
    assert_eq!(
        (&harness(&r, "cursor")["registered"], &harness(&r, "cursor")["user"]["mcp"]["state"], &harness(&r, "cursor")["user"]["skill"]["state"]),
        (&json!(false), &json!("missing"), &json!("missing"))
    );
    assert!(harness(&r, "codex")["found"].as_str().unwrap().ends_with("/bin/codex on PATH"));

    // Registered: every part with the file that holds it.
    std::fs::remove_file(sb.bin.join("codex")).unwrap();
    sb.setup(&["cursor", "codex", "--hooks", "--apply"]).ok();
    sb.setup(&["cursor", "--scope", "project", "--with", "mcp", "--apply"]).ok();
    let r = report(&sb, &[]);
    let cursor = harness(&r, "cursor");
    assert_eq!(cursor["registered"], true);
    assert_eq!(
        cursor["user"]["mcp"],
        json!({"file": sb.home(".cursor/mcp.json").to_str().unwrap(), "state": "registered", "command": format!("{BIN} mcp"), "command_found": true})
    );
    assert_eq!(cursor["user"]["skill"], json!({"file": sb.home(".agents/skills/codefast/SKILL.md").to_str().unwrap(), "state": "current"}));
    // The project's entry names the bare command, which this sandbox's PATH does not have.
    assert_eq!(
        cursor["project"]["mcp"],
        json!({"file": sb.project(".cursor/mcp.json").to_str().unwrap(), "state": "registered", "command": "codefast mcp", "command_found": false})
    );
    assert!(cursor["user"].get("hooks").is_none(), "setup writes no hooks for Cursor, so doctor looks for none");
    let codex = harness(&r, "codex");
    assert_eq!(
        (&codex["registered"], &codex["user"]["mcp"]["state"], &codex["user"]["hooks"]["state"], &codex["project"]["hooks"]["state"]),
        (&json!(true), &json!("registered"), &json!("registered"), &json!("missing")),
        "{codex}"
    );
    let said = text(&sb, &[]);
    for line in [
        format!(
            "  cursor        registered\n      mcp     ~/.cursor/mcp.json runs `{BIN} mcp`\n      skill   ~/.agents/skills/codefast/SKILL.md\n      mcp     .cursor/mcp.json runs `codefast mcp`, which does not exist\n"
        ),
        format!(
            "  codex         registered\n      mcp     ~/.codex/config.toml runs `{BIN} mcp`\n      skill   ~/.agents/skills/codefast/SKILL.md\n      hooks   ~/.codex/hooks.json\n"
        ),
    ] {
        assert!(said.contains(&line), "{line} in {said}");
    }

    // A server of that name that is not ours; an older copy of the skill; half the hooks.
    write(&sb.home(".cursor/mcp.json"), "{\"mcpServers\": {\"codefast\": {\"command\": \"node\", \"args\": [\"x.js\"]}}}");
    std::fs::remove_file(sb.project(".cursor/mcp.json")).unwrap();
    write(&sb.home(".agents/skills/codefast/SKILL.md"), "---\nname: codefast\ndescription: older\n---\n# codefast\n");
    let hooks = read(&sb.home(".codex/hooks.json")).replace("hook pre-tool-use", "hook-was-renamed");
    write(&sb.home(".codex/hooks.json"), &hooks);
    let r = report(&sb, &[]);
    assert_eq!(
        (&harness(&r, "cursor")["user"]["mcp"]["state"], &harness(&r, "cursor")["user"]["skill"]["state"], &harness(&r, "codex")["user"]["hooks"]["state"]),
        (&json!("other"), &json!("outdated"), &json!("partial"))
    );
    let said = text(&sb, &[]);
    assert!(said.contains("      mcp     ~/.cursor/mcp.json has another server named codefast: `node x.js`\n"), "{said}");
    assert!(said.contains("      skill   ~/.agents/skills/codefast/SKILL.md is an older copy: `codefast setup cursor --apply` updates it\n"), "{said}");
    assert!(said.contains("      hooks   ~/.codex/hooks.json registers only some of the hooks\n"), "{said}");
    // A file that cannot be parsed is said to be unreadable, and left alone.
    write(&sb.home(".cursor/mcp.json"), "{\"mcpServers\": ");
    assert_eq!(harness(&report(&sb, &[]), "cursor")["user"]["mcp"]["state"], "unreadable");
    assert!(text(&sb, &[]).contains("      mcp     ~/.cursor/mcp.json cannot be read\n"));
    assert_eq!(read(&sb.home(".cursor/mcp.json")), "{\"mcpServers\": ");
}

#[test]
fn a_plugin_install_counts_as_registered() {
    let sb = Sandbox::new("plugin");
    // As Claude Code 2.1 records an installed plugin (observed in a sandbox install).
    write(
        &sb.home(".claude/plugins/installed_plugins.json"),
        "{\n  \"version\": 2,\n  \"plugins\": {\n    \"codefast@codefast\": [{\"scope\": \"user\", \"installPath\": \"/x/cache/codefast/codefast/0.1.0\", \"version\": \"0.1.0\"}],\n    \"other@market\": [{\"scope\": \"user\"}]\n  }\n}\n",
    );
    write(&sb.home(".claude/settings.json"), "{\n  \"enabledPlugins\": {\"codefast@codefast\": true}\n}\n");
    let r = report(&sb, &[]);
    assert_eq!((&harness(&r, "claude-code")["registered"], &harness(&r, "claude-code")["plugin"]), (&json!(true), &json!("codefast@codefast")));
    assert!(text(&sb, &[]).contains("  claude-code   registered\n      plugin  codefast@codefast\n"));
    // setup sees the same and does not register the parts a second time.
    let out = sb.setup(&["claude-code", "--hooks", "--apply"]);
    assert!(
        out.ok().contains("  plugin ok       codefast@codefast is installed: the skill, the MCP server and the hooks come with it\n")
            && out.stdout.ends_with("0 changed, 1 already in place.\n"),
        "{}",
        out.stdout
    );
    assert!(!sb.home(".claude.json").exists() && !sb.home(".claude/skills").exists());
    // Installed but switched off is not registered.
    write(&sb.home(".claude/settings.json"), "{\n  \"enabledPlugins\": {\"codefast@codefast\": false}\n}\n");
    let r = report(&sb, &[]);
    assert_eq!((&harness(&r, "claude-code")["registered"], &harness(&r, "claude-code")["plugin"]), (&json!(false), &json!(null)));
    // Codex records an enabled plugin in config.toml.
    write(&sb.home(".codex/config.toml"), "[plugins.\"codefast@codefast\"]\nenabled = true\n");
    assert_eq!(harness(&report(&sb, &[]), "codex")["plugin"], "codefast@codefast");
}

/// Claude Code installs a plugin for one project when asked to (`--scope project`): the record
/// names that project, and the project's own settings switch the plugin on. In every other
/// project Claude Code lists it as disabled and loads nothing of it, so there codefast is not
/// registered and setup still has its work to do.
#[test]
fn a_plugin_installed_for_one_project_counts_there_and_nowhere_else() {
    let sb = Sandbox::new("plugin-scope");
    let other = sb.home("work/other");
    std::fs::create_dir_all(other.join(".git")).unwrap();
    std::fs::create_dir_all(sb.project("src/deep")).unwrap();
    // As Claude Code 2.1.323 records `claude plugin install codefast@codefast --scope project`
    // (observed in a sandbox install).
    write(
        &sb.home(".claude/plugins/installed_plugins.json"),
        &format!(
            "{{\n  \"version\": 2,\n  \"plugins\": {{\n    \"codefast@codefast\": [{{\"scope\": \"project\", \"installPath\": \"/x/cache/codefast/codefast/0.1.0\", \"version\": \"0.1.0\", \"projectPath\": \"{}\"}}]\n  }}\n}}\n",
            sb.project.display()
        ),
    );
    write(&sb.project(".claude/settings.json"), "{\n  \"enabledPlugins\": {\n    \"codefast@codefast\": true\n  }\n}\n");
    let claude = |dir: &std::path::Path| {
        let out = sb.run_in(dir, &["doctor", "--json"], &[]);
        let r: Value = serde_json::from_str(out.ok()).unwrap();
        let h = harness(&r, "claude-code");
        (h["registered"].clone(), h["plugin"].clone())
    };
    // In the project, at its top and below it: installed.
    assert_eq!(claude(&sb.project), (json!(true), json!("codefast@codefast")));
    assert_eq!(claude(&sb.project("src/deep")), (json!(true), json!("codefast@codefast")));
    // In another project: not installed, and setup registers the server and the skill.
    assert_eq!(claude(&other), (json!(false), json!(null)));
    let plan = sb.run_in(&other, &["setup", "claude-code"], &[]);
    assert!(
        plan.ok().contains("  mcp    plan     create ~/.claude.json\n") && plan.stdout.contains("  skill  plan     create ~/.claude/skills/codefast/SKILL.md"),
        "{}",
        plan.stdout
    );
    assert!(!plan.stdout.contains("is installed"), "{}", plan.stdout);
    // The project's own settings can switch off a plugin that is installed for every project,
    // and the local settings have the last word.
    write(
        &sb.home(".claude/plugins/installed_plugins.json"),
        "{\"version\": 2, \"plugins\": {\"codefast@codefast\": [{\"scope\": \"user\", \"installPath\": \"/x\", \"version\": \"0.1.0\"}]}}",
    );
    write(&sb.home(".claude/settings.json"), "{\"enabledPlugins\": {\"codefast@codefast\": true}}");
    assert_eq!(claude(&other), (json!(true), json!("codefast@codefast")));
    write(&sb.project(".claude/settings.json"), "{\"enabledPlugins\": {\"codefast@codefast\": false}}");
    assert_eq!(claude(&sb.project), (json!(false), json!(null)));
    assert_eq!(claude(&other), (json!(true), json!("codefast@codefast")));
    write(&sb.project(".claude/settings.local.json"), "{\"enabledPlugins\": {\"codefast@codefast\": true}}");
    assert_eq!(claude(&sb.project), (json!(true), json!("codefast@codefast")));

    // Codex: a repository's own .codex/config.toml switches a plugin on or off for that repository.
    write(&sb.home(".codex/config.toml"), "[plugins.\"codefast@codefast\"]\nenabled = true\n");
    let codex = |dir: &std::path::Path| {
        let out = sb.run_in(dir, &["doctor", "--json"], &[]);
        let r: Value = serde_json::from_str(out.ok()).unwrap();
        harness(&r, "codex")["plugin"].clone()
    };
    assert_eq!((codex(&sb.project), codex(&other)), (json!("codefast@codefast"), json!("codefast@codefast")));
    write(&sb.project(".codex/config.toml"), "[plugins.\"codefast@codefast\"]\nenabled = false\n");
    assert_eq!((codex(&sb.project), codex(&other)), (json!(null), json!("codefast@codefast")));
}

#[test]
fn a_codex_plugin_brings_the_skill_and_leaves_the_server_and_the_hooks_to_setup() {
    let sb = Sandbox::new("codex-plugin");
    // As `codex plugin add codefast@codefast` records it (codex-cli 0.164.0, in a sandbox).
    let recorded = "[marketplaces.codefast]\nsource_type = \"local\"\nsource = \"/x/src\"\n\n[plugins.\"codefast@codefast\"]\nenabled = true\n";
    write(&sb.home(".codex/config.toml"), recorded);
    // The plugin alone counts as registered, but its server is one Codex starts in the plugin's
    // directory: doctor says what that costs and how to do better.
    let r = report(&sb, &[]);
    let codex = harness(&r, "codex");
    assert_eq!(
        (&codex["registered"], &codex["plugin"], &codex["plugin_server_needs_root"], &codex["registered_twice"]),
        (&json!(true), &json!("codefast@codefast"), &json!(true), &json!([])),
        "{codex}"
    );
    assert!(
        text(&sb, &[]).contains(
            "  codex         registered\n      plugin  codefast@codefast\n      mcp     the plugin's server is started in the plugin's directory and needs `root` with every call: `codefast setup codex --apply` registers one that starts in the project\n"
        ),
        "{}",
        text(&sb, &[])
    );

    // Codex loads no hooks from a plugin with an Agent Plugins manifest (docs/HARNESSES.md), and
    // a server in config.toml takes the place of the plugin's server of the same name (seen with
    // codex-cli 0.164.0: only the configured one is started, in the project). So with the plugin
    // installed both are still setup's to register; the skill is not written a second time.
    let out = sb.setup(&["codex", "--hooks", "--apply"]);
    assert!(out.ok().contains("  plugin ok       codefast@codefast is installed: the skill comes with it\n"), "{}", out.stdout);
    assert!(
        out.stdout.contains("  mcp    done     edit ~/.codex/config.toml\n")
            && out.stdout.contains("  hooks  done     create ~/.codex/hooks.json (")
            && out.stdout.ends_with("2 changed, 1 already in place.\n"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains(
            "  note            Codex CLI uses the server registered here in place of the plugin's own, which it starts in the plugin's directory and which needs `root` with every call\n"
        ),
        "{}",
        out.stdout
    );
    assert!(!out.stdout.contains("as a plugin"), "the plugin route is not advertised to those who took it: {}", out.stdout);
    assert_eq!(read(&sb.home(".codex/config.toml")), format!("{recorded}\n[mcp_servers.codefast]\ncommand = \"{BIN}\"\nargs = [\"mcp\"]\n"));
    assert!(!sb.home(".agents").exists());
    let hooks = json(&sb.home(".codex/hooks.json"));
    assert_eq!(hooks["hooks"].as_object().unwrap().keys().collect::<Vec<_>>(), ["PostToolUse", "PreToolUse", "SessionStart", "UserPromptSubmit"]);
    // Nothing is registered twice by that: Codex runs one server named codefast, the configured one.
    let r = report(&sb, &[]);
    let codex = harness(&r, "codex");
    assert_eq!(
        (&codex["registered"], &codex["plugin"], &codex["user"]["mcp"]["state"], &codex["user"]["hooks"]["state"]),
        (&json!(true), &json!("codefast@codefast"), &json!("registered"), &json!("registered")),
        "{codex}"
    );
    assert_eq!((&codex["registered_twice"], &codex["plugin_server_needs_root"]), (&json!([]), &json!(false)), "{codex}");
    let said = text(&sb, &[]);
    assert!(!said.contains("      twice ") && !said.contains("needs `root`"), "{said}");
    assert!(sb.setup(&["codex", "--hooks", "--apply"]).ok().ends_with("0 changed, 3 already in place.\n"));

    // For Claude Code the plugin brings the hooks too.
    write(
        &sb.home(".claude/plugins/installed_plugins.json"),
        "{\"version\": 2, \"plugins\": {\"codefast@codefast\": [{\"scope\": \"user\", \"installPath\": \"/x\", \"version\": \"0.1.0\"}]}}",
    );
    write(
        &sb.home(".claude/settings.json"),
        &format!(
            "{{\"enabledPlugins\": {{\"codefast@codefast\": true}}, \"hooks\": {{\"SessionStart\": [{{\"hooks\": [{{\"type\": \"command\", \"command\": \"{BIN} hook session-start\"}}]}}]}}}}"
        ),
    );
    assert_eq!(harness(&report(&sb, &[]), "claude-code")["registered_twice"], json!(["hooks"]));
    assert!(text(&sb, &[]).contains("      twice   the plugin brings the hooks, and the files below register them again: remove one of the two\n"));

    // A plugin that is switched off brings nothing: everything is setup's to register.
    write(&sb.home(".codex/config.toml"), "[plugins.\"codefast@codefast\"]\nenabled = false\n");
    assert_eq!(harness(&report(&sb, &[]), "codex")["plugin"], json!(null));
    let out = sb.setup(&["codex", "--apply"]);
    assert!(
        out.ok().contains("  mcp    done     edit ~/.codex/config.toml\n")
            && out.stdout.contains("  skill  done     create ~/.agents/skills/codefast/SKILL.md"),
        "{}",
        out.stdout
    );
}

#[test]
fn the_project_instructions_are_reported() {
    let sb = Sandbox::new("rules");
    assert!(
        text(&sb, &[]).contains("project instructions\n  no AGENTS.md, CLAUDE.md or GEMINI.md here: `codefast setup agents-md --apply` writes AGENTS.md\n")
    );
    write(&sb.project("AGENTS.md"), "# Project\n\nUse tabs.\n");
    write(&sb.project("CLAUDE.md"), "@AGENTS.md\n");
    assert_eq!(report(&sb, &[])["rules"], json!({"AGENTS.md": "no block", "CLAUDE.md": "imports AGENTS.md"}));
    sb.setup(&["agents-md", "--apply"]).ok();
    assert_eq!(report(&sb, &[])["rules"], json!({"AGENTS.md": "current", "CLAUDE.md": "imports AGENTS.md"}));
    assert!(text(&sb, &[]).contains("  AGENTS.md codefast block is current\n  CLAUDE.md imports AGENTS.md\n"));
    // A block someone edited (or an older release wrote) is outdated, not missing.
    let edited = read(&sb.project("AGENTS.md")).replace("## Code navigation: codefast", "## Code navigation");
    write(&sb.project("AGENTS.md"), &edited);
    assert_eq!(report(&sb, &[])["rules"]["AGENTS.md"], "outdated");
    // Aider is registered when its configuration reads a file that carries the block.
    sb.setup(&["aider", "--apply"]).ok();
    let r = report(&sb, &[]);
    assert_eq!((&harness(&r, "aider")["registered"], &r["rules"]["AGENTS.md"]), (&json!(true), &json!("current")));
}

#[test]
fn doctor_takes_one_option() {
    let sb = Sandbox::new("usage");
    let bad = sb.run(&["doctor", "--fix"], &[]);
    assert_eq!((bad.code, bad.stdout.as_str()), (1, ""));
    assert!(bad.stderr.contains("unknown option `--fix` (usage: codefast doctor [--json])"), "{}", bad.stderr);
    assert!(sb.run(&["doctor", "help"], &[]).ok().contains("Usage: codefast doctor [--json]"));
    // The report is one JSON document with the same top-level members every time.
    let r = report(&sb, &[]);
    assert_eq!(r.as_object().unwrap().keys().collect::<Vec<_>>(), ["binary", "harnesses", "index", "not_found", "rules"]);
}
