//! `refs`, `callers`, `callees`, `path`: who uses a symbol, what it uses, and how control gets
//! from one function to another, including dispatch through operation tables.

use super::show::{line_starts, line_text};
use super::{Common, PathFilter, Spec, def_line, parse_spec, targets_for};
use crate::facts::{DefKind, RefKind};
use crate::lang::Lang;
use crate::output::{Out, clip, count};
use crate::resolve::{Class, Conf, Resolver, Target, Ty, class_of, common_prefix_dirs};
use crate::store::{DefId, DefV, Index, RefV};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque;

pub struct RefsOpts {
    /// Print the source line of each reference: `Some(true/false)`, or `None` to decide by size.
    pub text: Option<bool>,
    /// Reference kinds to keep (`call`, `read`, `write`, `type`, `bind`); empty = all.
    pub only: Vec<String>,
    /// Also list matches that agree by name only (receiver type unknown).
    pub loose: bool,
}

impl Default for RefsOpts {
    fn default() -> Self {
        RefsOpts { text: None, only: Vec::new(), loose: false }
    }
}

fn category(k: RefKind) -> (&'static str, u8) {
    match k {
        RefKind::Call | RefKind::MacroCall | RefKind::New | RefKind::MemberCall => ("calls", 0),
        RefKind::Bind => ("bound as callback / slot value", 1),
        RefKind::Write | RefKind::MemberWrite => ("writes", 2),
        RefKind::Read | RefKind::MemberRead | RefKind::Arg => ("reads", 3),
        RefKind::Type => ("type uses", 4),
        RefKind::Extends | RefKind::Implements => ("subtypes", 5),
        RefKind::Import => ("imports", 6),
        RefKind::Annotation => ("annotations", 7),
    }
}

fn only_matches(only: &[String], k: RefKind) -> bool {
    if only.is_empty() {
        return true;
    }
    only.iter().any(|o| match o.as_str() {
        "call" | "calls" => k.is_call(),
        "read" | "reads" => matches!(k, RefKind::Read | RefKind::MemberRead | RefKind::Arg),
        "write" | "writes" => matches!(k, RefKind::Write | RefKind::MemberWrite),
        "type" | "types" => matches!(k, RefKind::Type | RefKind::Extends | RefKind::Implements),
        "bind" | "binds" => k == RefKind::Bind,
        "import" | "imports" => k == RefKind::Import,
        other => k.tag() == other,
    })
}

pub fn encl_name(r: &RefV) -> String {
    match r.encl() {
        Some(d) => d.name_str().into_owned(),
        None => "<file>".to_string(),
    }
}

/// Sort key that lists references near the definition first and tests last.
pub fn proximity_key(ix: &Index, def_path: &str, r: &RefV) -> (u8, i64, String, u32) {
    let test = ix.is_test_file(r.file) as u8;
    let near = -(common_prefix_dirs(def_path, r.path()) as i64);
    (test, near, r.path().to_string(), r.line())
}

/// Print references grouped by file. Compact form: one line per file,
/// `path: encl:line,line encl:line`; text form: one line per reference with its source text.
fn print_refs(ix: &Index, refs: &[RefV], text: bool, out: &mut Out) -> usize {
    let mut shown = 0usize;
    let mut i = 0usize;
    while i < refs.len() {
        let file = refs[i].file;
        let mut j = i;
        while j < refs.len() && refs[j].file == file {
            j += 1;
        }
        let group = &refs[i..j];
        if text {
            if !out.line(ix.path(file)) {
                break;
            }
            let src = ix.source(file);
            let starts = src.as_ref().map(|s| line_starts(s));
            let mut fit = true;
            let mut last_line = 0u32;
            for r in group {
                // Several references on one line (nested calls) are one line of evidence.
                if r.line() == last_line {
                    shown += 1;
                    continue;
                }
                last_line = r.line();
                let t = match (&src, &starts) {
                    (Some(s), Some(st)) => String::from_utf8_lossy(line_text(s, st, r.line())).trim().to_string(),
                    _ => String::new(),
                };
                if !out.line(&format!("  {} {}: {}", r.line(), encl_name(r), clip(&t, 160))) {
                    fit = false;
                    break;
                }
                shown += 1;
            }
            if !fit {
                break;
            }
        } else {
            // encl -> lines, in first-seen order
            let mut order: Vec<String> = Vec::new();
            let mut lines: FxHashMap<String, Vec<u32>> = FxHashMap::default();
            for r in group {
                let e = encl_name(r);
                let v = lines.entry(e.clone()).or_default();
                if v.is_empty() {
                    order.push(e);
                }
                if v.last() != Some(&r.line()) {
                    v.push(r.line());
                }
            }
            let mut line = format!("{}:", ix.path(file));
            for e in &order {
                let ls: Vec<String> = lines[e].iter().map(|l| l.to_string()).collect();
                line.push_str(&format!(" {}:{}", e, ls.join(",")));
            }
            if !out.line(&line) {
                break;
            }
            shown += group.len();
        }
        i = j;
    }
    shown
}

/// Reference counts per directory (two leading path components), most frequent first.
fn dir_histogram(refs: &[RefV], top_n: usize) -> String {
    let mut dirs: FxHashMap<String, usize> = FxHashMap::default();
    for r in refs {
        let p = r.path();
        let mut it = p.split('/');
        let key = match (it.next(), it.next(), it.next()) {
            (Some(a), Some(b), Some(_)) => format!("{a}/{b}"),
            (Some(a), Some(_), None) => a.to_string(),
            _ => ".".to_string(),
        };
        *dirs.entry(key).or_default() += 1;
    }
    let mut top: Vec<(String, usize)> = dirs.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let shown: Vec<String> = top.iter().take(top_n).map(|(d, n)| format!("{d} {}", count(*n))).collect();
    let more = if top.len() > top_n { format!(", +{} dirs", top.len() - top_n) } else { String::new() };
    format!("{}{}", shown.join(", "), more)
}

/// Print references under the budget. A set too large to fit gets a per-directory histogram
/// first, so the shape of the whole set is visible even though only the nearest part is listed.
fn list_refs(ix: &Index, refs: &[RefV], text: bool, out: &mut Out) {
    if refs.is_empty() {
        return;
    }
    let nfiles = refs.iter().map(|r| r.file).collect::<FxHashSet<_>>().len();
    let est = refs.len() * if text { 22 } else { 6 } + nfiles * 9;
    let large = est > out.remaining();
    if large {
        out.push(&format!("by dir: {}", dir_histogram(refs, 10)));
    }
    let shown = print_refs(ix, refs, text, out);
    if shown < refs.len() {
        let rest = &refs[shown..];
        let rest_files = rest.iter().map(|r| r.file).collect::<FxHashSet<_>>().len();
        let hist = if large { String::new() } else { format!(": {}", dir_histogram(rest, 8)) };
        out.push(&format!("… {} more in {} files{} (narrow with --path DIR)", count(rest.len()), count(rest_files), hist));
    }
}

