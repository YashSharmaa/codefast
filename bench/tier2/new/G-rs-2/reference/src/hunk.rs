use crate::diff::Edit;

/// One line of a hunk. The string keeps its trailing `\n` when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkLine {
    Context(String),
    Removed(String),
    Added(String),
}

impl HunkLine {
    pub fn text(&self) -> &str {
        match self {
            HunkLine::Context(s) | HunkLine::Removed(s) | HunkLine::Added(s) => s,
        }
    }

    pub(crate) fn on_old(&self) -> bool {
        !matches!(self, HunkLine::Added(_))
    }

    pub(crate) fn on_new(&self) -> bool {
        !matches!(self, HunkLine::Removed(_))
    }
}

/// A group of nearby changes with surrounding context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub lines: Vec<HunkLine>,
}

/// The start number written in a hunk header: the 1-based number of the first
/// line, or, for an empty range, the number of lines before it.
pub(crate) fn start_number(lines_before: usize, count: usize) -> usize {
    if count == 0 { lines_before } else { lines_before + 1 }
}

impl Hunk {
    /// Builds a hunk from its lines, given how many old and new lines precede it.
    pub(crate) fn from_lines(old_before: usize, new_before: usize, lines: Vec<HunkLine>) -> Hunk {
        let old_count = lines.iter().filter(|l| l.on_old()).count();
        let new_count = lines.iter().filter(|l| l.on_new()).count();
        Hunk {
            old_start: start_number(old_before, old_count),
            old_count,
            new_start: start_number(new_before, new_count),
            new_count,
            lines,
        }
    }

    /// The lines this hunk expects in the old text.
    pub fn old_lines(&self) -> Vec<String> {
        self.lines.iter().filter(|l| l.on_old()).map(|l| l.text().to_string()).collect()
    }

    /// The lines this hunk produces in the new text.
    pub fn new_lines(&self) -> Vec<String> {
        self.lines.iter().filter(|l| l.on_new()).map(|l| l.text().to_string()).collect()
    }
}

fn to_line(e: &Edit) -> HunkLine {
    match e {
        Edit::Equal(s) => HunkLine::Context(s.clone()),
        Edit::Delete(s) => HunkLine::Removed(s.clone()),
        Edit::Insert(s) => HunkLine::Added(s.clone()),
    }
}

/// Groups an edit script into hunks with `context` unchanged lines around each
/// change. Changes separated by at most `2 * context` unchanged lines share a hunk.
pub fn make_hunks(edits: &[Edit], context: usize) -> Vec<Hunk> {
    let changes: Vec<usize> = (0..edits.len()).filter(|&i| edits[i].is_change()).collect();
    let mut hunks = Vec::new();
    let mut k = 0;
    while k < changes.len() {
        let first = changes[k];
        let mut last = first;
        while k + 1 < changes.len() && changes[k + 1] - last - 1 <= 2 * context {
            k += 1;
            last = changes[k];
        }
        k += 1;
        let begin = first.saturating_sub(context);
        let end = (last + 1 + context).min(edits.len());
        let old_before = edits[..begin].iter().filter(|e| !matches!(e, Edit::Insert(_))).count();
        let new_before = edits[..begin].iter().filter(|e| !matches!(e, Edit::Delete(_))).count();
        let lines = edits[begin..end].iter().map(to_line).collect();
        hunks.push(Hunk::from_lines(old_before, new_before, lines));
    }
    hunks
}
