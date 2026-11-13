//! `codefast hook` against Claude Code's hook payloads: what each event adds to the agent's
//! context, when it stays silent, and that the reply is exactly the documented output shape.

#[path = "packaging/payloads/mod.rs"]
mod payloads;
#[path = "packaging/support.rs"]
mod support;

use payloads::*;
use support::*;

const DEV_READ: &str =
    "codefast: dev_read is defined at src/io.c:4 int dev_read(struct dev *d, char *buf, int count)\nRead up to count bytes from the device\ncalled by: copy_all";

#[test]
fn a_grep_for_a_symbol_gets_its_definition_and_callers_once_per_session() {
    let (repo, home) = indexed("grep");
    let input = payload("claude-post-bash-grep", &repo.dir, &home);
    let first = hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[]);
    let note = first.context("PostToolUse");
    assert_eq!(note, DEV_READ);
    assert!(note.len() <= 420, "about 150 tokens at most: {} bytes", note.len());

    // The same symbol is not explained twice in one session, whatever the command.
    hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[]).assert_quiet("second identical search");
    let again = input.replace("grep -rn \\\"dev_read\\\" src/ --include=\\\"*.c\\\" | head -20", "rg -n dev_read");
    assert_ne!(again, input);
    hook(&home, &repo.dir, "post-tool-use", again.as_bytes(), &[]).assert_quiet("same symbol, other command");

    // Another session starts from nothing.
    let other = in_session(&input, "another-session");
    assert_eq!(hook(&home, &repo.dir, "post-tool-use", other.as_bytes(), &[]).context("PostToolUse"), DEV_READ);
    // A subagent has a context of its own: it is told once too, and no more than once.
    let mut explore: serde_json::Value = serde_json::from_str(&input).unwrap();
    explore["agent_id"] = serde_json::json!("a1b2c3");
    explore["agent_type"] = serde_json::json!("Explore");
    assert_eq!(hook(&home, &repo.dir, "post-tool-use", explore.to_string().as_bytes(), &[]).context("PostToolUse"), DEV_READ);
    hook(&home, &repo.dir, "post-tool-use", explore.to_string().as_bytes(), &[]).assert_quiet("the same subagent again");
    hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[]).assert_quiet("the main conversation still knows");

    let files = session_files(&repo);
    assert_eq!(files.len(), 3, "one state file per session and subagent: {files:?}");
    assert!(files.iter().all(|f| f.len() == 16 && f.bytes().all(|b| b.is_ascii_hexdigit())), "{files:?}");
}

#[test]
fn hooks_that_fire_together_say_it_once_between_them() {
    // Claude Code runs the PostToolUse hooks of parallel tool calls concurrently, and Codex
    // starts all matching hooks at once: twenty searches for one symbol are still one note.
    let (repo, home) = indexed("together");
    let search = payload("claude-post-bash-grep", &repo.dir, &home);
    let read = payload("claude-pre-read", &repo.dir, &home);
    for round in 0..6 {
        let session = format!("together-{round}");
        let ran = hooks_at_once(&home, &repo.dir, "post-tool-use", in_session(&search, &session).as_bytes(), 20);
        for r in &ran {
            assert_eq!((r.code, r.stderr.as_str()), (0, ""), "round {round}");
        }
        let said: Vec<&Ran> = ran.iter().filter(|r| !r.stdout.is_empty()).collect();
        assert_eq!(said.len(), 1, "round {round}: {} of 20 simultaneous hooks answered", said.len());
        assert_eq!(said[0].context("PostToolUse"), DEV_READ);

        let ran = hooks_at_once(&home, &repo.dir, "pre-tool-use", in_session(&read, &session).as_bytes(), 20);
        let said: Vec<&Ran> = ran.iter().filter(|r| !r.stdout.is_empty()).collect();
        assert_eq!(said.len(), 1, "round {round}: {} of 20 simultaneous read hints", said.len());
        assert!(said[0].context("PreToolUse").starts_with("src/big.c has 2,250 lines"));

        // The session remembers both afterwards.
        hook(&home, &repo.dir, "post-tool-use", in_session(&search, &session).as_bytes(), &[]).assert_quiet("after the burst");
        hook(&home, &repo.dir, "pre-tool-use", in_session(&read, &session).as_bytes(), &[]).assert_quiet("after the burst");
    }
    assert_eq!(session_files(&repo).len(), 6, "one state file per session");
}

