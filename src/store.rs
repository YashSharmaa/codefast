//! The index as queries see it: typed ids, views that resolve names lazily, and source access.
//!
//! An index is a base segment plus, when the working tree has changed since the base was built,
//! a small overlay segment holding the changed and new files. Files of the base that were
//! changed or deleted are marked dead and never surface. Queries see one id space for files and
//! definitions; names are addressed by `NameId`, which carries the name's id in each segment.

use crate::facts::{DefKind, RefKind, dflag, fflag};
use crate::index::Segment;
use crate::index::format::{DefRec, FileRec, ImportRec, PARENT_IS_NAME, RefRec};
use crate::lang::Lang;
use rustc_hash::FxHashMap;
use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct DefId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct FileId(pub u32);

pub const MAX_SEGS: usize = 2;
const NO: u32 = u32::MAX;

/// A name, as its id in each segment (`u32::MAX` where the segment does not know it).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct NameId(pub [u32; MAX_SEGS]);

pub(crate) struct Seg {
    pub seg: Segment,
    pub file_base: u32,
    pub def_base: u32,
    /// Bit set: the file is superseded by a newer segment, or no longer exists.
    dead: Vec<u64>,
    any_dead: bool,
}

impl Seg {
    pub fn new(seg: Segment, file_base: u32, def_base: u32) -> Seg {
        let n = seg.n_files() as usize;
        Seg { seg, file_base, def_base, dead: vec![0; n.div_ceil(64)], any_dead: false }
    }

    pub fn kill(&mut self, local_file: u32) {
        self.dead[local_file as usize / 64] |= 1 << (local_file % 64);
        self.any_dead = true;
    }

    #[inline]
    fn is_dead(&self, local_file: u32) -> bool {
        self.any_dead && self.dead[local_file as usize / 64] & (1 << (local_file % 64)) != 0
    }
}

pub struct Index {
    pub root: PathBuf,
    segs: Vec<Seg>,
    src: Mutex<FxHashMap<u32, Option<Arc<Vec<u8>>>>>,
}

/// A definition with its names resolved on demand.
#[derive(Clone, Copy)]
pub struct DefV<'a> {
    pub id: DefId,
    pub rec: &'a DefRec,
    ix: &'a Index,
    si: u8,
}

/// A reference occurrence.
#[derive(Clone, Copy)]
pub struct RefV<'a> {
    pub file: FileId,
    pub rec: RefRec,
    ix: &'a Index,
    si: u8,
}

/// A reference detached from the index it came from, to be kept in a cache.
#[derive(Clone, Copy)]
pub struct RefKey {
    pub file: FileId,
    rec: RefRec,
    si: u8,
}

impl<'a> RefV<'a> {
    pub fn key(&self) -> RefKey {
        RefKey { file: self.file, rec: self.rec, si: self.si }
    }
}

