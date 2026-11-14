use std::path::PathBuf;
use std::process::Command;

use patchwork::{apply_patch, apply_patch_fuzzy, Applied, Error, Hunk, HunkLine, Patch};

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

fn patch(hunks: Vec<Hunk>) -> Patch {
    Patch { old_name: "old".into(), new_name: "new".into(), hunks }
}

/// ` a`, `-b`, `+B`, ` c` at line 1.
fn abc() -> Hunk {
    hunk(1, 3, 1, 3, vec![c("a\n"), r("b\n"), a("B\n"), c("c\n")])
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
    p.push(format!("patchwork-fuzz-{name}"));
    std::fs::write(&p, text).unwrap();
    p.to_string_lossy().into_owned()
}

#[test]
fn fuzz_zero_is_plain_apply() {
    let p = patch(vec![abc(), hunk(6, 1, 6, 1, vec![r("q\n"), a("Q\n")])]);
    let text = "n\na\nb\nc\nd\ne\nq\n";
    let plain = apply_patch(text, &p).unwrap();
    assert_eq!(plain, apply_patch_fuzzy(text, &p, 0).unwrap());
    assert_eq!(plain, Applied { text: "n\na\nB\nc\nd\ne\nQ\n".into(), offsets: vec![1, 1], fuzz: vec![0, 0] });
    assert_eq!(apply_patch_fuzzy("A\nb\nc\n", &patch(vec![abc()]), 0), Err(Error::HunkFailed(1)));
}

#[test]
fn fuzz_one_drops_a_changed_leading_line() {
    let p = patch(vec![abc()]);
    assert_eq!(apply_patch("X\nb\nc\nd\n", &p), Err(Error::HunkFailed(1)));
    let got = apply_patch_fuzzy("X\nb\nc\nd\n", &p, 1).unwrap();
    assert_eq!(got, Applied { text: "X\nB\nc\nd\n".into(), offsets: vec![0], fuzz: vec![1] });
}

#[test]
fn fuzz_one_drops_a_changed_trailing_line() {
    let p = patch(vec![abc()]);
    let got = apply_patch_fuzzy("a\nb\nZ\n", &p, 1).unwrap();
    assert_eq!(got, Applied { text: "a\nB\nZ\n".into(), offsets: vec![0], fuzz: vec![1] });
    let got = apply_patch_fuzzy("a\nb\n", &patch(vec![abc()]), 2).unwrap();
    assert_eq!((got.text.as_str(), got.fuzz), ("a\nB\n", vec![1]));
}

#[test]
fn fuzz_two_when_two_lines_changed() {
    let p = patch(vec![hunk(1, 5, 1, 5, vec![c("p\n"), c("q\n"), r("x\n"), a("X\n"), c("s\n"), c("t\n")])]);
    let text = "P\nQ\nx\ns\nt\n";
    assert_eq!(apply_patch_fuzzy(text, &p, 1), Err(Error::HunkFailed(1)));
    let want = Applied { text: "P\nQ\nX\ns\nt\n".into(), offsets: vec![0], fuzz: vec![2] };
    assert_eq!(apply_patch_fuzzy(text, &p, 2).unwrap(), want);
    assert_eq!(apply_patch_fuzzy(text, &p, 5).unwrap(), want);
}

#[test]
fn fuzz_is_limited_by_the_context_at_each_end() {
    let p = patch(vec![hunk(1, 5, 1, 5, vec![c("a\n"), r("b\n"), a("B\n"), c("c\n"), c("d\n"), c("e\n")])]);
    let got = apply_patch_fuzzy("a\nb\nc\nd\nE\n", &p, 3).unwrap();
    assert_eq!((got.text.as_str(), got.fuzz.clone()), ("a\nB\nc\nd\nE\n", vec![1]));
    let got = apply_patch_fuzzy("Z\nb\nc\nD\nE\n", &p, 3).unwrap();
    assert_eq!(got, Applied { text: "Z\nB\nc\nD\nE\n".into(), offsets: vec![0], fuzz: vec![2] });
}

#[test]
fn fuzz_offset_counts_from_the_shortened_hunk() {
    let p = patch(vec![abc()]);
    let got = apply_patch_fuzzy("n1\nn2\nA\nb\nc\n", &p, 1).unwrap();
    assert_eq!(got, Applied { text: "n1\nn2\nA\nB\nc\n".into(), offsets: vec![2], fuzz: vec![1] });
    let p = patch(vec![hunk(5, 3, 5, 3, vec![c("a\n"), r("b\n"), a("B\n"), c("c\n")])]);
    let got = apply_patch_fuzzy("A\nb\nC\n", &p, 1).unwrap();
    assert_eq!(got, Applied { text: "A\nB\nC\n".into(), offsets: vec![-4], fuzz: vec![1] });
}

