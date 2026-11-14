use std::path::PathBuf;
use std::process::Command;

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
    p.push(format!("patchwork-hidden-{name}"));
    std::fs::write(&p, text).unwrap();
    p.to_string_lossy().into_owned()
}

#[test]
fn cli_diff_equal_and_different() {
    let a = file("d-a.txt", "a\nb\nc\n");
    let b = file("d-b.txt", "a\nB\nc\n");
    let same = file("d-same.txt", "a\nb\nc\n");
    assert_eq!(run(&["diff", &a, &same]), (0, String::new(), String::new()));
    let (code, out, _) = run(&["diff", &a, &b]);
    assert_eq!(code, 1);
    assert_eq!(out, format!("--- {a}\n+++ {b}\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n"));
}

#[test]
fn cli_diff_context_option() {
    let old: String = (1..=9).map(|i| format!("{i}\n")).collect();
    let a = file("u-a.txt", &old);
    let b = file("u-b.txt", &old.replace("5\n", "five\n"));
    let (code, out, _) = run(&["diff", &a, &b, "-U", "0"]);
    assert_eq!((code, out), (1, format!("--- {a}\n+++ {b}\n@@ -5,1 +5,1 @@\n-5\n+five\n")));
    let (_, out, _) = run(&["diff", &a, &b, "-U", "1"]);
    assert_eq!(out, format!("--- {a}\n+++ {b}\n@@ -4,3 +4,3 @@\n 4\n-5\n+five\n 6\n"));
    let (_, out, _) = run(&["diff", &a, &b]);
    assert!(out.contains("@@ -2,7 +2,7 @@\n 2\n"), "{out}");
    let (code, out, err) = run(&["diff", &a, &b, "-U", "x"]);
    assert_eq!((code, out.as_str(), err.trim_end()), (2, "", "error: invalid context: x"));
    let (code, _, err) = run(&["diff", &a, &b, "-U", "-1"]);
    assert_eq!((code, err.trim_end()), (2, "error: invalid context: -1"));
}

#[test]
fn cli_apply_prints_result_and_offsets() {
    let target = file("ap-target.txt", "new0\nnew1\none\ntwo\nthree\nfour");
    let patch = file(
        "ap.patch",
        "--- x\n+++ y\n@@ -1,3 +1,3 @@\n one\n-two\n+TWO\n three\n@@ -4,1 +4,1 @@\n-four\n\\ No newline at end of file\n+4\n",
    );
    let (code, out, err) = run(&["apply", &target, &patch]);
    assert_eq!(code, 0);
    assert_eq!(out, "new0\nnew1\none\nTWO\nthree\n4\n");
    assert_eq!(err, "hunk 1 offset 2\nhunk 2 offset 2\n");
    let exact = file("ap-exact.txt", "one\ntwo\nthree\nfour");
    assert_eq!(run(&["apply", &exact, &patch]), (0, "one\nTWO\nthree\n4\n".to_string(), String::new()));
}

#[test]
fn cli_apply_errors() {
    let target = file("ae-target.txt", "one\nthree\n");
    let patch = file("ae.patch", "--- x\n+++ y\n@@ -1,2 +1,2 @@\n one\n-two\n+TWO\n");
    let (code, out, err) = run(&["apply", &target, &patch]);
    assert_eq!((code, out.as_str(), err.trim_end()), (1, "", "error: hunk 1 failed"));
    let broken = file("ae-broken.patch", "--- x\n+++ y\n@@ -1,2 +1,2 @@\n one\n");
    let (code, _, err) = run(&["apply", &target, &broken]);
    assert_eq!(code, 2);
    assert!(err.starts_with("error: line 5: "), "{err}");
    let missing = format!("{}/patchwork-hidden-missing.txt", env!("CARGO_TARGET_TMPDIR"));
    let (code, _, err) = run(&["apply", &missing, &patch]);
    assert_eq!((code, err.trim_end().to_string()), (2, format!("error: cannot read {missing}")));
}

#[test]
fn cli_stat_and_reverse() {
    let patch = file(
        "sr.patch",
        "--- before.txt\n+++ after.txt\n@@ -1,3 +1,4 @@\n a\n-b\n+B\n+C\n c\n@@ -9,1 +10,0 @@\n-z\n",
    );
    assert_eq!(
        run(&["stat", &patch]),
        (0, "before.txt -> after.txt: 2 hunks, 2 added, 2 removed\n".to_string(), String::new())
    );
    let (code, out, _) = run(&["reverse", &patch]);
    assert_eq!(code, 0);
    assert_eq!(out, "--- after.txt\n+++ before.txt\n@@ -1,4 +1,3 @@\n a\n-B\n-C\n+b\n c\n@@ -10,0 +9,1 @@\n+z\n");
}

#[test]
fn cli_usage_errors() {
    let a = file("us-a.txt", "a\n");
    let cases: Vec<Vec<&str>> = vec![
        vec![],
        vec!["frobnicate"],
        vec!["diff", &a],
        vec!["diff", &a, &a, "-X", "3"],
        vec!["apply", &a],
        vec!["stat"],
        vec!["reverse"],
    ];
    for args in cases {
        let (code, out, err) = run(&args);
        assert_eq!((code, out.as_str()), (2, ""), "args {args:?}");
        assert!(err.starts_with("usage:"), "args {args:?} stderr {err}");
    }
}
