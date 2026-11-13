//! Which repository a tool call addresses. The order is: the `root` tool argument, the `--root`
//! flag, `CODEFAST_ROOT`, `CLAUDE_PROJECT_DIR`, then the working directory when it lies inside a
//! git work tree. With none of these the call fails with a tool error that says what is missing.
//! Whatever the source, a directory inside a repository stands for that repository, and the home
//! directory is never taken for a project.

#[path = "packaging/support.rs"]
mod support;

use serde_json::{Value, json};
use support::*;

/// Ask the server which repository it sees: the marker function of exactly one fixture resolves.
fn answers_from(s: &mut Session, repo: &Repo, extra: Value) -> bool {
    let mut arguments = json!({"queries": [repo.marker]});
    if let Some(extra) = extra.as_object() {
        for (k, v) in extra {
            arguments[k] = v.clone();
        }
    }
    let r = s.call("search", arguments);
    // The definition itself, not a near match from another fixture.
    r["isError"] == false && text(&r).contains(&format!("int {}(int x)", repo.marker))
}

#[test]
fn the_working_directory_counts_when_it_is_inside_a_work_tree() {
    let repo = Repo::new("cwd");
    let mut s = Session::start(&repo.path("src"), &[], &[]);
    assert!(answers_from(&mut s, &repo, Value::Null), "a subdirectory resolves to the work tree's top");
    assert!(repo.path(".codefast").is_dir(), "the index lives at the top, where the CLI puts it");
    assert!(!repo.path("src/.codefast").exists());
}

