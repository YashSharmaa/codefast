use patchwork::{
    apply_patch, format_patch, make_patch, parse_patch, reverse_patch, Applied, Error, Hunk, HunkLine, Patch, Whitespace,
};

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

fn parse_err_line(text: &str) -> usize {
    match parse_patch(text) {
        Err(Error::Parse { line, .. }) => line,
        other => panic!("expected parse error for {text:?}, got {other:?}"),
    }
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
        if n > 0 && self.next(3) == 0 {
            s.pop();
        }
        s
    }
}

const HANDWRITTEN: &str = "\
--- old.txt
+++ new.txt
@@ -1,3 +1,3 @@
 one
-two
+TWO
 three
@@ -10,2 +10,3 @@ section
 ten
+ten and a half
 eleven
\\ No newline at end of file
";

#[test]
fn format_simple_patch() {
    let p = make_patch("a.txt", "a\nb\nc\n", "b.txt", "a\nB\nc\n", 3, Whitespace::Exact);
    assert_eq!(format_patch(&p), "--- a.txt\n+++ b.txt\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n");
    let p = make_patch("x", "", "y", "1\n2\n", 3, Whitespace::Exact);
    assert_eq!(format_patch(&p), "--- x\n+++ y\n@@ -0,0 +1,2 @@\n+1\n+2\n");
    let p = make_patch("x", "same\n", "y", "same\n", 3, Whitespace::Exact);
    assert_eq!(format_patch(&p), "--- x\n+++ y\n");
}

#[test]
fn format_no_newline_markers() {
    let p = make_patch("o", "a\nb", "n", "a\nb\n", 3, Whitespace::Exact);
    assert_eq!(
        format_patch(&p),
        "--- o\n+++ n\n@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+b\n"
    );
    let p = make_patch("o", "a\nb\n", "n", "a\nc", 3, Whitespace::Exact);
    assert_eq!(
        format_patch(&p),
        "--- o\n+++ n\n@@ -1,2 +1,2 @@\n a\n-b\n+c\n\\ No newline at end of file\n"
    );
    let p = make_patch("o", "x\nend", "n", "y\nend", 3, Whitespace::Exact);
    assert_eq!(
        format_patch(&p),
        "--- o\n+++ n\n@@ -1,2 +1,2 @@\n-x\n+y\n end\n\\ No newline at end of file\n"
    );
}

#[test]
fn format_empty_lines_and_two_hunks() {
    let old: String = (1..=12).map(|i| if i == 6 { "\n".to_string() } else { format!("l{i}\n") }).collect();
    let new = old.replace("l2\n", "L2\n").replace("l11\n", "");
    let p = make_patch("o", &old, "n", &new, 1, Whitespace::Exact);
    assert_eq!(
        format_patch(&p),
        "--- o\n+++ n\n@@ -1,3 +1,3 @@\n l1\n-l2\n+L2\n l3\n@@ -10,3 +10,2 @@\n l10\n-l11\n l12\n"
    );
    let p = make_patch("o", &old, "n", &old.replace("\n\n", "\nblank\n"), 0, Whitespace::Exact);
    assert_eq!(format_patch(&p), "--- o\n+++ n\n@@ -6 +6 @@\n-\n+blank\n");
}

#[test]
fn parse_handwritten_patch() {
    let p = parse_patch(HANDWRITTEN).unwrap();
    let want = Patch {
        old_name: "old.txt".into(),
        new_name: "new.txt".into(),
        hunks: vec![
            hunk(1, 3, 1, 3, vec![c("one\n"), r("two\n"), a("TWO\n"), c("three\n")]),
            hunk(10, 2, 10, 3, vec![c("ten\n"), a("ten and a half\n"), c("eleven")]),
        ],
    };
    assert_eq!(p, want);
}

#[test]
fn parse_details() {
    let p = parse_patch("--- a b\t2020\n+++ \n@@ -0,0 +1,1 @@\n+x").unwrap();
    assert_eq!(p.old_name, "a b\t2020");
    assert_eq!(p.new_name, "");
    assert_eq!(p.hunks, vec![hunk(0, 0, 1, 1, vec![a("x\n")])]);
    let p = parse_patch("--- a\n+++ b\n").unwrap();
    assert!(p.hunks.is_empty());
    let p = parse_patch("--- a\n+++ b\n@@ -3,2 +3,1 @@\n \n-p\n\\ whatever\n").unwrap();
    assert_eq!(p.hunks, vec![hunk(3, 2, 3, 1, vec![c("\n"), r("p")])]);
    let p = parse_patch("--- a\n+++ b\n@@ -1,1 +1,1 @@\n-x\n\\ No newline at end of file\n+y\n@@ -7,0 +8,1 @@\n+z\n").unwrap();
    assert_eq!(p.hunks, vec![hunk(1, 1, 1, 1, vec![r("x"), a("y\n")]), hunk(7, 0, 8, 1, vec![a("z\n")])]);
}

