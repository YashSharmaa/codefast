//! `search`: the front door. One command for "I have a name", "I have some text" and "I have a
//! bug report": it routes the query, and every hit carries the structure around it (who calls
//! it, where it is bound, what else mentions it) so that a follow-up call is rarely needed.

use super::locate::{LocateOpts, locate};
use super::refs::{Via, callee_edges, caller_edges, encl_name, proximity_key};
use super::show::{ShowOpts, outline, show, show_def};
use super::{Common, PathFilter, Spec, def_line, parse_spec, targets_for};
use crate::facts::DefKind;
use crate::output::{Out, count};
use crate::resolve::{Class, Resolver, Target};
use crate::store::{FileId, Index};
use rustc_hash::FxHashSet;

pub struct SearchOpts {
    /// Longest body shown inline when exactly one symbol matches.
    pub body_lines: usize,
}

impl Default for SearchOpts {
    fn default() -> Self {
        SearchOpts { body_lines: 60 }
    }
}

/// One line of inverse structure: `called by: a, b, c (+12)` plus slot bindings.
pub fn usage_line(ix: &Index, rs: &Resolver, t: &Target, max_names: usize) -> Vec<String> {
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
    if !direct.is_empty() {
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
    if !by_name.is_empty() {
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
    if direct.is_empty() && binds.is_empty() && values == 0 && !has_cb && by_name.is_empty() && t.class == Class::Callable {
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
    for l in usage_line(ix, rs, t, 5) {
        out.line(&l);
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
    if !mentions.is_empty() {
        let mut ms: Vec<&str> = mentions.iter().map(|f| ix.path(*f)).collect();
        ms.sort();
        let shown: Vec<&str> = ms.iter().take(3).copied().collect();
        let more = if ms.len() > shown.len() { format!(" +{}", ms.len() - shown.len()) } else { String::new() };
        out.line(&format!("text-only mentions (comments, strings, unparsed files): {}{}", shown.join(", "), more));
    }
}

pub fn search(ix: &Index, rs: &Resolver, query: &str, common: &Common, opts: &SearchOpts, out: &mut Out) {
    let q = query.trim();
    if q.is_empty() {
        out.push("empty query");
        return;
    }
    // Free text: more than one word (unless it is just a kind word and a name: `struct file`).
    if q.split_whitespace().count() > 1 && super::split_kind_prefix(q).0.is_none() {
        locate(ix, q, common, &LocateOpts::default(), out);
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
