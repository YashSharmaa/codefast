//! `codefast hook` against Codex's hook payloads, and against the two harnesses whose hook
//! protocols are only known from their documentation (Cursor, Gemini CLI).
//!
//! Codex has no file-read tool: it reads and searches through its shell, often wrapped in
//! `bash -lc`. Its output schemas are closed (`additionalProperties: false`), and a field it
//! does not support marks the hook run as failed, so the reply check in `Ran::context` is strict.

#[path = "packaging/payloads/mod.rs"]
mod payloads;
#[path = "packaging/support.rs"]
mod support;

use payloads::*;
use serde_json::{Value, json};

#[test]
fn a_search_through_bash_lc_is_answered() {
    let (repo, home) = indexed("codex-rg");
    let input = payload("codex-post-bash-rg", &repo.dir, &home);
    let ran = hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[]);
    assert_eq!(
        ran.context("PostToolUse"),
        "codefast: dev_read is defined at src/io.c:4 int dev_read(struct dev *d, char *buf, int count)\nRead up to count bytes from the device\ncalled by: copy_all"
    );
    hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[]).assert_quiet("once per session");
}

#[test]
fn a_command_given_as_an_argument_vector_is_read_too() {
    let (repo, home) = indexed("codex-argv");
    let mut input: Value = serde_json::from_str(&payload("codex-post-bash-rg", &repo.dir, &home)).unwrap();
    input["tool_input"] = json!({"command": ["bash", "-lc", "grep -rn 'dev_write(' ."], "workdir": repo.path("src").to_str().unwrap()});
    let ran = hook(&home, &repo.dir, "post-tool-use", input.to_string().as_bytes(), &[]);
    assert!(ran.context("PostToolUse").starts_with("codefast: dev_write is defined at src/io.c:11 int dev_write("));
}

#[test]
fn printing_a_long_file_gets_the_outline_hint() {
    let (repo, home) = indexed("codex-cat");
    let input = payload("codex-pre-bash-cat", &repo.dir, &home);
    let hint = hook(&home, &repo.dir, "pre-tool-use", input.as_bytes(), &[]).context("PreToolUse");
    assert!(
        hint.starts_with("src/big.c has 2,250 lines (") && hint.contains("`codefast show src/big.c` prints its outline (450 symbols with line ranges)"),
        "{hint}"
    );
    hook(&home, &repo.dir, "pre-tool-use", input.as_bytes(), &[]).assert_quiet("once per file per session");
    // Part of a file is not a whole-file read.
    for partial in ["sed -n '1,120p' src/big.c", "head -n 200 src/big.c", "nl -ba src/big.c | sed -n '400,460p'", "rg -n big_fn_7 src/big.c"] {
        let other = in_session(&input.replace("nl -ba src/big.c", partial), partial);
        hook(&home, &repo.dir, "pre-tool-use", other.as_bytes(), &[]).assert_quiet(partial);
    }
}

#[test]
fn session_start_is_the_same_note() {
    let (repo, home) = indexed("codex-start");
    let ran = hook(&home, &repo.dir, "session-start", payload("codex-session-start", &repo.dir, &home).as_bytes(), &[]);
    let note = ran.context("SessionStart");
    assert!(note.starts_with("codefast has an index of this repository (5 files, ") && note.lines().count() == 1, "{note}");
    settle(&repo);
}

#[test]
fn cursor_gets_its_own_flat_reply() {
    let (repo, home) = indexed("cursor");
    let ran = hook(&home, &repo.dir, "post-tool-use", payload("cursor-post-shell", &repo.dir, &home).as_bytes(), &[]);
    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    let reply: Value = serde_json::from_str(&ran.stdout).unwrap();
    assert_eq!(reply.as_object().unwrap().keys().collect::<Vec<_>>(), ["additional_context"], "{reply}");
    assert!(reply["additional_context"].as_str().unwrap().starts_with("codefast: dev_read is defined at src/io.c:4 "), "{reply}");

    let start = hook(&home, &repo.dir, "session-start", payload("cursor-session-start", &repo.dir, &home).as_bytes(), &[]);
    let reply: Value = serde_json::from_str(&start.stdout).unwrap();
    assert!(reply["additional_context"].as_str().unwrap().starts_with("codefast has an index of this repository"), "{reply}");
    assert_eq!(reply.as_object().unwrap().len(), 1);

    // Cursor's preToolUse can only allow or deny: there is nothing to add, so nothing is said.
    let mut read: Value = serde_json::from_str(&payload("cursor-post-shell", &repo.dir, &home)).unwrap();
    read["hook_event_name"] = json!("preToolUse");
    read["tool_name"] = json!("Read");
    read["tool_input"] = json!({"file_path": repo.path("src/big.c").to_str().unwrap()});
    hook(&home, &repo.dir, "pre-tool-use", read.to_string().as_bytes(), &[]).assert_quiet("cursor preToolUse");
    settle(&repo);
}

#[test]
fn gemini_cli_after_tool_is_answered_under_its_own_name() {
    let (repo, home) = indexed("gemini");
    let ran = hook(&home, &repo.dir, "post-tool-use", payload("gemini-after-tool", &repo.dir, &home).as_bytes(), &[]);
    assert!(ran.context("AfterTool").starts_with("codefast: dev_read is defined at src/io.c:4 "));
    // The hooks file may also name the event the way Gemini CLI does.
    let other = in_session(&payload("gemini-after-tool", &repo.dir, &home), "second");
    assert!(hook(&home, &repo.dir, "AfterTool", other.as_bytes(), &[]).context("AfterTool").contains("dev_read"));
}
