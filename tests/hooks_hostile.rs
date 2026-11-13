//! `codefast hook` must never disturb the harness that runs it. Whatever arrives on stdin and
//! whatever state the repository is in, the process exits 0, writes nothing to stderr, and
//! prints either nothing or one well-formed reply.

#[path = "packaging/payloads/mod.rs"]
mod payloads;
#[path = "packaging/support.rs"]
mod support;

use payloads::*;
use serde_json::{Value, json};
use std::time::Duration;
use support::*;

const EVENTS: [&str; 3] = ["session-start", "pre-tool-use", "post-tool-use"];

/// A valid search payload with one member replaced.
fn search_with(repo: &Repo, home: &Dir, key: &str, value: Value) -> String {
    let mut v: Value = serde_json::from_str(&payload("claude-post-bash-grep", &repo.dir, home)).unwrap();
    v[key] = value;
    v.to_string()
}

#[test]
fn input_that_is_not_a_payload_is_ignored() {
    let (repo, home) = indexed("garbage");
    let truncated = payload("hostile-truncated", &repo.dir, &home);
    let inputs: Vec<(&str, Vec<u8>)> = vec![
        ("truncated JSON", truncated.into_bytes()),
        ("not JSON", b"grep -rn dev_read src/".to_vec()),
        ("an array", b"[{\"tool_name\": \"Bash\"}]".to_vec()),
        ("a string", b"\"PostToolUse\"".to_vec()),
        ("null", b"null".to_vec()),
        ("a number", b"42".to_vec()),
        ("single quotes", b"{'tool_name': 'Bash'}".to_vec()),
        ("NUL bytes", vec![0u8; 4096]),
        ("invalid UTF-8", b"{\"cwd\": \"\xff\xfe\xfd\", \"tool_name\": \"Bash\"}".to_vec()),
        ("binary", (0u8..=255).cycle().take(100_000).collect()),
        ("two objects", b"{\"tool_name\": \"Bash\"}{\"tool_name\": \"Bash\"}".to_vec()),
        ("an unclosed string", b"{\"tool_name\": \"Bash".to_vec()),
        ("only braces", "{".repeat(100_000).into_bytes()),
    ];
    for event in EVENTS {
        for (what, input) in &inputs {
            hook(&home, &repo.dir, event, input, &[]).assert_quiet(&format!("{event}: {what}"));
        }
    }
    // An empty object is a payload, just one with nothing to act on for the tool events.
    for event in ["pre-tool-use", "post-tool-use"] {
        for input in ["{}", "", "  \n", "{\"tool_name\": 7, \"tool_input\": \"grep\"}", "{\"tool_name\": \"Bash\", \"tool_input\": {\"command\": null}}"] {
            hook(&home, &repo.dir, event, input.as_bytes(), &[]).assert_quiet(&format!("{event}: {input:?}"));
        }
    }
    assert_eq!(session_files(&repo), [] as [&str; 0]);
}

