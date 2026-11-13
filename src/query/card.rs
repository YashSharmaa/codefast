//! What a definition is for, in one line: the first sentence of its documentation.
//!
//! A listing (an outline, a callee list) names functions. A name and a signature say what a
//! function takes, not what it does, and finding that out costs a read of its body. Where the
//! author wrote it down, this module hands the sentence over together with the name. Nothing is
//! generated: a definition without documentation gets no line.

use super::show::{doc_start, line_starts, line_text};
use crate::facts::DefKind;
use crate::lang::Lang;
use crate::store::{DefV, FileId, Index};
use rustc_hash::FxHashMap;
use std::sync::Arc;

/// Documentation sentences, with each source file read once.
pub struct Docs<'a> {
    ix: &'a Index,
    cache: FxHashMap<FileId, Option<(Arc<Vec<u8>>, Vec<usize>)>>,
}

impl<'a> Docs<'a> {
    pub fn new(ix: &'a Index) -> Docs<'a> {
        Docs { ix, cache: FxHashMap::default() }
    }

    /// The first sentence of `d`'s documentation, at most `max` characters, or nothing.
    pub fn line(&mut self, d: &DefV, max: usize) -> Option<String> {
        let ix = self.ix;
        let file = d.file();
        let entry = self.cache.entry(file).or_insert_with(|| {
            ix.source(file).map(|s| {
                let st = line_starts(&s);
                (s, st)
            })
        });
        let (src, starts) = entry.as_ref()?;
        let text = doc_text(ix.lang(file), src, starts, d)?;
        Some(crate::output::clip(&text, max))
    }

    /// ` // sentence` to put after a signature, or nothing.
    pub fn suffix(&mut self, d: &DefV, max: usize) -> String {
        match self.line(d, max) {
            Some(s) => format!("  // {s}"),
            None => String::new(),
        }
    }
}

/// How one comment line reads once its markers are gone.
enum Piece {
    /// Not prose: an attribute, a pragma, a ruler, an opening or closing marker.
    Skip,
    /// The end of the summary: a blank comment line, or the first tag of a tag list.
    Stop,
    Text(String),
}

/// Lines that are instructions to tools, not descriptions for readers.
const PRAGMAS: [&str; 24] = [
    "eslint-", "prettier-", "istanbul ", "noqa", "nolint", "NOLINT", "go:", "+build", "SPDX-", "Copyright", "(c) ", "clippy::", "rustfmt::", "@ts-", "ts-ignore",
    "pylint:", "mypy:", "type: ignore", "coding:", "-*-", "pragma ", "MARK:", "swiftlint:", "phpcs:",
];
const NOTES: [&str; 5] = ["TODO", "FIXME", "XXX", "HACK", "NOTE:"];
/// Tags whose text is the summary itself.
const SUMMARY_TAGS: [&str; 5] = ["brief", "summary", "description", "desc", "abstract"];

fn strip_tags(s: &str) -> String {
    // `/// <summary>Text</summary>` (C#), `<p>`, `{@link X}`: keep the words.
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

fn clean(raw: &[u8], lang: Lang) -> Piece {
    let line = String::from_utf8_lossy(raw);
    let t = line.trim();
    let hash = matches!(lang, Lang::Python | Lang::Ruby | Lang::Bash | Lang::Php);
    let body = if let Some(r) = t.strip_prefix("///").or_else(|| t.strip_prefix("//!")).or_else(|| t.strip_prefix("//")) {
        r
    } else if let Some(r) = t.strip_prefix("/**").or_else(|| t.strip_prefix("/*!")).or_else(|| t.strip_prefix("/*")) {
        r
    } else if t == "*/" {
        return Piece::Skip;
    } else if let Some(r) = t.strip_prefix('*') {
        r
    } else if lang == Lang::Lua && t.starts_with("--") {
        t.trim_start_matches('-')
    } else if hash && t.starts_with('#') && !t.starts_with("#[") && !t.starts_with("#!") {
        t.trim_start_matches('#')
    } else {
        // An attribute, annotation or decorator between the comment and the definition.
        return Piece::Skip;
    };
    let body = body.trim_end().strip_suffix("*/").unwrap_or(body).trim();
    if body.is_empty() {
        return Piece::Stop;
    }
    if !body.chars().any(char::is_alphabetic) || PRAGMAS.iter().any(|p| body.starts_with(p)) || NOTES.iter().any(|p| body.starts_with(p)) {
        return Piece::Skip;
    }
    if let Some(tag) = body.strip_prefix('@').or_else(|| body.strip_prefix('\\')) {
        let (name, rest) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
        return if SUMMARY_TAGS.contains(&name) && !rest.trim().is_empty() { Piece::Text(rest.trim().to_string()) } else { Piece::Stop };
    }
    if body.starts_with(':') || body.starts_with("Args:") || body.starts_with("Returns:") || body.starts_with("Parameters") {
        return Piece::Stop;
    }
    let text = strip_tags(body);
    let text = text.trim();
    if text.is_empty() { Piece::Skip } else { Piece::Text(text.to_string()) }
}

/// Where the first sentence of `s` ends: after a full stop that is followed by a space and a
/// capital or by the end of the text, unless the stop belongs to an abbreviation.
fn sentence_end(s: &str) -> Option<usize> {
    const ABBREV: [&str; 8] = ["e.g", "i.e", "etc", "vs", "cf", "approx", "resp", "incl"];
    let b = s.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c != b'.' && c != b'!' && c != b'?' {
            continue;
        }
        let rest = &s[i + 1..];
        let ends = rest.is_empty() || (rest.starts_with(' ') && rest.trim_start().chars().next().is_some_and(|n| n.is_uppercase() || !n.is_alphanumeric()));
        if !ends {
            continue;
        }
        let word = s[..i].rsplit(|ch: char| ch.is_whitespace() || ch == '(').next().unwrap_or("");
        if c == b'.' && (ABBREV.iter().any(|a| word.eq_ignore_ascii_case(a)) || word.len() == 1) {
            continue;
        }
        return Some(i + 1);
    }
    None
}

/// `name - text`, `name() - text`, `struct name - text` (kernel-doc) and `name: text`: the text.
fn without_name<'s>(text: &'s str, name: &str) -> &'s str {
    let mut t = text;
    for lead in ["struct ", "enum ", "union ", "typedef ", "macro ", "function "] {
        if let Some(r) = t.strip_prefix(lead) {
            t = r;
            break;
        }
    }
    let Some(r) = t.strip_prefix(name) else { return text };
    let r = r.strip_prefix("()").unwrap_or(r);
    for sep in [" - ", " -- ", ": ", " \u{2013} ", " \u{2014} "] {
        if let Some(rest) = r.strip_prefix(sep) {
            return rest.trim_start();
        }
    }
    text
}