#[test]
fn parse_errors_name_the_line() {
    assert_eq!(parse_err_line(""), 1);
    assert_eq!(parse_err_line("+++ a\n--- b\n"), 1);
    assert_eq!(parse_err_line("--- a\n"), 2);
    assert_eq!(parse_err_line("--- a\n+++b\n"), 2);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1, +1 @@\n x\n"), 3);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1,1 +1,1\n x\n"), 3);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1,1 +1,1 @@x\n x\n"), 3);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -a,1 +1,1 @@\n x\n"), 3);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -0,1 +1,1 @@\n x\n"), 3);
    assert_eq!(parse_err_line("--- a\n+++ b\nhello\n"), 3);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1,1 +1,1 @@\n x\n y\n"), 5);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1,2 +1,2 @@\n x\n"), 5);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1,2 +1,2 @@\n x"), 5);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1,1 +1,1 @@\n\n"), 4);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1,1 +1,1 @@\n*x\n"), 4);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1,1 +1,2 @@\n-a\n-b\n"), 5);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1,1 +1,1 @@\n\\ No newline at end of file\n x\n"), 4);
    assert_eq!(parse_err_line("--- a\n+++ b\n@@ -1,1 +1,1 @@\n x\n\\ one\n\\ two\n"), 6);
}

#[test]
fn format_parse_round_trip() {
    let mut rng = Lcg(2024);
    for _ in 0..150 {
        let (t1, t2) = (rng.text(20, 4), rng.text(20, 4));
        for ctx in [0, 1, 3] {
            let p = make_patch("left", &t1, "right", &t2, ctx, Whitespace::Exact);
            assert_eq!(parse_patch(&format_patch(&p)).unwrap(), p);
        }
    }
    assert_eq!(format_patch(&parse_patch(HANDWRITTEN).unwrap()), HANDWRITTEN.replace(" section", ""));
}

#[test]
fn apply_at_exact_position() {
    let p = parse_patch(HANDWRITTEN).unwrap();
    let text = "one\ntwo\nthree\n4\n5\n6\n7\n8\n9\nten\neleven";
    let got = apply_patch(text, &p).unwrap();
    assert_eq!(
        got,
        Applied {
            text: "one\nTWO\nthree\n4\n5\n6\n7\n8\n9\nten\nten and a half\neleven".into(),
            offsets: vec![0, 0],
            fuzz: vec![0, 0]
        }
    );
}

#[test]
fn apply_finds_moved_hunks() {
    let p = parse_patch(HANDWRITTEN).unwrap();
    let text = "new0\nnew1\none\ntwo\nthree\n4\n5\n6\n7\n8\n9\nten\neleven";
    let got = apply_patch(text, &p).unwrap();
    assert_eq!(got.offsets, vec![2, 2]);
    assert_eq!(got.text, "new0\nnew1\none\nTWO\nthree\n4\n5\n6\n7\n8\n9\nten\nten and a half\neleven");
    let text = "one\ntwo\nthree\n4\n5\n6\nten\neleven";
    let got = apply_patch(text, &p).unwrap();
    assert_eq!(got.offsets, vec![0, -3]);
    assert_eq!(got.text, "one\nTWO\nthree\n4\n5\n6\nten\nten and a half\neleven");
}

#[test]
fn apply_carries_previous_offset() {
    let p = patch(vec![hunk(1, 1, 1, 1, vec![r("a\n"), a("A\n")]), hunk(6, 1, 6, 1, vec![r("q\n"), a("Q\n")])]);
    let text = "X\nY\na\nb\nc\nq\nd\nq\ne\n";
    let got = apply_patch(text, &p).unwrap();
    assert_eq!(got.offsets, vec![2, 2]);
    assert_eq!(got.text, "X\nY\nA\nb\nc\nq\nd\nQ\ne\n");
}

#[test]
fn apply_ties_go_to_the_lower_position() {
    let p = patch(vec![hunk(3, 1, 3, 1, vec![r("q\n"), a("Q\n")])]);
    let got = apply_patch("a\nq\nb\nq\nc\n", &p).unwrap();
    assert_eq!(got.offsets, vec![-1]);
    assert_eq!(got.text, "a\nQ\nb\nq\nc\n");
    let got = apply_patch("a\nb\nc\nq\nq\n", &p).unwrap();
    assert_eq!(got.offsets, vec![1]);
    assert_eq!(got.text, "a\nb\nc\nQ\nq\n");
}