#[test]
fn fuzzed_offset_carries_to_the_next_hunk() {
    let p = patch(vec![abc(), hunk(6, 1, 6, 1, vec![r("q\n"), a("Q\n")])]);
    let text = "n1\nn2\nA\nb\nc\nq\nd\nq\ne\n";
    let got = apply_patch_fuzzy(text, &p, 1).unwrap();
    assert_eq!(got.offsets, vec![2, 2]);
    assert_eq!(got.fuzz, vec![1, 0]);
    assert_eq!(got.text, "n1\nn2\nA\nB\nc\nq\nd\nQ\ne\n");
}

#[test]
fn fuzz_zero_far_away_beats_fuzz_one_nearby() {
    let p = patch(vec![abc()]);
    let got = apply_patch_fuzzy("A\nb\nC\nm\na\nb\nc\n", &p, 2).unwrap();
    assert_eq!(got, Applied { text: "A\nb\nC\nm\na\nB\nc\n".into(), offsets: vec![4], fuzz: vec![0] });
}

#[test]
fn fuzz_cannot_help_a_hunk_without_context() {
    let p = patch(vec![hunk(2, 1, 2, 1, vec![r("x\n"), a("y\n")])]);
    assert_eq!(apply_patch_fuzzy("a\nX\nb\n", &p, 3), Err(Error::HunkFailed(1)));
    let p = patch(vec![abc(), hunk(3, 1, 3, 1, vec![r("x\n"), a("y\n")])]);
    assert_eq!(apply_patch_fuzzy("A\nb\nC\n", &p, 3), Err(Error::HunkFailed(2)));
}

#[test]
fn fuzz_on_a_hunk_of_only_context() {
    let p = patch(vec![hunk(1, 3, 1, 3, vec![c("a\n"), c("b\n"), c("c\n")])]);
    let got = apply_patch_fuzzy("A\nb\nC\n", &p, 3).unwrap();
    assert_eq!(got, Applied { text: "A\nb\nC\n".into(), offsets: vec![0], fuzz: vec![3] });
    let got = apply_patch_fuzzy("A\nb\nc\n", &p, 3).unwrap();
    assert_eq!(got.fuzz, vec![1]);
}

#[test]
fn cli_apply_with_fuzz_option() {
    let target = file("opt-target.txt", "A\nb\nc\n");
    let patch = file("opt.patch", "--- x\n+++ y\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n");
    let (code, out, err) = run(&["apply", &target, &patch]);
    assert_eq!((code, out.as_str(), err.trim_end()), (1, "", "error: hunk 1 failed"));
    assert_eq!(run(&["apply", &target, &patch, "-F", "0"]).0, 1);
    assert_eq!(
        run(&["apply", &target, &patch, "-F", "1"]),
        (0, "A\nB\nc\n".to_string(), "hunk 1 offset 0 fuzz 1\n".to_string())
    );
}

#[test]
fn cli_apply_reports_offsets_and_fuzz_in_order() {
    let target = file("ord-target.txt", "n1\nn2\na\nb\nc\nE\nf\ng\n");
    let patch = file(
        "ord.patch",
        "--- x\n+++ y\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n@@ -4,3 +4,3 @@\n e\n-f\n+F\n g\n",
    );
    let (code, out, err) = run(&["apply", &target, &patch, "-F", "2"]);
    assert_eq!(code, 0);
    assert_eq!(out, "n1\nn2\na\nB\nc\nE\nF\ng\n");
    assert_eq!(err, "hunk 1 offset 2\nhunk 2 offset 2 fuzz 1\n");
}

#[test]
fn cli_apply_bad_fuzz_arguments() {
    let target = file("bad-target.txt", "a\n");
    let patch = file("bad.patch", "--- x\n+++ y\n");
    let (code, out, err) = run(&["apply", &target, &patch, "-F", "x"]);
    assert_eq!((code, out.as_str(), err.trim_end()), (2, "", "error: invalid fuzz: x"));
    let (code, _, err) = run(&["apply", &target, &patch, "-F", "-1"]);
    assert_eq!((code, err.trim_end()), (2, "error: invalid fuzz: -1"));
    let cases: Vec<Vec<&str>> = vec![vec!["apply", &target, &patch, "-F"], vec!["apply", &target, &patch, "-X", "1"]];
    for args in cases {
        let (code, out, err) = run(&args);
        assert_eq!((code, out.as_str()), (2, ""), "args {args:?}");
        assert!(err.starts_with("usage:"), "args {args:?} stderr {err}");
    }
    assert_eq!(run(&["apply", &target, &patch, "-F", "3"]), (0, "a\n".to_string(), String::new()));
}
