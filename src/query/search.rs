//! `search`: the front door. One command for "I have a name", "I have some text" and "I have a
//! bug report": it routes the query, and every hit carries the structure around it (who calls
//! it, where it is bound, what else mentions it) so that a follow-up call is rarely needed.

use super::locate::{LocateOpts, locate};
use super::refs::{Via, callee_edges, caller_edges, encl_name, proximity_key, sites_summary};
use super::show::{ShowOpts, outline, show, show_def};
use super::{Common, PathFilter, Spec, def_line, parse_spec, targets_for};
use crate::facts::DefKind;
use crate::output::{Out, count};
use crate::resolve::{Class, Resolver, Target};
use crate::store::{DefId, FileId, Index};
use rustc_hash::FxHashSet;

pub struct SearchOpts {
    /// Longest body shown inline when exactly one symbol matches.
    pub body_lines: usize,
    /// Answer an open question with the source of its candidates where there is room for it.
    /// Off, it gets the ranking alone.
    pub source: bool,
}

impl Default for SearchOpts {
    fn default() -> Self {
        SearchOpts { body_lines: 60, source: true }
    }
}

/// One line of inverse structure: `called by: a, b, c (+12)` plus slot bindings.
pub fn usage_line(ix: &Index, rs: &Resolver, t: &Target, max_names: usize) -> Vec<String> {
    usage(ix, rs, t, max_names, true)
}

/// The lines of [`usage_line`], without the caller names when `callers` is false: for answers
/// that list every call site themselves.
fn usage(ix: &Index, rs: &Resolver, t: &Target, max_names: usize, callers: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let p = t.primary(ix);
    if t.class == Class::Member {
        return lines;
    }
    let (edges, loose) = caller_edges(rs, t);
    let def_path = p.path().to_string();
    let mut direct: Vec<_> = edges.iter().filter(|e| matches!(e.via, Via::Call)).map(|e| e.r).collect();
    direct.sort_by_cached_key(|r| proximity_key(ix, &def_path, r));
    // Where receivers carry no type, a rare method name is the evidence there is.
    let mut by_name: Vec<_> = if super::refs::loose_is_likely(rs, t, loose.len()) { loose } else { Vec::new() };
    by_name.retain(|r| !ix.is_test_file(r.file) || ix.is_test_file(p.file()));
    by_name.sort_by_cached_key(|r| proximity_key(ix, &def_path, r));
    if !direct.is_empty() && callers {
        let mut names: Vec<String> = Vec::new();
        for r in &direct {
            let n = encl_name(r);
            if !names.contains(&n) {
                names.push(n);
            }
        }
        let total = names.len();
        let shown: Vec<String> = names.into_iter().take(max_names).collect();
        let more = if total > shown.len() { format!(" (+{} more callers)", total - shown.len()) } else { String::new() };
        let label = if t.class == Class::Callable { "called by" } else { "used by" };
        lines.push(format!("{label}: {}{}", shown.join(", "), more));
    }
    if !by_name.is_empty() && callers {
        let mut names: Vec<String> = Vec::new();
        for r in &by_name {
            let n = format!("{} ({}:{})", encl_name(r), r.path(), r.line());
            if !names.contains(&n) {
                names.push(n);
            }
        }
        let total = names.len();
        let shown: Vec<String> = names.into_iter().take(max_names.min(4)).collect();
        let more = if total > shown.len() {
            format!(" (+{} more: codefast callers {})", total - shown.len(), String::from_utf8_lossy(&t.name))
        } else {
            String::new()
        };
        lines.push(format!("called by name (receiver type not written): {}{}", shown.join(", "), more));
    }
    let binds: Vec<_> = edges.iter().filter(|e| matches!(e.via, Via::Bind(_))).collect();
    if !binds.is_empty() {
        let slot_sites = edges.iter().filter(|e| matches!(e.via, Via::Slot(..))).count();
        let mut slots: Vec<String> = Vec::new();
        for e in &binds {
            if let Via::Bind(q) = &e.via {
                if let Some(reg) = q.strip_prefix(b"@") {
                    let s = format!("{}() at {}:{}", String::from_utf8_lossy(reg), e.r.path(), e.r.line());
                    if !slots.contains(&s) {
                        slots.push(s);
                    }
                    continue;
                }
                let (owners, field) = rs.slot(q);
                let s = match owners.first() {
                    Some(o) => format!(
                        "{}.{} in {} ({}:{})",
                        String::from_utf8_lossy(o).trim_start_matches('%'),
                        String::from_utf8_lossy(&field),
                        encl_name(&e.r),
                        e.r.path(),
                        e.r.line()
                    ),
                    None => format!("{} in {} ({}:{})", String::from_utf8_lossy(q), encl_name(&e.r), e.r.path(), e.r.line()),
                };
                if !slots.contains(&s) {
                    slots.push(s);
                }
            }
        }
        let total = slots.len();
        slots.truncate(3);
        let more = if total > slots.len() { format!(" (+{})", total - slots.len()) } else { String::new() };
        let sites = if slot_sites > 0 { format!("; dispatched from {slot_sites} typed call sites") } else { String::new() };
        lines.push(format!("registered / bound: {}{}{}", slots.join(", "), more, sites));
    }
    let cb: Vec<String> = {
        let mut v: Vec<String> = Vec::new();
        for e in edges.iter() {
            if let Via::Arg(g) = &e.via {
                let s = format!("{}() in {} ({}:{})", String::from_utf8_lossy(g), encl_name(&e.r), e.r.path(), e.r.line());
                if !v.contains(&s) {
                    v.push(s);
                }
            }
        }
        v
    };
    if !cb.is_empty() {
        let total = cb.len();
        let shown: Vec<String> = cb.into_iter().take(3).collect();
        let more = if total > shown.len() { format!(" (+{})", total - shown.len()) } else { String::new() };
        lines.push(format!("passed as a callback to: {}{}", shown.join(", "), more));
    }
    let has_cb = edges.iter().any(|e| matches!(e.via, Via::Arg(_)));
    let values = edges.iter().filter(|e| matches!(e.via, Via::Value)).count();
    if values > 0 && t.class == Class::Callable {
        lines.push(format!("passed as a value at {values} sites (codefast callers {} for the list)", String::from_utf8_lossy(&t.name)));
    }
    if direct.is_empty() && binds.is_empty() && values == 0 && !has_cb && by_name.is_empty() && t.class == Class::Callable && callers {
        lines.push("called by: no callers found in indexed code".to_string());
    }
    lines
}

