//! `context`: everything needed before changing a function, in one answer. Its body, the type
//! it belongs to, the signatures of what it calls, the places that call it and the tests that
//! exercise it. This is the reading an agent otherwise does in five or six separate steps.

use super::refs::{Via, callee_edges, caller_edges, encl_name, loose_is_likely, proximity_key};
use super::show::{ShowOpts, line_starts, line_text, show_def};
use super::{Common, PathFilter, def_line, parse_spec, sig_of, targets_for};
use crate::facts::DefKind;
use crate::lang::Lang;
use crate::output::{Out, clip, count};
use crate::resolve::{Class, Resolver, Target};
use crate::store::{DefId, DefV, FileId, Index, RefV};
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;

/// Source lines of references, read once per file.
struct Lines<'a> {
    ix: &'a Index,
    cache: FxHashMap<FileId, Option<(Arc<Vec<u8>>, Vec<usize>)>>,
}

impl<'a> Lines<'a> {
    fn new(ix: &'a Index) -> Lines<'a> {
        Lines { ix, cache: FxHashMap::default() }
    }

    fn text(&mut self, file: FileId, line: u32) -> String {
        let ix = self.ix;
        let e = self.cache.entry(file).or_insert_with(|| {
            ix.source(file).map(|s| {
                let st = line_starts(&s);
                (s, st)
            })
        });
        match e {
            Some((src, starts)) => String::from_utf8_lossy(line_text(src, starts, line)).trim().to_string(),
            None => String::new(),
        }
    }
}

/// A test function that exercises the target, and how directly.
struct TestHit<'a> {
    test: DefV<'a>,
    line: u32,
    /// The function of the target's own code through which the test reaches it, if not direct.
    through: Option<String>,
}

fn is_test_fn(ix: &Index, d: &DefV) -> bool {
    ix.is_test_file(d.file()) && matches!(d.kind(), DefKind::Function | DefKind::Method)
}

/// The command that runs the given tests, where the language has one convention for it.
fn run_hint(ix: &Index, tests: &[TestHit]) -> Option<String> {
    let first = tests.first()?;
    let file = first.test.file();
    let path = ix.path(file);
    let mut names: Vec<String> = Vec::new();
    for t in tests.iter().filter(|t| t.test.file() == file).take(4) {
        let n = t.test.name_str().into_owned();
        if !names.contains(&n) {
            names.push(n);
        }
    }
    match ix.lang(file) {
        Lang::Go => {
            let dir = match path.rfind('/') {
                Some(i) => &path[..i],
                None => ".",
            };
            // Subtests and helpers are reached through the top-level test that holds them.
            let tops: Vec<&String> = names.iter().filter(|n| n.starts_with("Test") || n.starts_with("Benchmark") || n.starts_with("Fuzz")).collect();
            if tops.is_empty() {
                Some(format!("go test ./{dir}/"))
            } else {
                Some(format!("go test ./{dir}/ -run '^({})$'", tops.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("|")))
            }
        }
        Lang::Rust => Some(format!("cargo test {}", first.test.name_str())),
        _ => None,
    }
}

