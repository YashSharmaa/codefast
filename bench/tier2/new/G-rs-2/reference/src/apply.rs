use crate::error::Error;
use crate::hunk::{Hunk, HunkLine};
use crate::lines::split_lines;
use crate::patch::Patch;

/// The result of applying a patch: the new text and, per hunk, the offset
/// (actual position minus the position its header names) where it was applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub text: String,
    pub offsets: Vec<isize>,
}

/// The 0-based index in the old text where a hunk's old side nominally starts.
fn nominal_index(h: &Hunk) -> usize {
    if h.old_count == 0 { h.old_start } else { h.old_start.saturating_sub(1) }
}

fn matches_at(text: &[String], at: usize, want: &[String]) -> bool {
    text.get(at..at + want.len()).is_some_and(|slice| slice == want)
}

/// Finds the position nearest to `expected` (ties to the lower one) within
/// `lo..=hi` where `want` matches.
fn find_position(text: &[String], want: &[String], expected: isize, lo: usize, hi: usize) -> Option<usize> {
    if lo > hi {
        return None;
    }
    let mut candidates: Vec<usize> = (lo..=hi).filter(|&k| matches_at(text, k, want)).collect();
    candidates.sort_by_key(|&k| ((k as isize - expected).unsigned_abs(), k));
    candidates.first().copied()
}

/// Applies `patch` to `text`. Hunks are placed in order, each at the position its
/// header names shifted by the previous hunk's offset, or else at the nearest
/// position where its old lines match.
pub fn apply_patch(text: &str, patch: &Patch) -> Result<Applied, Error> {
    let old = split_lines(text);
    let mut out: Vec<String> = Vec::new();
    let mut offsets = Vec::new();
    let mut pos = 0usize;
    let mut shift = 0isize;
    for (i, h) in patch.hunks.iter().enumerate() {
        let want = h.old_lines();
        let nominal = nominal_index(h) as isize;
        let hi = old.len().checked_sub(want.len()).ok_or(Error::HunkFailed(i + 1))?;
        let at = find_position(&old, &want, nominal + shift, pos, hi).ok_or(Error::HunkFailed(i + 1))?;
        shift = at as isize - nominal;
        offsets.push(shift);
        out.extend_from_slice(&old[pos..at]);
        out.extend(h.new_lines());
        pos = at + want.len();
    }
    out.extend_from_slice(&old[pos..]);
    Ok(Applied { text: out.concat(), offsets })
}

/// Swaps the two sides of a patch. Inside every run of changed lines the
/// removed lines are put before the added ones, keeping their order.
pub fn reverse_patch(patch: &Patch) -> Patch {
    let hunks = patch
        .hunks
        .iter()
        .map(|h| {
            let mut lines = Vec::new();
            let (mut removed, mut added) = (Vec::new(), Vec::new());
            for line in &h.lines {
                match line {
                    HunkLine::Context(s) => {
                        lines.append(&mut removed);
                        lines.append(&mut added);
                        lines.push(HunkLine::Context(s.clone()));
                    }
                    HunkLine::Removed(s) => added.push(HunkLine::Added(s.clone())),
                    HunkLine::Added(s) => removed.push(HunkLine::Removed(s.clone())),
                }
            }
            lines.append(&mut removed);
            lines.append(&mut added);
            Hunk {
                old_start: h.new_start,
                old_count: h.new_count,
                new_start: h.old_start,
                new_count: h.old_count,
                lines,
            }
        })
        .collect();
    Patch {
        old_name: patch.new_name.clone(),
        new_name: patch.old_name.clone(),
        hunks,
    }
}