#[test]
fn the_search_tools_are_understood_like_the_shell() {
    let (repo, home) = indexed("tools");
    let grep = hook(&home, &repo.dir, "post-tool-use", payload("claude-post-grep-tool", &repo.dir, &home).as_bytes(), &[]);
    assert_eq!(
        grep.context("PostToolUse"),
        "codefast: dev_write is defined at src/io.c:11 int dev_write(struct dev *d, const char *buf, int count)\ncalled by: copy_all"
    );
    let glob = hook(&home, &repo.dir, "post-tool-use", payload("claude-post-glob-tool", &repo.dir, &home).as_bytes(), &[]);
    let note = glob.context("PostToolUse");
    assert!(
        note.starts_with("codefast: copy_all is defined at src/main.c:3 static int copy_all(struct dev *in, struct dev *out)\ncalled by: main"),
        "{note}"
    );
}

#[test]
fn a_failed_search_is_answered_under_its_own_event_name() {
    let (repo, home) = indexed("failure");
    // `grep` exits 1 when nothing matches, which Claude Code reports as PostToolUseFailure. The
    // symbol was searched in the wrong directory: the answer says where it really is.
    let ran = hook(&home, &repo.dir, "post-tool-use", payload("claude-post-failure-bash-grep", &repo.dir, &home).as_bytes(), &[]);
    let note = ran.context("PostToolUseFailure");
    assert!(note.starts_with("codefast: log_msg is defined at src/io.h:9 void log_msg(const char *fmt, ...); [decl]\ncalled by: dev_write"), "{note}");
}

#[test]
fn a_whole_read_of_a_long_file_gets_one_line_about_its_outline() {
    let (repo, home) = indexed("read");
    let input = payload("claude-pre-read", &repo.dir, &home);
    let ran = hook(&home, &repo.dir, "pre-tool-use", input.as_bytes(), &[]);
    let hint = ran.context("PreToolUse");
    let kb = std::fs::metadata(repo.path("src/big.c")).unwrap().len().div_ceil(1024);
    assert_eq!(
        hint,
        format!(
            "src/big.c has 2,250 lines ({kb} KB): `codefast show src/big.c` prints its outline (450 symbols with line ranges), and `codefast show NAME` one symbol's body."
        )
    );
    assert_eq!(hint.lines().count(), 1);
    // The command in the hint is real: it prints the outline.
    let (code, outline, _) = run(&home, &repo.dir, &["show", "src/big.c"], &[], None);
    assert!(code == 0 && outline.starts_with("src/big.c (2,250 lines, 450 symbols)"), "{outline}");

    hook(&home, &repo.dir, "pre-tool-use", input.as_bytes(), &[]).assert_quiet("the same file again");
    // `cat` of the same file in the same session is the same read.
    let cat = payload("claude-pre-bash-cat", &repo.dir, &home);
    hook(&home, &repo.dir, "pre-tool-use", cat.as_bytes(), &[]).assert_quiet("cat of a file already hinted");
    let hint = hook(&home, &repo.dir, "pre-tool-use", in_session(&cat, "cat-session").as_bytes(), &[]).context("PreToolUse");
    assert!(hint.starts_with("src/big.c has 2,250 lines"), "{hint}");
}

#[test]
fn reads_that_need_no_hint_get_none() {
    let (repo, home) = indexed("quiet-reads");
    let whole = payload("claude-pre-read", &repo.dir, &home);
    let cases = [
        ("a line range", payload("claude-pre-read-range", &repo.dir, &home)),
        ("a short file", whole.replace("src/big.c", "src/io.c")),
        ("a file that does not exist", whole.replace("src/big.c", "src/missing.c")),
        ("a file outside the repository", whole.replace(&format!("{}/src/big.c", repo.str()), "/etc/hosts")),
        ("a command that reads nothing", payload("claude-pre-bash-other", &repo.dir, &home)),
        ("a piped cat", payload("claude-pre-bash-cat", &repo.dir, &home).replace("cat big.c", "cat big.c | head -40")),
    ];
    for (what, input) in &cases {
        hook(&home, &repo.dir, "pre-tool-use", input.as_bytes(), &[]).assert_quiet(what);
    }
    assert_eq!(session_files(&repo), [] as [&str; 0], "nothing said, nothing recorded");
    // A long file without symbols has no outline to offer.
    write(&repo.path("notes.txt"), &"one line of prose\n".repeat(900));
    let (code, _, _) = run(&home, &repo.dir, &["index"], &[], None);
    assert_eq!(code, 0);
    hook(&home, &repo.dir, "pre-tool-use", whole.replace("src/big.c", "notes.txt").as_bytes(), &[]).assert_quiet("prose");
}