/// Files that mention the name in text but hold no structural reference or definition of it.
pub fn text_only_mentions(ix: &Index, rs: &Resolver, t: &Target) -> Vec<FileId> {
    let Some(nid) = ix.name_id(&t.name) else { return Vec::new() };
    let mut structural: FxHashSet<FileId> = FxHashSet::default();
    for &d in &t.defs {
        structural.insert(ix.def(d).file());
    }
    // A reference that matches by name only is still code, not a comment or a string.
    rs.refs_to(t, |r, _| {
        structural.insert(r.file);
    });
    ix.files_with(nid).filter(|f| !structural.contains(f)).collect()
}

fn card(ix: &Index, rs: &Resolver, t: &Target, others: &[Target], opts: &SearchOpts, out: &mut Out) {
    let p = t.primary(ix);
    let nlines = (p.rec.end_line - p.rec.line + 1) as usize;
    let show_opts = ShowOpts { max_lines: opts.body_lines, full: false, outline: nlines > opts.body_lines && p.kind().is_container(), doc: true };
    show_def(ix, &p, &show_opts, out);
    // Every place that uses it, one line per file: what a change to it has to reach. This is
    // the part that makes the answer final for a rename or a changed signature, so it gets up
    // to half of what the body left of the budget.
    let room = if out.budget() == 0 { 2000 } else { (out.remaining() / 2).clamp(120, 900) };
    let listed = sites_summary(ix, rs, t, room, out);
    for l in usage(ix, rs, t, 5, false) {
        out.line(&l);
    }
    if !listed && t.class == Class::Callable {
        out.line("called by: no callers found in indexed code");
    }
    if p.kind().is_callable() && p.kind() != DefKind::Macro {
        let cs = callee_edges(rs, &p);
        let names: Vec<String> = cs
            .iter()
            .filter(|c| !c.member && c.targets.first().is_some_and(|&d| ix.def(d).kind() != DefKind::Macro))
            .map(|c| String::from_utf8_lossy(c.name).into_owned())
            .collect();
        if !names.is_empty() && nlines > opts.body_lines {
            let shown: Vec<&str> = names.iter().take(10).map(|s| s.as_str()).collect();
            let more = if names.len() > shown.len() { format!(" (+{})", names.len() - shown.len()) } else { String::new() };
            out.line(&format!("calls: {}{}", shown.join(", "), more));
        }
    }
    let mut also: Vec<String> = Vec::new();
    for &d in &t.defs {
        if d != p.id {
            let v = ix.def(d);
            also.push(format!("{}:{}{}", v.path(), v.rec.line, if v.is_decl() { " [decl]" } else { "" }));
        }
    }
    for e in others {
        let v = e.primary(ix);
        also.push(format!("{}:{} ({})", v.path(), v.rec.line, v.kind().tag()));
    }
    if !also.is_empty() {
        let total = also.len();
        also.truncate(5);
        let more = if total > also.len() { format!(" +{} more", total - also.len()) } else { String::new() };
        out.line(&format!("also: {}{}", also.join(", "), more));
    }
    let mentions = text_only_mentions(ix, rs, t);
    // A word that half the repository writes in prose (`Add`, `name`) is not a lead.
    if !mentions.is_empty() && mentions.len() <= 12 {
        let mut ms: Vec<&str> = mentions.iter().map(|f| ix.path(*f)).collect();
        ms.sort();
        let shown: Vec<&str> = ms.iter().take(3).copied().collect();
        let more = if ms.len() > shown.len() { format!(" +{}", ms.len() - shown.len()) } else { String::new() };
        out.line(&format!("text-only mentions (comments, strings, unparsed files): {}{}", shown.join(", "), more));
    }
}

