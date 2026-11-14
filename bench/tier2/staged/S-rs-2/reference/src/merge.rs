use crate::diff::{diff_lines, Edit, Whitespace};
use crate::lines::split_lines;

/// The result of a three-way merge: the merged text, with conflict markers
/// where the two sides disagree, and the number of conflicts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged {
    pub text: String,
    pub conflicts: usize,
}

/// One run of changes of one side: it replaces the base lines `start..end`
/// with `lines`.
struct Change {
    start: usize,
    end: usize,
    lines: Vec<String>,
    ours: bool,
}

/// The changes of an edit script from the base to one side.
fn changes(edits: &[Edit], ours: bool) -> Vec<Change> {
    let mut out = Vec::new();
    let mut current: Option<Change> = None;
    let mut base = 0;
    for e in edits {
        match e {
            Edit::Equal(_) => {
                out.extend(current.take());
                base += 1;
            }
            Edit::Delete(_) => {
                let c = current.get_or_insert(Change { start: base, end: base, lines: Vec::new(), ours });
                c.end += 1;
                base += 1;
            }
            Edit::Insert(s) => {
                let c = current.get_or_insert(Change { start: base, end: base, lines: Vec::new(), ours });
                c.lines.push(s.clone());
            }
        }
    }
    out.extend(current);
    out
}

/// One side's version of the base lines `start..end`: the base lines with that
/// side's changes (in order, all inside the range) applied.
fn version(base: &[String], start: usize, end: usize, changes: &[&Change]) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = start;
    for c in changes {
        out.extend_from_slice(&base[at..c.start]);
        out.extend(c.lines.iter().cloned());
        at = c.end;
    }
    out.extend_from_slice(&base[at..end]);
    out
}

/// Writes a marker line, starting a new line first when needed.
fn marker(out: &mut String, text: &str) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(text);
    out.push('\n');
}

/// Merges the changes from `base` to `ours` and from `base` to `theirs`,
/// comparing lines under `ws`.
/// Changes that touch or overlap form a group. A group changed by one side, or
/// changed the same way by both, is taken as it is. Any other group is a conflict.
pub fn merge3(base: &str, ours: &str, theirs: &str, ws: Whitespace) -> Merged {
    let base = split_lines(base);
    let mut all = changes(&diff_lines(&base, &split_lines(ours), ws), true);
    all.extend(changes(&diff_lines(&base, &split_lines(theirs), ws), false));
    all.sort_by_key(|c| (c.start, c.end));
    let mut out = String::new();
    let mut conflicts = 0;
    let mut at = 0;
    let mut i = 0;
    while i < all.len() {
        let start = all[i].start;
        let mut end = all[i].end;
        let mut j = i + 1;
        while j < all.len() && all[j].start <= end {
            end = end.max(all[j].end);
            j += 1;
        }
        let ours: Vec<&Change> = all[i..j].iter().filter(|c| c.ours).collect();
        let theirs: Vec<&Change> = all[i..j].iter().filter(|c| !c.ours).collect();
        out.push_str(&base[at..start].concat());
        let mine = version(&base, start, end, &ours);
        let other = version(&base, start, end, &theirs);
        if theirs.is_empty() || ws.same(&mine, &other) {
            out.push_str(&mine.concat());
        } else if ours.is_empty() {
            out.push_str(&other.concat());
        } else {
            conflicts += 1;
            marker(&mut out, "<<<<<<< ours");
            out.push_str(&mine.concat());
            marker(&mut out, "=======");
            out.push_str(&other.concat());
            marker(&mut out, ">>>>>>> theirs");
        }
        at = end;
        i = j;
    }
    out.push_str(&base[at..].concat());
    Merged { text: out, conflicts }
}
