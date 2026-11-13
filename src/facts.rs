//! The fact model: what an extractor records about one source file.
//!
//! Facts are *name-linked*, like object files before linking: a file's facts mention other
//! symbols only by name (plus an optional qualifier). Nothing in one file's facts points into
//! another file, so any file can be re-extracted alone and the "linking" happens at query time.

use hashbrown::HashTable;
use rustc_hash::FxHasher;
use std::hash::Hasher;

pub const NONE: u32 = u32::MAX;

// ---------------------------------------------------------------------------------------------
// Kinds
// ---------------------------------------------------------------------------------------------

/// Kind of a definition. Values are stored on disk: only append.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum DefKind {
    Function = 1,
    Method = 2,
    Class = 3,
    Struct = 4,
    Union = 5,
    Enum = 6,
    EnumMember = 7,
    Interface = 8,
    Trait = 9,
    TypeAlias = 10,
    Field = 11,
    Variable = 12,
    Constant = 13,
    Macro = 14,
    Module = 15,
    Namespace = 16,
    Property = 17,
    Impl = 18,
}

impl DefKind {
    pub fn from_u8(v: u8) -> DefKind {
        match v {
            1 => DefKind::Function,
            2 => DefKind::Method,
            3 => DefKind::Class,
            4 => DefKind::Struct,
            5 => DefKind::Union,
            6 => DefKind::Enum,
            7 => DefKind::EnumMember,
            8 => DefKind::Interface,
            9 => DefKind::Trait,
            10 => DefKind::TypeAlias,
            11 => DefKind::Field,
            12 => DefKind::Variable,
            13 => DefKind::Constant,
            14 => DefKind::Macro,
            15 => DefKind::Module,
            16 => DefKind::Namespace,
            17 => DefKind::Property,
            18 => DefKind::Impl,
            _ => DefKind::Variable,
        }
    }

    /// Short tag used in output. Kept to few characters: every byte of output is paid in tokens.
    pub fn tag(self) -> &'static str {
        match self {
            DefKind::Function => "fn",
            DefKind::Method => "method",
            DefKind::Class => "class",
            DefKind::Struct => "struct",
            DefKind::Union => "union",
            DefKind::Enum => "enum",
            DefKind::EnumMember => "enumval",
            DefKind::Interface => "iface",
            DefKind::Trait => "trait",
            DefKind::TypeAlias => "type",
            DefKind::Field => "field",
            DefKind::Variable => "var",
            DefKind::Constant => "const",
            DefKind::Macro => "macro",
            DefKind::Module => "mod",
            DefKind::Namespace => "ns",
            DefKind::Property => "prop",
            DefKind::Impl => "impl",
        }
    }

    pub fn parse(s: &str) -> Option<Vec<DefKind>> {
        Some(match s {
            "fn" | "func" | "function" => vec![DefKind::Function, DefKind::Method],
            "method" => vec![DefKind::Method],
            "class" => vec![DefKind::Class],
            "struct" => vec![DefKind::Struct, DefKind::Union],
            "union" => vec![DefKind::Union],
            "enum" => vec![DefKind::Enum],
            "enumval" | "enumerator" => vec![DefKind::EnumMember],
            "iface" | "interface" => vec![DefKind::Interface],
            "trait" => vec![DefKind::Trait],
            "type" => vec![DefKind::TypeAlias, DefKind::Class, DefKind::Struct, DefKind::Union, DefKind::Enum, DefKind::Interface, DefKind::Trait],
            "alias" | "typedef" => vec![DefKind::TypeAlias],
            "field" => vec![DefKind::Field, DefKind::Property],
            "var" | "variable" => vec![DefKind::Variable],
            "const" | "constant" => vec![DefKind::Constant, DefKind::EnumMember],
            "macro" => vec![DefKind::Macro],
            "mod" | "module" => vec![DefKind::Module, DefKind::Namespace],
            "ns" | "namespace" => vec![DefKind::Namespace],
            "prop" | "property" => vec![DefKind::Property],
            "impl" => vec![DefKind::Impl],
            _ => return None,
        })
    }

    pub fn is_callable(self) -> bool {
        matches!(self, DefKind::Function | DefKind::Method | DefKind::Macro)
    }

    pub fn is_type(self) -> bool {
        matches!(self, DefKind::Class | DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::Interface | DefKind::Trait | DefKind::TypeAlias)
    }

    /// Containers own members (fields, methods, enumerators).
    pub fn is_container(self) -> bool {
        matches!(
            self,
            DefKind::Class
                | DefKind::Struct
                | DefKind::Union
                | DefKind::Enum
                | DefKind::Interface
                | DefKind::Trait
                | DefKind::Impl
                | DefKind::Module
                | DefKind::Namespace
                | DefKind::TypeAlias
        )
    }
}