fn ambiguous_members(ix: &Index, name: &str, ents: &[Target], out: &mut Out) {
    out.push(&format!("`{name}` is a member of {} types; qualify it as Owner.{name}:", ents.len()));
    for t in ents.iter().take(12) {
        let p = t.primary(ix);
        out.line(&format!("  {}", def_line(&p)));
    }
    if ents.len() > 12 {
        out.push(&format!("  … {} more", ents.len() - 12));
    }
}

pub fn refs(ix: &Index, rs: &Resolver, specs: &[String], common: &Common, opts: &RefsOpts, out: &mut Out) {
    let pf = PathFilter::new(&common.paths);
    // Path filters restrict where references are searched, not which definition is meant.
    let def_filter = PathFilter::new(&[]);
    for (i, s) in specs.iter().enumerate() {
        if i > 0 {
            out.push("");
        }
        let spec = parse_spec(ix, s);
        let ents = targets_for(ix, rs, &spec, common, &def_filter);
        let Some(t) = ents.first() else {
            out.push(&format!("no symbol matches `{s}` (try: codefast find {s})"));
            continue;
        };
        if t.class == Class::Member && ents.len() > 1 && matches!(&spec, Spec::Sym { owner: None, .. }) {
            ambiguous_members(ix, s, &ents, out);
            continue;
        }
        let p = t.primary(ix);
        let mut exact: Vec<RefV> = Vec::new();
        let mut loose: Vec<RefV> = Vec::new();
        rs.refs_to(t, |r, c| {
            if !pf.matches(r.path()) || !only_matches(&opts.only, r.kind()) {
                return;
            }
            if !common.tests && ix.is_test_file(r.file) && !ix.is_test_file(p.file()) {
                return;
            }
            if c == Conf::Name { loose.push(r) } else { exact.push(r) }
        });
        // Members without any typed match: name-only matches are all there is.
        if exact.is_empty() && !loose.is_empty() && t.class != Class::Member {
            exact = std::mem::take(&mut loose);
        }
        let nfiles = exact.iter().map(|r| r.file).collect::<FxHashSet<_>>().len();
        out.push(&format!("{}  (all {} refs, {} files)", def_line(&p), count(exact.len()), count(nfiles)));
        if ents.len() > 1 {
            let others: Vec<String> = ents
                .iter()
                .skip(1)
                .take(4)
                .map(|e| {
                    let v = e.primary(ix);
                    format!("{}:{}", v.path(), v.rec.line)
                })
                .collect();
            out.push(&format!(
                "(other symbols named {}: {}{})",
                String::from_utf8_lossy(&t.name),
                others.join(", "),
                if ents.len() > 5 { format!(" +{}", ents.len() - 5) } else { String::new() }
            ));
        }
        let text = opts.text.unwrap_or(exact.len() <= 12);
        let def_path = p.path().to_string();
        // Group by category, each sorted by proximity.
        let mut cats: Vec<(u8, &'static str, Vec<RefV>)> = Vec::new();
        for r in exact {
            let (label, ord) = category(r.kind());
            match cats.iter_mut().find(|c| c.0 == ord) {
                Some(c) => c.2.push(r),
                None => cats.push((ord, label, vec![r])),
            }
        }
        cats.sort_by_key(|c| c.0);
        for (_, label, mut rs_) in cats {
            rs_.sort_by_cached_key(|r| proximity_key(ix, &def_path, r));
            out.push(&format!("{label} ({}):", count(rs_.len())));
            list_refs(ix, &rs_, text, out);
        }
        let mentions = super::search::text_only_mentions(ix, rs, t);
        if !mentions.is_empty() && opts.only.is_empty() {
            let mut ms: Vec<&str> = mentions.iter().map(|f| ix.path(*f)).filter(|p| pf.matches(p)).collect();
            ms.sort();
            if !ms.is_empty() {
                let shown: Vec<&str> = ms.iter().take(6).copied().collect();
                let more = if ms.len() > shown.len() { format!(" +{} files", ms.len() - shown.len()) } else { String::new() };
                out.push(&format!(
                    "text-only mentions, not references (comments, strings, unparsed files): {}{} (codefast grep -w {})",
                    shown.join(", "),
                    more,
                    String::from_utf8_lossy(&t.name)
                ));
            }
        }
        if !loose.is_empty() {
            if opts.loose {
                loose.sort_by_cached_key(|r| proximity_key(ix, &def_path, r));
                out.push(&format!("name-only matches, receiver type unknown ({}):", count(loose.len())));
                list_refs(ix, &loose, false, out);
            } else {
                out.push(&format!("(+{} name-only matches with unknown receiver type: --loose to list)", count(loose.len())));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Call graph edges
// ---------------------------------------------------------------------------------------------

#[derive(Clone)]
pub enum Via {
    /// Direct call.
    Call,
    /// Used as a value (passed as a callback, stored in a variable).
    Value,
    /// Bound into a slot (`.read = fn`); the string is the slot as written.
    Bind(Vec<u8>),
    /// Call dispatched through a slot the target is bound into.
    Slot(Vec<u8>, Vec<u8>),
    /// Passed as an argument to the named function (a callback registration).
    Arg(Vec<u8>),
}

#[derive(Clone)]
pub struct Edge<'a> {
    pub r: RefV<'a>,
    pub via: Via,
}

/// Incoming edges of an entity: direct calls, value uses, slot bindings and, for each slot it is
/// bound into, the call sites dispatching through that slot.
pub fn caller_edges<'a>(rs: &Resolver<'a>, t: &Target) -> (Vec<Edge<'a>>, usize) {
    let mut edges: Vec<Edge<'a>> = Vec::new();
    let mut slots: Vec<(Ty, Vec<u8>)> = Vec::new();
    let mut name_only = 0usize;
    let mut aliases: Vec<DefId> = Vec::new();
    rs.refs_to(t, |r, conf| match r.kind() {
        // In import-scoped code a match by name alone is not listed as a caller: it is counted.
        _ if conf == Conf::Name && crate::resolve::scoped_language(rs.ix.lang(r.file)) => name_only += 1,
        // A decorator or attribute applied to a definition runs the decorator.
        RefKind::Call | RefKind::MacroCall | RefKind::New | RefKind::MemberCall | RefKind::Annotation => edges.push(Edge { r, via: Via::Call }),
        RefKind::Read | RefKind::Write if t.class == Class::Callable => edges.push(Edge { r, via: Via::Value }),
        // A method read without being called: `run(obj.method)`.
        RefKind::MemberRead if t.class == Class::Callable && conf != Conf::Name => edges.push(Edge { r, via: Via::Value }),
        RefKind::Arg if t.class == Class::Callable => edges.push(Edge { r, via: Via::Arg(r.qual().unwrap_or(b"").to_vec()) }),
        RefKind::Bind => {
            let q = r.qual().unwrap_or(b"").to_vec();
            let (owners, field) = rs.slot_from(&q, Some(r.file));
            if owners.is_empty() {
                // `const alias = f`: calls of the alias are calls of `f`.
                if let Some(var) = r.encl().filter(|e| e.name() == field.as_slice() && rs.alias_target(e.id).is_some_and(|a| t.defs.contains(&a))) {
                    if !aliases.contains(&var.id) {
                        aliases.push(var.id);
                    }
                }
            }
            for o in owners {
                if !slots.iter().any(|(a, b)| *a == o && *b == field) {
                    slots.push((o, field.clone()));
                }
            }
            edges.push(Edge { r, via: Via::Bind(q) });
        }
        _ => {}
    });
    // A line that stores or passes the function is listed once, as the binding or the argument.
    let stored: FxHashSet<(crate::store::FileId, u32)> =
        edges.iter().filter(|e| matches!(e.via, Via::Bind(_) | Via::Arg(_))).map(|e| (e.r.file, e.r.line())).collect();
    edges.retain(|e| !(matches!(e.via, Via::Value) && e.r.kind() == RefKind::MemberRead && stored.contains(&(e.r.file, e.r.line()))));
    for a in aliases {
        let at = entity_of(rs, &rs.ix.def(a));
        rs.refs_to(&at, |r, conf| {
            if conf != Conf::Name && matches!(r.kind(), RefKind::Call | RefKind::MemberCall) {
                edges.push(Edge { r, via: Via::Slot(Vec::new(), at.name.clone()) });
            }
        });
    }
    for (owner, field) in slots {
        for (r, conf) in rs.slot_calls(&owner, &field) {
            if conf == Conf::Exact {
                edges.push(Edge { r, via: Via::Slot(owner.name.clone(), field.clone()) });
            } else {
                name_only += 1;
            }
        }
    }
    (edges, name_only)
}

pub struct Callee<'a> {
    pub name: &'a [u8],
    pub line: u32,
    pub count: u32,
    /// Resolved definitions, best first (empty: not defined in this repository).
    pub targets: Vec<DefId>,
    /// For member calls: slot owner types and how many functions are bound there.
    pub slot: Option<(Vec<u8>, Vec<DefId>)>,
    pub member: bool,
    /// Set when the function is not called here but handed to this callee as a callback
    /// (empty: used as a value, the receiving code is not known).
    pub passed_to: Option<&'a [u8]>,
    /// A member call whose receiver is a type of another package.
    pub external: bool,
}

fn within(d: Option<DefV>, target: DefId) -> bool {
    let mut cur = d;
    for _ in 0..12 {
        match cur {
            Some(v) if v.id == target => return true,
            Some(v) => cur = v.parent(),
            None => return false,
        }
    }
    false
}

/// Outgoing calls of a definition, in source order, de-duplicated.
pub fn callee_edges<'a>(rs: &Resolver<'a>, d: &DefV<'a>) -> Vec<Callee<'a>> {
    let ix = rs.ix;
    let file = d.file();
    // An object-like macro that names a function is an alias: its body "calls" that function.
    let alias_macro = d.kind() == DefKind::Macro && d.rec.arity_max == 0;
    let mut raw: Vec<RefV<'a>> = ix
        .refs_of_file(file)
        .filter(|r| {
            (r.kind().is_call() || r.kind() == RefKind::Arg || (alias_macro && r.kind() == RefKind::Read))
                && r.line() >= d.rec.line
                && r.line() <= d.rec.end_line
                && within(r.encl(), d.id)
        })
        .collect();
    raw.sort_by_key(|r| r.line());
    let mut out: Vec<Callee<'a>> = Vec::new();
    let mut seen: FxHashMap<(&[u8], Option<&[u8]>), usize> = FxHashMap::default();
    for r in raw {
        let key = (r.name(), r.qual());
        if let Some(&i) = seen.get(&key) {
            out[i].count += 1;
            continue;
        }
        if r.kind() == RefKind::Arg {
            // Only functions handed over as callbacks are edges; constants and variables are not.
            let targets: Vec<DefId> =
                rs.bound_targets(&r).iter().copied().filter(|&t| matches!(ix.def(t).kind(), DefKind::Function | DefKind::Method)).collect();
            if targets.is_empty() {
                continue;
            }
            seen.insert(key, out.len());
            out.push(Callee { name: r.name(), line: r.line(), count: 1, targets, slot: None, member: false, passed_to: r.qual(), external: false });
            continue;
        }
        // In languages without constructors a literal of a type (`T{...}`) runs no code.
        if r.kind() == RefKind::New && matches!(ix.lang(file), Lang::Go | Lang::Rust | Lang::C | Lang::Zig) {
            continue;
        }
        seen.insert(key, out.len());
        if r.kind() == RefKind::MemberCall {
            // The receiver's static type picks one method; overrides in subtypes, interface
            // implementations and functions bound into a slot are dynamic candidates.
            let mut slot = None;
            let mut external = false;
            let mut targets: Vec<DefId> = Vec::new();
            let owners = rs.owners_for_ref(&r);
            if let Some(owner) = owners.first() {
                let mut bound: Vec<DefId> = Vec::new();
                for m in rs.methods_of(owner, r.name()) {
                    let v = ix.def(m);
                    let abstract_decl = v.flags() & crate::facts::dflag::ABSTRACT != 0
                        || (v.is_decl() && v.parent().is_some_and(|p| matches!(p.kind(), DefKind::Interface | DefKind::Trait)));
                    if abstract_decl {
                        if !bound.contains(&m) {
                            bound.push(m);
                        }
                    } else if !targets.contains(&m) {
                        targets.push(m);
                    }
                }
                if !targets.is_empty() || !bound.is_empty() {
                    for sub in rs.descendants(owner).iter().take(40) {
                        for m in rs.methods_of(sub, r.name()) {
                            if !bound.contains(&m) && !targets.contains(&m) && bound.len() < 24 {
                                bound.push(m);
                            }
                        }
                    }
                }
                for (b, conf) in rs.slot_bindings(owner, r.name()) {
                    if conf != Conf::Exact {
                        continue;
                    }
                    if let Some(best) = rs.bound_targets(&b).first() {
                        if !bound.contains(best) {
                            bound.push(*best);
                        }
                    }
                }
                // Overloads share a name: the one with a body is the implementation.
                targets.sort_by_key(|&m| ix.def(m).is_decl());
                if targets.is_empty() {
                    // A field initialised with a function stands for it (`augment = this.extend`).
                    let mut fields = rs.members_of(owner, r.name());
                    if fields.is_empty() {
                        for a in rs.ancestors(owner) {
                            fields = rs.members_of(&a, r.name());
                            if !fields.is_empty() {
                                break;
                            }
                        }
                    }
                    if let Some(a) = fields.iter().find_map(|&f| rs.alias_target(f)) {
                        bound.retain(|&b| b != a);
                        targets.push(a);
                    }
                }
                slot = Some((owner.name.clone(), bound));
            }
            if targets.is_empty() && slot.as_ref().is_none_or(|(_, b)| b.is_empty()) {
                if let Some(list) = rs.namespace_member(&r) {
                    // `ns.f()` through a namespace binding: a plain call of a module-level function.
                    let fns: Vec<DefId> = list
                        .into_iter()
                        .filter(|&t| matches!(ix.def(t).kind(), DefKind::Function | DefKind::Method) || ix.def(t).flags() & crate::facts::dflag::FNPTR != 0)
                        .collect();
                    if !fns.is_empty() || slot.is_none() {
                        targets = fns;
                        slot = None;
                    }
                } else if slot.is_none() {
                    external = r.qual().is_some_and(|q| rs.path_external(q, file));
                }
            }
            out.push(Callee { name: r.name(), line: r.line(), count: 1, targets, slot, member: true, passed_to: None, external });
        } else {
            // `new T()` names a type; a plain call may also be a class being instantiated.
            let lang = ix.lang(file);
            let classes_callable = matches!(lang, Lang::Python | Lang::Kotlin | Lang::Swift | Lang::Scala | Lang::Ruby);
            let mut targets = if r.kind() == RefKind::New { rs.resolve_type(r.name(), r.qual(), file) } else { rs.resolve_ref_at(&r) };
            if targets.is_empty() {
                if r.kind() == RefKind::New {
                    targets = rs.resolve_ref_at(&r);
                } else if classes_callable {
                    targets = rs.resolve_type(r.name(), r.qual(), file);
                }
            }
            // A variable or a static field initialised with a function stands for it.
            if let Some(a) = targets.first().and_then(|&d| rs.alias_target(d)) {
                targets = vec![a];
            } else if targets.is_empty() && r.kind() == RefKind::Call {
                if let Some(a) = rs.static_alias(&r) {
                    targets = vec![a];
                } else if lang == Lang::Go && !rs.resolve_type(r.name(), r.qual(), file).is_empty() {
                    // `T(x)` with a type of this repository is a conversion: it runs no code.
                    seen.remove(&key);
                    continue;
                }
            }
            out.push(Callee { name: r.name(), line: r.line(), count: 1, targets, slot: None, member: false, passed_to: None, external: false });
        }
    }
    out
}

/// Functions and methods a definition hands on as values without calling them: callbacks given
/// to other code, handlers stored for later. Import-scoped languages only (in C the extractor
/// records callback arguments itself).
pub fn value_edges<'a>(rs: &Resolver<'a>, d: &DefV<'a>) -> Vec<Callee<'a>> {
    let ix = rs.ix;
    let file = d.file();
    let mut out: Vec<Callee<'a>> = Vec::new();
    if !crate::resolve::scoped_language(ix.lang(file)) {
        return out;
    }
    let mut raw: Vec<RefV<'a>> = ix
        .refs_of_file(file)
        .filter(|r| matches!(r.kind(), RefKind::Read | RefKind::MemberRead) && r.line() >= d.rec.line && r.line() <= d.rec.end_line && within(r.encl(), d.id))
        .collect();
    raw.sort_by_key(|r| r.line());
    let mut seen: FxHashSet<(&[u8], Option<&[u8]>)> = FxHashSet::default();
    for r in raw {
        if !seen.insert((r.name(), r.qual())) {
            continue;
        }
        let targets: Vec<DefId> = if r.kind() == RefKind::MemberRead {
            rs.method_value(&r)
        } else {
            rs.denotes(&r).into_iter().filter(|&t| matches!(ix.def(t).kind(), DefKind::Function | DefKind::Method)).collect()
        };
        if targets.is_empty() || targets.contains(&d.id) {
            continue;
        }
        out.push(Callee { name: r.name(), line: r.line(), count: 1, targets, slot: None, member: false, passed_to: Some(b""), external: false });
    }
    out
}

fn entity_of<'a>(rs: &Resolver<'a>, d: &DefV<'a>) -> Target {
    let ix = rs.ix;
    let all = ix.defs_named(d.name());
    let ents = rs.entities(&all);
    for t in ents {
        if t.defs.contains(&d.id) {
            return t;
        }
    }
    Target { name: d.name().to_vec(), defs: vec![d.id], class: class_of(d), local_file: None, owner: None, scope: Vec::new() }
}

// ---------------------------------------------------------------------------------------------
// callers
// ---------------------------------------------------------------------------------------------

pub fn callers(ix: &Index, rs: &Resolver, specs: &[String], common: &Common, depth: u32, out: &mut Out) {
    let pf = PathFilter::new(&common.paths);
    let def_filter = PathFilter::new(&[]);
    for (i, s) in specs.iter().enumerate() {
        if i > 0 {
            out.push("");
        }
        let spec = parse_spec(ix, s);
        let ents = targets_for(ix, rs, &spec, common, &def_filter);
        let Some(t) = ents.first() else {
            out.push(&format!("no symbol matches `{s}` (try: codefast find {s})"));
            continue;
        };
        if t.class == Class::Member && ents.len() > 1 && matches!(&spec, Spec::Sym { owner: None, .. }) {
            ambiguous_members(ix, s, &ents, out);
            continue;
        }
        let p = t.primary(ix);
        if depth > 1 {
            callers_tree(ix, rs, t, common, &pf, depth, out);
            continue;
        }
        // Members that hold callables are slots: their "callers" are the dispatch sites.
        if t.class == Class::Member {
            if let Some(owner) = &rs.target_owner(t) {
                let calls: Vec<RefV> =
                    rs.slot_calls(owner, &t.name).into_iter().filter(|(_, c)| *c == Conf::Exact).map(|(r, _)| r).filter(|r| pf.matches(r.path())).collect();
                let binds = rs.slot_bindings(owner, &t.name);
                out.push(&format!("{}  (slot: {} call sites, {} bound functions)", def_line(&p), count(calls.len()), count(binds.len())));
                if !calls.is_empty() {
                    out.push("call sites:");
                    let mut cs = calls;
                    cs.sort_by_cached_key(|r| proximity_key(ix, p.path(), r));
                    let text = cs.len() <= 12;
                    list_refs(ix, &cs, text, out);
                }
                if !binds.is_empty() {
                    out.push("bound functions:");
                    print_bindings(ix, rs, &binds, out);
                }
                continue;
            }
        }
        let (edges, name_only) = caller_edges(rs, t);
        let keep = |e: &&Edge| pf.matches(e.r.path()) && (common.tests || !ix.is_test_file(e.r.file) || ix.is_test_file(p.file()));
        let mut direct: Vec<RefV> = edges.iter().filter(keep).filter(|e| matches!(e.via, Via::Call)).map(|e| e.r).collect();
        let mut values: Vec<RefV> = edges.iter().filter(keep).filter(|e| matches!(e.via, Via::Value)).map(|e| e.r).collect();
        let mut args: Vec<&Edge> = edges.iter().filter(keep).filter(|e| matches!(e.via, Via::Arg(_))).collect();
        let binds: Vec<&Edge> = edges.iter().filter(keep).filter(|e| matches!(e.via, Via::Bind(_))).collect();
        let mut slot_calls: Vec<&Edge> = edges.iter().filter(keep).filter(|e| matches!(e.via, Via::Slot(..))).collect();
        let nfiles = direct.iter().map(|r| r.file).collect::<FxHashSet<_>>().len();
        let mut head = format!("{}  (all {} call sites, {} files", def_line(&p), count(direct.len()), count(nfiles));
        if !binds.is_empty() {
            head.push_str(&format!(", {} registrations / slot bindings", binds.len()));
        }
        if !args.is_empty() {
            head.push_str(&format!(", passed as a callback at {} sites", args.len()));
        }
        if !values.is_empty() {
            head.push_str(&format!(", {} uses as a value", values.len()));
        }
        head.push(')');
        out.push(&head);
        let def_path = p.path().to_string();
        if !direct.is_empty() {
            direct.sort_by_cached_key(|r| proximity_key(ix, &def_path, r));
            let text = direct.len() <= 10;
            list_refs(ix, &direct, text, out);
        }
        if !args.is_empty() {
            args.sort_by_cached_key(|e| proximity_key(ix, &def_path, &e.r));
            out.push(&format!("passed as a callback ({}):", args.len()));
            let mut shown = 0;
            for e in args.iter() {
                let Via::Arg(g) = &e.via else { continue };
                if !out.line(&format!("  to {}() at {}:{} in {}", String::from_utf8_lossy(g), e.r.path(), e.r.line(), encl_name(&e.r))) {
                    break;
                }
                shown += 1;
            }
            if shown < args.len() {
                out.push(&format!("  … {} more (narrow with --path DIR)", args.len() - shown));
            }
        }
        if !values.is_empty() {
            values.sort_by_cached_key(|r| proximity_key(ix, &def_path, r));
            out.push(&format!("used as a value ({}):", values.len()));
            let text = values.len() <= 8;
            list_refs(ix, &values, text, out);
        }
        if !binds.is_empty() {
            out.push(&format!("registered / bound ({}):", binds.len()));
            for e in binds.iter().take(12) {
                let Via::Bind(q) = &e.via else { continue };
                if let Some(reg) = q.strip_prefix(b"@") {
                    out.line(&format!("  registered with {}() at {}:{}", String::from_utf8_lossy(reg), e.r.path(), e.r.line()));
                    continue;
                }
                let (owners, field) = rs.slot(q);
                let slot = match owners.first() {
                    Some(o) => format!("{}.{}", String::from_utf8_lossy(o).trim_start_matches('%'), String::from_utf8_lossy(&field)),
                    None => String::from_utf8_lossy(q).into_owned(),
                };
                out.line(&format!("  {} in {} {}:{}", slot, encl_name(&e.r), e.r.path(), e.r.line()));
            }
            if binds.len() > 12 {
                out.push(&format!("  … {} more bindings", binds.len() - 12));
            }
        }
        if !slot_calls.is_empty() {
            slot_calls.sort_by_cached_key(|e| proximity_key(ix, &def_path, &e.r));
            // Group dispatch sites per slot.
            let mut by_slot: Vec<(String, Vec<RefV>)> = Vec::new();
            for e in &slot_calls {
                let Via::Slot(o, f) = &e.via else { continue };
                let key = if o.is_empty() {
                    format!("alias {}", String::from_utf8_lossy(f))
                } else {
                    format!("{}.{}", String::from_utf8_lossy(o).trim_start_matches('%'), String::from_utf8_lossy(f))
                };
                match by_slot.iter_mut().find(|(k, _)| *k == key) {
                    Some((_, v)) => v.push(e.r),
                    None => by_slot.push((key, vec![e.r])),
                }
            }
            for (slot, rs_) in by_slot {
                out.push(&format!("dispatched via {slot} ({} call sites):", rs_.len()));
                list_refs(ix, &rs_, rs_.len() <= 8, out);
            }
        }
        if name_only > 0 {
            out.push(&format!(
                "(+{} more possible call sites matched by name only, receiver type unknown: `codefast refs {} --loose` lists them)",
                count(name_only),
                s
            ));
        }
        if direct.is_empty() && values.is_empty() && binds.is_empty() && args.is_empty() && slot_calls.is_empty() {
            out.push("no callers found in indexed code");
        }
    }
}

fn print_bindings(ix: &Index, rs: &Resolver, binds: &[(RefV, Conf)], out: &mut Out) {
    let mut exact: Vec<&RefV> = binds.iter().filter(|(_, c)| *c == Conf::Exact).map(|(r, _)| r).collect();
    exact.sort_by(|a, b| a.path().cmp(b.path()).then(a.line().cmp(&b.line())));
    // One line per bound function, however many tables it is bound into.
    let mut lines: Vec<String> = Vec::new();
    let mut seen: FxHashSet<String> = FxHashSet::default();
    for r in &exact {
        let target = rs.bound_targets(r);
        let loc = match target.first() {
            Some(&d) => {
                let v = ix.def(d);
                format!("{}:{}", v.path(), v.rec.line)
            }
            None => format!("{}:{}", r.path(), r.line()),
        };
        let line = format!("  {} {}", String::from_utf8_lossy(r.name()), loc);
        if seen.insert(line.clone()) {
            lines.push(line);
        }
    }
    let mut shown = 0;
    for l in &lines {
        if !out.line(l) {
            break;
        }
        shown += 1;
    }
    if shown < lines.len() {
        out.push(&format!("  … {} more (narrow with --path DIR)", count(lines.len() - shown)));
    }
    let loose = binds.len() - exact.len();
    if loose > 0 {
        out.push(&format!("  (+{} bindings into same-named members of unknown type)", count(loose)));
    }
}

fn callers_tree(ix: &Index, rs: &Resolver, root: &Target, common: &Common, pf: &PathFilter, depth: u32, out: &mut Out) {
    let p = root.primary(ix);
    out.push(&format!("callers of {} ({}:{}), depth {}", p.name_str(), p.path(), p.rec.line, depth));
    // Breadth-first so every level gets its share of the budget; printed as an indented tree.
    struct Node {
        label: String,
        depth: u32,
        children: Vec<usize>,
        target: Option<Target>,
    }
    let mut nodes: Vec<Node> = vec![Node { label: String::new(), depth: 0, children: Vec::new(), target: Some(root.clone()) }];
    let mut seen: FxHashSet<DefId> = FxHashSet::default();
    for &d in &root.defs {
        seen.insert(d);
    }
    let mut queue: VecDeque<usize> = VecDeque::from([0]);
    let mut budget_nodes = if out.budget() == 0 { 4000 } else { out.budget() / 8 };
    let mut truncated = 0usize;
    while let Some(ni) = queue.pop_front() {
        let (t, d) = match (&nodes[ni].target, nodes[ni].depth) {
            (Some(t), d) => (t.clone(), d),
            _ => continue,
        };
        if d >= depth {
            continue;
        }
        let (edges, _) = caller_edges(rs, &t);
        // One child per calling definition.
        let mut per_caller: Vec<(DefId, u32, &'static str)> = Vec::new();
        for e in &edges {
            if !pf.matches(e.r.path()) || (!common.tests && ix.is_test_file(e.r.file)) {
                continue;
            }
            let tag = match e.via {
                Via::Call => "",
                Via::Value => " (as value)",
                Via::Bind(_) => " (binds)",
                Via::Slot(..) => " (via slot)",
                Via::Arg(_) => " (passes it as a callback)",
            };
            if let Some(c) = e.r.encl() {
                if !per_caller.iter().any(|(id, _, _)| *id == c.id) {
                    per_caller.push((c.id, e.r.line(), tag));
                }
            }
        }
        for (cid, line, tag) in per_caller {
            let c = ix.def(cid);
            if budget_nodes == 0 {
                truncated += 1;
                continue;
            }
            budget_nodes -= 1;
            let again = !seen.insert(cid);
            let label = format!("{} {}:{}{}{}", c.name_str(), c.path(), line, tag, if again { " ^" } else { "" });
            let expandable = !again && matches!(class_of(&c), Class::Callable);
            let target = if expandable && d + 1 < depth { Some(entity_of(rs, &c)) } else { None };
            let idx = nodes.len();
            nodes.push(Node { label, depth: d + 1, children: Vec::new(), target });
            nodes[ni].children.push(idx);
            if nodes[idx].target.is_some() {
                queue.push_back(idx);
            }
        }
    }
    // Depth-first print.
    let mut stack: Vec<usize> = nodes[0].children.iter().rev().copied().collect();
    let mut printed = 0usize;
    while let Some(i) = stack.pop() {
        let n = &nodes[i];
        let indent = "  ".repeat(n.depth as usize - 1);
        if out.line(&format!("{indent}{}", n.label)) {
            printed += 1;
        }
        for &c in n.children.iter().rev() {
            stack.push(c);
        }
    }
    let total = nodes.len() - 1;
    if total == 0 {
        out.push("no callers found in indexed code");
    }
    if printed < total || truncated > 0 {
        out.push(&format!("… {} more callers not shown (narrow with --path, lower --depth, or raise --budget)", count(total - printed + truncated)));
    }
    if nodes.iter().any(|n| n.label.ends_with('^')) {
        out.push("(^ = already expanded above)");
    }
}

// ---------------------------------------------------------------------------------------------
// callees
// ---------------------------------------------------------------------------------------------

pub fn callees(ix: &Index, rs: &Resolver, specs: &[String], common: &Common, sigs: bool, out: &mut Out) {
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
        let p = t.primary(ix);
        let mut cs = callee_edges(rs, &p);
        let ncalls = cs.len();
        // Functions handed on as values, unless the same function is also called here.
        let values: Vec<Callee> = value_edges(rs, &p).into_iter().filter(|v| !cs.iter().any(|c| c.targets.first() == v.targets.first())).collect();
        let nvalues = values.len();
        cs.extend(values);
        cs.sort_by_key(|c| c.line);
        let passed = if nvalues > 0 { format!(", {nvalues} functions passed on as values") } else { String::new() };
        out.push(&format!("{}  (all {} distinct callees{})", def_line(&p), ncalls, passed));
        let mut external: Vec<String> = Vec::new();
        let mut macros: Vec<String> = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        for c in &cs {
            let name = String::from_utf8_lossy(c.name);
            let times = if c.count > 1 { format!(" ×{}", c.count) } else { String::new() };
            if c.member && !c.targets.is_empty() {
                let v = ix.def(c.targets[0]);
                let more = c.slot.as_ref().map(|(_, b)| b.len()).filter(|&n| n > 0).map(|n| format!(" (+{n} overrides / bound)")).unwrap_or_default();
                let what = if sigs { format!(" {}", clip(&v.signature(), 140)) } else { String::new() };
                out.line(&format!("  {} {}{} {}:{}{}{}", c.line, v.qualified(), times, v.path(), v.rec.line, what, more));
                continue;
            }
            if c.member {
                match &c.slot {
                    Some((owner, bound)) => {
                        // The declaration in the interface itself is not an implementation.
                        let bound: Vec<DefId> = bound
                            .iter()
                            .copied()
                            .filter(|&d| {
                                let v = ix.def(d);
                                !(v.is_decl() && v.parent().is_some_and(|p| matches!(p.kind(), DefKind::Interface | DefKind::Trait)))
                            })
                            .collect();
                        // Methods that implement the slot share its name: say whose they are.
                        let mut names: Vec<String> = Vec::new();
                        for &d in &bound {
                            let v = ix.def(d);
                            let n = if v.name() == c.name { v.qualified() } else { v.name_str().into_owned() };
                            if !names.contains(&n) {
                                names.push(n);
                            }
                        }
                        let more = if names.len() > 3 { format!(", +{}", names.len() - 3) } else { String::new() };
                        names.truncate(3);
                        let impls = if bound.is_empty() { String::new() } else { format!(" → {} impls: {}{}", bound.len(), names.join(", "), more) };
                        out.line(&format!("  {} ->{}(){} [{}.{}]{}", c.line, name, times, String::from_utf8_lossy(owner), name, impls));
                    }
                    // A method of another package's type: named with the other external calls.
                    None if c.external => external.push(name.into_owned()),
                    // Nothing to point at: named together on one line.
                    None => unknown.push(format!("{name}{times}")),
                }
                continue;
            }
            match c.targets.first() {
                Some(&d) => {
                    let v = ix.def(d);
                    // Macros are rarely what a reader is after: name them on one line.
                    if v.kind() == DefKind::Macro && !sigs {
                        macros.push(name.into_owned());
                        continue;
                    }
                    let alt = match c.passed_to {
                        Some(g) if g.is_empty() => " [passed on as a value]".to_string(),
                        Some(g) => format!(" [callback passed to {}()]", String::from_utf8_lossy(g)),
                        None if c.targets.len() > 1 => format!(" (+{} variants)", c.targets.len() - 1),
                        None => String::new(),
                    };
                    let what = if sigs { format!(" {}", clip(&v.signature(), 140)) } else { String::new() };
                    // Reached under another name (an import alias, a variable that stands for
                    // the function): show the real one, and how it is written here.
                    let shown = if v.name() == c.name || v.name() == b"default" { name.into_owned() } else { format!("{} (written {})", v.name_str(), name) };
                    out.line(&format!("  {} {}{} {}:{}{}{}", c.line, shown, times, v.path(), v.rec.line, what, alt));
                }
                None => external.push(name.into_owned()),
            }
        }
        if !macros.is_empty() {
            out.line(&format!("  macros: {}", macros.join(", ")));
        }
        if !external.is_empty() {
            out.line(&format!("  not defined in repo: {}", external.join(", ")));
        }
        if !unknown.is_empty() {
            out.line(&format!("  method calls on a receiver of unknown type: {}", unknown.join(", ")));
        }
        if out.dropped > 0 {
            out.push(&format!("… {} more lines (raise --budget)", out.dropped));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// path
// ---------------------------------------------------------------------------------------------

/// An entity in the call graph, identified by its smallest definition id.
type Key = DefId;

fn key_of(t: &Target) -> Key {
    t.defs.iter().copied().min().unwrap_or(DefId(u32::MAX))
}

#[derive(Clone)]
struct Step {
    /// `file:line` of the call site and how the hop is made.
    path: String,
    line: u32,
    from: String,
    to: String,
    note: String,
}

struct PathSearch<'r, 'a> {
    ix: &'a Index,
    rs: &'r Resolver<'a>,
    ents: FxHashMap<DefId, (Key, Target)>,
}

impl<'r, 'a> PathSearch<'r, 'a> {
    /// The entity a definition belongs to, memoised.
    fn entity(&mut self, d: DefId) -> (Key, Target) {
        if let Some(hit) = self.ents.get(&d) {
            return hit.clone();
        }
        let t = entity_of(self.rs, &self.ix.def(d));
        let k = key_of(&t);
        for &m in &t.defs {
            self.ents.insert(m, (k, t.clone()));
        }
        (k, t)
    }

    /// Entities called by `t`, with the call site of each.
    fn forward(&mut self, t: &Target) -> Vec<(Key, Target, Step)> {
        let ix = self.ix;
        let mut out: Vec<(Key, Target, Step)> = Vec::new();
        let mut seen: FxHashSet<Key> = FxHashSet::default();
        for &d in t.defs.iter().filter(|&&d| !ix.def(d).is_decl()).take(6) {
            let v = ix.def(d);
            if !matches!(class_of(&v), Class::Callable) {
                continue;
            }
            let mut cs = callee_edges(self.rs, &v);
            cs.extend(value_edges(self.rs, &v));
            for c in cs {
                let mut targets: Vec<(DefId, String)> = Vec::new();
                if c.member {
                    for &t in c.targets.iter().take(4) {
                        targets.push((t, String::new()));
                    }
                    if let Some((owner, bound)) = &c.slot {
                        let note = format!(" via {}.{}", String::from_utf8_lossy(owner).trim_start_matches('%'), String::from_utf8_lossy(c.name));
                        for &b in bound.iter().take(400) {
                            targets.push((b, note.clone()));
                        }
                    }
                } else if let Some(&best) = c.targets.first() {
                    let note = match c.passed_to {
                        Some(g) if g.is_empty() => " (passed on as a value)".to_string(),
                        Some(g) => format!(" (passed as a callback to {}())", String::from_utf8_lossy(g)),
                        None => String::new(),
                    };
                    targets.push((best, note));
                }
                for (td, note) in targets {
                    let (k, e) = self.entity(td);
                    if !matches!(e.class, Class::Callable) || !seen.insert(k) {
                        continue;
                    }
                    let step = Step {
                        path: v.path().to_string(),
                        line: c.line,
                        from: v.name_str().into_owned(),
                        to: String::from_utf8_lossy(&e.name).into_owned(),
                        note,
                    };
                    out.push((k, e, step));
                }
            }
        }
        out
    }

    /// Entities that call `t` (directly or by dispatching through a slot it is bound into).
    fn backward(&mut self, t: &Target) -> Vec<(Key, Target, Step)> {
        let mut out: Vec<(Key, Target, Step)> = Vec::new();
        let mut seen: FxHashSet<Key> = FxHashSet::default();
        let (edges, _) = caller_edges(self.rs, t);
        for e in edges {
            let note = match &e.via {
                Via::Call => String::new(),
                Via::Slot(o, f) if o.is_empty() => format!(" via alias {}", String::from_utf8_lossy(f)),
                Via::Slot(o, f) => format!(" via {}.{}", String::from_utf8_lossy(o).trim_start_matches('%'), String::from_utf8_lossy(f)),
                Via::Arg(g) => format!(" (passed as a callback to {}())", String::from_utf8_lossy(g)),
                // Import-scoped code hands a function on by naming it.
                Via::Value if crate::resolve::scoped_language(self.ix.lang(e.r.file)) => " (passed on as a value)".to_string(),
                // A binding or a value use is not a call.
                _ => continue,
            };
            let Some(c) = e.r.encl() else { continue };
            if !matches!(class_of(&c), Class::Callable) {
                continue;
            }
            let (k, ce) = self.entity(c.id);
            if !seen.insert(k) {
                continue;
            }
            let step = Step {
                path: e.r.path().to_string(),
                line: e.r.line(),
                from: c.name_str().into_owned(),
                to: String::from_utf8_lossy(&t.name).into_owned(),
                note,
            };
            out.push((k, ce, step));
        }
        out
    }
}

/// Shortest call chain from `from` to `to`. The search runs from both ends at once: forwards
/// over what each function calls, backwards over who calls it, always growing the side that is
/// cheaper to grow, until the two meet.
pub fn path(ix: &Index, rs: &Resolver, from: &str, to: &str, common: &Common, max_depth: u32, out: &mut Out) {
    let pf = PathFilter::new(&[]);
    let cands = |s: &str| -> Vec<Target> {
        targets_for(ix, rs, &parse_spec(ix, s), common, &pf).into_iter().filter(|t| matches!(t.class, Class::Callable)).take(8).collect()
    };
    let (ca, cb) = (cands(from), cands(to));
    if ca.is_empty() || cb.is_empty() {
        let missing = if ca.is_empty() { from } else { to };
        out.push(&format!("cannot resolve `{missing}` to a function (try: codefast find {missing})"));
        return;
    }
    // Several functions may share either name: take the pair that lives closest together.
    let mut best = (0usize, 0usize, i64::MIN);
    for (i, x) in ca.iter().enumerate() {
        for (j, y) in cb.iter().enumerate() {
            let near = common_prefix_dirs(x.primary(ix).path(), y.primary(ix).path()) as i64 * 10 - (i + j) as i64;
            if near > best.2 {
                best = (i, j, near);
            }
        }
    }
    let (a, b) = (ca[best.0].clone(), cb[best.1].clone());
    let mut ps = PathSearch { ix, rs, ents: FxHashMap::default() };
    let (ka, kb) = (key_of(&a), key_of(&b));
    // fwd[k] = (previous entity towards `from`, the step into k); bwd[k] = (next entity towards
    // `to`, the step out of k).
    let mut fwd: FxHashMap<Key, Option<(Key, Step)>> = FxHashMap::default();
    let mut bwd: FxHashMap<Key, Option<(Key, Step)>> = FxHashMap::default();
    fwd.insert(ka, None);
    bwd.insert(kb, None);
    let mut ff: Vec<Target> = vec![a.clone()];
    let mut bf: Vec<Target> = vec![b.clone()];
    let mut meet: Option<Key> = if ka == kb { Some(ka) } else { None };
    let mut expanded = 0usize;
    let started = std::time::Instant::now();
    let mut depth = 0u32;
    let budget_nodes = 60_000usize;
    while meet.is_none() && !ff.is_empty() && !bf.is_empty() && depth < max_depth && expanded < budget_nodes && started.elapsed().as_secs() < 8 {
        depth += 1;
        // Backward steps over a widely used function are expensive: weigh by how many files
        // mention the names on that frontier.
        let bcost: u64 = bf.iter().map(|t| ix.name_id(&t.name).map(|n| ix.popularity(n) as u64).unwrap_or(1) + 4).sum();
        let fcost: u64 = ff.len() as u64 * 40;
        if fcost <= bcost {
            let mut next: Vec<Target> = Vec::new();
            'f: for t in std::mem::take(&mut ff) {
                expanded += 1;
                let kt = key_of(&t);
                for (k, e, step) in ps.forward(&t) {
                    if fwd.contains_key(&k) {
                        continue;
                    }
                    fwd.insert(k, Some((kt, step)));
                    if bwd.contains_key(&k) {
                        meet = Some(k);
                        break 'f;
                    }
                    next.push(e);
                }
                if expanded >= budget_nodes {
                    break;
                }
            }
            ff = next;
        } else {
            let mut next: Vec<Target> = Vec::new();
            'b: for t in std::mem::take(&mut bf) {
                expanded += 1;
                let kt = key_of(&t);
                for (k, e, step) in ps.backward(&t) {
                    if bwd.contains_key(&k) {
                        continue;
                    }
                    bwd.insert(k, Some((kt, step)));
                    if fwd.contains_key(&k) {
                        meet = Some(k);
                        break 'b;
                    }
                    next.push(e);
                }
                if expanded >= budget_nodes {
                    break;
                }
            }
            bf = next;
        }
    }
    let pa = a.primary(ix);
    let pb = b.primary(ix);
    let Some(m) = meet else {
        out.push(&format!(
            "no call path from {} to {} found ({} functions explored, {} hops deep). Calls through untyped function pointers, callbacks passed as arguments and token-pasted macros are not followed; try `codefast callers {}` and `codefast callees {}` to bridge the gap.",
            pa.name_str(),
            pb.name_str(),
            count(expanded),
            depth,
            pb.name_str(),
            pa.name_str()
        ));
        return;
    };
    // Steps from `from` to the meeting point, then on to `to`.
    let mut steps: Vec<Step> = Vec::new();
    let mut k = m;
    while let Some(Some((pk, step))) = fwd.get(&k) {
        steps.push(step.clone());
        k = *pk;
    }
    steps.reverse();
    let mut k = m;
    while let Some(Some((nk, step))) = bwd.get(&k) {
        steps.push(step.clone());
        k = *nk;
    }
    let chain: Vec<String> = std::iter::once(pa.name_str().into_owned()).chain(steps.iter().map(|s| s.to.clone())).collect();
    out.push(&format!("{} ({} hops)", chain.join(" → "), steps.len()));
    for s in &steps {
        out.line(&format!("  {}:{} {} calls {}{}", s.path, s.line, s.from, s.to, s.note));
    }
}
