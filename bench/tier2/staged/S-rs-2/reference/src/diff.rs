use crate::lines::strip_newline;

/// One step of an edit script turning old lines into new lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Equal(String),
    Delete(String),
    Insert(String),
}

impl Edit {
    pub(crate) fn is_change(&self) -> bool {
        !matches!(self, Edit::Equal(_))
    }
}

/// How lines are compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Whitespace {
    /// Lines are equal when they are the same string.
    Exact,
    /// Spaces and tabs at the end of a line (before its `\n`) are ignored.
    IgnoreTrailing,
    /// Every space and tab is ignored.
    IgnoreAll,
}

impl Whitespace {
    /// The form of `line` that is compared under this rule.
    pub(crate) fn normalize(self, line: &str) -> String {
        let (body, newline) = strip_newline(line);
        let mut out = match self {
            Whitespace::Exact => body.to_string(),
            Whitespace::IgnoreTrailing => body.trim_end_matches([' ', '\t']).to_string(),
            Whitespace::IgnoreAll => body.chars().filter(|c| !matches!(c, ' ' | '\t')).collect(),
        };
        if newline {
            out.push('\n');
        }
        out
    }

    /// Whether two lists of lines are equal under this rule.
    pub(crate) fn same(self, a: &[String], b: &[String]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| self.normalize(x) == self.normalize(y))
    }
}

/// Computes a minimal edit script: the number of deleted plus inserted lines is
/// as small as possible, and inside every run of changes the deletions come first.
/// Lines are compared under `ws`, and an `Equal` edit carries the old line.
pub fn diff_lines(old: &[String], new: &[String], ws: Whitespace) -> Vec<Edit> {
    let (n, m) = (old.len(), new.len());
    let old_k: Vec<String> = old.iter().map(|l| ws.normalize(l)).collect();
    let new_k: Vec<String> = new.iter().map(|l| ws.normalize(l)).collect();
    // lcs[i][j] = length of the longest common subsequence of old[i..] and new[j..]
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old_k[i] == new_k[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut edits = Vec::new();
    let (mut dels, mut ins) = (Vec::new(), Vec::new());
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && old_k[i] == new_k[j] && lcs[i][j] == lcs[i + 1][j + 1] + 1 {
            edits.append(&mut dels);
            edits.append(&mut ins);
            edits.push(Edit::Equal(old[i].clone()));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lcs[i + 1][j] >= lcs[i][j + 1]) {
            dels.push(Edit::Delete(old[i].clone()));
            i += 1;
        } else {
            ins.push(Edit::Insert(new[j].clone()));
            j += 1;
        }
    }
    edits.append(&mut dels);
    edits.append(&mut ins);
    edits
}
