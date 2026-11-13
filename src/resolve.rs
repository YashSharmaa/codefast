//! Query-time linking: which definitions does a reference denote, and which references denote a
//! definition. Facts are linked by name, then narrowed by kind, visibility, arity, receiver type
//! and proximity.

use crate::facts::{DefKind, RefKind, dflag, iflag};
use crate::lang::Lang;
use crate::linkx::{self, Fit};
use crate::store::{DefId, DefV, FileId, Index, RefKey, RefV};
use rustc_hash::{FxHashMap, FxHashSet};
use std::cell::RefCell;
use std::rc::Rc;

/// What role a definition plays; references are matched against roles, not exact kinds.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Class {
    Callable,
    Type,
    Value,
    Member,
    Module,
}

pub fn class_of(d: &DefV) -> Class {
    match d.kind() {
        DefKind::Function | DefKind::Method => Class::Callable,
        // Object-like macros behave like constants, function-like macros like functions.
        DefKind::Macro => {
            if d.rec.arity_max == 0 {
                Class::Value
            } else {
                Class::Callable
            }
        }
        DefKind::Class | DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::Interface | DefKind::Trait | DefKind::TypeAlias => Class::Type,
        DefKind::Variable | DefKind::Constant | DefKind::EnumMember => Class::Value,
        DefKind::Field | DefKind::Property => Class::Member,
        DefKind::Module | DefKind::Namespace | DefKind::Impl => Class::Module,
    }
}

/// How sure we are that a reference denotes the target.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Conf {
    /// Matched by name only; receiver type or scope could not be checked.
    Name = 0,
    /// Consistent with scope, kind and arity.
    Likely = 1,
    /// Scope or receiver type pins it down.
    Exact = 2,
}

/// A set of definitions that denote the same entity (a function with its prototypes, the
/// per-architecture variants of one global symbol, a field of one struct, ...).
#[derive(Clone, Debug)]
pub struct Target {
    pub name: Vec<u8>,
    pub defs: Vec<DefId>,
    pub class: Class,
    /// Set when the entity is only visible inside one file.
    pub local_file: Option<FileId>,
    /// Container name for members.
    pub owner: Option<Vec<u8>>,
    /// The definitions of the owning type when scoping pins them down (import-scoped
    /// languages); empty when the owner is known by name only.
    pub scope: Vec<DefId>,
}

impl Target {
    /// The definition to show for this entity: a real definition before declarations, product
    /// code before tests.
    /// The owning type of a member entity.
    pub fn owner_ty(&self) -> Option<Ty> {
        self.owner.as_ref().map(|o| Ty { name: o.clone(), defs: self.scope.clone() })
    }

    pub fn primary<'a>(&self, ix: &'a Index) -> DefV<'a> {
        let mut best = ix.def(self.defs[0]);
        let mut best_score = i64::MIN;
        for &d in &self.defs {
            let v = ix.def(d);
            let s = def_rank(ix, &v);
            if s > best_score {
                best_score = s;
                best = v;
            }
        }
        best
    }
}

/// A resolved type: its name and, where scoping rules can pin them down (import-scoped
/// languages), the definitions it denotes. An empty `defs` means "any type of this name", which
/// is how C-family code and pseudo owners are matched.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Ty {
    pub name: Vec<u8>,
    pub defs: Vec<DefId>,
}

impl Ty {
    pub fn named(name: &[u8]) -> Ty {
        Ty { name: name.to_vec(), defs: Vec::new() }
    }

    /// Do two resolved types denote the same type?
    pub fn same(&self, o: &Ty) -> bool {
        self.name == o.name && (self.defs.is_empty() || o.defs.is_empty() || self.defs.iter().any(|d| o.defs.contains(d)))
    }
}

fn push_ty(out: &mut Vec<Ty>, t: Ty) {
    match out.iter_mut().find(|o| o.name == t.name && (o.defs.is_empty() == t.defs.is_empty())) {
        Some(o) => {
            o.defs.extend(t.defs);
            o.defs.sort();
            o.defs.dedup();
        }
        None => out.push(t),
    }
}

/// Split `module|rest`: a name written with a module qualifier.
pub fn split_module(s: &[u8]) -> (Option<&[u8]>, &[u8]) {
    match memchr::memchr(b'|', s) {
        Some(i) => (Some(&s[..i]), &s[i + 1..]),
        None => (None, s),
    }
}

/// Intrinsic rank of a definition among same-named ones: higher is more likely "the" definition.
pub fn def_rank(ix: &Index, d: &DefV) -> i64 {
    let mut s = 0i64;
    if !d.is_decl() {
        s += 1000;
    }
    let f = ix.file(d.file());
    if f.flags & crate::facts::fflag::TEST == 0 {
        s += 300;
    }
    if f.flags & (crate::facts::fflag::VENDOR | crate::facts::fflag::GENERATED) == 0 {
        s += 200;
    }
    if d.flags() & dflag::EXPORTED != 0 {
        s += 100;
    }
    s += match d.kind() {
        DefKind::Function | DefKind::Method | DefKind::Class | DefKind::Struct | DefKind::Interface | DefKind::Trait => 60,
        DefKind::Union | DefKind::Enum | DefKind::TypeAlias => 50,
        DefKind::Macro => 40,
        DefKind::Variable | DefKind::Constant => 30,
        _ => 10,
    };
    // An implementation file beats an inline variant or stub in a header.
    if f.flags & crate::facts::fflag::HEADER != 0 && matches!(d.kind(), DefKind::Function | DefKind::Method) {
        s -= 80;
    }
    // Longer bodies are more likely the real implementation than a stub.
    s += ((d.rec.end_line - d.rec.line).min(50)) as i64;
    // Shallower paths first.
    s -= d.path().bytes().filter(|&b| b == b'/').count() as i64;
    s
}

fn kind_compatible(class: Class, k: RefKind) -> bool {
    match class {
        Class::Callable => {
            matches!(
                k,
                RefKind::Call | RefKind::Read | RefKind::Write | RefKind::Bind | RefKind::Arg | RefKind::MacroCall | RefKind::Annotation | RefKind::Import
            )
        }
        Class::Type => matches!(
            k,
            RefKind::Type
                | RefKind::New
                | RefKind::Extends
                | RefKind::Implements
                | RefKind::Call
                | RefKind::Read
                | RefKind::Arg
                | RefKind::Import
                | RefKind::Annotation
        ),
        Class::Value => matches!(k, RefKind::Read | RefKind::Write | RefKind::Call | RefKind::Bind | RefKind::Arg | RefKind::Import | RefKind::Type),
        Class::Member => matches!(k, RefKind::MemberRead | RefKind::MemberWrite | RefKind::MemberCall),
        // `include M` (Ruby) names a module as a supertype.
        Class::Module => matches!(k, RefKind::Import | RefKind::Read | RefKind::Type | RefKind::Extends | RefKind::Implements),
    }
}

pub struct Resolver<'a> {
    pub ix: &'a Index,
    type_paths: RefCell<FxHashMap<(Option<FileId>, Vec<u8>), Rc<Vec<Ty>>>>,
    owner_tys: RefCell<FxHashMap<DefId, Option<Rc<Ty>>>>,
    impls: RefCell<FxHashMap<DefId, Rc<Vec<DefId>>>>,
    supers_ty: RefCell<FxHashMap<DefId, Rc<Vec<Ty>>>>,
    subs_ty: RefCell<FxHashMap<Ty, Rc<Vec<Ty>>>>,
    module_idx: RefCell<Option<Rc<FxHashMap<Vec<u8>, Vec<FileId>>>>>,
    mod_files: RefCell<FxHashMap<(FileId, Vec<u8>), Rc<Vec<FileId>>>>,
    exports: RefCell<FxHashMap<(FileId, Vec<u8>), Rc<Vec<DefId>>>>,
    packages: RefCell<FxHashMap<FileId, Option<Rc<Vec<u8>>>>>,
    gomods: RefCell<FxHashMap<String, Option<Rc<(String, String)>>>>,
    file_names: RefCell<FxHashMap<FileId, Rc<FxHashMap<Vec<u8>, Vec<DefId>>>>>,
    pkg_idx: RefCell<Option<Rc<FxHashMap<String, Vec<FileId>>>>>,
    pkg_defs: RefCell<FxHashMap<(String, Vec<u8>), Rc<Vec<DefId>>>>,
    members_idx: RefCell<FxHashMap<Vec<u8>, Rc<FxHashMap<Vec<u8>, Vec<DefId>>>>>,
    desc: RefCell<FxHashMap<Ty, Rc<Vec<Ty>>>>,
    impl_maps: RefCell<FxHashMap<Vec<u8>, Rc<FxHashMap<DefId, Vec<DefId>>>>>,
    field_binds: RefCell<FxHashMap<Vec<u8>, Rc<Vec<RefKey>>>>,
    slot_binds: RefCell<FxHashMap<(Ty, Vec<u8>), Rc<Vec<(RefKey, Conf)>>>>,
    bound: RefCell<FxHashMap<(FileId, u32, Vec<u8>), Rc<Vec<DefId>>>>,
    importers: RefCell<FxHashMap<FileId, Rc<Vec<FileId>>>>,
    macro_links: RefCell<Option<Rc<MacroLinks>>>,
    pastes: RefCell<FxHashMap<Vec<u8>, Rc<Vec<(Vec<u8>, Vec<u8>)>>>>,
    aliases: RefCell<FxHashMap<DefId, Option<DefId>>>,
    canon: RefCell<FxHashMap<Vec<u8>, Vec<u8>>>,
    includes: RefCell<FxHashMap<FileId, Rc<Vec<Vec<u8>>>>>,
    suffix_files: RefCell<FxHashMap<Vec<u8>, Rc<Vec<FileId>>>>,
    owners: RefCell<FxHashMap<Vec<u8>, Rc<Vec<(Vec<u8>, bool)>>>>,
    inc_files: RefCell<FxHashMap<FileId, Rc<Vec<FileId>>>>,
    ents: RefCell<FxHashMap<Vec<u8>, Rc<Vec<Target>>>>,
    imports: RefCell<FxHashMap<FileId, Rc<Vec<Imp>>>>,
    supers: RefCell<FxHashMap<Vec<u8>, Rc<Vec<Vec<u8>>>>>,
    subs: RefCell<FxHashMap<Vec<u8>, Rc<Vec<Vec<u8>>>>>,
    global_usings: RefCell<Option<Rc<Vec<Imp>>>>,
    ext_recv: RefCell<FxHashMap<DefId, Option<Vec<u8>>>>,
    reopen: RefCell<FxHashMap<DefId, Rc<Vec<DefId>>>>,
}

/// Calls that only exist after macro expansion, as the Linux kernel writes them: a static call
/// goes through a key (`static_call(key)(..)`), the key is tied to a member of an ops table
/// (`static_call_update(key, ops.member)`), and wrapper macros build the key by pasting a name
/// onto a prefix (`kvm_x86_call(vcpu_run)` is `static_call(kvm_x86_vcpu_run)`).
#[derive(Default)]
pub struct MacroLinks {
    /// key -> the table slots its value is taken from
    key_slots: FxHashMap<Vec<u8>, Vec<(Ty, Vec<u8>)>>,
    /// key -> functions installed in it directly
    key_funcs: FxHashMap<Vec<u8>, Vec<Vec<u8>>>,
    /// pasted key prefix -> the table type whose members complete it
    prefix_owner: Vec<(Vec<u8>, Ty)>,
    /// wrapper macro -> key prefix
    wrappers: FxHashMap<Vec<u8>, Vec<u8>>,
}

const STATIC_CALLS: [&[u8]; 4] = [b"static_call", b"static_call_cond", b"static_call_mod", b"static_call_query"];
const STATIC_DEFS: [&[u8]; 4] = [b"DEFINE_STATIC_CALL", b"DEFINE_STATIC_CALL_NULL", b"DEFINE_STATIC_CALL_RET0", b"DECLARE_STATIC_CALL"];
const STATIC_UPDATE: &[u8] = b"static_call_update";

fn ident_at(s: &[u8], mut i: usize) -> (&[u8], usize) {
    while i < s.len() && s[i].is_ascii_whitespace() {
        i += 1;
    }
    let st = i;
    while i < s.len() && (s[i].is_ascii_alphanumeric() || s[i] == b'_') {
        i += 1;
    }
    (&s[st..i], i)
}

/// `PREFIX ## param` right after position `i` (which follows an opening parenthesis).
fn pasted_at<'s>(s: &'s [u8], i: usize) -> Option<(&'s [u8], &'s [u8], usize)> {
    let (prefix, mut j) = ident_at(s, i);
    while j < s.len() && s[j].is_ascii_whitespace() {
        j += 1;
    }
    if prefix.is_empty() || !s[j..].starts_with(b"##") {
        return None;
    }
    let (param, k) = ident_at(s, j + 2);
    (!param.is_empty()).then_some((prefix, param, k))
}

/// One import of a file, with names resolved to text.
#[derive(Clone, Debug)]
pub struct Imp {
    pub target: Vec<u8>,
    pub name: Option<Vec<u8>>,
    pub alias: Option<Vec<u8>>,
    pub flags: u8,
}

/// Languages whose references are linked through imports and packages rather than includes.
fn module_language(lang: Lang) -> bool {
    !matches!(lang, Lang::C | Lang::Cpp | Lang::ObjC | Lang::Other | Lang::Bash)
}

/// Is the definition a member of a type (a method, as opposed to a free or nested function)?
fn type_member(v: &DefV) -> bool {
    match v.parent() {
        Some(p) => p.kind().is_type() || p.kind() == DefKind::Impl,
        None => v.parent_name().is_some(),
    }
}

/// `same_family`, for the query layer.
pub fn same_language_family(a: Lang, b: Lang) -> bool {
    same_family(a, b)
}

/// JavaScript and the TypeScript dialects import each other freely; every other language keeps
/// to itself.
fn same_family(a: Lang, b: Lang) -> bool {
    let js = |l: Lang| matches!(l, Lang::JavaScript | Lang::TypeScript | Lang::Tsx);
    a == b || (js(a) && js(b))
}

/// Declarations visible everywhere without an import (TypeScript ambient declarations).
fn ambient(lang: Lang, def_path: &str) -> bool {
    matches!(lang, Lang::JavaScript | Lang::TypeScript | Lang::Tsx) && def_path.ends_with(".d.ts")
}

/// Is the language linked through imports and packages (as opposed to includes)?
pub fn scoped_language(lang: Lang) -> bool {
    module_language(lang)
}

/// Languages where a bare name must be defined in the file, imported, or (for package
/// languages) defined in the same package: anything else is a builtin or an external symbol.
fn strict_scoping(lang: Lang) -> bool {
    matches!(
        lang,
        Lang::Python
            | Lang::JavaScript
            | Lang::TypeScript
            | Lang::Tsx
            | Lang::Go
            | Lang::Rust
            | Lang::Java
            | Lang::Kotlin
            | Lang::Scala
            | Lang::Php
            | Lang::CSharp
    )
}

/// Can the definition have supertypes of its own: a type, an impl block, or a Ruby module
/// (which includes other modules)?
fn has_supers(ix: &Index, v: &DefV) -> bool {
    v.kind().is_type() || v.kind() == DefKind::Impl || (v.kind() == DefKind::Module && ix.lang(v.file()) == Lang::Ruby)
}

/// A type, or a Ruby module: Ruby mixes modules into classes, so they own members the way
/// types do.
fn type_like(ix: &Index, v: &DefV) -> bool {
    v.kind().is_type() || (v.kind() == DefKind::Module && ix.lang(v.file()) == Lang::Ruby)
}

/// Is the definition visible in its own file only? A Ruby `private` method is not: `private`
/// means "called without a receiver", from any file that reopens or inherits the class.
fn file_local(ix: &Index, v: &DefV) -> bool {
    v.is_local() && !(v.kind() == DefKind::Method && ix.lang(v.file()) == Lang::Ruby)
}

/// `$A::B::C...` -> `$B::C...`: the path without its root namespace, when the next segment is a
/// constant too. Ruby looks a constant up in the namespaces around it and then at the top level,
/// so when the root is not in the repository (a gem, or a module Rails autoloading makes from a
/// directory) the inner constant is looked up by its own name.
fn ruby_inner_path(path: &[u8]) -> Option<Vec<u8>> {
    let rest = path.strip_prefix(b"$")?;
    let i = rest.windows(2).position(|w| w == b"::")?;
    let tail = &rest[i + 2..];
    if !tail.first().is_some_and(|c| c.is_ascii_uppercase()) {
        return None;
    }
    let mut out = Vec::with_capacity(tail.len() + 1);
    out.push(b'$');
    out.extend_from_slice(tail);
    Some(out)
}

