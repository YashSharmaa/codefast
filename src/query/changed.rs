//! What an edit changed, by symbol, and what it left behind.
//!
//! Two questions that otherwise cost an agent a compile, a test run or a round of greps:
//!
//! * `changed`: which definitions the working tree adds, removes or alters against the last
//!   commit. A session that has lost its early context gets its bearings back from this in a
//!   few lines instead of reading its own diff.
//! * `check`: which places still use a definition that an edit removed or renamed, and which
//!   calls no longer fit a signature that changed. Renames and changed signatures fail by
//!   being carried through only in part, and in a language without a compiler nothing says so
//!   until the code runs.
//!
//!
//! `check` also looks at what the edit wrote: a use of a member that the receiver's type does
//! not have and that nothing in the repository defines. That is how a call of a method that
//! was imagined, not looked up, shows before the code runs.
//!
//! Both compare a file with its committed version (`git show HEAD:path`). No history is kept:
//! the answer is derived from the two texts each time, so it cannot go stale.

use super::refs::{dangling_member_refs, list_refs, proximity_key};
use super::{Common, PathFilter};
use crate::extract::Scratch;
use crate::facts::{DefKind, FileFacts, NONE, RefKind, dflag};
use crate::lang::{self, Lang, Parsers};
use crate::output::{Out, count};
use crate::resolve::{Class, Conf, Resolver, same_language_family};
use crate::store::{FileId, Index, RefV};
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::Path;
use std::process::{Command, Stdio};

/// One definition of one version of a file, reduced to what a comparison needs.
#[derive(Clone, Debug, PartialEq)]
pub struct DefSum {
    /// The container: the class of a method, the struct of a field, a Go receiver type.
    pub owner: Option<String>,
    pub name: String,
    pub kind: DefKind,
    pub line: u32,
    pub arity: (u8, u8),
    pub flags: u8,
    /// Of the definition's text, so that a moved but unchanged body compares equal.
    pub hash: u64,
}

impl DefSum {
    pub fn qualified(&self) -> String {
        match &self.owner {
            Some(o) => format!("{o}.{}", self.name),
            None => self.name.clone(),
        }
    }

    fn is_member(&self) -> bool {
        self.owner.is_some() && matches!(self.kind, DefKind::Method | DefKind::Field | DefKind::Property | DefKind::EnumMember)
    }
}

/// Functions and methods are one group, so that a function moved into a class is the same thing.
fn group(k: DefKind) -> u8 {
    match k {
        DefKind::Function | DefKind::Method => 1,
        DefKind::Class | DefKind::Struct | DefKind::Union | DefKind::Interface | DefKind::Trait | DefKind::Enum | DefKind::TypeAlias => 2,
        DefKind::Field | DefKind::Property => 3,
        DefKind::Variable | DefKind::Constant => 4,
        DefKind::EnumMember => 5,
        DefKind::Macro => 6,
        DefKind::Module | DefKind::Namespace | DefKind::Impl => 7,
    }
}

/// The definitions of `src`, read as the file `path`.
pub fn sums_of_source(path: &str, src: &[u8]) -> Vec<DefSum> {
    let lang = lang::detect(path, src);
    let mut facts = FileFacts::default();
    if !lang::extract(lang, src, path, &mut Parsers::default(), &mut facts, &mut Scratch::default()) {
        return Vec::new();
    }
    let starts = super::show::line_starts(src);
    let name = |i: u32| String::from_utf8_lossy(facts.names.get(i)).into_owned();
    facts
        .defs
        .iter()
        .filter(|d| !matches!(d.kind, DefKind::Impl | DefKind::Module | DefKind::Namespace))
        .map(|d| {
            let owner = if d.parent != NONE {
                Some(name(facts.defs[d.parent as usize].name))
            } else if d.qual != NONE {
                Some(name(d.qual))
            } else {
                None
            };
            let from = starts.get(d.line.saturating_sub(1) as usize).copied().unwrap_or(0);
            let to = starts.get(d.end_line as usize).copied().unwrap_or(src.len()).max(from);
            DefSum { owner, name: name(d.name), kind: d.kind, line: d.line, arity: (d.arity_min, d.arity_max), flags: d.flags, hash: xxhash_rust::xxh3::xxh3_64(&src[from..to]) }
        })
        .collect()
}

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git").arg("-C").arg(root).args(args).env("GIT_OPTIONAL_LOCKS", "0").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// The committed text of a file, or nothing when it is new, or the tree is not a git checkout.
pub fn head_blob(root: &Path, rel: &str) -> Option<Vec<u8>> {
    git(root, &["show", &format!("HEAD:./{rel}")])
}