/// Definition flags (bit set, stored on disk).
pub mod dflag {
    /// Declaration without a body (prototype, extern, forward declaration, interface method).
    pub const DECL: u8 = 1;
    /// Not visible outside its file / module (C `static`, non-exported JS, private).
    pub const LOCAL: u8 = 2;
    /// Explicitly exported / public API.
    pub const EXPORTED: u8 = 4;
    /// Lives in test code or is itself a test.
    pub const TEST: u8 = 8;
    /// Synthesised from a macro invocation (e.g. `SYSCALL_DEFINE3(read, ...)`).
    pub const MACRO_DEFINED: u8 = 16;
    /// A slot that holds a callable (function-pointer field, callback property).
    pub const FNPTR: u8 = 32;
    /// Abstract / virtual / overridable.
    pub const ABSTRACT: u8 = 64;
    /// Async, generator or otherwise not a plain synchronous callable.
    pub const ASYNC: u8 = 128;
}

/// Kind of a reference. Values are stored on disk (4 bits): only append, max 15.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum RefKind {
    /// Direct call by bare or qualified name: `foo(x)`, `pkg.Foo(x)`.
    Call = 1,
    /// Call through a member access: `x.f()`, `p->f()`. `qual` carries the receiver type path.
    MemberCall = 2,
    /// Use as a type.
    Type = 3,
    /// Read of a non-local value (also: function used as a value).
    Read = 4,
    /// Write to a non-local value.
    Write = 5,
    /// Read of a member: `x.f`.
    MemberRead = 6,
    /// Write of a member: `x.f = v`, `.f = v` in an initializer.
    MemberWrite = 7,
    /// A value bound into a slot: `.read = foo_read`, `ops->read = foo_read`, `{ read: fooRead }`.
    /// `name` is the bound value, `qual` is the slot (`T.f`).
    Bind = 8,
    /// Imported name.
    Import = 9,
    /// Inherits from / embeds.
    Extends = 10,
    /// Implements an interface or trait.
    Implements = 11,
    /// Instantiation: `new T()`, `T{...}`, `T(...)` when T is known to be a type.
    New = 12,
    /// Decorator / annotation / attribute use.
    Annotation = 13,
    /// Macro invocation that is syntactically distinct (`foo!()` in Rust).
    MacroCall = 14,
    /// A non-local name passed as a call argument: `request_irq(irq, handler, ...)`. `qual` is
    /// the name of the function receiving it. For functions this is a callback registration.
    Arg = 15,
}

impl RefKind {
    pub fn from_u8(v: u8) -> RefKind {
        match v {
            1 => RefKind::Call,
            2 => RefKind::MemberCall,
            3 => RefKind::Type,
            4 => RefKind::Read,
            5 => RefKind::Write,
            6 => RefKind::MemberRead,
            7 => RefKind::MemberWrite,
            8 => RefKind::Bind,
            9 => RefKind::Import,
            10 => RefKind::Extends,
            11 => RefKind::Implements,
            12 => RefKind::New,
            13 => RefKind::Annotation,
            14 => RefKind::MacroCall,
            15 => RefKind::Arg,
            _ => RefKind::Read,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            RefKind::Call => "call",
            RefKind::MemberCall => "mcall",
            RefKind::Type => "type",
            RefKind::Read => "read",
            RefKind::Write => "write",
            RefKind::MemberRead => "mread",
            RefKind::MemberWrite => "mwrite",
            RefKind::Bind => "bind",
            RefKind::Import => "import",
            RefKind::Extends => "extends",
            RefKind::Implements => "impl",
            RefKind::New => "new",
            RefKind::Annotation => "annot",
            RefKind::MacroCall => "macro",
            RefKind::Arg => "arg",
        }
    }

    pub fn is_call(self) -> bool {
        matches!(self, RefKind::Call | RefKind::MemberCall | RefKind::New | RefKind::MacroCall)
    }

    pub fn is_member(self) -> bool {
        matches!(self, RefKind::MemberCall | RefKind::MemberRead | RefKind::MemberWrite)
    }
}

// ---------------------------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------------------------

/// A definition. `name`, `type_name` are name ids (file-local while extracting, global after).
/// `parent` is the index of the containing definition within the same file, or `NONE`.
#[derive(Clone, Copy, Debug)]
pub struct Def {
    pub name: u32,
    pub parent: u32,
    /// Name id of the container when it is not a definition in this file (Go receiver type,
    /// C++ out-of-line `Class::method`), else `NONE`. Only meaningful when `parent == NONE`.
    pub qual: u32,
    /// 1-based first line.
    pub line: u32,
    /// 1-based last line, inclusive.
    pub end_line: u32,
    /// Byte offset of the signature within the file.
    pub sig_off: u32,
    /// Signature length in bytes (everything before the body), clamped.
    pub sig_len: u16,
    pub kind: DefKind,
    pub flags: u8,
    /// Name id of the declared type (variables, fields), the return type (functions) or the
    /// aliased type (type aliases); `NONE` if unknown or primitive.
    pub type_name: u32,
    /// Minimum and maximum number of parameters; `arity_max == 255` means variadic.
    pub arity_min: u8,
    pub arity_max: u8,
}

