//! Queries: turning a question from a coding agent into a compact, exact answer.

pub mod card;
pub mod changed;
pub mod context;
pub mod export;
pub mod find;
pub mod grep;
pub mod locate;
pub mod refs;
pub mod search;
pub mod show;
pub mod trace;

use crate::facts::DefKind;
use crate::resolve::{Resolver, Target};
use crate::store::{DefId, DefV, FileId, Index};

/// Options shared by all queries.
#[derive(Clone, Debug, Default)]
pub struct Common {
    /// Restrict to paths: plain text matches as a substring, patterns with `*` as globs.
    pub paths: Vec<String>,
    pub kinds: Option<Vec<DefKind>>,
    /// Include test code (off by default for searches, where it is usually noise).
    pub tests: bool,
    /// Token budget for the answer; 0 = unlimited.
    pub budget: usize,
}

pub struct PathFilter {
    subs: Vec<String>,
    globs: Option<globset::GlobSet>,
}

impl PathFilter {
    pub fn new(patterns: &[String]) -> PathFilter {
        let mut subs = Vec::new();
        let mut gb = globset::GlobSetBuilder::new();
        let mut any_glob = false;
        for p in patterns {
            if p.contains(['*', '?', '[', '{']) {
                for pat in [p.clone(), format!("**/{p}")] {
                    if let Ok(g) = globset::GlobBuilder::new(&pat).literal_separator(false).build() {
                        gb.add(g);
                        any_glob = true;
                    }
                }
            } else {
                subs.push(p.trim_start_matches("./").to_string());
            }
        }
        let globs = if any_glob { gb.build().ok() } else { None };
        PathFilter { subs, globs }
    }

    pub fn is_empty(&self) -> bool {
        self.subs.is_empty() && self.globs.is_none()
    }

    pub fn matches(&self, path: &str) -> bool {
        if self.is_empty() {
            return true;
        }
        if self.subs.iter().any(|s| path.contains(s.as_str())) {
            return true;
        }
        self.globs.as_ref().is_some_and(|g| g.is_match(path))
    }
}

/// What the user pointed at.
#[derive(Debug, Clone)]
pub enum Spec {
    /// `path:line`
    Loc {
        file: FileId,
        line: u32,
    },
    /// `path:start-end`
    Range {
        file: FileId,
        start: u32,
        end: u32,
    },
    File(FileId),
    /// `name`, `Owner.name`, `Owner::name`, optionally prefixed by a kind word (`struct file`)
    Sym {
        owner: Option<String>,
        name: String,
        kinds: Option<Vec<DefKind>>,
    },
}

/// Find an indexed file by exact path or by unique path suffix.
pub fn resolve_file(ix: &Index, s: &str) -> Option<FileId> {
    let s = s.trim_start_matches("./");
    if let Some(root) = ix.root.to_str() {
        if let Some(rel) = s.strip_prefix(root) {
            let rel = rel.trim_start_matches('/');
            if let Some(f) = ix.file_by_path(rel) {
                return Some(f);
            }
        }
    }
    if let Some(f) = ix.file_by_path(s) {
        return Some(f);
    }
    if !s.contains('.') && !s.contains('/') {
        return None;
    }
    let suffix = format!("/{s}");
    let mut hit = None;
    for f in ix.files() {
        if ix.path(f).ends_with(&suffix) {
            if hit.is_some() {
                return None; // ambiguous
            }
            hit = Some(f);
        }
    }
    hit
}

/// `struct file`, `fn vfs_read`, `macro container_of`: a kind word in front of a name.
pub fn split_kind_prefix(s: &str) -> (Option<Vec<DefKind>>, &str) {
    let t = s.trim();
    if let Some((kw, rest)) = t.split_once(char::is_whitespace) {
        let rest = rest.trim();
        if !rest.is_empty() && !rest.contains(char::is_whitespace) {
            let kinds = match kw {
                "struct" => Some(vec![DefKind::Struct, DefKind::Class, DefKind::TypeAlias]),
                "union" => Some(vec![DefKind::Union]),
                "enum" => Some(vec![DefKind::Enum]),
                "class" => Some(vec![DefKind::Class, DefKind::Struct]),
                "interface" | "trait" | "protocol" => Some(vec![DefKind::Interface, DefKind::Trait]),
                "fn" | "func" | "function" | "def" | "method" => Some(vec![DefKind::Function, DefKind::Method]),
                "macro" | "#define" | "define" => Some(vec![DefKind::Macro]),
                "type" | "typedef" => DefKind::parse("type"),
                "var" | "const" | "static" => Some(vec![DefKind::Variable, DefKind::Constant]),
                _ => None,
            };
            if kinds.is_some() {
                return (kinds, rest);
            }
        }
    }
    (None, t)
}

