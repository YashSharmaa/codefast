//! `codefast rename TARGET NEW`: rename one function or method, at its definition and at every
//! place the index is sure refers to it.
//!
//! A rename by hand is a search, a read of every hit to tell this `Add` from the other types'
//! `Add`, and an edit per file. All three are things the index has already done. The command
//! changes only what it can vouch for: a reference whose receiver type or scope pins it to the
//! target. A call that matches by name alone is left as it is and listed, and so is a line
//! where the name stands for more than one thing. Nothing is renamed in comments or strings.

use super::{Common, PathFilter, parse_spec, targets_for};
use crate::lang::Lang;
use crate::output::{Out, count};
use crate::resolve::{Class, Conf, Resolver, Target};
use crate::store::{FileId, Index};
use rustc_hash::FxHashMap;
use std::collections::BTreeMap;
use std::path::Path;

fn ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// Where `name` stands as a whole word in the code of one line: not in a string and not in a
/// comment. Each occurrence comes with whether a member access leads to it (`x.name`,
/// `p->name`, `T::name`).
fn occurrences(line: &[u8], name: &[u8], lang: Lang) -> Vec<(usize, bool)> {
    let hash_comments = matches!(lang, Lang::Python | Lang::Ruby | Lang::Php | Lang::Bash);
    let mut out = Vec::new();
    let mut i = 0;
    let mut quote = 0u8;
    while i < line.len() {
        let c = line[i];
        if quote != 0 {
            if c == b'\\' {
                i += 1;
            } else if c == quote {
                quote = 0;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' | b'`' => quote = c,
            // A quote after a word is a Rust lifetime or an English apostrophe, not a string.
            b'\'' if !(i > 0 && ident(line[i - 1])) => quote = c,
            b'/' if line.get(i + 1) == Some(&b'/') => break,
            b'/' if line.get(i + 1) == Some(&b'*') => {
                i = memchr::memmem::find(&line[i + 2..], b"*/").map_or(line.len(), |e| i + 2 + e + 2);
                continue;
            }
            b'#' if hash_comments => break,
            _ if line[i..].starts_with(name) && !(i > 0 && ident(line[i - 1])) && !line.get(i + name.len()).copied().is_some_and(ident) => {
                let before = line[..i].trim_ascii_end();
                let member = before.ends_with(b".") || before.ends_with(b"->") || before.ends_with(b"::");
                out.push((i, member));
                i += name.len();
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// What is to happen on one line of one file.
#[derive(Default)]
struct LinePlan {
    /// References to the target that are sure, and definitions of it.
    ours: usize,
    /// Of those, how many are reached through a member access.
    ours_member: usize,
    /// A definition of the target starts here.
    def: bool,
}

pub struct Options {
    pub dry_run: bool,
}

pub fn rename(ix: &Index, rs: &Resolver, root: &Path, target: &str, new: &str, opts: &Options, out: &mut Out) {
    let new_ok = !new.is_empty() && !new.as_bytes()[0].is_ascii_digit() && new.bytes().all(ident);
    if !new_ok {
        out.push(&format!("`{new}` is not a name: nothing was changed."));
        return;
    }
    let common = Common { paths: Vec::new(), kinds: None, tests: true, budget: 0 };
    let pf = PathFilter::new(&[]);
    let all: Vec<Target> = targets_for(ix, rs, &parse_spec(ix, target), &common, &pf);
    let callable: Vec<&Target> = all.iter().filter(|t| t.class == Class::Callable).collect();
    let t = match callable.as_slice() {
        [one] => *one,
        [] => {
            out.push(&format!(
                "`{target}` is not a function or method this index knows: nothing was changed. (`codefast find {target}` lists what the name stands for.)"
            ));
            return;
        }
        many => {
            out.push(&format!("`{target}` names {} functions. Say which one, as Owner.name or path:line. Nothing was changed.", many.len()));
            for t in many.iter().take(8) {
                out.push(&format!("  {}", super::def_line(&t.primary(ix))));
            }
            return;
        }
    };
    let old = t.name.clone();
    let old_str = String::from_utf8_lossy(&old).into_owned();
    let shown = t.primary(ix).qualified();
    if old.as_slice() == new.as_bytes() {
        out.push(&format!("{shown} is called `{new}` already."));
        return;
    }
    // The new name must be free where the old one lives.
    if let Some(owner) = t.owner_ty() {
        if let Some(&d) = rs.members_of(&owner, new.as_bytes()).first() {
            out.push(&format!("{} already has a member `{new}` ({}). Nothing was changed.", String::from_utf8_lossy(&owner.name), super::def_line(&ix.def(d))));
            return;
        }
    } else if let Some(d) = ix.defs_named(new.as_bytes()).into_iter().find(|&d| t.defs.iter().any(|&m| ix.def(m).file() == ix.def(d).file())) {
        out.push(&format!("`{new}` is already defined next to it ({}). Nothing was changed.", super::def_line(&ix.def(d))));
        return;
    }

    // What refers to it: for sure, through a supertype, or by name only.
    let mut plans: BTreeMap<(String, FileId), BTreeMap<u32, LinePlan>> = BTreeMap::new();
    let mut sure: FxHashMap<(FileId, u32), usize> = FxHashMap::default();
    let mut dispatched: Vec<(FileId, u32)> = Vec::new();
    let mut by_name: Vec<(FileId, u32)> = Vec::new();
    rs.refs_to(t, |r, conf| match conf {
        Conf::Exact => {
            let p = plans.entry((ix.path(r.file).to_string(), r.file)).or_default().entry(r.line()).or_default();
            p.ours += 1;
            if r.kind().is_member() {
                p.ours_member += 1;
            }
            *sure.entry((r.file, r.line())).or_default() += 1;
        }
        Conf::Likely => dispatched.push((r.file, r.line())),
        Conf::Name => by_name.push((r.file, r.line())),
    });
    for &d in &t.defs {
        let v = ix.def(d);
        let p = plans.entry((v.path().to_string(), v.file())).or_default().entry(v.rec.line).or_default();
        p.ours += 1;
        p.def = true;
    }
    if !dispatched.is_empty() {
        // A call on a supertype or an interface may run this method. Renaming it alone would
        // leave that call pointing at a name the type no longer has.
        dispatched.sort();
        dispatched.dedup();
        out.push(&format!(
            "{shown} is also called through a supertype or an interface at {} place{}. Renaming it alone would break them, so nothing was changed. Rename the declaration they go through, and its other implementations, in the same step:",
            count(dispatched.len()),
            if dispatched.len() == 1 { "" } else { "s" }
        ));
        for (f, l) in dispatched.iter().take(12) {
            out.push(&format!("  {}:{}", ix.path(*f), l));
        }
        return;
    }

    // Work the edits out file by file.
    let mut edits: Vec<(String, Vec<u8>, Vec<u32>)> = Vec::new();
    let mut unclear: Vec<(String, u32, String)> = Vec::new();
    let (mut n_defs, mut n_refs) = (0usize, 0usize);
    for ((path, file), lines) in &plans {
        let Some(src) = ix.source(*file) else { continue };
        let lang = ix.lang(*file);
        let starts = super::show::line_starts(&src);
        // Everything else in this file that carries the name, by line.
        let mut named: FxHashMap<u32, usize> = FxHashMap::default();
        for r in ix.refs_of_file(*file).filter(|r| r.name() == old.as_slice()) {
            *named.entry(r.line()).or_default() += 1;
        }
        for d in ix.defs_of_file(*file).filter(|d| d.name() == old.as_slice() && !t.defs.contains(&d.id)) {
            *named.entry(d.rec.line).or_default() += 1;
        }
        let mut text = src.to_vec();
        let mut changed: Vec<u32> = Vec::new();
        // From the last line up, so that offsets before an edit stay what they were.
        for (&line, plan) in lines.iter().rev() {
            let i = line as usize;
            if i == 0 || i > starts.len() {
                continue;
            }
            let (s, e) = (starts[i - 1], if i < starts.len() { starts[i] } else { src.len() });
            let occ = occurrences(&src[s..e], &old, lang);
            let refs_here = sure.get(&(*file, line)).copied().unwrap_or(0);
            let others = named.get(&line).copied().unwrap_or(0).saturating_sub(refs_here);
            // Which occurrences are ours: all of them when nothing else on the line has the
            // name, the member accesses when that is what our references are.
            let members: Vec<usize> = occ.iter().filter(|o| o.1).map(|o| o.0).collect();
            let plain: Vec<usize> = occ.iter().filter(|o| !o.1).map(|o| o.0).collect();
            let take: Option<Vec<usize>> = if others > 0 {
                None
            } else if occ.len() == plan.ours {
                Some(occ.iter().map(|o| o.0).collect())
            } else if plan.def && plan.ours == 1 && plain.len() == 1 {
                Some(plain)
            } else if !plan.def && plan.ours_member == plan.ours && members.len() == plan.ours {
                Some(members)
            } else {
                None
            };
            let Some(mut take) = take else {
                let shown = String::from_utf8_lossy(&src[s..e]).trim().to_string();
                unclear.push((path.clone(), line, crate::output::clip(&shown, 110)));
                continue;
            };
            take.sort_unstable_by(|a, b| b.cmp(a));
            for at in take {
                text.splice(s + at..s + at + old.len(), new.bytes());
            }
            changed.push(line);
            if plan.def {
                n_defs += 1;
            }
            n_refs += plan.ours - usize::from(plan.def);
        }
        if !changed.is_empty() {
            changed.reverse();
            edits.push((path.clone(), text, changed));
        }
    }
    if n_defs == 0 {
        out.push(&format!("The definition of {shown} could not be rewritten where it stands. Nothing was changed."));
        return;
    }
    if !opts.dry_run {
        for (path, text, _) in &edits {
            if let Err(e) = std::fs::write(root.join(path), text) {
                out.push(&format!("{path} could not be written ({e}). Files before it in this list are renamed, this one and those after it are not:"));
                for (p, _, _) in &edits {
                    out.push(&format!("  {p}"));
                }
                return;
            }
        }
    }
    let verb = if opts.dry_run { "would rename" } else { "renamed" };
    let defs = if n_defs == 1 { "its definition".to_string() } else { format!("its {} definitions", count(n_defs)) };
    out.push(&format!(
        "{verb} {shown} to {new}: {defs} and {} reference{} in {} file{}",
        count(n_refs),
        if n_refs == 1 { "" } else { "s" },
        count(edits.len()),
        if edits.len() == 1 { "" } else { "s" }
    ));
    for (path, _, lines) in &edits {
        let ls: Vec<String> = lines.iter().map(u32::to_string).collect();
        out.push(&format!("{path}: {}", ls.join(",")));
    }
    if !unclear.is_empty() {
        out.push(&format!(
            "not changed, {} line{} where `{old_str}` stands for more than this (edit by hand):",
            count(unclear.len()),
            if unclear.len() == 1 { "" } else { "s" }
        ));
        for (p, l, text) in unclear.iter().take(20) {
            out.push(&format!("  {p}:{l}: {text}"));
        }
    }
    by_name.sort();
    by_name.dedup();
    if !by_name.is_empty() {
        out.push(&format!(
            "not changed, {} use{} of `{old_str}` on a receiver whose type is not known (each may be another type's):",
            count(by_name.len()),
            if by_name.len() == 1 { "" } else { "s" }
        ));
        for (f, l) in by_name.iter().take(20) {
            let text = ix
                .source(*f)
                .map(|s| String::from_utf8_lossy(super::show::line_text(&s, &super::show::line_starts(&s), *l)).trim().to_string())
                .unwrap_or_default();
            out.push(&format!("  {}:{l}: {}", ix.path(*f), crate::output::clip(&text, 110)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occurrences_are_whole_words_in_code() {
        let at = |line: &str| occurrences(line.as_bytes(), b"Add", Lang::Go);
        assert_eq!(at("\ts.Add(l)"), [(3, true)]);
        assert_eq!(at("func (s *Set) Add(xs ...int) {"), [(14, false)]);
        // Not inside another word, a string or a comment.
        assert_eq!(at("x.AddAll(); y.PreAdd()"), []);
        assert_eq!(at("log(\"Add failed\") // Add again"), []);
        assert_eq!(at("a.Add(b) /* Add */ ; c.Add(d)"), [(2, true), (23, true)]);
        assert_eq!(occurrences(b"x.add(1)  # add one", b"add", Lang::Python), [(2, true)]);
        assert_eq!(occurrences(b"p->add(q); T::add(r)", b"add", Lang::Cpp), [(3, true), (14, true)]);
    }
}
