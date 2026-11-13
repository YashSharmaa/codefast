//! Free-text localisation: a bug report, a question or a stack trace goes in; ranked files and
//! symbols come out. No language model: the query is analysed lexically (identifiers, paths,
//! stack frames, plain words), scored with BM25 over the word index, and adjusted with
//! structural evidence (which files define the named symbols, which paths carry the words).

use super::find::split_words;
use super::{Common, PathFilter, sig_of};
use crate::facts::{DefKind, fflag};
use crate::output::{Out, count};
use crate::store::{DefId, FileId, Index, NameId};
use rustc_hash::{FxHashMap, FxHashSet};

#[derive(Clone, Debug)]
pub struct Term {
    pub text: String,
    /// How much the query cares about this term (code-like tokens weigh more than prose).
    pub weight: f32,
    pub kind: TermKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TermKind {
    /// Looks like a symbol: `tcp_v4_connect`, `FooBar`, `MAX_LEN`, a stack frame.
    Ident,
    /// Plain word.
    Word,
}

#[derive(Default, Debug)]
pub struct Query {
    pub terms: Vec<Term>,
    /// Path fragments mentioned in the text (`fs/ext4/inode.c`, `inode.c`).
    pub paths: Vec<String>,
    /// Functions named by stack frames, innermost first.
    pub frames: Vec<String>,
    pub mentions_tests: bool,
}

const STOP: &[&str] = &[
    "the",
    "and",
    "for",
    "with",
    "this",
    "that",
    "when",
    "from",
    "are",
    "was",
    "were",
    "has",
    "have",
    "had",
    "not",
    "but",
    "can",
    "will",
    "should",
    "would",
    "could",
    "into",
    "then",
    "than",
    "also",
    "any",
    "all",
    "its",
    "use",
    "used",
    "using",
    "does",
    "did",
    "get",
    "got",
    "see",
    "new",
    "one",
    "two",
    "may",
    "via",
    "per",
    "out",
    "you",
    "your",
    "our",
    "their",
    "there",
    "here",
    "which",
    "what",
    "why",
    "how",
    "where",
    "who",
    "been",
    "being",
    "some",
    "such",
    "just",
    "only",
    "more",
    "most",
    "after",
    "before",
    "about",
    "above",
    "below",
    "because",
    "while",
    "these",
    "those",
    "they",
    "them",
    "like",
    "want",
    "need",
    "make",
    "made",
    "does",
    "doing",
    "still",
    "now",
    "even",
    "too",
    "very",
    "much",
    "many",
    "each",
    "other",
    "same",
    "both",
    "few",
    "http",
    "https",
    "www",
    "com",
    "org",
    "github",
    "issue",
    "issues",
    "please",
    "thanks",
    "thank",
    "hello",
    "seems",
    "seem",
    "following",
    "follows",
    "example",
    "expected",
    "actual",
    "actually",
    "instead",
    "however",
    "since",
];

const SRC_EXT: &[&str] = &[
    "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "py", "pyi", "js", "jsx", "mjs", "ts", "tsx", "go", "rs", "java", "kt", "rb", "php", "cs", "swift", "scala",
    "lua", "sh", "S", "s", "asm", "dts", "dtsi", "yaml", "yml", "json", "toml", "md", "rst", "txt", "cfg", "ini", "html", "css", "sql", "proto", "cmake", "mk",
];

fn is_code_like(tok: &str) -> bool {
    let b = tok.as_bytes();
    if b.len() < 3 {
        return false;
    }
    let has_us = b.contains(&b'_');
    let has_digit = b.iter().any(|c| c.is_ascii_digit());
    let upper = b.iter().filter(|c| c.is_ascii_uppercase()).count();
    let lower = b.iter().filter(|c| c.is_ascii_lowercase()).count();
    let inner_upper = b[1..].iter().any(|c| c.is_ascii_uppercase()) && lower > 0;
    has_us || inner_upper || (upper >= 3 && lower == 0) || (has_digit && lower + upper >= 2)
}

/// Analyse free text into weighted terms, path mentions and stack frames.
pub fn analyse(text: &str) -> Query {
    let mut q = Query::default();
    let mut weights: FxHashMap<(String, TermKind), f32> = FxHashMap::default();
    let mut order: Vec<(String, TermKind)> = Vec::new();
    let add = |tok: &str, kind: TermKind, w: f32, weights: &mut FxHashMap<(String, TermKind), f32>, order: &mut Vec<(String, TermKind)>| {
        let key = (tok.to_string(), kind);
        match weights.get_mut(&key) {
            // Repeated mentions matter, with diminishing returns.
            Some(x) => *x += w * 0.3,
            None => {
                weights.insert(key.clone(), w);
                order.push(key);
            }
        }
    };
    let lower_all = text.to_ascii_lowercase();
    q.mentions_tests = lower_all.contains("test");
    // Chunks: runs of characters that can be part of an identifier, a path or a qualified name.
    let is_chunk = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | ':' | '-' | '+' | '>' | '#');
    let mut prev_tick = false;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let (start, c) = chars[i];
        if !is_chunk(c) {
            prev_tick = c == '`' || c == '\'' || c == '"';
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && is_chunk(chars[j].1) {
            j += 1;
        }
        let end = if j < chars.len() { chars[j].0 } else { text.len() };
        let followed_by_paren = j < chars.len() && chars[j].1 == '(';
        let quoted = prev_tick;
        prev_tick = false;
        let raw = &text[start..end];
        i = j;
        let chunk = raw.trim_matches(|c: char| matches!(c, '.' | ':' | '-' | '/' | '+' | '>' | '#'));
        if chunk.len() < 2 {
            continue;
        }
        // Kernel stack frame: func+0x1a/0x60
        if let Some(p) = chunk.find("+0x") {
            let f = &chunk[..p];
            if f.len() >= 3 && f.bytes().all(|b| b == b'_' || b == b'.' || b.is_ascii_alphanumeric()) {
                let f = f.split('.').next().unwrap_or(f); // strip .constprop.0 / .isra.0
                q.frames.push(f.to_string());
                add(f, TermKind::Ident, 3.0, &mut weights, &mut order);
                continue;
            }
        }
        // Path mention.
        let ext = chunk.rsplit('.').next().unwrap_or("");
        let looks_file = chunk.contains('.') && SRC_EXT.contains(&ext) && !chunk.ends_with('.');
        if chunk.contains('/') || looks_file {
            let p = chunk.split(':').next().unwrap_or(chunk).trim_start_matches("./").trim_start_matches("a/").trim_start_matches("b/");
            if p.len() >= 3 && (looks_file || p.matches('/').count() >= 1) && !p.starts_with("//") {
                q.paths.push(p.to_string());
            }
            // The path's components are also evidence.
            for part in p.split(|c: char| c == '/' || c == '.') {
                if part.len() >= 3 && !SRC_EXT.contains(&part) {
                    let kind = if is_code_like(part) { TermKind::Ident } else { TermKind::Word };
                    let lowered = part.to_ascii_lowercase();
                    add(if kind == TermKind::Word { &lowered } else { part }, kind, 1.0, &mut weights, &mut order);
                }
            }
            continue;
        }
        // Qualified names: a.b.c, a::b, a->b, A#b
        for part in chunk.split(|c: char| matches!(c, '.' | ':' | '-' | '>' | '#' | '+')) {
            if part.len() < 2 || part.bytes().all(|b| b.is_ascii_digit()) || part.as_bytes()[0].is_ascii_digit() {
                continue;
            }
            if !part.bytes().all(|b| b == b'_' || b.is_ascii_alphanumeric()) {
                continue;
            }
            if is_code_like(part) || followed_by_paren || quoted {
                let mut w = 2.0;
                if quoted {
                    w += 0.5;
                }
                if followed_by_paren {
                    w += 0.5;
                }
                add(part, TermKind::Ident, w, &mut weights, &mut order);
            } else {
                let lw = part.to_ascii_lowercase();
                if lw.len() >= 3 && !STOP.contains(&lw.as_str()) {
                    add(&lw, TermKind::Word, 1.0, &mut weights, &mut order);
                }
            }
        }
    }
    q.terms = order.into_iter().map(|k| Term { weight: weights[&k], text: k.0, kind: k.1 }).collect();
    q.paths.sort();
    q.paths.dedup();
    q
}

