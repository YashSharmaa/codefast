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

/// Computes a minimal edit script: the number of deleted plus inserted lines is
/// as small as possible, and inside every run of changes the deletions come first.
pub fn diff_lines(old: &[String], new: &[String]) -> Vec<Edit> {
    let (n, m) = (old.len(), new.len());
    // lcs[i][j] = length of the longest common subsequence of old[i..] and new[j..]
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if old[i] == new[j] {
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
        if i < n && j < m && old[i] == new[j] && lcs[i][j] == lcs[i + 1][j + 1] + 1 {
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
