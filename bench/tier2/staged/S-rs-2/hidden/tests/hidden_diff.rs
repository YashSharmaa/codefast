use patchwork::{diff_lines, join_lines, make_hunks, make_patch, split_lines, Edit, Hunk, HunkLine, Whitespace};

fn ls(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn numbered(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("{i}\n")).collect()
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % bound
    }
}

fn random_lines(rng: &mut Lcg, max_len: u64, alphabet: u64) -> Vec<String> {
    let n = rng.next(max_len + 1) as usize;
    let mut out: Vec<String> = (0..n).map(|_| format!("{}\n", (b'a' + rng.next(alphabet) as u8) as char)).collect();
    if n > 0 && rng.next(3) == 0 {
        out[n - 1].pop();
    }
    out
}

fn lcs(a: &[String], b: &[String]) -> usize {
    let mut t = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            t[i][j] = if a[i - 1] == b[j - 1] { t[i - 1][j - 1] + 1 } else { t[i - 1][j].max(t[i][j - 1]) };
        }
    }
    t[a.len()][b.len()]
}

fn check_script(old: &[String], new: &[String], edits: &[Edit]) {
    let mut o = Vec::new();
    let mut n = Vec::new();
    let mut changes = 0;
    let mut seen_insert_in_run = false;
    for e in edits {
        match e {
            Edit::Equal(s) => {
                o.push(s.clone());
                n.push(s.clone());
                seen_insert_in_run = false;
            }
            Edit::Delete(s) => {
                assert!(!seen_insert_in_run, "delete after insert in a run: {edits:?}");
                o.push(s.clone());
                changes += 1;
            }
            Edit::Insert(s) => {
                n.push(s.clone());
                changes += 1;
                seen_insert_in_run = true;
            }
        }
    }
    assert_eq!(o, old);
    assert_eq!(n, new);
    assert_eq!(changes, old.len() + new.len() - 2 * lcs(old, new), "not minimal: {old:?} -> {new:?}");
}

#[test]
fn split_lines_keeps_newlines() {
    assert_eq!(split_lines("a\nb"), ls(&["a\n", "b"]));
    assert_eq!(split_lines("a\nb\n"), ls(&["a\n", "b\n"]));
    assert_eq!(split_lines("a\n\n"), ls(&["a\n", "\n"]));
    assert_eq!(split_lines("\n"), ls(&["\n"]));
    assert_eq!(split_lines("x"), ls(&["x"]));
    assert_eq!(split_lines("a\r\nb\r"), ls(&["a\r\n", "b\r"]));
    assert!(split_lines("").is_empty());
    for t in ["", "a", "a\n", "a\n\nb", "\n\n\n", "x\r\ny"] {
        assert_eq!(join_lines(&split_lines(t)), t);
    }
}

#[test]
fn diff_unique_script_exact() {
    let old = ls(&["a\n", "b\n", "c\n", "d\n"]);
    let new = ls(&["a\n", "x\n", "c\n", "d\n", "e\n"]);
    let want = vec![
        Edit::Equal("a\n".into()),
        Edit::Delete("b\n".into()),
        Edit::Insert("x\n".into()),
        Edit::Equal("c\n".into()),
        Edit::Equal("d\n".into()),
        Edit::Insert("e\n".into()),
    ];
    assert_eq!(diff_lines(&old, &new, Whitespace::Exact), want);
}

#[test]
fn diff_deletions_come_first_in_a_run() {
    let old = ls(&["a\n", "b\n", "c\n", "z\n"]);
    let new = ls(&["x\n", "y\n", "c\n", "w\n"]);
    let want = vec![
        Edit::Delete("a\n".into()),
        Edit::Delete("b\n".into()),
        Edit::Insert("x\n".into()),
        Edit::Insert("y\n".into()),
        Edit::Equal("c\n".into()),
        Edit::Delete("z\n".into()),
        Edit::Insert("w\n".into()),
    ];
    assert_eq!(diff_lines(&old, &new, Whitespace::Exact), want);
}

#[test]
fn diff_trailing_newline_matters() {
    let old = split_lines("a\nb");
    let new = split_lines("a\nb\n");
    let want = vec![Edit::Equal("a\n".into()), Edit::Delete("b".into()), Edit::Insert("b\n".into())];
    assert_eq!(diff_lines(&old, &new, Whitespace::Exact), want);
}

#[test]
fn diff_empty_and_identical_inputs() {
    assert!(diff_lines(&[], &[], Whitespace::Exact).is_empty());
    assert_eq!(diff_lines(&[], &ls(&["x\n", "y"]), Whitespace::Exact), vec![Edit::Insert("x\n".into()), Edit::Insert("y".into())]);
    assert_eq!(diff_lines(&ls(&["x\n"]), &[], Whitespace::Exact), vec![Edit::Delete("x\n".into())]);
    let same = numbered(5);
    let edits = diff_lines(&same, &same, Whitespace::Exact);
    assert_eq!(edits, same.iter().map(|s| Edit::Equal(s.clone())).collect::<Vec<_>>());
}

#[test]
fn diff_is_minimal_on_repeated_lines() {
    let mut rng = Lcg(7);
    for _ in 0..300 {
        let a = random_lines(&mut rng, 12, 3);
        let b = random_lines(&mut rng, 12, 3);
        check_script(&a, &b, &diff_lines(&a, &b, Whitespace::Exact));
    }
    let a = ls(&["a\n", "b\n", "a\n", "b\n", "a\n"]);
    let b = ls(&["b\n", "a\n", "b\n", "a\n", "b\n"]);
    check_script(&a, &b, &diff_lines(&a, &b, Whitespace::Exact));
}

