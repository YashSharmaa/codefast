//! The hooks that answer before they are asked: a card for a symbol the prompt names, the uses
//! an edit left behind, and the list of changed definitions after a compaction. Each of them
//! speaks only when it has a fact, and says it once.

#[path = "packaging/payloads/mod.rs"]
mod payloads;
#[path = "packaging/support.rs"]
mod support;

use payloads::*;
use support::*;
use std::process::Command;

fn git(repo: &Repo, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(&*repo.dir)
        .args(["-c", "user.name=test", "-c", "user.email=test@localhost", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// The test repository as a real git work tree with one commit, and its index.
fn committed(tag: &str) -> (Repo, Dir) {
    let repo = Repo::new(tag);
    std::fs::remove_dir_all(repo.path(".git")).unwrap();
    git(&repo, &["init", "-q", "."]);
    write(&repo.path(".gitignore"), ".codefast/\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    let home = scratch("home");
    let (code, out, err) = run(&home, &repo.dir, &["index"], &[], None);
    assert_eq!(code, 0, "codefast index: {out}{err}");
    (repo, home)
}

fn edit(repo: &Repo, rel: &str, from: &str, to: &str) {
    let path = repo.path(rel);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(from), "{rel} has no {from:?}");
    std::fs::write(&path, text.replace(from, to)).unwrap();
}

const CARD: &str = "codefast: dev_read is defined at src/io.c:4 int dev_read(struct dev *d, char *buf, int count)\n  Read up to count bytes from the device\n  called from 1 site in 1 file (`codefast callers dev_read` lists every one)";

#[test]
fn a_prompt_that_names_a_symbol_gets_its_card_once() {
    let (repo, home) = indexed("prompt");
    let input = payload("claude-user-prompt", &repo.dir, &home);
    let ran = hook(&home, &repo.dir, "user-prompt-submit", input.as_bytes(), &[]);
    let card = ran.context("UserPromptSubmit");
    assert_eq!(card, CARD);
    assert!(card.len() <= 420, "about 150 tokens at most: {} bytes", card.len());
    // Not again for the next prompt, and not again when the agent then greps for it.
    hook(&home, &repo.dir, "user-prompt-submit", input.as_bytes(), &[]).assert_quiet("the same symbol in a later prompt");
    hook(&home, &repo.dir, "post-tool-use", payload("claude-post-bash-grep", &repo.dir, &home).as_bytes(), &[]).assert_quiet("a grep for the symbol the card was about");
    // Codex sends the same event with its own fields.
    let codex = hook(&home, &repo.dir, "user-prompt-submit", payload("codex-user-prompt", &repo.dir, &home).as_bytes(), &[]);
    assert_eq!(codex.context("UserPromptSubmit"), CARD);
}

#[test]
fn a_prompt_gets_no_card_for_what_the_index_cannot_pin_down() {
    let (repo, home) = indexed("prompt-quiet");
    let ask = |session: &str, prompt: &str| {
        let mut v: serde_json::Value = serde_json::from_str(&payload("claude-user-prompt", &repo.dir, &home)).unwrap();
        v["prompt"] = prompt.into();
        v["session_id"] = session.into();
        hook(&home, &repo.dir, "user-prompt-submit", v.to_string().as_bytes(), &[])
    };
    // Prose, a name the repository does not have, and a word that is only a field.
    ask("a", "Please make the tests pass and stop when you are done. Add a note to the changelog.").assert_quiet("prose");
    ask("b", "Why is `parse_config_file` slow?").assert_quiet("a symbol this repository does not define");
    ask("c", "The read path is slow, fix it.").assert_quiet("a plain word that happens to be a field name");
    // Two cards at most, in the order the prompt names them. A static function has the file it lives in.
    let two = ask("d", "`dev_write` and `copy_all` and `dev_read` all need a timeout parameter.").context("UserPromptSubmit");
    assert_eq!(two.matches(" is defined at ").count(), 2, "{two}");
    assert!(two.starts_with("codefast: dev_write is defined at src/io.c:11 ") && two.contains("\ncopy_all is defined at src/main.c:3 "), "{two}");
    // A method named in words, with the type it belongs to.
    let said = ask("e", "Rename the field read of dev_ops to read_bytes everywhere.");
    assert!(said.stdout.is_empty() || said.context("UserPromptSubmit").contains("dev_ops.read is defined at src/io.h:3"), "{}", said.stdout);
}

#[test]
fn an_edit_that_renames_a_function_is_told_what_still_uses_the_old_name() {
    let (repo, home) = committed("edit-rename");
    edit(&repo, "src/main.c", "static int copy_all(struct dev *in", "static int copy_everything(struct dev *in");
    let input = payload("claude-post-edit", &repo.dir, &home);
    let ran = hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[]);
    assert_eq!(
        ran.context("PostToolUse"),
        "codefast: copy_all is no longer defined in src/main.c (new there: copy_everything). Still used at 1 site in 1 file:\nsrc/main.c: main:15"
    );
    // Said once: the next edit of the same file adds nothing, and neither does a shell command
    // that may have written files.
    hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[]).assert_quiet("the same finding again");
    let sed = payload("claude-post-bash-grep", &repo.dir, &home).replace("grep -rn \\\"dev_read\\\" src/ --include=\\\"*.c\\\" | head -20", "sed -i -e 's/a/a/' src/main.c");
    hook(&home, &repo.dir, "post-tool-use", sed.as_bytes(), &[]).assert_quiet("a later shell edit");
    // In another session the shell edit is where it is noticed.
    let other = hook(&home, &repo.dir, "post-tool-use", in_session(&sed, "another").as_bytes(), &[]);
    assert!(other.context("PostToolUse").starts_with("codefast: copy_all is no longer defined in src/main.c"), "{}", other.stdout);
    // Codex edits through apply_patch and names the files in the patch.
    let patch = hook(&home, &repo.dir, "post-tool-use", in_session(&payload("codex-post-apply-patch", &repo.dir, &home), "codex").as_bytes(), &[]);
    assert!(patch.context("PostToolUse").contains("Still used at 1 site in 1 file:\nsrc/main.c: main:15"), "{}", patch.stdout);

    // Once the call is renamed too, nothing is left behind and nothing is said.
    edit(&repo, "src/main.c", "return copy_all(&a, &b);", "return copy_everything(&a, &b);");
    hook(&home, &repo.dir, "post-tool-use", in_session(&input, "after-the-fix").as_bytes(), &[]).assert_quiet("a complete rename");
}

#[test]
fn an_edit_that_adds_a_parameter_is_told_which_calls_no_longer_fit() {
    let (repo, home) = committed("edit-arity");
    edit(&repo, "src/io.c", "int dev_write(struct dev *d, const char *buf, int count)", "int dev_write(struct dev *d, const char *buf, int count, int flags)");
    edit(&repo, "src/io.h", "int dev_write(struct dev *d, const char *buf, int count);", "int dev_write(struct dev *d, const char *buf, int count, int flags);");
    let input = payload("claude-post-edit", &repo.dir, &home).replace("src/main.c", "src/io.c");
    let ran = hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[]);
    assert_eq!(
        ran.context("PostToolUse"),
        "codefast: dev_write took 3 arguments and now takes 4 arguments. 1 call in 1 file pass another number:\nsrc/main.c: copy_all:8"
    );
    // The command says the same when asked.
    let (code, out, _) = run(&home, &repo.dir, &["check"], &[], None);
    assert!(code == 0 && out.starts_with("dev_write took 3 arguments and now takes 4 arguments."), "{out}");
    // An edit that changes a body and nothing else has nothing to report.
    let (repo, home) = committed("edit-body");
    edit(&repo, "src/io.c", "if (!d)\n        return -1;", "if (!d || count < 0)\n        return -1;");
    hook(&home, &repo.dir, "post-tool-use", payload("claude-post-edit", &repo.dir, &home).replace("src/main.c", "src/io.c").as_bytes(), &[]).assert_quiet("a changed body");
    let (_, out, _) = run(&home, &repo.dir, &["check"], &[], None);
    assert_eq!(out, "1 changed file checked: nothing they removed or re-signatured is still used elsewhere\n");
}

