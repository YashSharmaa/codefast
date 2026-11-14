use std::path::PathBuf;
use std::process::Command;

use patchwork::{apply_patch, format_patch, make_patch, parse_patch, parse_patches, reverse_patch, Error, HunkLine, Whitespace};

fn r(s: &str) -> HunkLine {
    HunkLine::Removed(s.to_string())
}
fn a(s: &str) -> HunkLine {
    HunkLine::Added(s.to_string())
}

fn err_line(result: Result<impl std::fmt::Debug, Error>) -> usize {
    match result {
        Err(Error::Parse { line, .. }) => line,
        other => panic!("expected a parse error, got {other:?}"),
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
    p.push(format!("patchwork-markers-{name}"));
    std::fs::write(&p, text).unwrap();
    p.to_string_lossy().into_owned()
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % bound
    }
    fn text(&mut self, max_len: u64, alphabet: u64) -> String {
        let n = self.next(max_len + 1);
        let mut s = String::new();
        for _ in 0..n {
            s.push((b'a' + self.next(alphabet) as u8) as char);
            s.push('\n');
        }
        if n > 0 && self.next(2) == 0 {
            s.pop();
        }
        s
    }
}

const JOINED: &str = "--- x\n+++ y\n@@ -1 +1,2 @@\n-old\n+a\n\\ No newline at end of file\n+b\n";

#[test]
fn the_reported_patch_is_rejected() {
    assert_eq!(err_line(parse_patch(JOINED)), 7);
    assert_eq!(err_line(parse_patches(JOINED)), 7);
}

#[test]
fn cli_rejects_the_reported_patch() {
    let target = file("target.txt", "old\n");
    let patch = file("joined.patch", JOINED);
    let (code, out, err) = run(&["apply", &target, &patch]);
    assert_eq!((code, out.as_str()), (2, ""));
    assert!(err.starts_with("error: line 7: "), "{err}");
    let (code, _, err) = run(&["stat", &patch]);
    assert_eq!(code, 2);
    assert!(err.starts_with("error: line 7: "), "{err}");
}

#[test]
fn removed_or_context_line_after_the_old_end() {
    assert_eq!(err_line(parse_patch("--- a\n+++ b\n@@ -1,2 +1,0 @@\n-a\n\\ No newline at end of file\n-b\n")), 6);
    assert_eq!(err_line(parse_patch("--- a\n+++ b\n@@ -1,2 +1 @@\n-a\n\\ No newline at end of file\n c\n")), 6);
}

#[test]
fn a_context_line_without_newline_ends_both_sides() {
    assert_eq!(err_line(parse_patch("--- a\n+++ b\n@@ -1 +1,2 @@\n a\n\\ No newline at end of file\n+b\n")), 6);
    assert_eq!(err_line(parse_patch("--- a\n+++ b\n@@ -1,2 +1 @@\n a\n\\ No newline at end of file\n-b\n")), 6);
}

#[test]
fn the_other_side_may_go_on() {
    let p = parse_patch("--- a\n+++ b\n@@ -1 +1,2 @@\n-a\n\\ No newline at end of file\n+a\n+b\n").unwrap();
    assert_eq!(p.hunks[0].lines, vec![r("a"), a("a\n"), a("b\n")]);
    let p = parse_patch("--- a\n+++ b\n@@ -1 +1 @@\n+a\n\\ No newline at end of file\n-b\n").unwrap();
    assert_eq!(p.hunks[0].lines, vec![a("a"), r("b\n")]);
    let p = parse_patch("--- a\n+++ b\n@@ -1 +1 @@\n-b\n\\ No newline at end of file\n+B\n\\ No newline at end of file\n").unwrap();
    assert_eq!(p.hunks[0].lines, vec![r("b"), a("B")]);
}

#[test]
fn a_later_hunk_may_not_continue_an_ended_side() {
    let text = "--- a\n+++ b\n@@ -1 +1 @@\n-z\n\\ No newline at end of file\n+Z\n@@ -5 +6 @@\n k\n";
    assert_eq!(err_line(parse_patch(text)), 8);
    let text = "--- a\n+++ b\n@@ -1 +1 @@\n-z\n\\ No newline at end of file\n+Z\n@@ -5,0 +6 @@\n+q\n";
    let p = parse_patch(text).unwrap();
    assert_eq!(p.hunks[1].lines, vec![a("q\n")]);
    let text = "--- a\n+++ b\n@@ -1 +1 @@\n-z\n+Z\n\\ No newline at end of file\n@@ -5 +6,0 @@\n-q\n";
    assert_eq!(parse_patch(text).unwrap().hunks[1].lines, vec![r("q\n")]);
    let text = "--- a\n+++ b\n@@ -1 +1 @@\n-z\n+Z\n\\ No newline at end of file\n@@ -5 +6 @@\n k\n";
    assert_eq!(err_line(parse_patch(text)), 8);
}

#[test]
fn every_file_patch_starts_afresh() {
    let first = "--- a\n+++ b\n@@ -1 +1 @@\n-x\n\\ m\n+y\n\\ m\n";
    let ok = format!("{first}--- c\n+++ d\n@@ -1 +1 @@\n-x\n+y\n");
    let got = parse_patches(&ok).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[1].hunks[0].lines, vec![r("x\n"), a("y\n")]);
    let bad = format!("{first}--- c\n+++ d\n@@ -1,2 +1 @@\n-x\n\\ m\n-w\n+y\n");
    assert_eq!(err_line(parse_patches(&bad)), 13);
}

#[test]
fn patches_patchwork_writes_still_parse() {
    let mut rng = Lcg(606);
    for _ in 0..150 {
        let (t1, t2) = (rng.text(10, 3), rng.text(10, 3));
        for ctx in [0, 1, 3] {
            let p = make_patch("l", &t1, "r", &t2, ctx, Whitespace::Exact);
            assert_eq!(parse_patch(&format_patch(&p)).unwrap(), p);
            let rev = reverse_patch(&p);
            assert_eq!(parse_patch(&format_patch(&rev)).unwrap(), rev);
            assert_eq!(apply_patch(&t2, &rev).unwrap().text, t1);
        }
    }
}

#[test]
fn error_lines_for_markers_are_unchanged() {
    assert_eq!(err_line(parse_patch("--- a\n+++ b\n@@ -1 +1 @@\n\\ No newline at end of file\n x\n")), 4);
    assert_eq!(err_line(parse_patch("--- a\n+++ b\n@@ -1 +1 @@\n x\n\\ one\n\\ two\n")), 6);
    let p = parse_patch("--- a\n+++ b\n@@ -1,2 +1,2 @@\n a\n-b\n+c\n\\ No newline at end of file\n").unwrap();
    assert_eq!(p.hunks[0].lines.last(), Some(&a("c")));
}