/// Files that differ from the last commit, as paths under `root`: modified, added, deleted and
/// untracked ones. Empty when the tree is not a git checkout.
pub fn dirty_files(root: &Path) -> Vec<String> {
    let Some(out) = git(root, &["status", "--porcelain=v1", "-z", "--untracked-files=all", "--no-renames", "--", "."]) else { return Vec::new() };
    // Paths are given from the top of the work tree, which may lie above `root`.
    let prefix = git(root, &["rev-parse", "--show-prefix"]).map(|p| String::from_utf8_lossy(&p).trim().to_string()).unwrap_or_default();
    let mut files = Vec::new();
    for entry in out.split(|&b| b == 0) {
        if entry.len() < 4 {
            continue;
        }
        let path = String::from_utf8_lossy(&entry[3..]);
        if let Some(rel) = path.strip_prefix(prefix.as_str()) {
            files.push(rel.to_string());
        }
        if files.len() >= 2000 {
            break;
        }
    }
    files.sort();
    files
}

#[derive(Default, Debug)]
pub struct Delta {
    pub removed: Vec<DefSum>,
    pub added: Vec<DefSum>,
    /// The same definition with another body, as it is now.
    pub altered: Vec<DefSum>,
    /// A callable whose parameter count changed: before, after.
    pub arity: Vec<(DefSum, DefSum)>,
}

impl Delta {
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.added.is_empty() && self.altered.is_empty()
    }

    /// What probably became of a removed definition: the one added in the same container, of
    /// the same kind, at the same place or with the same parameters.
    pub fn successor(&self, gone: &DefSum) -> Option<&DefSum> {
        let same = |a: &&DefSum| a.owner == gone.owner && group(a.kind) == group(gone.kind);
        let mut near: Vec<&DefSum> = self.added.iter().filter(same).filter(|a| a.line.abs_diff(gone.line) <= 3).collect();
        if near.len() != 1 {
            near = self.added.iter().filter(same).filter(|a| a.arity == gone.arity && gone.kind.is_callable()).collect();
        }
        match near[..] {
            [one] => Some(one),
            _ => None,
        }
    }
}

type Key = (Option<String>, String, u8);

fn key(d: &DefSum) -> Key {
    (d.owner.clone(), d.name.clone(), group(d.kind))
}

/// Compare two versions of one file, definition by definition.
pub fn delta(old: &[DefSum], new: &[DefSum]) -> Delta {
    let mut olds: FxHashMap<Key, Vec<&DefSum>> = FxHashMap::default();
    for d in old {
        olds.entry(key(d)).or_default().push(d);
    }
    let mut news: FxHashMap<Key, Vec<&DefSum>> = FxHashMap::default();
    for d in new {
        news.entry(key(d)).or_default().push(d);
    }
    let mut out = Delta::default();
    for d in old {
        if !news.contains_key(&key(d)) && !out.removed.iter().any(|r| key(r) == key(d)) {
            out.removed.push(d.clone());
        }
    }
    for d in new {
        let k = key(d);
        match olds.get(&k) {
            None => {
                if !out.added.iter().any(|a| key(a) == k) {
                    out.added.push(d.clone());
                }
            }
            Some(before) => {
                // Overloads and redefinitions: the name stands for several bodies. It changed
                // when the set of bodies did.
                let now = &news[&k];
                let same = before.len() == now.len() && before.iter().all(|b| now.iter().any(|n| n.hash == b.hash));
                if !same && !out.altered.iter().any(|a| key(a) == k) {
                    out.altered.push(d.clone());
                    if let ([b], [n]) = (&before[..], &now[..]) {
                        if b.arity != n.arity && n.kind.is_callable() {
                            out.arity.push(((*b).clone(), (*n).clone()));
                        }
                    }
                }
            }
        }
    }
    out
}