#[test]
fn session_start_reports_the_index() {
    let (repo, home) = indexed("start");
    let ran = hook(&home, &repo.dir, "session-start", payload("claude-session-start", &repo.dir, &home).as_bytes(), &[]);
    let note = ran.context("SessionStart");
    // One line: what the tool can do is in its skill and its tool list, not repeated here.
    assert_eq!(note.lines().count(), 1, "{note}");
    assert!(note.starts_with("codefast has an index of this repository (5 files, "), "{note}");
    assert!(note.ends_with("symbols): `codefast search NAME` prints a symbol's definition, its body and every place that uses it in one call."), "{note}");
    assert!(ran.elapsed.as_secs() < 5, "session-start returned after {:?}", ran.elapsed);
    settle(&repo);
}

#[test]
fn session_start_builds_a_missing_index_without_waiting_for_it() {
    let repo = Repo::new("first-run");
    let home = scratch("home");
    assert!(!index_dir(&repo).exists());
    let ran = hook(&home, &repo.dir, "session-start", payload("claude-session-start", &repo.dir, &home).as_bytes(), &[]);
    let note = ran.context("SessionStart");
    assert_eq!(note.lines().count(), 1, "{note}");
    assert!(
        note.starts_with("codefast has an index of this repository (") || note.starts_with("codefast is indexing this repository in the background"),
        "{note}"
    );
    assert!(ran.elapsed.as_millis() < 3000, "session-start must not wait for the build: {:?}", ran.elapsed);
    // The detached build finishes on its own, and the index it leaves is the real one.
    settle(&repo);
    assert!(index_dir(&repo).join(".gitignore").exists(), "the index directory ignores itself");
    let search = hook(&home, &repo.dir, "post-tool-use", payload("claude-post-bash-grep", &repo.dir, &home).as_bytes(), &[]);
    assert_eq!(search.context("PostToolUse"), DEV_READ);
}

#[test]
fn nothing_is_indexed_outside_a_repository() {
    let home = scratch("home");
    let plain = scratch("not-a-repo");
    write(&plain.join("a.c"), "int f(void) { return 0; }\n");
    let input = payload("claude-session-start", &plain, &home);
    hook(&home, &plain, "session-start", input.as_bytes(), &[]).assert_quiet("a directory that is not a work tree");
    // A home directory is never indexed, even when it is a git work tree.
    std::fs::create_dir_all(home.join(".git")).unwrap();
    hook(&home, &home, "session-start", payload("claude-session-start", &home, &home).as_bytes(), &[]).assert_quiet("a home directory");
    std::thread::sleep(std::time::Duration::from_millis(400));
    assert!(!plain.join(".codefast").exists() && !home.join(".codefast").exists());
}

#[test]
fn the_switch_turns_all_three_off() {
    let repo = Repo::new("off");
    let home = scratch("home");
    for value in ["off", "OFF", "0", "false"] {
        let off = [("CODEFAST_HOOKS", value)];
        hook(&home, &repo.dir, "session-start", payload("claude-session-start", &repo.dir, &home).as_bytes(), &off).assert_quiet("session-start");
        hook(&home, &repo.dir, "post-tool-use", payload("claude-post-bash-grep", &repo.dir, &home).as_bytes(), &off).assert_quiet("post-tool-use");
        hook(&home, &repo.dir, "pre-tool-use", payload("claude-pre-read", &repo.dir, &home).as_bytes(), &off).assert_quiet("pre-tool-use");
    }
    std::thread::sleep(std::time::Duration::from_millis(400));
    assert!(!index_dir(&repo).exists(), "a disabled session-start builds nothing");
    // Any other value leaves them on.
    let on = hook(&home, &repo.dir, "session-start", payload("claude-session-start", &repo.dir, &home).as_bytes(), &[("CODEFAST_HOOKS", "on")]);
    on.context("SessionStart");
    settle(&repo);
}

#[test]
fn the_payload_names_the_repository_not_the_process() {
    let (repo, home) = indexed("cwd");
    let elsewhere = scratch("elsewhere");
    let input = payload("claude-post-bash-grep", &repo.dir, &home);
    // Plugin hooks may run in the plugin's directory: the payload's `cwd` is what counts.
    assert_eq!(hook(&home, &elsewhere, "post-tool-use", input.as_bytes(), &[]).context("PostToolUse"), DEV_READ);
    // A session working in a subdirectory still means the whole work tree.
    let sub = in_session(&input.replace(&format!("\"cwd\": \"{}\"", repo.str()), &format!("\"cwd\": \"{}/src\"", repo.str())), "sub")
        .replace("src/ --include", ". --include");
    assert_eq!(hook(&home, &elsewhere, "post-tool-use", sub.as_bytes(), &[]).context("PostToolUse"), DEV_READ);
    assert!(!repo.path("src/.codefast").exists() && !elsewhere.join(".codefast").exists());
    // An explicit root wins over the payload.
    let other = Repo::new("cwd-other");
    let (code, _, _) = run(&home, &other.dir, &["index"], &[], None);
    assert_eq!(code, 0);
    let marker = in_session(&input.replace("dev_read", &other.marker), "explicit");
    let ran = hook(&home, &elsewhere, "post-tool-use", marker.as_bytes(), &[("CODEFAST_ROOT", other.str())]);
    assert!(ran.context("PostToolUse").starts_with(&format!("codefast: {0} is defined at lib/marker.c:1 int {0}(int x)", other.marker)));
}