pub fn parse_spec(ix: &Index, s: &str) -> Spec {
    let (kinds, s) = split_kind_prefix(s);
    // path:line or path:start-end
    if let Some(i) = s.rfind(':') {
        let (p, rest) = (&s[..i], &s[i + 1..]);
        if !p.is_empty() && !p.ends_with(':') {
            let range: Option<(u32, Option<u32>)> = match rest.split_once('-') {
                Some((a, b)) => a.parse().ok().zip(b.parse().ok()).map(|(a, b)| (a, Some(b))),
                None => rest.parse().ok().map(|a| (a, None)),
            };
            if let Some((a, b)) = range {
                if let Some(file) = resolve_file(ix, p) {
                    return match b {
                        Some(b) => Spec::Range { file, start: a, end: b.max(a) },
                        None => Spec::Loc { file, line: a },
                    };
                }
            }
        }
    }
    if ix.name_id(s.as_bytes()).is_none() || s.contains('/') {
        if let Some(file) = resolve_file(ix, s) {
            return Spec::File(file);
        }
    }
    // Owner.name / Owner::name / Owner#name / Owner->name
    for sep in ["::", "->", "#", "."] {
        if let Some(i) = s.rfind(sep) {
            let (owner, name) = (&s[..i], &s[i + sep.len()..]);
            if !owner.is_empty() && !name.is_empty() {
                let owner = owner.rsplit(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or(owner);
                return Spec::Sym { owner: Some(owner.to_string()), name: name.to_string(), kinds };
            }
        }
    }
    Spec::Sym { owner: None, name: s.to_string(), kinds }
}

/// Definitions a symbol spec denotes, filtered by the common options.
pub fn defs_for(ix: &Index, owner: Option<&str>, name: &str, common: &Common, pf: &PathFilter) -> Vec<DefId> {
    let mut defs = ix.defs_named(name.as_bytes());
    if let Some(o) = owner {
        let narrowed: Vec<DefId> = defs.iter().copied().filter(|&d| ix.def(d).parent_name() == Some(o.as_bytes())).collect();
        if !narrowed.is_empty() {
            defs = narrowed;
        } else {
            // `file.c::name` style: the "owner" may be a file name.
            let by_file: Vec<DefId> = defs.iter().copied().filter(|&d| ix.def(d).path().contains(o)).collect();
            defs = by_file;
        }
    }
    if let Some(kinds) = &common.kinds {
        defs.retain(|&d| kinds.contains(&ix.def(d).kind()));
    }
    if !pf.is_empty() {
        defs.retain(|&d| pf.matches(ix.def(d).path()));
    }
    defs
}

/// Prefixes that symbol-defining macros put in front of the name they are given.
const MACRO_NAME_PREFIXES: [&str; 4] = ["sys_", "compat_sys_", "dev_attr_", "trace_"];

/// Entities for a spec, best first.
pub fn targets_for(ix: &Index, rs: &Resolver, spec: &Spec, common: &Common, pf: &PathFilter) -> Vec<Target> {
    match spec {
        Spec::Sym { owner, name, kinds } => {
            let mut defs = defs_for(ix, owner.as_deref(), name, common, pf);
            if let Some(k) = kinds {
                let narrowed: Vec<DefId> = defs.iter().copied().filter(|&d| k.contains(&ix.def(d).kind())).collect();
                if !narrowed.is_empty() {
                    defs = narrowed;
                }
            }
            let mut ents = rs.entities(&defs);
            if owner.is_none() {
                // A name given to a defining macro (`SYSCALL_DEFINE3(openat, ..)`) is how people
                // refer to what the macro defines (`sys_openat`): offer those alongside.
                let mut extra = false;
                for prefix in MACRO_NAME_PREFIXES {
                    let alias = format!("{prefix}{name}");
                    let adefs: Vec<DefId> =
                        defs_for(ix, None, &alias, common, pf).into_iter().filter(|&d| ix.def(d).flags() & crate::facts::dflag::MACRO_DEFINED != 0).collect();
                    if !adefs.is_empty() {
                        ents.extend(rs.entities(&adefs));
                        extra = true;
                    }
                }
                if extra {
                    let rank = |t: &Target| t.defs.iter().map(|&d| crate::resolve::def_rank(ix, &ix.def(d))).max().unwrap_or(0) + (t.defs.len() as i64).min(20);
                    ents.sort_by_key(|t| std::cmp::Reverse(rank(t)));
                }
            }
            ents
        }
        Spec::Loc { file, line } => match ix.def_at(*file, *line) {
            Some(d) => {
                // The whole entity the definition belongs to, with this definition's group first.
                let all = ix.defs_named(d.name());
                let ents = rs.entities(&all);
                let (mine, rest): (Vec<Target>, Vec<Target>) = ents.into_iter().partition(|t| t.defs.contains(&d.id));
                mine.into_iter().chain(rest).take(1).collect()
            }
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// One-line description of a definition: `path:line signature`.
pub fn def_line(d: &DefV) -> String {
    let mut s = format!("{}:{} {}", d.path(), d.rec.line, sig_of(d));
    if let Some(p) = d.parent_name() {
        if matches!(d.kind(), DefKind::Field | DefKind::Property | DefKind::EnumMember | DefKind::Method) {
            s.push_str(&format!("  (in {})", String::from_utf8_lossy(p)));
        }
    }
    s
}

/// Signature with a marker when it is only a declaration.
pub fn sig_of(d: &DefV) -> String {
    let mut sig = crate::output::clip(&d.signature(), 200);
    if d.is_decl() {
        sig.push_str(" [decl]");
    }
    sig
}
