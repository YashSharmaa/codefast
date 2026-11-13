//! Bulk export of the call graph as JSON Lines, for scoring against compiler-built oracles.

use super::PathFilter;
use super::refs::callee_edges;
use crate::facts::DefKind;
use crate::resolve::Resolver;
use crate::store::{DefId, DefV, Index};
use rustc_hash::FxHashSet;
use serde_json::json;
use std::io::Write;

fn endpoint(d: &DefV) -> serde_json::Value {
    json!({"file": d.path(), "line": d.rec.line, "name": d.name_str()})
}

/// Functions a call to `target` reaches: the function itself, or, for a macro, whatever the
/// macro's body calls (a compiler sees those calls in the function that uses the macro).
fn expand<'a>(ix: &'a Index, rs: &Resolver<'a>, target: DefId, depth: u32, seen: &mut FxHashSet<DefId>, out: &mut Vec<DefId>) {
    if !seen.insert(target) {
        return;
    }
    let v = ix.def(target);
    match v.kind() {
        DefKind::Function | DefKind::Method => out.push(target),
        // Instantiation: the class stands for its constructor.
        k if k.is_type() => out.push(target),
        DefKind::Macro if depth < 4 => {
            for c in callee_edges(rs, &v) {
                if c.member || c.passed_to.is_some() {
                    continue;
                }
                if let Some(&t) = c.targets.first() {
                    expand(ix, rs, t, depth + 1, seen, out);
                }
            }
        }
        _ => {}
    }
}

/// One line per (caller, callee, site): direct calls resolved to their best definition, calls
/// through macros expanded, member calls resolved through the receiver's static type. With
/// `dynamic`, the candidates of dynamic dispatch (overrides, interface implementations,
/// functions bound into a slot) are added.
pub fn export_edges(ix: &Index, rs: &Resolver, pf: &PathFilter, dynamic: bool, w: &mut impl Write) -> std::io::Result<usize> {
    let mut n = 0usize;
    for f in ix.files() {
        if !pf.matches(ix.path(f)) {
            continue;
        }
        // Accessors (`get x() { .. }`) are properties with a body.
        let defs: Vec<DefV> = ix
            .defs_of_file(f)
            .filter(|d| {
                !d.is_decl() && (matches!(d.kind(), DefKind::Function | DefKind::Method) || (d.kind() == DefKind::Property && d.rec.end_line > d.rec.line))
            })
            .collect();
        for d in defs {
            let from = endpoint(&d);
            for c in callee_edges(rs, &d) {
                // Only resolved edges are exported: a match by name alone is a hint, not an edge.
                if c.passed_to.is_some() || c.by_name {
                    continue;
                }
                let mut targets: Vec<(DefId, &'static str)> = Vec::new();
                if c.member {
                    for &t in c.targets.iter().take(1) {
                        targets.push((t, "static"));
                    }
                    if dynamic {
                        if let Some((_, bound)) = &c.slot {
                            for &b in bound {
                                targets.push((b, "dynamic"));
                            }
                        }
                    }
                } else if let Some(&t) = c.targets.first() {
                    let mut reached = Vec::new();
                    let mut seen = FxHashSet::default();
                    expand(ix, rs, t, 0, &mut seen, &mut reached);
                    let via_macro = ix.def(t).kind() == DefKind::Macro;
                    for r in reached {
                        targets.push((r, if via_macro { "macro" } else { "static" }));
                    }
                }
                for (t, kind) in targets {
                    let to = endpoint(&ix.def(t));
                    writeln!(w, "{}", json!({"caller": from, "callee": to, "site_line": c.line, "kind": kind}))?;
                    n += 1;
                }
            }
        }
    }
    Ok(n)
}
