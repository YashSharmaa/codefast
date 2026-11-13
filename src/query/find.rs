//! `find`: ranked symbol search by name. Exact, qualified, prefix, word-boundary, substring and
//! multi-term matching, with prototypes folded into their definitions.

use super::{Common, PathFilter, def_line};
use crate::output::{Out, count};
use crate::resolve::{Resolver, def_rank};
use crate::store::{DefId, Index, NameId};
use rustc_hash::FxHashMap;

pub struct FindOpts {
    pub limit: usize,
    pub regex: bool,
}

impl Default for FindOpts {
    fn default() -> Self {
        FindOpts { limit: 15, regex: false }
    }
}

/// Local name ids in one lexicon whose text contains `needle` (case-sensitive).
pub fn scan_blob(blob: &[u8], offs: &[u32], needle: &[u8], out: &mut Vec<u32>) {
    if needle.is_empty() {
        return;
    }
    let finder = memchr::memmem::Finder::new(needle);
    let mut pos = 0usize;
    while pos < blob.len() {
        let Some(i) = finder.find(&blob[pos..]) else { break };
        let at = pos + i;
        let id = offs.partition_point(|&o| o as usize <= at) - 1;
        let end = offs[id + 1] as usize;
        if at + needle.len() <= end {
            out.push(id as u32);
            pos = end;
        } else {
            // The match straddles two names: not a real hit.
            pos = at + 1;
        }
    }
}

/// Names containing any of the needles.
fn scan_names(ix: &Index, needles: &[Vec<u8>]) -> Vec<NameId> {
    let mut ids = ix.scan_lexicon(|blob, offs, out| {
        for n in needles {
            scan_blob(blob, offs, n, out);
        }
        out.sort_unstable();
        out.dedup();
    });
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn case_variants(q: &str) -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = vec![q.as_bytes().to_vec(), q.to_ascii_lowercase().into_bytes(), q.to_ascii_uppercase().into_bytes()];
    let mut cap = q.to_ascii_lowercase().into_bytes();
    if let Some(c) = cap.first_mut() {
        *c = c.to_ascii_uppercase();
    }
    v.push(cap);
    v.sort();
    v.dedup();
    v
}

fn contains_ci(hay: &[u8], needle_lower: &[u8]) -> Option<usize> {
    if needle_lower.is_empty() || hay.len() < needle_lower.len() {
        return None;
    }
    (0..=hay.len() - needle_lower.len()).find(|&i| hay[i..i + needle_lower.len()].iter().zip(needle_lower).all(|(a, b)| a.to_ascii_lowercase() == *b))
}

/// Is position `i` of `name` the start of an identifier word (`foo_bar`, `fooBar`, `FOOBar`)?
fn is_word_start(name: &[u8], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let (p, c) = (name[i - 1], name[i]);
    !p.is_ascii_alphanumeric() || (p.is_ascii_lowercase() && c.is_ascii_uppercase()) || (p.is_ascii_digit() != c.is_ascii_digit() && !c.is_ascii_lowercase())
}

/// Score how well `name` matches one query term; 0 = no match.
fn term_score(name: &[u8], term: &str, term_lower: &[u8]) -> i64 {
    if name == term.as_bytes() {
        return 1000;
    }
    if name.eq_ignore_ascii_case(term.as_bytes()) {
        return 900;
    }
    let Some(at) = contains_ci(name, term_lower) else { return 0 };
    let extra = (name.len() - term_lower.len()).min(60) as i64;
    let ends_at_boundary = at + term_lower.len() == name.len() || is_word_start(name, at + term_lower.len());
    if at == 0 && ends_at_boundary {
        760 - extra
    } else if at == 0 {
        700 - extra
    } else if is_word_start(name, at) && ends_at_boundary {
        640 - extra
    } else if is_word_start(name, at) {
        560 - extra
    } else {
        400 - extra
    }
}

