//! Shared machinery for language extractors: the context that collects facts, scope tracking,
//! and helpers for mapping tree-sitter node kinds to compact per-language enums.

use crate::facts::{Def, DefKind, FileFacts, Import, NONE, Ref, RefKind};
use rustc_hash::{FxHashMap, FxHashSet};
use tree_sitter::{Language, Node, TreeCursor};

/// Recursion guard: expression trees in generated code can be extremely deep.
pub const MAX_DEPTH: u32 = 1500;

/// Reusable allocations, one per worker thread.
#[derive(Default)]
pub struct Scratch {
    pub def_stack: Vec<u32>,
    scopes: Vec<FxHashMap<u32, u32>>,
    scope_depth: usize,
    /// Start bytes of nodes that were already accounted for (declaration names, callee names).
    pub handled: FxHashSet<u32>,
    /// Start bytes of identifiers that are assignment targets.
    pub writes: FxHashSet<u32>,
    pub buf: Vec<u8>,
}

impl Scratch {
    pub fn reset(&mut self) {
        self.def_stack.clear();
        self.scope_depth = 0;
        self.handled.clear();
        self.writes.clear();
        self.buf.clear();
    }
}

/// Collects the facts of one file. Language walkers drive it.
pub struct Ctx<'a> {
    pub src: &'a [u8],
    pub facts: &'a mut FileFacts,
    pub s: &'a mut Scratch,
    pub path: &'a str,
    pub is_header: bool,
    pub depth: u32,
}

impl<'a> Ctx<'a> {
    pub fn new(src: &'a [u8], path: &'a str, facts: &'a mut FileFacts, s: &'a mut Scratch) -> Self {
        s.reset();
        Ctx { src, facts, s, path, is_header: false, depth: 0 }
    }

    #[inline]
    pub fn text(&self, n: Node) -> &'a [u8] {
        &self.src[n.start_byte()..n.end_byte()]
    }

    #[inline]
    pub fn intern(&mut self, s: &[u8]) -> u32 {
        self.facts.names.intern(s)
    }

    /// Intern the text of a node.
    #[inline]
    pub fn name(&mut self, n: Node) -> u32 {
        let s = &self.src[n.start_byte()..n.end_byte()];
        self.facts.names.intern(s)
    }

    #[inline]
    pub fn name_bytes(&self, id: u32) -> &[u8] {
        self.facts.names.get(id)
    }

    // ----- definitions -----

    #[inline]
    pub fn encl(&self) -> u32 {
        *self.s.def_stack.last().unwrap_or(&NONE)
    }

    pub fn push_def(&mut self, d: u32) {
        self.s.def_stack.push(d);
    }

    pub fn pop_def(&mut self) {
        self.s.def_stack.pop();
    }

    /// Record a definition spanning `node`, whose signature ends at byte `sig_end`.
    pub fn def(&mut self, name: u32, kind: DefKind, node: Node, sig_end: usize) -> u32 {
        let start = node.start_byte();
        let sig_len = sig_end.saturating_sub(start).min(u16::MAX as usize) as u16;
        self.add_def(Def {
            name,
            parent: NONE,
            qual: NONE,
            line: line_of(node),
            end_line: end_line_of(node),
            sig_off: start as u32,
            sig_len,
            kind,
            flags: 0,
            type_name: NONE,
            arity_min: 0,
            arity_max: 0,
        })
    }

    pub fn add_def(&mut self, d: Def) -> u32 {
        let i = self.facts.defs.len() as u32;
        self.facts.defs.push(d);
        i
    }

    #[inline]
    pub fn def_mut(&mut self, i: u32) -> &mut Def {
        &mut self.facts.defs[i as usize]
    }

    // ----- references -----

    #[inline]
    pub fn add_ref(&mut self, name: u32, kind: RefKind, line: u32, qual: u32, nargs: u8) {
        let encl = self.encl();
        self.facts.refs.push(Ref { name, qual, line, encl, kind, nargs });
    }

    pub fn add_import(&mut self, target: u32, name: u32, alias: u32, line: u32, flags: u8) {
        self.facts.imports.push(Import { target, name, alias, line, flags });
    }

    // ----- local scopes -----

    pub fn push_scope(&mut self) {
        self.s.scope_depth += 1;
        if self.s.scopes.len() < self.s.scope_depth {
            self.s.scopes.push(FxHashMap::default());
        } else {
            self.s.scopes[self.s.scope_depth - 1].clear();
        }
    }

    pub fn pop_scope(&mut self) {
        self.s.scope_depth = self.s.scope_depth.saturating_sub(1);
    }

    #[inline]
    pub fn in_scope(&self) -> bool {
        self.s.scope_depth > 0
    }

    /// Declare a local name with an optional declared type (`NONE` if unknown).
    #[inline]
    pub fn declare(&mut self, name: u32, ty: u32) {
        if self.s.scope_depth > 0 {
            self.s.scopes[self.s.scope_depth - 1].insert(name, ty);
        }
    }

    /// `Some(type)` if `name` is a local in any enclosing scope (type may be `NONE`).
    #[inline]
    pub fn local_type(&self, name: u32) -> Option<u32> {
        for i in (0..self.s.scope_depth).rev() {
            if let Some(&t) = self.s.scopes[i].get(&name) {
                return Some(t);
            }
        }
        None
    }

    #[inline]
    pub fn is_local(&self, name: u32) -> bool {
        self.local_type(name).is_some()
    }

    #[inline]
    pub fn mark(&mut self, n: Node) {
        self.s.handled.insert(n.start_byte() as u32);
    }

    #[inline]
    pub fn is_marked(&self, n: Node) -> bool {
        self.s.handled.contains(&(n.start_byte() as u32))
    }
}

