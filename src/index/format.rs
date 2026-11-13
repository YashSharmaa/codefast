//! On-disk segment format.
//!
//! A segment is one file: a header, a section table, then 8-byte aligned sections that are read
//! in place through a memory map. Fixed-size records are plain little-endian structs; variable
//! data (reference blocks, postings) is LEB128-encoded.
//!
//! ```text
//! META  key=value lines (root, counts, tool version)
//! NSTR  name bytes, sorted          NOFF  u32 offsets (names + 1)
//! PSTR  path bytes, sorted          POFF  u32 offsets (files + 1)
//! FILE  FileRec per file            DEFS  DefRec per definition, grouped by file
//! DNAM  u32 def ids sorted by name
//! RBLK  per-file reference blocks   IBLK  per-file import blocks
//! WOFF  u32 offsets (names + 1)     WDAT  per-name delta-coded file ids ("which files mention X")
//! ```

use bytemuck::{Pod, Zeroable};

pub const MAGIC: [u8; 8] = *b"CFSEG\0\0\x01";
pub const VERSION: u32 = 3;

/// High bit of `DefRec::parent`: the remaining bits are a name id (container known by name only).
pub const PARENT_IS_NAME: u32 = 0x8000_0000;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct Header {
    pub magic: [u8; 8],
    pub version: u32,
    pub nsections: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct SectionEntry {
    pub tag: [u8; 4],
    pub _pad: u32,
    pub offset: u64,
    pub len: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct FileRec {
    pub size: u64,
    pub mtime_ns: i64,
    pub hash: u64,
    pub ref_off: u64,
    pub imp_off: u64,
    pub ref_len: u32,
    pub imp_len: u32,
    pub def_start: u32,
    pub def_count: u32,
    pub nlines: u32,
    pub parse_errors: u32,
    pub lang: u8,
    pub flags: u8,
    pub _pad: [u8; 6],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct DefRec {
    pub name: u32,
    /// Global def id of the container, `PARENT_IS_NAME | name id`, or `u32::MAX`.
    pub parent: u32,
    pub line: u32,
    pub end_line: u32,
    pub sig_off: u32,
    pub type_name: u32,
    pub sig_len: u16,
    pub kind: u8,
    pub flags: u8,
    pub arity_min: u8,
    pub arity_max: u8,
    pub _pad: u16,
}

const _: () = assert!(std::mem::size_of::<DefRec>() == 32);
const _: () = assert!(std::mem::size_of::<FileRec>() == 72);

// ---------------------------------------------------------------------------------------------
// LEB128
// ---------------------------------------------------------------------------------------------

#[inline]
pub fn put_varint(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Decode one varint at `*pos`, advancing it. Malformed input yields garbage, never a panic.
#[inline]
pub fn get_varint(buf: &[u8], pos: &mut usize) -> u32 {
    let mut v = 0u32;
    let mut shift = 0u32;
    while let Some(&b) = buf.get(*pos) {
        *pos += 1;
        v |= ((b & 0x7f) as u32) << shift;
        if b < 0x80 || shift >= 28 {
            break;
        }
        shift += 7;
    }
    v
}

// ---------------------------------------------------------------------------------------------
// Reference blocks
// ---------------------------------------------------------------------------------------------
//
// block := varint(count) ref*
// ref   := head:u8 [varint(name delta)] varint(line) [varint(encl + 1)] [varint(qual)] [nargs:u8]
//
// head bits: 0-3 kind, 4 same name as previous, 5 same enclosing def as previous,
//            6 has qualifier, 7 has argument count.
// Refs are sorted by (name, line); with "same name", `line` is a delta from the previous line.

pub const RH_SAME_NAME: u8 = 1 << 4;
pub const RH_SAME_ENCL: u8 = 1 << 5;
pub const RH_HAS_QUAL: u8 = 1 << 6;
pub const RH_HAS_NARGS: u8 = 1 << 7;

/// A decoded reference. `encl` is an index into the file's definitions, or `u32::MAX`.
#[derive(Clone, Copy, Debug)]
pub struct RefRec {
    pub name: u32,
    pub qual: u32,
    pub line: u32,
    pub encl: u32,
    pub kind: u8,
    pub nargs: u8,
}

pub struct RefIter<'a> {
    buf: &'a [u8],
    pos: usize,
    left: u32,
    name: u32,
    line: u32,
    encl: u32,
}

impl<'a> RefIter<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        let mut pos = 0;
        let left = if buf.is_empty() { 0 } else { get_varint(buf, &mut pos) };
        RefIter { buf, pos, left, name: 0, line: 0, encl: u32::MAX }
    }

    pub fn remaining(&self) -> u32 {
        self.left
    }
}

impl<'a> Iterator for RefIter<'a> {
    type Item = RefRec;

    #[inline]
    fn next(&mut self) -> Option<RefRec> {
        if self.left == 0 || self.pos >= self.buf.len() {
            return None;
        }
        self.left -= 1;
        let head = self.buf[self.pos];
        self.pos += 1;
        if head & RH_SAME_NAME != 0 {
            self.line += get_varint(self.buf, &mut self.pos);
        } else {
            self.name += get_varint(self.buf, &mut self.pos);
            self.line = get_varint(self.buf, &mut self.pos);
        }
        if head & RH_SAME_ENCL == 0 {
            self.encl = get_varint(self.buf, &mut self.pos).wrapping_sub(1);
        }
        let qual = if head & RH_HAS_QUAL != 0 { get_varint(self.buf, &mut self.pos) } else { u32::MAX };
        let nargs = if head & RH_HAS_NARGS != 0 {
            let b = self.buf.get(self.pos).copied().unwrap_or(255);
            self.pos += 1;
            b
        } else {
            255
        };
        Some(RefRec { name: self.name, qual, line: self.line, encl: self.encl, kind: head & 0x0f, nargs })
    }
}

/// Encode refs that are already sorted by (name, line).
pub fn encode_refs(refs: &[RefRec], out: &mut Vec<u8>) {
    put_varint(out, refs.len() as u32);
    let mut prev_name = 0u32;
    let mut prev_line = 0u32;
    let mut prev_encl = u32::MAX;
    for (i, r) in refs.iter().enumerate() {
        let same_name = i > 0 && r.name == prev_name;
        let same_encl = r.encl == prev_encl;
        let mut head = r.kind & 0x0f;
        if same_name {
            head |= RH_SAME_NAME;
        }
        if same_encl {
            head |= RH_SAME_ENCL;
        }
        if r.qual != u32::MAX {
            head |= RH_HAS_QUAL;
        }
        if r.nargs != 255 {
            head |= RH_HAS_NARGS;
        }
        out.push(head);
        if same_name {
            put_varint(out, r.line - prev_line);
        } else {
            put_varint(out, r.name - prev_name);
            put_varint(out, r.line);
        }
        if !same_encl {
            put_varint(out, r.encl.wrapping_add(1));
        }
        if r.qual != u32::MAX {
            put_varint(out, r.qual);
        }
        if r.nargs != 255 {
            out.push(r.nargs);
        }
        prev_name = r.name;
        prev_line = r.line;
        prev_encl = r.encl;
    }
}

// ---------------------------------------------------------------------------------------------
// Import blocks
// ---------------------------------------------------------------------------------------------
//
// block  := varint(count) import*
// import := varint(target) varint(name + 1) varint(alias + 1) varint(line) flags:u8

#[derive(Clone, Copy, Debug)]
pub struct ImportRec {
    pub target: u32,
    pub name: u32,
    pub alias: u32,
    pub line: u32,
    pub flags: u8,
}

pub fn encode_imports(imports: &[ImportRec], out: &mut Vec<u8>) {
    put_varint(out, imports.len() as u32);
    for im in imports {
        put_varint(out, im.target);
        put_varint(out, im.name.wrapping_add(1));
        put_varint(out, im.alias.wrapping_add(1));
        put_varint(out, im.line);
        out.push(im.flags);
    }
}

pub struct ImportIter<'a> {
    buf: &'a [u8],
    pos: usize,
    left: u32,
}