#[test]
fn an_enormous_payload_is_stepped_over_not_parsed() {
    let (repo, home) = indexed("enormous");
    // 40 MB of tool output in front of the fields that matter: still answered, and quickly.
    let mut rest: Value = serde_json::from_str(&payload("claude-post-bash-grep", &repo.dir, &home)).unwrap();
    rest.as_object_mut().unwrap().remove("tool_response");
    let big = format!("{{\"tool_response\": {{\"stdout\": \"{}\", \"stderr\": \"\"}}, {}", "x".repeat(40 << 20), &rest.to_string()[1..]);
    assert!(big.len() > 40 << 20 && serde_json::from_str::<Value>(&big).is_ok());
    let ran = hook(&home, &repo.dir, "post-tool-use", big.as_bytes(), &[]);
    assert!(ran.context("PostToolUse").starts_with("codefast: dev_read is defined at src/io.c:4 "));
    assert!(ran.elapsed < Duration::from_secs(10), "40 MB took {:?}", ran.elapsed);

    // Beyond 64 MB nothing is looked at.
    let huge = search_with(&repo, &home, "session_id", json!("huge")).replacen('{', &format!("{{\"padding\": \"{}\", ", "y".repeat(65 << 20)), 1);
    hook(&home, &repo.dir, "post-tool-use", huge.as_bytes(), &[]).assert_quiet("more than 64 MB");

    // A megabyte of command line, a million nested arrays, a tool input the size of a file.
    let long_command = search_with(&repo, &home, "tool_input", json!({"command": format!("grep -rn dev_read {}", "src/ ".repeat(200_000))}));
    hook(&home, &repo.dir, "post-tool-use", in_session(&long_command, "long").as_bytes(), &[]).assert_quiet("a megabyte of command");
    let deep =
        search_with(&repo, &home, "session_id", json!("deep")).replacen('{', &format!("{{\"deep\": {}{}, ", "[".repeat(1_000_000), "]".repeat(1_000_000)), 1);
    let ran = hook(&home, &repo.dir, "post-tool-use", deep.as_bytes(), &[]);
    assert!(ran.context("PostToolUse").contains("dev_read"), "nesting outside the wanted fields does not matter");
    let deep_input =
        search_with(&repo, &home, "tool_input", serde_json::from_str(&format!("{{\"command\": {}{}}}", "[".repeat(100), "]".repeat(100))).unwrap());
    hook(&home, &repo.dir, "post-tool-use", in_session(&deep_input, "deep-input").as_bytes(), &[]).assert_quiet("a nested command");
}

#[test]
fn shell_metacharacters_are_read_never_run() {
    let (repo, home) = indexed("metachar");
    let input = payload("hostile-shell-metacharacters", &repo.dir, &home);
    hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[]).assert_quiet("expansions and separators around the pattern");
    hook(&home, &repo.dir, "pre-tool-use", input.as_bytes(), &[]).assert_quiet("the same command before it runs");
    let commands = [
        "cat $(touch PWNED-6) src/big.c",
        "cat `touch PWNED-7`",
        "cat src/big.c; touch PWNED-8",
        "cat 'src/big.c' && touch PWNED-9 || cat src/big.c",
        "cat <(touch PWNED-10)",
        "bash -c \"$(touch PWNED-11)\"",
        "eval 'touch PWNED-12'; grep -rn dev_read src",
        "grep -rn dev_read src\ntouch PWNED-13",
        "find . -name '*dev_read*' -exec touch PWNED-14 {} +",
        "find . -name dev_read -delete -o -exec sh -c 'touch PWNED-15' \\;",
    ];
    for (i, command) in commands.iter().enumerate() {
        for (event, name) in [("pre-tool-use", "PreToolUse"), ("post-tool-use", "PostToolUse")] {
            let mut v: Value = serde_json::from_str(&search_with(&repo, &home, "tool_input", json!({"command": command}))).unwrap();
            v["hook_event_name"] = json!(name);
            v["session_id"] = json!(format!("m{i}"));
            let ran = hook(&home, &repo.dir, event, v.to_string().as_bytes(), &[]);
            assert_eq!((ran.code, ran.stderr.as_str()), (0, ""), "{event}: {command}");
            if !ran.stdout.is_empty() {
                ran.context(name);
            }
        }
    }
    std::thread::sleep(Duration::from_millis(200));
    let created: Vec<String> =
        std::fs::read_dir(&*repo.dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("PWNED")).collect();
    assert_eq!(created, [] as [&str; 0], "a hook reads commands, it never runs them");
    assert!(repo.path("src/big.c").exists());
}