#[test]
fn after_a_compaction_the_session_is_told_what_it_changed() {
    let (repo, home) = committed("compact");
    let input = payload("claude-session-compact", &repo.dir, &home);
    // Nothing changed yet: the ordinary one-line note.
    let clean = hook(&home, &repo.dir, "session-start", input.as_bytes(), &[]).context("SessionStart");
    assert!(clean.starts_with("codefast has an index of this repository (") && clean.lines().count() == 1, "{clean}");
    edit(&repo, "src/main.c", "static int copy_all(struct dev *in", "static int copy_everything(struct dev *in");
    edit(&repo, "src/io.c", "if (!d)\n        return -1;", "if (!d || count < 0)\n        return -1;");
    write(&repo.path("src/extra.c"), "int extra_one(int x)\n{\n    return x;\n}\n");
    let recap = hook(&home, &repo.dir, "session-start", input.as_bytes(), &[]).context("SessionStart");
    assert_eq!(
        recap,
        "codefast: uncommitted changes, by definition (3 source files; + added, - removed, ~ body changed):\nnew file src/extra.c: +extra_one\nsrc/io.c: ~dev_read\nsrc/main.c: -copy_all +copy_everything\n`codefast check` lists uses left behind by these changes, `codefast search NAME` a symbol with every place that uses it."
    );
    // The command prints the same list, followed by what the changes left behind.
    let (code, out, _) = run(&home, &repo.dir, &["changed"], &[], None);
    assert!(code == 0 && out.contains("src/main.c: -copy_all +copy_everything\n") && out.contains("\ncopy_all is no longer defined in src/main.c"), "{out}");
    settle(&repo);
}

