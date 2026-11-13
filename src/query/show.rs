//! `show`: print exactly the source an agent needs: one symbol's body, a file's outline, or a
//! line range, instead of a whole file.

use super::{Common, PathFilter, Spec, def_line, parse_spec, sig_of, targets_for};
use crate::facts::DefKind;
use crate::lang::Lang;
use crate::output::{Out, count};
use crate::resolve::Resolver;
use crate::store::{DefV, FileId, Index};

pub struct ShowOpts {
    /// Longest body printed in full; longer ones are cut with a pointer to the rest.
    pub max_lines: usize,
    pub full: bool,
    /// Force an outline (signatures only) even for small containers.
    pub outline: bool,
    /// Include the leading doc comment.
    pub doc: bool,
}

impl Default for ShowOpts {
    fn default() -> Self {
        ShowOpts { max_lines: 160, full: false, outline: false, doc: true }
    }
}

/// Byte offsets of line starts; `lines[i]` is the start of line `i + 1`.
pub fn line_starts(src: &[u8]) -> Vec<usize> {
    let mut v = Vec::with_capacity(src.len() / 32 + 1);
    v.push(0);
    for i in memchr::memchr_iter(b'\n', src) {
        v.push(i + 1);
    }
    v
}

pub fn line_text<'a>(src: &'a [u8], starts: &[usize], line: u32) -> &'a [u8] {
    let i = line as usize;
    if i == 0 || i > starts.len() {
        return b"";
    }
    let s = starts[i - 1];
    let e = if i < starts.len() { starts[i] } else { src.len() };
    let mut t = &src[s..e.max(s)];
    while let Some((&last, rest)) = t.split_last() {
        if last == b'\n' || last == b'\r' || last == b' ' || last == b'\t' {
            t = rest;
        } else {
            break;
        }
    }
    t
}

/// Print lines `from..=to`. Returns the number of lines that did not fit.
///
/// Line numbers are sparse: the first line, every fifth line, and nothing else. A number on
/// every line costs a fifth more tokens for no extra information: edits address text, and any
/// line is at most four lines from a numbered one.
pub fn print_lines(out: &mut Out, src: &[u8], starts: &[usize], from: u32, to: u32) -> u32 {
    let last = (starts.len() as u32).min(to);
    let first = from.max(1);
    let mut n = first;
    while n <= last {
        let text = String::from_utf8_lossy(line_text(src, starts, n));
        let row = if n == first || n % 5 == 0 { format!("{n}\t{text}") } else { format!("\t{text}") };
        if !out.line(&row) {
            return last - n + 1;
        }
        n += 1;
    }
    0
}

fn is_comment_line(lang: Lang, t: &[u8]) -> bool {
    let t = t.trim_ascii_start();
    if t.is_empty() {
        return false;
    }
    let slash = t.starts_with(b"//") || t.starts_with(b"/*") || t.starts_with(b"*") || t.starts_with(b"*/");
    match lang {
        Lang::Python | Lang::Ruby | Lang::Bash => t.starts_with(b"#") || t.starts_with(b"@"),
        Lang::Rust => slash || t.starts_with(b"#[") || t.starts_with(b"#!["),
        Lang::Lua => t.starts_with(b"--"),
        Lang::Java | Lang::Kotlin | Lang::Scala | Lang::TypeScript | Lang::Tsx | Lang::JavaScript | Lang::Swift | Lang::CSharp | Lang::Php => {
            slash || t.starts_with(b"@") || t.starts_with(b"[")
        }
        _ => slash,
    }
}

/// First line of the comment block directly above `line`, or `line` when there is none.
pub fn doc_start(lang: Lang, src: &[u8], starts: &[usize], line: u32) -> u32 {
    let mut l = line;
    while l > 1 && line - l < 40 {
        if is_comment_line(lang, line_text(src, starts, l - 1)) {
            l -= 1;
        } else {
            break;
        }
    }
    l
}

fn members<'a>(ix: &'a Index, d: &DefV<'a>) -> Vec<DefV<'a>> {
    ix.defs_of_file(d.file()).filter(|m| m.parent().is_some_and(|p| p.id == d.id)).collect()
}

