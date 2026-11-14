use crate::error::Error;
use crate::hunk::{Hunk, HunkLine};
use crate::lines::split_lines;
use crate::patch::Patch;

/// The result of applying a patch: the new text and, per hunk, the offset
/// (actual position minus the position its header names) where it was applied
/// and the fuzz it needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub text: String,
    pub offsets: Vec<isize>,
    pub fuzz: Vec<usize>,
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

/// The number of `Context` lines at the start and at the end of a hunk. A hunk
/// made only of context has all of it at the start.
fn context_runs(h: &Hunk) -> (usize, usize) {
    let is_context = |l: &&HunkLine| matches!(l, HunkLine::Context(_));
    let lead = h.lines.iter().take_while(is_context).count();
    if lead == h.lines.len() {
        return (lead, 0);
    }
    (lead, h.lines.iter().rev().take_while(is_context).count())
}

/// Where a hunk went: the index of its first kept old line, how many old lines
/// it replaced, the lines it put there, its offset and its fuzz.
struct Placement {
    at: usize,
    len: usize,
    new: Vec<String>,
    offset: isize,
    fuzz: usize,
}

fn place(old: &[String], h: &Hunk, shift: isize, pos: usize, max_fuzz: usize) -> Option<Placement> {
    let (lead, trail) = context_runs(h);
    for fuzz in 0..=max_fuzz {
        let (front, back) = (fuzz.min(lead), fuzz.min(trail));
        let lines = &h.lines[front..h.lines.len() - back];
        let want: Vec<String> = lines.iter().filter(|l| l.on_old()).map(|l| l.text().to_string()).collect();
        let nominal = (nominal_index(h) + front) as isize;
        if let Some(hi) = old.len().checked_sub(want.len()) {
            if let Some(at) = find_position(old, &want, nominal + shift, pos, hi) {
                let new = lines.iter().filter(|l| l.on_new()).map(|l| l.text().to_string()).collect();
                return Some(Placement { at, len: want.len(), new, offset: at as isize - nominal, fuzz });
            }
        }
        if front == lead && back == trail {
            break;
        }
    }
    None
}

/// Applies `patch` to `text` without fuzz.
pub fn apply_patch(text: &str, patch: &Patch) -> Result<Applied, Error> {
    apply_patch_fuzzy(text, patch, 0)
}

/// Applies `patch` to `text`. Hunks are placed in order, each at the position its
/// header names shifted by the previous hunk's offset, or else at the nearest
/// position where its old lines match. A hunk that does not fit is tried again
/// with up to `max_fuzz` context lines dropped at each end.
pub fn apply_patch_fuzzy(text: &str, patch: &Patch, max_fuzz: usize) -> Result<Applied, Error> {
    let old = split_lines(text);
    let mut out: Vec<String> = Vec::new();
    let mut offsets = Vec::new();
    let mut fuzz = Vec::new();
    let mut pos = 0usize;
    let mut shift = 0isize;
    for (i, h) in patch.hunks.iter().enumerate() {
        let p = place(&old, h, shift, pos, max_fuzz).ok_or(Error::HunkFailed(i + 1))?;
        shift = p.offset;
        offsets.push(p.offset);
        fuzz.push(p.fuzz);
        out.extend_from_slice(&old[pos..p.at]);
        out.extend(p.new);
        pos = p.at + p.len;
    }
    out.extend_from_slice(&old[pos..]);
    Ok(Applied { text: out.concat(), offsets, fuzz })
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