fn finish(mut words: String, name: &str) -> Option<String> {
    if let Some(e) = sentence_end(&words) {
        words.truncate(e);
    }
    let text = without_name(words.trim(), name).trim().trim_end_matches('.').trim();
    // A comment that only repeats the name says nothing.
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    if letters < 4 || text.eq_ignore_ascii_case(name) {
        return None;
    }
    Some(text.to_string())
}

/// The docstring that opens a Python body.
fn python_docstring(src: &[u8], d: &DefV) -> Option<String> {
    if d.rec.sig_len == u16::MAX {
        return None;
    }
    let mut i = d.rec.sig_off as usize + d.rec.sig_len as usize;
    let end = src.len().min(i + 4096);
    // The colon of the block, then blank space up to the first statement.
    while i < end && (src[i] == b':' || src[i].is_ascii_whitespace()) {
        i += 1;
    }
    while i < end && matches!(src[i], b'r' | b'R' | b'u' | b'U' | b'b' | b'B' | b'f' | b'F') {
        i += 1;
    }
    let rest = &src[i..end];
    let (open, close): (usize, &[u8]) = if rest.starts_with(b"\"\"\"") {
        (3, b"\"\"\"")
    } else if rest.starts_with(b"'''") {
        (3, b"'''")
    } else if rest.starts_with(b"\"") {
        (1, b"\"")
    } else if rest.starts_with(b"'") {
        (1, b"'")
    } else {
        return None;
    };
    let body = &rest[open..];
    let stop = memchr::memmem::find(body, close).unwrap_or(body.len());
    let text = String::from_utf8_lossy(&body[..stop]);
    let mut words = String::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() {
            if words.is_empty() {
                continue;
            }
            break;
        }
        if l.starts_with(':') || l.starts_with("Args:") || l.starts_with("Returns:") || l.starts_with("Parameters") || l.starts_with(">>>") {
            break;
        }
        if !words.is_empty() {
            words.push(' ');
        }
        words.push_str(l);
        if sentence_end(&words).is_some() || words.len() > 400 {
            break;
        }
    }
    finish(words, &d.name_str())
}