pub fn show_def(ix: &Index, d: &DefV, opts: &ShowOpts, out: &mut Out) {
    let file = d.file();
    let Some(src) = ix.source(file) else {
        out.push(&format!("{} (file not readable)", def_line(d)));
        return;
    };
    let starts = line_starts(&src);
    let (from, to) = (d.rec.line, d.rec.end_line);
    let nlines = (to - from + 1) as usize;
    let lang = ix.lang(file);
    let kind = d.kind();
    out.push(&format!("{}:{}-{} {}", d.path(), from, to, d.qualified()));

    // Large containers: members' signatures tell more per token than the raw body.
    let container = kind.is_container() && kind != DefKind::TypeAlias;
    if container && !opts.full && (opts.outline || nlines > opts.max_lines) {
        out.push(&format!("{}\t{}", from, sig_of(d)));
        let ms = members(ix, d);
        let mut dropped = 0;
        for m in &ms {
            if !out.line(&format!("{}\t  {}", m.rec.line, sig_of(m))) {
                dropped += 1;
            }
        }
        if dropped > 0 {
            out.push(&format!("… {} more members (codefast show {}:{}-{})", dropped, d.path(), from, to));
        }
        return;
    }

    let mut first = from;
    if opts.doc {
        let ds = doc_start(lang, &src, &starts, from);
        // Long comment blocks: keep the opening lines, where the summary lives.
        if from - ds > 12 {
            print_lines(out, &src, &starts, ds, ds + 9);
            out.push(&format!("…\t({} more comment lines)", from - ds - 10));
        } else {
            first = ds;
        }
    }
    let limit = if opts.full { u32::MAX } else { opts.max_lines as u32 };
    let end = if nlines as u32 > limit { from + limit - 1 } else { to };
    let unfit = print_lines(out, &src, &starts, first, end);
    let rest_from = end + 1 - unfit;
    if rest_from <= to {
        out.push(&format!("… {} more lines (codefast show {}:{}-{})", to - rest_from + 1, d.path(), rest_from, to));
    }
}

/// Outline of a file: its definitions as an indented tree of signatures.
pub fn outline(ix: &Index, file: FileId, opts: &ShowOpts, out: &mut Out) {
    let f = ix.file(file);
    let defs: Vec<DefV> = ix.defs_of_file(file).collect();
    out.push(&format!("{} ({} lines, {} symbols)", ix.path(file), count(f.nlines as usize), count(defs.len())));
    if defs.is_empty() {
        if f.flags & crate::facts::fflag::UNPARSED != 0 {
            out.push("(not parsed: unsupported language or too large; use `codefast show PATH:START-END` or grep)");
        }
        return;
    }
    // Small files get everything; big ones drop leaf members first, then long macro runs.
    let many = defs.len() > 80 && !opts.full;
    let nmacros = defs.iter().filter(|d| d.kind() == DefKind::Macro).count();
    let collapse_macros = many && nmacros > 40;
    // The id of the file's first definition. Ids are global, and `f.def_start` counts inside
    // the file's own segment, so for a file edited since the last full index the two differ.
    let base = defs.first().map_or(0, |d| d.id.0);
    let mut depth_cache: Vec<u8> = vec![0; defs.len()];
    let mut macro_run: Option<(u32, u32, usize)> = None;
    let mut dropped = 0usize;
    let flush_macros = |run: &mut Option<(u32, u32, usize)>, out: &mut Out| {
        if let Some((a, b, n)) = run.take() {
            out.line(&format!("{a}-{b}\t… {n} more #define"));
        }
    };
    let mut macros_seen = 0usize;
    let mut docs = super::card::Docs::new(ix);
    for (i, d) in defs.iter().enumerate() {
        let depth = match d.parent() {
            Some(p) if p.id.0 >= base && ((p.id.0 - base) as usize) < i => depth_cache[(p.id.0 - base) as usize].saturating_add(1),
            _ => 0,
        };
        depth_cache[i] = depth;
        let kind = d.kind();
        if many && matches!(kind, DefKind::Field | DefKind::EnumMember | DefKind::Property) {
            continue;
        }
        if collapse_macros && kind == DefKind::Macro {
            macros_seen += 1;
            if macros_seen > 12 {
                macro_run = Some(match macro_run {
                    Some((a, _, n)) => (a, d.rec.line, n + 1),
                    None => (d.rec.line, d.rec.line, 1),
                });
                continue;
            }
        } else {
            flush_macros(&mut macro_run, out);
        }
        let mut line = String::new();
        if d.rec.end_line > d.rec.line {
            line.push_str(&format!("{}-{}\t", d.rec.line, d.rec.end_line));
        } else {
            line.push_str(&format!("{}\t", d.rec.line));
        }
        for _ in 0..depth {
            line.push_str("  ");
        }
        line.push_str(&sig_of(d));
        // What it is for, where the author said so. In a long outline only functions and
        // types get the sentence: that is where it saves opening the body.
        if !many || kind.is_callable() || kind.is_type() {
            line.push_str(&docs.suffix(d, if many { 60 } else { 80 }));
        }
        if many && kind.is_container() {
            let n = defs.iter().filter(|m| m.parent().is_some_and(|p| p.id == d.id)).count();
            if n > 0 {
                line.push_str(&format!(" {{{n} members}}"));
            }
        }
        if !out.line(&line) {
            dropped += 1;
        }
    }
    flush_macros(&mut macro_run, out);
    if dropped > 0 {
        out.push(&format!("… {} more symbols (narrow with --kind, or raise --budget)", count(dropped)));
    }
}