/// `collection.go Collection.Add`, `*Collection Add`, `(*Collection).Add`, `Collection::add in
/// pkg/`: several words that name one symbol, the way people and models write them. Returns the
/// symbol as `Owner.name` or `name`, and the paths that were given with it.
fn symbol_in_words(ix: &Index, rs: &Resolver, q: &str, common: &Common) -> Option<(String, Vec<String>)> {
    let mut names: Vec<String> = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    for raw in q.split_whitespace() {
        let w = raw.trim_matches(|c: char| matches!(c, '*' | '&' | '(' | ')' | ',' | ':' | '`' | '\'' | '"'));
        if w.is_empty() || matches!(w, "in" | "of" | "method" | "function" | "func" | "fn" | "def" | "class" | "struct" | "type" | "the") {
            continue;
        }
        // `who calls X in FILE`, `X callers`: the answer for a symbol has its callers, so the
        // words that ask for them are not part of its name. Only where no symbol has that name.
        if matches!(w, "who" | "calls" | "call" | "callers" | "caller" | "called" | "by" | "uses" | "usages" | "used" | "references" | "definition" | "defined" | "where" | "is")
            && ix.defs_named(w.as_bytes()).is_empty()
        {
            continue;
        }
        let w = w.replace(").", ".").replace("::", ".").replace("->", ".").replace('#', ".");
        let w = w.trim_start_matches(['*', '&', '(']).to_string();
        let is_path = w.contains('/') || (ix.name_id(w.as_bytes()).is_none() && w.contains('.') && super::resolve_file(ix, &w).is_some());
        if is_path {
            paths.push(w);
        } else if w.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '$') {
            names.push(w);
        } else {
            return None;
        }
    }
    if names.is_empty() || names.len() > 2 || (names.len() == 1 && paths.is_empty()) {
        return None;
    }
    let mut narrowed = common.clone();
    narrowed.paths.extend(paths.iter().cloned());
    let pf = PathFilter::new(&narrowed.paths);
    let resolves = |name: &str| !targets_for(ix, rs, &parse_spec(ix, name), &narrowed, &pf).is_empty();
    let name = match &names[..] {
        [one] => one.clone(),
        // `Collection Add`: a type and its member, in either order.
        [a, b] => [format!("{a}.{b}"), format!("{b}.{a}")].into_iter().find(|n| owner_has(ix, n))?,
        _ => return None,
    };
    resolves(&name).then_some((name, paths))
}

/// Whether `Owner.name` names a member that a definition called `Owner` really holds.
fn owner_has(ix: &Index, spec: &str) -> bool {
    let Some((owner, name)) = spec.rsplit_once('.') else { return false };
    ix.defs_named(name.as_bytes()).iter().any(|&d| ix.def(d).parent_name() == Some(owner.as_bytes()))
}

/// Whether `search` takes this query for free text (a report, a question) and not for a symbol,
/// a path or a place. Callers that have a default budget use it to give such a query the
/// larger one, [`super::locate::CODE_BUDGET`].
pub fn is_free_text(ix: &Index, rs: &Resolver, query: &str, common: &Common) -> bool {
    let q = query.trim();
    q.split_whitespace().count() > 1
        && super::split_kind_prefix(q).0.is_none()
        && symbol_in_words(ix, rs, q, common).is_none()
        && several_symbols(ix, q).is_none()
        && code_fragment(ix, q).is_none()
        && written_at(ix, q).is_none()
        && !stands_as_typed(ix, q, common)
        && named_in_a_sentence(ix, q).is_none()
}

/// A message of the program, quoted in a short query: the one place in the code where that
/// text is written out, and the definition around it. Whoever asks with a message wants the
/// function that prints it and who calls that function, not ten files that share its words.
fn written_at(ix: &Index, q: &str) -> Option<(FileId, u32, DefId)> {
    if q.len() > MESSAGE_BYTES {
        return None;
    }
    let (file, line) = match super::locate::origins(ix, q, 600, None).into_iter().next() {
        Some(o) => (o.file, o.line),
        None => whole_text(ix, q)?,
    };
    let def = ix.def_at(file, line)?;
    Some((file, line, def.id))
}

