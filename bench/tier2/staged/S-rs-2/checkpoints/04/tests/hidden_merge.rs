use std::path::PathBuf;
use std::process::Command;

use patchwork::{merge3, Merged};

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
    p.push(format!("patchwork-merge-{name}"));
    std::fs::write(&p, text).unwrap();
    p.to_string_lossy().into_owned()
}

fn clean(text: &str) -> Merged {
    Merged { text: text.to_string(), conflicts: 0 }
}

fn conflict(before: &str, ours: &str, theirs: &str, after: &str) -> String {
    format!("{before}<<<<<<< ours\n{ours}=======\n{theirs}>>>>>>> theirs\n{after}")
}

#[test]
fn merge_takes_changes_from_both_sides() {
    assert_eq!(merge3("a\nb\nc\nd\n", "a\nB\nc\nd\n", "a\nb\nc\nD\n"), clean("a\nB\nc\nD\n"));
    let base = "1\n2\n3\n4\n5\n6\n";
    assert_eq!(merge3(base, "0\n1\n2\n3\n4\n5\n6\n", "1\n2\n3\n4\n6\n"), clean("0\n1\n2\n3\n4\n6\n"));
    assert_eq!(merge3(base, "1\n3\n4\n5\n6\n", "1\n2\n3\n4\n5\n6\n7\n"), clean("1\n3\n4\n5\n6\n7\n"));
}

#[test]
fn merge_with_only_one_side_changed() {
    let base = "a\nb\nc\n";
    assert_eq!(merge3(base, "a\nX\nY\nc\n", base), clean("a\nX\nY\nc\n"));
    assert_eq!(merge3(base, base, "c\n"), clean("c\n"));
    assert_eq!(merge3(base, base, base), clean(base));
    assert_eq!(merge3(base, "a\nb\nc", base), clean("a\nb\nc"));
}

#[test]
fn merge_same_change_on_both_sides_is_taken_once() {
    assert_eq!(merge3("a\nb\nc\n", "a\nB\nc\n", "a\nB\nc\n"), clean("a\nB\nc\n"));
    assert_eq!(merge3("a\nb\nc\n", "a\nc\n", "a\nc\n"), clean("a\nc\n"));
    assert_eq!(merge3("a\nc\n", "a\nb\nc\n", "a\nb\nc\n"), clean("a\nb\nc\n"));
}

#[test]
fn merge_conflict_on_the_same_line() {
    let got = merge3("a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n");
    assert_eq!(got.text, "a\n<<<<<<< ours\nX\n=======\nY\n>>>>>>> theirs\nc\n");
    assert_eq!(got.conflicts, 1);
}

#[test]
fn merge_adjacent_changes_conflict() {
    let got = merge3("a\nb\nc\nd\n", "a\nB\nc\nd\n", "a\nb\nC\nd\n");
    assert_eq!(got, Merged { text: conflict("a\n", "B\nc\n", "b\nC\n", "d\n"), conflicts: 1 });
    let got = merge3("a\nb\nc\nd\n", "a\nb\nC\nd\n", "a\nB\nc\nd\n");
    assert_eq!(got, Merged { text: conflict("a\n", "b\nC\n", "B\nc\n", "d\n"), conflicts: 1 });
}

#[test]
fn merge_different_insertions_at_one_place() {
    let got = merge3("a\nb\n", "a\nX\nb\n", "a\nY\nZ\nb\n");
    assert_eq!(got, Merged { text: conflict("a\n", "X\n", "Y\nZ\n", "b\n"), conflicts: 1 });
}

#[test]
fn merge_insertion_touching_a_change() {
    let got = merge3("a\nb\nc\nd\n", "a\nb\nX\nc\nd\n", "a\nb\nC\nd\n");
    assert_eq!(got, Merged { text: conflict("a\nb\n", "X\nc\n", "C\n", "d\n"), conflicts: 1 });
    let got = merge3("a\nb\nc\nd\n", "a\nb\nc\nX\nd\n", "a\nb\nC\nd\n");
    assert_eq!(got, Merged { text: conflict("a\nb\n", "c\nX\n", "C\n", "d\n"), conflicts: 1 });
    let got = merge3("a\nb\nc\nd\n", "a\nX\nb\nc\nd\n", "a\nb\nC\nd\n");
    assert_eq!(got, clean("a\nX\nb\nC\nd\n"));
}