/// The delta of one file against its committed version. `None` when the file is unchanged, is
/// not a source file, or has no committed version to compare with (a new file removes nothing).
pub fn file_delta(root: &Path, rel: &str) -> Option<(Delta, bool)> {
    let now = std::fs::read(root.join(rel)).ok();
    let before = head_blob(root, rel);
    let (old, new, fresh) = match (&before, &now) {
        (Some(b), Some(n)) if b == n => return None,
        (Some(b), Some(n)) => (sums_of_source(rel, b), sums_of_source(rel, n), false),
        (Some(b), None) => (sums_of_source(rel, b), Vec::new(), false),
        (None, Some(n)) => (Vec::new(), sums_of_source(rel, n), true),
        (None, None) => return None,
    };
    let d = delta(&old, &new);
    (!d.is_empty()).then_some((d, fresh))
}

/// Places that still name a definition `file` no longer has. Empty when the name is defined
/// elsewhere with the same owner (it moved), and when nothing can be said with confidence.
pub fn left_behind<'a>(ix: &'a Index, rs: &Resolver<'a>, file: Option<FileId>, lang: Lang, gone: &DefSum) -> (Vec<RefV<'a>>, Vec<RefV<'a>>) {
    let none = (Vec::new(), Vec::new());
    let Some(nid) = ix.name_id(gone.name.as_bytes()) else { return none };
    let owner = gone.owner.as_deref().map(str::as_bytes);
    let moved = ix.defs_named_id(nid).any(|d| {
        let v = ix.def(d);
        same_language_family(ix.lang(v.file()), lang) && v.parent_name() == owner && group(v.kind()) == group(gone.kind)
    });
    if moved {
        return none;
    }
    if gone.is_member() {
        let owner = gone.owner.as_deref().unwrap_or("");
        let typed = dangling_member_refs(ix, rs, owner, &gone.name);
        // Uses of the name on a value whose type is not written. Where no type has a member
        // of this name any more they can only have meant this one. Where other types still
        // do, each may be theirs: a handful is worth showing, a crowd is not.
        let mut loose = Vec::new();
        ix.for_each_ref_named(nid, |r| {
            if !r.kind().is_member() || !same_language_family(ix.lang(r.file), lang) || typed.iter().any(|t| t.file == r.file && t.line() == r.line()) {
                return;
            }
            let known = r.qual().is_some_and(|q| !rs.type_path_from(q.strip_prefix(b"^").unwrap_or(q), Some(r.file)).is_empty());
            if !known {
                loose.push(r);
            }
        });
        if rs.member_owner_count(gone.name.as_bytes()) > 0 && loose.len() > 12 {
            loose.clear();
        }
        return (typed, loose);
    }
    // A free name: every use of it in a file that could see the definition. Files that import
    // from the defining file, files of its directory (one package in most languages), and in
    // C and C++, where names are global, every file.
    let mut scope: FxHashSet<FileId> = FxHashSet::default();
    let global = matches!(lang, Lang::C | Lang::Cpp);
    if let Some(f) = file {
        scope.insert(f);
        scope.extend(rs.importers_of(f).iter().copied());
    }
    let dir = |p: &str| p.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
    let home = file.map(|f| dir(ix.path(f)));
    let mut hits = Vec::new();
    ix.for_each_ref_named(nid, |r| {
        if r.kind().is_member() || !same_language_family(ix.lang(r.file), lang) {
            return;
        }
        let visible = global || scope.contains(&r.file) || home.as_ref().is_some_and(|h| dir(r.path()) == *h);
        if visible {
            hits.push(r);
        }
    });
    (hits, Vec::new())
}