#[test]
fn hunk_with_default_context() {
    let old = numbered(10);
    let mut new = old.clone();
    new[4] = "five\n".into();
    let hunks = make_hunks(&diff_lines(&old, &new, Whitespace::Exact), 3);
    assert_eq!(hunks.len(), 1);
    let h = &hunks[0];
    assert_eq!((h.old_start, h.old_count, h.new_start, h.new_count), (2, 7, 2, 7));
    let want = vec![
        HunkLine::Context("2\n".into()),
        HunkLine::Context("3\n".into()),
        HunkLine::Context("4\n".into()),
        HunkLine::Removed("5\n".into()),
        HunkLine::Added("five\n".into()),
        HunkLine::Context("6\n".into()),
        HunkLine::Context("7\n".into()),
        HunkLine::Context("8\n".into()),
    ];
    assert_eq!(h.lines, want);
}

#[test]
fn hunks_merge_when_gap_is_at_most_twice_context() {
    let old = numbered(20);
    let mut new = old.clone();
    new[2] = "three\n".into();
    new[9] = "ten\n".into();
    let hunks = make_hunks(&diff_lines(&old, &new, Whitespace::Exact), 3);
    assert_eq!(hunks.len(), 1);
    assert_eq!((hunks[0].old_start, hunks[0].old_count, hunks[0].new_start, hunks[0].new_count), (1, 13, 1, 13));
    let mut new = old.clone();
    new[2] = "three\n".into();
    new[10] = "eleven\n".into();
    let hunks = make_hunks(&diff_lines(&old, &new, Whitespace::Exact), 3);
    let heads: Vec<(usize, usize, usize, usize)> =
        hunks.iter().map(|h| (h.old_start, h.old_count, h.new_start, h.new_count)).collect();
    assert_eq!(heads, vec![(1, 6, 1, 6), (8, 7, 8, 7)]);
}

#[test]
fn hunks_with_other_context_sizes() {
    let old = numbered(12);
    let mut new = old.clone();
    new[3] = "four\n".into();
    new[7] = "eight\n".into();
    let heads = |c: usize| -> Vec<(usize, usize, usize, usize)> {
        make_hunks(&diff_lines(&old, &new, Whitespace::Exact), c)
            .iter()
            .map(|h| (h.old_start, h.old_count, h.new_start, h.new_count))
            .collect()
    };
    assert_eq!(heads(1), vec![(3, 3, 3, 3), (7, 3, 7, 3)]);
    assert_eq!(heads(2), vec![(2, 9, 2, 9)]);
    assert_eq!(heads(0), vec![(4, 1, 4, 1), (8, 1, 8, 1)]);
    assert_eq!(heads(100), vec![(1, 12, 1, 12)]);
}

#[test]
fn zero_count_ranges_use_the_line_before() {
    let old = ls(&["a\n", "b\n", "c\n"]);
    let ins = make_hunks(&diff_lines(&old, &ls(&["a\n", "x\n", "b\n", "c\n"]), Whitespace::Exact), 0);
    assert_eq!(ins, vec![Hunk { old_start: 1, old_count: 0, new_start: 2, new_count: 1, lines: vec![HunkLine::Added("x\n".into())] }]);
    let del = make_hunks(&diff_lines(&old, &ls(&["a\n", "c\n"]), Whitespace::Exact), 0);
    assert_eq!(del, vec![Hunk { old_start: 2, old_count: 1, new_start: 1, new_count: 0, lines: vec![HunkLine::Removed("b\n".into())] }]);
    let front = make_hunks(&diff_lines(&old, &ls(&["x\n", "a\n", "b\n", "c\n"]), Whitespace::Exact), 0);
    assert_eq!((front[0].old_start, front[0].old_count, front[0].new_start, front[0].new_count), (0, 0, 1, 1));
}

#[test]
fn empty_text_on_one_side() {
    let p = make_patch("o", "", "n", "x\ny\n", 3, Whitespace::Exact);
    assert_eq!(p.hunks.len(), 1);
    let h = &p.hunks[0];
    assert_eq!((h.old_start, h.old_count, h.new_start, h.new_count), (0, 0, 1, 2));
    let p = make_patch("o", "x\ny", "n", "", 3, Whitespace::Exact);
    let h = &p.hunks[0];
    assert_eq!((h.old_start, h.old_count, h.new_start, h.new_count), (1, 2, 0, 0));
    assert_eq!(h.lines, vec![HunkLine::Removed("x\n".into()), HunkLine::Removed("y".into())]);
    let p = make_patch("o", "same\n", "n", "same\n", 3, Whitespace::Exact);
    assert!(p.hunks.is_empty());
    assert_eq!((p.old_name.as_str(), p.new_name.as_str()), ("o", "n"));
    assert!(make_hunks(&[], 3).is_empty());
}

#[test]
fn hunk_contents_cover_every_change() {
    let mut rng = Lcg(99);
    for _ in 0..200 {
        let a = random_lines(&mut rng, 25, 4);
        let b = random_lines(&mut rng, 25, 4);
        let edits = diff_lines(&a, &b, Whitespace::Exact);
        let changes = edits.iter().filter(|e| !matches!(e, Edit::Equal(_))).count();
        for ctx in 0..3 {
            let hunks = make_hunks(&edits, ctx);
            let mut in_hunks = 0;
            for h in &hunks {
                let old_n = h.lines.iter().filter(|l| !matches!(l, HunkLine::Added(_))).count();
                let new_n = h.lines.iter().filter(|l| !matches!(l, HunkLine::Removed(_))).count();
                assert_eq!((h.old_count, h.new_count), (old_n, new_n));
                in_hunks += h.lines.iter().filter(|l| !matches!(l, HunkLine::Context(_))).count();
            }
            assert_eq!(in_hunks, changes);
        }
    }
}