#[test]
fn regex_characters_in_the_pattern_are_never_a_symbol() {
    let (repo, home) = indexed("regex");
    hook(&home, &repo.dir, "post-tool-use", payload("hostile-regex-symbol", &repo.dir, &home).as_bytes(), &[]).assert_quiet("a pattern full of operators");
    let silent = [
        "dev_read.*",
        "dev_(read|write)",
        "dev_read|dev_write",
        "[d]ev_read",
        "dev_rea.",
        "dev_read+",
        "dev_read{1,3}",
        "(dev_read)",
        "dev_read\\d",
        "dev_read\\",
        "\\",
        "(((",
        "*",
        ".*",
        "",
        " ",
    ];
    for (i, pattern) in silent.iter().enumerate() {
        let grep = in_session(&search_with(&repo, &home, "tool_input", json!({"command": format!("grep -rnE '{pattern}' src")})), &format!("r{i}"));
        hook(&home, &repo.dir, "post-tool-use", grep.as_bytes(), &[]).assert_quiet(pattern);
        let mut tool: Value = serde_json::from_str(&payload("claude-post-grep-tool", &repo.dir, &home)).unwrap();
        tool["tool_input"]["pattern"] = json!(pattern);
        hook(&home, &repo.dir, "post-tool-use", in_session(&tool.to_string(), &format!("t{i}")).as_bytes(), &[]).assert_quiet(pattern);
    }
    // The decorations that only anchor one identifier are seen through.
    let answered = ["\\bdev_read\\b", "^dev_read\\(", "dev_read\\s*\\(", "\\<dev_read\\>", "int\\s+dev_read", "dev_read("];
    for (i, pattern) in answered.iter().enumerate() {
        let mut tool: Value = serde_json::from_str(&payload("claude-post-grep-tool", &repo.dir, &home)).unwrap();
        tool["tool_input"]["pattern"] = json!(pattern);
        let ran = hook(&home, &repo.dir, "post-tool-use", in_session(&tool.to_string(), &format!("a{i}")).as_bytes(), &[]);
        if *pattern == "int\\s+dev_read" {
            ran.assert_quiet("a type in front is not a kind word");
        } else {
            assert!(ran.context("PostToolUse").starts_with("codefast: dev_read is defined at src/io.c:4 "), "{pattern}");
        }
    }
}

#[test]
fn the_session_id_never_chooses_a_path() {
    let (repo, home) = indexed("session-id");
    let ran = hook(&home, &repo.dir, "post-tool-use", payload("hostile-session-id", &repo.dir, &home).as_bytes(), &[]);
    assert!(ran.context("PostToolUse").contains("dev_read"));
    let files = session_files(&repo);
    assert!(files.len() == 1 && files[0].len() == 16 && files[0].bytes().all(|b| b.is_ascii_hexdigit()), "{files:?}");
    let parent = repo.dir.parent().unwrap();
    assert!(!parent.join("outside").exists() && !repo.path("outside").exists() && !index_dir(&repo).join("outside").exists());
    // Ids of any shape work, and each is its own session.
    for (i, id) in [json!(""), json!("a/b"), json!("..\\..\\x"), json!("x".repeat(100_000)), json!(17), json!(null), json!(["a"])].into_iter().enumerate() {
        let input = search_with(&repo, &home, "session_id", id).replace("dev_read", &format!("big_fn_{i}"));
        let ran = hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[]);
        assert!(ran.context("PostToolUse").contains(&format!("big_fn_{i} is defined at src/big.c:")), "session id #{i}");
    }
    assert!(session_files(&repo).iter().all(|f| f == "anonymous" || (f.len() == 16 && f.bytes().all(|b| b.is_ascii_hexdigit()))), "{:?}", session_files(&repo));
}