/// A comment after the code on the definition's own line: `int count; /* items in use */`.
fn trailing_comment(lang: Lang, line: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(line);
    let hash = matches!(lang, Lang::Python | Lang::Ruby | Lang::Bash);
    let at = if hash { text.rfind(" # ").map(|i| i + 3) } else { text.rfind("//").map(|i| i + 2).or_else(|| text.rfind("/*").map(|i| i + 2)) }?;
    // Not a comment when it sits inside a string or a URL.
    let before = &text[..at];
    if before.matches('"').count() % 2 == 1 || before.ends_with("://") {
        return None;
    }
    let body = text[at..].trim().trim_end_matches("*/").trim().trim_start_matches(['<', '/', '!', '*']).trim();
    if PRAGMAS.iter().any(|p| body.starts_with(p)) || NOTES.iter().any(|p| body.starts_with(p)) {
        return None;
    }
    finish(body.to_string(), name)
}

fn doc_text(lang: Lang, src: &[u8], starts: &[usize], d: &DefV) -> Option<String> {
    let name = d.name_str();
    let kind = d.kind();
    if lang == Lang::Python && (kind.is_callable() || kind.is_type()) {
        if let Some(s) = python_docstring(src, d) {
            return Some(s);
        }
    }
    let first = d.rec.line;
    let top = doc_start(lang, src, starts, first);
    let mut words = String::new();
    for l in top..first {
        match clean(line_text(src, starts, l), lang) {
            Piece::Skip => continue,
            Piece::Stop if words.is_empty() => continue,
            Piece::Stop => break,
            Piece::Text(t) => {
                if !words.is_empty() {
                    words.push(' ');
                }
                words.push_str(&t);
                if sentence_end(&words).is_some() || words.len() > 400 {
                    break;
                }
            }
        }
    }
    if let Some(s) = finish(words, &name) {
        return Some(s);
    }
    if matches!(kind, DefKind::Field | DefKind::EnumMember | DefKind::Variable | DefKind::Constant | DefKind::Property | DefKind::Macro) && d.rec.end_line == first {
        return trailing_comment(lang, line_text(src, starts, first), &name);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lang: Lang, lines: &[&str]) -> Option<String> {
        let mut words = String::new();
        for l in lines {
            match clean(l.as_bytes(), lang) {
                Piece::Skip => continue,
                Piece::Stop if words.is_empty() => continue,
                Piece::Stop => break,
                Piece::Text(t) => {
                    if !words.is_empty() {
                        words.push(' ');
                    }
                    words.push_str(&t);
                    if sentence_end(&words).is_some() {
                        break;
                    }
                }
            }
        }
        finish(words, "resolve")
    }

    #[test]
    fn the_first_sentence_of_each_comment_style() {
        assert_eq!(text(Lang::Rust, &["/// Link a name to its definition. The rest is detail.", "#[inline]"]).as_deref(), Some("Link a name to its definition"));
        assert_eq!(text(Lang::Go, &["// resolve links a name to its", "// definition, e.g. a call. More."]).as_deref(), Some("resolve links a name to its definition, e.g. a call"));
        assert_eq!(text(Lang::C, &["/**", " * resolve() - link a name to its definition", " * @name: the name", " */"]).as_deref(), Some("link a name to its definition"));
        assert_eq!(text(Lang::Java, &["/**", " * Links a name.", " *", " * @param n the name", " */", "@Override"]).as_deref(), Some("Links a name"));
        assert_eq!(text(Lang::CSharp, &["/// <summary>", "/// Links a name to its <see cref=\"Def\"/> record.", "/// </summary>"]).as_deref(), Some("Links a name to its  record"));
        assert_eq!(text(Lang::Python, &["# Link a name to its definition."]).as_deref(), Some("Link a name to its definition"));
        assert_eq!(text(Lang::TypeScript, &["/** @description Link a name. */"]).as_deref(), Some("Link a name"));
    }

    #[test]
    fn what_is_not_a_description() {
        assert_eq!(text(Lang::Rust, &["// TODO: make this faster"]), None);
        assert_eq!(text(Lang::JavaScript, &["// eslint-disable-next-line no-console"]), None);
        assert_eq!(text(Lang::C, &["/* resolve */"]), None);
        assert_eq!(text(Lang::C, &["/* SPDX-License-Identifier: MIT */"]), None);
        assert_eq!(text(Lang::Java, &["@Override", "@Deprecated"]), None);
        assert_eq!(text(Lang::C, &["/* ------------------------------ */"]), None);
    }

    #[test]
    fn a_trailing_comment_describes_a_field() {
        assert_eq!(trailing_comment(Lang::C, b"\tint count; /* items in use */", "count").as_deref(), Some("items in use"));
        assert_eq!(trailing_comment(Lang::Go, b"\tCount int // items in use", "Count").as_deref(), Some("items in use"));
        assert_eq!(trailing_comment(Lang::C, b"\tconst char *url = \"http://x\";", "url"), None);
        assert_eq!(trailing_comment(Lang::Python, b"count = 0  # items in use", "count").as_deref(), Some("items in use"));
    }
}
