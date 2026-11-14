use crate::diff::diff_lines;
use crate::error::Error;
use crate::hunk::{make_hunks, Hunk, HunkLine};
use crate::lines::{split_lines, strip_newline};

const NO_NEWLINE: &str = "\\ No newline at end of file";

/// A unified diff for one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    pub old_name: String,
    pub new_name: String,
    pub hunks: Vec<Hunk>,
}

/// Diffs two texts and groups the result into a patch.
pub fn make_patch(old_name: &str, old_text: &str, new_name: &str, new_text: &str, context: usize) -> Patch {
    let edits = diff_lines(&split_lines(old_text), &split_lines(new_text));
    Patch {
        old_name: old_name.to_string(),
        new_name: new_name.to_string(),
        hunks: make_hunks(&edits, context),
    }
}

/// Writes a patch in unified diff format.
pub fn format_patch(patch: &Patch) -> String {
    let mut out = format!("--- {}\n+++ {}\n", patch.old_name, patch.new_name);
    for h in &patch.hunks {
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            h.old_start, h.old_count, h.new_start, h.new_count
        ));
        for line in &h.lines {
            let prefix = match line {
                HunkLine::Context(_) => ' ',
                HunkLine::Removed(_) => '-',
                HunkLine::Added(_) => '+',
            };
            let (body, had_newline) = strip_newline(line.text());
            out.push(prefix);
            out.push_str(body);
            out.push('\n');
            if !had_newline {
                out.push_str(NO_NEWLINE);
                out.push('\n');
            }
        }
    }
    out
}

fn parse_number(s: &str) -> Option<usize> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn parse_range(s: &str) -> Option<(usize, usize)> {
    let (start, count) = s.split_once(',')?;
    let (start, count) = (parse_number(start)?, parse_number(count)?);
    if count > 0 && start == 0 {
        return None;
    }
    Some((start, count))
}

/// Parses `@@ -S,C +S,C @@`, optionally followed by a space and ignored text.
fn parse_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, tail) = rest.split_once(" @@")?;
    if !(tail.is_empty() || tail.starts_with(' ')) {
        return None;
    }
    let (old, new) = ranges.split_once(" +")?;
    let (os, oc) = parse_range(old)?;
    let (ns, nc) = parse_range(new)?;
    Some((os, oc, ns, nc))
}

/// Parses unified diff text for one file.
pub fn parse_patch(text: &str) -> Result<Patch, Error> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    let fail = |idx: usize, message: &str| Error::Parse { line: idx + 1, message: message.to_string() };
    let header = |idx: usize, prefix: &str| -> Result<String, Error> {
        lines
            .get(idx)
            .and_then(|l| l.strip_prefix(prefix))
            .map(str::to_string)
            .ok_or_else(|| fail(idx, &format!("expected '{prefix}'")))
    };
    let old_name = header(0, "--- ")?;
    let new_name = header(1, "+++ ")?;
    let mut hunks = Vec::new();
    let mut idx = 2;
    while idx < lines.len() {
        let (os, oc, ns, nc) = parse_header(lines[idx]).ok_or_else(|| fail(idx, "bad hunk header"))?;
        idx += 1;
        let (mut old_left, mut new_left) = (oc, nc);
        let mut body: Vec<HunkLine> = Vec::new();
        loop {
            let done = old_left == 0 && new_left == 0;
            let Some(line) = lines.get(idx) else {
                if done {
                    break;
                }
                return Err(fail(idx, "unexpected end of patch"));
            };
            if let Some(_marker) = line.strip_prefix('\\') {
                let last = body.last_mut().ok_or_else(|| fail(idx, "marker without a line"))?;
                let (HunkLine::Context(s) | HunkLine::Removed(s) | HunkLine::Added(s)) = last;
                if s.pop() != Some('\n') {
                    return Err(fail(idx, "repeated marker"));
                }
                idx += 1;
                continue;
            }
            if done {
                break;
            }
            let mut chars = line.chars();
            let kind = chars.next();
            let content = format!("{}\n", chars.as_str());
            let entry = match kind {
                Some(' ') if old_left > 0 && new_left > 0 => {
                    old_left -= 1;
                    new_left -= 1;
                    HunkLine::Context(content)
                }
                Some('-') if old_left > 0 => {
                    old_left -= 1;
                    HunkLine::Removed(content)
                }
                Some('+') if new_left > 0 => {
                    new_left -= 1;
                    HunkLine::Added(content)
                }
                _ => return Err(fail(idx, "bad hunk line")),
            };
            body.push(entry);
            idx += 1;
        }
        hunks.push(Hunk { old_start: os, old_count: oc, new_start: ns, new_count: nc, lines: body });
    }
    Ok(Patch { old_name, new_name, hunks })
}
