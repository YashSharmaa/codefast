use std::path::PathBuf;
use std::process::Command;

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_verso"))
        .args(args)
        .output()
        .expect("run verso");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn registry_file(name: &str, text: &str) -> String {
    let mut p = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    p.push(format!("verso-hidden-{name}.txt"));
    std::fs::write(&p, text).unwrap();
    p.to_string_lossy().into_owned()
}

const REG: &str = "\
[app 1.0.0]
lib = ^2
util = >=0.3.0, <0.5.0
[app 0.9.0]
[lib 2.4.1]
util = ^0.4
[lib 3.0.0]
[util 0.4.2]
[util 0.5.0]
";

#[test]
fn cli_match() {
    assert_eq!(run(&["match", "1.4.0", "^1.2"]), (0, "yes\n".to_string(), String::new()));
    assert_eq!(run(&["match", "2.0.0", "^1.2"]).1, "no\n");
    assert_eq!(run(&["match", "1.0.0-beta", ">=1.0.0-alpha, <1.0.0"]).1, "yes\n");
    assert_eq!(run(&["match", "1.0.0-beta", "*"]).1, "no\n");
}

#[test]
fn cli_max() {
    let (code, out, _) = run(&["max", "~1.2", "1.2.0", "1.3.0", "1.2.10", "1.2.9"]);
    assert_eq!((code, out.as_str()), (0, "1.2.10\n"));
    assert_eq!(run(&["max", ">=5.0.0", "1.0.0"]).1, "none\n");
    assert_eq!(run(&["max", "*"]), (0, "none\n".to_string(), String::new()));
}

#[test]
fn cli_sort() {
    let (code, out, _) = run(&["sort", "1.0.0", "1.0.0-rc.1", "0.10.0", "0.9.0", "1.0.0-rc.1"]);
    assert_eq!(code, 0);
    assert_eq!(out, "0.9.0\n0.10.0\n1.0.0-rc.1\n1.0.0-rc.1\n1.0.0\n");
}

#[test]
fn cli_resolve_prints_sorted_lines() {
    let path = registry_file("ok", REG);
    let (code, out, err) = run(&["resolve", &path, "app@*"]);
    assert_eq!(code, 0, "stderr: {err}");
    assert_eq!(out, "app 1.0.0\nlib 2.4.1\nutil 0.4.2\n");
    let (code, out, _) = run(&["resolve", &path, "util@*", "app@<1.0.0"]);
    assert_eq!(code, 0);
    assert_eq!(out, "app 0.9.0\nutil 0.5.0\n");
    assert_eq!(run(&["resolve", &path]), (0, String::new(), String::new()));
}

#[test]
fn cli_resolve_failures() {
    let path = registry_file("fail", REG);
    let (code, out, err) = run(&["resolve", &path, "app@>=1.0.0", "util@^0.5"]);
    assert_eq!((code, out.as_str(), err.trim_end()), (1, "", "error: no solution"));
    let (code, _, err) = run(&["resolve", &path, "nope@*"]);
    assert_eq!((code, err.trim_end()), (2, "error: unknown package: nope"));
    let (code, _, err) = run(&["resolve", &path, "app"]);
    assert_eq!((code, err.trim_end()), (2, "error: invalid requirement: app"));
    let (code, _, err) = run(&["resolve", &path, "app@>=1"]);
    assert_eq!((code, err.trim_end()), (2, "error: invalid constraint: >=1"));
    let bad = registry_file("bad", "[app 1.0.0]\n\nlib ^2\n");
    let (code, _, err) = run(&["resolve", &bad, "app@*"]);
    assert_eq!(code, 2);
    assert!(err.starts_with("error: line 3: "), "stderr: {err}");
    let missing = format!("{}/verso-hidden-missing.txt", env!("CARGO_TARGET_TMPDIR"));
    let (code, _, err) = run(&["resolve", &missing, "app@*"]);
    assert_eq!((code, err.trim_end().to_string()), (2, format!("error: cannot read {missing}")));
}

#[test]
fn cli_usage_and_parse_errors() {
    let (code, out, err) = run(&["match", "1.0", "*"]);
    assert_eq!((code, out.as_str(), err.trim_end()), (2, "", "error: invalid version: 1.0"));
    let (code, _, err) = run(&["max", "^1", "1.0.0", "x"]);
    assert_eq!((code, err.trim_end()), (2, "error: invalid version: x"));
    for args in [&[][..], &["frobnicate"][..], &["match", "1.0.0"][..], &["max"][..], &["resolve"][..]] {
        let (code, _, err) = run(args);
        assert_eq!(code, 2, "args {args:?}");
        assert!(err.starts_with("usage:"), "args {args:?} stderr {err}");
    }
}