#[test]
fn outside_a_repository_the_error_names_what_is_missing() {
    let nowhere = scratch("nowhere");
    let mut s = Session::start(&nowhere, &[], &[]);
    let r = s.call("search", json!({"queries": ["anything"]}));
    assert_eq!(r["isError"], true, "{r}");
    let message = text(&r);
    for needle in ["no repository root", "`root`", "absolute path", "--root", "CODEFAST_ROOT", "git work tree", nowhere.to_str().unwrap()] {
        assert!(message.contains(needle), "the error should mention {needle}: {message}");
    }
    assert!(!nowhere.join(".codefast").exists(), "nothing is indexed by accident");
    // The instructions say so up front, in both eras, so the model passes `root` from the first call.
    let discover = s.modern("server/discover", Value::Null);
    assert!(discover["result"]["instructions"].as_str().unwrap().contains("pass `root`"));
    let init = s.legacy("initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "c", "version": "1"}}));
    assert!(init["result"]["instructions"].as_str().unwrap().contains("pass `root`"));
    // The listing itself needs no repository.
    assert_eq!(s.modern("tools/list", Value::Null)["result"]["tools"].as_array().unwrap().len(), 5);
}

#[test]
fn the_root_argument_wins_over_everything() {
    let (a, b, c, d) = (Repo::new("arg-a"), Repo::new("arg-b"), Repo::new("arg-c"), Repo::new("arg-d"));
    // Started in A, with --root B, CODEFAST_ROOT=C: a call that names D gets D.
    let mut s = Session::start(&a.dir, &["--root", b.str()], &[("CODEFAST_ROOT", c.str()), ("CLAUDE_PROJECT_DIR", c.str())]);
    assert!(answers_from(&mut s, &d, json!({"root": d.str()})));
    assert!(answers_from(&mut s, &b, Value::Null), "without the argument the flag applies");
    assert!(!answers_from(&mut s, &a, Value::Null));
    // Each call stands alone: naming D once does not make it the default.
    assert!(!answers_from(&mut s, &d, Value::Null));
    // A file URI and a directory below the top are understood.
    assert!(answers_from(&mut s, &d, json!({"root": format!("file://{}", d.str())})));
    assert!(answers_from(&mut s, &d, json!({"root": d.path("src").to_str().unwrap()})));
    assert!(!d.path("src/.codefast").exists(), "a subdirectory is never indexed on its own");
    // An empty string means "not given".
    assert!(answers_from(&mut s, &b, json!({"root": ""})));
}

#[test]
fn the_flag_beats_the_environment() {
    let (flag, env, cwd) = (Repo::new("flag"), Repo::new("flag-env"), Repo::new("flag-cwd"));
    for args in [vec!["--root".to_string(), flag.str().to_string()], vec![format!("--root={}", flag.str())]] {
        let args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let mut s = Session::start(&cwd.dir, &args, &[("CODEFAST_ROOT", env.str()), ("CLAUDE_PROJECT_DIR", env.str())]);
        assert!(answers_from(&mut s, &flag, Value::Null));
        assert!(!answers_from(&mut s, &env, Value::Null));
    }
}

#[test]
fn codefast_root_beats_claude_project_dir_which_beats_the_working_directory() {
    let (own, claude, cwd) = (Repo::new("env-own"), Repo::new("env-claude"), Repo::new("env-cwd"));
    let mut s = Session::start(&cwd.dir, &[], &[("CODEFAST_ROOT", own.str()), ("CLAUDE_PROJECT_DIR", claude.str())]);
    assert!(answers_from(&mut s, &own, Value::Null));
    let mut s = Session::start(&cwd.dir, &[], &[("CLAUDE_PROJECT_DIR", claude.str())]);
    assert!(answers_from(&mut s, &claude, Value::Null));
    assert!(!answers_from(&mut s, &cwd, Value::Null));
    // Claude Code sets the variable for stdio servers even when they start elsewhere.
    let nowhere = scratch("env-nowhere");
    let mut s = Session::start(&nowhere, &[], &[("CLAUDE_PROJECT_DIR", claude.str())]);
    assert!(answers_from(&mut s, &claude, Value::Null));
}

#[test]
fn placeholders_a_harness_left_unexpanded_are_not_roots() {
    let repo = Repo::new("placeholder");
    // `--root ${workspaceFolder}` and `CODEFAST_ROOT=${CLAUDE_PROJECT_DIR}` arrive verbatim when
    // a harness does not know the variable; the next source in the order applies instead.
    let mut s = Session::start(&repo.dir, &["--root", "${workspaceFolder}"], &[("CODEFAST_ROOT", "${CLAUDE_PROJECT_DIR}"), ("CLAUDE_PROJECT_DIR", "")]);
    assert!(answers_from(&mut s, &repo, Value::Null), "the working directory is used");
    assert!(!repo.path("${workspaceFolder}").exists());
    let nowhere = scratch("placeholder-nowhere");
    let mut s = Session::start(&nowhere, &["--root", "${workspacePath}"], &[]);
    let r = s.call("search", json!({"queries": ["x"]}));
    assert_eq!(r["isError"], true);
    assert!(text(&r).contains("no repository root"), "{}", text(&r));
}

#[test]
fn the_plugin_directory_is_not_mistaken_for_the_project() {
    // Agent Plugins starts a plugin's server in the plugin's own directory, which is a git
    // checkout. That checkout is not what the user is working on.
    let plugin = Repo::new("plugin-root");
    for var in ["PLUGIN_ROOT", "CLAUDE_PLUGIN_ROOT", "CODEFAST_PLUGIN_ROOT"] {
        let mut s = Session::start(&plugin.dir, &[], &[(var, plugin.str())]);
        let r = s.call("search", json!({"queries": [plugin.marker]}));
        assert_eq!(r["isError"], true, "{var}: {r}");
        assert!(text(&r).contains("plugin directory") && text(&r).contains("`root`"), "{}", text(&r));
        assert!(!plugin.path(".codefast").exists(), "{var}: the plugin checkout was indexed");
        // The project named by the caller still works from there.
        let project = Repo::new("plugin-project");
        assert!(answers_from(&mut s, &project, json!({"root": project.str()})));
    }
    // With an explicit root the plugin directory is irrelevant.
    let project = Repo::new("plugin-explicit");
    let mut s = Session::start(&plugin.dir, &["--root", project.str()], &[("PLUGIN_ROOT", plugin.str())]);
    assert!(answers_from(&mut s, &project, Value::Null));
}

#[test]
fn a_bad_root_argument_is_a_tool_error_the_model_can_fix() {
    let repo = Repo::new("badroot");
    let plain = scratch("badroot-plain");
    let mut s = Session::in_repo(&repo);
    let cases = [
        (json!("src"), "absolute path"),
        (json!("./"), "absolute path"),
        (json!("/definitely/not/here/at/all"), "not a directory"),
        (json!(plain.to_str().unwrap()), "not inside a git work tree"),
        (json!(42), "must be a string"),
        (json!(["/a", "/b"]), "must be a string"),
    ];
    for (root, expect) in cases {
        let r = s.call("search", json!({"queries": ["dev_read"], "root": root}));
        assert_eq!(r["isError"], true, "{root}: {r}");
        assert!(text(&r).contains(expect), "{root}: expected `{expect}` in `{}`", text(&r));
    }
    assert!(!plain.join(".codefast").exists(), "a directory outside any work tree is never indexed");
    // A directory that was indexed on purpose is accepted even without git.
    std::fs::create_dir_all(plain.join(".codefast")).unwrap();
    write(&plain.join("one.c"), "int only_here(void)\n{\n    return 1;\n}\n");
    let r = s.call("search", json!({"queries": ["only_here"], "root": plain.to_str().unwrap()}));
    assert_eq!(r["isError"], false, "{r}");
    assert!(text(&r).contains("one.c:1"), "{}", text(&r));
    // A file inside the repository stands for the repository.
    assert!(answers_from(&mut s, &repo, json!({"root": repo.path("src/io.c").to_str().unwrap()})));
    // A path is resolved before the repository around it is looked for: this one leaves the
    // repository again and names the directory outside every work tree.
    let around = format!("{}/src/../../{}", repo.str(), plain.file_name().unwrap().to_str().unwrap());
    std::fs::remove_dir_all(plain.join(".codefast")).unwrap();
    let r = s.call("search", json!({"queries": ["dev_read"], "root": around}));
    assert_eq!(r["isError"], true, "{around} is not inside the repository it passes through: {r}");
    assert!(text(&r).contains("not inside a git work tree"), "{}", text(&r));
    // An error repeats the root it was given, but not at any length.
    let r = s.call("search", json!({"queries": ["dev_read"], "root": "relative/".repeat(20_000)}));
    assert_eq!(r["isError"], true);
    assert!(text(&r).len() < 2_000, "{} bytes of error", text(&r).len());
}

#[test]
fn a_file_uri_names_the_same_directory_as_its_path() {
    // Harnesses hand workspace folders to the model as URIs, where a space is `%20`.
    let outer = scratch("uri");
    let dir = outer.join("My Project (v2)");
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    write(&dir.join("src/space.c"), "int in_a_spaced_path(void)\n{\n    return 1;\n}\n");
    let nowhere = scratch("uri-nowhere");
    let mut s = Session::start(&nowhere, &[], &[]);
    let plain = dir.to_str().unwrap().to_string();
    let escaped = plain.replace(' ', "%20").replace('(', "%28").replace(')', "%29");
    for root in
        [plain.clone(), format!("file://{plain}"), format!("file://{escaped}"), format!("file://localhost{escaped}"), format!("file://{escaped}/src/space.c")]
    {
        let r = s.call("search", json!({"queries": ["in_a_spaced_path"], "root": root}));
        assert_eq!(r["isError"], false, "{root}: {r}");
        assert!(text(&r).contains("src/space.c:1"), "{root}: {}", text(&r));
    }
    // Outside a URI a percent sign is an ordinary character.
    let r = s.call("search", json!({"queries": ["x"], "root": escaped}));
    assert_eq!(r["isError"], true, "{r}");
    assert!(text(&r).contains("not a directory"), "{}", text(&r));
    // A URI of another host, or of another scheme, is not a local path.
    for root in ["file://example.com/tmp", "https://example.com/repo", "file:relative"] {
        let r = s.call("search", json!({"queries": ["x"], "root": root}));
        assert_eq!(r["isError"], true, "{root}: {r}");
    }
}

#[test]
fn a_configured_root_that_does_not_exist_is_reported_per_call() {
    let cwd = Repo::new("gone-cwd");
    let mut s = Session::start(&cwd.dir, &["--root", "/definitely/not/here/at/all"], &[]);
    let r = s.call("search", json!({"queries": ["dev_read"]}));
    assert_eq!(r["isError"], true, "the flag is not silently replaced by the working directory: {r}");
    assert!(text(&r).contains("/definitely/not/here/at/all"), "{}", text(&r));
    // Listing still works, and so does a call that names a real repository.
    assert!(s.modern("tools/list", Value::Null)["result"]["tools"].is_array());
    assert!(answers_from(&mut s, &cwd, json!({"root": cwd.str()})));
}

#[test]
fn a_configured_directory_below_the_top_resolves_to_its_repository() {
    // Harnesses fill these settings with the directory the session was started in: Claude Code
    // sets CLAUDE_PROJECT_DIR to the launch directory, Gemini expands `--root ${workspacePath}`,
    // and the Claude plugin passes CODEFAST_ROOT=${CLAUDE_PROJECT_DIR}. Started below the top of
    // a repository, each of them must still mean that repository, exactly as the command line
    // run from the same directory does.
    let nowhere = scratch("sub-nowhere");
    for (tag, how) in [("sub-flag", "flag"), ("sub-own", "CODEFAST_ROOT"), ("sub-claude", "CLAUDE_PROJECT_DIR")] {
        let repo = Repo::new(tag);
        let sub = repo.path("src");
        let sub = sub.to_str().unwrap();
        let mut s = match how {
            "flag" => Session::start(&nowhere, &["--root", sub], &[]),
            var => Session::start(&nowhere, &[], &[(var, sub)]),
        };
        // The marker lives in lib/, outside the configured directory.
        assert!(answers_from(&mut s, &repo, Value::Null), "{how}: the whole repository answers, not only src/");
        let r = s.call("search", json!({"queries": [repo.marker]}));
        assert!(text(&r).contains("lib/marker.c:1"), "{how}: paths are relative to the top: {}", text(&r));
        assert!(repo.path(".codefast").is_dir(), "{how}: the index lives at the top");
        assert!(!repo.path("src/.codefast").exists(), "{how}: a subdirectory is never indexed on its own");
        // The model is told what the paths in answers are relative to, in both eras.
        let discover = s.modern("server/discover", Value::Null);
        let instructions = discover["result"]["instructions"].as_str().unwrap();
        assert!(instructions.contains(repo.str()), "{how}: the instructions name the root: {instructions}");
        assert!(instructions.len() <= 4096 && instructions.starts_with("codefast"));
        assert_eq!(discover["result"]["cacheScope"], "private", "a result that carries a local path is not for shared caches");
        let init = s.legacy("initialize", json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "c", "version": "1"}}));
        assert!(init["result"]["instructions"].as_str().unwrap().contains(repo.str()));
    }
    // The same holds for the working directory, which already resolved to the top.
    let repo = Repo::new("sub-cwd");
    let mut s = Session::start(&repo.path("src"), &[], &[]);
    assert!(answers_from(&mut s, &repo, Value::Null));
    assert!(s.modern("server/discover", Value::Null)["result"]["instructions"].as_str().unwrap().contains(repo.str()));
    // Started at the top there is nothing to explain, and the instructions stay free of paths.
    let mut s = Session::in_repo(&repo);
    let instructions = s.modern("server/discover", Value::Null)["result"]["instructions"].as_str().unwrap().to_string();
    assert!(!instructions.contains(repo.str()), "{instructions}");
    // A directory that was indexed on purpose keeps its own index.
    let own = Repo::new("sub-indexed");
    std::fs::create_dir_all(own.path("src/.codefast")).unwrap();
    let mut s = Session::start(&nowhere, &[], &[("CLAUDE_PROJECT_DIR", own.path("src").to_str().unwrap())]);
    assert!(!answers_from(&mut s, &own, Value::Null), "the marker in lib/ is outside the directory indexed on purpose");
    let r = s.call("search", json!({"queries": ["copy_all"]}));
    assert!(text(&r).contains("main.c:3"), "{}", text(&r));
}

