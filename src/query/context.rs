//! `context`: everything needed before changing a function, in one answer. Its body, the type
//! it belongs to, the types its signature names, the signatures of what it calls, the places
//! that call it and the tests that reach it, also through a caller or two. This is the reading
//! an agent otherwise does in five or six separate steps.

use super::refs::{Via, callee_edges, caller_edges, encl_name, loose_is_likely, proximity_key};
use super::show::{ShowOpts, line_starts, line_text, show_def};
use super::{Common, PathFilter, def_line, parse_spec, sig_of, targets_for};
use crate::facts::{DefKind, RefKind};
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

    let mut owner_shown: Option<DefId> = p.parent().filter(|o| o.kind().is_type()).map(|o| o.id);
    if p.parent().is_none_or(|o| o.kind() == DefKind::Impl) {
        // Methods declared outside their type (Go, Rust): the type is shown as it is written.
        if let Some(ty) = rs.owner_ty(&p) {
            if let Some(&td) = ty.defs.iter().find(|&&d| ix.def(d).kind().is_type()) {
                let tv = ix.def(td);
                if out.fits(((tv.rec.end_line - tv.rec.line + 2) as usize).min(32) * 12) {
                    out.push("method of:");
                    show_def(ix, &tv, &ShowOpts { max_lines: 30, full: false, outline: false, doc: false }, out);
                    owner_shown = Some(td);
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
    let uses = Uses::of(ix, rs, t);
    uses.print_callers(ix, t, &mut lines, 8, out);
    let tests = uses.tests(ix, rs, &p);
    if common.tests || !ix.is_test_file(p.file()) {
        print_tests(ix, t, &tests, 8, true, out);
    }

    // 6. The types its signature names, where the repository defines them: what a call has
    // to hand over and what comes back. Last, so that a large type takes the room that is
    // left and never the room of a call site or a test.
    let shown: Vec<DefId> = owner_shown.into_iter().collect();
    print_signature_types(ix, rs, &p, &shown, 3, 30, out);
}

/// The places that use a function, sorted by how near they are to it.
struct Uses<'a> {
    sure: Vec<RefV<'a>>,
    named: Vec<RefV<'a>>,
    code: Vec<RefV<'a>>,
    code_named: Vec<RefV<'a>>,
}

impl<'a> Uses<'a> {
    fn of(ix: &'a Index, rs: &Resolver<'a>, t: &Target) -> Uses<'a> {
        let p = t.primary(ix);
        let (edges, loose) = caller_edges(rs, t);
        let def_path = p.path().to_string();
        let mut sure: Vec<RefV> = edges.iter().filter(|e| matches!(e.via, Via::Call | Via::Slot(..) | Via::Arg(_) | Via::Value)).map(|e| e.r).collect();
        let mut named: Vec<RefV> = if loose_is_likely(rs, t, loose.len()) { loose } else { Vec::new() };
        sure.sort_by_cached_key(|r| proximity_key(ix, &def_path, r));
        named.sort_by_cached_key(|r| proximity_key(ix, &def_path, r));
        let code: Vec<RefV> = sure.iter().filter(|r| !ix.is_test_file(r.file)).copied().collect();
        let code_named: Vec<RefV> = named.iter().filter(|r| !ix.is_test_file(r.file)).copied().collect();
        Uses { sure, named, code, code_named }
    }

    /// `called from (N sites):` and the line of each, nearest first.
    fn print_callers(&self, ix: &Index, t: &Target, lines: &mut Lines, max: usize, out: &mut Out) {
        let p = t.primary(ix);
        if !self.code.is_empty() || !self.code_named.is_empty() {
            let total = self.code.len() + self.code_named.len();
            out.push(&format!("called from ({} site{}):", count(total), if total == 1 { "" } else { "s" }));
            let mut shown = 0usize;
            for (r, by_name) in self.code.iter().map(|r| (r, false)).chain(self.code_named.iter().map(|r| (r, true))).take(max) {
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
    }

    /// The tests that reach it: those that call it, and when they are few, those that call
    /// what calls it, one and then two calls out. A helper deep inside a package is tested
    /// through the function the package offers, and that test is where its behaviour is
    /// written down.
    fn tests(&self, ix: &'a Index, rs: &Resolver<'a>, p: &DefV<'a>) -> Vec<TestHit<'a>> {
        let in_test = |r: &RefV| ix.is_test_file(r.file);
        let mut tests: Vec<TestHit> = Vec::new();
        let mut seen: FxHashSet<DefId> = FxHashSet::default();
        for r in self.sure.iter().chain(self.named.iter()).filter(|r| in_test(r)) {
            if let Some(e) = r.encl().filter(|e| is_test_fn(ix, e)) {
                if seen.insert(e.id) {
                    tests.push(TestHit { test: e, line: r.line(), through: None });
                }
            }
        }
        // The functions of the code that call it, each with the path that led there.
        let mut ring: Vec<(DefV, String)> = Vec::new();
        let mut visited: FxHashSet<DefId> = FxHashSet::default();
        visited.insert(p.id);
        for r in self.code.iter().chain(self.code_named.iter()) {
            if let Some(e) = r.encl().filter(|e| matches!(e.kind(), DefKind::Function | DefKind::Method)) {
                if visited.insert(e.id) {
                    ring.push((e, e.name_str().into_owned()));
                }
            }
        }
        for _step in 0..2 {
            if tests.len() >= 3 || ring.is_empty() {
                break;
            }
            let mut next: Vec<(DefV, String)> = Vec::new();
            for (c, path) in ring.iter().take(6) {
                let all = ix.defs_named(c.name());
                let Some(ct) = rs.entities(&all).into_iter().find(|x| x.defs.contains(&c.id)) else { continue };
                let (ce, cl) = caller_edges(rs, &ct);
                let cl = if loose_is_likely(rs, &ct, cl.len()) { cl } else { Vec::new() };
                for r in ce.iter().map(|e| e.r).chain(cl.into_iter()) {
                    let Some(e) = r.encl() else { continue };
                    if in_test(&r) {
                        if is_test_fn(ix, &e) && seen.insert(e.id) {
                            tests.push(TestHit { test: e, line: r.line(), through: Some(path.clone()) });
                        }
                    } else if matches!(e.kind(), DefKind::Function | DefKind::Method) && visited.insert(e.id) && next.len() < 12 {
                        next.push((e, format!("{path} and {}", e.name_str())));
                    }
                }
                if tests.len() >= 12 {
                    break;
                }
            }
            ring = next;
        }
        // Tests named after the function, in files named after it, come first.
        let key = p.name_str().to_lowercase();
        tests.sort_by_key(|h| {
            let in_name = h.test.name_str().to_lowercase().contains(&key);
            let in_file = h.test.path().to_lowercase().contains(&key);
            (h.through.is_some(), !in_file, !in_name)
        });
        tests
    }
}

/// `tests that reach it (N):`, the command that runs them, and one short test in full.
fn print_tests(ix: &Index, _t: &Target, tests: &[TestHit], max: usize, example: bool, out: &mut Out) {
    if tests.is_empty() {
        return;
    }
    out.push(&format!("tests that reach it ({}):", tests.len()));
    for h in tests.iter().take(max) {
        let via = h.through.as_ref().map(|c| format!("  (through {c})")).unwrap_or_default();
        if !out.line(&format!("  {}:{} {}{}", h.test.path(), h.line, h.test.qualified(), via)) {
            break;
        }
    }
    if tests.len() > max {
        out.push(&format!("  … {} more", tests.len() - max));
    }
    if let Some(cmd) = run_hint(ix, tests) {
        out.line(&format!("  run: {cmd}"));
    }
    if !example {
        return;
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

/// The line on which the body of a definition opens: where its signature ends.
fn body_opens(src: &[u8], starts: &[usize], d: &DefV) -> Option<u32> {
    (d.rec.line..=d.rec.end_line.min(d.rec.line + 12)).find(|&l| {
        let t = String::from_utf8_lossy(line_text(src, starts, l));
        let t = t.trim_end();
        t.ends_with('{') || t.ends_with(':')
    })
}

/// The types a definition's signature names, each resolved to the one type of this repository
/// it means. A name that could be two types, or a type from somewhere else, is left out.
fn signature_types<'a>(ix: &'a Index, rs: &Resolver<'a>, p: &DefV<'a>) -> Vec<DefV<'a>> {
    let Some(src) = ix.source(p.file()) else { return Vec::new() };
    let starts = line_starts(&src);
    let last = body_opens(&src, &starts, p).unwrap_or(p.rec.line);
    let mut out: Vec<DefV> = Vec::new();
    for r in ix.refs_of_file(p.file()) {
        if r.kind() != RefKind::Type || r.line() < p.rec.line || r.line() > last {
            continue;
        }
        let defs: Vec<DefId> = rs.resolve_type(r.name(), r.qual(), r.file).into_iter().filter(|&d| ix.def(d).kind().is_type()).collect();
        let ents = rs.entities(&defs);
        let [one] = &ents[..] else { continue };
        let v = one.primary(ix);
        // A one-line alias (`typedef long loff_t;`) is a name for a number: nothing to show.
        if v.kind() == DefKind::TypeAlias && v.rec.end_line == v.rec.line {
            continue;
        }
        if v.id != p.id && !out.iter().any(|o| o.id == v.id) {
            out.push(v);
        }
    }
    out
}

/// `its signature names:` and those types, a short one whole and a long one as its members.
fn print_signature_types<'a>(ix: &'a Index, rs: &Resolver<'a>, p: &DefV<'a>, shown: &[DefId], max: usize, max_lines: usize, out: &mut Out) {
    if !matches!(p.kind(), DefKind::Function | DefKind::Method) {
        return;
    }
    let types: Vec<DefV> = signature_types(ix, rs, p).into_iter().filter(|v| !shown.contains(&v.id)).collect();
    let mut printed = false;
    for v in types.iter().take(max) {
        let n = (v.rec.end_line - v.rec.line + 2) as usize;
        if !out.fits(n.min(12) * 10) {
            break;
        }
        if !printed {
            out.push("its signature names:");
            printed = true;
        }
        show_def(ix, v, &ShowOpts { max_lines, full: false, outline: false, doc: false }, out);
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