/// The one place where the whole query stands in a string literal of the code: a message
/// too short, or too full of small words, for the phrases of it to count as evidence alone.
fn whole_text(ix: &Index, q: &str) -> Option<(FileId, u32)> {
    let q = q.trim().trim_matches(|c| matches!(c, '"' | '\'' | '`'));
    let words: Vec<&str> = q.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| w.len() >= 3).collect();
    if q.len() < 12 || words.len() < 3 {
        return None;
    }
    // The files that have its rarest word are the only ones that can have all of it.
    let rarest = words.iter().filter_map(|w| ix.name_id(w.as_bytes()).map(|n| (ix.popularity(n), n))).min_by_key(|x| x.0)?;
    let finder = memchr::memmem::Finder::new(q.as_bytes());
    let mut found: Option<(FileId, u32)> = None;
    for (f, _) in ix.postings(rarest.1).take(400) {
        let rec = ix.file(f);
        if !ix.is_live(f) || rec.flags & (crate::facts::fflag::TEST | crate::facts::fflag::DOC | crate::facts::fflag::GENERATED | crate::facts::fflag::VENDOR) != 0 {
            continue;
        }
        let Some(src) = ix.source(f) else { continue };
        let Some(pos) = finder.find(&src) else { continue };
        let start = memchr::memrchr(b'\n', &src[..pos]).map(|i| i + 1).unwrap_or(0);
        // Inside a literal: a quote opens before it on its line.
        if !src[start..pos].iter().any(|c| matches!(c, b'"' | b'\'' | b'`')) {
            continue;
        }
        if found.is_some() {
            return None; // written in two files: no one place
        }
        found = Some((f, 1 + memchr::memchr_iter(b'\n', &src[..pos]).count() as u32));
    }
    found
}

/// A text that stands in the code exactly as it was typed, in whatever number of places: a
/// message that several functions print, a declaration, a line of a table. Whoever types it
/// is looking for those places, and gets them. Ranking files by its words, and printing
/// their functions, answers another question at twenty times the length.
fn stands_as_typed(ix: &Index, q: &str, common: &Common) -> bool {
    let q = q.trim().trim_matches(|c| matches!(c, '"' | '\'' | '`'));
    let words: Vec<&str> = q.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| w.len() >= 3).collect();
    // Two words that some comment happens to hold in that order ("control method", "ftdi_sio
    // driver") are a question and not a quotation: it takes a third word.
    if q.len() < 8 || q.len() > MESSAGE_BYTES || q.split_whitespace().count() < 3 || words.is_empty() {
        return false;
    }
    // Every word of it has to be known, and the files that have its rarest word are the
    // only ones that can have all of it.
    let mut rarest: Option<(u32, crate::store::NameId)> = None;
    for w in &words {
        let Some(n) = ix.name_id(w.as_bytes()) else { return false };
        let p = ix.popularity(n);
        if rarest.is_none_or(|r| p < r.0) {
            rarest = Some((p, n));
        }
    }
    let Some((_, name)) = rarest else { return false };
    let finder = memchr::memmem::Finder::new(q.as_bytes());
    for (f, _) in ix.postings(name).take(400) {
        let rec = ix.file(f);
        let skip = crate::facts::fflag::DOC | crate::facts::fflag::GENERATED | crate::facts::fflag::VENDOR | if common.tests { 0 } else { crate::facts::fflag::TEST };
        if !ix.is_live(f) || rec.flags & skip != 0 {
            continue;
        }
        if ix.source(f).is_some_and(|src| finder.find(&src).is_some()) {
            return true;
        }
    }
    false
}

/// A query longer than this is a report that quotes a message, and its files are ranked.
const MESSAGE_BYTES: usize = 200;

/// `nv_adma_port_suspend nv_adma_port_resume`: two to four words, each the exact name of
/// something defined here and each shaped like an identifier. Models put the names they want
/// into one query. Ranking files for those words would answer a question that was not asked.
fn several_symbols<'a>(ix: &Index, q: &'a str) -> Option<Vec<&'a str>> {
    let words: Vec<&str> = q.split_whitespace().collect();
    if !(2..=4).contains(&words.len()) {
        return None;
    }
    let named = |w: &&str| w.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') && super::locate::is_code_like(w) && !ix.defs_named(w.as_bytes()).is_empty();
    words.iter().all(named).then_some(words)
}

/// Words that make a query a sentence about something and not a list of terms, and the words
/// for what is wanted of a name.
const SENTENCE_WORDS: &[&str] = &[
    "the", "a", "an", "in", "of", "to", "for", "from", "with", "by", "at", "through", "into", "via", "on", "when", "where", "who", "what", "which", "how", "why", "is",
    "are", "does", "do", "that", "its", "not", "no", "calls", "call", "callers", "caller", "called", "uses", "usages", "used", "references", "definition", "defined",
];