/// A reference to a non-local name.
#[derive(Clone, Copy, Debug)]
pub struct Ref {
    pub name: u32,
    /// Qualifier name id or `NONE`. For member refs: the receiver's type path (`T`, `T::f`,
    /// `$global`, `f()`); for `Bind`: the slot (`T.f`); for qualified calls: the module / type.
    pub qual: u32,
    pub line: u32,
    /// Index of the innermost enclosing definition in the same file, or `NONE` at file scope.
    pub encl: u32,
    pub kind: RefKind,
    /// Number of call arguments (calls only), 255 if unknown.
    pub nargs: u8,
}

/// Import flags.
pub mod iflag {
    /// `#include <...>`, or an import that names an external package.
    pub const SYSTEM: u8 = 1;
    /// Wildcard import (`from x import *`, `import x.*`).
    pub const WILDCARD: u8 = 2;
    /// Re-export (`export * from`, `pub use`).
    pub const REEXPORT: u8 = 4;
}

/// An import / include.
#[derive(Clone, Copy, Debug)]
pub struct Import {
    /// Name id of the target as written (`linux/fs.h`, `a.b.c`, `./util`).
    pub target: u32,
    /// Imported symbol name id, or `NONE` when the whole module / file is imported.
    pub name: u32,
    /// Local alias name id, or `NONE`.
    pub alias: u32,
    pub line: u32,
    pub flags: u8,
}

// ---------------------------------------------------------------------------------------------
// Per-file name interner
// ---------------------------------------------------------------------------------------------

#[inline]
pub fn hash_bytes(b: &[u8]) -> u64 {
    let mut h = FxHasher::default();
    h.write(b);
    h.finish()
}

/// Arena-backed interner for the names seen in one file. Reused across files by `clear`.
#[derive(Default)]
pub struct LocalNames {
    buf: Vec<u8>,
    spans: Vec<(u32, u32)>,
    table: HashTable<u32>,
}

impl LocalNames {
    pub fn clear(&mut self) {
        self.buf.clear();
        self.spans.clear();
        self.table.clear();
    }

    #[inline]
    pub fn intern(&mut self, s: &[u8]) -> u32 {
        let h = hash_bytes(s);
        let Self { buf, spans, table } = self;
        let get = |i: u32| -> &[u8] {
            let (o, l) = spans[i as usize];
            &buf[o as usize..(o + l) as usize]
        };
        if let Some(&i) = table.find(h, |&i| get(i) == s) {
            return i;
        }
        let i = spans.len() as u32;
        let off = buf.len() as u32;
        buf.extend_from_slice(s);
        spans.push((off, s.len() as u32));
        let (buf, spans) = (&*buf, &*spans);
        table.insert_unique(h, i, |&j| {
            let (o, l) = spans[j as usize];
            hash_bytes(&buf[o as usize..(o + l) as usize])
        });
        i
    }

    #[inline]
    pub fn get(&self, i: u32) -> &[u8] {
        let (o, l) = self.spans[i as usize];
        &self.buf[o as usize..(o + l) as usize]
    }

    pub fn len(&self) -> usize {
        self.spans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

// ---------------------------------------------------------------------------------------------
// File facts
// ---------------------------------------------------------------------------------------------

/// Everything extracted from one file. Name ids are local (`names`) until `globalize`.
#[derive(Default)]
pub struct FileFacts {
    pub names: LocalNames,
    pub defs: Vec<Def>,
    pub refs: Vec<Ref>,
    pub imports: Vec<Import>,
    pub nlines: u32,
    /// Number of tree-sitter ERROR / MISSING nodes: a quality signal for this file's facts.
    pub parse_errors: u32,
}

impl FileFacts {
    pub fn clear(&mut self) {
        self.names.clear();
        self.defs.clear();
        self.refs.clear();
        self.imports.clear();
        self.nlines = 0;
        self.parse_errors = 0;
    }
}

/// Facts with global name ids, ready for the index writer. Owned and compact.
pub struct FileOut {
    pub path: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub hash: u64,
    pub lang: u8,
    pub flags: u8,
    pub nlines: u32,
    pub parse_errors: u32,
    pub defs: Vec<Def>,
    pub refs: Vec<Ref>,
    pub imports: Vec<Import>,
    /// Words occurring in the file's text: (name id, occurrences capped at 255).
    pub words: Vec<(u32, u8)>,
}

/// File flags (stored on disk).
pub mod fflag {
    /// Test code (path or content heuristics).
    pub const TEST: u8 = 1;
    /// Vendored / third-party code.
    pub const VENDOR: u8 = 2;
    /// Generated code.
    pub const GENERATED: u8 = 4;
    /// Header / declaration file.
    pub const HEADER: u8 = 8;
    /// Not parsed (unsupported language, too large or binary): only path and text are searchable.
    pub const UNPARSED: u8 = 16;
    /// Documentation / prose.
    pub const DOC: u8 = 32;
}