/// Calls of `now` (a definition of `file`) that pass a number of arguments it does not take.
pub fn misfit_calls<'a>(ix: &'a Index, rs: &Resolver<'a>, file: FileId, now: &DefSum) -> Vec<RefV<'a>> {
    let (min, max) = now.arity;
    if max == 255 && min == 0 {
        return Vec::new();
    }
    let owner = now.owner.as_deref().map(str::as_bytes);
    let defs: Vec<_> = ix.defs_of_file(file).filter(|d| d.name() == now.name.as_bytes() && d.parent_name() == owner && d.kind().is_callable()).map(|d| d.id).collect();
    let Some(t) = rs.entities(&defs).into_iter().find(|t| t.class == Class::Callable) else { return Vec::new() };
    let mut out = Vec::new();
    rs.refs_to(&t, |r, conf| {
        let n = r.rec.nargs;
        let call = matches!(r.kind(), RefKind::Call | RefKind::MemberCall | RefKind::New);
        if call && conf != Conf::Name && n != 255 && (n < min || (max != 255 && n > max)) {
            out.push(r);
        }
    });
    out
}

/// Uses, in the definitions an edit added or altered, of a member that does not exist: the
/// receiver is a value of a type of this repository whose ancestors are all in the repository
/// too, neither it nor they have a member of that name, and no definition of that name exists
/// anywhere. Each of the three conditions guards against a different false alarm (a member
/// inherited from a library type, an extension defined elsewhere, a name the index knows).
/// Rust is left out: its methods come from traits a type does not list.
fn phantom_members<'a>(ix: &'a Index, rs: &Resolver<'a>, file: FileId, d: &Delta) -> Vec<(RefV<'a>, String, Vec<String>)> {
    if matches!(ix.lang(file), Lang::Rust | Lang::Other) {
        return Vec::new();
    }
    // A file the grammar could not read in full (Cython under a `.py` grammar, a macro-heavy
    // header) may hold definitions the index did not see: nothing can be said to be missing.
    let unsure = |f: FileId| ix.file(f).parse_errors > 0 || ix.path(f).ends_with(".pyx") || ix.path(f).ends_with(".pxd");
    if unsure(file) {
        return Vec::new();
    }
    let touched: FxHashSet<&str> = d.added.iter().chain(&d.altered).map(|s| s.name.as_str()).collect();
    let mut out: Vec<(RefV, String, Vec<String>)> = Vec::new();
    let mut seen: FxHashSet<(Vec<u8>, Vec<u8>)> = FxHashSet::default();
    // Where classes get attributes at run time (a metaclass, a factory, `setattr`), an attribute
    // read proves nothing. A call of a method nobody defines is still worth a line.
    let dynamic = matches!(ix.lang(file), Lang::Python | Lang::Ruby | Lang::Php | Lang::JavaScript);
    for r in ix.refs_of_file(file) {
        if !(r.kind() == RefKind::MemberCall || (r.kind() == RefKind::MemberRead && !dynamic)) || r.name().starts_with(b"__") {
            continue;
        }
        let Some(encl) = r.encl() else { continue };
        if !touched.contains(encl.name_str().as_ref()) {
            continue;
        }
        // Defined somewhere, as anything: then it may be reached in a way the index does not see.
        if ix.name_id(r.name()).is_some_and(|n| ix.defs_named_id(n).next().is_some()) {
            continue;
        }
        let Some(q) = r.qual() else { continue };
        let tys = rs.type_path_from(q.strip_prefix(b"^").unwrap_or(q), Some(file));
        let [ty] = &tys[..] else { continue };
        // A type of this repository, and so is everything it extends, implements or embeds:
        // a base that is not defined here (a library class) may bring any member with it.
        let known = |t: &crate::resolve::Ty| {
            !t.defs.is_empty()
                && t.defs.iter().all(|&x| {
                    let v = ix.def(x);
                    v.kind().is_type()
                        && v.kind() != DefKind::TypeAlias
                        && ix
                            .refs_of_file(v.file())
                            .filter(|b| matches!(b.kind(), RefKind::Extends | RefKind::Implements) && b.encl().is_some_and(|e| e.id == x))
                            // Written with a module in front (`unittest.TestCase`) it may be a
                            // library's class that shares its name with one of ours.
                            .all(|b| b.qual().is_none() && ix.name_id(b.name()).is_some_and(|n| ix.defs_named_id(n).any(|y| ix.def(y).kind().is_type())))
                })
        };
        if !known(ty) || !rs.ancestors(ty).iter().all(known) || ty.defs.iter().any(|&x| unsure(ix.def(x).file())) || !seen.insert((ty.name.clone(), r.name().to_vec())) {
            continue;
        }
        // What the type does have, nearest names first: the one that was meant is usually there.
        let wanted = String::from_utf8_lossy(r.name()).to_lowercase();
        let mut members: Vec<String> = Vec::new();
        for &t in &ty.defs {
            let home = ix.def(t);
            for m in ix.defs_of_file(home.file()) {
                if m.parent_name() == Some(ty.name.as_slice()) && matches!(m.kind(), DefKind::Method | DefKind::Function | DefKind::Field | DefKind::Property) {
                    let n = m.name_str().into_owned();
                    if !members.contains(&n) && !n.starts_with("__") {
                        members.push(n);
                    }
                }
            }
        }
        if members.is_empty() {
            continue;
        }
        let shared = |m: &String| {
            let m = m.to_lowercase();
            let prefix = m.bytes().zip(wanted.bytes()).take_while(|(a, b)| a == b).count();
            let within = usize::from(m.contains(&wanted) || wanted.contains(&m)) * m.len().min(wanted.len());
            std::cmp::Reverse(prefix.max(within))
        };
        members.sort_by_key(shared);
        members.truncate(6);
        out.push((r, String::from_utf8_lossy(&ty.name).into_owned(), members));
    }
    out
}

