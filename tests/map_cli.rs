//! `codefast map`: a small Python project of three clear parts (billing, auth, report) with one
//! call from report into billing and one pair of auth files that import each other. The binary
//! runs in a sandbox (`packaging/support.rs`) and what it prints is checked.

#[path = "packaging/support.rs"]
mod support;

use serde_json::Value;
use support::*;

struct Project {
    dir: Dir,
    home: Dir,
}

impl Project {
    fn new() -> Project {
        let dir = scratch("map");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        for (rel, text) in FILES {
            write(&dir.join(rel), text);
        }
        Project { dir, home: scratch("map-home") }
    }

    fn run(&self, args: &[&str]) -> String {
        let (code, out, err) = run(&self.home, &self.dir, args, &[], None);
        assert_eq!(code, 0, "{args:?}: {err}");
        out
    }
}

const FILES: &[(&str, &str)] = &[
    (
        "billing/money.py",
        "def fmt_money(cents):\n    whole = cents // 100\n    return \"%d.%02d\" % (whole, cents % 100)\n\n\ndef add_money(a, b):\n    total = a + b\n    return total\n",
    ),
    (
        "billing/tax.py",
        "from billing.money import fmt_money, add_money\n\n\ndef tax_for(cents):\n    tax = cents // 5\n    return add_money(cents, tax)\n\n\ndef show_tax(cents):\n    return fmt_money(tax_for(cents))\n",
    ),
    (
        "billing/invoice.py",
        "from billing.money import fmt_money, add_money\nfrom billing.tax import tax_for\n\n\ndef invoice_total(items):\n    total = 0\n    for i in items:\n        total = add_money(total, tax_for(i))\n    return fmt_money(total)\n",
    ),
    (
        "billing/ledger.py",
        "from billing.money import fmt_money, add_money\nfrom billing.invoice import invoice_total\nfrom billing.tax import tax_for\n\n\ndef post_ledger(items):\n    t = invoice_total(items)\n    x = add_money(tax_for(1), 2)\n    return fmt_money(x), t\n",
    ),
    (
        "auth/session.py",
        "from auth.users import lookup_user\nfrom auth.tokens import check_token\n\n\ndef open_session(name, tok):\n    check_token(tok)\n    return lookup_user(name)\n",
    ),
    (
        "auth/users.py",
        "from auth.session import open_session\nfrom auth.tokens import make_token\n\n\ndef login(name):\n    tok = make_token(name)\n    return open_session(name, tok)\n\n\ndef lookup_user(name):\n    found = {\"name\": name}\n    return found\n",
    ),
    (
        "auth/tokens.py",
        "from auth.crypto import sign\n\n\ndef make_token(name):\n    return sign(name)\n\n\ndef check_token(tok):\n    return sign(tok) == tok\n",
    ),
    ("auth/crypto.py", "def sign(text):\n    out = text[::-1]\n    return out\n"),
    (
        "report/layout.py",
        "def pad(text, width):\n    out = text.ljust(width)\n    return out\n\n\ndef rule(width):\n    line = \"-\" * width\n    return line\n",
    ),
    (
        "report/table.py",
        "from report.layout import pad, rule\nfrom billing.money import fmt_money\n\n\ndef render_table(rows):\n    out = [rule(20)]\n    for r in rows:\n        out.append(pad(fmt_money(r), 20))\n    return out\n",
    ),
    (
        "report/charts.py",
        "from report.layout import pad, rule\nfrom report.table import render_table\n\n\ndef bar_chart(values):\n    head = render_table(values)\n    return head + [pad(\"#\" * v, 30) for v in values] + [rule(30)]\n",
    ),
    (
        "report/render.py",
        "from report.charts import bar_chart\nfrom report.table import render_table\nfrom report.layout import rule\n\n\ndef render(values):\n    a = render_table(values)\n    b = bar_chart(values)\n    return a + b + [rule(10)]\n",
    ),
];

/// The first line of each community: `N. name: ...`.
fn community_heads(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| {
            let digits = l.bytes().take_while(|b| b.is_ascii_digit()).count();
            digits > 0 && l[digits..].starts_with(". ")
        })
        .collect()
}

fn has(text: &str, needle: &str) {
    assert!(text.contains(needle), "expected `{needle}` in:\n{text}");
}

#[test]
fn three_parts_a_hub_a_coupling_and_a_cycle() {
    let p = Project::new();
    let out = p.run(&["map"]);
    has(&out, "12 files in 3 communities.");
    // The three parts, each named after its directory and holding its own four files.
    let heads = community_heads(&out);
    assert_eq!(heads.len(), 3, "{out}");
    let mut names: Vec<&str> = heads.iter().map(|l| l.split_once(". ").unwrap().1.split(':').next().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["auth", "billing", "report"], "{out}");
    for h in &heads {
        has(h, ": 4 files, ");
    }
    // The definition everything goes through.
    let hubs = out.split("Hubs, by the files that use them:\n").nth(1).expect("hubs section");
    let first_hub = hubs.lines().next().unwrap();
    has(first_hub, "fmt_money  billing/money.py:1  used by 4 files, 4 calls  [billing]");
    // The one call from report into billing.
    has(
        &out,
        "Unexpected couplings, the rarest first:\n  report -> billing: render_table report/table.py:5 calls fmt_money billing/money.py:1 (the only call between the two, across top-level directories)\n",
    );
    has(&out, "report: 4 files");
    has(&out, "calls into: billing 1");
    // The two auth files that import each other, with the line of each import.
    has(&out, "Import cycles: 1 group of files that import each other:\n  2 files: auth/session.py:1 -> auth/users.py:1 -> auth/session.py\n");
}

