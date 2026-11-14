use std::path::PathBuf;
use std::process::Command;

use patchwork::{apply_patch, diff_lines, format_patch, make_patch, merge3, split_lines, Edit, HunkLine, Merged, Whitespace};

fn ls(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn eq(s: &str) -> Edit {
    Edit::Equal(s.to_string())
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_patchwork"))
        .args(args)
        .output()
        .expect("run patchwork");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn file(name: &str, text: &str) -> String {
    let mut p = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    p.push(format!("patchwork-ws-{name}"));
    std::fs::write(&p, text).unwrap();
    p.to_string_lossy().into_owned()
}

/// Creates a fresh directory holding the given files.
fn tree(name: &str, files: &[(&str, &str)]) -> String {
    let mut root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    root.push(format!("patchwork-ws-tree-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    for (rel, text) in files {
        std::fs::write(root.join(rel), text).unwrap();
    }
    root.to_string_lossy().into_owned()
}

const ALL: [Whitespace; 3] = [Whitespace::Exact, Whitespace::IgnoreTrailing, Whitespace::IgnoreAll];

#[test]
fn exact_compares_whole_lines() {
    let got = diff_lines(&ls(&["a \n", "b\n"]), &ls(&["a\n", "b\n"]), Whitespace::Exact);
    assert_eq!(got, vec![Edit::Delete("a \n".into()), Edit::Insert("a\n".into()), eq("b\n")]);
    for ws in ALL {
        let got = diff_lines(&split_lines("a\nb"), &split_lines("a\nb\n"), ws);
        assert_eq!(got, vec![eq("a\n"), Edit::Delete("b".into()), Edit::Insert("b\n".into())], "{ws:?}");
    }
}

#[test]
fn ignore_trailing_equal_lines_carry_the_old_line() {
    let old = ls(&["a  \n", "b\t\n", "c\n", "d \t"]);
    let new = ls(&["a\n", "b \n", "c \n", "d"]);
    assert_eq!(diff_lines(&old, &new, Whitespace::IgnoreTrailing), vec![eq("a  \n"), eq("b\t\n"), eq("c\n"), eq("d \t")]);
    assert_eq!(diff_lines(&old, &new, Whitespace::IgnoreAll), vec![eq("a  \n"), eq("b\t\n"), eq("c\n"), eq("d \t")]);
}

#[test]
fn ignore_trailing_keeps_leading_and_inner_space() {
    let old = ls(&[" a\n", "b c\n"]);
    let new = ls(&["a\n", "bc\n"]);
    let got = diff_lines(&old, &new, Whitespace::IgnoreTrailing);
    let want = vec![
        Edit::Delete(" a\n".into()),
        Edit::Delete("b c\n".into()),
        Edit::Insert("a\n".into()),
        Edit::Insert("bc\n".into()),
    ];
    assert_eq!(got, want);
    assert_eq!(diff_lines(&old, &new, Whitespace::IgnoreAll), vec![eq(" a\n"), eq("b c\n")]);
}

#[test]
fn ignore_all_removes_every_space_and_tab() {
    let got = diff_lines(&ls(&["\ta b\tc \n", "  \n", "x\n"]), &ls(&["abc\n", "\n", "y\n"]), Whitespace::IgnoreAll);
    assert_eq!(got, vec![eq("\ta b\tc \n"), eq("  \n"), Edit::Delete("x\n".into()), Edit::Insert("y\n".into())]);
    let got = diff_lines(&ls(&["  \n"]), &ls(&["\n"]), Whitespace::IgnoreTrailing);
    assert_eq!(got, vec![eq("  \n")]);
    let got = diff_lines(&ls(&["a\r\n"]), &ls(&["a\n"]), Whitespace::IgnoreAll);
    assert_eq!(got, vec![Edit::Delete("a\r\n".into()), Edit::Insert("a\n".into())]);
}

#[test]
fn diff_is_minimal_under_the_rule() {
    let old = ls(&["a \n", "b\n", "c\n"]);
    let new = ls(&["a\n", "B\n", "c\t\n"]);
    let got = diff_lines(&old, &new, Whitespace::IgnoreTrailing);
    assert_eq!(got, vec![eq("a \n"), Edit::Delete("b\n".into()), Edit::Insert("B\n".into()), eq("c\n")]);
}

#[test]
fn make_patch_with_a_rule() {
    let (old, new) = ("a \nb\nc\n", "a\nB\nc\n");
    let p = make_patch("o", old, "n", new, 0, Whitespace::IgnoreTrailing);
    assert_eq!(format_patch(&p), "--- o\n+++ n\n@@ -2 +2 @@\n-b\n+B\n");
    let p = make_patch("o", old, "n", new, 0, Whitespace::Exact);
    assert_eq!(format_patch(&p), "--- o\n+++ n\n@@ -1,2 +1,2 @@\n-a \n-b\n+a\n+B\n");
    let p = make_patch("o", "x y\n", "n", "xy \n", 3, Whitespace::IgnoreAll);
    assert!(p.hunks.is_empty());
}

#[test]
fn context_lines_keep_the_old_text() {
    let (old, new) = ("x y\nb\n", "xy\nB\n");
    let p = make_patch("o", old, "n", new, 1, Whitespace::IgnoreAll);
    assert_eq!(p.hunks.len(), 1);
    assert_eq!(
        p.hunks[0].lines,
        vec![HunkLine::Context("x y\n".into()), HunkLine::Removed("b\n".into()), HunkLine::Added("B\n".into())]
    );
    assert_eq!(apply_patch(old, &p).unwrap().text, "x y\nB\n");
}

#[test]
fn merge_ignores_whitespace_only_changes() {
    let base = "a\nb\nc\nd\ne\n";
    let (ours, theirs) = ("a \nB\nc\nd\ne\n", "a\nb\nc\nD\ne\n");
    assert_eq!(merge3(base, ours, theirs, Whitespace::Exact).text, "a \nB\nc\nD\ne\n");
    assert_eq!(
        merge3(base, ours, theirs, Whitespace::IgnoreTrailing),
        Merged { text: "a\nB\nc\nD\ne\n".into(), conflicts: 0 }
    );
    let got = merge3("a\nb\n", "a \nb\n", "a\nb\t\n", Whitespace::IgnoreTrailing);
    assert_eq!(got, Merged { text: "a\nb\n".into(), conflicts: 0 });
}

#[test]
fn merge_versions_compare_under_the_rule() {
    let (base, ours, theirs) = ("a\nb\nc\n", "a\nX \nc\n", "a\nX\nc\n");
    assert_eq!(merge3(base, ours, theirs, Whitespace::Exact).conflicts, 1);
    assert_eq!(merge3(base, ours, theirs, Whitespace::IgnoreTrailing), Merged { text: ours.into(), conflicts: 0 });
    let got = merge3(base, "a\nX Y\nc\n", "a\nXY\nc\n", Whitespace::IgnoreTrailing);
    assert_eq!(got.conflicts, 1);
    let got = merge3(base, "a\nX Y\nc\n", "a\nXY\nc\n", Whitespace::IgnoreAll);
    assert_eq!(got, Merged { text: "a\nX Y\nc\n".into(), conflicts: 0 });
}

#[test]
fn cli_diff_whitespace_options() {
    let a = file("d-a.txt", "one \ntwo\nthree\n");
    let b = file("d-b.txt", "one\nTWO\nthree\n");
    let (code, out, _) = run(&["diff", &a, &b, "-Z", "-U", "0"]);
    assert_eq!((code, out), (1, format!("--- {a}\n+++ {b}\n@@ -2 +2 @@\n-two\n+TWO\n")));
    let (code, out, _) = run(&["diff", &a, &b, "-U", "0", "-w"]);
    assert_eq!((code, out), (1, format!("--- {a}\n+++ {b}\n@@ -2 +2 @@\n-two\n+TWO\n")));
    let (code, out, _) = run(&["diff", &a, &b, "-U", "0"]);
    assert_eq!((code, out), (1, format!("--- {a}\n+++ {b}\n@@ -1,2 +1,2 @@\n-one \n-two\n+one\n+TWO\n")));
    let c = file("d-c.txt", "one\ttwo\n");
    let d = file("d-d.txt", "one two \n");
    assert_eq!(run(&["diff", &c, &d, "-w"]), (0, String::new(), String::new()));
    assert_eq!(run(&["diff", &c, &d, "-Z"]).0, 1);
    let (left, right) = (tree("t-left", &[("x.txt", "a \nb\n")]), tree("t-right", &[("x.txt", "a\nb\n")]));
    assert_eq!(run(&["diff", &left, &right, "-Z"]), (0, String::new(), String::new()));
    let (code, out, _) = run(&["diff", &left, &right, "-U", "0"]);
    assert_eq!((code, out), (1, format!("--- {left}/x.txt\n+++ {right}/x.txt\n@@ -1 +1 @@\n-a \n+a\n")));
}

#[test]
fn cli_diff_bad_options() {
    let a = file("o-a.txt", "a\n");
    let cases: Vec<Vec<&str>> = vec![
        vec!["diff", &a, &a, "-Z", "-w"],
        vec!["diff", &a, &a, "-w", "-w"],
        vec!["diff", &a, &a, "-U", "1", "-U", "2"],
        vec!["diff", &a, &a, "-w", "-U"],
        vec!["diff", &a, &a, "-b"],
    ];
    for args in cases {
        let (code, out, err) = run(&args);
        assert_eq!((code, out.as_str()), (2, ""), "args {args:?}");
        assert!(err.starts_with("usage:"), "args {args:?} stderr {err}");
    }
    let (code, _, err) = run(&["diff", &a, &a, "-w", "-U", "x"]);
    assert_eq!((code, err.trim_end()), (2, "error: invalid context: x"));
}

#[test]
fn cli_merge_whitespace_options() {
    let base = file("m-base.txt", "a\nb\nc\n");
    let ours = file("m-ours.txt", "a\nX \nc\n");
    let theirs = file("m-theirs.txt", "a\nX\nc\n");
    assert_eq!(run(&["merge", &base, &ours, &theirs, "-Z"]), (0, "a\nX \nc\n".to_string(), String::new()));
    assert_eq!(run(&["merge", &base, &ours, &theirs, "-w"]).0, 0);
    let (code, out, err) = run(&["merge", &base, &ours, &theirs]);
    assert_eq!((code, out.as_str(), err.as_str()), (1, "a\n<<<<<<< ours\nX \n=======\nX\n>>>>>>> theirs\nc\n", "conflicts: 1\n"));
    let cases: Vec<Vec<&str>> = vec![
        vec!["merge", &base, &ours, &theirs, "-Z", "-w"],
        vec!["merge", &base, &ours, &theirs, "-U", "1"],
        vec!["merge", &base, &ours, &theirs, "-Z", "-Z"],
    ];
    for args in cases {
        let (code, out, err) = run(&args);
        assert_eq!((code, out.as_str()), (2, ""), "args {args:?}");
        assert!(err.starts_with("usage:"), "args {args:?} stderr {err}");
    }
}
