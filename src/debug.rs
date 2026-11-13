//! Human-readable dump of a file's extracted facts. Used by `codefast debug-facts` and by the
//! extractor snapshot tests, so the format must stay deterministic.

use crate::extract::Scratch;
use crate::facts::{FileFacts, NONE};
use crate::lang::{self, Parsers};
use std::fmt::Write;

/// Extract `src` as if it lived at `path` and render every fact.
pub fn dump_facts(path: &str, src: &[u8]) -> String {
    let lang = lang::detect(path, src);
    let mut parsers = Parsers::default();
    let mut facts = FileFacts::default();
    let mut scratch = Scratch::default();
    let ok = lang::extract(lang, src, path, &mut parsers, &mut facts, &mut scratch);
    let mut out = String::new();
    let _ = writeln!(out, "lang={} ok={} lines={} errors={}", lang.name(), ok, facts.nlines, facts.parse_errors);
    let nm = |i: u32| -> String { if i == NONE { "-".into() } else { String::from_utf8_lossy(facts.names.get(i)).into_owned() } };
    let _ = writeln!(out, "--- defs ({})", facts.defs.len());
    for (i, d) in facts.defs.iter().enumerate() {
        let s = (d.sig_off as usize).min(src.len());
        let e = (s + d.sig_len as usize).min(src.len());
        let sig = String::from_utf8_lossy(&src[s..e]).split_whitespace().collect::<Vec<_>>().join(" ");
        let parent = if d.parent != NONE {
            nm(facts.defs[d.parent as usize].name)
        } else if d.qual != NONE {
            format!("~{}", nm(d.qual))
        } else {
            "-".to_string()
        };
        let _ = writeln!(
            out,
            "{i:3} {:7} {:28} L{}-{} parent={} type={} arity={}..{} flags={:#04x} | {}",
            d.kind.tag(),
            nm(d.name),
            d.line,
            d.end_line,
            parent,
            nm(d.type_name),
            d.arity_min,
            d.arity_max,
            d.flags,
            sig.chars().take(100).collect::<String>()
        );
    }
    let _ = writeln!(out, "--- refs ({})", facts.refs.len());
    let mut refs: Vec<_> = facts.refs.iter().collect();
    refs.sort_by_key(|r| (r.line, r.kind as u8, facts.names.get(r.name).to_vec()));
    for r in refs {
        let encl = if r.encl == NONE { "<file>".to_string() } else { nm(facts.defs[r.encl as usize].name) };
        let nargs = if r.nargs == 255 { String::new() } else { format!(" nargs={}", r.nargs) };
        let qual = if r.qual == NONE { String::new() } else { format!(" qual={}", nm(r.qual)) };
        let _ = writeln!(out, "L{:<4} {:7} {:28} in {}{}{}", r.line, r.kind.tag(), nm(r.name), encl, qual, nargs);
    }
    let _ = writeln!(out, "--- imports ({})", facts.imports.len());
    for im in &facts.imports {
        let _ = writeln!(out, "L{:<4} target={} name={} alias={} flags={}", im.line, nm(im.target), nm(im.name), nm(im.alias), im.flags);
    }
    out
}