#[test]
fn a_configured_directory_outside_any_repository_is_served_as_it_is() {
    // A project that does not use git still works when the harness names it.
    for var in ["CODEFAST_ROOT", "CLAUDE_PROJECT_DIR"] {
        let plain = scratch("plain-project");
        write(&plain.join("one.c"), "int plain_project_fn(void)\n{\n    return 1;\n}\n");
        let nowhere = scratch("plain-nowhere");
        let mut s = Session::start(&nowhere, &[], &[(var, plain.to_str().unwrap())]);
        let r = s.call("search", json!({"queries": ["plain_project_fn"]}));
        assert_eq!(r["isError"], false, "{var}: {r}");
        assert!(text(&r).contains("one.c:1"), "{var}: {}", text(&r));
    }
}

#[test]
fn the_home_directory_is_never_indexed_by_accident() {
    // A harness started in the home directory hands that directory over as the project. Walking
    // it would take minutes, read private files and leave an index of them behind.
    let nowhere = scratch("home-nowhere");
    for how in ["flag", "CODEFAST_ROOT", "CLAUDE_PROJECT_DIR", "parent"] {
        // The home directory sits one level down, so that "the directory above home" is a
        // throwaway directory too.
        let outer = scratch("fake-home");
        let home = outer.join("me");
        write(&home.join("Documents/notes.c"), "int private_notes(void)\n{\n    return 1;\n}\n");
        let h = home.to_str().unwrap();
        let above = outer.to_str().unwrap().to_string();
        let mut s = match how {
            "flag" => Session::start(&nowhere, &["--root", h], &[("HOME", h)]),
            "parent" => Session::start(&nowhere, &[], &[("HOME", h), ("CLAUDE_PROJECT_DIR", &above)]),
            var => Session::start(&nowhere, &[], &[("HOME", h), (var, h)]),
        };
        let r = s.call("search", json!({"queries": ["private_notes"]}));
        assert_eq!(r["isError"], true, "{how}: {r}");
        let message = text(&r);
        for needle in ["home directory", "`root`", "--root", "CODEFAST_ROOT"] {
            assert!(message.contains(needle), "{how}: the error should mention {needle}: {message}");
        }
        assert!(!home.join(".codefast").exists(), "{how}: the home directory was indexed");
        assert!(!outer.join(".codefast").exists(), "{how}: the directory above home was indexed");
        // The instructions ask for a root up front, and a call that names a repository works.
        assert!(s.modern("server/discover", Value::Null)["result"]["instructions"].as_str().unwrap().contains("pass `root`"));
        let project = Repo::new("home-project");
        assert!(answers_from(&mut s, &project, json!({"root": project.str()})));
        // A project inside the home directory is an ordinary project.
        let inside = home.join("work/app");
        write(&inside.join("app.c"), "int app_entry(void)\n{\n    return 1;\n}\n");
        let mut s = Session::start(&nowhere, &[], &[("HOME", h), ("CLAUDE_PROJECT_DIR", inside.to_str().unwrap())]);
        let r = s.call("search", json!({"queries": ["app_entry"]}));
        assert_eq!(r["isError"], false, "{how}: {r}");
        assert!(inside.join(".codefast").is_dir() && !home.join(".codefast").exists());
    }
    // A home directory that is a repository, or was indexed on purpose, is a choice and is honoured.
    for marker in [".git", ".codefast"] {
        let home = scratch("chosen-home");
        std::fs::create_dir_all(home.join(marker)).unwrap();
        write(&home.join("dot.c"), "int dotfiles_fn(void)\n{\n    return 1;\n}\n");
        let h = home.to_str().unwrap();
        let mut s = Session::start(&nowhere, &[], &[("HOME", h), ("CLAUDE_PROJECT_DIR", h)]);
        let r = s.call("search", json!({"queries": ["dotfiles_fn"]}));
        assert_eq!(r["isError"], false, "{marker}: {r}");
    }
}