/// One thing an edit left inconsistent.
pub struct Finding {
    /// Stable for the same finding: what a caller uses to say it only once.
    pub key: String,
    pub text: String,
    /// The finding without its list of places, for a reader who has just been shown that list.
    pub brief: String,
    /// The symbol it is about, as `search` and `callers` name it.
    pub symbol: String,
}

fn sites(ix: &Index, refs: &mut Vec<RefV>, near: &str, budget: usize) -> (String, usize, usize) {
    refs.sort_by_cached_key(|r| proximity_key(ix, near, r));
    let files = refs.iter().map(|r| r.file).collect::<FxHashSet<_>>().len();
    let mut sub = Out::new(budget);
    list_refs(ix, refs, false, &mut sub);
    (sub.finish(), refs.len(), files)
}

fn arity_text((min, max): (u8, u8)) -> String {
    match (min, max) {
        (a, 255) => format!("{a} or more arguments"),
        (a, b) if a == b => format!("{a} argument{}", if a == 1 { "" } else { "s" }),
        (a, b) => format!("{a} to {b} arguments"),
    }
}

/// What the edits to `rel` left behind in the rest of the code. `ix` has to be current.
pub fn check_file(ix: &Index, rs: &Resolver, root: &Path, rel: &str, budget: usize) -> Vec<Finding> {
    let Some((d, _)) = file_delta(root, rel) else { return Vec::new() };
    let file = ix.file_by_path(rel);
    let lang = file.map(|f| ix.lang(f)).unwrap_or_else(|| Lang::from_path(rel));
    let mut out = Vec::new();
    // Callables and types first: they are what other files use.
    let mut removed: Vec<&DefSum> = d.removed.iter().filter(|g| g.flags & dflag::TEST == 0).collect();
    removed.sort_by_key(|g| (!g.kind.is_callable() && !g.kind.is_type(), g.line));
    for gone in removed.into_iter().take(8) {
        let (mut typed, mut loose) = left_behind(ix, rs, file, lang, gone);
        if typed.is_empty() && loose.is_empty() {
            continue;
        }
        let name = gone.qualified();
        let new = d.successor(gone).map(|a| format!(" (new there: {})", a.name)).unwrap_or_default();
        let mut text = String::new();
        let total = typed.len() + loose.len();
        let brief = format!("{name} is no longer defined in {rel}{new}. The {} place{} that still use{} it {} listed above.", count(total), if total == 1 { "" } else { "s" }, if total == 1 { "s" } else { "" }, if total == 1 { "is" } else { "are" });
        if !typed.is_empty() {
            let (list, n, files) = sites(ix, &mut typed, rel, budget);
            text.push_str(&format!("{name} is no longer defined in {rel}{new}. Still used at {} site{} in {} file{}:\n{list}", count(n), if n == 1 { "" } else { "s" }, count(files), if files == 1 { "" } else { "s" }));
        }
        if !loose.is_empty() {
            let (list, n, files) = sites(ix, &mut loose, rel, budget / 2);
            let lead = if text.is_empty() { format!("{name} is no longer defined in {rel}{new}. ") } else { String::new() };
            let others = rs.member_owner_count(gone.name.as_bytes()) > 0;
            let whose = if others { format!(" (other types have `{}` too, so each of these may be theirs)", gone.name) } else { String::new() };
            text.push_str(&format!(
                "{lead}Used by that name on a value whose type is not written at {} site{} in {} file{}{whose}:\n{list}",
                count(n),
                if n == 1 { "" } else { "s" },
                count(files),
                if files == 1 { "" } else { "s" }
            ));
        }
        out.push(Finding { key: format!("gone:{rel}:{name}"), text: text.trim_end().to_string(), brief, symbol: name });
    }
    if let Some(f) = file {
        for (r, ty, members) in phantom_members(ix, rs, f, &d).into_iter().take(4) {
            let name = String::from_utf8_lossy(r.name()).into_owned();
            let home = rs.type_path_from(r.qual().map(|q| q.strip_prefix(b"^").unwrap_or(q)).unwrap_or(b""), Some(f)).first().and_then(|t| t.defs.first().map(|&x| ix.def(x)));
            let at = home.map(|h| format!(" ({}:{})", h.path(), h.rec.line)).unwrap_or_default();
            let text = format!(
                "{rel}:{} uses {ty}.{name}, and nothing in this repository defines `{name}`. {ty}{at} has: {}.",
                r.line(),
                members.join(", ")
            );
            out.push(Finding { key: format!("phantom:{rel}:{ty}.{name}"), brief: text.clone(), text, symbol: format!("{ty}.{name}") });
        }
        for (before, now) in d.arity.iter().take(6) {
            let mut calls = misfit_calls(ix, rs, f, now);
            if calls.is_empty() {
                continue;
            }
            let (list, n, files) = sites(ix, &mut calls, rel, budget);
            let text = format!(
                    "{} took {} and now takes {}. {} call{} in {} file{} pass another number:\n{}",
                    now.qualified(),
                    arity_text(before.arity),
                    arity_text(now.arity),
                    count(n),
                    if n == 1 { "" } else { "s" },
                    count(files),
                    if files == 1 { "" } else { "s" },
                    list.trim_end()
                );
            // A changed signature is news even to a reader who has the list of callers.
            out.push(Finding { key: format!("arity:{rel}:{}:{}-{}", now.qualified(), now.arity.0, now.arity.1), brief: text.clone(), text, symbol: now.qualified() });
        }
    }
    out
}

