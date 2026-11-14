use verso::{Error, Version};

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

#[test]
fn parse_fields_and_display() {
    let x = v("1.2.3");
    assert_eq!((x.major, x.minor, x.patch), (1, 2, 3));
    assert!(x.pre.is_empty());
    assert!(!x.is_prerelease());
    assert_eq!(x.to_string(), "1.2.3");
    let y = v("10.0.7-alpha.1");
    assert_eq!((y.major, y.minor, y.patch), (10, 0, 7));
    assert_eq!(y.pre, vec!["alpha".to_string(), "1".to_string()]);
    assert!(y.is_prerelease());
    assert_eq!(y.to_string(), "10.0.7-alpha.1");
    assert_eq!(Version::new(4, 5, 6), v("4.5.6"));
    let z: Version = "0.0.0".parse().unwrap();
    assert_eq!(z, Version::new(0, 0, 0));
}

#[test]
fn prerelease_hyphens_belong_to_identifiers() {
    let x = v("1.0.0-x-y.z");
    assert_eq!(x.pre, vec!["x-y".to_string(), "z".to_string()]);
    assert_eq!(v("1.0.0--").pre, vec!["-".to_string()]);
    assert_eq!(v("1.0.0-0.3.7").pre, vec!["0", "3", "7"]);
    assert_eq!(x.to_string(), "1.0.0-x-y.z");
}

#[test]
fn parse_rejects_invalid_text() {
    numbers_must_fit_u64();
    let bad = [
        "", "1", "1.2", "1.2.3.4", "01.2.3", "1.02.3", "1.2.03", "1.2.3-", "1.2.3-a..b",
        "1.2.3-01", "1.2.3+build", "1.2.3-rc.1+b", "v1.2.3", " 1.2.3", "1.2.3 ", "1.2.x",
        "1.2.-3", "1.2.3-a_b", "1..3", "-1.2.3", "1.2.3.",
    ];
    for s in bad {
        assert_eq!(Version::parse(s), Err(Error::InvalidVersion(s.to_string())), "input {s:?}");
    }
}

fn numbers_must_fit_u64() {
    assert_eq!(v("18446744073709551615.0.0").major, u64::MAX);
    assert!(Version::parse("18446744073709551616.0.0").is_err());
    assert!(Version::parse("1.0.0-18446744073709551616").is_err());
    assert!(Version::parse("1.0.0-18446744073709551615").is_ok());
}

fn core_parts_compare_numerically() {
    assert!(v("1.9.0") < v("1.10.0"));
    assert!(v("2.0.0") > v("1.99.99"));
    assert!(v("1.2.3") < v("1.2.4"));
    assert!(v("0.10.0") > v("0.9.9"));
    assert_eq!(v("3.3.3").cmp(&v("3.3.3")), std::cmp::Ordering::Equal);
}

#[test]
fn semver_precedence_chain() {
    core_parts_compare_numerically();
    let chain = [
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-alpha.beta",
        "1.0.0-beta",
        "1.0.0-beta.2",
        "1.0.0-beta.11",
        "1.0.0-rc.1",
        "1.0.0",
        "1.0.1-0",
        "1.0.1",
    ];
    for w in chain.windows(2) {
        assert!(v(w[0]) < v(w[1]), "{} < {}", w[0], w[1]);
        assert!(v(w[1]) > v(w[0]));
    }
}

#[test]
fn identifier_kinds_and_ascii_order() {
    sorting_a_list();
    assert!(v("1.0.0-9") < v("1.0.0-10"));
    assert!(v("1.0.0-999") < v("1.0.0-a"));
    assert!(v("1.0.0-B") < v("1.0.0-a"));
    assert!(v("1.0.0-alpha-2") > v("1.0.0-alpha"));
    assert!(v("1.0.0-a.b.c") > v("1.0.0-a.b"));
    assert!(v("1.0.0-10a") < v("1.0.0-9a"));
}

fn sorting_a_list() {
    let mut list: Vec<Version> = ["2.0.0", "1.0.0-rc.1", "1.0.0", "0.9.0", "1.0.0-beta", "1.0.0"]
        .iter()
        .map(|s| v(s))
        .collect();
    list.sort();
    let got: Vec<String> = list.iter().map(|x| x.to_string()).collect();
    assert_eq!(got, ["0.9.0", "1.0.0-beta", "1.0.0-rc.1", "1.0.0", "1.0.0", "2.0.0"]);
}

#[test]
fn error_display_text() {
    assert_eq!(Error::InvalidVersion("x".into()).to_string(), "invalid version: x");
    assert_eq!(Error::InvalidConstraint("y".into()).to_string(), "invalid constraint: y");
    assert_eq!(Error::Parse { line: 4, message: "bad".into() }.to_string(), "line 4: bad");
    assert_eq!(Error::UnknownPackage("p".into()).to_string(), "unknown package: p");
    assert_eq!(Error::NoSolution.to_string(), "no solution");
}
