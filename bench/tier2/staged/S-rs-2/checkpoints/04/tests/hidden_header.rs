use std::path::PathBuf;
use std::process::Command;

use patchwork::{format_patch, make_patch, parse_patch, Error, Hunk, HunkLine};

fn c(s: &str) -> HunkLine {
    HunkLine::Context(s.to_string())
}
fn r(s: &str) -> HunkLine {
    HunkLine::Removed(s.to_string())
}
fn a(s: &str) -> HunkLine {
    HunkLine::Added(s.to_string())
}

fn hunk(os: usize, oc: usize, ns: usize, nc: usize, lines: Vec<HunkLine>) -> Hunk {
    Hunk { old_start: os, old_count: oc, new_start: ns, new_count: nc, lines }
}

fn parse_err_line(text: &str) -> usize {
    match parse_patch(text) {
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
    p.push(format!("patchwork-header-{name}"));
    std::fs::write(&p, text).unwrap();
    p.to_string_lossy().into_owned()
}

#[test]
fn format_omits_a_count_of_one() {
    let old: String = (1..=9).map(|i| format!("{i}\n")).collect();
    let p = make_patch("a", &old, "b", &old.replace("5\n", "five\n"), 0);
    assert_eq!(format_patch(&p), "--- a\n+++ b\n@@ -5 +5 @@\n-5\n+five\n");
    let p = make_patch("o", "x", "n", "y", 3);
    assert_eq!(
        format_patch(&p),
        "--- o\n+++ n\n@@ -1 +1 @@\n-x\n\\ No newline at end of file\n+y\n\\ No newline at end of file\n"
    );
}

#[test]
fn format_omits_each_side_separately() {
    let p = make_patch("o", "a\nb\nc\n", "n", "a\nc\n", 0);
    assert_eq!(format_patch(&p), "--- o\n+++ n\n@@ -2 +1,0 @@\n-b\n");
    let p = make_patch("o", "a\nc\n", "n", "a\nb\nc\n", 0);
    assert_eq!(format_patch(&p), "--- o\n+++ n\n@@ -1,0 +2 @@\n+b\n");
    let p = make_patch("o", "a\nb\nc\nd\n", "n", "a\nb\nX\n", 0);
    assert_eq!(format_patch(&p), "--- o\n+++ n\n@@ -3,2 +3 @@\n-c\n-d\n+X\n");
}

#[test]
fn format_keeps_zero_and_larger_counts() {
    assert_eq!(format_patch(&make_patch("o", "", "n", "x\n", 3)), "--- o\n+++ n\n@@ -0,0 +1 @@\n+x\n");
    assert_eq!(format_patch(&make_patch("o", "x\n", "n", "", 3)), "--- o\n+++ n\n@@ -1 +0,0 @@\n-x\n");
    let p = make_patch("o", "a\nb\nc\n", "n", "a\nB\nc\n", 3);
    assert_eq!(format_patch(&p), "--- o\n+++ n\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n");
}

#[test]
fn parse_accepts_omitted_counts() {
    let p = parse_patch("--- a\n+++ b\n@@ -5 +5 @@\n-x\n+y\n").unwrap();
    assert_eq!(p.hunks, vec![hunk(5, 1, 5, 1, vec![r("x\n"), a("y\n")])]);
    let p = parse_patch("--- a\n+++ b\n@@ -3,2 +3 @@ fn main\n-c\n-d\n+X\n").unwrap();
    assert_eq!(p.hunks, vec![hunk(3, 2, 3, 1, vec![r("c\n"), r("d\n"), a("X\n")])]);
    let p = parse_patch("--- a\n+++ b\n@@ -1,0 +2 @@\n+b\n@@ -7 +8,2 @@\n k\n+l\n").unwrap();
    assert_eq!(p.hunks, vec![hunk(1, 0, 2, 1, vec![a("b\n")]), hunk(7, 1, 8, 2, vec![c("k\n"), a("l\n")])]);
    let p = parse_patch("--- a\n+++ b\n@@ -1,1 +1 @@\n-x\n+y\n").unwrap();
    assert_eq!(p.hunks, vec![hunk(1, 1, 1, 1, vec![r("x\n"), a("y\n")])]);
    assert_eq!(format_patch(&p), "--- a\n+++ b\n@@ -1 +1 @@\n-x\n+y\n");
}

#[test]
fn parse_rejects_bad_short_ranges() {
    for header in ["@@ -0 +1 @@", "@@ -1 +0 @@", "@@ -1, +1 @@", "@@ - +1 @@", "@@ -1 + @@", "@@ -,1 +1 @@", "@@ -1 +1,x @@", "@@ -1 +1 @@x"] {
        let text = format!("--- a\n+++ b\n{header}\n x\n");
        assert_eq!(parse_err_line(&text), 3, "{header}");
    }
}

#[test]
fn omitted_counts_limit_the_body() {
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1 +1 @@\n x\n y\n"), 5);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1 +1,2 @@\n-a\n-b\n"), 5);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1 +1 @@\n"), 4);
    let p = parse_patch("--- a\n+++ b\n@@ -2 +2 @@\n-b\n\\ No newline at end of file\n+B\n").unwrap();
    assert_eq!(p.hunks, vec![hunk(2, 1, 2, 1, vec![r("b"), a("B\n")])]);
}

#[test]
fn cli_writes_and_reads_short_headers() {
    let a = file("d-a.txt", "one\ntwo\nthree\n");
    let b = file("d-b.txt", "one\nTWO\nthree\n");
    let (code, out, _) = run(&["diff", &a, &b, "-U", "0"]);
    assert_eq!((code, out), (1, format!("--- {a}\n+++ {b}\n@@ -2 +2 @@\n-two\n+TWO\n")));
    let patch = file("rev.patch", "--- x\n+++ y\n@@ -4,1 +4,2 @@\n-q\n+r\n+s\n@@ -9,0 +11,1 @@\n+t\n");
    let (code, out, _) = run(&["reverse", &patch]);
    assert_eq!(code, 0);
    assert_eq!(out, "--- y\n+++ x\n@@ -4,2 +4 @@\n-r\n-s\n+q\n@@ -11 +9,0 @@\n-t\n");
    let short = file("ap.patch", "--- x\n+++ y\n@@ -2 +2,2 @@\n-two\n+2\n+2b\n");
    assert_eq!(run(&["apply", &a, &short]), (0, "one\n2\n2b\nthree\n".to_string(), String::new()));
}