/// How often the check for members that do not exist would speak about code as it is: every
/// definition of every file is treated as just written. In code that builds and passes its
/// tests each finding is a false alarm, so this is the measure of the check's precision.
pub fn false_alarms(ix: &Index, rs: &Resolver, common: &Common, out: &mut Out) {
    let pf = PathFilter::new(&common.paths);
    let (mut files, mut refs, mut found) = (0usize, 0usize, 0usize);
    for f in ix.files() {
        if !pf.matches(ix.path(f)) || (!common.tests && ix.is_test_file(f)) {
            continue;
        }
        let all = Delta { altered: ix.defs_of_file(f).map(|d| DefSum { owner: None, name: d.name_str().into_owned(), kind: d.kind(), line: d.rec.line, arity: (0, 0), flags: 0, hash: 0 }).collect(), ..Default::default() };
        files += 1;
        refs += ix.refs_of_file(f).filter(|r| matches!(r.kind(), RefKind::MemberCall | RefKind::MemberRead)).count();
        for (r, ty, _) in phantom_members(ix, rs, f, &all) {
            found += 1;
            out.line(&format!("{}:{} {ty}.{}", r.path(), r.line(), String::from_utf8_lossy(r.name())));
        }
    }
    out.push(&format!("{} member uses in {} files, {} reported as naming a member that does not exist", count(refs), count(files), count(found)));
}