/// `where io_uring invokes io_fsync through its opcode table`, `blk_rq_map_user_iov callers`:
/// a sentence, or a name with the word for what is wanted of it, that names one to three
/// things defined here. That is a question about those things, and their answer (the
/// definition, the body, who calls it) is the answer. Ranking files by the words of the
/// sentence finds the same place and prints thousands of tokens of its neighbours with it.
/// Returns the names, and the paths the sentence gives.
fn named_in_a_sentence(ix: &Index, q: &str) -> Option<(Vec<String>, Vec<String>)> {
    let words: Vec<&str> = q.split_whitespace().collect();
    if !(2..=12).contains(&words.len()) {
        return None;
    }
    let (mut sentence, mut names, mut paths) = (false, Vec::<String>::new(), Vec::<String>::new());
    for raw in words {
        let w = raw.trim_matches(|c: char| matches!(c, '(' | ')' | '*' | '&' | ',' | ':' | ';' | '?' | '!' | '"' | '\'' | '`'));
        if w.is_empty() {
            continue;
        }
        let shaped = super::locate::is_code_like(w);
        if !shaped && SENTENCE_WORDS.contains(&w.to_ascii_lowercase().as_str()) {
            sentence = true;
        } else if w.contains('/') || (w.contains('.') && ix.name_id(w.as_bytes()).is_none() && super::resolve_file(ix, w).is_some()) {
            paths.push(w.to_string());
        } else if shaped && w.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '$')) {
            let name = w.replace("::", ".");
            let defined = match name.rsplit_once('.') {
                Some(_) => owner_has(ix, &name),
                None => !ix.defs_named(name.as_bytes()).is_empty(),
            };
            // A word shaped like a name that names nothing here is a guess, and a guess is
            // helped by the ranking.
            if !defined {
                return None;
            }
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    (sentence && (1..=3).contains(&names.len())).then_some((names, paths))
}

/// `pp->flags |= NV_ADMA_ATAPI_SETUP_COMPLETE`, `host->n_ports`: a piece of code, which is
/// looked for as it is written (with any spacing) and not ranked as words. Returns the pattern,
/// and the files a word of the query names, to look in.
fn code_fragment(ix: &Index, q: &str) -> Option<(String, Vec<String>)> {
    // What only code has, and what prose has too: the second kind counts when a word of the
    // query is shaped like an identifier.
    const SURE: [&str; 13] = ["->", "|=", "&=", "+=", "-=", "==", "!=", "<=", ">=", "<<", ">>", "&&", "||"];
    const MAYBE: [&str; 3] = ["=", "(", "["];
    let mut paths: Vec<String> = Vec::new();
    let mut pieces: Vec<&str> = Vec::new();
    for w in q.split_whitespace() {
        // A file named along with the code says where to look.
        if w.contains('.') && !w.contains("->") && !w.contains('(') && ix.name_id(w.as_bytes()).is_none() && super::resolve_file(ix, w).is_some() {
            paths.push(w.to_string());
        } else {
            pieces.push(w);
        }
    }
    let code = pieces.join(" ");
    let has_name = pieces.iter().any(|w| w.split(|c: char| !(c.is_alphanumeric() || c == '_')).any(|t| t.len() >= 3 && super::locate::is_code_like(t)));
    let sure = SURE.iter().any(|op| code.contains(op));
    if !sure && !(has_name && MAYBE.iter().any(|op| code.contains(op))) {
        return None;
    }
    // Tokens in a row with any spacing between them, each taken literally.
    let mut pattern = String::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, pattern: &mut String| {
        if !cur.is_empty() {
            if !pattern.is_empty() {
                pattern.push_str(r"\s*");
            }
            pattern.push_str(&regex::escape(cur));
            cur.clear();
        }
    };
    let mut prev_word = false;
    for ch in code.chars() {
        let word = ch.is_alphanumeric() || ch == '_';
        if ch.is_whitespace() {
            flush(&mut cur, &mut pattern);
        } else {
            if !cur.is_empty() && word != prev_word {
                flush(&mut cur, &mut pattern);
            }
            cur.push(ch);
        }
        prev_word = word;
    }
    flush(&mut cur, &mut pattern);
    Some((pattern, paths))
}

/// What each of several answers may take of a room that all of them share: the short ones
/// take what they need, and the long ones split what that leaves evenly.
fn fair_cap(sizes: &[usize], pool: usize) -> usize {
    let mut sorted = sizes.to_vec();
    sorted.sort_unstable();
    let mut left = pool;
    for (i, &size) in sorted.iter().enumerate() {
        let share = left / (sorted.len() - i);
        if size > share {
            return share;
        }
        left -= size;
    }
    usize::MAX
}

/// How long after a hook of the plugin ran a repository counts as having its hooks at work.
const HOOKS_AT_WORK: std::time::Duration = std::time::Duration::from_secs(600);