#[cfg(unix)]
#[test]
fn an_unreadable_repository_is_left_alone() {
    use std::os::unix::fs::PermissionsExt;
    struct Restore(std::path::PathBuf, u32);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(self.1));
        }
    }
    let lock = |path: std::path::PathBuf, mode: u32| {
        let old = std::fs::metadata(&path).unwrap().permissions().mode();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        Restore(path, old)
    };
    let (repo, home) = indexed("unreadable");
    let elsewhere = scratch("elsewhere");
    let inputs = [("session-start", "claude-session-start"), ("pre-tool-use", "claude-pre-read"), ("post-tool-use", "claude-post-bash-grep")];
    {
        // The index cannot be read.
        let _guard = lock(index_dir(&repo).join("base.cfseg"), 0o000);
        for (event, name) in &inputs[1..] {
            hook(&home, &repo.dir, event, payload(name, &repo.dir, &home).as_bytes(), &[]).assert_quiet(&format!("{event}: unreadable index"));
        }
    }
    {
        // The index directory cannot be written: nothing can be remembered, so nothing is said,
        // rather than the same note on every call.
        let _guard = lock(index_dir(&repo), 0o555);
        for _ in 0..2 {
            hook(&home, &repo.dir, "post-tool-use", payload("claude-post-bash-grep", &repo.dir, &home).as_bytes(), &[])
                .assert_quiet("read-only index directory");
            hook(&home, &repo.dir, "pre-tool-use", payload("claude-pre-read", &repo.dir, &home).as_bytes(), &[]).assert_quiet("read-only index directory");
        }
    }
    {
        // The whole repository is closed.
        let _guard = lock(repo.dir.to_path_buf(), 0o000);
        for (event, name) in &inputs {
            hook(&home, &elsewhere, event, payload(name, &repo.dir, &home).as_bytes(), &[]).assert_quiet(&format!("{event}: unreadable repository"));
        }
    }
    {
        // A source file is closed: its definition cannot be checked against the disk.
        let _guard = lock(repo.path("src/io.c"), 0o000);
        let ran = hook(&home, &repo.dir, "post-tool-use", payload("claude-post-bash-grep", &repo.dir, &home).as_bytes(), &[]);
        assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    }
    // A repository that is gone.
    let gone = payload("claude-post-bash-grep", &repo.dir.join("no/such/dir"), &home);
    for event in EVENTS {
        hook(&home, &elsewhere, event, gone.as_bytes(), &[]).assert_quiet(&format!("{event}: missing directory"));
    }
    // A corrupt index.
    std::fs::write(index_dir(&repo).join("base.cfseg"), b"CFSEG\0\0\x01 not really").unwrap();
    for (event, name) in &inputs[1..] {
        hook(&home, &repo.dir, event, payload(name, &repo.dir, &home).as_bytes(), &[]).assert_quiet(&format!("{event}: corrupt index"));
    }
}

#[test]
fn file_names_are_quoted_or_skipped() {
    let repo = Repo::new("names");
    let home = scratch("home");
    let body: String = (0..500).map(|i| format!("int odd_fn_{i}(int x)\n{{\n    return x + {i};\n}}\n")).collect();
    write(&repo.path("src/we ird;name$(touch PWNED) it's.c"), &body);
    write(&repo.path("src/new\nline.c"), &body);
    let (code, _, _) = run(&home, &repo.dir, &["index"], &[], None);
    assert_eq!(code, 0);
    let read = |name: &str, session: &str| {
        let mut v: Value = serde_json::from_str(&payload("claude-pre-read", &repo.dir, &home)).unwrap();
        v["tool_input"]["file_path"] = json!(repo.path(name).to_str().unwrap());
        hook(&home, &repo.dir, "pre-tool-use", in_session(&v.to_string(), session).as_bytes(), &[])
    };
    let hint = read("src/we ird;name$(touch PWNED) it's.c", "odd").context("PreToolUse");
    // Pasted into a shell, the command in the hint names exactly that file.
    assert!(hint.contains("`codefast show 'src/we ird;name$(touch PWNED) it'\\''s.c'` prints its outline"), "{hint}");
    assert_eq!(hint.lines().count(), 1);
    read("src/new\nline.c", "newline").assert_quiet("a file name that cannot be written on one line");
    assert!(!repo.path("PWNED").exists());
}