#[test]
fn merge_groups_join_transitively_and_count() {
    let got = merge3("a\nb\nc\nd\ne\n", "a\nB\nc\nD\ne\n", "a\nb\nC\nd\ne\n");
    assert_eq!(got, Merged { text: conflict("a\n", "B\nc\nD\n", "b\nC\nd\n", "e\n"), conflicts: 1 });
    let base = "1\n2\n3\n4\n5\n6\n7\n";
    let got = merge3(base, "1\nA\n3\n4\n5\nB\n7\n", "1\nX\n3\n4\n5\nY\n7\n");
    assert_eq!(got.conflicts, 2);
    let first = conflict("1\n", "A\n", "X\n", "3\n4\n5\n");
    assert_eq!(got.text, conflict(&first, "B\n", "Y\n", "7\n"));
}

#[test]
fn merge_deletion_against_change() {
    let got = merge3("a\nb\nc\n", "a\nc\n", "a\nB\nc\n");
    assert_eq!(got, Merged { text: "a\n<<<<<<< ours\n=======\nB\n>>>>>>> theirs\nc\n".into(), conflicts: 1 });
}

#[test]
fn merge_markers_start_on_their_own_line() {
    let got = merge3("a\nb", "a\nB", "a\nC");
    assert_eq!(got, Merged { text: "a\n<<<<<<< ours\nB\n=======\nC\n>>>>>>> theirs\n".into(), conflicts: 1 });
    let got = merge3("a\nb\n", "a\nB", "a\nC\n");
    assert_eq!(got.text, "a\n<<<<<<< ours\nB\n=======\nC\n>>>>>>> theirs\n");
    assert_eq!(merge3("a\nb", "a\nB", "a\nb"), clean("a\nB"));
}

#[test]
fn merge_from_an_empty_base() {
    assert_eq!(merge3("", "x\n", "y\n"), Merged { text: conflict("", "x\n", "y\n", ""), conflicts: 1 });
    assert_eq!(merge3("", "x\n", "x\n"), clean("x\n"));
    assert_eq!(merge3("", "", "y\n"), clean("y\n"));
    assert_eq!(merge3("", "", ""), clean(""));
}

#[test]
fn cli_merge_prints_the_result() {
    let base = file("c-base.txt", "a\nb\nc\nd\n");
    let ours = file("c-ours.txt", "a\nB\nc\nd\n");
    let theirs = file("c-theirs.txt", "a\nb\nc\nD");
    assert_eq!(run(&["merge", &base, &ours, &theirs]), (0, "a\nB\nc\nD".to_string(), String::new()));
    let other = file("c-other.txt", "a\nX\nc\nd\n");
    let (code, out, err) = run(&["merge", &base, &ours, &other]);
    assert_eq!(code, 1);
    assert_eq!(out, "a\n<<<<<<< ours\nB\n=======\nX\n>>>>>>> theirs\nc\nd\n");
    assert_eq!(err, "conflicts: 1\n");
}

#[test]
fn cli_merge_errors() {
    let base = file("e-base.txt", "a\n");
    let missing = format!("{}/patchwork-merge-missing.txt", env!("CARGO_TARGET_TMPDIR"));
    let (code, out, err) = run(&["merge", &base, &missing, &base]);
    assert_eq!((code, out.as_str(), err.trim_end().to_string()), (2, "", format!("error: cannot read {missing}")));
    let cases: Vec<Vec<&str>> = vec![vec!["merge"], vec!["merge", &base, &base], vec!["merge", &base, &base, &base, &base]];
    for args in cases {
        let (code, out, err) = run(&args);
        assert_eq!((code, out.as_str()), (2, ""), "args {args:?}");
        assert!(err.starts_with("usage:"), "args {args:?} stderr {err}");
    }
}
