use std::collections::BTreeMap;

use verso::{resolve, Constraint, Error, Registry, Version};

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

fn reqs(list: &[(&str, &str)]) -> Vec<(String, Constraint)> {
    list.iter()
        .map(|(n, c)| (n.to_string(), Constraint::parse(c).unwrap()))
        .collect()
}

fn solve(text: &str, root: &[(&str, &str)]) -> Result<Vec<(String, String)>, Error> {
    let reg = Registry::parse(text).unwrap();
    let out: BTreeMap<String, Version> = resolve(&reg, &reqs(root))?;
    Ok(out.into_iter().map(|(k, v)| (k, v.to_string())).collect())
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

fn parse_line(text: &str) -> usize {
    match Registry::parse(text) {
        Err(Error::Parse { line, .. }) => line,
        other => panic!("expected a parse error for {text:?}, got {other:?}"),
    }
}

const BASIC: &str = "\
# a small registry
[web 2.1.0]
http = ^1.2
log = >=0.3.0, <0.5.0

  [web 1.0.0]
http = ~1.0

[http 1.0.4]
[http 1.3.0]
log = ^0.4
[http 1.2.0]
[log 0.4.1]
   # indented comment
[log 0.3.9]
[log 0.5.0]
";

#[test]
fn registry_parse_contents() {
    registry_dependency_line_details();
    let reg = Registry::parse(BASIC).unwrap();
    assert_eq!(reg.packages(), vec!["http", "log", "web"]);
    assert!(reg.contains("web"));
    assert!(!reg.contains("nope"));
    assert_eq!(reg.versions("http"), vec![v("1.0.4"), v("1.2.0"), v("1.3.0")]);
    assert_eq!(reg.versions("nope"), Vec::<Version>::new());
    let deps = reg.dependencies("web", &v("2.1.0")).unwrap();
    let names: Vec<&str> = deps.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["http", "log"]);
    assert!(deps[1].1.matches(&v("0.4.9")));
    assert!(!deps[1].1.matches(&v("0.5.0")));
    assert_eq!(reg.dependencies("http", &v("1.2.0")).unwrap().len(), 0);
    assert!(reg.dependencies("http", &v("9.9.9")).is_none());
    assert!(reg.dependencies("zzz", &v("1.0.0")).is_none());
}

fn registry_dependency_line_details() {
    let text = "[a 1.0.0]\n  b   =   =1.0.0  \nc= >= 2.0.0 , < 3.0.0\n[b 1.0.0]\n[c 2.5.0]\r\n";
    let reg = Registry::parse(text).unwrap();
    let deps = reg.dependencies("a", &v("1.0.0")).unwrap();
    assert_eq!(deps[0].0, "b");
    assert!(deps[0].1.matches(&v("1.0.0")));
    assert!(!deps[0].1.matches(&v("1.0.1")));
    assert_eq!(deps[1].0, "c");
    assert!(deps[1].1.matches(&v("2.5.0")));
    assert_eq!(reg.versions("c"), vec![v("2.5.0")]);
    assert!(Registry::parse("").unwrap().packages().is_empty());
    assert!(Registry::parse("[my_pkg-2 0.1.0]\nother-x = *\n").is_ok());
}

#[test]
fn registry_errors_report_line_numbers() {
    assert_eq!(parse_line("# c\nb = ^1\n"), 2);
    assert_eq!(parse_line("[a 1.0.0]\n\n[a]\n"), 3);
    assert_eq!(parse_line("[a 1.0.0 extra]\n"), 1);
    assert_eq!(parse_line("[a 1.0.0\n"), 1);
    assert_eq!(parse_line("[a 1.0]\n"), 1);
    assert_eq!(parse_line("[a b 1.0.0]\n"), 1);
    assert_eq!(parse_line("[a! 1.0.0]\n"), 1);
    assert_eq!(parse_line("[a 1.0.0]\n[b 1.0.0]\n[a 1.0.0]\n"), 3);
    assert_eq!(parse_line("[a 1.0.0]\nb ^1\n"), 2);
    assert_eq!(parse_line("[a 1.0.0]\nb = ^1\nc = >=1\n"), 3);
    assert_eq!(parse_line("[a 1.0.0]\nb c = ^1\n"), 2);
    assert_eq!(parse_line("[a 1.0.0]\n = ^1\n"), 2);
    assert_eq!(parse_line("[a 1.0.0]\nb = ^1\n# x\nb = ^2\n"), 4);
    assert_eq!(parse_line("[a 1.0.0]\nb = ^1 # trailing comment\n"), 2);
}

#[test]
fn resolve_picks_highest_compatible() {
    let got = solve(BASIC, &[("web", "*")]).unwrap();
    assert_eq!(got, pairs(&[("http", "1.3.0"), ("log", "0.4.1"), ("web", "2.1.0")]));
}

#[test]
fn resolve_respects_root_constraint() {
    duplicate_root_requirements_intersect();
    empty_root_gives_empty_map();
    let got = solve(BASIC, &[("web", "<2.0.0")]).unwrap();
    assert_eq!(got, pairs(&[("http", "1.0.4"), ("web", "1.0.0")]));
    let got = solve(BASIC, &[("log", "*")]).unwrap();
    assert_eq!(got, pairs(&[("log", "0.5.0")]));
}