/// Split `tcpV4Connect` / `tcp_v4_connect` / `tcp connect` into lowercase words.
pub fn split_words(s: &str) -> Vec<String> {
    let b = s.as_bytes();
    let mut words = Vec::new();
    let mut start = None;
    for i in 0..b.len() {
        let c = b[i];
        if !c.is_ascii_alphanumeric() {
            if let Some(st) = start.take() {
                words.push(s[st..i].to_ascii_lowercase());
            }
            continue;
        }
        match start {
            None => start = Some(i),
            Some(st) => {
                let p = b[i - 1];
                let boundary = (p.is_ascii_lowercase() && c.is_ascii_uppercase())
                    || (p.is_ascii_uppercase() && c.is_ascii_uppercase() && b.get(i + 1).is_some_and(|n| n.is_ascii_lowercase()));
                if boundary {
                    words.push(s[st..i].to_ascii_lowercase());
                    start = Some(i);
                }
            }
        }
    }
    if let Some(st) = start {
        words.push(s[st..].to_ascii_lowercase());
    }
    words.retain(|w| !w.is_empty());
    words
}

struct Row {
    score: i64,
    primary: DefId,
    others: usize,
    decls: usize,
}

/// Rank the definitions matching `query`. Returns rows best-first and the total match count.
fn search(ix: &Index, rs: &Resolver, query: &str, common: &Common, pf: &PathFilter, opts: &FindOpts) -> (Vec<Row>, usize) {
    // name id -> match score
    let mut cands: FxHashMap<NameId, i64> = FxHashMap::default();
    let mut owner: Option<String> = None;
    let mut q = query.trim();

    if opts.regex || q.contains(['*', '?']) {
        let pat = if opts.regex { q.to_string() } else { glob_to_regex(q) };
        if let Ok(re) = regex::bytes::Regex::new(&pat) {
            let hits = ix.scan_lexicon(|blob, offs, out| {
                for i in 0..offs.len() - 1 {
                    if re.is_match(&blob[offs[i] as usize..offs[i + 1] as usize]) {
                        out.push(i as u32);
                    }
                }
            });
            for n in hits {
                cands.insert(n, 800 - ix.name(n).len().min(100) as i64);
            }
        }
    } else {
        // Qualified form: Owner.name / Owner::name
        for sep in ["::", "->", "#", "."] {
            if let Some(i) = q.rfind(sep) {
                if i > 0 && i + sep.len() < q.len() && !q.contains(' ') {
                    owner = Some(q[..i].rsplit(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or(&q[..i]).to_string());
                    q = &q[i + sep.len()..];
                    break;
                }
            }
        }
        let terms: Vec<&str> = q.split_whitespace().collect();
        let terms: Vec<String> = if terms.len() == 1 { vec![terms[0].to_string()] } else { terms.iter().map(|s| s.to_string()).collect() };
        collect_terms(ix, &terms, &mut cands);
        if cands.is_empty() {
            // Retry with the query split into identifier words, any order.
            let words = split_words(q);
            if words.len() > 1 {
                collect_terms(ix, &words, &mut cands);
            }
        }
    }

    // Keep the best-matching names before touching definitions.
    let mut names: Vec<(i64, NameId)> = cands.into_iter().map(|(n, s)| (s, n)).collect();
    names.sort_unstable_by(|a, b| b.cmp(a));
    names.truncate(4000);

    let mut rows: Vec<Row> = Vec::new();
    for (nscore, nid) in names {
        let mut defs: Vec<DefId> = ix.defs_named_id(nid).collect();
        if defs.is_empty() {
            continue;
        }
        if let Some(o) = &owner {
            defs.retain(|&d| ix.def(d).parent_name().is_some_and(|p| p.eq_ignore_ascii_case(o.as_bytes())) || ix.def(d).path().contains(o.as_str()));
        }
        if let Some(kinds) = &common.kinds {
            defs.retain(|&d| kinds.contains(&ix.def(d).kind()));
        }
        if !pf.is_empty() {
            defs.retain(|&d| pf.matches(ix.def(d).path()));
        }
        if !common.tests {
            let non_test: Vec<DefId> = defs.iter().copied().filter(|&d| !ix.is_test_file(ix.def(d).file())).collect();
            if !non_test.is_empty() {
                defs = non_test;
            }
        }
        if defs.is_empty() {
            continue;
        }
        let pop = (ix.popularity(nid) as f64 + 1.0).log2() as i64 * 12;
        for ent in rs.entities(&defs) {
            let p = ent.primary(ix);
            let decls = ent.defs.iter().filter(|&&d| ix.def(d).is_decl()).count();
            let real = ent.defs.len() - decls;
            rows.push(Row {
                score: nscore * 4 + def_rank(ix, &p) / 4 + pop,
                primary: p.id,
                others: real.saturating_sub(1) + if real == 0 { decls.saturating_sub(1) } else { 0 },
                decls: if real == 0 { 0 } else { decls },
            });
        }
    }
    rows.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.primary.cmp(&b.primary)));
    let total = rows.len();
    (rows, total)
}