impl Index {
    /// The reference a key was taken from.
    pub fn ref_of(&self, k: RefKey) -> RefV<'_> {
        RefV { file: k.file, rec: k.rec, ix: self, si: k.si }
    }

    /// Assemble an index from a base segment and an optional overlay. `dead_base_files` are
    /// local file ids of the base that the overlay replaces or that were deleted.
    pub fn assemble(root: PathBuf, base: Segment, overlay: Option<Segment>, dead_base_files: &[u32]) -> Index {
        let (nf, nd) = (base.n_files(), base.defs().len() as u32);
        let mut b = Seg::new(base, 0, 0);
        for &f in dead_base_files {
            b.kill(f);
        }
        let mut segs = vec![b];
        if let Some(o) = overlay {
            segs.push(Seg::new(o, nf, nd));
        }
        Index { root, segs, src: Mutex::new(FxHashMap::default()) }
    }

    pub fn n_segments(&self) -> usize {
        self.segs.len()
    }

    pub fn segment(&self, i: usize) -> &Segment {
        &self.segs[i].seg
    }

    /// Number of files in the overlay (0 when the base is current).
    pub fn overlay_files(&self) -> u32 {
        self.segs.get(1).map(|s| s.seg.n_files()).unwrap_or(0)
    }

    #[inline]
    fn seg_of_file(&self, f: FileId) -> (usize, u32) {
        if self.segs.len() > 1 && f.0 >= self.segs[1].file_base { (1, f.0 - self.segs[1].file_base) } else { (0, f.0) }
    }

    #[inline]
    fn seg_of_def(&self, d: DefId) -> (usize, u32) {
        if self.segs.len() > 1 && d.0 >= self.segs[1].def_base { (1, d.0 - self.segs[1].def_base) } else { (0, d.0) }
    }

    // ----- files -----

    /// Size of the file id space. Ids below it may be dead: check `is_live`.
    pub fn n_files(&self) -> u32 {
        self.segs.iter().map(|s| s.seg.n_files()).sum()
    }

    pub fn is_live(&self, f: FileId) -> bool {
        let (si, l) = self.seg_of_file(f);
        !self.segs[si].is_dead(l)
    }

    /// All live files.
    pub fn files(&self) -> impl Iterator<Item = FileId> + '_ {
        (0..self.n_files()).map(FileId).filter(move |&f| self.is_live(f))
    }

    pub fn file(&self, f: FileId) -> &FileRec {
        let (si, l) = self.seg_of_file(f);
        &self.segs[si].seg.files()[l as usize]
    }

    pub fn path(&self, f: FileId) -> &str {
        let (si, l) = self.seg_of_file(f);
        self.segs[si].seg.path(l)
    }

    pub fn lang(&self, f: FileId) -> Lang {
        Lang::from_u8(self.file(f).lang)
    }

    pub fn file_by_path(&self, path: &str) -> Option<FileId> {
        for s in self.segs.iter().rev() {
            if let Some(l) = s.seg.file_by_path(path) {
                if !s.is_dead(l) {
                    return Some(FileId(s.file_base + l));
                }
            }
        }
        None
    }

    pub fn is_test_file(&self, f: FileId) -> bool {
        self.file(f).flags & fflag::TEST != 0
    }

    /// File contents, read from disk once per process.
    pub fn source(&self, f: FileId) -> Option<Arc<Vec<u8>>> {
        let mut cache = self.src.lock().unwrap();
        if let Some(hit) = cache.get(&f.0) {
            return hit.clone();
        }
        let data = std::fs::read(self.root.join(self.path(f))).ok().map(Arc::new);
        if cache.len() > 4096 {
            cache.clear();
        }
        cache.insert(f.0, data.clone());
        data
    }

    // ----- names -----

    pub fn name_id(&self, s: &[u8]) -> Option<NameId> {
        let mut ids = [NO; MAX_SEGS];
        let mut any = false;
        for (i, seg) in self.segs.iter().enumerate() {
            if let Some(n) = seg.seg.name_id(s) {
                ids[i] = n;
                any = true;
            }
        }
        if any { Some(NameId(ids)) } else { None }
    }

    pub fn name(&self, n: NameId) -> &[u8] {
        for (i, seg) in self.segs.iter().enumerate() {
            if n.0[i] != NO {
                return seg.seg.name(n.0[i]);
            }
        }
        b""
    }

    /// Bytes of postings a name has: proportional to the number of files mentioning it.
    pub fn popularity(&self, n: NameId) -> u32 {
        self.segs.iter().enumerate().map(|(i, s)| if n.0[i] == NO { 0 } else { s.seg.posting_len(n.0[i]) }).sum()
    }

    /// Scan every segment's lexicon. `scan` receives a name blob with its offsets and reports
    /// matching local ids; the result is each matching name once, as a `NameId`.
    pub fn scan_lexicon(&self, mut scan: impl FnMut(&[u8], &[u32], &mut Vec<u32>)) -> Vec<NameId> {
        let mut out: Vec<NameId> = Vec::new();
        let mut local: Vec<u32> = Vec::new();
        for (i, s) in self.segs.iter().enumerate() {
            let (blob, offs) = s.seg.names_raw();
            local.clear();
            scan(blob, offs, &mut local);
            for &l in &local {
                let mut ids = [NO; MAX_SEGS];
                ids[i] = l;
                let mut dup = false;
                if self.segs.len() > 1 {
                    let name = s.seg.name(l);
                    for (j, o) in self.segs.iter().enumerate() {
                        if j == i {
                            continue;
                        }
                        if let Some(x) = o.seg.name_id(name) {
                            // A name known to an earlier segment was already reported from there.
                            if j < i {
                                dup = true;
                                break;
                            }
                            ids[j] = x;
                        }
                    }
                }
                if !dup {
                    out.push(NameId(ids));
                }
            }
        }
        out
    }

    // ----- definitions -----

    pub fn n_defs(&self) -> u32 {
        self.segs.iter().map(|s| s.seg.defs().len() as u32).sum()
    }

    pub fn def(&self, id: DefId) -> DefV<'_> {
        let (si, l) = self.seg_of_def(id);
        DefV { id, rec: self.segs[si].seg.def(l), ix: self, si: si as u8 }
    }

    pub fn defs_named(&self, name: &[u8]) -> Vec<DefId> {
        match self.name_id(name) {
            Some(n) => self.defs_named_id(n).collect(),
            None => Vec::new(),
        }
    }

    pub fn defs_named_id(&self, n: NameId) -> impl Iterator<Item = DefId> + '_ {
        self.segs.iter().enumerate().flat_map(move |(i, s)| {
            let list: &[u32] = if n.0[i] == NO { &[] } else { s.seg.defs_named(n.0[i]) };
            list.iter().filter(move |&&d| !s.any_dead || !s.is_dead(s.seg.file_of_def(d))).map(move |&d| DefId(s.def_base + d))
        })
    }

    pub fn defs_of_file(&self, f: FileId) -> impl Iterator<Item = DefV<'_>> + '_ {
        let (si, l) = self.seg_of_file(f);
        let s = &self.segs[si];
        let (base, recs) = s.seg.defs_of_file(l);
        recs.iter().enumerate().map(move |(i, rec)| DefV { id: DefId(s.def_base + base + i as u32), rec, ix: self, si: si as u8 })
    }

    /// Innermost definition of `f` whose range contains `line`.
    pub fn def_at(&self, f: FileId, line: u32) -> Option<DefV<'_>> {
        let mut best: Option<DefV<'_>> = None;
        for d in self.defs_of_file(f) {
            if d.rec.line <= line && line <= d.rec.end_line {
                let better = match &best {
                    Some(b) => (d.rec.end_line - d.rec.line) <= (b.rec.end_line - b.rec.line),
                    None => true,
                };
                if better {
                    best = Some(d);
                }
            }
        }
        best
    }

    // ----- references -----

    /// All references in a file, in (name, line) order.
    pub fn refs_of_file(&self, f: FileId) -> impl Iterator<Item = RefV<'_>> + '_ {
        let (si, l) = self.seg_of_file(f);
        self.segs[si].seg.refs_of_file(l).map(move |rec| RefV { file: f, rec, ix: self, si: si as u8 })
    }

    /// Posting list of a name: live files that mention it, with occurrence counts.
    pub fn postings(&self, n: NameId) -> impl Iterator<Item = (FileId, u8)> + '_ {
        self.segs
            .iter()
            .enumerate()
            .flat_map(move |(i, s)| s.seg.postings(n.0[i]).filter(move |&(f, _)| !s.is_dead(f)).map(move |(f, tf)| (FileId(s.file_base + f), tf)))
    }

    /// Live files that mention a name anywhere in their text or facts.
    pub fn files_with(&self, n: NameId) -> impl Iterator<Item = FileId> + '_ {
        self.postings(n).map(|(f, _)| f)
    }

    /// Visit every reference to a name across the repository.
    pub fn for_each_ref_named<'a>(&'a self, n: NameId, mut f: impl FnMut(RefV<'a>)) {
        for (i, s) in self.segs.iter().enumerate() {
            let name = n.0[i];
            if name == NO {
                continue;
            }
            for file in s.seg.files_with(name) {
                if s.is_dead(file) {
                    continue;
                }
                for rec in s.seg.refs_of_file(file) {
                    if rec.name == name {
                        f(RefV { file: FileId(s.file_base + file), rec, ix: self, si: i as u8 });
                    } else if rec.name > name {
                        break;
                    }
                }
            }
        }
    }

    pub fn imports_of_file(&self, f: FileId) -> impl Iterator<Item = ImportRec> + '_ {
        let (si, l) = self.seg_of_file(f);
        self.segs[si].seg.imports_of_file(l)
    }

    /// Text of a name id found in a record of file `f` (ids in records are local to its segment).
    pub fn local_name(&self, f: FileId, id: u32) -> Option<&[u8]> {
        if id == NO {
            return None;
        }
        let (si, _) = self.seg_of_file(f);
        Some(self.segs[si].seg.name(id))
    }
}