fn one(ix: &Index, rs: &Resolver, t: &Target, common: &Common, out: &mut Out) {
    let p = t.primary(ix);
    let mut lines = Lines::new(ix);

    // 1. The definition itself.
    show_def(ix, &p, &ShowOpts { max_lines: 140, full: false, outline: false, doc: true }, out);

    // 2. The type it belongs to: fields in full, the other methods by signature.
    if let Some(owner) = p.parent().filter(|o| o.kind().is_type() || o.kind() == DefKind::Impl) {
        let members: Vec<DefV> = ix.defs_of_file(owner.file()).filter(|m| m.parent().is_some_and(|x| x.id == owner.id) && m.id != p.id).collect();
        if !members.is_empty() {
            out.push(&format!("member of {}  {}:{}-{}", sig_of(&owner), owner.path(), owner.rec.line, owner.rec.end_line));
            let (fields, methods): (Vec<&DefV>, Vec<&DefV>) = members.iter().partition(|m| !matches!(m.kind(), DefKind::Method | DefKind::Function));
            // A large type is not repeated here: its fields, and a pointer to the rest.
            let (nf, nm) = if members.len() > 24 { (12, 0) } else { (16, 8) };
            let mut shown = 0usize;
            for m in fields.iter().take(nf).chain(methods.iter().take(nm)) {
                if !out.line(&format!("  {}\t{}", m.rec.line, sig_of(m))) {
                    break;
                }
                shown += 1;
            }
            if shown < members.len() {
                out.push(&format!("  … {} more members (codefast show {})", members.len() - shown, owner.name_str()));
            }
        }
    }

    if p.parent().is_none_or(|o| o.kind() == DefKind::Impl) {
        // Methods declared outside their type (Go, Rust): the type is shown as it is written.
        if let Some(ty) = rs.owner_ty(&p) {
            if let Some(&td) = ty.defs.iter().find(|&&d| ix.def(d).kind().is_type()) {
                let tv = ix.def(td);
                if out.fits(((tv.rec.end_line - tv.rec.line + 2) as usize).min(32) * 12) {
                    out.push("method of:");
                    show_def(ix, &tv, &ShowOpts { max_lines: 30, full: false, outline: false, doc: false }, out);
                }
            }
        }
    }

    // 3. What it calls, with signatures: enough to write a correct call.
    if t.class == Class::Callable {
        let cs = callee_edges(rs, &p);
        let mut external: Vec<String> = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        let mut printed = false;
        let mut docs = super::card::Docs::new(ix);
        for c in &cs {
            let name = String::from_utf8_lossy(c.name).into_owned();
            let target = c.targets.first().copied().or_else(|| c.slot.as_ref().and_then(|(_, b)| b.first().copied()));
            match target {
                Some(d) => {
                    let v = ix.def(d);
                    if v.kind() == DefKind::Macro {
                        continue;
                    }
                    if !printed {
                        out.push("calls:");
                        printed = true;
                    }
                    let more = if c.by_name {
                        " [by name]"
                    } else if c.member && c.targets.is_empty() {
                        " (one of several implementations)"
                    } else {
                        ""
                    };
                    out.line(&format!("  {} {}:{}  {}{}{}", c.line, v.path(), v.rec.line, clip(&v.signature(), 150), more, docs.suffix(&v, 80)));
                    if c.by_name {
                        // The other methods of that name: the receiver's type decides, and it
                        // is not written.
                        for &o in c.targets.iter().skip(1).take(2) {
                            let ov = ix.def(o);
                            out.line(&format!("     or {}:{}  {} [by name]", ov.path(), ov.rec.line, clip(&ov.signature(), 150)));
                        }
                    }
                }
                None if c.member && !c.external => {
                    if !unknown.contains(&name) {
                        unknown.push(name);
                    }
                }
                None => {
                    if !external.contains(&name) {
                        external.push(name);
                    }
                }
            }
        }
        external.retain(|n| !unknown.contains(n));
        if !unknown.is_empty() {
            out.line(&format!("  methods of values whose type is not written: {}", unknown.join(", ")));
        }
        if !external.is_empty() {
            out.line(&format!("  not defined in this repository: {}", external.join(", ")));
        }
    }

    // 4. Who calls it, and 5. which tests exercise it.
    let (edges, loose) = caller_edges(rs, t);
    let def_path = p.path().to_string();
    let mut sure: Vec<RefV> = edges.iter().filter(|e| matches!(e.via, Via::Call | Via::Slot(..) | Via::Arg(_) | Via::Value)).map(|e| e.r).collect();
    let mut named: Vec<RefV> = if loose_is_likely(rs, t, loose.len()) { loose } else { Vec::new() };
    sure.sort_by_cached_key(|r| proximity_key(ix, &def_path, r));
    named.sort_by_cached_key(|r| proximity_key(ix, &def_path, r));
    let in_test = |r: &RefV| ix.is_test_file(r.file);
    let code: Vec<RefV> = sure.iter().filter(|r| !in_test(r)).copied().collect();
    let code_named: Vec<RefV> = named.iter().filter(|r| !in_test(r)).copied().collect();
    if !code.is_empty() || !code_named.is_empty() {
        let total = code.len() + code_named.len();
        out.push(&format!("called from ({} sites):", count(total)));
        let mut shown = 0usize;
        for (r, by_name) in code.iter().map(|r| (r, false)).chain(code_named.iter().map(|r| (r, true))).take(8) {
            let note = if by_name { "  [by name]" } else { "" };
            if !out.line(&format!("  {}:{} {}: {}{}", r.path(), r.line(), encl_name(r), clip(&lines.text(r.file, r.line()), 140), note)) {
                break;
            }
            shown += 1;
        }
        if shown < total {
            out.push(&format!("  … {} more (codefast callers {})", count(total - shown), String::from_utf8_lossy(&t.name)));
        }
    } else if t.class == Class::Callable && !ix.is_test_file(p.file()) {
        out.push("called from: no call site found outside tests");
    }

    let mut tests: Vec<TestHit> = Vec::new();
    let mut seen: FxHashSet<DefId> = FxHashSet::default();
    for r in sure.iter().chain(named.iter()).filter(|r| in_test(r)) {
        if let Some(e) = r.encl().filter(|e| is_test_fn(ix, e)) {
            if seen.insert(e.id) {
                tests.push(TestHit { test: e, line: r.line(), through: None });
            }
        }
    }
    if tests.len() < 3 {
        // No test calls it by name: look one step out, at the tests of its callers.
        let mut callers: Vec<DefV> = Vec::new();
        for r in code.iter().chain(code_named.iter()) {
            if let Some(e) = r.encl().filter(|e| matches!(e.kind(), DefKind::Function | DefKind::Method)) {
                if e.id != p.id && !callers.iter().any(|c| c.id == e.id) {
                    callers.push(e);
                }
            }
        }
        for c in callers.iter().take(6) {
            let all = ix.defs_named(c.name());
            let Some(ct) = rs.entities(&all).into_iter().find(|x| x.defs.contains(&c.id)) else { continue };
            let (ce, cl) = caller_edges(rs, &ct);
            let cl = if loose_is_likely(rs, &ct, cl.len()) { cl } else { Vec::new() };
            for r in ce.iter().map(|e| e.r).chain(cl.into_iter()).filter(|r| in_test(r)) {
                if let Some(e) = r.encl().filter(|e| is_test_fn(ix, e)) {
                    if seen.insert(e.id) {
                        tests.push(TestHit { test: e, line: r.line(), through: Some(c.name_str().into_owned()) });
                    }
                }
            }
            if tests.len() >= 12 {
                break;
            }
        }
    }
    if !tests.is_empty() && (common.tests || !ix.is_test_file(p.file())) {
        // Tests named after the function, in files named after it, come first.
        let key = String::from_utf8_lossy(&t.name).to_lowercase();
        tests.sort_by_key(|h| {
            let in_name = h.test.name_str().to_lowercase().contains(&key);
            let in_file = h.test.path().to_lowercase().contains(&key);
            (h.through.is_some(), !in_file, !in_name)
        });
        out.push(&format!("tests that reach it ({}):", tests.len()));
        for h in tests.iter().take(8) {
            let via = h.through.as_ref().map(|c| format!("  (through {c})")).unwrap_or_default();
            if !out.line(&format!("  {}:{} {}{}", h.test.path(), h.line, h.test.qualified(), via)) {
                break;
            }
        }
        if tests.len() > 8 {
            out.push(&format!("  … {} more", tests.len() - 8));
        }
        if let Some(cmd) = run_hint(ix, &tests) {
            out.line(&format!("  run: {cmd}"));
        }
        // One short test, in full: it shows how the function is meant to be used.
        let short = tests.iter().filter(|h| h.through.is_none()).min_by_key(|h| h.test.rec.end_line - h.test.rec.line);
        if let Some(h) = short.filter(|h| h.test.rec.end_line - h.test.rec.line < 30) {
            let need = (h.test.rec.end_line - h.test.rec.line + 3) as usize * 14;
            if out.fits(need) {
                out.push("example test:");
                show_def(ix, &h.test, &ShowOpts { max_lines: 30, full: false, outline: false, doc: false }, out);
            }
        }
    }
}

pub fn context(ix: &Index, rs: &Resolver, specs: &[String], common: &Common, out: &mut Out) {
    let pf = PathFilter::new(&common.paths);
    for (i, s) in specs.iter().enumerate() {
        if i > 0 {
            out.push("");
        }
        let spec = parse_spec(ix, s);
        let ents = targets_for(ix, rs, &spec, common, &pf);
        let Some(t) = ents.first() else {
            out.push(&format!("no symbol matches `{s}` (try: codefast find {s})"));
            continue;
        };
        one(ix, rs, t, common, out);
        if ents.len() > 1 {
            let others: Vec<String> = ents.iter().skip(1).take(4).map(|e| def_line(&e.primary(ix))).collect();
            out.push(&format!("other symbols with this name: {}", others.join(" | ")));
        }
        if out.dropped > 0 {
            out.push(&format!("… {} more lines (raise --budget)", out.dropped));
        }
    }
}