/// The answers to the queries of one call, and the bound on all of it in tokens.
///
/// Each query gets its answer. A call that asks three things and is told one of them is a call
/// that will be made again. Measured on sessions that localise kernel bugs: with one room for
/// the whole call, half the queries of a call of several were answered in a few lines, and
/// nearly half of those were asked about again later.
///
/// The open questions of a call (free text, not a name or a place) are one question asked
/// several ways. Their words are ranked as one text and answered once, where the first of them
/// stands: the file a session is after is first in that ranking more often than in the
/// ranking of any one wording.
///
/// `budget` is the room of one query's answer. `named` says the caller chose it, and then it
/// is the room of the whole call. `open_room` gives the room of the answer to the open
/// questions where the caller named none.
pub fn search_all(
    ix: &Index,
    rs: &Resolver,
    queries: &[String],
    common: &Common,
    opts: &SearchOpts,
    budget: usize,
    named: bool,
    open_room: &mut dyn FnMut() -> usize,
) -> (String, usize) {
    let open: Vec<&str> = queries.iter().map(|q| q.trim()).filter(|q| is_free_text(ix, rs, q, common)).collect();
    // The parts of the answer in the order they were asked for: `None` is the open questions.
    let mut parts: Vec<Option<&str>> = Vec::new();
    for q in queries {
        let q = q.trim();
        if !open.contains(&q) {
            parts.push(Some(q));
        } else if !parts.contains(&None) {
            parts.push(None);
        }
    }
    // A caller that names the size of the answer gets what it got before there was source in
    // an answer: the ranking, in that size. The source is what an open question gets where
    // nothing was said about size. Measured: sessions that localise kernel bugs name no budget
    // in any of 703 calls, and sessions that ask one question about the code name one in half
    // of theirs and then want the place, not eight functions.
    //
    // The same holds where the hooks of the plugin are at work in the repository: that session
    // has been handed its card before it asked, and was measured with the ranking (0.3.0).
    // With source in the answer its kernel questions cost 1.13 of that. The source is for the
    // session that has the tools and nothing else, which is where it was measured to help.
    let pushed = crate::recent::was_hooked(&crate::build::index_dir(&ix.root), HOOKS_AT_WORK);
    let opts = &SearchOpts { source: opts.source && (budget == 0 || (!named && !pushed)), ..*opts };
    let render = |part: &Option<&str>, room: usize| -> String {
        let mut one = Out::new(room);
        match (part, &open[..]) {
            (Some(q), _) => search(ix, rs, q, common, opts, &mut one),
            (None, [q]) => search(ix, rs, q, common, opts, &mut one),
            (None, _) => {
                let code = opts.source && (room == 0 || room >= super::locate::CODE_MIN_BUDGET);
                locate(ix, &open.join("\n"), common, &LocateOpts { code, questions: open.len(), ..Default::default() }, &mut one);
            }
        }
        one.finish()
    };
    let unlimited = budget == 0;
    // Each part in the room of one answer first: most are far shorter than that.
    let mut bound = 0usize;
    let mut texts: Vec<String> = Vec::with_capacity(parts.len());
    let mut shared: Vec<usize> = Vec::new(); // the parts that share the room of the call
    for (i, part) in parts.iter().enumerate() {
        if part.is_none() && !named && !unlimited {
            // The answer to the open questions has a room of its own.
            let room = open_room();
            bound += room;
            texts.push(render(part, room));
        } else {
            texts.push(render(part, budget));
            shared.push(i);
        }
    }
    if !unlimited && !shared.is_empty() {
        // A call's answers to names and places take three such rooms at most, or the one
        // the caller named.
        let pool = if named { budget } else { budget * shared.len().min(3) };
        bound += pool;
        let sizes: Vec<usize> = shared.iter().map(|&i| crate::output::est_tokens(&texts[i])).collect();
        let cap = fair_cap(&sizes, pool);
        for (&i, &size) in shared.iter().zip(&sizes) {
            if size > cap {
                texts[i] = render(&parts[i], cap.max(150));
            }
        }
    }
    // Two wordings of one question get one answer: the second is not printed again.
    let mut said: Vec<&str> = Vec::new();
    for t in &texts {
        let t = t.trim_end();
        if !t.is_empty() && !said.contains(&t) {
            said.push(t);
        }
    }
    (said.join("\n\n") + "\n", bound)
}