#[test]
fn events_and_payloads_that_do_not_belong_are_ignored() {
    let (repo, home) = indexed("mismatch");
    let search = payload("claude-post-bash-grep", &repo.dir, &home);
    let read = payload("claude-pre-read", &repo.dir, &home);
    for event in ["stop", "Notification", "user-prompt-submit", "../../etc/passwd", "session-start; touch PWNED", "", "\u{0}"] {
        if event.contains('\0') {
            continue; // not passable as an argument
        }
        hook(&home, &repo.dir, event, search.as_bytes(), &[]).assert_quiet(&format!("event {event:?}"));
    }
    // A payload of one event handed to the command of another.
    hook(&home, &repo.dir, "pre-tool-use", search.as_bytes(), &[]).assert_quiet("a PostToolUse payload at pre-tool-use");
    hook(&home, &repo.dir, "post-tool-use", read.as_bytes(), &[]).assert_quiet("a PreToolUse payload at post-tool-use");
    hook(&home, &repo.dir, "post-tool-use", search.replace("\"PostToolUse\"", "\"Stop\"").as_bytes(), &[]).assert_quiet("another event's name");
    // Tools the hooks have nothing to say about.
    for tool in ["Write", "Edit", "WebFetch", "Agent", "mcp__codefast__search", "PowerShell"] {
        hook(&home, &repo.dir, "post-tool-use", in_session(&search.replace("\"Bash\"", &format!("\"{tool}\"")), tool).as_bytes(), &[]).assert_quiet(tool);
    }
    assert!(!repo.path("PWNED").exists());
    assert_eq!(session_files(&repo), [] as [&str; 0]);
}

#[test]
fn a_damaged_index_never_reaches_the_harness_as_an_error() {
    let (repo, home) = indexed("damaged");
    let base = index_dir(&repo).join("base.cfseg");
    let good = std::fs::read(&base).unwrap();
    let inputs = [("pre-tool-use", "claude-pre-read", "PreToolUse"), ("post-tool-use", "claude-post-bash-grep", "PostToolUse")];
    // Cut short at every eighth of its length, and with stretches of its body overwritten: the
    // header still says "index", the contents no longer add up.
    let mut damaged: Vec<Vec<u8>> = (1..8).map(|k| good[..good.len() * k / 8].to_vec()).collect();
    for k in 1..8 {
        let mut bytes = good.clone();
        let (from, len) = (good.len() * k / 8, good.len() / 16);
        for b in &mut bytes[from..from + len] {
            *b = 0xff;
        }
        damaged.push(bytes);
    }
    for (n, bytes) in damaged.iter().enumerate() {
        std::fs::write(&base, bytes).unwrap();
        for (event, name, event_name) in inputs {
            let input = in_session(&payload(name, &repo.dir, &home), &format!("damaged-{n}"));
            let ran = hook(&home, &repo.dir, event, input.as_bytes(), &[]);
            assert_eq!((ran.code, ran.stderr.as_str()), (0, ""), "{event} on damaged index #{n}");
            if !ran.stdout.is_empty() {
                ran.context(event_name);
            }
        }
    }
}

#[test]
fn an_index_that_must_not_be_refreshed_is_not() {
    let (repo, home) = indexed("no-refresh");
    let source = std::fs::read_to_string(repo.path("src/io.c")).unwrap();
    write(&repo.path("src/io.c"), &format!("/* moved */\n{source}"));
    let frozen = [("CODEFAST_NO_REFRESH", "1")];
    let input = payload("claude-post-bash-grep", &repo.dir, &home);
    hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &frozen).assert_quiet("a stale definition");
    hook(&home, &repo.dir, "session-start", payload("claude-session-start", &repo.dir, &home).as_bytes(), &frozen).context("SessionStart");
    std::thread::sleep(Duration::from_millis(600));
    let listed: Vec<String> = std::fs::read_dir(index_dir(&repo)).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(listed.iter().all(|f| f == "base.cfseg" || f == ".gitignore"), "no overlay, no lock, no stamp: {listed:?}");
}