#[test]
fn resolve_backtracks_over_conflicts() {
    let text = "\
[a 2.0.0]
c = ^1
[a 1.0.0]
c = ^2
[b 2.0.0]
c = ^2
[b 1.0.0]
c = ^1
[c 1.0.0]
[c 2.0.0]
";
    let got = solve(text, &[("a", "*"), ("b", "*")]).unwrap();
    assert_eq!(got, pairs(&[("a", "2.0.0"), ("b", "1.0.0"), ("c", "1.0.0")]));
    let got = solve(text, &[("b", "*"), ("a", "<2.0.0")]).unwrap();
    assert_eq!(got, pairs(&[("a", "1.0.0"), ("b", "2.0.0"), ("c", "2.0.0")]));
}

#[test]
fn shared_dependency_must_satisfy_everyone() {
    let text = "\
[app 1.0.0]
x = ^1
y = ^1
[x 1.0.0]
z = >=1.0.0
[y 1.0.0]
z = <1.3.0
[z 1.0.0]
[z 1.2.0]
[z 1.5.0]
";
    let got = solve(text, &[("app", "1.0.0")]).unwrap();
    assert_eq!(got, pairs(&[("app", "1.0.0"), ("x", "1.0.0"), ("y", "1.0.0"), ("z", "1.2.0")]));
}

#[test]
fn unknown_root_package() {
    assert_eq!(solve(BASIC, &[("web", "*"), ("zz", "*"), ("yy", "*")]), Err(Error::UnknownPackage("zz".into())));
    assert_eq!(solve(BASIC, &[("aa", ">=9.0.0")]), Err(Error::UnknownPackage("aa".into())));
}

#[test]
fn no_solution_cases() {
    assert_eq!(solve(BASIC, &[("web", ">=3.0.0")]), Err(Error::NoSolution));
    assert_eq!(solve(BASIC, &[("log", "^0.4"), ("log", ">=0.5.0")]), Err(Error::NoSolution));
    let text = "[a 1.0.0]\nb = ^2\n[b 1.0.0]\n";
    assert_eq!(solve(text, &[("a", "*")]), Err(Error::NoSolution));
}

fn duplicate_root_requirements_intersect() {
    let got = solve(BASIC, &[("log", ">=0.3.0"), ("log", "<0.5.0")]).unwrap();
    assert_eq!(got, pairs(&[("log", "0.4.1")]));
}

#[test]
fn missing_dependency_package_makes_version_unusable() {
    let text = "\
[a 2.0.0]
ghost = *
[a 1.5.0]
z = ^1
[a 1.0.0]
[z 1.0.0]
q = ^9
[q 1.0.0]
";
    let got = solve(text, &[("a", "*")]).unwrap();
    assert_eq!(got, pairs(&[("a", "1.0.0")]));
}

#[test]
fn unreachable_packages_are_left_out() {
    let text = "\
[a 1.0.0]
b = ^1
[b 1.0.0]
[c 1.0.0]
[d 1.0.0]
a = ^1
";
    let got = solve(text, &[("a", "*")]).unwrap();
    assert_eq!(got, pairs(&[("a", "1.0.0"), ("b", "1.0.0")]));
}

#[test]
fn earlier_names_decide_first_and_absent_ranks_lowest() {
    let text = "\
[z 2.0.0]
b = *
[z 1.0.0]
a = *
[a 1.0.0]
[b 1.0.0]
";
    let got = solve(text, &[("z", "*")]).unwrap();
    assert_eq!(got, pairs(&[("a", "1.0.0"), ("z", "1.0.0")]));
    let got = solve(text, &[("z", "*"), ("b", "*")]).unwrap();
    assert_eq!(got, pairs(&[("a", "1.0.0"), ("b", "1.0.0"), ("z", "1.0.0")]));
}

#[test]
fn presence_of_an_earlier_name_beats_a_higher_later_version() {
    let text = "\
[m 1.0.0]
k = >=1.0.0
[k 2.0.0]
[k 1.0.0]
j = ^1
[j 1.0.0]
";
    let got = solve(text, &[("m", "*")]).unwrap();
    assert_eq!(got, pairs(&[("j", "1.0.0"), ("k", "1.0.0"), ("m", "1.0.0")]));
    let got = solve(text, &[("m", "*"), ("j", "*")]).unwrap();
    assert_eq!(got, pairs(&[("j", "1.0.0"), ("k", "2.0.0"), ("m", "1.0.0")]));
}

#[test]
fn cycles_and_self_dependencies() {
    let text = "\
[a 1.0.0]
b = ^1
[a 2.0.0]
b = ^2
[b 1.0.0]
a = ^1
[b 2.0.0]
a = <2.0.0
[s 1.0.0]
s = =1.0.0
[s 2.0.0]
s = =1.0.0
";
    let got = solve(text, &[("a", "*")]).unwrap();
    assert_eq!(got, pairs(&[("a", "1.0.0"), ("b", "1.0.0")]));
    let got = solve(text, &[("s", "*")]).unwrap();
    assert_eq!(got, pairs(&[("s", "1.0.0")]));
}

#[test]
fn prerelease_versions_in_registry() {
    let text = "[p 1.0.0]\n[p 2.0.0-beta.1]\n[p 2.0.0-beta.2]\n[p 1.1.0-rc.1]\n";
    assert_eq!(solve(text, &[("p", "*")]).unwrap(), pairs(&[("p", "1.0.0")]));
    assert_eq!(solve(text, &[("p", ">=2.0.0-alpha")]).unwrap(), pairs(&[("p", "2.0.0-beta.2")]));
    assert_eq!(solve(text, &[("p", "^1.1.0-rc.0")]).unwrap(), pairs(&[("p", "1.1.0-rc.1")]));
}

fn empty_root_gives_empty_map() {
    assert_eq!(solve(BASIC, &[]).unwrap(), Vec::<(String, String)>::new());
}