pub fn show(ix: &Index, rs: &Resolver, specs: &[String], common: &Common, opts: &ShowOpts, out: &mut Out) {
    let pf = PathFilter::new(&common.paths);
    for (i, s) in specs.iter().enumerate() {
        if i > 0 {
            out.push("");
        }
        let spec = parse_spec(ix, s);
        match &spec {
            Spec::File(f) => outline(ix, *f, opts, out),
            Spec::Range { file, start, end } => {
                let Some(src) = ix.source(*file) else {
                    out.push(&format!("{}: not readable", ix.path(*file)));
                    continue;
                };
                let starts = line_starts(&src);
                let encl = ix.def_at(*file, *start).map(|d| format!(" (in {})", d.qualified())).unwrap_or_default();
                out.push(&format!("{}:{}-{}{}", ix.path(*file), start, end, encl));
                let unfit = print_lines(out, &src, &starts, *start, *end);
                if unfit > 0 {
                    out.push(&format!("… {} more lines (codefast show {}:{}-{})", unfit, ix.path(*file), end + 1 - unfit, end));
                }
            }
            Spec::Loc { file, line } => match ix.def_at(*file, *line) {
                Some(d) => show_def(ix, &d, opts, out),
                None => {
                    // Not inside any symbol: show the surrounding lines.
                    let Some(src) = ix.source(*file) else { continue };
                    let starts = line_starts(&src);
                    let (a, b) = (line.saturating_sub(10).max(1), line + 10);
                    out.push(&format!("{}:{}-{}", ix.path(*file), a, b));
                    print_lines(out, &src, &starts, a, b);
                }
            },
            Spec::Sym { name, .. } => {
                let ents = targets_for(ix, rs, &spec, common, &pf);
                if ents.is_empty() {
                    out.push(&format!("no symbol named `{name}` (try: codefast find {name})"));
                    continue;
                }
                let best = &ents[0];
                let p = best.primary(ix);
                show_def(ix, &p, opts, out);
                // Other definitions of the same entity (prototypes, per-arch variants) and other
                // entities sharing the name: one line each, so no second round trip is needed.
                let mut also: Vec<String> = Vec::new();
                for &d in &best.defs {
                    if d != p.id {
                        let v = ix.def(d);
                        also.push(format!("{}:{}{}", v.path(), v.rec.line, if v.is_decl() { " [decl]" } else { "" }));
                    }
                }
                for e in ents.iter().skip(1) {
                    let v = e.primary(ix);
                    also.push(format!("{}:{} ({})", v.path(), v.rec.line, v.kind().tag()));
                }
                if !also.is_empty() {
                    let total = also.len();
                    also.truncate(6);
                    let more = if total > also.len() { format!(" +{} more", total - also.len()) } else { String::new() };
                    out.push(&format!("also: {}{}", also.join(", "), more));
                }
            }
        }
    }
}
