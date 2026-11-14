use crate::diff::{diff_lines, Whitespace};
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

/// Diffs two texts, comparing lines under `ws`, and groups the result into a patch.
pub fn make_patch(
    old_name: &str,
    old_text: &str,
    new_name: &str,
    new_text: &str,
    context: usize,
    ws: Whitespace,
) -> Patch {
    let edits = diff_lines(&split_lines(old_text), &split_lines(new_text), ws);
    Patch {
        old_name: old_name.to_string(),
        new_name: new_name.to_string(),
        hunks: make_hunks(&edits, context),
    }
}

/// One side of a hunk header: `S,C`, or just `S` when the count is 1.
fn format_range(start: usize, count: usize) -> String {
    if count == 1 { start.to_string() } else { format!("{start},{count}") }
}

/// Writes a patch in unified diff format.
pub fn format_patch(patch: &Patch) -> String {
    let mut out = format!("--- {}\n+++ {}\n", patch.old_name, patch.new_name);
    for h in &patch.hunks {
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            format_range(h.old_start, h.old_count),
            format_range(h.new_start, h.new_count)
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

/// Writes several file patches one after another.
pub fn format_patches(patches: &[Patch]) -> String {
    patches.iter().map(format_patch).collect()
}

/// Parses `S,C`, or `S` alone for a count of 1.
fn parse_range(s: &str) -> Option<(usize, usize)> {
    let (start, count) = match s.split_once(',') {
        Some((start, count)) => (parse_number(start)?, parse_number(count)?),
        None => (parse_number(s)?, 1),
    };
    if count > 0 && start == 0 {
        return None;
    }
    Some((start, count))
}

/// Parses `@@ -S,C +S,C @@` (each count optional), optionally followed by a
/// space and ignored text.
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

fn fail(idx: usize, message: &str) -> Error {
    Error::Parse { line: idx + 1, message: message.to_string() }
}

/// Splits patch text at `\n`. A final `\n` does not start another line.
fn text_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Which sides of a file patch already have a line without `\n`. Nothing more
/// may come on such a side.
#[derive(Default)]
struct Ended {
    old: bool,
    new: bool,
}

/// Reads a hunk body starting at `lines[idx]` until both counts are used up.
/// Returns the hunk lines and the index after the body.
fn parse_body(
    lines: &[&str],
    mut idx: usize,
    old_count: usize,
    new_count: usize,
    ended: &mut Ended,
) -> Result<(Vec<HunkLine>, usize), Error> {
    let (mut old_left, mut new_left) = (old_count, new_count);
    let mut body: Vec<HunkLine> = Vec::new();
    loop {
        let done = old_left == 0 && new_left == 0;
        let Some(line) = lines.get(idx) else {
            if done {
                break;
            }
            return Err(fail(idx, "unexpected end of patch"));
        };
        if line.starts_with('\\') {
            let last = body.last_mut().ok_or_else(|| fail(idx, "marker without a line"))?;
            let (HunkLine::Context(s) | HunkLine::Removed(s) | HunkLine::Added(s)) = last;
            if s.pop() != Some('\n') {
                return Err(fail(idx, "repeated marker"));
            }
            ended.old |= last.on_old();
            ended.new |= last.on_new();
            idx += 1;
            continue;
        }
        if done {
            break;
        }
        let mut chars = line.chars();
        let kind = chars.next();
        let after_end = match kind {
            Some(' ') => ended.old || ended.new,
            Some('-') => ended.old,
            Some('+') => ended.new,
            _ => false,
        };
        if after_end {
            return Err(fail(idx, "line after the end of the file"));
        }
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
    Ok((body, idx))
}

/// Parses the file patch that starts at `lines[idx]`. With `several`, a line
/// starting with `--- ` where a hunk header is expected ends it. Returns the
/// patch and the index of the line after it.
fn parse_file(lines: &[&str], start: usize, several: bool) -> Result<(Patch, usize), Error> {
    let header = |idx: usize, prefix: &str| -> Result<String, Error> {
        lines
            .get(idx)
            .and_then(|l| l.strip_prefix(prefix))
            .map(str::to_string)
            .ok_or_else(|| fail(idx, &format!("expected '{prefix}'")))
    };
    let old_name = header(start, "--- ")?;
    let new_name = header(start + 1, "+++ ")?;
    let mut hunks = Vec::new();
    let mut ended = Ended::default();
    let mut idx = start + 2;
    while idx < lines.len() {
        if several && lines[idx].starts_with("--- ") {
            break;
        }
        let (os, oc, ns, nc) = parse_header(lines[idx]).ok_or_else(|| fail(idx, "bad hunk header"))?;
        let (body, next) = parse_body(lines, idx + 1, oc, nc, &mut ended)?;
        hunks.push(Hunk { old_start: os, old_count: oc, new_start: ns, new_count: nc, lines: body });
        idx = next;
    }
    Ok((Patch { old_name, new_name, hunks }, idx))
}

/// Parses unified diff text for one file.
pub fn parse_patch(text: &str) -> Result<Patch, Error> {
    Ok(parse_file(&text_lines(text), 0, false)?.0)
}

/// Parses unified diff text for one or more files, one after another.
pub fn parse_patches(text: &str) -> Result<Vec<Patch>, Error> {
    let lines = text_lines(text);
    let mut patches = Vec::new();
    let mut idx = 0;
    loop {
        let (patch, next) = parse_file(&lines, idx, true)?;
        patches.push(patch);
        idx = next;
        if idx >= lines.len() {
            return Ok(patches);
        }
    }
}
