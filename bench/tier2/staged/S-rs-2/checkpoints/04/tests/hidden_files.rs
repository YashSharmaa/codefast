use std::path::PathBuf;
use std::process::Command;

use patchwork::{format_patch, format_patches, make_patch, parse_patch, parse_patches, Error, Hunk, HunkLine, Patch};

fn r(s: &str) -> HunkLine {
    HunkLine::Removed(s.to_string())
}
fn a(s: &str) -> HunkLine {
    HunkLine::Added(s.to_string())
}

fn hunk(os: usize, oc: usize, ns: usize, nc: usize, lines: Vec<HunkLine>) -> Hunk {
    Hunk { old_start: os, old_count: oc, new_start: ns, new_count: nc, lines }
}

fn named(old: &str, new: &str, hunks: Vec<Hunk>) -> Patch {
    Patch { old_name: old.into(), new_name: new.into(), hunks }
}

fn parse_err_line(text: &str) -> usize {
    match parse_patches(text) {
        Err(Error::Parse { line, .. }) => line,
        other => panic!("expected parse error for {text:?}, got {other:?}"),
    }
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
    p.push(format!("patchwork-files-{name}"));
    std::fs::write(&p, text).unwrap();
    p.to_string_lossy().into_owned()
}

/// Creates a fresh directory holding the given files (paths relative to it).
fn tree(name: &str, files: &[(&str, &str)]) -> String {
    let mut root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    root.push(format!("patchwork-files-tree-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    for (rel, text) in files {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
    }
    root.to_string_lossy().into_owned()
}

const TWO_FILES: &str = "\
--- a/one.txt
+++ b/one.txt
@@ -2 +2 @@
-two
+TWO
--- a/two.txt
+++ b/two.txt
@@ -1,2 +1 @@
--- old
-keep
+keep
\\ No newline at end of file
";

#[test]
fn parse_patches_reads_files_one_after_another() {
    let text = format!("{TWO_FILES}--- empty\n+++ empty\n--- c\n+++ d\n@@ -0,0 +1 @@\n+x\n");
    let got = parse_patches(&text).unwrap();
    let want = vec![
        named("a/one.txt", "b/one.txt", vec![hunk(2, 1, 2, 1, vec![r("two\n"), a("TWO\n")])]),
        named("a/two.txt", "b/two.txt", vec![hunk(1, 2, 1, 1, vec![r("-- old\n"), r("keep\n"), a("keep")])]),
        named("empty", "empty", vec![]),
        named("c", "d", vec![hunk(0, 0, 1, 1, vec![a("x\n")])]),
    ];
    assert_eq!(got, want);
}

#[test]
fn one_file_texts_and_parse_patch() {
    let one = "--- x\n+++ y\n@@ -1,2 +1,2 @@\n a\n-b\n+B\n";
    assert_eq!(parse_patches(one).unwrap(), vec![parse_patch(one).unwrap()]);
    assert_eq!(parse_patches("--- x\n+++ y\n").unwrap(), vec![named("x", "y", vec![])]);
    match parse_patch(TWO_FILES) {
        Err(Error::Parse { line, .. }) => assert_eq!(line, 6),
        other => panic!("parse_patch must reject a second file, got {other:?}"),
    }
}

#[test]
fn parse_patches_errors_count_lines_from_the_start() {
    assert_eq!(parse_err_line(""), 1);
    assert_eq!(parse_err_line("--- a\n+++ b\n--- c\n"), 4);
    assert_eq!(parse_err_line("--- a\n+++ b\n--- c\n--- d\n"), 4);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1 +1 @@\n-x\n+y\n--- c\n+++ d\n@@ -1 +1 @@\n*z\n"), 9);
    assert_eq!(parse_err_line("--- a\n+++ b\n--- c\n+++ d\n@@ -1,2 +1,2 @@\n x\n"), 7);
    assert_eq!(parse_err_line("--- a\n+++ b\n--- c\n+++ d\nhello\n"), 5);
}

#[test]
fn format_patches_writes_them_in_order() {
    let p1 = make_patch("o1", "a\nb\n", "n1", "a\nB\n", 3);
    let p2 = make_patch("o2", "", "n2", "z", 3);
    let text = format_patches(&[p1.clone(), p2.clone()]);
    assert_eq!(text, format!("{}{}", format_patch(&p1), format_patch(&p2)));
    assert_eq!(parse_patches(&text).unwrap(), vec![p1, p2]);
    assert_eq!(format_patches(&[]), "");
    assert_eq!(format_patches(&parse_patches(TWO_FILES).unwrap()), TWO_FILES);
}

#[test]
fn cli_stat_and_reverse_several_files() {
    let patch = file("sr.patch", TWO_FILES);
    let (code, out, err) = run(&["stat", &patch]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(out, "a/one.txt -> b/one.txt: 1 hunks, 1 added, 1 removed\na/two.txt -> b/two.txt: 1 hunks, 1 added, 2 removed\n");
    let (code, out, _) = run(&["reverse", &patch]);
    assert_eq!(code, 0);
    assert_eq!(
        out,
        "--- b/one.txt\n+++ a/one.txt\n@@ -2 +2 @@\n-TWO\n+two\n--- b/two.txt\n+++ a/two.txt\n@@ -1 +1,2 @@\n-keep\n\\ No newline at end of file\n+-- old\n+keep\n"
    );
    let (code, _, err) = run(&["apply", &patch, &patch]);
    assert_eq!(code, 2);
    assert!(err.starts_with("error: line 6: "), "{err}");
}

#[test]
fn cli_diff_two_directories() {
    let old = tree("d-old", &[("a.txt", "1\n2\n3\n"), ("sub/b.txt", "same\n"), ("only_old.txt", "gone\n"), ("empty_old.txt", "")]);
    let new = tree("d-new", &[("a.txt", "1\nTWO\n3\n"), ("sub/b.txt", "same\n"), ("a/x.txt", "x\n")]);
    let (code, out, err) = run(&["diff", &old, &new, "-U", "0"]);
    assert_eq!((code, err.as_str()), (1, ""));
    let want = format!(
        "--- {old}/a.txt\n+++ {new}/a.txt\n@@ -2 +2 @@\n-2\n+TWO\n\
         --- {old}/a/x.txt\n+++ {new}/a/x.txt\n@@ -0,0 +1 @@\n+x\n\
         --- {old}/only_old.txt\n+++ {new}/only_old.txt\n@@ -1 +0,0 @@\n-gone\n"
    );
    assert_eq!(out, want);
    let (_, out, _) = run(&["diff", &old, &new]);
    assert!(out.starts_with(&format!("--- {old}/a.txt\n+++ {new}/a.txt\n@@ -1,3 +1,3 @@\n 1\n-2\n")), "{out}");
    let same = tree("d-same", &[("sub/b.txt", "same\n"), ("e.txt", "")]);
    let same2 = tree("d-same2", &[("sub/b.txt", "same\n")]);
    assert_eq!(run(&["diff", &same, &same2]), (0, String::new(), String::new()));
}

#[test]
fn cli_diff_directory_and_file_mismatch() {
    let dir = tree("m-dir", &[("x", "1\n")]);
    let plain = file("m-plain.txt", "1\n");
    let (code, out, err) = run(&["diff", &dir, &plain]);
    assert_eq!((code, out.as_str(), err.trim_end().to_string()), (2, "", format!("error: cannot read {dir}")));
    let (code, _, err) = run(&["diff", &plain, &dir]);
    assert_eq!((code, err.trim_end().to_string()), (2, format!("error: cannot read {dir}")));
    let left = tree("m-left", &[("x", "1\n")]);
    let right = tree("m-right", &[("x/y", "1\n")]);
    let (code, _, err) = run(&["diff", &left, &right]);
    assert_eq!((code, err.trim_end().to_string()), (2, format!("error: cannot read {right}/x")));
}
