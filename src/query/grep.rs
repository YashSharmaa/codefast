//! `grep`: regex search over the repository's text, answered from the word index where possible
//! and annotated with the enclosing symbol of every match.

use super::show::{line_starts, line_text};
use super::{Common, PathFilter};
use crate::output::{Out, clip, count};
use crate::store::{FileId, Index};
use crate::text::is_word_byte;
use rayon::prelude::*;
use regex::bytes::{Regex, RegexBuilder};
use regex_syntax::hir::literal::{ExtractKind, Extractor};

pub struct GrepOpts {
    pub ignore_case: bool,
    pub word: bool,
    pub fixed: bool,
    /// List matching files with counts instead of lines.
    pub files_only: bool,
    /// Lines of context after each match.
    pub after: u32,
}

impl Default for GrepOpts {
    fn default() -> Self {
        GrepOpts { ignore_case: false, word: false, fixed: false, files_only: false, after: 0 }
    }
}

struct FileHits {
    file: FileId,
    /// (line, text) of matching lines, capped.
    lines: Vec<(u32, String)>,
    total: usize,
}

/// The longest run of word bytes in `lit`.
fn longest_word_run(lit: &[u8]) -> &[u8] {
    let mut best: &[u8] = b"";
    let mut start = None;
    for (i, &b) in lit.iter().enumerate() {
        if is_word_byte(b) {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            if i - s > best.len() {
                best = &lit[s..i];
            }
        }
    }
    if let Some(s) = start {
        if lit.len() - s > best.len() {
            best = &lit[s..];
        }
    }
    best
}

/// Word fragments of which every match must contain at least one. `None` when the pattern gives
/// no usable literal (the search then visits every file).
fn required_fragments(pattern: &str, ignore_case: bool) -> Option<Vec<Vec<u8>>> {
    let hir = regex_syntax::ParserBuilder::new().case_insensitive(ignore_case).multi_line(true).utf8(false).build().parse(pattern).ok()?;
    let mut best: Option<Vec<Vec<u8>>> = None;
    for kind in [ExtractKind::Prefix, ExtractKind::Suffix] {
        let mut ex = Extractor::new();
        ex.kind(kind).limit_total(64);
        let seq = ex.extract(&hir);
        let Some(lits) = seq.literals() else { continue };
        if lits.is_empty() {
            continue;
        }
        let mut frags: Vec<Vec<u8>> = Vec::new();
        let mut ok = true;
        for l in lits {
            let run = longest_word_run(l.as_bytes());
            if run.len() < 3 {
                ok = false;
                break;
            }
            frags.push(run.to_ascii_lowercase());
        }
        if !ok {
            continue;
        }
        frags.sort();
        frags.dedup();
        let min_len = frags.iter().map(|f| f.len()).min().unwrap_or(0);
        let better = match &best {
            Some(b) => min_len > b.iter().map(|f| f.len()).min().unwrap_or(0),
            None => true,
        };
        if better {
            best = Some(frags);
        }
    }
    best
}

/// Files whose text contains a word containing any of the fragments (case-insensitively).
fn candidate_files(ix: &Index, frags: &[Vec<u8>]) -> Option<Vec<FileId>> {
    let alt: Vec<String> = frags.iter().map(|f| regex::escape(&String::from_utf8_lossy(f))).collect();
    let re = RegexBuilder::new(&alt.join("|")).case_insensitive(true).unicode(false).build().ok()?;
    let mut too_many = false;
    let names = ix.scan_lexicon(|blob, offs, out| {
        let mut pos = 0usize;
        while pos < blob.len() {
            let Some(m) = re.find_at(blob, pos) else { break };
            let id = offs.partition_point(|&o| o as usize <= m.start()) - 1;
            let end = offs[id + 1] as usize;
            if m.end() <= end {
                out.push(id as u32);
                pos = end;
            } else {
                pos = m.start() + 1;
            }
            if out.len() > 200_000 {
                too_many = true; // too unselective to be worth it
                break;
            }
        }
    });
    if too_many {
        return None;
    }
    let mut seen = vec![false; ix.n_files() as usize];
    let mut files: Vec<FileId> = Vec::new();
    for n in names {
        for f in ix.files_with(n) {
            if !seen[f.0 as usize] {
                seen[f.0 as usize] = true;
                files.push(f);
            }
        }
    }
    files.sort_unstable();
    Some(files)
}

fn scan_file(ix: &Index, file: FileId, re: &Regex, cap: usize) -> Option<FileHits> {
    let src = std::fs::read(ix.root.join(ix.path(file))).ok()?;
    let mut lines: Vec<(u32, String)> = Vec::new();
    let mut total = 0usize;
    let mut line = 1u32;
    let mut counted_to = 0usize;
    let mut last_line = 0u32;
    for m in re.find_iter(&src) {
        line += memchr::memchr_iter(b'\n', &src[counted_to..m.start()]).count() as u32;
        counted_to = m.start();
        if line == last_line {
            continue;
        }
        last_line = line;
        total += 1;
        if lines.len() < cap {
            let ls = src[..m.start()].iter().rposition(|&b| b == b'\n').map(|i| i + 1).unwrap_or(0);
            let le = memchr::memchr(b'\n', &src[m.start()..]).map(|i| m.start() + i).unwrap_or(src.len());
            let text = String::from_utf8_lossy(&src[ls..le]).trim().to_string();
            lines.push((line, text));
        }
    }
    if total == 0 { None } else { Some(FileHits { file, lines, total }) }
}