/// `codefast check [PATH...]`: what the uncommitted edits left behind.
pub fn check(ix: &Index, rs: &Resolver, paths: &[String], common: &Common, out: &mut Out) {
    let root = ix.root.clone();
    let pf = PathFilter::new(&common.paths);
    let files: Vec<String> = if paths.is_empty() { dirty_files(&root) } else { paths.iter().map(|p| p.trim_start_matches("./").to_string()).collect() };
    let files: Vec<String> = files.into_iter().filter(|f| pf.matches(f) && Lang::from_path(f).has_extractor()).collect();
    if files.is_empty() {
        out.push(if paths.is_empty() { "no source file differs from the last commit" } else { "none of these is a source file codefast reads" });
        return;
    }
    let mut n = 0;
    for rel in &files {
        for f in check_file(ix, rs, &root, rel, 600) {
            if n > 0 {
                out.push("");
            }
            out.push(&f.text);
            n += 1;
        }
    }
    if n == 0 {
        out.push(&format!("{} changed file{} checked: nothing they removed or re-signatured is still used elsewhere", count(files.len()), if files.len() == 1 { "" } else { "s" }));
    }
}

fn names(list: &[DefSum], sign: char, max: usize) -> Vec<String> {
    let mut v: Vec<String> = list.iter().filter(|d| !matches!(d.kind, DefKind::Field | DefKind::Variable | DefKind::EnumMember) || list.len() <= 4).map(|d| format!("{sign}{}", d.qualified())).collect();
    if v.len() > max {
        let more = v.len() - max;
        v.truncate(max);
        v.push(format!("{sign}{more} more"));
    }
    v
}