#[inline]
pub fn line_of(n: Node) -> u32 {
    n.start_position().row as u32 + 1
}

/// Last line of a node, inclusive. Nodes that swallow their trailing newline (preprocessor
/// directives) end at column 0 of the following line.
#[inline]
pub fn end_line_of(n: Node) -> u32 {
    let s = n.start_position();
    let e = n.end_position();
    if e.column == 0 && e.row > s.row { e.row as u32 } else { e.row as u32 + 1 }
}

/// First named child of a node.
#[inline]
pub fn first_named<'t>(n: Node<'t>) -> Option<Node<'t>> {
    n.named_child(0)
}

/// Iterate the direct children of the cursor's node, calling `f` with the cursor on each child.
#[inline]
pub fn each_child<'t>(c: &mut TreeCursor<'t>, mut f: impl FnMut(&mut TreeCursor<'t>)) {
    if c.goto_first_child() {
        loop {
            f(c);
            if !c.goto_next_sibling() {
                break;
            }
        }
        c.goto_parent();
    }
}

/// Maps tree-sitter node kind ids to a compact per-language enum with one array lookup.
pub struct KindTable<K: Copy> {
    map: Vec<K>,
    /// Kind for ERROR nodes, whose id (65535) lies outside the grammar's symbol table.
    error: K,
}

impl<K: Copy> KindTable<K> {
    pub fn build(lang: &Language, default: K, pairs: &[(&str, K)]) -> Self {
        let n = lang.node_kind_count();
        let mut map = vec![default; n];
        let lookup: FxHashMap<&str, K> = pairs.iter().cloned().collect();
        for id in 0..n {
            let id16 = id as u16;
            if !lang.node_kind_is_named(id16) {
                continue;
            }
            if let Some(name) = lang.node_kind_for_id(id16) {
                if let Some(&k) = lookup.get(name) {
                    map[id] = k;
                }
            }
        }
        let error = lookup.get("ERROR").copied().unwrap_or(default);
        KindTable { map, error }
    }

    #[inline]
    pub fn get(&self, id: u16) -> K {
        match self.map.get(id as usize) {
            Some(&k) => k,
            None => self.error,
        }
    }
}

/// Field id by name, or 0 if the grammar has no such field (0 never matches a real field).
pub fn field(lang: &Language, name: &str) -> u16 {
    lang.field_id_for_name(name).map(|f| f.get()).unwrap_or(0)
}

/// Child by field id; tolerates the 0 "no such field" sentinel.
#[inline]
pub fn child<'t>(n: Node<'t>, field_id: u16) -> Option<Node<'t>> {
    if field_id == 0 { None } else { n.child_by_field_id(field_id) }
}

/// True when the cursor's current node is attached to its parent through `field_id`.
#[inline]
pub fn is_field(c: &TreeCursor, field_id: u16) -> bool {
    c.field_id().map(|f| f.get()) == Some(field_id) && field_id != 0
}