pub struct Ranked {
    pub files: Vec<(FileId, f32)>,
    pub defs: Vec<(DefId, f32)>,
    /// Number of files with any evidence.
    pub candidates: usize,
    /// Places where a run of words of the query occurs verbatim in a file.
    pub phrases: Vec<PhraseHit>,
}

/// A run of consecutive words of the query found verbatim in a file: the strongest lexical
/// evidence there is (a log line, an error message, a quoted piece of code).
#[derive(Clone, Debug)]
pub struct PhraseHit {
    pub file: FileId,
    pub line: u32,
    pub text: String,
    /// In how many files the phrase occurs.
    pub spread: u32,
    /// The occurrence is inside a string literal.
    pub quoted: bool,
}

const PHRASE_WORDS: usize = 3;
/// A phrase that occurs in more files than this says little about any of them.
const PHRASE_MAX_FILES: usize = 24;
const PHRASE_MAX_READS: usize = 2500;

/// Runs of `PHRASE_WORDS` consecutive words of the query, as written (separators included).
fn query_phrases(text: &str) -> Vec<(&str, Vec<&str>)> {
    let mut out: Vec<(&str, Vec<&str>)> = Vec::new();
    let mut seen: FxHashSet<&str> = FxHashSet::default();
    for line in text.lines() {
        let b = line.as_bytes();
        // Word spans of the line.
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let mut i = 0usize;
        while i < b.len() {
            if !(b[i].is_ascii_alphabetic() || b[i] == b'_') {
                // A number is not a word, but it does not end a word either.
                if b[i].is_ascii_digit() {
                    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
                continue;
            }
            let st = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            if (crate::text::MIN_WORD..=crate::text::MAX_WORD).contains(&(i - st)) {
                spans.push((st, i));
            }
        }
        if spans.len() < PHRASE_WORDS {
            continue;
        }
        for w in spans.windows(PHRASE_WORDS) {
            // Consecutive words: one or two separator bytes between neighbours, no more.
            let tight = w.windows(2).all(|p| {
                let gap = &b[p[0].1..p[1].0];
                !gap.is_empty() && gap.len() <= 2 && gap.iter().all(|c| c.is_ascii_punctuation() || *c == b' ')
            });
            if !tight {
                continue;
            }
            let raw = &line[w[0].0..w[PHRASE_WORDS - 1].1];
            let words: Vec<&str> = w.iter().map(|&(a, e)| &line[a..e]).collect();
            // Needs some substance: not three short function words.
            let long = words.iter().filter(|x| x.len() >= 5).count();
            let stop = words.iter().filter(|x| STOP.contains(&x.to_ascii_lowercase().as_str()) || x.len() < 3).count();
            if raw.len() < 12 || long == 0 || stop >= 2 {
                continue;
            }
            if seen.insert(raw) {
                out.push((raw, words));
            }
        }
    }
    out
}

/// Find the query's phrases in the repository: word-index intersection, then a literal check.
fn phrase_hits(ix: &Index, text: &str) -> Vec<PhraseHit> {
    let mut cands: Vec<(usize, &str, Vec<u32>)> = Vec::new();
    let mut posting_cache: FxHashMap<&str, Option<FxHashSet<u32>>> = FxHashMap::default();
    for (raw, words) in query_phrases(text) {
        // Rarest word first: a phrase whose rarest word is everywhere is not worth the reads.
        let mut ids: Vec<(u32, &str, NameId)> = Vec::new();
        let mut ok = true;
        for w in &words {
            match ix.name_id(w.as_bytes()) {
                Some(n) => ids.push((ix.popularity(n), w, n)),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        ids.sort_by_key(|x| x.0);
        let mut files: Vec<u32> = ix.postings(ids[0].2).map(|(f, _)| f.0).take(20_000).collect();
        if files.len() >= 20_000 {
            continue;
        }
        for (_, w, n) in ids.iter().skip(1) {
            let set = posting_cache.entry(*w).or_insert_with(|| {
                let v: FxHashSet<u32> = ix.postings(*n).map(|(f, _)| f.0).collect();
                Some(v)
            });
            if let Some(set) = set {
                files.retain(|f| set.contains(f));
            }
            if files.is_empty() {
                break;
            }
        }
        if files.is_empty() || files.len() > 400 {
            continue;
        }
        cands.push((files.len(), raw, files));
    }
    // Cheapest first, within a budget of file reads.
    cands.sort_by_key(|c| c.0);
    let mut reads = 0usize;
    let mut out: Vec<PhraseHit> = Vec::new();
    for (n, raw, files) in cands {
        if reads + n > PHRASE_MAX_READS {
            break;
        }
        reads += n;
        let finder = memchr::memmem::Finder::new(raw.as_bytes());
        let mut hits: Vec<(FileId, u32, bool)> = Vec::new();
        for f in files {
            let fid = FileId(f);
            if !ix.is_live(fid) {
                continue;
            }
            let Some(src) = ix.source(fid) else { continue };
            // Prefer an occurrence inside a string literal (the message itself) to one in a
            // comment that talks about it.
            let mut best: Option<(usize, bool)> = None;
            for pos in finder.find_iter(&src).take(16) {
                let ls = memchr::memrchr(b'\n', &src[..pos]).map(|i| i + 1).unwrap_or(0);
                let quoted = memchr::memchr(b'"', &src[ls..pos]).is_some();
                if best.is_none() || (quoted && !best.is_some_and(|b| b.1)) {
                    best = Some((pos, quoted));
                }
                if quoted {
                    break;
                }
            }
            if let Some((pos, quoted)) = best {
                let line = 1 + memchr::memchr_iter(b'\n', &src[..pos]).count() as u32;
                hits.push((fid, line, quoted));
            }
        }
        if hits.is_empty() || hits.len() > PHRASE_MAX_FILES {
            continue;
        }
        let spread = hits.len() as u32;
        for (file, line, quoted) in hits {
            out.push(PhraseHit { file, line, text: raw.to_string(), spread, quoted });
        }
    }
    out
}

struct Acc {
    score: Vec<f32>,
    hits: Vec<u16>,
}

impl Acc {
    fn new(n: usize) -> Acc {
        Acc { score: vec![0.0; n], hits: vec![0; n] }
    }
}

/// Weight of one verbatim phrase that occurs in a single file.
const PHRASE_W: f32 = 14.0;
const K1: f32 = 1.2;
const B: f32 = 0.4;

fn idf(n: f32, df: f32) -> f32 {
    (1.0 + (n - df + 0.5) / (df + 0.5)).ln()
}

/// Names in the lexicon that contain `frag` at an identifier word boundary.
fn names_with_subword(ix: &Index, frag_lower: &[u8], cap: usize) -> Vec<NameId> {
    // Case variants cover snake_case, camelCase and CONSTANT_CASE spellings.
    let mut variants: Vec<Vec<u8>> = vec![frag_lower.to_vec(), frag_lower.to_ascii_uppercase()];
    let mut cap_v = frag_lower.to_vec();
    if let Some(c) = cap_v.first_mut() {
        *c = c.to_ascii_uppercase();
    }
    variants.push(cap_v);
    let mut ids = ix.scan_lexicon(|blob, offs, out| {
        for v in &variants {
            let finder = memchr::memmem::Finder::new(v);
            let mut pos = 0usize;
            while pos < blob.len() && out.len() < cap {
                let Some(i) = finder.find(&blob[pos..]) else { break };
                let at = pos + i;
                let id = offs.partition_point(|&o| o as usize <= at) - 1;
                let (s, e) = (offs[id] as usize, offs[id + 1] as usize);
                pos = at + 1;
                if at + v.len() > e {
                    continue;
                }
                let name = &blob[s..e];
                let rel = at - s;
                // Word boundary on the left; the right side may continue (plural, suffix).
                let left_ok = rel == 0 || !name[rel - 1].is_ascii_alphanumeric() || (name[rel - 1].is_ascii_lowercase() && name[rel].is_ascii_uppercase());
                if left_ok && name[0] != b'/' && !name.contains(&b':') && !name.contains(&b'.') {
                    out.push(id as u32);
                    pos = e;
                }
            }
        }
        out.sort_unstable();
        out.dedup();
    });
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Rank files and symbols for a free-text query.
pub fn rank(ix: &Index, text: &str, common: &Common) -> (Query, Ranked) {
    let q = analyse(text);
    let nfiles = ix.n_files() as usize;
    let pf = PathFilter::new(&common.paths);
    let live: Vec<FileId> = ix.files().collect();
    let text_files = live.iter().filter(|&&f| ix.file(f).hash != 0).count().max(1) as f32;
    let avg_len = (live.iter().map(|&f| ix.file(f).size as f64).sum::<f64>() / text_files as f64).max(1.0) as f32;
    let mut acc = Acc::new(nfiles);
    let mut def_scores: FxHashMap<DefId, f32> = FxHashMap::default();

    // Keep the strongest terms: long reports repeat themselves.
    let mut terms = q.terms.clone();
    terms.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
    terms.truncate(48);

    for t in &terms {
        // 1. The term as a whole word in file text.
        let mut variants: Vec<String> = vec![t.text.clone()];
        if t.kind == TermKind::Word {
            let mut cap = t.text.clone();
            if let Some(c) = cap.get_mut(0..1) {
                c.make_ascii_uppercase();
            }
            variants.push(cap);
            variants.push(t.text.to_ascii_uppercase());
        }
        let mut posts: FxHashMap<u32, u32> = FxHashMap::default();
        let mut has_def = false;
        for v in &variants {
            if let Some(nid) = ix.name_id(v.as_bytes()) {
                for (f, tf) in ix.postings(nid) {
                    *posts.entry(f.0).or_default() += tf as u32;
                }
                // 2. Files that DEFINE a symbol with this exact name are prime suspects.
                let defs: Vec<DefId> = ix.defs_named_id(nid).collect();
                if !defs.is_empty() && t.kind == TermKind::Ident {
                    has_def = true;
                    let real: Vec<&DefId> = defs.iter().filter(|d| !matches!(ix.def(**d).kind(), DefKind::Field | DefKind::EnumMember)).collect();
                    let share = 6.0 * t.weight / (real.len().max(1) as f32).sqrt();
                    for d in real.iter().take(64) {
                        let v = ix.def(**d);
                        let bonus = if v.is_decl() { share * 0.4 } else { share };
                        acc.score[v.file().0 as usize] += bonus;
                        *def_scores.entry(**d).or_default() += bonus;
                    }
                }
            }
        }
        let df = posts.len() as f32;
        if df > 0.0 {
            let w = idf(text_files, df) * t.weight * if has_def { 1.3 } else { 1.0 };
            for (&f, &tf) in &posts {
                let dl = ix.file(FileId(f)).size as f32;
                let tfn = tf as f32 / (tf as f32 + K1 * (1.0 - B + B * dl / avg_len));
                acc.score[f as usize] += w * tfn;
                acc.hits[f as usize] += 1;
            }
        }
        // 3. The term as part of longer identifiers (`timeout` in `conn_timeout`): weaker
        //    evidence, and the way plain words reach code.
        let frag = t.text.to_ascii_lowercase();
        if frag.len() >= 4 {
            let names = names_with_subword(ix, frag.as_bytes(), 3000);
            if !names.is_empty() && names.len() < 3000 {
                let mut sub: FxHashMap<u32, u32> = FxHashMap::default();
                for &n in &names {
                    if variants.iter().any(|v| ix.name(n) == v.as_bytes()) {
                        continue;
                    }
                    for (f, tf) in ix.postings(n) {
                        *sub.entry(f.0).or_default() += tf as u32;
                    }
                    // Symbols whose names contain the word.
                    for d in ix.defs_named_id(n).take(8) {
                        let v = ix.def(d);
                        if matches!(v.kind(), DefKind::Field | DefKind::EnumMember) {
                            continue;
                        }
                        *def_scores.entry(d).or_default() += 0.6 * t.weight / (names.len() as f32).sqrt();
                    }
                }
                let sdf = sub.len() as f32;
                if sdf > 0.0 {
                    let w = idf(text_files, sdf) * t.weight * 0.45;
                    for (&f, &tf) in &sub {
                        let dl = ix.file(FileId(f)).size as f32;
                        let tfn = tf as f32 / (tf as f32 + K1 * (1.0 - B + B * dl / avg_len));
                        acc.score[f as usize] += w * tfn;
                        if !posts.contains_key(&f) {
                            acc.hits[f as usize] += 1;
                        }
                    }
                }
            }
        }
        // 4. The term in the file's path.
        let key = format!("/{}", frag);
        if let Some(pid) = ix.name_id(key.as_bytes()) {
            let pfiles: Vec<u32> = ix.postings(pid).map(|(f, _)| f.0).collect();
            let w = idf(text_files, pfiles.len() as f32) * t.weight * 0.9;
            for f in pfiles {
                acc.score[f as usize] += w;
            }
        }
    }

    // Explicit path mentions.
    for p in &q.paths {
        let suffix = format!("/{p}");
        for &f in &live {
            let path = ix.path(f);
            if path == p || path.ends_with(&suffix) {
                acc.score[f.0 as usize] += 25.0;
            }
        }
    }

    // Stack frames: the defining files, innermost frames first.
    for (depth, fr) in q.frames.iter().enumerate() {
        let w = 10.0 / (1.0 + depth as f32 * 0.5);
        let defs = ix.defs_named(fr.as_bytes());
        let n = defs.iter().filter(|d| ix.def(**d).kind().is_callable() && !ix.def(**d).is_decl()).count().max(1) as f32;
        for d in defs {
            let v = ix.def(d);
            if v.kind().is_callable() && !v.is_decl() {
                acc.score[v.file().0 as usize] += w / n.sqrt();
                *def_scores.entry(d).or_default() += w / n.sqrt();
            }
        }
    }

    // Verbatim phrases: a log line or an error message of the report found in a file.
    let phrases = phrase_hits(ix, text);
    {
        let mut per_file: FxHashMap<FileId, f32> = FxHashMap::default();
        for h in &phrases {
            let w = PHRASE_W / (h.spread as f32).sqrt() * if h.quoted { 1.0 } else { 0.6 };
            // Overlapping runs of one message count with diminishing returns.
            let e = per_file.entry(h.file).or_default();
            let add = w / (1.0 + *e / PHRASE_W);
            *e += add;
            acc.score[h.file.0 as usize] += add;
            acc.hits[h.file.0 as usize] += 1;
            if let Some(d) = ix.def_at(h.file, h.line) {
                *def_scores.entry(d.id).or_default() += add;
            }
        }
    }

    // Coordination and priors.
    let mut ranked: Vec<(FileId, f32)> = Vec::new();
    for &fid in &live {
        let f = fid.0 as usize;
        let mut s = acc.score[f];
        if s <= 0.0 {
            continue;
        }
        let rec = ix.file(fid);
        if !pf.matches(ix.path(fid)) {
            continue;
        }
        // Matching several distinct terms beats matching one term many times.
        s *= 1.0 + 0.12 * (acc.hits[f].min(12) as f32);
        if rec.flags & fflag::TEST != 0 && !q.mentions_tests {
            s *= 0.35;
        }
        if rec.flags & fflag::DOC != 0 {
            s *= 0.3;
        }
        if rec.flags & (fflag::VENDOR | fflag::GENERATED) != 0 {
            s *= 0.4;
        }
        if rec.flags & fflag::UNPARSED != 0 {
            s *= 0.6;
        }
        if rec.flags & fflag::HEADER != 0 {
            s *= 0.8;
        }
        ranked.push((fid, s));
    }
    let candidates = ranked.len();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(200);

    // Inside the best files, the functions whose own lines carry the query's terms. (Letting
    // the best function lift its file was tried and did not help file ranking.)
    body_evidence(ix, &ranked, &terms, text_files, &mut def_scores);

    // Symbols: evidence from names, scaled by how well their file did.
    let file_score: FxHashMap<FileId, f32> = ranked.iter().cloned().collect();
    let top = ranked.first().map(|x| x.1).unwrap_or(1.0).max(1e-6);
    let mut defs: Vec<(DefId, f32)> = def_scores
        .into_iter()
        .filter_map(|(d, s)| {
            let v = ix.def(d);
            let fs = *file_score.get(&v.file())?;
            let mut s = s * (0.5 + fs / top);
            if v.is_decl() {
                s *= 0.5;
            }
            Some((d, s))
        })
        .collect();
    defs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
    defs.truncate(200);
    (q, Ranked { files: ranked, defs, candidates, phrases })
}

/// How many of the best files have their functions read and scored.
const BODY_FILES: usize = 15;
const BODY_W: f32 = 1.0;

/// Score the functions of the best files by the query terms their own text contains: a term as
/// a whole identifier, or as a word inside a longer identifier.
fn body_evidence(ix: &Index, files: &[(FileId, f32)], terms: &[Term], nfiles: f32, def_scores: &mut FxHashMap<DefId, f32>) {
    let mut exact: FxHashMap<&[u8], (usize, f32)> = FxHashMap::default();
    let mut words: FxHashMap<String, (usize, f32)> = FxHashMap::default();
    for (i, t) in terms.iter().enumerate() {
        let df = ix.name_id(t.text.as_bytes()).map(|n| ix.popularity(n)).unwrap_or(1).max(1) as f32;
        let w = idf(nfiles, df) * t.weight;
        match t.kind {
            TermKind::Ident => {
                exact.insert(t.text.as_bytes(), (i, w));
                for sub in split_words(&t.text) {
                    if sub.len() >= 4 {
                        words.entry(sub).or_insert((i, w * 0.3));
                    }
                }
            }
            TermKind::Word => {
                words.entry(t.text.to_ascii_lowercase()).or_insert((i, w * 0.7));
            }
        }
    }
    if exact.is_empty() && words.is_empty() {
        return;
    }
    let mut counts: Vec<u32> = vec![0; terms.len()];
    let mut weight_of: Vec<f32> = vec![0.0; terms.len()];
    for (file, _) in files.iter().take(BODY_FILES) {
        let Some(src) = ix.source(*file) else { continue };
        let starts = super::show::line_starts(&src);
        for d in ix.defs_of_file(*file) {
            if !d.kind().is_callable() || d.is_decl() || d.rec.end_line <= d.rec.line {
                continue;
            }
            let a = starts.get(d.rec.line as usize - 1).copied().unwrap_or(src.len());
            let b = starts.get(d.rec.end_line as usize).copied().unwrap_or(src.len());
            if a >= b || b > src.len() {
                continue;
            }
            counts.iter_mut().for_each(|c| *c = 0);
            let body = &src[a..b];
            let mut i = 0usize;
            while i < body.len() {
                if !(body[i].is_ascii_alphabetic() || body[i] == b'_') {
                    i += 1;
                    continue;
                }
                let st = i;
                while i < body.len() && (body[i].is_ascii_alphanumeric() || body[i] == b'_') {
                    i += 1;
                }
                let tok = &body[st..i];
                if tok.len() < 3 {
                    continue;
                }
                if let Some(&(t, w)) = exact.get(tok) {
                    counts[t] += 1;
                    weight_of[t] = weight_of[t].max(w);
                } else if !words.is_empty() {
                    if let Ok(ts) = std::str::from_utf8(tok) {
                        for sub in split_words(ts) {
                            if let Some(&(t, w)) = words.get(&sub) {
                                counts[t] += 1;
                                weight_of[t] = weight_of[t].max(w);
                            }
                        }
                    }
                }
            }
            let mut sc = 0.0f32;
            let mut distinct = 0u32;
            for t in 0..terms.len() {
                if counts[t] > 0 {
                    let c = counts[t] as f32;
                    sc += weight_of[t] * c / (c + 1.5);
                    distinct += 1;
                }
            }
            if distinct > 0 {
                // Several different terms in one function say more than one term repeated.
                sc *= 1.0 + 0.15 * (distinct.min(8) as f32 - 1.0);
                *def_scores.entry(d.id).or_default() += BODY_W * sc;
            }
        }
    }
}

/// Best symbols of a file for this query: those already scored, else the file's largest
/// functions that mention the query's terms.
fn file_symbols(ix: &Index, file: FileId, ranked: &Ranked, q: &Query, limit: usize) -> Vec<DefId> {
    let mut out: Vec<DefId> = ranked.defs.iter().filter(|(d, _)| ix.def(*d).file() == file).map(|(d, _)| *d).take(limit).collect();
    if out.len() >= limit {
        return out;
    }
    // Fall back to word overlap between symbol names and query terms.
    let words: FxHashSet<String> = q.terms.iter().flat_map(|t| split_words(&t.text)).filter(|w| w.len() >= 3).collect();
    let mut scored: Vec<(usize, DefId)> = Vec::new();
    for d in ix.defs_of_file(file) {
        if !(d.kind().is_callable() || d.kind().is_type()) || d.is_decl() || out.contains(&d.id) {
            continue;
        }
        let name = d.name_str();
        let overlap = split_words(&name).iter().filter(|w| words.contains(*w)).count();
        if overlap > 0 {
            scored.push((overlap, d.id));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    for (_, d) in scored {
        if out.len() >= limit {
            break;
        }
        out.push(d);
    }
    out
}

pub struct LocateOpts {
    pub top_files: usize,
    pub syms_per_file: usize,
    /// Machine-readable ranking: `F<TAB>path` rows for files, then `S<TAB>path<TAB>name<TAB>line`
    /// rows for symbols, best first.
    pub plain: bool,
}

impl Default for LocateOpts {
    fn default() -> Self {
        LocateOpts { top_files: 10, syms_per_file: 3, plain: false }
    }
}

pub fn locate(ix: &Index, text: &str, common: &Common, opts: &LocateOpts, out: &mut Out) {
    let (q, ranked) = rank(ix, text, common);
    if opts.plain {
        for (f, _) in ranked.files.iter().take(opts.top_files) {
            out.push(&format!("F\t{}", ix.path(*f)));
        }
        for (d, _) in ranked.defs.iter().take(opts.top_files * 5) {
            let v = ix.def(*d);
            out.push(&format!("S\t{}\t{}\t{}", v.path(), v.name_str(), v.rec.line));
        }
        return;
    }
    if ranked.files.is_empty() {
        out.push("no file matches the query's terms (try fewer, more specific words, or `codefast grep`)");
        return;
    }
    // Where the query's own words occur verbatim (a log line, an error message).
    let mut exact: Vec<&PhraseHit> = ranked.phrases.iter().filter(|h| h.spread <= 3).collect();
    exact.sort_by(|a, b| b.quoted.cmp(&a.quoted).then_with(|| a.spread.cmp(&b.spread)).then_with(|| b.text.len().cmp(&a.text.len())));
    let mut shown: FxHashSet<(FileId, u32)> = FxHashSet::default();
    let mut n_exact = 0;
    for h in exact {
        if n_exact >= 4 || !shown.insert((h.file, h.line)) {
            continue;
        }
        if n_exact == 0 {
            out.push("exact text:");
        }
        let encl = ix.def_at(h.file, h.line).map(|d| format!(" in {}", d.name_str())).unwrap_or_default();
        out.line(&format!("  {}:{}{}  \"{}\"", ix.path(h.file), h.line, encl, crate::output::clip(&h.text, 60)));
        n_exact += 1;
    }
    out.push(&format!("{} candidate files; top {}:", count(ranked.candidates), ranked.files.len().min(opts.top_files)));
    for (i, (f, _)) in ranked.files.iter().take(opts.top_files).enumerate() {
        if !out.line(&format!("{}. {}", i + 1, ix.path(*f))) {
            break;
        }
        for d in file_symbols(ix, *f, &ranked, &q, opts.syms_per_file) {
            let v = ix.def(d);
            if !out.line(&format!("   {}-{} {}", v.rec.line, v.rec.end_line, sig_of(&v))) {
                break;
            }
        }
    }
    if ranked.candidates > opts.top_files {
        out.push(&format!(
            "… {} more candidates (narrow with --path DIR or add identifiers from the report)",
            count(ranked.candidates - opts.top_files.min(ranked.candidates))
        ));
    }
}