pub fn grep(ix: &Index, pattern: &str, common: &Common, opts: &GrepOpts, out: &mut Out) {
    let mut pat = if opts.fixed { regex::escape(pattern) } else { pattern.to_string() };
    if opts.word {
        pat = format!(r"\b(?:{pat})\b");
    }
    let re = match RegexBuilder::new(&pat).case_insensitive(opts.ignore_case).multi_line(true).unicode(false).build() {
        Ok(r) => r,
        Err(e) => {
            out.push(&format!("invalid pattern: {e}"));
            return;
        }
    };
    let pf = PathFilter::new(&common.paths);
    let cands = required_fragments(&pat, opts.ignore_case).and_then(|f| candidate_files(ix, &f));
    let indexed = cands.is_some();
    let files: Vec<FileId> = match cands {
        Some(c) => c,
        None => (0..ix.n_files()).map(FileId).collect(),
    };
    let files: Vec<FileId> = files.into_iter().filter(|&f| ix.file(f).hash != 0 && pf.matches(ix.path(f)) && (common.tests || !ix.is_test_file(f))).collect();
    let mut hits: Vec<FileHits> = files.par_iter().filter_map(|&f| scan_file(ix, f, &re, 50)).collect();
    // Code before prose and unparsed files, then by path.
    hits.sort_by_cached_key(|h| {
        let fl = ix.file(h.file).flags;
        ((fl & crate::facts::fflag::DOC != 0) as u8, ix.path(h.file).to_string())
    });
    let total: usize = hits.iter().map(|h| h.total).sum();
    if total == 0 {
        let scope = if indexed { "" } else { " (full scan)" };
        out.push(&format!("no matches for /{pattern}/{scope}"));
        if !common.tests {
            out.push("(test files are skipped by default: --tests on the command line, `tests: true` over MCP)");
        }
        return;
    }
    out.push(&format!("{} matching lines in {} files", count(total), count(hits.len())));
    if opts.files_only {
        let mut shown = 0;
        for h in &hits {
            if !out.line(&format!("{} ({})", ix.path(h.file), h.total)) {
                break;
            }
            shown += 1;
        }
        if shown < hits.len() {
            out.push(&format!("… {} more files (narrow with --path DIR)", count(hits.len() - shown)));
        }
        return;
    }
    let est = total * 18 + hits.len() * 8;
    if est > out.remaining() {
        out.push(&format!("by dir: {}", dir_histogram(ix, &hits, 10)));
    }
    let mut shown_lines = 0usize;
    let mut shown_files = 0usize;
    'files: for h in &hits {
        let path = ix.path(h.file);
        let src = if opts.after > 0 { ix.source(h.file) } else { None };
        let starts = src.as_ref().map(|s| line_starts(s));
        let single = h.lines.len() == 1 && h.total == 1 && opts.after == 0;
        if !single && !out.line(path) {
            break;
        }
        for (line, text) in &h.lines {
            let encl = match ix.def_at(h.file, *line) {
                Some(d) if d.rec.line != *line => format!(" {}:", d.name_str()),
                _ => String::new(),
            };
            let row = if single { format!("{path}:{line}{encl} {}", clip(text, 200)) } else { format!("  {line}{encl} {}", clip(text, 200)) };
            if !out.line(&row) {
                break 'files;
            }
            shown_lines += 1;
            if let (Some(s), Some(st)) = (&src, &starts) {
                for l in line + 1..=line + opts.after {
                    let t = String::from_utf8_lossy(line_text(s, st, l));
                    if !out.line(&format!("  {l}  {}", clip(&t, 200))) {
                        break 'files;
                    }
                }
            }
        }
        if h.total > h.lines.len() {
            out.line(&format!("  … {} more in this file", h.total - h.lines.len()));
        }
        shown_files += 1;
    }
    if shown_files < hits.len() {
        let rest: usize = hits[shown_files..].iter().map(|h| h.total).sum();
        out.push(&format!(
            "… {} more lines in {} files (narrow with --path DIR, or -l for the file list)",
            count(rest.max(total - shown_lines)),
            count(hits.len() - shown_files)
        ));
    }
}

fn dir_histogram(ix: &Index, hits: &[FileHits], top_n: usize) -> String {
    let mut dirs: rustc_hash::FxHashMap<String, usize> = rustc_hash::FxHashMap::default();
    for h in hits {
        let p = ix.path(h.file);
        let mut it = p.split('/');
        let key = match (it.next(), it.next(), it.next()) {
            (Some(a), Some(b), Some(_)) => format!("{a}/{b}"),
            (Some(a), Some(_), None) => a.to_string(),
            _ => ".".to_string(),
        };
        *dirs.entry(key).or_default() += h.total;
    }
    let mut top: Vec<(String, usize)> = dirs.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let shown: Vec<String> = top.iter().take(top_n).map(|(d, n)| format!("{d} {}", count(*n))).collect();
    let more = if top.len() > top_n { format!(", +{} dirs", top.len() - top_n) } else { String::new() };
    format!("{}{}", shown.join(", "), more)
}