#[test]
fn apply_failures_name_the_hunk() {
    let p = parse_patch(HANDWRITTEN).unwrap();
    assert_eq!(apply_patch("one\ntwo\nthree\nten\nzzz\n", &p), Err(Error::HunkFailed(2)));
    assert_eq!(apply_patch("one\nthree\n", &p), Err(Error::HunkFailed(1)));
    assert_eq!(apply_patch("", &p), Err(Error::HunkFailed(1)));
    assert_eq!(Error::HunkFailed(2).to_string(), "hunk 2 failed");
    assert_eq!(Error::Parse { line: 3, message: "m".into() }.to_string(), "line 3: m");
}

#[test]
fn apply_keeps_hunks_in_order() {
    let p = patch(vec![hunk(3, 1, 3, 1, vec![r("c\n"), a("C\n")]), hunk(1, 1, 1, 1, vec![r("a\n"), a("A\n")])]);
    assert_eq!(apply_patch("a\nb\nc\n", &p), Err(Error::HunkFailed(2)));
    let p = patch(vec![hunk(2, 1, 2, 1, vec![r("x\n"), a("1\n")]), hunk(2, 1, 2, 1, vec![r("x\n"), a("2\n")])]);
    let got = apply_patch("x\nx\nx\n", &p).unwrap();
    assert_eq!(got.text, "x\n1\n2\n");
    assert_eq!(got.offsets, vec![0, 1]);
}

#[test]
fn apply_checks_trailing_newlines() {
    let p = parse_patch("--- a\n+++ b\n@@ -2,1 +2,1 @@\n-b\n\\ No newline at end of file\n+B\n").unwrap();
    assert_eq!(apply_patch("a\nb\n", &p), Err(Error::HunkFailed(1)));
    assert_eq!(apply_patch("a\nb", &p).unwrap().text, "a\nB\n");
}

#[test]
fn apply_pure_insertions() {
    let p = patch(vec![hunk(2, 0, 3, 2, vec![a("x\n"), a("y\n")])]);
    assert_eq!(apply_patch("a\nb\nc\n", &p).unwrap(), Applied { text: "a\nb\nx\ny\nc\n".into(), offsets: vec![0], fuzz: vec![0] });
    let got = apply_patch("a\n", &p).unwrap();
    assert_eq!(got, Applied { text: "a\nx\ny\n".into(), offsets: vec![-1], fuzz: vec![0] });
    let p = patch(vec![hunk(0, 0, 1, 1, vec![a("top\n")])]);
    assert_eq!(apply_patch("", &p).unwrap().text, "top\n");
    assert_eq!(apply_patch("a\n", &p).unwrap().text, "top\na\n");
}

#[test]
fn apply_and_reverse_round_trip() {
    let mut rng = Lcg(31337);
    for _ in 0..150 {
        let (t1, t2) = (rng.text(30, 3), rng.text(30, 3));
        for ctx in [0, 1, 2, 3] {
            let p = make_patch("l", &t1, "r", &t2, ctx, Whitespace::Exact);
            let fwd = apply_patch(&t1, &p).unwrap();
            assert_eq!(fwd.text, t2);
            assert!(fwd.offsets.iter().all(|&o| o == 0));
            let rev = reverse_patch(&p);
            let back = apply_patch(&t2, &rev).unwrap();
            assert_eq!(back.text, t1);
            assert!(back.offsets.iter().all(|&o| o == 0));
            assert_eq!(reverse_patch(&rev), p);
        }
    }
}

#[test]
fn reverse_swaps_sides_and_orders_runs() {
    let p = patch(vec![
        hunk(4, 5, 6, 5, vec![c("k\n"), r("a\n"), r("b\n"), a("X\n"), c("m\n"), r("z\n"), a("Y\n"), a("W")]),
        hunk(20, 0, 19, 1, vec![a("n\n")]),
    ]);
    let rev = reverse_patch(&p);
    assert_eq!((rev.old_name.as_str(), rev.new_name.as_str()), ("new", "old"));
    assert_eq!(
        rev.hunks,
        vec![
            hunk(6, 5, 4, 5, vec![c("k\n"), r("X\n"), a("a\n"), a("b\n"), c("m\n"), r("Y\n"), r("W"), a("z\n")]),
            hunk(19, 1, 20, 0, vec![r("n\n")]),
        ]
    );
}