#[test]
fn a_file_edited_since_indexing_is_never_quoted_from_the_old_index() {
    let (repo, home) = indexed("stale");
    let source = std::fs::read_to_string(repo.path("src/io.c")).unwrap();
    write(&repo.path("src/io.c"), &format!("/* moved */\n/* down */\n{source}"));
    let input = payload("claude-post-bash-grep", &repo.dir, &home);
    // Exact or silent: the indexed line number is wrong now, so nothing is said...
    hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[]).assert_quiet("a stale definition");
    assert_eq!(session_files(&repo), [] as [&str; 0]);
    // ...and a refresh is started behind the scenes, after which the answer is the new line.
    settle(&repo);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let ran = hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[]);
        if !ran.stdout.is_empty() {
            assert!(ran.context("PostToolUse").starts_with("codefast: dev_read is defined at src/io.c:6 int dev_read("), "{}", ran.stdout);
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the index was not refreshed");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn the_caller_summary_never_holds_the_answer_back() {
    let (repo, home) = indexed("budget");
    let input = payload("claude-post-bash-grep", &repo.dir, &home);
    // With no time for the summary, the definition still goes out, with a pointer instead.
    let ran = hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[("CODEFAST_HOOK_BUDGET_MS", "0")]);
    assert_eq!(
        ran.context("PostToolUse"),
        "codefast: dev_read is defined at src/io.c:4 int dev_read(struct dev *d, char *buf, int count)\nRead up to count bytes from the device\nmentioned in 3 files; `codefast callers dev_read` lists its call sites"
    );
    // A type has no callers to summarise: the pointer is to its uses.
    let ops = in_session(&input.replace("\\\"dev_read\\\" src/", "'struct dev_ops {' src/"), "types");
    assert_eq!(
        hook(&home, &repo.dir, "post-tool-use", ops.as_bytes(), &[]).context("PostToolUse"),
        "codefast: dev_ops is defined at src/io.h:2 struct dev_ops\nmentioned in 1 file; `codefast refs dev_ops` lists its uses"
    );
}

#[test]
fn without_codefast_on_the_path_the_note_names_the_binary() {
    let (repo, home) = indexed("path");
    let input = payload("claude-post-bash-grep", &repo.dir, &home);
    // With no time for the caller summary the note points at the command that has it.
    let note = hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[("PATH", "/usr/bin:/bin"), ("CODEFAST_HOOK_BUDGET_MS", "0")]).context("PostToolUse");
    assert!(note.ends_with(&format!("`{BIN} callers dev_read` lists its call sites")), "{note}");

    // Run from the Claude Code plugin, the hook's own PATH lacks the command, but the agent's
    // shell has the plugin's bin/ on it: the bare name is the right one to print.
    let plugin = scratch("plugin");
    write(&plugin.join("bin/codefast"), "#!/bin/sh\n");
    let from_plugin = [("PATH", "/usr/bin:/bin"), ("CLAUDE_PLUGIN_ROOT", plugin.to_str().unwrap()), ("CODEFAST_HOOK_BUDGET_MS", "0")];
    let note = hook(&home, &repo.dir, "post-tool-use", in_session(&input, "plugin").as_bytes(), &from_plugin).context("PostToolUse");
    assert!(note.ends_with("`codefast callers dev_read` lists its call sites"), "{note}");
    // Codex sets the same variable next to its own and puts nothing on PATH.
    let from_codex =
        [("PATH", "/usr/bin:/bin"), ("CLAUDE_PLUGIN_ROOT", plugin.to_str().unwrap()), ("PLUGIN_ROOT", plugin.to_str().unwrap()), ("CODEFAST_HOOK_BUDGET_MS", "0")];
    let note = hook(&home, &repo.dir, "post-tool-use", in_session(&input, "codex-plugin").as_bytes(), &from_codex).context("PostToolUse");
    assert!(note.contains(&format!("`{BIN} callers dev_read`")), "{note}");
}