impl<'a> DefV<'a> {
    #[inline]
    fn seg(&self) -> &'a Seg {
        &self.ix.segs[self.si as usize]
    }

    #[inline]
    fn local(&self) -> u32 {
        self.id.0 - self.seg().def_base
    }

    pub fn name(&self) -> &'a [u8] {
        self.seg().seg.name(self.rec.name)
    }

    pub fn name_str(&self) -> Cow<'a, str> {
        String::from_utf8_lossy(self.name())
    }

    pub fn kind(&self) -> DefKind {
        DefKind::from_u8(self.rec.kind)
    }

    pub fn flags(&self) -> u8 {
        self.rec.flags
    }

    pub fn is_decl(&self) -> bool {
        self.rec.flags & dflag::DECL != 0
    }

    pub fn is_local(&self) -> bool {
        self.rec.flags & dflag::LOCAL != 0
    }

    pub fn file(&self) -> FileId {
        let s = self.seg();
        FileId(s.file_base + s.seg.file_of_def(self.local()))
    }

    pub fn path(&self) -> &'a str {
        let s = self.seg();
        s.seg.path(s.seg.file_of_def(self.local()))
    }

    pub fn parent(&self) -> Option<DefV<'a>> {
        let p = self.rec.parent;
        if p == NO || p & PARENT_IS_NAME != 0 {
            None
        } else {
            let s = self.seg();
            Some(DefV { id: DefId(s.def_base + p), rec: s.seg.def(p), ix: self.ix, si: self.si })
        }
    }

    /// Name of the container: the parent definition's name, or the by-name qualifier.
    pub fn parent_name(&self) -> Option<&'a [u8]> {
        let p = self.rec.parent;
        let s = &self.seg().seg;
        if p == NO {
            None
        } else if p & PARENT_IS_NAME != 0 {
            Some(s.name(p & !PARENT_IS_NAME))
        } else {
            Some(s.name(s.def(p).name))
        }
    }

    pub fn type_name(&self) -> Option<&'a [u8]> {
        if self.rec.type_name == NO { None } else { Some(self.seg().seg.name(self.rec.type_name)) }
    }

    /// `Parent.name` when the definition has a container, else `name`.
    pub fn qualified(&self) -> String {
        match self.parent_name() {
            Some(p) => format!("{}.{}", String::from_utf8_lossy(p), self.name_str()),
            None => self.name_str().into_owned(),
        }
    }

    /// The signature text, whitespace-normalised, read from the source file.
    pub fn signature(&self) -> String {
        let Some(src) = self.ix.source(self.file()) else { return self.name_str().into_owned() };
        let s = self.rec.sig_off as usize;
        let e = (s + self.rec.sig_len as usize).min(src.len());
        if s >= e {
            return self.name_str().into_owned();
        }
        let sig = normalize_ws(&src[s..e]);
        let name = self.name();
        // A stale file would yield unrelated text: the signature must mention the name
        // (macro-defined functions carry a derived name, so they are exempt).
        if self.rec.flags & dflag::MACRO_DEFINED == 0 && memchr::memmem::find(sig.as_bytes(), name).is_none() {
            return self.name_str().into_owned();
        }
        sig
    }
}