#[test]
fn a_call_of_a_method_nobody_defines_is_named_with_what_the_type_does_have() {
    let repo = Repo::new("phantom");
    std::fs::remove_dir_all(repo.path(".git")).unwrap();
    write(&repo.path("shop/cart.py"), "class Cart:\n    def __init__(self):\n        self.items = []\n\n    def add(self, item):\n        self.items.append(item)\n\n    def total(self):\n        return len(self.items)\n");
    write(&repo.path("shop/checkout.py"), "from shop.cart import Cart\n\n\ndef checkout(names):\n    cart = Cart()\n    for n in names:\n        cart.add(n)\n    return cart.total()\n");
    git(&repo, &["init", "-q", "."]);
    write(&repo.path(".gitignore"), ".codefast/\n");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    let home = scratch("home");
    assert_eq!(run(&home, &repo.dir, &["index"], &[], None).0, 0);
    // The edit calls a method that was imagined, and one that exists.
    edit(&repo, "shop/checkout.py", "        cart.add(n)\n", "        cart.add_item(n)\n        cart.add(n)\n");
    let input = payload("claude-post-edit", &repo.dir, &home).replace("src/main.c", "shop/checkout.py");
    let said = hook(&home, &repo.dir, "post-tool-use", input.as_bytes(), &[]).context("PostToolUse");
    assert_eq!(said, "codefast: shop/checkout.py:7 uses Cart.add_item, and nothing in this repository defines `add_item`. Cart (shop/cart.py:1) has: add, items, total.");
    // A class with a base from a library may have any member: nothing is said about it.
    edit(&repo, "shop/cart.py", "class Cart:", "import collections\n\n\nclass Cart(collections.UserList):");
    edit(&repo, "shop/checkout.py", "cart.add_item(n)", "cart.extend_all(n)");
    hook(&home, &repo.dir, "post-tool-use", in_session(&input, "library-base").as_bytes(), &[]).assert_quiet("a member that may come from a library class");
}

#[test]
fn a_prompt_that_asks_for_a_rename_gets_the_command_that_does_it() {
    let (repo, home) = committed("prompt-rename");
    let input = payload("claude-user-prompt", &repo.dir, &home).replace("dev_read returns -1 for a device that is open. Find out why and fix it.", "Rename `dev_read` to `dev_fetch` everywhere, tests included.");
    assert!(input.contains("Rename `dev_read`"), "{input}");
    let said = hook(&home, &repo.dir, "user-prompt-submit", input.as_bytes(), &[]).context("UserPromptSubmit");
    // One card, led by the command. The usual card about the symbol is not sent with it.
    assert_eq!(
        said,
        "codefast: `codefast rename dev_read NEW_NAME` renames dev_read in one step: the definition at src/io.c:4 and every call the index is sure of (called from 1 site in 1 file). It lists what it leaves alone."
    );
}