fn collect_terms(ix: &Index, terms: &[String], cands: &mut FxHashMap<NameId, i64>) {
    if terms.is_empty() {
        return;
    }
    let lowers: Vec<Vec<u8>> = terms.iter().map(|t| t.to_ascii_lowercase().into_bytes()).collect();
    // Scan for each term; the term with the fewest hits drives the candidate set.
    let mut hits: Vec<Vec<NameId>> = Vec::with_capacity(terms.len());
    for t in terms {
        hits.push(scan_names(ix, &case_variants(t)));
    }
    let base = (0..terms.len()).min_by_key(|&i| hits[i].len()).unwrap();
    'names: for &nid in &hits[base] {
        let name = ix.name(nid);
        let mut total = 0i64;
        for (i, t) in terms.iter().enumerate() {
            let s = term_score(name, t, &lowers[i]);
            if s == 0 {
                // Other terms may match the path instead; checked per definition below.
                if terms.len() == 1 {
                    continue 'names;
                }
                let in_path = ix.defs_named_id(nid).any(|d| contains_ci(ix.def(d).path().as_bytes(), &lowers[i]).is_some());
                if !in_path {
                    continue 'names;
                }
                total += 150;
            } else {
                total += s;
            }
        }
        cands.insert(nid, total / terms.len() as i64);
    }
}

fn glob_to_regex(g: &str) -> String {
    let mut re = String::from("(?i)^");
    for ch in g.chars() {
        match ch {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            c if c.is_alphanumeric() || c == '_' => re.push(c),
            c => {
                re.push('\\');
                re.push(c);
            }
        }
    }
    re.push('$');
    re
}

/// The best-matching definitions for a name-like query, best first, with the total match count.
pub fn top_matches(ix: &Index, rs: &Resolver, query: &str, common: &Common, limit: usize) -> (Vec<DefId>, usize) {
    let pf = PathFilter::new(&common.paths);
    let (rows, total) = search(ix, rs, query, common, &pf, &FindOpts { limit, regex: false });
    (rows.into_iter().take(limit).map(|r| r.primary).collect(), total)
}

pub fn find(ix: &Index, rs: &Resolver, queries: &[String], common: &Common, opts: &FindOpts, out: &mut Out) {
    let pf = PathFilter::new(&common.paths);
    for (qi, q) in queries.iter().enumerate() {
        if queries.len() > 1 {
            if qi > 0 {
                out.push("");
            }
            out.push(&format!("## {q}"));
        }
        let (rows, total) = search(ix, rs, q, common, &pf, opts);
        if rows.is_empty() {
            out.push(&format!("no symbol matches `{q}` (try: codefast grep '{q}')"));
            continue;
        }
        let mut shown = 0usize;
        for row in rows.iter().take(opts.limit) {
            let d = ix.def(row.primary);
            let mut line = def_line(&d);
            if row.decls > 0 || row.others > 0 {
                let mut extra = Vec::new();
                if row.others > 0 {
                    extra.push(format!("+{} more defs", row.others));
                }
                if row.decls > 0 {
                    extra.push(format!("+{} decl", row.decls));
                }
                line.push_str(&format!("  [{}]", extra.join(", ")));
            }
            if !out.line(&line) {
                break;
            }
            shown += 1;
        }
        if total > shown {
            out.push(&format!("… {} more matches (narrow with --kind / --path, or raise --limit)", count(total - shown)));
        }
    }
}