impl<'a> Resolver<'a> {
    pub fn new(ix: &'a Index) -> Self {
        Resolver {
            ix,
            type_paths: RefCell::new(FxHashMap::default()),
            owner_tys: RefCell::new(FxHashMap::default()),
            impls: RefCell::new(FxHashMap::default()),
            supers_ty: RefCell::new(FxHashMap::default()),
            subs_ty: RefCell::new(FxHashMap::default()),
            module_idx: RefCell::new(None),
            mod_files: RefCell::new(FxHashMap::default()),
            exports: RefCell::new(FxHashMap::default()),
            packages: RefCell::new(FxHashMap::default()),
            gomods: RefCell::new(FxHashMap::default()),
            file_names: RefCell::new(FxHashMap::default()),
            pkg_idx: RefCell::new(None),
            pkg_defs: RefCell::new(FxHashMap::default()),
            members_idx: RefCell::new(FxHashMap::default()),
            desc: RefCell::new(FxHashMap::default()),
            impl_maps: RefCell::new(FxHashMap::default()),
            field_binds: RefCell::new(FxHashMap::default()),
            slot_binds: RefCell::new(FxHashMap::default()),
            bound: RefCell::new(FxHashMap::default()),
            importers: RefCell::new(FxHashMap::default()),
            macro_links: RefCell::new(None),
            pastes: RefCell::new(FxHashMap::default()),
            aliases: RefCell::new(FxHashMap::default()),
            canon: RefCell::new(FxHashMap::default()),
            includes: RefCell::new(FxHashMap::default()),
            suffix_files: RefCell::new(FxHashMap::default()),
            owners: RefCell::new(FxHashMap::default()),
            inc_files: RefCell::new(FxHashMap::default()),
            ents: RefCell::new(FxHashMap::default()),
            imports: RefCell::new(FxHashMap::default()),
            supers: RefCell::new(FxHashMap::default()),
            subs: RefCell::new(FxHashMap::default()),
            global_usings: RefCell::new(None),
            ext_recv: RefCell::new(FxHashMap::default()),
            reopen: RefCell::new(FxHashMap::default()),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Include relation
    // -----------------------------------------------------------------------------------------

    /// Include / import targets of a file as written, with leading `./` and `../` removed.
    fn include_targets(&self, f: FileId) -> Rc<Vec<Vec<u8>>> {
        if let Some(hit) = self.includes.borrow().get(&f) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut out: Vec<Vec<u8>> = Vec::new();
        for im in ix.imports_of_file(f) {
            // `using namespace a::b`, `using a::b::c` and namespace aliases are recorded as
            // imports too. They name no file.
            if im.flags & iflag::WILDCARD != 0 || im.name != crate::facts::NONE || im.alias != crate::facts::NONE {
                continue;
            }
            if let Some(mut t) = ix.local_name(f, im.target) {
                if t.windows(2).any(|w| w == b"::") {
                    continue;
                }
                loop {
                    if let Some(r) = t.strip_prefix(b"./") {
                        t = r;
                    } else if let Some(r) = t.strip_prefix(b"../") {
                        t = r;
                    } else {
                        break;
                    }
                }
                if !t.is_empty() {
                    out.push(t.to_vec());
                }
            }
        }
        let rc = Rc::new(out);
        self.includes.borrow_mut().insert(f, rc.clone());
        rc
    }

    /// Files whose path is `target` or ends with `/target`.
    fn files_with_suffix(&self, target: &[u8]) -> Rc<Vec<FileId>> {
        if let Some(hit) = self.suffix_files.borrow().get(target) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut out: Vec<FileId> = Vec::new();
        // The longest alphanumeric run of the file name is one of the path words of every match.
        let base = target.rsplit(|&b| b == b'/').next().unwrap_or(target);
        let mut best: &[u8] = b"";
        let mut start = None;
        for (i, &b) in base.iter().enumerate() {
            if b.is_ascii_alphanumeric() {
                if start.is_none() {
                    start = Some(i);
                }
            } else if let Some(s0) = start.take() {
                if i - s0 > best.len() {
                    best = &base[s0..i];
                }
            }
        }
        if let Some(s0) = start {
            if base.len() - s0 > best.len() {
                best = &base[s0..];
            }
        }
        if best.len() >= 2 {
            let mut key = Vec::with_capacity(best.len() + 1);
            key.push(b'/');
            key.extend(best.iter().map(|b| b.to_ascii_lowercase()));
            if let Some(nid) = ix.name_id(&key) {
                for f in ix.files_with(nid) {
                    let p = ix.path(f).as_bytes();
                    if p == target || (p.len() > target.len() && p.ends_with(target) && p[p.len() - target.len() - 1] == b'/') {
                        out.push(f);
                    }
                }
            }
        }
        let rc = Rc::new(out);
        self.suffix_files.borrow_mut().insert(target.to_vec(), rc.clone());
        rc
    }

    /// The files `f` includes, resolved. An include target names a path suffix; when several
    /// files carry that suffix the one nearest to `f` is meant, then the shallowest (the
    /// conventional include root). Candidates that tie on both are all taken as included.
    fn resolved_includes(&self, f: FileId) -> Rc<Vec<FileId>> {
        if let Some(hit) = self.inc_files.borrow().get(&f) {
            return hit.clone();
        }
        let ix = self.ix;
        let from = ix.path(f);
        let mut out: Vec<FileId> = Vec::new();
        for inc in self.include_targets(f).iter() {
            let cands = self.files_with_suffix(inc);
            if cands.is_empty() {
                continue;
            }
            let key = |c: FileId| {
                let p = ix.path(c);
                (common_prefix_dirs(from, p) as i32, -(p.bytes().filter(|&b| b == b'/').count() as i32))
            };
            let best = cands.iter().map(|&c| key(c)).max().unwrap_or((0, 0));
            for &c in cands.iter() {
                if key(c) == best && !out.contains(&c) {
                    out.push(c);
                }
            }
        }
        let rc = Rc::new(out);
        self.inc_files.borrow_mut().insert(f, rc.clone());
        rc
    }

    /// Does `f` include `header` directly?
    pub fn includes(&self, f: FileId, header: FileId) -> bool {
        self.resolved_includes(f).contains(&header)
    }

    /// Everything `f` includes within `depth` levels.
    fn include_closure(&self, f: FileId, depth: u32) -> FxHashSet<FileId> {
        let mut seen: FxHashSet<FileId> = FxHashSet::default();
        let mut frontier = vec![f];
        for _ in 0..depth {
            let mut next = Vec::new();
            for x in frontier {
                for &i in self.resolved_includes(x).iter() {
                    if seen.insert(i) {
                        next.push(i);
                    }
                }
            }
            if next.is_empty() || seen.len() > 6000 {
                break;
            }
            frontier = next;
        }
        seen
    }

    // -----------------------------------------------------------------------------------------
    // Imports and packages
    // -----------------------------------------------------------------------------------------

    /// The imports of a file.
    pub fn file_imports(&self, f: FileId) -> Rc<Vec<Imp>> {
        if let Some(hit) = self.imports.borrow().get(&f) {
            return hit.clone();
        }
        let mut out = self.own_imports(f);
        if self.ix.lang(f) == Lang::CSharp {
            // `global using` directives hold in every file of the project.
            for i in self.global_usings().iter() {
                if !out.iter().any(|o| o.target == i.target && o.name == i.name && o.alias == i.alias) {
                    out.push(i.clone());
                }
            }
        }
        let rc = Rc::new(out);
        self.imports.borrow_mut().insert(f, rc.clone());
        rc
    }

    /// The `global using` directives of all C# files (recorded with `REEXPORT`).
    fn global_usings(&self) -> Rc<Vec<Imp>> {
        if let Some(hit) = self.global_usings.borrow().as_ref() {
            return hit.clone();
        }
        let ix = self.ix;
        let mut out: Vec<Imp> = Vec::new();
        for f in ix.files() {
            if ix.lang(f) != Lang::CSharp || !ix.imports_of_file(f).any(|im| im.flags & iflag::REEXPORT != 0) {
                continue;
            }
            for i in self.own_imports(f) {
                if i.flags & iflag::REEXPORT != 0 && !out.iter().any(|o| o.target == i.target && o.name == i.name && o.alias == i.alias) {
                    out.push(i);
                }
            }
        }
        let rc = Rc::new(out);
        *self.global_usings.borrow_mut() = Some(rc.clone());
        rc
    }

    /// The imports a file writes itself.
    fn own_imports(&self, f: FileId) -> Vec<Imp> {
        let ix = self.ix;
        ix.imports_of_file(f)
            .filter_map(|im| {
                Some(Imp {
                    target: ix.local_name(f, im.target)?.to_vec(),
                    name: ix.local_name(f, im.name).map(|n| n.to_vec()),
                    alias: ix.local_name(f, im.alias).map(|n| n.to_vec()),
                    flags: im.flags,
                })
            })
            .collect()
    }

    // -----------------------------------------------------------------------------------------
    // Modules and packages
    // -----------------------------------------------------------------------------------------

    /// Files of import-scoped languages keyed by the last component of the module path they
    /// answer to.
    fn module_index(&self) -> Rc<FxHashMap<Vec<u8>, Vec<FileId>>> {
        if let Some(m) = self.module_idx.borrow().as_ref() {
            return m.clone();
        }
        let ix = self.ix;
        let mut map: FxHashMap<Vec<u8>, Vec<FileId>> = FxHashMap::default();
        for f in ix.files() {
            let lang = ix.lang(f);
            if !module_language(lang) {
                continue;
            }
            let path = ix.path(f);
            let key: String = match (lang, self.go_import_path(path)) {
                (Lang::Go, Some(ip)) => linkx::last_segment(&ip).to_string(),
                _ => linkx::module_key(lang, path).to_string(),
            };
            if !key.is_empty() {
                map.entry(key.into_bytes()).or_default().push(f);
            }
        }
        let rc = Rc::new(map);
        *self.module_idx.borrow_mut() = Some(rc.clone());
        rc
    }

    /// The Go module that governs directory `dir`: (module root directory, module path), read
    /// from the nearest `go.mod`.
    fn go_module(&self, dir: &str) -> Option<Rc<(String, String)>> {
        if let Some(hit) = self.gomods.borrow().get(dir) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut out = None;
        let mut d = dir;
        loop {
            let gm = if d.is_empty() { "go.mod".to_string() } else { format!("{d}/go.mod") };
            if let Some(src) = ix.file_by_path(&gm).and_then(|f| ix.source(f)) {
                let text = String::from_utf8_lossy(&src);
                let module = text.lines().find_map(|l| l.trim().strip_prefix("module ").map(|m| m.trim().trim_matches('"').to_string()));
                if let Some(m) = module.filter(|m| !m.is_empty()) {
                    out = Some(Rc::new((d.to_string(), m)));
                }
                break;
            }
            match d.rfind('/') {
                Some(i) => d = &d[..i],
                None if !d.is_empty() => d = "",
                None => break,
            }
        }
        self.gomods.borrow_mut().insert(dir.to_string(), out.clone());
        out
    }

    /// The import path of the Go package a file belongs to, when a `go.mod` (or a `vendor`
    /// directory) tells: the module path plus the directory below the module root.
    fn go_import_path(&self, def_path: &str) -> Option<String> {
        let dir = match def_path.rfind('/') {
            Some(i) => &def_path[..i],
            None => "",
        };
        // A vendored copy answers to the path below `vendor/`.
        if let Some(rest) = dir.strip_prefix("vendor/") {
            return Some(rest.to_string());
        }
        if let Some(i) = dir.rfind("/vendor/") {
            return Some(dir[i + 8..].to_string());
        }
        let m = self.go_module(dir)?;
        let rel = dir.strip_prefix(m.0.as_str()).unwrap_or(dir).trim_start_matches('/');
        Some(if rel.is_empty() { m.1.clone() } else { format!("{}/{}", m.1, rel) })
    }

    /// Does the file at `def_path` belong to module `target` as written in `from_path`? Path
    /// arithmetic, made exact for Go by the repository's `go.mod` files.
    fn mfit(&self, lang: Lang, from_path: &str, target: &str, def_path: &str) -> Fit {
        if lang == Lang::Go {
            if let Some(ip) = self.go_import_path(def_path) {
                return if ip == target { Fit::Exact } else { Fit::No };
            }
        }
        linkx::module_match(lang, from_path, target, def_path)
    }

    /// The files that are module `target` as written in file `from` (a Go package is all the
    /// files of its directory). Empty when the module is not part of this repository.
    pub fn module_files(&self, from: FileId, target: &[u8]) -> Rc<Vec<FileId>> {
        let key = (from, target.to_vec());
        if let Some(hit) = self.mod_files.borrow().get(&key) {
            return hit.clone();
        }
        let ix = self.ix;
        let lang = ix.lang(from);
        let from_path = ix.path(from);
        let mut out: Vec<FileId> = Vec::new();
        if lang == Lang::CSharp {
            out = self.namespace_files(from, target);
        } else if let Some(t) = std::str::from_utf8(target).ok().filter(|t| !t.is_empty()) {
            if lang == Lang::Php {
                // A PHP import names a namespace, which is not tied to a path: the files that
                // declare it. An include names a file.
                out = self.php_module_files(from, t);
            }
            if lang == Lang::Rust {
                // `crate`, `self`, `super`: named by position, not by a path to match.
                for p in linkx::rust_anchor_files(from_path, t) {
                    if let Some(f) = ix.file_by_path(&p) {
                        if !out.contains(&f) {
                            out.push(f);
                        }
                    }
                }
            }
            if let Some(k) = linkx::target_key(lang, from_path, t) {
                if let Some(cands) = self.module_index().get(k.as_bytes()) {
                    for &f in cands {
                        if same_family(ix.lang(f), lang) && self.mfit(lang, from_path, t, ix.path(f)) == Fit::Exact {
                            out.push(f);
                        }
                    }
                }
            }
        }
        let rc = Rc::new(out);
        self.mod_files.borrow_mut().insert(key, rc.clone());
        rc
    }

    /// The files a PHP import target names: the files that declare namespace `target` (`\` is
    /// the global namespace), or for an include (`lib/x.php`, `./x.php`) the file at that path
    /// next to the including file, else the one file whose path ends with it.
    fn php_module_files(&self, from: FileId, target: &str) -> Vec<FileId> {
        let ix = self.ix;
        if target.ends_with(".php") || target.contains('/') {
            let from_path = ix.path(from);
            let dir = from_path.rfind('/').map_or("", |i| &from_path[..i]);
            if let Some(f) = ix.file_by_path(&linkx::norm_join(dir, target)) {
                return vec![f];
            }
            let t = target.trim_start_matches("./").trim_start_matches('/');
            let cands = self.files_with_suffix(t.as_bytes());
            return if cands.len() == 1 { cands.to_vec() } else { Vec::new() };
        }
        let ns = if target == "\\" { "" } else { target.trim_start_matches('\\') };
        match self.package_index().get(&format!("p:{ns}")) {
            Some(files) => files.iter().copied().filter(|&f| ix.lang(f) == Lang::Php).collect(),
            None => Vec::new(),
        }
    }

    /// Functions and constants named `name` in PHP's global namespace, seen from a file of
    /// another namespace.
    fn php_global_defs(&self, from: FileId, name: &[u8], accept: &dyn Fn(&DefV) -> bool) -> Vec<DefId> {
        let ix = self.ix;
        if self.package_of(from).is_none_or(|p| p.is_empty()) {
            return Vec::new();
        }
        let all = ix.defs_named(name);
        if all.len() > 4096 {
            return Vec::new();
        }
        all.into_iter()
            .filter(|&d| {
                let v = ix.def(d);
                matches!(v.kind(), DefKind::Function | DefKind::Constant)
                    && v.parent().is_none()
                    && ix.lang(v.file()) == Lang::Php
                    && self.package_of(v.file()).is_some_and(|p| p.is_empty())
                    && accept(&v)
            })
            .collect()
    }

    /// Definitions named `name` that the module in file `m` provides: its own definitions outside
    /// function bodies, else what it re-exports under that name (followed a few levels).
    fn file_exports(&self, m: FileId, name: &[u8], depth: u32) -> Rc<Vec<DefId>> {
        let key = (m, name.to_vec());
        if let Some(hit) = self.exports.borrow().get(&key) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut out: Vec<DefId> = Vec::new();
        // Module-level names only: a method is not what `module.name` denotes. Where a file is
        // a class (Java), the members of its top-level types count (static imports).
        let class_file = matches!(ix.lang(m), Lang::Java | Lang::Kotlin | Lang::Scala);
        for d in self.defs_in_file(m, name) {
            let v = ix.def(d);
            // An `impl T` block is named after its type. It is not what the name `T` denotes,
            // and must not hide a re-export of the type itself.
            if v.kind() == DefKind::Impl {
                continue;
            }
            let top = match v.parent() {
                None => v.parent_name().is_none(),
                Some(p) => p.kind() == DefKind::Namespace || p.kind() == DefKind::Module || (class_file && p.kind().is_type() && p.parent().is_none()),
            };
            if top {
                out.push(d);
            }
        }
        // C# has no re-exports: a `global using` is visible in the importing files only.
        if out.is_empty() && depth < 5 && ix.lang(m) != Lang::CSharp {
            // Cycles of re-exports end at this placeholder.
            self.exports.borrow_mut().insert(key.clone(), Rc::new(Vec::new()));
            let lang = ix.lang(m);
            // A name a JS / Python module imports can be imported from it again.
            let any_import = matches!(lang, Lang::Python | Lang::JavaScript | Lang::TypeScript | Lang::Tsx);
            for i in self.file_imports(m).iter() {
                let reexport = i.flags & iflag::REEXPORT != 0;
                if i.target.is_empty() {
                    // `export { a as b }`, `export default a`: a name of this very file.
                    if let Some(real) = i.name.as_deref().filter(|_| reexport && i.alias.as_deref() == Some(name)) {
                        for &d in self.file_exports(m, real, depth + 1).iter() {
                            if !out.contains(&d) {
                                out.push(d);
                            }
                        }
                    }
                    continue;
                }
                let real: &[u8] = if i.name.is_none() {
                    // `export * from`; `export * as ns from` binds a namespace, not the names.
                    if i.flags & iflag::WILDCARD == 0 || i.alias.is_some() || !(reexport || lang == Lang::Python) {
                        continue;
                    }
                    name
                } else if i.alias.as_deref().or(i.name.as_deref()) == Some(name) && (reexport || any_import) {
                    i.name.as_deref().unwrap_or(name)
                } else {
                    continue;
                };
                for &f2 in self.module_files(m, &i.target).iter() {
                    if f2 == m {
                        continue;
                    }
                    for &d in self.file_exports(f2, real, depth + 1).iter() {
                        if !out.contains(&d) {
                            out.push(d);
                        }
                    }
                }
            }
        }
        let rc = Rc::new(out);
        self.exports.borrow_mut().insert(key, rc.clone());
        rc
    }

    /// Members called `name` of the object a module exports by default. Which object that is
    /// is not recorded, so the convention is used: the top-level type, namespace or object
    /// named after the file (`severity.ts` exports `Severity`).
    fn default_members(&self, m: FileId, name: &[u8]) -> Rc<Vec<DefId>> {
        let mut key_name = b"\0default\0".to_vec();
        key_name.extend_from_slice(name);
        let key = (m, key_name);
        if let Some(hit) = self.exports.borrow().get(&key) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut out: Vec<DefId> = Vec::new();
        if matches!(ix.lang(m), Lang::JavaScript | Lang::TypeScript | Lang::Tsx) {
            let squash = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect::<String>();
            let stem = squash(linkx::module_key(ix.lang(m), ix.path(m)));
            let tops: Vec<DefId> =
                ix.defs_of_file(m).filter(|d| d.parent().is_none() && d.parent_name().is_none() && squash(&d.name_str()) == stem).map(|d| d.id).collect();
            if !tops.is_empty() {
                for d in self.defs_in_file(m, name) {
                    if ix.def(d).parent().is_some_and(|p| tops.contains(&p.id)) {
                        out.push(d);
                    }
                }
            }
        }
        let rc = Rc::new(out);
        self.exports.borrow_mut().insert(key, rc.clone());
        rc
    }

    fn exports_of(&self, files: &[FileId], name: &[u8]) -> Vec<DefId> {
        let mut out: Vec<DefId> = Vec::new();
        for &f in files {
            for &d in self.file_exports(f, name, 0).iter() {
                if !out.contains(&d) {
                    out.push(d);
                }
            }
        }
        out
    }

    /// Files that import module file `m`. An import spells the module's name, so only files
    /// that mention it are looked at.
    pub fn importers_of(&self, m: FileId) -> Rc<Vec<FileId>> {
        if let Some(hit) = self.importers.borrow().get(&m) {
            return hit.clone();
        }
        let ix = self.ix;
        let lang = ix.lang(m);
        let mut out: Vec<FileId> = Vec::new();
        if let Some(id) = ix.name_id(linkx::module_key(lang, ix.path(m)).as_bytes()) {
            for f in ix.files_with(id) {
                if f == m || !same_family(ix.lang(f), lang) || out.contains(&f) {
                    continue;
                }
                if self.file_imports(f).iter().any(|i| !i.target.is_empty() && self.module_files(f, &i.target).contains(&m)) {
                    out.push(f);
                }
            }
        }
        let rc = Rc::new(out);
        self.importers.borrow_mut().insert(m, rc.clone());
        rc
    }

    /// What `name` denotes when written after qualifier `q` in file `from`, where `q` is a
    /// module (an import target, a module path), possibly followed by one attribute: a namespace
    /// the module re-exports, or a class or object it defines. `None` when `q` is not a module of
    /// this repository; an empty list when it is one and has no such name.
    pub fn qualified_exports(&self, from: FileId, q: &[u8], name: &[u8]) -> Option<Vec<DefId>> {
        let ix = self.ix;
        let files = self.module_files(from, q);
        if files.is_empty() && ix.lang(from) == Lang::CSharp && !q.is_empty() {
            // A type written before a nested type (`Outer.Inner`, `N.Outer.Inner`): its members.
            let mut out: Vec<DefId> = Vec::new();
            let mut tys = self.type_path_from(q, Some(from)).to_vec();
            if tys.is_empty()
                && let Some(i) = q.iter().rposition(|&b| b == b'.')
            {
                tys = self.types_named(&q[i + 1..], Some(&q[..i]), Some(from));
            }
            for t in tys.iter() {
                for d in self.members_of(t, name) {
                    if !out.contains(&d) {
                        out.push(d);
                    }
                }
            }
            return if out.is_empty() { None } else { Some(out) };
        }
        if !files.is_empty() {
            let mut out = self.exports_of(&files, name);
            if out.is_empty() {
                // `import Severity from './severity'` then `Severity.Error`: a member of what
                // the module exports by default.
                for &f in files.iter() {
                    for &d in self.default_members(f, name).iter() {
                        if !out.contains(&d) {
                            out.push(d);
                        }
                    }
                }
            }
            if out.is_empty() && self.file_imports(from).iter().any(|i| i.name.is_none() && i.alias.as_deref() == Some(name) && i.target.as_slice() == q) {
                // `const run = require('./run'); run()`: the binding is the module, and calling
                // it calls what the module exports as a whole.
                out = self.exports_of(&files, b"default");
            }
            return Some(out);
        }
        // A module followed by one attribute: `pkg.mod.obj`, `./index.js.ns`, `a::b::Type`.
        let cut = q.iter().rposition(|&b| b == b'.' || b == b':' || b == b'/')?;
        let attr = &q[cut + 1..];
        let mut end = cut;
        while end > 0 && q[end - 1] == b':' {
            end -= 1;
        }
        if attr.is_empty() || end == 0 {
            return None;
        }
        let base = self.module_files(from, &q[..end]);
        if base.is_empty() {
            return None;
        }
        let mut out: Vec<DefId> = Vec::new();
        let mut known = false;
        for &m in base.iter() {
            for i in self.file_imports(m).iter() {
                if i.name.is_none() && i.alias.as_deref() == Some(attr) {
                    known = true;
                    for d in self.exports_of(&self.module_files(m, &i.target), name) {
                        if !out.contains(&d) {
                            out.push(d);
                        }
                    }
                }
            }
            for &c in self.file_exports(m, attr, 0).iter() {
                known = true;
                let cv = ix.def(c);
                // Members declared inside the class, namespace or object.
                for d in ix.defs_named(name) {
                    if ix.def(d).parent().is_some_and(|p| p.id == c) && !out.contains(&d) {
                        out.push(d);
                    }
                }
                // A variable with a declared type: members of that type.
                if matches!(cv.kind(), DefKind::Variable | DefKind::Constant) {
                    if let Some(t) = cv.type_name() {
                        let (tm, tn) = split_module(t);
                        for ty in self.types_named(tn, tm, Some(cv.file())) {
                            for d in self.members_of(&ty, name) {
                                if !out.contains(&d) {
                                    out.push(d);
                                }
                            }
                        }
                    }
                }
            }
        }
        if known { Some(out) } else { None }
    }

    /// `ns.name` where `ns` is a namespace bound by a named import (`import { util } from './index'`
    /// with `export * as util from './util'` there): the definitions `name` denotes. `None` when
    /// the receiver is not such a namespace.
    pub fn namespace_member(&self, r: &RefV) -> Option<Vec<DefId>> {
        if !r.kind().is_member() || !module_language(self.ix.lang(r.file)) {
            return None;
        }
        let (module, rest) = split_module(r.qual()?);
        let var = rest.strip_prefix(b"$")?;
        if var.iter().any(|&b| b == b':' || b == b'|' || b == b'(') {
            return None;
        }
        if let Some(m) = module {
            // A namespace that module `m` re-exports: `core.util.f()`.
            let files = self.module_namespace(r.file, m, var);
            return if files.is_empty() { None } else { Some(self.exports_of(&files, r.name())) };
        }
        let mut out: Vec<DefId> = Vec::new();
        let mut known = false;
        for i in self.file_imports(r.file).iter() {
            let Some(real) = i.name.as_deref() else {
                // `const z = { ...nsA, ...nsB }`: each namespace is bound under the name `z`.
                if i.alias.as_deref() == Some(var) && !i.target.is_empty() {
                    let files = self.module_files(r.file, &i.target);
                    if !files.is_empty() {
                        known = true;
                        for d in self.exports_of(&files, r.name()) {
                            if !out.contains(&d) {
                                out.push(d);
                            }
                        }
                    }
                }
                continue;
            };
            if i.alias.as_deref().unwrap_or(real) != var {
                continue;
            }
            for &m in self.module_files(r.file, &i.target).iter() {
                self.namespace_exports(m, real, r.name(), 0, &mut out, &mut known);
            }
        }
        if known { Some(out) } else { None }
    }

    fn namespace_exports(&self, m: FileId, ns: &[u8], name: &[u8], depth: u32, out: &mut Vec<DefId>, known: &mut bool) {
        for i in self.file_imports(m).iter() {
            if i.name.is_none() && i.alias.as_deref() == Some(ns) {
                // `export * as ns from './x'`, or `import * as ns from './x'` exported by name.
                *known = true;
                for d in self.exports_of(&self.module_files(m, &i.target), name) {
                    if !out.contains(&d) {
                        out.push(d);
                    }
                }
            } else if depth < 3 {
                // The namespace itself is re-exported from further away.
                let wildcard = i.name.is_none() && i.alias.is_none() && i.flags & iflag::WILDCARD != 0;
                let named = i.name.is_some() && i.alias.as_deref().or(i.name.as_deref()) == Some(ns);
                if wildcard || named {
                    let real = i.name.as_deref().unwrap_or(ns);
                    for &m2 in self.module_files(m, &i.target).iter() {
                        if m2 != m {
                            self.namespace_exports(m2, real, name, depth + 1, out, known);
                        }
                    }
                }
            }
        }
    }

    /// Definitions named `name` in file `f`, from a per-file table built on first use.
    fn defs_in_file(&self, f: FileId, name: &[u8]) -> Vec<DefId> {
        if let Some(m) = self.file_names.borrow().get(&f) {
            return m.get(name).cloned().unwrap_or_default();
        }
        let mut map: FxHashMap<Vec<u8>, Vec<DefId>> = FxHashMap::default();
        for d in self.ix.defs_of_file(f) {
            map.entry(d.name().to_vec()).or_default().push(d.id);
        }
        let out = map.get(name).cloned().unwrap_or_default();
        self.file_names.borrow_mut().insert(f, Rc::new(map));
        out
    }

    /// The namespace a file shares with others: its declared package, else its directory.
    /// `None` in languages where every file is a namespace of its own.
    fn package_key(&self, f: FileId) -> Option<String> {
        let ix = self.ix;
        if ix.lang(f) == Lang::CSharp {
            // A C# namespace spans directories and says nothing about them.
            let ns = self.package_of(f);
            return Some(format!("p:{}", ns.as_deref().map(|n| String::from_utf8_lossy(n)).unwrap_or_default()));
        }
        if !linkx::dir_is_package(ix.lang(f)) {
            return None;
        }
        match self.package_of(f) {
            Some(p) => Some(format!("p:{}", String::from_utf8_lossy(&p))),
            None => {
                let path = ix.path(f);
                Some(format!("d:{}", path.rfind('/').map(|i| &path[..i]).unwrap_or("")))
            }
        }
    }

    /// Files of package languages by namespace.
    fn package_index(&self) -> Rc<FxHashMap<String, Vec<FileId>>> {
        if let Some(m) = self.pkg_idx.borrow().as_ref() {
            return m.clone();
        }
        let ix = self.ix;
        let mut map: FxHashMap<String, Vec<FileId>> = FxHashMap::default();
        for f in ix.files() {
            if let Some(k) = self.package_key(f) {
                map.entry(k).or_default().push(f);
            }
        }
        let rc = Rc::new(map);
        *self.pkg_idx.borrow_mut() = Some(rc.clone());
        rc
    }

    /// Definitions named `name` in the other files of the package of `from`.
    fn package_defs(&self, from: FileId, name: &[u8]) -> Vec<DefId> {
        let Some(key) = self.package_key(from) else { return Vec::new() };
        if self.ix.lang(from) == Lang::CSharp {
            // The namespace of the file, then the namespaces around it, nearest first.
            let mut k = key;
            loop {
                let hit = self.package_defs_in(from, k.clone(), name);
                if !hit.is_empty() || k == "p:" {
                    return hit;
                }
                k = match k.rfind('.') {
                    Some(i) => k[..i].to_string(),
                    None => "p:".to_string(),
                };
            }
        }
        self.package_defs_in(from, key, name)
    }

    /// Definitions named `name` in the files of the package `key`, other than `from`.
    fn package_defs_in(&self, from: FileId, key: String, name: &[u8]) -> Vec<DefId> {
        let ck = (key, name.to_vec());
        let all: Rc<Vec<DefId>> = match self.pkg_defs.borrow().get(&ck) {
            Some(hit) => hit.clone(),
            None => Rc::new(Vec::new()),
        };
        let all = if all.is_empty() && !self.pkg_defs.borrow().contains_key(&ck) {
            let lang = self.ix.lang(from);
            let mut v: Vec<DefId> = Vec::new();
            if let Some(files) = self.package_index().get(&ck.0) {
                for &f in files {
                    if same_family(lang, self.ix.lang(f)) {
                        v.extend(self.defs_in_file(f, name));
                    }
                }
            }
            let rc = Rc::new(v);
            self.pkg_defs.borrow_mut().insert(ck, rc.clone());
            rc
        } else {
            all
        };
        all.iter().copied().filter(|&d| self.ix.def(d).file() != from).collect()
    }

    /// The C# files of namespace `target` as written in file `from`: the namespace of that
    /// name, else one below a namespace around `from` (`Linq` inside `namespace Newtonsoft.Json`
    /// is `Newtonsoft.Json.Linq`).
    fn namespace_files(&self, from: FileId, target: &[u8]) -> Vec<FileId> {
        let t = String::from_utf8_lossy(target.strip_prefix(b"global::").unwrap_or(target)).into_owned();
        if t.is_empty() {
            return Vec::new();
        }
        let idx = self.package_index();
        let pick =
            |k: &str| -> Vec<FileId> { idx.get(k).map(|v| v.iter().copied().filter(|&f| self.ix.lang(f) == Lang::CSharp).collect()).unwrap_or_default() };
        let out = pick(&format!("p:{t}"));
        if !out.is_empty() {
            return out;
        }
        let Some(own) = self.package_of(from) else { return out };
        let mut ns = String::from_utf8_lossy(&own).into_owned();
        while !ns.is_empty() {
            let hit = pick(&format!("p:{ns}.{t}"));
            if !hit.is_empty() {
                return hit;
            }
            ns = match ns.rfind('.') {
                Some(i) => ns[..i].to_string(),
                None => String::new(),
            };
        }
        out
    }

    /// The package a file declares (Java `package a.b;`), when the language records one.
    fn package_of(&self, f: FileId) -> Option<Rc<Vec<u8>>> {
        if let Some(hit) = self.packages.borrow().get(&f) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut out = None;
        if ix.lang(f) == Lang::CSharp {
            // The innermost of the namespaces the file opens before its first other
            // definition; the global namespace when it opens none.
            let mut ns: &[u8] = b"";
            for v in ix.defs_of_file(f) {
                if v.kind() != DefKind::Namespace {
                    break;
                }
                ns = v.name();
            }
            out = Some(Rc::new(ns.to_vec()));
        } else if matches!(ix.lang(f), Lang::Java | Lang::Kotlin | Lang::Scala | Lang::Php) {
            if let Some(v) = ix.defs_of_file(f).next() {
                if v.kind() == DefKind::Namespace && v.parent().is_none() {
                    out = Some(Rc::new(v.name().to_vec()));
                }
            }
            // A PHP file without a namespace declaration is in the global namespace, which all
            // such files share.
            if out.is_none() && ix.lang(f) == Lang::Php {
                out = Some(Rc::new(Vec::new()));
            }
        }
        self.packages.borrow_mut().insert(f, out.clone());
        out
    }

    /// Do two files share a package namespace (unqualified names resolve across them)?
    pub fn same_package(&self, a: FileId, b: FileId) -> bool {
        let ix = self.ix;
        let lang = ix.lang(a);
        if lang == Lang::CSharp && ix.lang(b) == Lang::CSharp {
            // The names of a namespace and of the namespaces around it are visible inside it.
            let (Some(x), Some(y)) = (self.package_of(a), self.package_of(b)) else { return false };
            return y.is_empty() || x == y || (x.len() > y.len() && x.starts_with(&y) && x[y.len()] == b'.');
        }
        if !linkx::dir_is_package(lang) || !same_family(lang, ix.lang(b)) {
            return false;
        }
        // A PHP namespace is what the file declares, wherever the file is.
        if lang != Lang::Php && linkx::same_dir(ix.path(a), ix.path(b)) {
            return true;
        }
        match (self.package_of(a), self.package_of(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        }
    }

    /// Evidence from the imports and the package of `from` that the unqualified name `name`
    /// written there denotes definition `v` (of another file): imported by name from the module
    /// that provides it (4000), from a package it lives in (3400), same package (3500), reachable
    /// through a wildcard import (2500), or nothing (0).
    fn import_evidence(&self, from: FileId, name: &[u8], v: &DefV) -> i64 {
        let ix = self.ix;
        let lang = ix.lang(from);
        let (from_path, def_path) = (ix.path(from), v.path());
        let fit = |t: &[u8]| self.mfit(lang, from_path, std::str::from_utf8(t).unwrap_or(""), def_path);
        let mut best = 0i64;
        for i in self.file_imports(from).iter() {
            if let Some(real) = i.name.as_deref() {
                if i.alias.as_deref().unwrap_or(real) == name && real == v.name() {
                    let files = self.module_files(from, &i.target);
                    if !files.is_empty() {
                        if self.exports_of(&files, real).contains(&v.id) {
                            best = best.max(4000);
                        }
                    } else {
                        match fit(&i.target) {
                            Fit::Exact => best = best.max(4000),
                            Fit::Inside => best = best.max(3400),
                            Fit::No => {}
                        }
                    }
                }
            } else if i.flags & iflag::WILDCARD != 0 && v.name() == name {
                let files = self.module_files(from, &i.target);
                let hit = if files.is_empty() { fit(&i.target) != Fit::No } else { self.exports_of(&files, name).contains(&v.id) };
                if hit {
                    best = best.max(2500);
                }
            }
        }
        if best == 0 && v.name() == name && self.same_package(from, v.file()) {
            best = 3500;
        }
        best
    }

    /// Evidence that a reference written in `from` as `name` (optionally qualified by `qual`)
    /// denotes definition `v`, from the file's imports and package. Positive: the import or the
    /// qualifier leads to the definition's module or owner. Negative: the reference is
    /// qualified, and the qualifier leads elsewhere. Zero: no evidence either way.
    pub fn link_bonus(&self, from: FileId, name: &[u8], qual: Option<&[u8]>, v: &DefV) -> i64 {
        let ix = self.ix;
        let lang = ix.lang(from);
        if !module_language(lang) {
            return 0;
        }
        let same_file = v.file() == from;
        let Some(q) = qual else {
            return if same_file { 0 } else { self.import_evidence(from, name, v) };
        };
        // The qualifier is the definition's owner type (static call, implicit `this`).
        if v.parent_name().is_some() {
            if let Some(owner) = self.owner_ty(v) {
                let (strict_up, path) = match q.strip_prefix(b"^") {
                    Some(rest) => (true, rest),
                    None => (false, q),
                };
                for t in self.qualifier_types(from, path).iter() {
                    if (!strict_up && t.same(&owner)) || self.ancestors(t).iter().any(|a| a.same(&owner)) {
                        return 4500;
                    }
                }
            }
        }
        // The qualifier is a module that provides the definition.
        if let Some(list) = self.qualified_exports(from, q, name) {
            return if list.contains(&v.id) { 4500 } else { -3000 };
        }
        if same_file {
            return -3000;
        }
        // The module is not a file we can name: fall back to where the definition lives.
        let (from_path, def_path) = (ix.path(from), v.path());
        let fit = |t: &[u8]| self.mfit(lang, from_path, std::str::from_utf8(t).unwrap_or(""), def_path);
        if v.parent_name().is_none() || v.parent().is_some_and(|p| matches!(p.kind(), DefKind::Module | DefKind::Namespace)) {
            if fit(q) != Fit::No {
                return 4500;
            }
            // The qualifier is a local alias of an import.
            let sep: &[u8] = match lang {
                Lang::Rust => b"::",
                Lang::Go | Lang::JavaScript | Lang::TypeScript | Lang::Tsx => b"/",
                _ => b".",
            };
            for i in self.file_imports(from).iter() {
                let default_alias = linkx::last_segment(std::str::from_utf8(&i.target).unwrap_or(""));
                let local: &[u8] = i.alias.as_deref().or(i.name.as_deref()).unwrap_or(default_alias.as_bytes());
                if local.is_empty() || !q.starts_with(local) {
                    continue;
                }
                let rest = &q[local.len()..];
                if !(rest.is_empty() || rest.starts_with(b".") || rest.starts_with(b"::") || rest.starts_with(b"/")) {
                    continue;
                }
                let mut expanded = i.target.clone();
                if let Some(n) = &i.name {
                    expanded.extend_from_slice(sep);
                    expanded.extend_from_slice(n);
                }
                expanded.extend_from_slice(rest);
                if fit(&expanded) != Fit::No || fit(&i.target) == Fit::Exact {
                    return 4500;
                }
            }
        }
        -3000
    }

    // -----------------------------------------------------------------------------------------
    // Type hierarchy
    // -----------------------------------------------------------------------------------------

    /// Direct supertypes, by name, of every type (or impl block) named `name`.
    fn supertype_names(&self, name: &[u8]) -> Rc<Vec<Vec<u8>>> {
        if let Some(hit) = self.supers.borrow().get(name) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut out: Vec<Vec<u8>> = Vec::new();
        for d in ix.defs_named(name) {
            let v = ix.def(d);
            if !has_supers(ix, &v) {
                continue;
            }
            for r in ix.refs_of_file(v.file()) {
                if matches!(r.kind(), RefKind::Extends | RefKind::Implements) && r.encl().is_some_and(|e| e.id == d) {
                    let n = r.name().to_vec();
                    if n != name && !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
        }
        let rc = Rc::new(out);
        self.supers.borrow_mut().insert(name.to_vec(), rc.clone());
        rc
    }

    /// Direct subtypes by name: types that extend or implement something called `name`.
    fn subtype_names(&self, name: &[u8]) -> Rc<Vec<Vec<u8>>> {
        if let Some(hit) = self.subs.borrow().get(name) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut out: Vec<Vec<u8>> = Vec::new();
        if let Some(nid) = ix.name_id(name) {
            ix.for_each_ref_named(nid, |r| {
                if matches!(r.kind(), RefKind::Extends | RefKind::Implements) && out.len() < 400 {
                    if let Some(e) = r.encl() {
                        let n = e.name().to_vec();
                        if n != name && !out.contains(&n) {
                            out.push(n);
                        }
                    }
                }
            });
        }
        let rc = Rc::new(out);
        self.subs.borrow_mut().insert(name.to_vec(), rc.clone());
        rc
    }

    /// The types a type-like reference (`extends B`, `: T`, `new T`) names, from where it is written.
    pub fn types_of_ref(&self, r: &RefV) -> Vec<Ty> {
        if self.ix.lang(r.file) == Lang::Ruby {
            return self.ruby_const_types(r.name(), r.qual(), r.file);
        }
        self.types_named(r.name(), r.qual(), Some(r.file))
    }

    /// The classes and modules a Ruby constant `name` written in namespace `ns` (`A::B` is `B`
    /// in `A`) stands for: the constant path walked as a value, which reaches classes, modules
    /// and constants that alias them.
    fn ruby_const_types(&self, name: &[u8], ns: Option<&[u8]>, from: FileId) -> Vec<Ty> {
        let ix = self.ix;
        let mut p: Vec<u8> = Vec::with_capacity(name.len() + 16);
        p.push(b'$');
        if let Some(q) = ns {
            p.extend_from_slice(q);
            p.extend_from_slice(b"::");
        }
        p.extend_from_slice(name);
        self.type_path_from(&p, Some(from)).iter().filter(|t| t.defs.iter().any(|&d| type_like(ix, &ix.def(d)))).cloned().collect()
    }

    fn supers_of_def(&self, d: DefId) -> Rc<Vec<Ty>> {
        if let Some(hit) = self.supers_ty.borrow().get(&d) {
            return hit.clone();
        }
        let ix = self.ix;
        let v = ix.def(d);
        let mut out: Vec<Ty> = Vec::new();
        if has_supers(ix, &v) {
            for r in ix.refs_of_file(v.file()) {
                if matches!(r.kind(), RefKind::Extends | RefKind::Implements) && r.encl().is_some_and(|e| e.id == d) {
                    for t in self.types_of_ref(&r) {
                        if !t.defs.contains(&d) {
                            push_ty(&mut out, t);
                        }
                    }
                }
            }
        }
        let rc = Rc::new(out);
        self.supers_ty.borrow_mut().insert(d, rc.clone());
        rc
    }

    /// Direct supertypes of a type.
    pub fn supertypes(&self, ty: &Ty) -> Vec<Ty> {
        if ty.defs.is_empty() {
            return self.supertype_names(&ty.name).iter().map(|n| Ty::named(n)).collect();
        }
        let mut out: Vec<Ty> = Vec::new();
        for &d in &ty.defs {
            for t in self.supers_of_def(d).iter() {
                push_ty(&mut out, t.clone());
            }
        }
        out
    }

    /// Direct subtypes: types that extend or implement `ty`.
    pub fn subtypes(&self, ty: &Ty) -> Rc<Vec<Ty>> {
        if let Some(hit) = self.subs_ty.borrow().get(ty) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut out: Vec<Ty> = Vec::new();
        if ty.defs.is_empty() {
            out = self.subtype_names(&ty.name).iter().map(|n| Ty::named(n)).collect();
        } else if let Some(nid) = ix.name_id(&ty.name) {
            let mut sites: Vec<RefV> = Vec::new();
            ix.for_each_ref_named(nid, |r| {
                if matches!(r.kind(), RefKind::Extends | RefKind::Implements) && sites.len() < 2000 {
                    sites.push(r);
                }
            });
            for r in sites {
                let Some(e) = r.encl() else { continue };
                if ty.defs.contains(&e.id) || !self.types_of_ref(&r).iter().any(|t| t.same(ty)) {
                    continue;
                }
                if out.len() < 400 {
                    push_ty(&mut out, self.ty_of_def(&e));
                }
            }
            for t in self.structural_impls(ty) {
                push_ty(&mut out, t);
            }
        }
        let rc = Rc::new(out);
        self.subs_ty.borrow_mut().insert(ty.clone(), rc.clone());
        rc
    }

    fn is_go_iface(&self, d: DefId) -> bool {
        let v = self.ix.def(d);
        v.kind() == DefKind::Interface && self.ix.lang(v.file()) == Lang::Go
    }

    /// The method names a Go interface asks for: its own and those of the interfaces it embeds.
    fn iface_methods(&self, iface: DefId, depth: u32) -> Vec<(Vec<u8>, DefId)> {
        let ix = self.ix;
        let v = ix.def(iface);
        let mut out: Vec<(Vec<u8>, DefId)> = Vec::new();
        for d in ix.defs_of_file(v.file()) {
            if d.kind() == DefKind::Method && d.parent().is_some_and(|p| p.id == iface) && !out.iter().any(|(m, _)| m.as_slice() == d.name()) {
                out.push((d.name().to_vec(), d.id));
            }
        }
        if depth < 3 {
            for t in self.supers_of_def(iface).iter() {
                for &sd in &t.defs {
                    if self.is_go_iface(sd) {
                        for m in self.iface_methods(sd, depth + 1) {
                            if !out.iter().any(|(n, _)| *n == m.0) {
                                out.push(m);
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// Does type `ty` have a method that could implement interface method `want`: the same
    /// name, the same number of parameters, and a result exactly when the interface has one?
    fn implements_method(&self, ty: &Ty, name: &[u8], want: DefId) -> bool {
        let ix = self.ix;
        let w = ix.def(want);
        self.members_of(ty, name).into_iter().any(|m| {
            let v = ix.def(m);
            v.rec.arity_min == w.rec.arity_min && v.rec.arity_max == w.rec.arity_max && v.type_name().is_some() == w.type_name().is_some()
        })
    }

    /// Go has no `implements`: a type satisfies an interface when it has its methods. The types
    /// of this repository whose own methods cover those of the interface, by name.
    fn structural_impls(&self, iface: &Ty) -> Vec<Ty> {
        let ix = self.ix;
        let Some(&id) = iface.defs.iter().find(|&&d| self.is_go_iface(d)) else { return Vec::new() };
        let methods = self.iface_methods(id, 0);
        if methods.is_empty() || methods.len() > 24 {
            return Vec::new();
        }
        // Start from the method the fewest types have.
        let Some((rarest, _)) = methods.iter().min_by_key(|(m, _)| ix.defs_named(m).len()) else { return Vec::new() };
        let cands = ix.defs_named(rarest);
        if cands.len() > 4000 {
            return Vec::new();
        }
        let mut out: Vec<Ty> = Vec::new();
        for d in cands {
            let v = ix.def(d);
            if v.kind() != DefKind::Method || ix.lang(v.file()) != Lang::Go || v.parent().is_some_and(|p| p.kind() == DefKind::Interface) {
                continue;
            }
            let Some(t) = self.owner_ty(&v) else { continue };
            if t.defs.is_empty() || out.contains(&t) {
                continue;
            }
            if methods.iter().all(|(m, md)| self.implements_method(&t, m, *md)) {
                out.push(t);
                if out.len() >= 64 {
                    break;
                }
            }
        }
        out
    }

    /// The Go interfaces declaring a method `name` that type `ty` satisfies: a call of `name`
    /// through one of them may land in `ty`'s method.
    fn structural_ifaces(&self, ty: &Ty, name: &[u8]) -> Vec<Ty> {
        let ix = self.ix;
        let mut out: Vec<Ty> = Vec::new();
        if ty.defs.is_empty() || !ty.defs.iter().any(|&d| ix.lang(ix.def(d).file()) == Lang::Go) {
            return out;
        }
        let cands = ix.defs_named(name);
        if cands.len() > 4000 {
            return out;
        }
        for d in cands {
            let v = ix.def(d);
            let Some(p) = v.parent() else { continue };
            if !self.is_go_iface(p.id) {
                continue;
            }
            let it = self.ty_of_def(&p);
            if out.contains(&it) {
                continue;
            }
            let methods = self.iface_methods(p.id, 0);
            if !methods.is_empty() && methods.len() <= 24 && methods.iter().all(|(m, md)| self.implements_method(ty, m, *md)) {
                out.push(it);
                if out.len() >= 32 {
                    break;
                }
            }
        }
        out
    }

    /// All supertypes, nearest first (not including `ty`).
    pub fn ancestors(&self, ty: &Ty) -> Vec<Ty> {
        let mut out: Vec<Ty> = Vec::new();
        let mut frontier: Vec<Ty> = vec![ty.clone()];
        for _ in 0..8 {
            let mut next = Vec::new();
            for t in &frontier {
                for s in self.supertypes(t) {
                    if !s.same(ty) && !out.iter().any(|o| o == &s) {
                        out.push(s.clone());
                        next.push(s);
                    }
                }
            }
            if next.is_empty() || out.len() > 64 {
                break;
            }
            frontier = next;
        }
        out
    }

    /// All subtypes (not including `ty`).
    pub fn descendants(&self, ty: &Ty) -> Vec<Ty> {
        if let Some(hit) = self.desc.borrow().get(ty) {
            return hit.to_vec();
        }
        let out = self.descendants_uncached(ty);
        self.desc.borrow_mut().insert(ty.clone(), Rc::new(out.clone()));
        out
    }

    fn descendants_uncached(&self, ty: &Ty) -> Vec<Ty> {
        let mut out: Vec<Ty> = Vec::new();
        let mut frontier: Vec<Ty> = vec![ty.clone()];
        for _ in 0..6 {
            let mut next = Vec::new();
            for t in &frontier {
                for s in self.subtypes(t).iter() {
                    if !s.same(ty) && !out.iter().any(|o| o == s) {
                        out.push(s.clone());
                        next.push(s.clone());
                    }
                }
            }
            if next.is_empty() || out.len() > 300 {
                break;
            }
            frontier = next;
        }
        out
    }

    /// Members named `name` declared by type `ty` (fields, methods, properties, nested items).
    pub fn members_of(&self, ty: &Ty, name: &[u8]) -> Vec<DefId> {
        let ix = self.ix;
        let idx = self.members_named(name);
        let Some(defs) = idx.get(ty.name.as_slice()) else { return Vec::new() };
        match ty.defs.first().map(|&d| ix.lang(ix.def(d).file())) {
            None => defs.clone(),
            Some(l) => defs
                .iter()
                .copied()
                .filter(|&d| {
                    let v = ix.def(d);
                    same_family(l, ix.lang(v.file())) && v.parent_name() == Some(ty.name.as_slice()) && self.owner_ty(&v).is_some_and(|o| o.same(ty))
                })
                .collect(),
        }
    }

    /// Every member called `name`, by the name of its container (alias-resolved where types
    /// are global names). Built once per member name.
    fn members_named(&self, name: &[u8]) -> Rc<FxHashMap<Vec<u8>, Vec<DefId>>> {
        if let Some(hit) = self.members_idx.borrow().get(name) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut map: FxHashMap<Vec<u8>, Vec<DefId>> = FxHashMap::default();
        for d in ix.defs_named(name) {
            let v = ix.def(d);
            let Some(p) = v.parent_name() else { continue };
            // A function nested in a function is a local helper, not a member.
            if v.parent().is_some_and(|pd| matches!(pd.kind(), DefKind::Function | DefKind::Method)) {
                continue;
            }
            map.entry(p.to_vec()).or_default().push(d);
            if !module_language(ix.lang(v.file())) {
                let c = self.canon_type(p);
                if c.as_slice() != p {
                    map.entry(c).or_default().push(d);
                }
            }
        }
        let rc = Rc::new(map);
        self.members_idx.borrow_mut().insert(name.to_vec(), rc.clone());
        rc
    }

    /// Methods named `name` declared by type `owner`.
    pub fn methods_of(&self, owner: &Ty, name: &[u8]) -> Vec<DefId> {
        let ix = self.ix;
        self.members_of(owner, name).into_iter().filter(|&d| matches!(ix.def(d).kind(), DefKind::Method | DefKind::Function | DefKind::Property)).collect()
    }

    /// Does type `ty` itself declare a member called `field`?
    pub fn has_member(&self, ty: &Ty, field: &[u8]) -> bool {
        if ty.defs.is_empty() {
            return self.member_owners(field).iter().any(|(o, _)| o.as_slice() == ty.name.as_slice());
        }
        !self.members_of(ty, field).is_empty()
    }

    /// How strongly a reference made in `from` is tied to an entity: same file, a header of the
    /// entity that `from` includes, an import that leads to it, or mere proximity in the tree.
    fn affinity(&self, from: FileId, t: &Target) -> i32 {
        let ix = self.ix;
        let from_path = ix.path(from);
        let mut best = 0i32;
        for &d in &t.defs {
            let v = ix.def(d);
            let f = v.file();
            let a = if f == from {
                1000
            } else if ix.file(f).flags & crate::facts::fflag::HEADER != 0 && self.includes(from, f) {
                500
            } else {
                let link = self.link_bonus(from, &t.name, None, &v);
                if link > 0 { 400 + (link / 10) as i32 } else { (common_prefix_dirs(from_path, v.path()) as i32 * 10).min(200) }
            };
            best = best.max(a);
        }
        best
    }

    /// Split same-named global definitions into linkage domains: a definition belongs with the
    /// headers its file includes. Unrelated programs in one repository (a kernel function and a
    /// test helper of the same name) end up as separate entities.
    fn split_domains(&self, defs: Vec<DefId>) -> Vec<Vec<DefId>> {
        let ix = self.ix;
        let n = defs.len();
        let real = defs.iter().filter(|&&d| !ix.def(d).is_decl()).count();
        if n < 2 || real < 2 || n > 96 {
            return vec![defs];
        }
        let files: Vec<FileId> = defs.iter().map(|&d| ix.def(d).file()).collect();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut Vec<usize>, mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        // A definition belongs with the headers its file reaches within a few include levels
        // (the prototype is rarely included directly by every file that defines the function).
        let closures: Vec<FxHashSet<FileId>> = files.iter().map(|&f| self.include_closure(f, 3)).collect();
        for i in 0..n {
            for j in 0..n {
                if i == j {
                    continue;
                }
                let linked = files[i] == files[j] || (ix.file(files[j]).flags & crate::facts::fflag::HEADER != 0 && closures[i].contains(&files[j]));
                if linked {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    if a != b {
                        parent[a] = b;
                    }
                }
            }
        }
        let mut comps: FxHashMap<usize, Vec<usize>> = FxHashMap::default();
        for i in 0..n {
            let r = find(&mut parent, i);
            comps.entry(r).or_default().push(i);
        }
        let mut with_def: Vec<Vec<usize>> = Vec::new();
        let mut decl_only: Vec<Vec<usize>> = Vec::new();
        for (_, c) in comps {
            if c.iter().any(|&i| !ix.def(defs[i]).is_decl()) { with_def.push(c) } else { decl_only.push(c) }
        }
        if with_def.is_empty() {
            return vec![defs];
        }
        // Prototypes nobody linked: attach them to the nearest definition.
        for c in decl_only {
            let p = ix.path(files[c[0]]);
            let best =
                (0..with_def.len()).max_by_key(|&k| with_def[k].iter().map(|&i| common_prefix_dirs(p, ix.path(files[i]))).max().unwrap_or(0)).unwrap_or(0);
            with_def[best].extend(c);
        }
        with_def.sort_by_key(|c| c.iter().copied().min().unwrap_or(0));
        with_def.into_iter().map(|c| c.into_iter().map(|i| defs[i]).collect()).collect()
    }

    // -----------------------------------------------------------------------------------------
    // Entities
    // -----------------------------------------------------------------------------------------

    /// Group same-named definitions into entities, best first.
    pub fn entities(&self, defs: &[DefId]) -> Vec<Target> {
        let ix = self.ix;
        // Members are grouped by their owning type: by its name where types are global names,
        // by its definitions where scoping tells same-named types of different packages apart.
        let mut groups: FxHashMap<(Class, Option<FileId>, Option<Vec<u8>>, Vec<DefId>), Vec<DefId>> = FxHashMap::default();
        for &d in defs {
            let v = ix.def(d);
            let class = class_of(&v);
            let local = if file_local(ix, &v) { Some(v.file()) } else { None };
            // Methods are members of their container as far as identity goes.
            let member = class == Class::Member
                || (class == Class::Callable && v.kind() == DefKind::Method)
                || (v.kind() == DefKind::EnumMember && module_language(ix.lang(v.file())) && v.parent().is_some_and(|p| p.kind().is_type()));
            let (owner, scope) = match if member { self.owner_ty(&v) } else { None } {
                Some(t) => (Some(if t.defs.is_empty() { v.parent_name().unwrap_or(&t.name).to_vec() } else { t.name }), t.defs),
                None => (None, Vec::new()),
            };
            groups.entry((class, local, owner, scope)).or_default().push(d);
        }
        let mut out: Vec<Target> = Vec::new();
        for ((class, local_file, owner, scope), defs) in groups {
            let name = ix.def(defs[0]).name().to_vec();
            let global = local_file.is_none() && owner.is_none() && matches!(class, Class::Callable | Class::Value | Class::Type);
            if local_file.is_none()
                && owner.is_none()
                && matches!(class, Class::Type | Class::Module)
                && defs.iter().all(|&d| ix.lang(ix.def(d).file()) == Lang::Ruby)
            {
                // A Ruby class or module is one entity across the files that reopen it, and
                // a different one in each namespace.
                let mut parts: Vec<(Option<Vec<u8>>, Vec<DefId>)> = Vec::new();
                for d in defs {
                    let ns = ix.def(d).parent_name().map(|p| p.to_vec());
                    match parts.iter_mut().find(|p| p.0 == ns) {
                        Some(p) => p.1.push(d),
                        None => parts.push((ns, vec![d])),
                    }
                }
                for (_, part) in parts {
                    out.push(Target { name: name.clone(), defs: part, class, local_file, owner: None, scope: Vec::new() });
                }
            } else if global {
                for part in self.split_domains(defs) {
                    out.push(Target { name: name.clone(), defs: part, class, local_file, owner: owner.clone(), scope: Vec::new() });
                }
            } else {
                out.push(Target { name, defs, class, local_file, owner, scope });
            }
        }
        // Rank entities: by their best definition, then by how many definitions they gather.
        let mut scored: Vec<(i64, Target)> = out
            .drain(..)
            .map(|t| {
                let best = t.defs.iter().map(|&d| def_rank(ix, &ix.def(d))).max().unwrap_or(0);
                (best + (t.defs.len() as i64).min(20), t)
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.defs[0].cmp(&b.1.defs[0])));
        scored.into_iter().map(|(_, t)| t).collect()
    }

    // -----------------------------------------------------------------------------------------
    // Type paths
    // -----------------------------------------------------------------------------------------

    /// Follow typedef-style aliases to the name that owns the members.
    pub fn canon_type(&self, name: &[u8]) -> Vec<u8> {
        if let Some(hit) = self.canon.borrow().get(name) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut cur = name.to_vec();
        for _ in 0..6 {
            let defs = ix.defs_named(&cur);
            let mut has_record = false;
            let mut alias_to: Option<Vec<u8>> = None;
            for d in &defs {
                let v = ix.def(*d);
                match v.kind() {
                    DefKind::Struct | DefKind::Union | DefKind::Class | DefKind::Interface | DefKind::Trait | DefKind::Enum => has_record = true,
                    DefKind::TypeAlias => {
                        if let Some(t) = v.type_name() {
                            if t != cur.as_slice() {
                                alias_to = Some(t.to_vec());
                            }
                        }
                    }
                    _ => {}
                }
            }
            match (has_record, alias_to) {
                (false, Some(t)) => cur = t,
                _ => break,
            }
        }
        self.canon.borrow_mut().insert(name.to_vec(), cur.clone());
        cur
    }

    /// Definitions that `name` denotes when written in file `from`. With a module qualifier:
    /// what that module provides. Without: the nearest of defined in the file, imported by name,
    /// defined in the same package, reachable through a wildcard import or ambient.
    fn scoped_defs(&self, name: &[u8], module: Option<&[u8]>, from: FileId, accept: &dyn Fn(&DefV) -> bool) -> Vec<DefId> {
        let ix = self.ix;
        let lang = ix.lang(from);
        if name.is_empty() {
            return Vec::new();
        }
        let _ = lang;
        if let Some(m) = module {
            if let Some(list) = self.qualified_exports(from, m, name) {
                return list.into_iter().filter(|&d| accept(&ix.def(d))).collect();
            }
            let (from_path, ms) = (ix.path(from), std::str::from_utf8(m).unwrap_or(""));
            return ix
                .defs_named(name)
                .into_iter()
                .filter(|&d| {
                    let v = ix.def(d);
                    accept(&v) && same_family(lang, ix.lang(v.file())) && self.mfit(lang, from_path, ms, v.path()) != Fit::No
                })
                .collect();
        }
        self.scoped_defs_by_place(name, from, accept)
    }

    /// The places a file's scope reaches, nearest first: the file itself, the modules its
    /// imports name (re-exports followed, aliases undone), its package, its wildcard imports.
    /// The name is looked up in each place. The first place that has it wins.
    fn scoped_defs_by_place(&self, name: &[u8], from: FileId, accept: &dyn Fn(&DefV) -> bool) -> Vec<DefId> {
        let ix = self.ix;
        let lang = ix.lang(from);
        let from_path = ix.path(from);
        let ok = |d: &DefId| accept(&ix.def(*d));
        let mut here: Vec<DefId> = self.defs_in_file(from, name).into_iter().filter(ok).collect();
        if lang == Lang::Ruby {
            // Ruby imports name files, not symbols: a constant or method is visible everywhere
            // once its file is loaded (Rails autoloading loads them all). The file's own first.
            let all = ix.defs_named(name);
            if all.len() <= 4096 {
                here.extend(all.into_iter().filter(|&d| {
                    let v = ix.def(d);
                    v.file() != from && ix.lang(v.file()) == Lang::Ruby && accept(&v)
                }));
            }
            return here;
        }
        if !here.is_empty() {
            if lang == Lang::CSharp && here.iter().all(|&d| ix.def(d).kind().is_type()) {
                // The other parts of a partial type live in other files of the namespace.
                if let Some(key) = self.package_key(from) {
                    for d in self.package_defs_in(from, key, name) {
                        if ok(&d) && ix.def(d).kind().is_type() && !here.contains(&d) {
                            here.push(d);
                        }
                    }
                }
            }
            return here;
        }
        let imps = self.file_imports(from);
        // The definitions of `real` that the import of `target` reaches.
        let via = |target: &[u8], real: &[u8], inside_ok: bool| -> Vec<DefId> {
            let files = self.module_files(from, target);
            if !files.is_empty() {
                return self.exports_of(&files, real);
            }
            // Not a file we can name (a Java package, an unusual layout): fall back to where
            // the definitions live, for names that are not everywhere.
            let cands = ix.defs_named(real);
            if cands.len() > 4096 {
                return Vec::new();
            }
            let ts = std::str::from_utf8(target).unwrap_or("");
            cands
                .into_iter()
                .filter(|&d| {
                    let v = ix.def(d);
                    v.file() != from && same_family(lang, ix.lang(v.file())) && {
                        let fit = self.mfit(lang, from_path, ts, v.path());
                        fit == Fit::Exact || (inside_ok && fit == Fit::Inside)
                    }
                })
                .collect()
        };
        let mut named: Vec<DefId> = Vec::new();
        for i in imps.iter() {
            let Some(real) = i.name.as_deref() else { continue };
            if i.alias.as_deref().unwrap_or(real) == name {
                let mut hit = via(&i.target, real, true);
                if real == b"default" && !hit.iter().any(ok) {
                    // The module does not say what it exports by default. A default import is
                    // nearly always bound to the name the module itself uses for it.
                    hit = via(&i.target, name, true);
                }
                for d in hit {
                    if ok(&d) && !named.contains(&d) {
                        named.push(d);
                    }
                }
            }
        }
        if !named.is_empty() {
            return named;
        }
        let pkg: Vec<DefId> = self.package_defs(from, name).into_iter().filter(ok).collect();
        if !pkg.is_empty() {
            return pkg;
        }
        let mut wild: Vec<DefId> = Vec::new();
        for i in imps.iter() {
            if i.name.is_none() && i.flags & iflag::WILDCARD != 0 {
                for d in via(&i.target, name, true) {
                    if ok(&d) && !wild.contains(&d) {
                        wild.push(d);
                    }
                }
            }
        }
        if wild.is_empty() && lang == Lang::Php {
            // A function or constant PHP does not find in the current namespace is looked up in
            // the global one. Classes are not.
            wild = self.php_global_defs(from, name, accept);
        }
        if wild.is_empty() && matches!(lang, Lang::JavaScript | Lang::TypeScript | Lang::Tsx) {
            let all = ix.defs_named(name);
            if all.len() <= 4096 {
                wild = all.into_iter().filter(|d| ok(d) && ambient(lang, ix.def(*d).path())).collect();
            }
        }
        wild
    }

    /// `impl` blocks of a type (languages that separate them from the type's declaration).
    fn impls_of(&self, d: DefId) -> Rc<Vec<DefId>> {
        if let Some(hit) = self.impls.borrow().get(&d) {
            return hit.clone();
        }
        let ix = self.ix;
        let v = ix.def(d);
        let mut out: Vec<DefId> = Vec::new();
        if ix.lang(v.file()) == Lang::Rust && v.kind() != DefKind::Impl {
            if let Some(list) = self.impl_map(v.name()).get(&d) {
                out = list.clone();
            }
        }
        let rc = Rc::new(out);
        self.impls.borrow_mut().insert(d, rc.clone());
        rc
    }

    /// Every `impl` block called `name`, by the type definition it implements: the type of
    /// that name in the block's own file, else the one its file can see. Built once per name.
    fn impl_map(&self, name: &[u8]) -> Rc<FxHashMap<DefId, Vec<DefId>>> {
        if let Some(hit) = self.impl_maps.borrow().get(name) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut map: FxHashMap<DefId, Vec<DefId>> = FxHashMap::default();
        for i in ix.defs_named(name) {
            let iv = ix.def(i);
            if iv.kind() != DefKind::Impl {
                continue;
            }
            for t in self.scoped_defs(name, None, iv.file(), &|x| x.kind().is_type()) {
                map.entry(t).or_default().push(i);
            }
        }
        let rc = Rc::new(map);
        self.impl_maps.borrow_mut().insert(name.to_vec(), rc.clone());
        rc
    }

    fn add_type_def(&self, d: DefId, follow: u32, out: &mut Vec<Ty>) {
        let ix = self.ix;
        let v = ix.def(d);
        let mut defs = vec![d];
        defs.extend(self.impls_of(d).iter().copied());
        defs.sort();
        defs.dedup();
        push_ty(out, Ty { name: v.name().to_vec(), defs });
        // An alias also has the members of what it stands for.
        if v.kind() == DefKind::TypeAlias && follow > 0 {
            if let Some(t) = v.type_name() {
                let (m, n) = split_module(t);
                for d2 in self.scoped_defs(n, m, v.file(), &|x| x.kind().is_type()) {
                    if d2 != d {
                        self.add_type_def(d2, follow - 1, out);
                    }
                }
            }
        }
    }

    /// The types `name` denotes when written in file `scope`, optionally qualified by a module.
    /// Outside import-scoped languages a type is its (alias-resolved) name.
    pub fn types_named(&self, name: &[u8], module: Option<&[u8]>, scope: Option<FileId>) -> Vec<Ty> {
        if name.is_empty() {
            return Vec::new();
        }
        let Some(from) = scope.filter(|&f| module_language(self.ix.lang(f))) else {
            return vec![Ty::named(&self.canon_type(name))];
        };
        let mut out = Vec::new();
        for d in self.scoped_defs(name, module, from, &|v| v.kind().is_type()) {
            self.add_type_def(d, 3, &mut out);
        }
        if out.is_empty() && module.is_none() && self.ix.lang(from) == Lang::Rust {
            // A primitive (`str`, `u32`) that this repository implements: its impl blocks.
            let ix = self.ix;
            let all = ix.defs_named(name);
            if !all.is_empty() && all.iter().all(|&d| ix.def(d).kind() == DefKind::Impl) {
                let mut defs: Vec<DefId> = all;
                defs.sort();
                out.push(Ty { name: name.to_vec(), defs });
            }
        }
        out
    }

    /// The type a definition is: itself with its impl blocks, or for an impl block the type it
    /// implements.
    pub fn ty_of_def(&self, v: &DefV) -> Ty {
        let ix = self.ix;
        if !module_language(ix.lang(v.file())) {
            return Ty::named(&self.canon_type(v.name()));
        }
        if v.kind() == DefKind::Impl {
            let mut out = Vec::new();
            for d in self.scoped_defs(v.name(), None, v.file(), &|x| x.kind().is_type()) {
                self.add_type_def(d, 0, &mut out);
            }
            return out.into_iter().find(|t| t.name == v.name()).unwrap_or_else(|| Ty { name: v.name().to_vec(), defs: vec![v.id] });
        }
        if ix.lang(v.file()) == Lang::Ruby && type_like(ix, v) {
            return Ty { name: v.name().to_vec(), defs: self.reopenings(v).to_vec() };
        }
        let mut defs = vec![v.id];
        defs.extend(self.impls_of(v.id).iter().copied());
        defs.sort();
        defs.dedup();
        Ty { name: v.name().to_vec(), defs }
    }

    /// Every definition of a Ruby class or module: a class reopened in another file (or
    /// another `class Foo` block) is the same class when it sits in a namespace of the same
    /// name.
    fn reopenings(&self, v: &DefV) -> Rc<Vec<DefId>> {
        if let Some(hit) = self.reopen.borrow().get(&v.id) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut out: Vec<DefId> = ix
            .defs_named(v.name())
            .into_iter()
            .filter(|&d| {
                let o = ix.def(d);
                d == v.id || (ix.lang(o.file()) == Lang::Ruby && type_like(ix, &o) && o.parent_name() == v.parent_name())
            })
            .collect();
        out.sort();
        let rc = Rc::new(out);
        for &d in rc.iter() {
            self.reopen.borrow_mut().insert(d, rc.clone());
        }
        rc
    }

    /// The type that owns a member definition.
    pub fn owner_ty(&self, v: &DefV) -> Option<Ty> {
        let pname = v.parent_name()?;
        let ix = self.ix;
        if !module_language(ix.lang(v.file())) {
            return Some(Ty::named(&self.canon_type(pname)));
        }
        if let Some(hit) = self.owner_tys.borrow().get(&v.id) {
            return hit.as_ref().map(|t| (**t).clone());
        }
        let ty = match v.parent() {
            Some(p) => self.ty_of_def(&p),
            None => {
                // The container is known by name (a Go receiver type, an out-of-line definition):
                // it is the type of that name visible from the member's file.
                let mut out = Vec::new();
                for d in self.scoped_defs(pname, None, v.file(), &|x| type_like(ix, x)) {
                    self.add_type_def(d, 0, &mut out);
                }
                out.into_iter().find(|t| t.name == pname).unwrap_or_else(|| Ty::named(pname))
            }
        };
        self.owner_tys.borrow_mut().insert(v.id, Some(Rc::new(ty.clone())));
        Some(ty)
    }

    /// The types a declared type denotes: a name, optionally `module|`-qualified, or a path
    /// through nested types (`Outer::Inner`).
    fn declared_tys(&self, t: &[u8], scope: Option<FileId>) -> Vec<Ty> {
        let (m, n) = split_module(t);
        if scope.is_some() && n.windows(2).any(|w| w == b"::") {
            return self.type_path_from(t, scope).to_vec();
        }
        self.types_named(n, m, scope)
    }

    /// Types of member `name` of `ty`: the declared type of a field, or with `call` the declared
    /// result of a method. Members a type does not declare itself come from its supertypes.
    fn member_tys(&self, ty: &Ty, name: &[u8], call: bool, out: &mut Vec<Ty>, depth: u32) {
        let ix = self.ix;
        let mut found = false;
        for d in self.members_of(ty, name) {
            let v = ix.def(d);
            // A nested type is a member that denotes itself (`Outer::Inner`). So do a nested
            // namespace and an enum variant with fields of its own.
            if !call && !ty.defs.is_empty() {
                if v.kind().is_type() {
                    found = true;
                    self.add_type_def(d, 3, out);
                    continue;
                }
                if matches!(v.kind(), DefKind::Namespace | DefKind::Module | DefKind::EnumMember) {
                    found = true;
                    push_ty(out, Ty { name: v.name().to_vec(), defs: vec![d] });
                    continue;
                }
            }
            let is_fn = matches!(v.kind(), DefKind::Method | DefKind::Function);
            if call != is_fn && !(call && v.flags() & dflag::FNPTR != 0) {
                continue;
            }
            found = true;
            if let Some(t) = v.type_name() {
                let scope = if ty.defs.is_empty() { None } else { Some(v.file()) };
                for x in self.declared_tys(t, scope) {
                    push_ty(out, x);
                }
            }
        }
        if found || depth >= 3 {
            return;
        }
        if ty.defs.is_empty()
            && call
            && depth == 0
            && self.member_owners(name).iter().all(|(o, _)| o.as_slice() != ty.name.as_slice())
            && self.supertype_names(&ty.name).is_empty()
        {
            // Include-scoped code writes an unqualified call inside a class as `Class::f()`.
            // When the class has no such method, `f` is a free function.
            self.result_tys(name, None, None, out);
            return;
        }
        if ty.defs.is_empty() && !call {
            // `#define sk_prot __sk_common.skc_prot`: a member alias. Follow the path.
            let mut alias: Option<Vec<Vec<u8>>> = None;
            for d in ix.defs_named(name) {
                let v = ix.def(d);
                if v.kind() == DefKind::Macro && v.rec.arity_max == 0 {
                    alias = member_alias(&v.signature(), name);
                    if alias.is_some() {
                        break;
                    }
                }
            }
            if let Some(path) = alias {
                let mut cur: Vec<Ty> = vec![ty.clone()];
                for f in &path {
                    let mut next = Vec::new();
                    for t in &cur {
                        self.member_tys(t, f, false, &mut next, depth + 1);
                    }
                    cur = next;
                    if cur.is_empty() {
                        return;
                    }
                }
                for c in cur {
                    push_ty(out, c);
                }
                return;
            }
        }
        let before = out.len();
        for a in self.supertypes(ty) {
            let before = out.len();
            self.member_tys(&a, name, call, out, depth + 1);
            if out.len() > before {
                break;
            }
        }
        if out.len() == before && depth == 0 {
            for h in self.php_trait_hosts(ty) {
                self.member_tys(&h, name, call, out, depth + 1);
            }
        }
    }

    /// `$this` inside a PHP trait is the class that uses the trait: for a member the trait does
    /// not declare, the classes using it (directly or through another trait) are where it is.
    fn php_trait_hosts(&self, ty: &Ty) -> Vec<Ty> {
        let ix = self.ix;
        let is_trait = ty.defs.iter().any(|&d| {
            let v = ix.def(d);
            v.kind() == DefKind::Trait && ix.lang(v.file()) == Lang::Php
        });
        if !is_trait {
            return Vec::new();
        }
        let mut out = self.descendants(ty);
        out.truncate(40);
        out
    }

    /// Types of a non-local variable, or the container itself when a namespace, class or
    /// untyped object is used as a value.
    fn value_tys(&self, var: &[u8], module: Option<&[u8]>, scope: Option<FileId>, out: &mut Vec<Ty>) {
        let ix = self.ix;
        let accept = |v: &DefV| matches!(v.kind(), DefKind::Variable | DefKind::Constant) || v.kind().is_container();
        let defs: Vec<DefId> = match scope {
            Some(f) => self.scoped_defs(var, module, f, &accept),
            None => ix.defs_named(var).into_iter().filter(|&d| accept(&ix.def(d))).collect(),
        };
        let mut container = false;
        for d in defs {
            let v = ix.def(d);
            if matches!(v.kind(), DefKind::Variable | DefKind::Constant) {
                match v.type_name() {
                    Some(t) => {
                        for x in self.declared_tys(t, scope.map(|_| v.file())) {
                            push_ty(out, x);
                        }
                    }
                    // An untyped object (`const util = { f() {} }`) owns its members itself.
                    None if scope.is_some() => push_ty(out, Ty { name: var.to_vec(), defs: vec![d] }),
                    None => container = true,
                }
            } else if scope.is_some() {
                // A namespace, module or class used as a value: members are looked up on it.
                // (A reopened Ruby class brings all its definitions at once.)
                if out.iter().any(|t| t.name.as_slice() == var && t.defs.contains(&d)) {
                    continue;
                }
                push_ty(out, self.ty_of_def(&v));
            } else {
                container = true;
            }
        }
        if container && !out.iter().any(|c| c.name == var) {
            out.push(Ty::named(var));
        }
    }

    /// Declared result types of the functions `func` denotes.
    fn result_tys(&self, func: &[u8], module: Option<&[u8]>, scope: Option<FileId>, out: &mut Vec<Ty>) {
        let ix = self.ix;
        let accept = |v: &DefV| matches!(v.kind(), DefKind::Function | DefKind::Method);
        let defs: Vec<DefId> = match scope {
            Some(f) => self.scoped_defs(func, module, f, &accept),
            None => ix.defs_named(func).into_iter().filter(|&d| accept(&ix.def(d))).collect(),
        };
        for d in defs {
            let v = ix.def(d);
            if let Some(t) = v.type_name() {
                for x in self.declared_tys(t, scope.map(|_| v.file())) {
                    push_ty(out, x);
                }
            }
        }
    }

    /// Resolve a receiver type path written in file `from` to the types at its end. Roots: `T`
    /// (a type), `$v` (a non-local variable), `f()` (a call result), each optionally
    /// `module|`-qualified; segments: `::field` and `::method()`.
    pub fn type_path_from(&self, path: &[u8], from: Option<FileId>) -> Rc<Vec<Ty>> {
        let ix = self.ix;
        let scope = from.filter(|&f| module_language(ix.lang(f)));
        let key = (scope, path.to_vec());
        if let Some(hit) = self.type_paths.borrow().get(&key) {
            return hit.clone();
        }
        // A path whose walk leads back to itself (a constant aliasing a path through its own
        // name) ends at this placeholder.
        self.type_paths.borrow_mut().insert(key.clone(), Rc::new(Vec::new()));
        let (module, rest) = split_module(path);
        let mut parts = split_path(rest);
        let root = parts.remove(0);
        let mut cur: Vec<Ty> = Vec::new();
        // A type may itself be called `$Name` (generated code, some libraries).
        let dollar_type = if root.starts_with(b"$") && scope.is_some() { self.types_named(root, module, scope) } else { Vec::new() };
        if !dollar_type.is_empty() {
            cur = dollar_type;
        } else if let Some(var) = root.strip_prefix(b"$") {
            self.value_tys(var, module, scope, &mut cur);
            if cur.is_empty() && module.is_none() && !parts.is_empty() {
                // The name may be a module imported from its package: the next segment is then
                // looked up in that module.
                if let Some(f) = scope {
                    for i in self.file_imports(f).iter() {
                        let Some(real) = i.name.as_deref() else { continue };
                        if i.alias.as_deref().unwrap_or(real) != var {
                            continue;
                        }
                        let mut sub = i.target.clone();
                        if !sub.ends_with(b".") {
                            sub.push(b'.');
                        }
                        sub.extend_from_slice(real);
                        if self.module_files(f, &sub).is_empty() {
                            continue;
                        }
                        let first = parts.remove(0);
                        match first.strip_suffix(b"()") {
                            Some(func) => self.result_tys(func, Some(&sub), scope, &mut cur),
                            None => self.value_tys(first, Some(&sub), scope, &mut cur),
                        }
                        break;
                    }
                }
            }
        } else if let Some(func) = root.strip_suffix(b"()") {
            self.result_tys(func, module, scope, &mut cur);
            if cur.is_empty() && scope.is_some() {
                // `T(x)` is a conversion when `T` is a type the file cannot see is one.
                cur = self.types_named(func, module, scope);
            }
        } else {
            cur = self.types_named(root, module, scope);
            if cur.is_empty() && module.is_none() && !parts.is_empty() && scope.is_some_and(|f| ix.lang(f) == Lang::CSharp) {
                // `Linq.Token` read as an outer type may be a namespace and a type in it.
                let mut ns = root.to_vec();
                while !parts.is_empty() && !parts[0].ends_with(b"()") {
                    let seg = parts.remove(0);
                    cur = self.types_named(seg, Some(&ns), scope);
                    if !cur.is_empty() {
                        break;
                    }
                    ns.push(b'.');
                    ns.extend_from_slice(seg);
                }
            }
        }
        // Where the walk lost its way: the segment to look up again by its own name.
        let mut lost: Option<usize> = if cur.is_empty() { Some(0) } else { None };
        for (i, seg) in parts.iter().enumerate() {
            if cur.is_empty() {
                break;
            }
            let (name, call) = match seg.strip_suffix(b"()") {
                Some(n) => (n, true),
                None => (*seg, false),
            };
            let mut next = Vec::new();
            for t in &cur {
                self.member_tys(t, name, call, &mut next, 0);
            }
            if next.is_empty() && i == 0 && !call && module.is_none() && scope.is_some_and(|f| ix.lang(f) == Lang::CSharp) {
                // C# reads `Name` inside a type as a member of it first, else as a type: the
                // C# extractor writes such a name as `C::Name`.
                next = self.types_named(name, None, scope);
            }
            if next.is_empty() && call && scope.is_some_and(|f| ix.lang(f) == Lang::CSharp) {
                // A C# extension method of the receiver's type: its declared result.
                let cands = ix.defs_named(name);
                if cands.len() <= 512 {
                    for d in cands {
                        let Some(recv) = self.ext_receiver(d) else { continue };
                        if !cur.iter().any(|t| t.name == recv || self.ancestors(t).iter().any(|a| a.name == recv)) {
                            continue;
                        }
                        let v = ix.def(d);
                        if let Some(tn) = v.type_name() {
                            for x in self.declared_tys(tn, Some(v.file())) {
                                push_ty(&mut next, x);
                            }
                        }
                    }
                }
            }
            cur = next;
            cur.truncate(4);
            if cur.is_empty() {
                lost = Some(i);
            }
        }
        if cur.is_empty() && module.is_none() && root.starts_with(b"$") && scope.is_some_and(|f| ix.lang(f) == Lang::Ruby) {
            // Ruby: a constant that its namespace does not hold here (the namespace is a gem's,
            // or a module Rails autoloading makes from a directory) is looked up by its own
            // name, the way Ruby falls back to the namespaces around it and the top level.
            if let Some(i) = lost.filter(|&i| parts.get(i).is_some_and(|s| s.first().is_some_and(|c| c.is_ascii_uppercase()))) {
                let mut again: Vec<u8> = b"$".to_vec();
                for (j, seg) in parts[i..].iter().enumerate() {
                    if j > 0 {
                        again.extend_from_slice(b"::");
                    }
                    again.extend_from_slice(seg);
                }
                cur = self.type_path_from(&again, from).to_vec();
            }
        }
        let rc = Rc::new(cur);
        self.type_paths.borrow_mut().insert(key, rc.clone());
        rc
    }

    /// Name-only form of `type_path_from`, for callers without a file context.
    pub fn type_path(&self, path: &[u8]) -> Vec<Vec<u8>> {
        self.type_path_from(path, None).iter().map(|t| t.name.clone()).collect()
    }

    /// The types a qualifier of a non-member reference can name: a type path, or a type written
    /// after its module (`pkg.mod.Class`, `a::b::Type`).
    fn qualifier_types(&self, from: FileId, q: &[u8]) -> Vec<Ty> {
        let tys = self.type_path_from(q, Some(from));
        if !tys.is_empty() {
            return tys.to_vec();
        }
        if memchr::memchr(b'|', q).is_none() {
            let cut = if self.ix.lang(from) == Lang::Rust {
                q.windows(2).rposition(|w| w == b"::").map(|i| (i, i + 2))
            } else {
                q.iter().rposition(|&b| b == b'.').map(|i| (i, i + 1))
            };
            if let Some((i, j)) = cut.filter(|&(i, j)| i > 0 && j < q.len()) {
                let out = self.types_named(&q[j..], Some(&q[..i]), Some(from));
                if !out.is_empty() {
                    return out;
                }
            }
        }
        // A capitalised name that is no type from here is a constant object: `Registry.as(..)`
        // is a call on the value `Registry`.
        let (module, rest) = split_module(q);
        if module_language(self.ix.lang(from)) && !rest.is_empty() && !rest.starts_with(b"$") && memchr::memchr(b'.', rest).is_none() {
            let mut v: Vec<u8> = Vec::with_capacity(q.len() + 1);
            if let Some(m) = module {
                v.extend_from_slice(m);
                v.push(b'|');
            }
            v.push(b'$');
            v.extend_from_slice(rest);
            return self.type_path_from(&v, Some(from)).to_vec();
        }
        Vec::new()
    }

    /// Is the root of a receiver path positively not of this repository: qualified by a module
    /// that is not ours, or a type name no visible definition carries?
    pub fn root_external(&self, path: &[u8], from: FileId) -> bool {
        if !module_language(self.ix.lang(from)) {
            return false;
        }
        let path = path.strip_prefix(b"^").unwrap_or(path);
        let (module, rest) = split_module(path);
        let root = split_path(rest)[0];
        let (name, is_type) = if let Some(v) = root.strip_prefix(b"$") {
            (v, false)
        } else if let Some(f) = root.strip_suffix(b"()") {
            (f, false)
        } else {
            (root, true)
        };
        match module {
            Some(m) => self.scoped_defs(name, Some(m), from, &|_| true).is_empty() && self.module_namespace(from, m, name).is_empty(),
            None if is_type => self.scoped_defs(name, None, from, &|v| v.kind().is_type() || v.kind().is_container()).is_empty(),
            // A variable nothing visible declares (`Buffer`, `window`) belongs to the platform.
            None => {
                let missing = root.starts_with(b"$")
                    && self.scoped_defs(name, None, from, &|_| true).is_empty()
                    && self.scoped_defs(root, None, from, &|_| true).is_empty();
                if missing && self.ix.lang(from) == Lang::Ruby {
                    // `A::B` with `A` outside the repository: `B` may still be ours.
                    if let Some(inner) = ruby_inner_path(path) {
                        return self.root_external(&inner, from);
                    }
                }
                missing
            }
        }
    }

    /// Does a receiver path end in a type of another package: its root is external, or one of
    /// its members exists and is declared with a type this repository does not define?
    pub fn path_external(&self, path: &[u8], from: FileId) -> bool {
        let ix = self.ix;
        if !module_language(ix.lang(from)) {
            return false;
        }
        if self.root_external(path, from) {
            return true;
        }
        let path = path.strip_prefix(b"^").unwrap_or(path);
        let (module, rest) = split_module(path);
        let parts = split_path(rest);
        if parts.len() < 2 {
            return false;
        }
        // Walk the path one member at a time and look at the member where it stops.
        let mut prefix: Vec<u8> = Vec::new();
        if let Some(m) = module {
            prefix.extend_from_slice(m);
            prefix.push(b'|');
        }
        prefix.extend_from_slice(parts[0]);
        for (i, seg) in parts[1..].iter().enumerate() {
            let cur = self.type_path_from(&prefix, Some(from));
            if cur.is_empty() {
                return false;
            }
            prefix.extend_from_slice(b"::");
            prefix.extend_from_slice(seg);
            if !self.type_path_from(&prefix, Some(from)).is_empty() {
                continue;
            }
            let (name, call) = match seg.strip_suffix(b"()") {
                Some(n) => (n, true),
                None => (*seg, false),
            };
            for t in cur.iter() {
                for d in self.members_of(t, name) {
                    let v = ix.def(d);
                    let is_fn = matches!(v.kind(), DefKind::Method | DefKind::Function);
                    if call == is_fn && v.type_name().is_some() {
                        return true;
                    }
                }
            }
            // `C::Name` from the C# extractor where neither `C` nor its ancestors have a member
            // `Name` and no type `Name` is visible: a type of a library (`Enum.Parse`).
            if i == 0 && module.is_none() && ix.lang(from) == Lang::CSharp && !call {
                return !cur.iter().any(|t| self.has_member(t, name) || self.ancestors(t).iter().any(|a| self.has_member(a, name)));
            }
            return false;
        }
        false
    }

    /// The module files a namespace `ns` of module `m` stands for (`export * as ns from './x'`).
    fn module_namespace(&self, from: FileId, m: &[u8], ns: &[u8]) -> Vec<FileId> {
        let mut out: Vec<FileId> = Vec::new();
        for &f in self.module_files(from, m).iter() {
            for i in self.file_imports(f).iter() {
                if i.name.is_none() && i.alias.as_deref() == Some(ns) {
                    for &x in self.module_files(f, &i.target).iter() {
                        if !out.contains(&x) {
                            out.push(x);
                        }
                    }
                }
            }
        }
        out
    }

    /// How many types declare a member called `name`. A name that few types carry is a strong
    /// hint by itself, which matters where receivers have no written type.
    pub fn member_owner_count(&self, name: &[u8]) -> usize {
        self.member_owners(name).len()
    }

    /// Types that declare a member called `field`, with whether it holds a callable.
    fn member_owners(&self, field: &[u8]) -> Rc<Vec<(Vec<u8>, bool)>> {
        if let Some(hit) = self.owners.borrow().get(field) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut out: Vec<(Vec<u8>, bool)> = Vec::new();
        for d in ix.defs_named(field) {
            let v = ix.def(d);
            // Anything declared inside a container is a member of it: fields and methods of a
            // type, functions and constants of a namespace or object.
            if matches!(
                v.kind(),
                DefKind::Class
                    | DefKind::Struct
                    | DefKind::Union
                    | DefKind::Enum
                    | DefKind::Interface
                    | DefKind::Trait
                    | DefKind::Impl
                    | DefKind::Module
                    | DefKind::Namespace
            ) {
                continue;
            }
            let Some(p) = v.parent_name() else { continue };
            // A function nested in a function is a local helper, not a member.
            if v.parent().is_some_and(|pd| matches!(pd.kind(), DefKind::Function | DefKind::Method)) {
                continue;
            }
            let owner = self.canon_type(p);
            let callable = matches!(v.kind(), DefKind::Method | DefKind::Function) || v.flags() & dflag::FNPTR != 0;
            match out.iter_mut().find(|(o, _)| *o == owner) {
                Some(e) => e.1 |= callable,
                None => out.push((owner, callable)),
            }
        }
        let rc = Rc::new(out);
        self.owners.borrow_mut().insert(field.to_vec(), rc.clone());
        rc
    }

    /// The types a member access `recv.field` can belong to, given the receiver's type path when
    /// one was recorded, without a file context (C-family code, slots written as text).
    pub fn owners_for(&self, qual: Option<&[u8]>, field: &[u8]) -> Vec<Ty> {
        self.owners_for_from(qual, field, true, None)
    }

    /// Owners of the member a reference accesses. In languages with rich built-in types a
    /// member name that only one repository type declares proves nothing about an untyped
    /// receiver (`x.toString()`), so the unique-name inference is limited to C-family code.
    pub fn owners_for_ref(&self, r: &RefV) -> Vec<Ty> {
        let modular = module_language(self.ix.lang(r.file));
        self.owners_for_from(r.qual(), r.name(), !modular, if modular { Some(r.file) } else { None })
    }

    /// `from`: the file the reference is written in (import-scoped languages): type names are
    /// resolved as that file sees them. A path that leads to a type without such a member is
    /// followed up the type's ancestors; when nothing is known and `unique_ok`, a member name
    /// declared by exactly one type identifies that type.
    pub fn owners_for_from(&self, qual: Option<&[u8]>, field: &[u8], unique_ok: bool, from: Option<FileId>) -> Vec<Ty> {
        // `%NAME` is a pseudo owner: a registration table that is not a struct (the LSM hook
        // list). It stands for itself.
        if let Some(q) = qual {
            if q.first() == Some(&b'%') {
                return vec![Ty::named(q)];
            }
        }
        let mut out: Vec<Ty> = Vec::new();
        if let Some(q) = qual.filter(|q| !q.is_empty()) {
            // `^T`: a strict ancestor of T (a `super` call).
            let (strict_up, path) = match q.strip_prefix(b"^") {
                Some(rest) => (true, rest),
                None => (false, q),
            };
            for t in self.type_path_from(path, from).iter() {
                // The member belongs to the nearest type on the way up that declares it.
                if !strict_up && self.has_member(t, field) {
                    push_ty(&mut out, t.clone());
                    continue;
                }
                if let Some(a) = self.ancestors(t).into_iter().find(|a| self.has_member(a, field)) {
                    push_ty(&mut out, a);
                    continue;
                }
                for h in self.php_trait_hosts(t) {
                    if self.has_member(&h, field) {
                        push_ty(&mut out, h);
                    }
                }
                // Ruby: `self` in a method of a module is an object of a class that includes the
                // module, which may define what the module calls.
                if !strict_up && t.defs.iter().any(|&d| self.ix.def(d).kind() == DefKind::Module && self.ix.lang(self.ix.def(d).file()) == Lang::Ruby) {
                    for s in self.descendants(t).iter().take(64) {
                        if self.has_member(s, field) {
                            push_ty(&mut out, s.clone());
                        }
                    }
                }
            }
        }
        if out.is_empty() && unique_ok && !qual.is_some_and(|q| q.starts_with(b"^")) {
            let holders = self.member_owners(field);
            if holders.len() == 1 {
                out.push(Ty::named(&holders[0].0));
            }
        }
        out
    }

    /// Split a slot `Q.f` written in file `from` into (owner types, field). Owner types are empty
    /// when unknown.
    pub fn slot_from(&self, slot: &[u8], from: Option<FileId>) -> (Vec<Ty>, Vec<u8>) {
        match slot.iter().rposition(|&b| b == b'.') {
            Some(i) => {
                let (q, f) = (&slot[..i], &slot[i + 1..]);
                let modular = from.is_some_and(|x| module_language(self.ix.lang(x)));
                let owners = self.owners_for_from(Some(q), f, !modular, if modular { from } else { None });
                (owners, f.to_vec())
            }
            None => (Vec::new(), slot.to_vec()),
        }
    }

    /// Name-only form of `slot_from`, for display.
    pub fn slot(&self, slot: &[u8]) -> (Vec<Vec<u8>>, Vec<u8>) {
        let (owners, f) = self.slot_from(slot, None);
        (owners.into_iter().map(|t| t.name).collect(), f)
    }

    // -----------------------------------------------------------------------------------------
    // References to an entity
    // -----------------------------------------------------------------------------------------

    /// The owning type of a member entity, alias-resolved where types are global names.
    pub fn target_owner(&self, t: &Target) -> Option<Ty> {
        t.owner.as_ref().map(|o| if t.scope.is_empty() { Ty::named(&self.canon_type(o)) } else { Ty { name: o.clone(), defs: t.scope.clone() } })
    }

    /// Visit every reference that can denote `t`.
    pub fn refs_to(&self, t: &Target, mut emit: impl FnMut(RefV<'a>, Conf)) {
        let ix = self.ix;
        let Some(nid) = ix.name_id(&t.name) else { return };
        // Files that define their own file-local entity of the same name and role shadow a
        // global one.
        let mut shadow: FxHashSet<FileId> = FxHashSet::default();
        if t.local_file.is_none() && t.class != Class::Member {
            for d in ix.defs_named_id(nid) {
                let v = ix.def(d);
                if file_local(ix, &v) && class_of(&v) == t.class {
                    shadow.insert(v.file());
                }
            }
        }
        let owner_canon: Option<Ty> = self.target_owner(t);
        // What a call may be written against and still land here: the owner's supertypes and,
        // in Go, the interfaces it satisfies.
        let owner_up: Vec<Ty> = owner_canon
            .as_ref()
            .map(|o| {
                let mut up = self.ancestors(o);
                for i in self.structural_ifaces(o, &t.name) {
                    if !up.contains(&i) {
                        up.push(i);
                    }
                }
                up
            })
            .unwrap_or_default();
        // Other global entities with this name and role compete for each reference.
        let rivals: Vec<Target> = if t.local_file.is_none() && t.owner.is_none() && t.class != Class::Member {
            self.entities_named(&t.name)
                .iter()
                .filter(|e| e.class == t.class && e.local_file.is_none() && !e.defs.iter().any(|d| t.defs.contains(d)))
                .cloned()
                .collect()
        } else {
            Vec::new()
        };
        // Ties in affinity go to the entity with the more prominent definition.
        let rank_of = |e: &Target| e.defs.iter().map(|&d| def_rank(ix, &ix.def(d))).max().unwrap_or(0);
        let my_rank = rank_of(t);
        let rival_ranks: Vec<i64> = rivals.iter().map(rank_of).collect();
        let mut verdicts: FxHashMap<FileId, bool> = FxHashMap::default();
        let memberish = owner_canon.is_some();
        // A C# extension method is called as a member of its `this` parameter's type.
        let ext: Option<DefId> = if memberish { t.defs.iter().copied().find(|&d| self.ext_receiver(d).is_some()) } else { None };
        // No other definition anywhere carries the name.
        let lone = ix.defs_named_id(nid).count() == t.defs.len();
        ix.for_each_ref_named(nid, |r| {
            let ok = if memberish {
                r.kind().is_member() || matches!(r.kind(), RefKind::Call | RefKind::Read | RefKind::Write | RefKind::New | RefKind::Bind | RefKind::Arg)
            } else {
                kind_compatible(t.class, r.kind())
            };
            if !ok {
                // `ns.name` through a namespace binding denotes a module-level definition.
                if !memberish
                    && t.class != Class::Member
                    && r.kind().is_member()
                    && self.namespace_member(&r).is_some_and(|l| t.defs.iter().any(|d| l.contains(d)))
                {
                    emit(r, Conf::Exact);
                }
                return;
            }
            if !memberish && strict_scoping(ix.lang(r.file)) {
                // Import-scoped code: the reference counts when it resolves here, by the same
                // rules that answer "what does this name denote".
                let hit = self.denotes(&r);
                if hit.iter().any(|d| t.defs.contains(d)) {
                    emit(r, Conf::Exact);
                } else if hit.is_empty() && lone && r.qual().is_none() && r.kind() != RefKind::Import {
                    // Not resolvable from where it stands, and nothing else has the name.
                    emit(r, Conf::Name);
                }
                return;
            }
            match t.local_file {
                Some(f) => {
                    if r.file != f {
                        return;
                    }
                }
                None => {
                    if !memberish && shadow.contains(&r.file) {
                        return;
                    }
                    if !rivals.is_empty() {
                        let mine = *verdicts.entry(r.file).or_insert_with(|| {
                            let a = self.affinity(r.file, t);
                            rivals.iter().zip(&rival_ranks).all(|(e, &er)| {
                                let b = self.affinity(r.file, e);
                                b < a || (b == a && er <= my_rank)
                            })
                        });
                        if !mine {
                            return;
                        }
                    }
                }
            }
            let conf = match (&owner_canon, t.class) {
                // Members: fields, properties and methods are reached through a receiver.
                (Some(owner), _) => {
                    let k = r.kind();
                    if let (Some(e), RefKind::MemberCall) = (ext, k) {
                        if let Some(c) = self.ext_match(&r, e) {
                            emit(r, c);
                            return;
                        }
                    }
                    if k.is_member() {
                        let types = self.owners_for_ref(&r);
                        if types.is_empty() {
                            // A receiver that is positively a type of another package is not ours.
                            if r.qual().is_some_and(|q| self.path_external(q, r.file)) {
                                return;
                            }
                            Conf::Name
                        } else if types.iter().any(|x| x.same(owner)) {
                            Conf::Exact
                        } else if !r.qual().is_some_and(|q| q.starts_with(b"^")) && types.iter().any(|x| owner_up.iter().any(|a| a.same(x))) {
                            // A call on a supertype may dispatch to this override. A `super` /
                            // `parent::` call does not dispatch: it runs the ancestor's method.
                            Conf::Likely
                        } else {
                            return;
                        }
                    } else {
                        if matches!(k, RefKind::Bind | RefKind::Arg) {
                            // A method stored in a slot or handed over as a callback. The
                            // qualifier names the slot or the callee, not a receiver.
                            if self.bound_targets(&r).iter().any(|d| t.defs.contains(d)) {
                                emit(r, Conf::Exact);
                            }
                            return;
                        }
                        // `Type.member`, an implicit-this call, a method reference.
                        let Some(q) = r.qual() else {
                            // `new T(..)` runs the constructor, a method named like its type.
                            if matches!(r.kind(), RefKind::New | RefKind::Call) && r.name() == owner.name.as_slice() {
                                if !module_language(ix.lang(r.file)) || self.types_of_ref(&r).iter().any(|x| x.same(owner)) {
                                    emit(r, Conf::Exact);
                                }
                                return;
                            }
                            // A bare name: an enum constant in its own scope, a static import.
                            if strict_scoping(ix.lang(r.file)) && self.denotes(&r).iter().any(|d| t.defs.contains(d)) {
                                emit(r, Conf::Exact);
                            }
                            return;
                        };
                        let path = q.strip_prefix(b"^").unwrap_or(q);
                        let modular = module_language(ix.lang(r.file));
                        let tys: Vec<Ty> = if modular { self.qualifier_types(r.file, path) } else { vec![Ty::named(&self.canon_type(path))] };
                        if tys.iter().any(|x| x.same(owner) || self.ancestors(x).iter().any(|a| a.same(owner))) {
                            Conf::Exact
                        } else if tys.iter().any(|x| owner_up.iter().any(|a| a.same(x))) {
                            Conf::Likely
                        } else if modular && tys.is_empty() && self.qualified_exports(r.file, q, &t.name).is_some_and(|l| t.defs.iter().any(|d| l.contains(d)))
                        {
                            // A member reached through a module-level object (`mod.registry.get()`).
                            Conf::Exact
                        } else if r.kind() == RefKind::Call
                            && ix.lang(r.file) == Lang::CSharp
                            && self.static_using_members(r.file, &t.name).iter().any(|d| t.defs.contains(d))
                        {
                            // `F()` with `using static N.T;` where `T` declares `F`.
                            Conf::Exact
                        } else {
                            return;
                        }
                    }
                }
                (None, Class::Member) => Conf::Name,
                (None, _) => {
                    // A qualified reference must lead to this entity's module or owner.
                    if let Some(q) = r.qual() {
                        if module_language(ix.lang(r.file))
                            && !r.kind().is_member()
                            && r.kind() != RefKind::Bind
                            && r.kind() != RefKind::Arg
                            && r.kind() != RefKind::Import
                        {
                            let fits = if ix.lang(r.file) == Lang::Ruby {
                                // A Ruby qualifier is the namespace a constant is written in.
                                if matches!(r.kind(), RefKind::Type | RefKind::Extends | RefKind::Implements | RefKind::New) {
                                    self.types_of_ref(&r).iter().any(|x| x.defs.iter().any(|d| t.defs.contains(d)))
                                } else {
                                    t.defs.iter().any(|&d| self.link_bonus(r.file, &t.name, Some(q), &ix.def(d)) > 0)
                                        || self.qualifier_types(r.file, q).is_empty()
                                }
                            } else {
                                t.defs.iter().any(|&d| self.link_bonus(r.file, &t.name, Some(q), &ix.def(d)) > 0)
                                    || t.defs.iter().any(|&d| ix.def(d).file() == r.file)
                            };
                            if !fits {
                                return;
                            }
                        }
                    }
                    if t.local_file.is_some() { Conf::Exact } else { Conf::Likely }
                }
            };
            emit(r, conf);
        });
        // A constructor under a fixed name (`__init__`, `constructor`) runs where its class is
        // instantiated.
        if let Some(owner) =
            owner_canon.as_ref().filter(|_| matches!(t.name.as_slice(), b"__init__" | b"constructor" | b"__new__" | b"__construct" | b"initialize"))
        {
            if let Some(oid) = ix.name_id(&owner.name) {
                ix.for_each_ref_named(oid, |r| {
                    let lang = ix.lang(r.file);
                    // Ruby: `T.new(..)` runs `T#initialize`.
                    let ruby_new = lang == Lang::Ruby
                        && t.name.as_slice() == b"initialize"
                        && r.kind() == RefKind::New
                        && self.types_of_ref(&r).iter().any(|x| x.same(owner));
                    if ruby_new
                        || (matches!(r.kind(), RefKind::New | RefKind::Call)
                            && t.name.as_slice() != b"initialize"
                            && strict_scoping(lang)
                            && self.denotes(&r).iter().any(|d| owner.defs.contains(d)))
                    {
                        emit(r, Conf::Exact);
                    }
                });
            }
        }
        // Files that import the name under another name refer to it by that name.
        if !memberish {
            let mut aliases: Vec<(FileId, Vec<u8>)> = Vec::new();
            ix.for_each_ref_named(nid, |r| {
                if r.kind() == RefKind::Import && strict_scoping(ix.lang(r.file)) {
                    for i in self.file_imports(r.file).iter() {
                        if i.name.as_deref() == Some(t.name.as_slice()) {
                            if let Some(a) = i.alias.as_ref().filter(|a| a.as_slice() != t.name.as_slice()) {
                                if !aliases.contains(&(r.file, a.clone())) {
                                    aliases.push((r.file, a.clone()));
                                }
                            }
                        }
                    }
                }
            });
            for (file, alias) in aliases {
                for r in ix.refs_of_file(file) {
                    if r.name() == alias.as_slice()
                        && r.kind() != RefKind::Import
                        && kind_compatible(t.class, r.kind())
                        && self.denotes(&r).iter().any(|d| t.defs.contains(d))
                    {
                        emit(r, Conf::Exact);
                    }
                }
            }
            // Exported by its own module under another name (`export { a as b }`,
            // `export default a`), or as the module itself (`module.exports = a`).
            let mut exported: Vec<(FileId, Vec<u8>)> = Vec::new();
            for &d in &t.defs {
                let v = ix.def(d);
                let m = v.file();
                if !strict_scoping(ix.lang(m)) || v.parent().is_some() {
                    continue;
                }
                if t.name.as_slice() == b"default" && !exported.iter().any(|(f, n)| *f == m && n.as_slice() == b"default") {
                    exported.push((m, b"default".to_vec()));
                }
                for i in self.file_imports(m).iter() {
                    if i.target.is_empty() && i.name.as_deref() == Some(t.name.as_slice()) {
                        if let Some(a) = i.alias.as_ref().filter(|a| !exported.iter().any(|(f, n)| *f == m && n == *a)) {
                            exported.push((m, a.clone()));
                        }
                    }
                }
            }
            for (m, name) in exported {
                if name.as_slice() == b"default" {
                    // Importers give the default export a name of their own.
                    for &f in self.importers_of(m).iter() {
                        let locals: Vec<Vec<u8>> = self
                            .file_imports(f)
                            .iter()
                            .filter(|i| {
                                (i.name.is_none() || i.name.as_deref() == Some(b"default".as_slice()))
                                    && i.alias.as_ref().is_some_and(|a| a.as_slice() != t.name.as_slice())
                                    && self.module_files(f, &i.target).contains(&m)
                            })
                            .filter_map(|i| i.alias.clone())
                            .collect();
                        if locals.is_empty() {
                            continue;
                        }
                        for r in ix.refs_of_file(f) {
                            if r.kind() != RefKind::Import
                                && locals.iter().any(|l| l.as_slice() == r.name())
                                && self.denotes(&r).iter().any(|d| t.defs.contains(d))
                            {
                                emit(r, Conf::Exact);
                            }
                        }
                    }
                } else if let Some(aid) = ix.name_id(&name) {
                    ix.for_each_ref_named(aid, |r| {
                        if r.kind() != RefKind::Import && strict_scoping(ix.lang(r.file)) && self.denotes(&r).iter().any(|d| t.defs.contains(d)) {
                            emit(r, Conf::Exact);
                        }
                    });
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // What a reference denotes
    // -----------------------------------------------------------------------------------------

    /// What a reference written in an import-scoped language denotes: the definitions its name
    /// reaches from where it stands. Empty when the name is not of this repository, or when
    /// that cannot be told.
    pub fn denotes(&self, r: &RefV) -> Vec<DefId> {
        let (name, qual, from) = (r.name(), r.qual(), r.file);
        match r.kind() {
            k if k.is_member() => {
                let mut out: Vec<DefId> = Vec::new();
                for o in self.owners_for_ref(r) {
                    for d in self.members_of(&o, name) {
                        if !out.contains(&d) {
                            out.push(d);
                        }
                    }
                }
                if out.is_empty() {
                    if let Some(l) = self.namespace_member(r) {
                        out = l;
                    }
                }
                if out.is_empty() && r.kind() == RefKind::MemberCall {
                    out = self.extension_targets(r);
                }
                out
            }
            RefKind::Call | RefKind::MacroCall => {
                let mut t = self.resolve_ref_at(r);
                if t.is_empty() {
                    t = self.resolve_type(name, qual, from);
                }
                t
            }
            RefKind::New => {
                let mut t = self.resolve_type(name, qual, from);
                if t.is_empty() {
                    t = self.resolve_ref_at(r);
                }
                t
            }
            RefKind::Type | RefKind::Extends | RefKind::Implements => self.types_of_ref(r).into_iter().flat_map(|t| t.defs).collect(),
            // A bound value or a callback argument carries the slot or the callee as its
            // qualifier: the name itself is written bare.
            RefKind::Bind | RefKind::Arg => self.named_defs(name, None, from),
            _ => self.named_defs(name, qual, from),
        }
    }

    /// The definitions a name used as a value denotes: bare, or after a module or type qualifier.
    fn named_defs(&self, name: &[u8], qual: Option<&[u8]>, from: FileId) -> Vec<DefId> {
        let ix = self.ix;
        let bare_members = !matches!(ix.lang(from), Lang::Python | Lang::JavaScript | Lang::TypeScript | Lang::Tsx | Lang::Go | Lang::Rust | Lang::Php);
        let Some(q) = qual else {
            return self.scoped_defs(name, None, from, &|v| bare_members || !type_member(v));
        };
        if let Some(list) = self.qualified_exports(from, q, name) {
            if !list.is_empty() {
                return list;
            }
        }
        self.qualified_members(name, q, from)
    }

    /// Members called `name` of what qualifier `q` names: a type (its nearest declaring
    /// ancestor), a namespace or module declared in scope, or in languages with implicit `this`
    /// a class that lexically encloses the named one.
    fn qualified_members(&self, name: &[u8], q: &[u8], from: FileId) -> Vec<DefId> {
        let ix = self.ix;
        let (strict_up, path) = match q.strip_prefix(b"^") {
            Some(rest) => (true, rest),
            None => (false, q),
        };
        let mut out: Vec<DefId> = Vec::new();
        let add = |hit: Vec<DefId>, out: &mut Vec<DefId>| {
            for d in hit {
                if !out.contains(&d) {
                    out.push(d);
                }
            }
        };
        let nearest = |t: &Ty, own: bool| -> Vec<DefId> {
            let mut hit = if own { self.members_of(t, name) } else { Vec::new() };
            if hit.is_empty() {
                for a in self.ancestors(t) {
                    hit = self.members_of(&a, name);
                    if !hit.is_empty() {
                        break;
                    }
                }
            }
            hit
        };
        let tys = self.qualifier_types(from, path);
        for t in &tys {
            add(nearest(t, !strict_up), &mut out);
        }
        if !out.is_empty() {
            return out;
        }
        // An inner class reaches the members of the classes around it.
        if matches!(ix.lang(from), Lang::Java | Lang::Kotlin | Lang::Scala | Lang::CSharp) {
            for t in &tys {
                for &d in &t.defs {
                    let mut cur = ix.def(d).parent();
                    for _ in 0..8 {
                        let Some(p) = cur else { break };
                        if p.kind().is_type() {
                            add(nearest(&self.ty_of_def(&p), true), &mut out);
                            if !out.is_empty() {
                                return out;
                            }
                        }
                        cur = p.parent();
                    }
                }
            }
        }
        // A namespace or an inline module of that name.
        let (pm, pn) = split_module(path);
        let last = pn.rsplit(|&b| b == b':' || b == b'.').next().unwrap_or(pn).trim_ascii_start();
        let last = last.strip_prefix(b"$").unwrap_or(last);
        for c in self.scoped_defs(last, pm, from, &|v| matches!(v.kind(), DefKind::Module | DefKind::Namespace)) {
            for d in ix.defs_named(name) {
                if ix.def(d).parent().is_some_and(|p| p.id == c) && !out.contains(&d) {
                    out.push(d);
                }
            }
        }
        out
    }

    /// Static members called `name` of the types that `using static N.T;` directives of C#
    /// file `from` name.
    pub fn static_using_members(&self, from: FileId, name: &[u8]) -> Vec<DefId> {
        let mut out: Vec<DefId> = Vec::new();
        for i in self.file_imports(from).iter() {
            if i.name.is_some() || i.flags & iflag::WILDCARD == 0 || !self.module_files(from, &i.target).is_empty() {
                continue;
            }
            // Not a namespace of this repository: `N.T` names a type (or something external).
            let (module, ty) = match i.target.iter().rposition(|&b| b == b'.') {
                Some(k) => (Some(&i.target[..k]), &i.target[k + 1..]),
                None => (None, i.target.as_slice()),
            };
            for t in self.types_named(ty, module, Some(from)) {
                for d in self.members_of(&t, name) {
                    if !out.contains(&d) {
                        out.push(d);
                    }
                }
            }
        }
        out
    }

    /// The simple name of the type a C# extension method extends: the type of its `this`
    /// parameter, read off the signature. `None` for any other definition.
    pub fn ext_receiver(&self, d: DefId) -> Option<Vec<u8>> {
        if let Some(hit) = self.ext_recv.borrow().get(&d) {
            return hit.clone();
        }
        let ix = self.ix;
        let v = ix.def(d);
        let mut out = None;
        if v.kind() == DefKind::Method && ix.lang(v.file()) == Lang::CSharp && v.parent().is_some() {
            let sig = v.signature();
            // `(this T x`, possibly with a line break after the parenthesis.
            let at = sig.match_indices("this ").map(|(i, _)| i).find(|&i| sig[..i].trim_end().ends_with('('));
            if let Some(i) = at {
                let i = sig[..i].trim_end().len() - 1;
                let rest = sig[i + 1..].trim_start()[5..].trim_start();
                let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')).unwrap_or(rest.len());
                let mut full = &rest[..end];
                // `Foo<T>(this T x) where T : Bar`: the constraint is the extended type.
                let generic = sig[..i].rfind('<').is_some_and(|j| sig[j..i].split(|c: char| c == ',' || c == '<' || c == '>').any(|p| p.trim() == full));
                if generic {
                    let pat = format!("where {full} :");
                    if let Some(w) = sig.find(&pat) {
                        let c = sig[w + pat.len()..].trim_start();
                        let e = c.find(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '.')).unwrap_or(c.len());
                        full = &c[..e];
                    }
                }
                let simple = full.rsplit('.').next().unwrap_or(full);
                let simple = match simple {
                    "string" => "String",
                    "object" => "Object",
                    s => s,
                };
                if !simple.is_empty() {
                    out = Some(simple.as_bytes().to_vec());
                }
            }
        }
        self.ext_recv.borrow_mut().insert(d, out.clone());
        out
    }

    /// Whether the member call `r` can call extension method `d`: its receiver is of the
    /// extended type or derives from it, and does not have a method of that name itself.
    pub fn ext_match(&self, r: &RefV, d: DefId) -> Option<Conf> {
        let recv = self.ext_receiver(d)?;
        let Some(q) = r.qual() else { return Some(Conf::Name) };
        let path = q.strip_prefix(b"^").unwrap_or(q);
        let tys = self.type_path_from(path, Some(r.file));
        if tys.is_empty() {
            // A type this repository does not define (`string`, a library type): its name, as
            // written or as declared for the member the path ends in.
            let (_, rest) = split_module(path);
            if rest == recv.as_slice() {
                return Some(Conf::Likely);
            }
            if let Some(i) = path.windows(2).rposition(|w| w == b"::") {
                let (head, seg) = (&path[..i], &path[i + 2..]);
                let (seg, call) = match seg.strip_suffix(b"()") {
                    Some(x) => (x, true),
                    None => (seg, false),
                };
                for t in self.type_path_from(head, Some(r.file)).iter() {
                    let mut ms = self.members_of(t, seg);
                    if ms.is_empty() {
                        ms = self.ancestors(t).iter().map(|a| self.members_of(a, seg)).find(|m| !m.is_empty()).unwrap_or_default();
                    }
                    for m in ms {
                        let v = self.ix.def(m);
                        let is_fn = matches!(v.kind(), DefKind::Method | DefKind::Function);
                        if is_fn == call && v.type_name().is_some_and(|tn| split_module(tn).1.rsplit(|&b| b == b':').next() == Some(recv.as_slice())) {
                            return Some(Conf::Likely);
                        }
                    }
                }
            }
            return if self.path_external(path, r.file) { None } else { Some(Conf::Name) };
        }
        let name = r.name();
        let ext_tys = self.types_named(&recv, None, Some(self.ix.def(d).file()));
        let mut hit = false;
        for t in tys.iter() {
            // An instance method of the receiver's own type wins over any extension.
            if self.has_member(t, name) || self.ancestors(t).iter().any(|a| self.has_member(a, name)) {
                return None;
            }
            let fits = |x: &Ty| x.name == recv && (ext_tys.is_empty() || ext_tys.iter().any(|e| e.same(x)));
            if fits(t) || self.ancestors(t).iter().any(fits) || self.supertype_names(&t.name).iter().any(|n| *n == recv) {
                hit = true;
            }
        }
        if hit { Some(Conf::Exact) } else { None }
    }

    /// The C# extension methods a member call `r` reaches when its receiver's type has no such
    /// method.
    pub fn extension_targets(&self, r: &RefV) -> Vec<DefId> {
        let ix = self.ix;
        if ix.lang(r.file) != Lang::CSharp {
            return Vec::new();
        }
        let cands = ix.defs_named(r.name());
        if cands.len() > 512 {
            return Vec::new();
        }
        cands.into_iter().filter(|&d| ix.lang(ix.def(d).file()) == Lang::CSharp && matches!(self.ext_match(r, d), Some(Conf::Exact | Conf::Likely))).collect()
    }

    /// `resolve_ref` for a call site, with the scoping of nested functions: a function declared
    /// inside another one is only callable from within it.
    pub fn resolve_ref_at(&self, r: &RefV) -> Vec<DefId> {
        let ix = self.ix;
        let mut t = self.resolve_ref(r.name(), r.qual(), r.file, r.rec.nargs);
        if t.len() > 1 || t.first().is_some_and(|&d| ix.def(d).parent().is_some_and(|p| matches!(p.kind(), DefKind::Function | DefKind::Method))) {
            let mut around: Vec<DefId> = Vec::new();
            let mut cur = r.encl();
            for _ in 0..12 {
                let Some(e) = cur else { break };
                around.push(e.id);
                cur = e.parent();
            }
            let visible = |d: DefId| match ix.def(d).parent() {
                Some(p) if matches!(p.kind(), DefKind::Function | DefKind::Method) => around.contains(&p.id),
                _ => true,
            };
            // The innermost enclosing function's own helper wins over an outer one.
            let nested: Vec<DefId> = t
                .iter()
                .copied()
                .filter(|&d| ix.def(d).parent().is_some_and(|p| matches!(p.kind(), DefKind::Function | DefKind::Method) && around.contains(&p.id)))
                .collect();
            if !nested.is_empty() {
                return nested;
            }
            t.retain(|&d| visible(d));
        }
        t
    }

    /// All entities with a given name, memoised (the partition into entities is the costly part
    /// of resolution and is needed for every call site of a name).
    pub fn entities_named(&self, name: &[u8]) -> Rc<Vec<Target>> {
        if let Some(hit) = self.ents.borrow().get(name) {
            return hit.clone();
        }
        let rc = Rc::new(self.entities(&self.ix.defs_named(name)));
        self.ents.borrow_mut().insert(name.to_vec(), rc.clone());
        rc
    }

    /// Candidate definitions for a call to `name` made from `from` with `nargs` arguments,
    /// best first: one representative (the real definition) per candidate entity.
    pub fn resolve_call(&self, name: &[u8], from: FileId, nargs: u8) -> Vec<DefId> {
        self.resolve_ref(name, None, from, nargs)
    }

    /// The type (class, struct, ...) a reference names: `new T()`, a call of a class.
    pub fn resolve_type(&self, name: &[u8], qual: Option<&[u8]>, from: FileId) -> Vec<DefId> {
        let ix = self.ix;
        if ix.lang(from) == Lang::Ruby {
            // A constant written in a namespace (`A::B.new`) is looked up through it.
            let mut out: Vec<DefId> = Vec::new();
            for t in self.ruby_const_types(name, qual, from) {
                for d in t.defs {
                    if ix.def(d).kind().is_type() && !out.contains(&d) {
                        out.push(d);
                    }
                }
            }
            out.sort_by_key(|&d| (std::cmp::Reverse(def_rank(ix, &ix.def(d))), d));
            return out;
        }
        self.resolve_want(name, qual, from, 255, true)
    }

    /// As `resolve_call`, for a reference written with a qualifier (`mod.f()`, `Type::f()`).
    /// Returns nothing when the language's scoping rules say the name is not from this
    /// repository (an external package, a builtin).
    pub fn resolve_ref(&self, name: &[u8], qual: Option<&[u8]>, from: FileId, nargs: u8) -> Vec<DefId> {
        self.resolve_want(name, qual, from, nargs, false)
    }

    fn resolve_want(&self, name: &[u8], qual: Option<&[u8]>, from: FileId, nargs: u8, want_type: bool) -> Vec<DefId> {
        let ix = self.ix;
        let from_path = ix.path(from);
        let lang = ix.lang(from);
        let strict = strict_scoping(lang);
        // Without implicit `this`, a bare `name()` never denotes a method of a type.
        let no_bare_members =
            qual.is_none() && matches!(lang, Lang::Python | Lang::JavaScript | Lang::TypeScript | Lang::Tsx | Lang::Go | Lang::Rust | Lang::Php);
        if strict {
            let wanted = |v: &DefV| {
                if no_bare_members && !want_type && type_member(v) {
                    return false;
                }
                match class_of(v) {
                    Class::Type => want_type,
                    _ if want_type => false,
                    Class::Callable => true,
                    Class::Value => v.flags() & dflag::FNPTR != 0 || self.alias_target(v.id).is_some(),
                    _ => false,
                }
            };
            // What the module provides under this name, whatever it is called where defined
            // (`export { _min as min } from`), or what an aliased import stands for.
            let mut exact: Vec<DefId> = Vec::new();
            match qual {
                Some(q) => {
                    if let Some(list) = self.qualified_exports(from, q, name) {
                        exact = list.into_iter().filter(|&d| wanted(&ix.def(d))).collect();
                    }
                }
                None => {
                    for i in self.file_imports(from).iter() {
                        let Some(real) = i.name.as_deref() else { continue };
                        if i.alias.as_deref() == Some(name) && real != name {
                            // What the module exports under the real name: re-exports and
                            // local export aliases (`export default f`) are followed.
                            for d in self.exports_of(&self.module_files(from, &i.target), real) {
                                if wanted(&ix.def(d)) && !exact.contains(&d) {
                                    exact.push(d);
                                }
                            }
                            for d in ix.defs_named(real) {
                                let v = ix.def(d);
                                if wanted(&v) && v.file() != from && self.import_evidence(from, name, &v) >= 3400 && !exact.contains(&d) {
                                    exact.push(d);
                                }
                            }
                        }
                    }
                }
            }
            if !exact.is_empty() {
                exact.sort_by_key(|&d| (std::cmp::Reverse(def_rank(ix, &ix.def(d))), d));
                return exact;
            }
            // Collect the definitions the scope reaches: the file, its imports and its package
            // for a bare name, the nearest declaring type for a qualified one.
            {
                let mut cands: Vec<DefId> = match qual {
                    None => self.scoped_defs(name, None, from, &wanted),
                    Some(q) => {
                        let mut found: Vec<DefId> = self.qualified_members(name, q, from).into_iter().filter(|&d| wanted(&ix.def(d))).collect();
                        // A statically imported member is written like a method of the class.
                        if found.is_empty() && matches!(lang, Lang::Java | Lang::Kotlin | Lang::Scala) {
                            found = self.scoped_defs(name, None, from, &|v| wanted(v) && v.file() != from);
                        }
                        // `using static N.T;` makes the static members of `T` callable by name.
                        if found.is_empty() && lang == Lang::CSharp {
                            found = self.static_using_members(from, name).into_iter().filter(|&d| wanted(&ix.def(d))).collect();
                        }
                        found
                    }
                };
                let fits = |d: DefId| {
                    let v = ix.def(d);
                    nargs == 255 || want_type || (nargs >= v.rec.arity_min && (v.rec.arity_max == 255 || nargs <= v.rec.arity_max))
                };
                if !cands.is_empty() {
                    cands.sort_by_key(|&d| (!fits(d), ix.def(d).file() != from, std::cmp::Reverse(def_rank(ix, &ix.def(d))), d));
                    return cands;
                }
                // Nothing in scope. Names that are everywhere end here. Rarer ones get the
                // slower path below, which also knows macros and qualifiers that are not files.
                if ix.defs_named(name).len() > 256 {
                    return cands;
                }
            }
        }
        let ents = self.entities_named(name);
        let mut scored: Vec<(i64, DefId, bool)> = Vec::new();
        for e in ents.iter() {
            if let Some(lf) = e.local_file {
                if lf != from {
                    continue;
                }
            }
            // How well each definition of the entity fits this call site; the entity scores as
            // its best-fitting member (a prototype the caller includes speaks for the function).
            let mut best_score = i64::MIN;
            let mut rep: Option<(bool, i64, DefId)> = None;
            for &d in &e.defs {
                let v = ix.def(d);
                // Object-like macros can stand for a function (`#define sdsfree hi_sdsfree`).
                let callable = match class_of(&v) {
                    Class::Type => want_type,
                    _ if want_type => false,
                    Class::Callable => true,
                    Class::Value => v.flags() & dflag::FNPTR != 0 || v.kind() == DefKind::Macro,
                    _ => false,
                };
                if !callable || (no_bare_members && !want_type && type_member(&v)) {
                    continue;
                }
                let same_file = v.file() == from;
                let mut s = def_rank(ix, &v);
                if v.kind() == DefKind::Macro && v.rec.arity_max == 0 {
                    // Only when nothing callable fits better.
                    s -= 600;
                }
                let link = self.link_bonus(from, name, qual, &v);
                s += link;
                // In import-scoped languages a name that is neither local, imported nor in the
                // package is not this definition.
                if strict && (link < 0 || (!same_file && link == 0 && v.kind() != DefKind::Macro)) {
                    s -= 100_000;
                }
                if same_file {
                    s += 5000;
                } else if ix.file(v.file()).flags & crate::facts::fflag::HEADER != 0 && self.includes(from, v.file()) {
                    s += 3000;
                }
                if nargs != 255 && !want_type && v.flags() & dflag::MACRO_DEFINED == 0 {
                    let ok = nargs >= v.rec.arity_min && (v.rec.arity_max == 255 || nargs <= v.rec.arity_max);
                    if ok {
                        s += 400;
                    } else {
                        s -= 2000;
                    }
                }
                s += 20 * common_prefix_dirs(from_path, v.path()) as i64;
                best_score = best_score.max(s);
                // The representative is a real definition when the entity has one.
                let key = (!v.is_decl(), s, d);
                if rep.is_none_or(|r| (key.0, key.1) > (r.0, r.1)) {
                    rep = Some(key);
                }
            }
            if let Some((_, _, d)) = rep {
                scored.push((best_score, d, e.local_file.is_some()));
            }
        }
        scored.retain(|x| x.0 > -50_000);
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        // A file-local definition wins outright.
        if scored.first().is_some_and(|x| x.2) {
            scored.truncate(1);
        }
        scored.into_iter().map(|(_, d, _)| d).collect()
    }

    /// The functions a bound value or a callback argument denotes. The record carries the bare
    /// name. When the same line reads that name as a member (`slot = this.method`,
    /// `const f = T.create`), the receiver decides.
    pub fn bound_targets(&self, b: &RefV<'a>) -> Rc<Vec<DefId>> {
        let key = (b.file, b.line(), b.name().to_vec());
        if let Some(hit) = self.bound.borrow().get(&key) {
            return hit.clone();
        }
        let ix = self.ix;
        let read = if module_language(ix.lang(b.file)) {
            ix.refs_of_file(b.file).find(|r| r.kind() == RefKind::MemberRead && r.line() == b.line() && r.name() == b.name())
        } else {
            None
        };
        let mut out: Vec<DefId> = match read {
            Some(m) => self.method_value(&m),
            None => self.resolve_call(b.name(), b.file, 255).into_iter().take(1).collect(),
        };
        out.sort_by_key(|&d| ix.def(d).is_decl());
        let rc = Rc::new(out);
        self.bound.borrow_mut().insert(key, rc.clone());
        rc
    }

    /// The methods a member read denotes when it reads a method as a value (`obj.method` with
    /// no call). Empty for fields.
    pub fn method_value(&self, m: &RefV) -> Vec<DefId> {
        let ix = self.ix;
        let name = m.name();
        let mut hit: Vec<DefId> = Vec::new();
        for o in self.owners_for_ref(m) {
            let mut own = self.members_of(&o, name);
            if own.is_empty() {
                for a in self.ancestors(&o) {
                    own = self.members_of(&a, name);
                    if !own.is_empty() {
                        break;
                    }
                }
            }
            for d in own {
                if !hit.contains(&d) {
                    hit.push(d);
                }
            }
        }
        if hit.is_empty() {
            if let Some(l) = self.namespace_member(m) {
                hit = l;
            }
        }
        // A field initialised with a function stands for it (`static create = makeThing`).
        let mut out: Vec<DefId> = Vec::new();
        for d in hit {
            let f = if matches!(ix.def(d).kind(), DefKind::Method | DefKind::Function) { Some(d) } else { self.alias_target(d) };
            if let Some(f) = f.filter(|f| !out.contains(f)) {
                out.push(f);
            }
        }
        out
    }

    /// The function a variable or field stands for when its initialiser is nothing but a
    /// function (`const f = T.create`, `static create = createEnum`, `augment = this.extend`).
    pub fn alias_target(&self, d: DefId) -> Option<DefId> {
        if let Some(hit) = self.aliases.borrow().get(&d) {
            return *hit;
        }
        // Guards against a cycle of aliases.
        self.aliases.borrow_mut().insert(d, None);
        let ix = self.ix;
        let v = ix.def(d);
        let mut out = None;
        if matches!(v.kind(), DefKind::Variable | DefKind::Constant | DefKind::Field)
            && matches!(ix.lang(v.file()), Lang::JavaScript | Lang::TypeScript | Lang::Tsx | Lang::Python | Lang::Ruby)
        {
            let name = v.name();
            let into_slot = |q: &[u8]| q.len() > name.len() && q.ends_with(name) && q[q.len() - name.len() - 1] == b'.';
            let binds: Vec<RefV<'a>> = ix.refs_of_file(v.file()).filter(|r| r.kind() == RefKind::Bind && r.qual().is_some_and(into_slot)).collect();
            let own: Vec<&RefV<'a>> =
                binds.iter().filter(|r| r.line() >= v.rec.line && r.line() <= v.rec.end_line && r.encl().is_some_and(|e| e.id == d)).collect();
            // One value, bound once: nothing else is ever stored there.
            if let [b] = own.as_slice() {
                if binds.iter().filter(|r| r.qual() == b.qual()).count() == 1 {
                    out = match self.bound_targets(b).first() {
                        Some(&t) if matches!(ix.def(t).kind(), DefKind::Method | DefKind::Function) => Some(t),
                        Some(&t) if t != d => self.alias_target(t),
                        _ => None,
                    };
                }
            }
        }
        self.aliases.borrow_mut().insert(d, out);
        out
    }

    /// The function behind `T.slot(..)` when `slot` is a static field initialised with one.
    pub fn static_alias(&self, r: &RefV) -> Option<DefId> {
        let q = r.qual()?;
        if !module_language(self.ix.lang(r.file)) {
            return None;
        }
        for ty in self.qualifier_types(r.file, q.strip_prefix(b"^").unwrap_or(q)) {
            let mut fields = self.members_of(&ty, r.name());
            if fields.is_empty() {
                for a in self.ancestors(&ty) {
                    fields = self.members_of(&a, r.name());
                    if !fields.is_empty() {
                        break;
                    }
                }
            }
            if let Some(a) = fields.iter().find_map(|&f| self.alias_target(f)) {
                return Some(a);
            }
        }
        None
    }

    // -----------------------------------------------------------------------------------------
    // Macro-mediated calls (kernel static calls, name-pasting macros)
    // -----------------------------------------------------------------------------------------

    /// The text of a macro definition, continuation lines included.
    fn macro_text(&self, v: &DefV) -> Vec<u8> {
        let Some(src) = self.ix.source(v.file()) else { return Vec::new() };
        let mut line = 1u32;
        let mut start = 0usize;
        let mut end = src.len();
        for (i, &b) in src.iter().enumerate() {
            if b == b'\n' {
                line += 1;
                if line == v.rec.line {
                    start = i + 1;
                }
                if line == v.rec.end_line.min(v.rec.line + 12) + 1 {
                    end = i;
                    break;
                }
            }
        }
        if start >= end { Vec::new() } else { src[start..end].to_vec() }
    }

    /// For a macro that is nothing but a pasted name (`#define vt_op(name) vt_##name`): the
    /// text before and after the parameter, once per definition of the macro.
    fn paste_of(&self, mac: &[u8]) -> Rc<Vec<(Vec<u8>, Vec<u8>)>> {
        if let Some(hit) = self.pastes.borrow().get(mac) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut out: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for d in ix.defs_named(mac) {
            let v = ix.def(d);
            if v.kind() != DefKind::Macro || v.rec.arity_max != 1 {
                continue;
            }
            let text = self.macro_text(&v);
            // #define NAME(param) BODY
            let Some(open) = memchr::memchr(b'(', &text) else { continue };
            let Some(close) = memchr::memchr(b')', &text) else { continue };
            if close < open {
                continue;
            }
            let param: Vec<u8> = text[open + 1..close].iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
            let body: Vec<u8> = text[close + 1..].iter().copied().filter(|b| !b.is_ascii_whitespace() && *b != b'\\').collect();
            if param.is_empty() {
                continue;
            }
            // prefix##param, param##suffix or prefix##param##suffix, and nothing else
            let parts: Vec<&[u8]> = body.split(|&b| b == b'#').filter(|p| !p.is_empty()).collect();
            let is_ident = |p: &[u8]| !p.is_empty() && p.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_');
            if !(2..=3).contains(&parts.len()) || !parts.iter().all(|p| is_ident(p)) || parts.iter().filter(|p| **p == param.as_slice()).count() != 1 {
                continue;
            }
            let at = parts.iter().position(|p| *p == param.as_slice()).unwrap_or(0);
            let prefix: Vec<u8> = parts[..at].concat();
            let suffix: Vec<u8> = parts[at + 1..].concat();
            if !out.iter().any(|(a, b)| *a == prefix && *b == suffix) {
                out.push((prefix, suffix));
            }
        }
        let rc = Rc::new(out);
        self.pastes.borrow_mut().insert(mac.to_vec(), rc.clone());
        rc
    }

    /// The names `mac(arg)` can expand to when `mac` only pastes a name together.
    pub fn pasted_names(&self, mac: &[u8], arg: &[u8]) -> Vec<Vec<u8>> {
        self.paste_of(mac).iter().map(|(p, s)| [p.as_slice(), arg, s.as_slice()].concat()).collect()
    }

    /// References on the same line of the same file as `r`.
    fn same_line(&self, r: &RefV<'a>) -> Vec<RefV<'a>> {
        self.ix.refs_of_file(r.file).filter(|x| x.line() == r.line()).collect()
    }

    fn macro_links(&self) -> Rc<MacroLinks> {
        if let Some(hit) = self.macro_links.borrow().as_ref() {
            return hit.clone();
        }
        let ix = self.ix;
        let mut ml = MacroLinks::default();
        let is_fn = |name: &[u8]| ix.defs_named(name).iter().any(|&d| ix.def(d).kind() == DefKind::Function);
        let tie = |ml: &mut MacroLinks, key: &[u8], slot: Option<(Ty, Vec<u8>)>, func: Option<&[u8]>| {
            if let Some(sl) = slot {
                let v = ml.key_slots.entry(key.to_vec()).or_default();
                if !v.contains(&sl) {
                    v.push(sl);
                }
            }
            if let Some(f) = func {
                let v = ml.key_funcs.entry(key.to_vec()).or_default();
                if !v.iter().any(|x| x.as_slice() == f) {
                    v.push(f.to_vec());
                }
            }
        };
        let mut macros: Vec<DefId> = Vec::new();
        for name in STATIC_DEFS.iter().chain(std::iter::once(&STATIC_UPDATE)).chain(STATIC_CALLS.iter()) {
            let Some(id) = ix.name_id(name) else { continue };
            let is_update = *name == STATIC_UPDATE;
            let is_def = STATIC_DEFS.contains(name);
            let mut sites: Vec<RefV<'a>> = Vec::new();
            ix.for_each_ref_named(id, |r| {
                if matches!(r.kind(), RefKind::Call | RefKind::MacroCall) {
                    sites.push(r);
                }
            });
            for r in sites {
                let encl = r.encl();
                if let Some(m) = encl.as_ref().filter(|e| e.kind() == DefKind::Macro) {
                    // Inside a macro body: read from its text below.
                    if !macros.contains(&m.id) {
                        macros.push(m.id);
                    }
                    continue;
                }
                if !(is_update || is_def) {
                    continue;
                }
                let line = self.same_line(&r);
                let slot = line
                    .iter()
                    .find(|x| x.kind() == RefKind::MemberRead)
                    .and_then(|m| self.owners_for_ref(m).into_iter().next().map(|o| (o, m.name().to_vec())));
                let args: Vec<&[u8]> =
                    line.iter().filter(|x| matches!(x.kind(), RefKind::Arg | RefKind::Read) && x.name() != r.name()).map(|x| x.name()).collect();
                // The key: the variable the definition creates, else the argument that is not
                // a function.
                let key: Option<Vec<u8>> = if is_def {
                    encl.as_ref().filter(|e| matches!(e.kind(), DefKind::Variable | DefKind::Function)).map(|e| e.name().to_vec())
                } else {
                    args.iter().find(|a| !is_fn(a) || ml.key_slots.contains_key(**a) || ml.key_funcs.contains_key(**a)).map(|a| a.to_vec())
                };
                let Some(key) = key else { continue };
                let func = if slot.is_none() { args.iter().find(|a| **a != key.as_slice() && is_fn(a)).copied() } else { None };
                tie(&mut ml, &key, slot, func);
            }
        }
        for m in macros {
            let v = ix.def(m);
            let text = self.macro_text(&v);
            let Some(open) = memchr::memchr(b'(', &text) else { continue };
            let Some(close) = memchr::memchr(b')', &text) else { continue };
            if close < open {
                continue;
            }
            let params: Vec<Vec<u8>> =
                text[open + 1..close].split(|&b| b == b',').map(|p| p.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect()).collect();
            let body = &text[close + 1..];
            for name in STATIC_CALLS.iter().chain(STATIC_DEFS.iter()).chain(std::iter::once(&STATIC_UPDATE)) {
                let mut from = 0usize;
                while let Some(at) = memchr::memmem::find(&body[from..], name) {
                    let start = from + at;
                    from = start + name.len();
                    // A whole word, followed by its argument list.
                    if start > 0 && (body[start - 1].is_ascii_alphanumeric() || body[start - 1] == b'_') {
                        continue;
                    }
                    let mut i = from;
                    while i < body.len() && body[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    if body.get(i) != Some(&b'(') {
                        continue;
                    }
                    let Some((prefix, param, after)) = pasted_at(body, i + 1) else { continue };
                    if !params.iter().any(|p| p.as_slice() == param) {
                        continue;
                    }
                    if STATIC_CALLS.contains(name) {
                        ml.wrappers.insert(v.name().to_vec(), prefix.to_vec());
                        continue;
                    }
                    // The rest of the argument list names the table: `((struct T *)0)->p` or
                    // `var.p`.
                    let end = memchr::memchr(b';', &body[after..]).map(|e| after + e).unwrap_or(body.len());
                    let rest = &body[after..end];
                    let mut owner: Option<Ty> = None;
                    if let Some(st) = memchr::memmem::find(rest, b"struct") {
                        let (tname, _) = ident_at(rest, st + 6);
                        if !tname.is_empty() {
                            owner = Some(Ty::named(&self.canon_type(tname)));
                        }
                    }
                    if owner.is_none() {
                        // `var.param` or `var->param`
                        let mut j = 0usize;
                        while j < rest.len() {
                            let (id, k) = ident_at(rest, j);
                            if id.is_empty() {
                                j = k + 1;
                                continue;
                            }
                            let mut n = k;
                            while n < rest.len() && rest[n].is_ascii_whitespace() {
                                n += 1;
                            }
                            let sep = if rest[n..].starts_with(b"->") {
                                2
                            } else if rest[n..].starts_with(b".") {
                                1
                            } else {
                                0
                            };
                            if sep > 0 && ident_at(rest, n + sep).0 == param {
                                let mut tys = Vec::new();
                                self.value_tys(id, None, None, &mut tys);
                                owner = tys.into_iter().next();
                                break;
                            }
                            j = k;
                        }
                    }
                    if let Some(o) = owner {
                        if !ml.prefix_owner.iter().any(|(p, t)| p.as_slice() == prefix && *t == o) {
                            ml.prefix_owner.push((prefix.to_vec(), o));
                        }
                    }
                }
            }
        }
        let rc = Rc::new(ml);
        *self.macro_links.borrow_mut() = Some(rc.clone());
        rc
    }

    /// The table slot a static call goes through, given the argument record of its key:
    /// `static_call(key)(..)`, or a wrapper macro such as `kvm_x86_call(member)(..)`.
    pub fn static_call_slot(&self, arg: &RefV) -> Option<(Ty, Vec<u8>)> {
        if arg.kind() != RefKind::Arg || self.ix.lang(arg.file) != Lang::C {
            return None;
        }
        let q = arg.qual()?;
        let ml = self.macro_links();
        if STATIC_CALLS.contains(&q) {
            if let Some(s) = ml.key_slots.get(arg.name()).and_then(|v| v.first()) {
                return Some(s.clone());
            }
            return ml
                .prefix_owner
                .iter()
                .find(|(p, _)| arg.name().starts_with(p) && arg.name().len() > p.len())
                .map(|(p, o)| (o.clone(), arg.name()[p.len()..].to_vec()));
        }
        let prefix = ml.wrappers.get(q)?;
        ml.prefix_owner.iter().find(|(p, _)| p == prefix).map(|(_, o)| (o.clone(), arg.name().to_vec()))
    }

    /// Functions installed directly in the static-call key named by `arg`.
    pub fn static_call_funcs(&self, arg: &RefV) -> Vec<DefId> {
        let ml = self.macro_links();
        let Some(q) = arg.qual() else { return Vec::new() };
        if !STATIC_CALLS.contains(&q) {
            return Vec::new();
        }
        let mut out: Vec<DefId> = Vec::new();
        for f in ml.key_funcs.get(arg.name()).map(|v| v.as_slice()).unwrap_or(&[]) {
            if let Some(&d) = self.resolve_call(f, arg.file, 255).first() {
                if !out.contains(&d) {
                    out.push(d);
                }
            }
        }
        out
    }

    /// Static-call sites that dispatch through slot `owner.field`.
    fn static_slot_sites(&self, owner: &Ty, field: &[u8], out: &mut Vec<(RefV<'a>, Conf)>) {
        let ix = self.ix;
        if ix.name_id(b"static_call").is_none() {
            return;
        }
        let ml = self.macro_links();
        let push = |name: &[u8], quals: &dyn Fn(&[u8]) -> bool, out: &mut Vec<(RefV<'a>, Conf)>| {
            if let Some(id) = ix.name_id(name) {
                ix.for_each_ref_named(id, |r| {
                    if r.kind() == RefKind::Arg && r.qual().is_some_and(quals) {
                        out.push((r, Conf::Exact));
                    }
                });
            }
        };
        for (key, slots) in ml.key_slots.iter() {
            if slots.iter().any(|(o, f)| o.same(owner) && f.as_slice() == field) {
                push(key, &|q| STATIC_CALLS.contains(&q), out);
            }
        }
        for (prefix, o) in ml.prefix_owner.iter() {
            if !o.same(owner) {
                continue;
            }
            // The pasted key itself, and the wrapper macros that paste it.
            push(&[prefix.as_slice(), field].concat(), &|q| STATIC_CALLS.contains(&q), out);
            for (w, p) in ml.wrappers.iter() {
                if p == prefix {
                    push(field, &|q| q == w.as_slice(), out);
                }
            }
        }
    }

    /// Call sites of the static-call keys that function `name` is installed in directly.
    pub fn static_key_sites(&self, name: &[u8]) -> Vec<(Vec<u8>, RefV<'a>)> {
        let ix = self.ix;
        let mut out = Vec::new();
        if ix.name_id(b"static_call").is_none() {
            return out;
        }
        let ml = self.macro_links();
        for (key, funcs) in ml.key_funcs.iter() {
            if !funcs.iter().any(|f| f.as_slice() == name) {
                continue;
            }
            if let Some(id) = ix.name_id(key) {
                ix.for_each_ref_named(id, |r| {
                    if r.kind() == RefKind::Arg && r.qual().is_some_and(|q| STATIC_CALLS.contains(&q)) {
                        out.push((key.clone(), r));
                    }
                });
            }
        }
        out
    }

    /// Functions written into slot `owner.field` through a name-pasting macro
    /// (`.vcpu_run = vt_op(vcpu_run)`), with the place of each such entry.
    pub fn pasted_bindings(&self, owner: &Ty, field: &[u8]) -> Vec<(RefV<'a>, Vec<DefId>)> {
        let ix = self.ix;
        let mut out: Vec<(RefV<'a>, Vec<DefId>)> = Vec::new();
        let Some(fid) = ix.name_id(field) else { return out };
        let mut sites: Vec<RefV<'a>> = Vec::new();
        ix.for_each_ref_named(fid, |r| {
            if r.kind() == RefKind::Arg && ix.lang(r.file) == Lang::C && r.qual().is_some_and(|q| !self.paste_of(q).is_empty()) {
                sites.push(r);
            }
        });
        for r in sites {
            // The entry it initialises: a write of a member on the same line, of this type.
            let line = self.same_line(&r);
            let writes = line.iter().any(|w| w.kind() == RefKind::MemberWrite && w.name() == field && self.owners_for_ref(w).iter().any(|o| o.same(owner)));
            if !writes {
                continue;
            }
            let mut fns: Vec<DefId> = Vec::new();
            for name in self.pasted_names(r.qual().unwrap_or(b""), field) {
                for d in self.resolve_call(&name, r.file, 255).into_iter().take(1) {
                    if matches!(ix.def(d).kind(), DefKind::Function) && !fns.contains(&d) {
                        fns.push(d);
                    }
                }
            }
            if !fns.is_empty() {
                out.push((r, fns));
            }
        }
        out
    }

    /// The table slots function `name` is written into through a name-pasting macro.
    pub fn pasted_slots_of(&self, name: &[u8], defs: &[DefId]) -> Vec<(RefV<'a>, Ty, Vec<u8>)> {
        let ix = self.ix;
        let mut out: Vec<(RefV<'a>, Ty, Vec<u8>)> = Vec::new();
        if !defs.iter().any(|&d| ix.lang(ix.def(d).file()) == Lang::C) {
            return out;
        }
        // Every way to cut the name into prefix + member (+ suffix) at an underscore.
        for (i, &b) in name.iter().enumerate() {
            if b != b'_' || i + 1 >= name.len() {
                continue;
            }
            let member = &name[i + 1..];
            let Some(mid) = ix.name_id(member) else { continue };
            let mut sites: Vec<RefV<'a>> = Vec::new();
            ix.for_each_ref_named(mid, |r| {
                if r.kind() == RefKind::Arg && ix.lang(r.file) == Lang::C {
                    sites.push(r);
                }
            });
            for r in sites {
                let Some(q) = r.qual() else { continue };
                if !self.pasted_names(q, member).iter().any(|n| n.as_slice() == name) {
                    continue;
                }
                for w in self.same_line(&r) {
                    if w.kind() == RefKind::MemberWrite && w.name() == member {
                        for o in self.owners_for_ref(&w) {
                            if !out.iter().any(|(_, t, f)| *t == o && f.as_slice() == member) {
                                out.push((r, o, member.to_vec()));
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// Functions bound into slot `owner.field` anywhere in the repository.
    pub fn slot_bindings(&self, owner: &Ty, field: &[u8]) -> Vec<(RefV<'a>, Conf)> {
        let key = (owner.clone(), field.to_vec());
        let ix = self.ix;
        if let Some(hit) = self.slot_binds.borrow().get(&key) {
            return hit.iter().map(|&(k, c)| (ix.ref_of(k), c)).collect();
        }
        let mut out: Vec<(RefV<'a>, Conf)> = Vec::new();
        for &k in self.binds_into(field).iter() {
            let r = ix.ref_of(k);
            let Some(q) = r.qual() else { continue };
            let (owners, _) = self.slot_from(q, Some(r.file));
            if owners.is_empty() {
                out.push((r, Conf::Name));
            } else if owners.iter().any(|o| o.same(owner)) {
                out.push((r, Conf::Exact));
            }
        }
        self.slot_binds.borrow_mut().insert(key, Rc::new(out.iter().map(|(r, c)| (r.key(), *c)).collect()));
        out
    }

    /// Every binding of a value into a slot called `field` (`.field = fn`), found once per name.
    fn binds_into(&self, field: &[u8]) -> Rc<Vec<RefKey>> {
        if let Some(hit) = self.field_binds.borrow().get(field) {
            return hit.clone();
        }
        let ix = self.ix;
        let mut out: Vec<RefKey> = Vec::new();
        if let Some(fid) = ix.name_id(field) {
            // Every bind into `*.field` sits in a file that also writes member `field`.
            for file in ix.files_with(fid) {
                for r in ix.refs_of_file(file) {
                    if r.kind() != RefKind::Bind {
                        continue;
                    }
                    let Some(q) = r.qual() else { continue };
                    if q.ends_with(field) && q.len() > field.len() && q[q.len() - field.len() - 1] == b'.' {
                        out.push(r.key());
                    }
                }
            }
        }
        let rc = Rc::new(out);
        self.field_binds.borrow_mut().insert(field.to_vec(), rc.clone());
        rc
    }

    /// Call sites that dispatch through slot `owner.field` (`x->field(...)`).
    pub fn slot_calls(&self, owner: &Ty, field: &[u8]) -> Vec<(RefV<'a>, Conf)> {
        let ix = self.ix;
        let mut out = Vec::new();
        let Some(fid) = ix.name_id(field) else { return out };
        ix.for_each_ref_named(fid, |r| {
            if r.kind() == RefKind::Call {
                // `T.slot(..)` on the type itself.
                if let Some(q) = r.qual().filter(|_| module_language(ix.lang(r.file))) {
                    if self.qualifier_types(r.file, q.strip_prefix(b"^").unwrap_or(q)).iter().any(|t| t.same(owner)) {
                        out.push((r, Conf::Exact));
                    }
                }
                return;
            }
            if r.kind() != RefKind::MemberCall {
                return;
            }
            let types = self.owners_for_ref(&r);
            if types.is_empty() {
                out.push((r, Conf::Name));
            } else if types.iter().any(|t| t.same(owner)) {
                out.push((r, Conf::Exact));
            }
        });
        self.static_slot_sites(owner, field, &mut out);
        out
    }
}

/// Parse `#define NAME a.b.c` (or `a->b`) into the member path `[a, b, c]`.
fn member_alias(sig: &str, name: &[u8]) -> Option<Vec<Vec<u8>>> {
    let rest = sig.strip_prefix("#define")?.trim_start();
    let rest = rest.strip_prefix(std::str::from_utf8(name).ok()?)?.trim();
    if rest.is_empty() || rest == std::str::from_utf8(name).ok()? {
        return None;
    }
    let mut parts = Vec::new();
    for part in rest.split(|c| c == '.' || c == '>').map(|p| p.trim_end_matches('-').trim()) {
        if part.is_empty() || !part.bytes().all(|b| b == b'_' || b.is_ascii_alphanumeric()) || part.as_bytes()[0].is_ascii_digit() {
            return None;
        }
        parts.push(part.as_bytes().to_vec());
    }
    if parts.is_empty() { None } else { Some(parts) }
}

fn split_path(path: &[u8]) -> Vec<&[u8]> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i + 1 < path.len() {
        if path[i] == b':' && path[i + 1] == b':' {
            parts.push(&path[start..i]);
            start = i + 2;
            i += 2;
        } else {
            i += 1;
        }
    }
    parts.push(&path[start..]);
    parts
}

/// Number of leading directories two paths share.
pub fn common_prefix_dirs(a: &str, b: &str) -> usize {
    let mut n: usize = 0;
    let (mut ia, mut ib) = (a.split('/'), b.split('/'));
    loop {
        match (ia.next(), ib.next()) {
            (Some(x), Some(y)) if x == y => n += 1,
            _ => break,
        }
    }
    // The last matching component may be the file name itself: only directories count.
    if a == b { n.saturating_sub(1) } else { n }
}