#[test]
fn the_same_bytes_every_time() {
    let p = Project::new();
    let a = p.run(&["map"]);
    let b = p.run(&["map"]);
    assert_eq!(a, b);
    assert_eq!(p.run(&["map", "--json"]), p.run(&["map", "--json"]));
    // Another checkout of the same files gives the same answer.
    let q = Project::new();
    assert_eq!(q.run(&["map"]), a);
}

#[test]
fn the_budget_is_kept_and_what_is_left_out_is_counted() {
    let p = Project::new();
    let full = p.run(&["map", "--budget", "0"]);
    let budget = 300;
    let out = p.run(&["map", "--budget", &budget.to_string()]);
    // The estimate every command uses: 2.8 bytes per token, plus one per line.
    let tokens: usize = out.lines().map(|l| (l.len() as f64 / 2.8).ceil() as usize + 1).sum();
    assert!(tokens <= budget, "{tokens} tokens for a budget of {budget}:\n{out}");
    let shown = community_heads(&out).len();
    assert!((1..3).contains(&shown), "{out}");
    has(&out, &format!("… {} more communit", 3 - shown));
    has(&out, "(raise --budget, or -p DIR for one part)");
    assert!(!full.contains("… "), "{full}");
}

#[test]
fn json_has_the_same_content() {
    let p = Project::new();
    let v: Value = serde_json::from_str(&p.run(&["map", "--json"])).expect("json");
    assert_eq!(v["files"], 12);
    let cs = v["communities"].as_array().unwrap();
    assert_eq!(cs.len(), 3);
    for c in cs {
        assert!(c["id"].is_u64() && c["name"].is_string() && c["files"] == 4 && c["definitions"].is_u64(), "{c}");
        assert!(c["top"].is_array() && c["calls_into"].is_array(), "{c}");
    }
    let hub = &v["hubs"][0];
    assert_eq!(
        (hub["name"].as_str(), hub["path"].as_str(), hub["line"].as_u64(), hub["files"].as_u64()),
        (Some("fmt_money"), Some("billing/money.py"), Some(1), Some(4))
    );
    let k = &v["couplings"][0];
    assert_eq!(k["caller"]["name"], "render_table");
    assert_eq!(k["callee"]["path"], "billing/money.py");
    assert_eq!(k["calls_between"], 1);
    let cyc = &v["cycles"][0];
    assert_eq!(cyc["files"], 2);
    let steps: Vec<(&str, u64)> = cyc["steps"].as_array().unwrap().iter().map(|s| (s["path"].as_str().unwrap(), s["line"].as_u64().unwrap())).collect();
    assert_eq!(steps, [("auth/session.py", 1), ("auth/users.py", 1)]);
    assert_eq!(v["left_out"]["communities"], 0);
}

#[test]
fn path_restricts_the_map_to_part_of_the_tree() {
    let p = Project::new();
    let out = p.run(&["map", "-p", "auth/"]);
    has(&out, "4 files in 1 community.");
    has(&out, "1. auth: 4 files");
    assert!(!out.contains("billing"), "{out}");
}

#[test]
fn html_is_one_file_that_loads_nothing() {
    let p = Project::new();
    let page = p.dir.join("map.html");
    let out = p.run(&["map", "--html", page.to_str().unwrap()]);
    has(&out, "3 communities drawn of 3");
    let html = std::fs::read_to_string(&page).unwrap();
    assert!(html.starts_with("<!doctype html>"));
    // Nothing from the network: no address at all apart from XML namespaces.
    for scheme in ["http://", "https://"] {
        for (i, _) in html.match_indices(scheme) {
            let rest = &html[i..];
            assert!(rest.starts_with("http://www.w3.org/"), "network reference at {i}: {}", &rest[..rest.len().min(80)]);
        }
    }
    for tag in ["<link", "src=", "@import", "url("] {
        assert!(!html.contains(tag), "the page refers to another file: {tag}");
    }
    // The embedded data parses and holds the communities.
    let start = html.find("<script id=\"data\" type=\"application/json\">").expect("data element") + "<script id=\"data\" type=\"application/json\">".len();
    let end = start + html[start..].find("</script>").unwrap();
    let data: Value = serde_json::from_str(&html[start..end]).expect("embedded json");
    assert_eq!(data["communities"].as_array().unwrap().len(), 3);
    assert_eq!(data["files"], 12);
    assert!(data["links"].as_array().unwrap().iter().any(|l| l[2] == 1), "{}", data["links"]);
    assert!(html[start..end].contains("fmt_money"));
    // Light and dark both have their colours.
    assert!(html.contains("prefers-color-scheme: dark"));
}