/// The symbol-level summary of the uncommitted changes, one line per file. Returns the number
/// of files listed.
pub fn summary(ix: &Index, common: &Common, out: &mut Out) -> usize {
    let root = ix.root.clone();
    let pf = PathFilter::new(&common.paths);
    let files: Vec<String> = dirty_files(&root).into_iter().filter(|f| pf.matches(f)).collect();
    let mut lines: Vec<String> = Vec::new();
    let mut other = 0usize;
    for rel in &files {
        if !Lang::from_path(rel).has_extractor() {
            other += 1;
            continue;
        }
        if !root.join(rel).exists() {
            lines.push(format!("{rel}: deleted"));
            continue;
        }
        match file_delta(&root, rel) {
            Some((d, fresh)) => {
                let mut parts = names(&d.removed, '-', 6);
                parts.extend(names(&d.added, '+', 6));
                parts.extend(names(&d.altered, '~', 8));
                let lead = if fresh { "new file " } else { "" };
                lines.push(format!("{lead}{rel}: {}", parts.join(" ")));
            }
            // Changed, but no definition differs: comments, formatting, code outside any symbol.
            None if head_blob(&root, rel).is_some() => lines.push(format!("{rel}: no definition differs")),
            None => lines.push(format!("new file {rel}")),
        }
    }
    if lines.is_empty() && other == 0 {
        return 0;
    }
    out.push(&format!("uncommitted changes, by definition ({} source file{}; + added, - removed, ~ body changed):", count(lines.len()), if lines.len() == 1 { "" } else { "s" }));
    let mut dropped = 0;
    for l in &lines {
        if !out.line(l) {
            dropped += 1;
        }
    }
    if dropped > 0 {
        out.push(&format!("… {} more files (narrow with --path DIR, or raise --budget)", count(dropped)));
    }
    if other > 0 {
        out.push(&format!("and {} other file{} (not source code codefast reads)", count(other), if other == 1 { "" } else { "s" }));
    }
    lines.len()
}

/// `codefast changed`: the summary, then what the changes left behind.
pub fn changed(ix: &Index, rs: &Resolver, common: &Common, out: &mut Out) {
    if summary(ix, common, out) == 0 {
        out.push("nothing differs from the last commit (or this is not a git work tree)");
        return;
    }
    let root = ix.root.clone();
    let pf = PathFilter::new(&common.paths);
    for rel in dirty_files(&root).into_iter().filter(|f| pf.matches(f) && Lang::from_path(f).has_extractor()) {
        for f in check_file(ix, rs, &root, &rel, 400) {
            out.push("");
            out.push(&f.text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rename_is_one_removed_and_one_added_at_the_same_place() {
        let old = sums_of_source("a.py", b"class A:\n    def run(self, x):\n        return x\n\ndef keep():\n    return 1\n");
        let new = sums_of_source("a.py", b"class A:\n    def execute(self, x):\n        return x\n\ndef keep():\n    return 1\n");
        let d = delta(&old, &new);
        assert_eq!(d.removed.iter().map(DefSum::qualified).collect::<Vec<_>>(), ["A.run"]);
        assert_eq!(d.added.iter().map(DefSum::qualified).collect::<Vec<_>>(), ["A.execute"]);
        assert_eq!(d.successor(&d.removed[0]).map(|a| a.name.as_str()), Some("execute"));
        // The class holds a different method now, so its text changed. `keep` did not.
        assert_eq!(d.altered.iter().map(DefSum::qualified).collect::<Vec<_>>(), ["A"]);
    }

    #[test]
    fn a_new_parameter_is_a_changed_arity() {
        let old = sums_of_source("a.go", b"package a\n\nfunc Load(path string) error {\n\treturn nil\n}\n");
        let new = sums_of_source("a.go", b"package a\n\nfunc Load(path string, strict bool) error {\n\treturn nil\n}\n");
        let d = delta(&old, &new);
        assert!(d.removed.is_empty() && d.added.is_empty());
        assert_eq!(d.arity.len(), 1);
        assert_eq!((d.arity[0].0.arity, d.arity[0].1.arity), ((1, 1), (2, 2)));
        assert_eq!(arity_text((1, 1)), "1 argument");
        assert_eq!(arity_text((1, 255)), "1 or more arguments");
    }

    #[test]
    fn a_moved_body_is_not_a_change() {
        let old = sums_of_source("a.py", b"def a():\n    return 1\n\ndef b():\n    return 2\n");
        let new = sums_of_source("a.py", b"def b():\n    return 2\n\n\ndef a():\n    return 1\n");
        assert!(delta(&old, &new).is_empty());
    }
}