impl<'a> RefV<'a> {
    #[inline]
    fn seg(&self) -> &'a Seg {
        &self.ix.segs[self.si as usize]
    }

    pub fn name(&self) -> &'a [u8] {
        self.seg().seg.name(self.rec.name)
    }

    pub fn qual(&self) -> Option<&'a [u8]> {
        if self.rec.qual == NO { None } else { Some(self.seg().seg.name(self.rec.qual)) }
    }

    pub fn kind(&self) -> RefKind {
        RefKind::from_u8(self.rec.kind)
    }

    pub fn line(&self) -> u32 {
        self.rec.line
    }

    /// The innermost definition enclosing this reference, if any.
    pub fn encl(&self) -> Option<DefV<'a>> {
        if self.rec.encl == NO {
            return None;
        }
        let s = self.seg();
        let f = &s.seg.files()[(self.file.0 - s.file_base) as usize];
        if self.rec.encl >= f.def_count {
            return None;
        }
        let local = f.def_start + self.rec.encl;
        Some(DefV { id: DefId(s.def_base + local), rec: s.seg.def(local), ix: self.ix, si: self.si })
    }

    pub fn path(&self) -> &'a str {
        self.ix.path(self.file)
    }
}

/// Collapse whitespace runs to single spaces and trim a trailing `{`, `=` or line continuation.
pub fn normalize_ws(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for ch in text.chars() {
        if ch.is_whitespace() || ch == '\\' && out.ends_with(' ') {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(ch);
        }
    }
    let trimmed = out.trim_end_matches(|c: char| c == '{' || c == '=' || c == '\\' || c.is_whitespace());
    trimmed.to_string()
}