pub fn search(ix: &Index, rs: &Resolver, query: &str, common: &Common, opts: &SearchOpts, out: &mut Out) {
    let q = query.trim();
    if q.is_empty() {
        out.push("empty query");
        return;
    }
    // Free text: more than one word (unless it is just a kind word and a name: `struct file`),
    // and unless the words together name one symbol.
    if q.split_whitespace().count() > 1 && super::split_kind_prefix(q).0.is_none() {
        if let Some((name, paths)) = symbol_in_words(ix, rs, q, common) {
            let mut narrowed = common.clone();
            narrowed.paths.extend(paths);
            search(ix, rs, &name, &narrowed, opts, out);
            return;
        }
        // Several names in one query: each is answered, in the room the answer has.
        if let Some(names) = several_symbols(ix, q) {
            for (i, name) in names.iter().enumerate() {
                let mut one = Out::new(if out.budget() == 0 { 0 } else { (out.remaining() / (names.len() - i)).max(150) });
                search(ix, rs, name, common, opts, &mut one);
                if i > 0 {
                    out.push("");
                }
                out.push(one.finish().trim_end());
            }
            return;
        }
        // A quoted message: the function that writes it, with its body and its callers.
        if let Some((file, line, def)) = written_at(ix, q) {
            let d = ix.def(def);
            out.push(&format!("this text is written at {}:{}, in {}:", ix.path(file), line, d.qualified()));
            let mut narrowed = common.clone();
            narrowed.paths.push(ix.path(file).to_string());
            let name = d.qualified();
            search(ix, rs, &name, &narrowed, opts, out);
            return;
        }
        // A piece of code is looked for where it is written. Where it is written nowhere,
        // its words are ranked like any other text.
        if let Some((pattern, paths)) = code_fragment(ix, q) {
            let mut narrowed = common.clone();
            narrowed.paths.extend(paths);
            let mut found = Out::new(out.remaining());
            super::grep::grep(ix, &pattern, &narrowed, &super::grep::GrepOpts::default(), &mut found);
            let text = found.finish();
            if !text.starts_with("no matches") && !text.starts_with("invalid pattern") {
                out.push(text.trim_end());
                return;
            }
        }
        // A text that stands in the code as typed: the places, each with its function.
        if stands_as_typed(ix, q, common) {
            let literal = q.trim_matches(|c| matches!(c, '"' | '\'' | '`'));
            let mut found = Out::new(out.remaining());
            super::grep::grep(ix, literal, common, &super::grep::GrepOpts { fixed: true, ..Default::default() }, &mut found);
            let text = found.finish();
            if !text.starts_with("no matches") {
                out.push(text.trim_end());
                return;
            }
        }
        // A sentence about one to three things defined here: each of them is answered.
        if let Some((names, paths)) = named_in_a_sentence(ix, q) {
            let mut narrowed = common.clone();
            narrowed.paths.extend(paths);
            for (i, name) in names.iter().enumerate() {
                let mut one = Out::new(if out.budget() == 0 { 0 } else { (out.remaining() / (names.len() - i)).max(150) });
                search(ix, rs, name, &narrowed, opts, &mut one);
                if i > 0 {
                    out.push("");
                }
                out.push(one.finish().trim_end());
            }
            return;
        }
        // An open question is answered with the code of the candidates where there is room
        // for it, and with the ranking alone where the caller named the size of the answer.
        let code = opts.source && (out.budget() == 0 || out.remaining() >= super::locate::CODE_MIN_BUDGET);
        locate(ix, q, common, &LocateOpts { code, ..Default::default() }, out);
        return;
    }
    let pf = PathFilter::new(&common.paths);
    let spec = parse_spec(ix, q);
    match &spec {
        Spec::File(f) => {
            outline(ix, *f, &ShowOpts::default(), out);
            return;
        }
        Spec::Loc { .. } | Spec::Range { .. } => {
            show(ix, rs, &[q.to_string()], common, &ShowOpts::default(), out);
            return;
        }
        Spec::Sym { .. } => {}
    }
    let ents = targets_for(ix, rs, &spec, common, &pf);
    if !ents.is_empty() {
        // One entity, or one that clearly dominates (a function with its prototypes next to
        // unrelated fields of the same name): show it in full.
        let callable_or_type = |t: &Target| matches!(t.class, Class::Callable | Class::Type | Class::Value);
        let strong: Vec<&Target> = ents.iter().filter(|t| callable_or_type(t)).collect();
        if ents.len() <= 3 || strong.len() == 1 {
            // Few candidates: show the best in full and name the others, rather than cost a
            // second call to pick one.
            let main = if strong.len() == 1 { strong[0] } else { &ents[0] };
            let others: Vec<Target> = ents.iter().filter(|t| !std::ptr::eq(*t, main)).cloned().collect();
            card(ix, rs, main, &others, opts, out);
            return;
        }
        out.push(&format!("{} symbols named {}:", ents.len(), q));
        let mut shown = 0;
        for t in ents.iter().take(8) {
            let p = t.primary(ix);
            if !out.line(&def_line(&p)) {
                break;
            }
            shown += 1;
            if let Some(l) = usage_line(ix, rs, t, 3).first() {
                out.line(&format!("  {l}"));
            }
        }
        if ents.len() > shown {
            out.push(&format!("… {} more (narrow with --path DIR or --kind, or ask for Owner.{})", ents.len() - shown, q));
        }
        return;
    }
    // `Owner.name` that nothing is called. Two things it usually is, and each has an exact
    // answer where "no matches" was a dead end: a member of that owner written a little
    // differently (`Config.unmarshal` for `Config.Unmarshal`), or the member of a type that
    // is called something else (`Remote.marshal`, where the type is `RemoteConfig`).
    if let Spec::Sym { owner: Some(owner), name, kinds } = &spec {
        let owner_of = |d: &crate::store::DefV| {
            let q = d.qualified();
            let o = q.rsplit_once(['.', ':']).map(|(o, _)| o.trim_end_matches(':').to_string()).unwrap_or_default();
            o.rsplit(['.', ':']).next().unwrap_or("").to_string()
        };
        // 1. The same owner, a similar name.
        let (near, _) = super::find::top_matches(ix, rs, name, common, 40);
        let mut mine: Vec<DefId> = near.into_iter().filter(|&d| owner_of(&ix.def(d)) == *owner).collect();
        // Written the same but for its case: that one, whatever else is near.
        let same: Vec<DefId> = mine.iter().copied().filter(|&d| ix.def(d).name_str().eq_ignore_ascii_case(name)).collect();
        if same.len() == 1 {
            mine = same;
        }
        if let [d] = mine[..] {
            let v = ix.def(d);
            let ents = rs.entities(&ix.defs_named(v.name()));
            if let Some(main) = ents.iter().find(|t| t.defs.contains(&d)) {
                out.push(&format!("no `{owner}.{name}`; the one member of {owner} with a similar name is {}:", v.qualified()));
                card(ix, rs, main, &[], opts, out);
                return;
            }
        }
        if mine.len() > 1 {
            out.push(&format!("no `{owner}.{name}`; {} members of {owner} have a similar name:", count(mine.len())));
            for d in mine.iter().take(10) {
                out.line(&def_line(&ix.def(*d)));
            }
            return;
        }
        // 2. The same name, another owner.
        let bare = Spec::Sym { owner: None, name: name.clone(), kinds: kinds.clone() };
        let mut named: Vec<Target> = targets_for(ix, rs, &bare, common, &pf).into_iter().filter(|t| !owner_of(&t.primary(ix)).is_empty()).collect();
        if !named.is_empty() {
            // One of them on a type whose name holds the name that was asked for, or is held
            // by it: that is the one that was meant.
            let asked = owner.to_lowercase();
            let alike: Vec<usize> = named
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    let o = owner_of(&t.primary(ix)).to_lowercase();
                    o.len() >= 3 && asked.len() >= 3 && (o.contains(&asked) || asked.contains(&o))
                })
                .map(|(i, _)| i)
                .collect();
            if let [i] = alike[..] {
                let main = named.remove(i);
                out.push(&format!("no `{owner}.{name}`; the one `{name}` on a type of a similar name is {}:", main.primary(ix).qualified()));
                card(ix, rs, &main, &[], opts, out);
                return;
            }
            out.push(&format!("no `{owner}.{name}`; `{name}` is defined on {} other{}:", count(named.len()), if named.len() == 1 { "" } else { "s" }));
            let mut shown = 0;
            for t in named.iter().take(10) {
                if !out.line(&def_line(&t.primary(ix))) {
                    break;
                }
                shown += 1;
            }
            if named.len() > shown {
                out.push(&format!("… {} more (codefast find {name})", count(named.len() - shown)));
            }
            return;
        }
    }
    // No symbol with exactly this name: near matches, then plain text.
    let (near, total) = super::find::top_matches(ix, rs, q, common, 8);
    if !near.is_empty() {
        if total == 1 {
            // One near match: answer about it, rather than cost a second call to ask.
            let v = ix.def(near[0]);
            let ents = rs.entities(&ix.defs_named(v.name()));
            if let Some(main) = ents.iter().find(|t| t.defs.contains(&near[0])) {
                out.push(&format!("no symbol named exactly `{q}`; the one similar name is {}:", v.name_str()));
                let others: Vec<Target> = ents.iter().filter(|t| !std::ptr::eq(*t, main)).cloned().collect();
                card(ix, rs, main, &others, opts, out);
                return;
            }
        }
        out.push(&format!("no symbol named exactly `{q}`; {} similar:", count(total)));
        for d in &near {
            out.line(&def_line(&ix.def(*d)));
        }
        if total > near.len() {
            out.push(&format!("… {} more (codefast find {q} --limit N)", count(total - near.len())));
        }
        return;
    }
    let gopts = super::grep::GrepOpts { fixed: true, ..Default::default() };
    super::grep::grep(ix, q, common, &gopts, out);
}

#[cfg(test)]
mod tests {
    use super::fair_cap;

    /// Short answers take what they need, and the long ones share what is left evenly.
    #[test]
    fn a_shared_room_is_split_among_the_answers_that_need_it() {
        // Everything fits: no cap.
        assert_eq!(fair_cap(&[200, 300, 400], 1500), usize::MAX);
        // One long answer next to two short ones gets all that they leave.
        assert_eq!(fair_cap(&[200, 300, 4000], 1500), 1000);
        // Two long answers split what the short one leaves.
        assert_eq!(fair_cap(&[100, 4000, 4000], 1500), 700);
        // All of them long: an even split.
        assert_eq!(fair_cap(&[4000, 4000, 4000], 1500), 500);
        assert_eq!(fair_cap(&[], 1500), usize::MAX);
    }
}
