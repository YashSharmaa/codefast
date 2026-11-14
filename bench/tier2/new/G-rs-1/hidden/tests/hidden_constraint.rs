use verso::{Constraint, Error, Version};

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

fn m(c: &str, ver: &str) -> bool {
    Constraint::parse(c).unwrap().matches(&v(ver))
}

fn check(c: &str, yes: &[&str], no: &[&str]) {
    for ver in yes {
        assert!(m(c, ver), "{c} should match {ver}");
    }
    for ver in no {
        assert!(!m(c, ver), "{c} should not match {ver}");
    }
}

#[test]
fn plain_comparators() {
    check(">1.2.3", &["1.2.4", "2.0.0"], &["1.2.3", "1.0.0"]);
    check(">=1.2.3", &["1.2.3", "1.3.0"], &["1.2.2"]);
    check("<1.2.3", &["1.2.2", "0.0.1"], &["1.2.3", "1.2.4"]);
    check("<=1.2.3", &["1.2.3", "1.0.0"], &["1.2.4"]);
    check("=1.2.3", &["1.2.3"], &["1.2.4", "1.2.2"]);
    check("1.2.3", &["1.2.3"], &["1.2.4", "1.3.0"]);
}

#[test]
fn caret_with_nonzero_major() {
    check("^1.2.3", &["1.2.3", "1.9.0", "1.99.99"], &["1.2.2", "2.0.0", "0.9.0"]);
    check("^1.2", &["1.2.0", "1.5.1"], &["1.1.9", "2.0.0"]);
    check("^1", &["1.0.0", "1.9.9"], &["0.9.9", "2.0.0"]);
}

#[test]
fn caret_with_zero_major() {
    check("^0.2.3", &["0.2.3", "0.2.9"], &["0.2.2", "0.3.0", "1.0.0"]);
    check("^0.2", &["0.2.0", "0.2.7"], &["0.1.9", "0.3.0"]);
    check("^0", &["0.0.0", "0.9.9"], &["1.0.0"]);
}

#[test]
fn caret_with_zero_major_and_minor() {
    check("^0.0.3", &["0.0.3"], &["0.0.2", "0.0.4", "0.1.0"]);
    check("^0.0", &["0.0.0", "0.0.9"], &["0.1.0"]);
    check("^0.0.0", &["0.0.0"], &["0.0.1"]);
}

#[test]
fn tilde_ranges() {
    check("~1.2.3", &["1.2.3", "1.2.9"], &["1.2.2", "1.3.0"]);
    check("~1.2", &["1.2.0", "1.2.9"], &["1.1.0", "1.3.0"]);
    check("~1", &["1.0.0", "1.9.0"], &["2.0.0", "0.9.0"]);
    check("~0.2.3", &["0.2.5"], &["0.3.0"]);
    check("~0", &["0.0.0", "0.5.0"], &["1.0.0"]);
}

#[test]
fn star_matches_releases_only() {
    check("*", &["0.0.0", "1.2.3", "99.0.0"], &["1.0.0-alpha", "0.0.1-0"]);
}

#[test]
fn and_groups_and_or() {
    let c = ">=1.0.0, <2.0.0 || >=3.0.0, <3.5.0";
    check(c, &["1.0.0", "1.9.9", "3.0.0", "3.4.9"], &["0.9.0", "2.0.0", "2.5.0", "3.5.0"]);
    check("^1.2, <1.3.0 || 5.0.0", &["1.2.5", "5.0.0"], &["1.3.0", "1.4.0", "5.0.1"]);
    check(">1.0.0, <1.0.0", &[], &["1.0.0", "0.5.0", "2.0.0"]);
}

#[test]
fn prerelease_rule_basic() {
    prerelease_rule_exact_and_upper();
    check(">=1.0.0-alpha", &["1.0.0-alpha", "1.0.0-beta", "1.0.0", "1.1.0"], &["1.1.0-alpha", "0.9.0"]);
    check("<2.0.0", &["1.9.9"], &["2.0.0-alpha", "1.5.0-rc.1"]);
    check("^1.2.3-beta.2", &["1.2.3-beta.2", "1.2.3-beta.10", "1.2.3", "1.4.0"], &["1.2.3-beta.1", "1.2.4-alpha", "2.0.0-rc.1"]);
    check("~1.2.3-rc.1", &["1.2.3-rc.2", "1.2.8"], &["1.2.3-beta", "1.3.0"]);
}

fn prerelease_rule_exact_and_upper() {
    check("=1.0.0-rc.1", &["1.0.0-rc.1"], &["1.0.0-rc.2", "1.0.0"]);
    check("1.0.0-rc.1", &["1.0.0-rc.1"], &["1.0.0"]);
    check("<1.0.0-rc.1", &["1.0.0-beta", "0.9.0"], &["1.0.0-rc.1", "0.9.0-alpha"]);
    check("*, >=1.5.0-alpha", &["1.5.0-beta", "1.6.0"], &["1.6.0-beta", "1.4.0"]);
}

#[test]
fn prerelease_rule_is_per_group() {
    let c = ">=1.5.0-alpha, <1.5.0-alpha.2 || >=1.0.0";
    check(c, &["1.5.0-alpha", "1.5.0-alpha.1", "1.5.0", "1.2.0"], &["1.5.0-beta", "1.6.0-alpha"]);
    check("* || >=1.5.0-alpha", &["1.5.0-beta", "1.0.0"], &["1.4.0-beta"]);
}

#[test]
fn whitespace_is_allowed_where_stated() {
    let c = "  >= 1.2.3 ,<  2.0.0 ||   ^ 3 ";
    check(c, &["1.2.3", "1.9.0", "3.4.0"], &["2.0.0", "4.0.0", "1.2.2"]);
    check("~ 1.2", &["1.2.4"], &["1.3.0"]);
    check("= 1.0.0", &["1.0.0"], &["1.0.1"]);
}

#[test]
fn invalid_constraints() {
    let bad = [
        "", " ", ">=1.2", "=1", "<1", "^1.2-beta", "~1-rc", "1.0.0,", ", 1.0.0", "|| 1.0.0",
        "1.0.0 ||", "1.0.0 || || 2.0.0", ">>1.0.0", "=>1.0.0", "~", "^", "1.0.0 | 2.0.0", "^1.x",
        "*1", "1.0.0 2.0.0", "!=1.0.0", "^1.2.3.4", "v1.0.0", ">=1.0.0+build", "1. 0.0", "**",
    ];
    for s in bad {
        assert_eq!(Constraint::parse(s).err(), Some(Error::InvalidConstraint(s.to_string())), "input {s:?}");
    }
    let parsed: Result<Constraint, Error> = "^1.2".parse();
    assert!(parsed.is_ok());
}

#[test]
fn max_satisfying_picks_greatest_match() {
    let list: Vec<Version> = ["1.0.0", "1.4.2", "2.0.0", "1.5.0-beta", "1.4.10", "0.9.0"]
        .iter()
        .map(|s| v(s))
        .collect();
    let c = Constraint::parse("^1.0").unwrap();
    assert_eq!(c.max_satisfying(&list), Some(&v("1.4.10")));
    let c = Constraint::parse(">=1.5.0-alpha, <2.0.0").unwrap();
    assert_eq!(c.max_satisfying(&list), Some(&v("1.5.0-beta")));
    let c = Constraint::parse(">=3.0.0").unwrap();
    assert_eq!(c.max_satisfying(&list), None);
    assert_eq!(c.max_satisfying(&[]), None);
}