impl<'a> ImportIter<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        let mut pos = 0;
        let left = if buf.is_empty() { 0 } else { get_varint(buf, &mut pos) };
        ImportIter { buf, pos, left }
    }
}

impl<'a> Iterator for ImportIter<'a> {
    type Item = ImportRec;

    fn next(&mut self) -> Option<ImportRec> {
        if self.left == 0 || self.pos >= self.buf.len() {
            return None;
        }
        self.left -= 1;
        let target = get_varint(self.buf, &mut self.pos);
        let name = get_varint(self.buf, &mut self.pos).wrapping_sub(1);
        let alias = get_varint(self.buf, &mut self.pos).wrapping_sub(1);
        let line = get_varint(self.buf, &mut self.pos);
        let flags = self.buf.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        Some(ImportRec { target, name, alias, line, flags })
    }
}

// ---------------------------------------------------------------------------------------------
// Postings
// ---------------------------------------------------------------------------------------------

/// Iterator over a posting list: ascending file ids with a term frequency each.
///
/// posting := varint(delta << 1 | has_tf) [tf:u8]      (tf is 1 when absent)
pub struct PostingIter<'a> {
    buf: &'a [u8],
    pos: usize,
    cur: u32,
    first: bool,
}

impl<'a> PostingIter<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        PostingIter { buf, pos: 0, cur: 0, first: true }
    }
}

impl<'a> Iterator for PostingIter<'a> {
    type Item = (u32, u8);

    #[inline]
    fn next(&mut self) -> Option<(u32, u8)> {
        if self.pos >= self.buf.len() {
            return None;
        }
        let v = get_varint(self.buf, &mut self.pos);
        let d = v >> 1;
        if self.first {
            self.cur = d;
            self.first = false;
        } else {
            self.cur += d;
        }
        let tf = if v & 1 != 0 {
            let b = self.buf.get(self.pos).copied().unwrap_or(1);
            self.pos += 1;
            b
        } else {
            1
        };
        Some((self.cur, tf))
    }
}

/// Append one posting. `delta` is the gap from the previous file id (the id itself for the first).
#[inline]
pub fn put_posting(out: &mut Vec<u8>, delta: u32, tf: u8) {
    if tf > 1 {
        put_varint(out, delta << 1 | 1);
        out.push(tf);
    } else {
        put_varint(out, delta << 1);
    }
}
