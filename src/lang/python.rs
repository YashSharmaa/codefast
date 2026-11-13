//! Python extractor (tree-sitter-python).
//!
//! Python declares nothing, so the walker reconstructs what a reader infers from the code:
//!   * module-level assignments are variables (constants when ALL_CAPS), class-level ones and
//!     `self.x = ...` in methods are fields; the first binding is the definition, later ones
//!     are writes;
//!   * every name a function binds is one of its locals, wherever the binding statement sits:
//!     a statement-level pre-scan hoists them before the body is walked, so that closures and
//!     loops reading a name above its assignment do not leak references;
//!   * receivers are typed from annotations, constructor calls, literals, `self` / `cls` and
//!     plain aliases of those, so that `x.m()` can be linked to the class that owns `m`. An
//!     inferred type holds only while every assignment seen so far agrees on it;
//!   * names bound by imports are tracked per file (per function for local imports):
//!     `alias.f()` on an imported module is a call qualified by the module path as written.
//!
//! Qualifiers (`Ref::qual`):
//!   * `os.path`: a module path, the import target as written plus the attribute chain. `import
//!     os` then `os.path.join()`; `import numpy as np` then `np.linalg.norm()` (`numpy.linalg`).
//!     A lower-case `from m import name` binds a module or an object of `m`: `name.f()` has
//!     qualifier `m.name` (`from . import helpers` gives `.helpers`);
//!   * `Class`: `Class.f()` (a Call), `x.f()` with `x: Class` and `self.f()` (MemberCalls);
//!     `TARGET|Class` when the class is written through a module (`models.User.create()`);
//!   * `Class::attr` for `self.attr.f()`, `$var::attr` for a module-level `var`, `f()` for the
//!     result of a call, `Class::m()` for the result of a method, `TARGET|f()` and
//!     `TARGET|$CONST` for a function and a constant of a module, `^Class` for `super()`
//!     inside `Class`;
//!   * `Class.slot` on a Bind: `self.slot = handler`, or `slot = handler` in a class body.
//!
//! Type names (`Def::type_name`, roots of the paths above) written through a module keep it:
//! `x: models.User` after `import app.models as models` is `app.models|User`.
//!
//! Arity counts the parameters a call may name (keyword-only ones included) without a leading
//! `self` / `cls`; `nargs` counts positional and keyword arguments alike, so the two compare.

use crate::extract::*;
use crate::facts::*;
use crate::lang::Lang;
use rustc_hash::FxHashMap;
use std::marker::PhantomData;
use std::sync::OnceLock;
use tree_sitter::{Node, Tree, TreeCursor};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    Other,
    Skip,
    Error,
    Comment,
    Identifier,
    Attribute,
    Call,
    Subscript,
    Assignment,
    AugmentedAssignment,
    NamedExpression,
    Lambda,
    Await,
    FunctionDefinition,
    ClassDefinition,
    DecoratedDefinition,
    Decorator,
    Parameters,
    TypedParameter,
    DefaultParameter,
    TypedDefaultParameter,
    ListSplatPattern,
    DictionarySplatPattern,
    TuplePattern,
    ListPattern,
    PatternList,
    ImportStatement,
    ImportFromStatement,
    FutureImportStatement,
    AliasedImport,
    DottedName,
    RelativeImport,
    ImportPrefix,
    WildcardImport,
    GlobalStatement,
    ExpressionStatement,
    PassStatement,
    RaiseStatement,
    /// Statements and clauses that only nest other statements (`if`, `while`, `try`, `with`, blocks).
    Container,
    ForStatement,
    ExceptClause,
    AsPattern,
    AsPatternTarget,
    ForInClause,
    ListComprehension,
    DictionaryComprehension,
    SetComprehension,
    GeneratorExpression,
    KeywordArgument,
    ArgumentList,
    ListSplat,
    DictionarySplat,
    ParenthesizedExpression,
    Tuple,
    List,
    Dictionary,
    Set,
    String,
    ConcatenatedString,
    Interpolation,
    Integer,
    Float,
    True,
    False,
    None,
    Ellipsis,
    BinaryOperator,
    ConditionalExpression,
    Type,
    GenericType,
    UnionType,
    ConstrainedType,
    MemberType,
    SplatType,
    TypeParameter,
    TypeAliasStatement,
    CasePattern,
    ClassPattern,
    KeywordPattern,
    SplatPattern,
    UnionPattern,
    DictPattern,
}

struct Tables {
    kinds: KindTable<K>,
    f_name: u16,
    f_body: u16,
    f_parameters: u16,
    f_return_type: u16,
    f_type_parameters: u16,
    f_superclasses: u16,
    f_definition: u16,
    f_function: u16,
    f_arguments: u16,
    f_object: u16,
    f_attribute: u16,
    f_left: u16,
    f_right: u16,
    f_type: u16,
    f_value: u16,
    f_alias: u16,
    f_module_name: u16,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let lang: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
        let pairs: &[(&str, K)] = &[
            ("ERROR", K::Error),
            ("comment", K::Comment),
            ("identifier", K::Identifier),
            ("attribute", K::Attribute),
            ("call", K::Call),
            ("subscript", K::Subscript),
            ("assignment", K::Assignment),
            ("augmented_assignment", K::AugmentedAssignment),
            ("named_expression", K::NamedExpression),
            ("lambda", K::Lambda),
            ("function_definition", K::FunctionDefinition),
            ("class_definition", K::ClassDefinition),
            ("decorated_definition", K::DecoratedDefinition),
            ("decorator", K::Decorator),
            ("parameters", K::Parameters),
            ("lambda_parameters", K::Parameters),
            ("typed_parameter", K::TypedParameter),
            ("default_parameter", K::DefaultParameter),
            ("typed_default_parameter", K::TypedDefaultParameter),
            ("list_splat_pattern", K::ListSplatPattern),
            ("dictionary_splat_pattern", K::DictionarySplatPattern),
            ("tuple_pattern", K::TuplePattern),
            ("list_pattern", K::ListPattern),
            ("pattern_list", K::PatternList),
            ("import_statement", K::ImportStatement),
            ("import_from_statement", K::ImportFromStatement),
            ("future_import_statement", K::FutureImportStatement),
            ("aliased_import", K::AliasedImport),
            ("dotted_name", K::DottedName),
            ("relative_import", K::RelativeImport),
            ("import_prefix", K::ImportPrefix),
            ("wildcard_import", K::WildcardImport),
            ("global_statement", K::GlobalStatement),
            ("nonlocal_statement", K::Skip),
            ("expression_statement", K::ExpressionStatement),
            ("pass_statement", K::PassStatement),
            ("raise_statement", K::RaiseStatement),
            ("block", K::Container),
            ("if_statement", K::Container),
            ("elif_clause", K::Container),
            ("else_clause", K::Container),
            ("while_statement", K::Container),
            ("try_statement", K::Container),
            ("finally_clause", K::Container),
            ("with_statement", K::Container),
            ("with_clause", K::Container),
            ("with_item", K::Container),
            ("match_statement", K::Container),
            ("case_clause", K::Container),
            ("for_statement", K::ForStatement),
            ("except_clause", K::ExceptClause),
            ("as_pattern", K::AsPattern),
            ("as_pattern_target", K::AsPatternTarget),
            ("for_in_clause", K::ForInClause),
            ("list_comprehension", K::ListComprehension),
            ("dictionary_comprehension", K::DictionaryComprehension),
            ("set_comprehension", K::SetComprehension),
            ("generator_expression", K::GeneratorExpression),
            ("keyword_argument", K::KeywordArgument),
            ("argument_list", K::ArgumentList),
            ("list_splat", K::ListSplat),
            ("dictionary_splat", K::DictionarySplat),
            ("parenthesized_expression", K::ParenthesizedExpression),
            ("tuple", K::Tuple),
            ("list", K::List),
            ("dictionary", K::Dictionary),
            ("set", K::Set),
            ("string", K::String),
            ("concatenated_string", K::ConcatenatedString),
            ("interpolation", K::Interpolation),
            ("integer", K::Integer),
            ("float", K::Float),
            ("true", K::True),
            ("false", K::False),
            ("none", K::None),
            ("ellipsis", K::Ellipsis),
            ("binary_operator", K::BinaryOperator),
            ("conditional_expression", K::ConditionalExpression),
            ("type", K::Type),
            ("generic_type", K::GenericType),
            ("union_type", K::UnionType),
            ("constrained_type", K::ConstrainedType),
            ("member_type", K::MemberType),
            ("splat_type", K::SplatType),
            ("type_parameter", K::TypeParameter),
            ("type_alias_statement", K::TypeAliasStatement),
            ("case_pattern", K::CasePattern),
            ("class_pattern", K::ClassPattern),
            ("keyword_pattern", K::KeywordPattern),
            ("splat_pattern", K::SplatPattern),
            ("union_pattern", K::UnionPattern),
            ("dict_pattern", K::DictPattern),
            // leaves that never contain references
            ("string_start", K::Skip),
            ("string_content", K::Skip),
            ("string_end", K::Skip),
            ("escape_sequence", K::Skip),
            ("escape_interpolation", K::Skip),
            ("type_conversion", K::Skip),
            ("line_continuation", K::Skip),
            ("break_statement", K::Skip),
            ("continue_statement", K::Skip),
            ("keyword_separator", K::Skip),
            ("positional_separator", K::Skip),
            ("complex_pattern", K::Skip),
            // nothing special: their children are walked
            ("module", K::Other),
            ("assert_statement", K::Other),
            ("return_statement", K::Other),
            ("delete_statement", K::Other),
            ("print_statement", K::Other),
            ("exec_statement", K::Other),
            ("chevron", K::Other),
            ("await", K::Await),
            ("yield", K::Other),
            ("boolean_operator", K::Other),
            ("comparison_operator", K::Other),
            ("not_operator", K::Other),
            ("unary_operator", K::Other),
            ("expression_list", K::Other),
            ("parenthesized_list_splat", K::Other),
            ("pair", K::Other),
            ("slice", K::Other),
            ("if_clause", K::Other),
            ("format_specifier", K::Other),
            ("format_expression", K::Other),
        ];
        // Tokens (anonymous nodes) are nearly half of a tree and hold nothing: skipping them
        // outright saves a failed descent each. That is only sound while the table above names
        // every node kind of the grammar; a kind it does not know (a newer grammar) must keep
        // falling through to `children()`.
        let complete = (0..lang.node_kind_count()).all(|id| {
            let id = id as u16;
            !lang.node_kind_is_named(id) || lang.node_kind_for_id(id).is_some_and(|name| pairs.iter().any(|p| p.0 == name))
        });
        if std::env::var_os("CODEFAST_PY_KINDS").is_some() {
            for id in 0..lang.node_kind_count() as u16 {
                if lang.node_kind_is_named(id) && !lang.node_kind_for_id(id).is_some_and(|name| pairs.iter().any(|p| p.0 == name)) {
                    eprintln!("unlisted kind: {:?}", lang.node_kind_for_id(id));
                }
            }
            eprintln!("complete={complete}");
        }
        let kinds = KindTable::build(&lang, if complete { K::Skip } else { K::Other }, pairs);
        Tables {
            kinds,
            f_name: field(&lang, "name"),
            f_body: field(&lang, "body"),
            f_parameters: field(&lang, "parameters"),
            f_return_type: field(&lang, "return_type"),
            f_type_parameters: field(&lang, "type_parameters"),
            f_superclasses: field(&lang, "superclasses"),
            f_definition: field(&lang, "definition"),
            f_function: field(&lang, "function"),
            f_arguments: field(&lang, "arguments"),
            f_object: field(&lang, "object"),
            f_attribute: field(&lang, "attribute"),
            f_left: field(&lang, "left"),
            f_right: field(&lang, "right"),
            f_type: field(&lang, "type"),
            f_value: field(&lang, "value"),
            f_alias: field(&lang, "alias"),
            f_module_name: field(&lang, "module_name"),
        }
    })
}

// ----- per-name knowledge, indexed by file-local name id -----

const N_SEEN: u16 = 1;
/// Well-known builtin: a bare use is not a reference.
const N_STOP: u16 = 1 << 1;
/// CapWords (or a single capital letter): looks like a class.
const N_CAP: u16 = 1 << 2;
/// ALL_CAPS: looks like a constant.
const N_UPPER: u16 = 1 << 3;
/// Builtin type (`str`, `dict`, ...): usable as a type qualifier.
const N_BTYPE: u16 = 1 << 4;
/// Bound by `import a.b` / `import a.b as x`: stands for a module path.
const N_ALIAS: u16 = 1 << 5;
/// Bound by `from m import x`.
const N_SYM: u16 = 1 << 6;
/// ... under another local name; `bound` maps it to the original.
const N_SYMALIAS: u16 = 1 << 7;
/// Defined at module level in this file.
const N_FILEDEF: u16 = 1 << 8;
/// A class defined in this file.
const N_FILECLASS: u16 = 1 << 9;
/// Declared as a local somewhere in this file (only then are the scopes worth searching).
const N_LOCAL: u16 = 1 << 10;
/// Listed in `__all__`.
const N_EXPORT: u16 = 1 << 11;

/// How an import binds a name.
const N_IMPORT: u16 = N_ALIAS | N_SYM | N_SYMALIAS;

/// Names that need more than "emit a read" when met as a bare identifier.
const N_SPECIAL: u16 = N_STOP | N_ALIAS | N_SYMALIAS | N_LOCAL;

// ----- states of a local in the scope maps (`Ctx::declare`) -----
//
// `NONE`: bound to something of unknown type. Any other value below `GLOBAL` is the name id of
// the local's type path, with `STICKY` set when it comes from an annotation.

/// Hoisted by the pre-scan, no binding seen yet.
const UNBOUND: u32 = NONE - 1;
/// Declared `global`: not a local at all.
const GLOBAL: u32 = NONE - 2;
const STICKY: u32 = 1 << 31;
/// Result of inferring the type of `None`: says nothing about the variable.
const NEUTRAL: u32 = UNBOUND;

// ----- decorators that change what a definition is -----

const D_PROPERTY: u8 = 1;
const D_STATIC: u8 = 2;
const D_CLASSMETHOD: u8 = 4;
const D_ABSTRACT: u8 = 8;
const D_OVERLOAD: u8 = 16;

/// Longest receiver type path worth recording: a path that keeps growing (`a = b.f()`,
/// `c = a.g()`, ...) is cut off here.
const MAX_PATH: usize = 160;

/// Decorators shorter than this stay part of the signature (`@property def x(self)`).
const MAX_DECORATOR_SIG: usize = 100;

#[derive(Clone, Copy, PartialEq, Eq)]
enum FK {
    Module,
    Class,
    Function,
}

/// The receiver is the class itself (`cls` of a classmethod).
const F_RECV_CLS: u8 = 1;
/// Body of a `Protocol` / `ABC` class: bodiless methods are declarations.
const F_STUB: u8 = 2;
/// Body of a test class.
const F_TEST: u8 = 4;

/// What encloses the code being walked.
#[derive(Clone, Copy)]
struct Frame {
    kind: FK,
    /// Definition that owns new definitions (`NONE` at module level).
    def: u32,
    /// Class that `recv` is an instance of; for a class body, the class itself.
    class_def: u32,
    class_name: u32,
    /// Name of the receiver parameter (`self` / `cls`) visible here, or `NONE`.
    recv: u32,
    flags: u8,
}

/// How an expression used as a receiver (`recv.attr`) is known.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rk {
    Unknown,
    /// An imported module path (`os.path`).
    Module,
    /// A lower-case name bound by `from m import name`: a module or an object, used like a module.
    Sym,
    /// A class, named directly.
    Type,
    /// A value whose type path is known (`T`, `T::f`, `$global`, `f()`, `^Base`).
    Path,
}

/// What a bare name denotes where it is used.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bare {
    Local,
    /// A module alias or a builtin: not a reference by itself.
    Silent,
    /// An attribute of the class whose body is being walked.
    Member,
    /// A module-level or imported name, under the name it has where it is defined; with the
    /// module it comes from when it is written through the alias of an import.
    Global(u32, u32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Use {
    Read,
    Write,
    Call,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum How {
    /// `target = value`: defines at module / class level, binds a local in a function.
    Assign,
    /// `target += value`: the name already exists.
    Aug,
    /// Loop, `with`, `except`, walrus, comprehension and match bindings: never a definition.
    Bind,
}

#[derive(Clone, Copy)]
struct Tgt<'t> {
    how: How,
    /// The assignment statement: span of the definitions its targets create.
    stmt: Option<Node<'t>>,
    sig_end: usize,
    /// Type path of the bound value (`NONE` when unknown).
    ty: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Body {
    Normal,
    /// Only a docstring, `pass` or `...`.
    Stub,
    /// Only `raise NotImplementedError`.
    Raises,
}

struct W<'a, 't> {
    cx: Ctx<'a>,
    t: &'static Tables,
    cur: Frame,
    frames: Vec<Frame>,
    /// `N_*` flags per name id; 0 = not classified yet.
    ninfo: Vec<u16>,
    /// Module alias -> import target, aliased imported symbol -> its original name.
    bound: FxHashMap<u32, u32>,
    /// Name bound by `from m import x` -> `m` (the import target as written).
    sym_target: FxHashMap<u32, u32>,
    /// Targets of `import a.b.Name` statements with a capitalised component: `a.b.Name` is
    /// then a module, not a class of `a.b`.
    cap_modules: Vec<u32>,
    /// (owner definition or `NONE` for the module, name) -> definition index.
    members: FxHashMap<u64, u32>,
    /// Parameters waiting for their function's scope to open: (name, local state).
    params: Vec<(u32, u32)>,
    /// Names declared `global`, collected by the pre-scan.
    tmp: Vec<u32>,
    /// Names bound by `:=` inside the comprehensions being walked.
    walrus: Vec<u32>,
    comp_depth: u32,
    /// Import bindings made inside the functions being walked: (name, import flags it had,
    /// what it was bound to, the module it came from). They end with the function.
    import_undo: Vec<(u32, u16, u32, u32)>,
    _tree: PhantomData<&'t ()>,
}

pub fn extract(_lang: Lang, src: &[u8], tree: &Tree, path: &str, facts: &mut FileFacts, scratch: &mut Scratch) {
    let mut cx = Ctx::new(src, path, facts, scratch);
    cx.is_header = Lang::is_header(path);
    let mut w = W {
        cx,
        t: tables(),
        cur: Frame { kind: FK::Module, def: NONE, class_def: NONE, class_name: NONE, recv: NONE, flags: 0 },
        frames: Vec::new(),
        ninfo: Vec::new(),
        bound: FxHashMap::default(),
        sym_target: FxHashMap::default(),
        cap_modules: Vec::new(),
        members: FxHashMap::default(),
        params: Vec::new(),
        tmp: Vec::new(),
        walrus: Vec::new(),
        comp_depth: 0,
        import_undo: Vec::new(),
        _tree: PhantomData,
    };
    // File scope: holds module-level loop / `with` / `except` variables, which are not definitions.
    w.cx.push_scope();
    let mut c = tree.walk();
    w.prescan_module(&mut c);
    w.visit(&mut c);
    w.cx.pop_scope();
    w.finish();
}

#[inline]
fn mkey(owner: u32, name: u32) -> u64 {
    ((owner as u64) << 32) | name as u64
}

impl<'a, 't> W<'a, 't> {
    #[inline]
    fn k(&self, n: Node<'t>) -> K {
        self.t.kinds.get(n.kind_id())
    }

    // ----- names -----

    #[inline]
    fn info(&mut self, id: u32) -> u16 {
        match self.ninfo.get(id as usize) {
            Some(&f) if f != 0 => f,
            _ => self.classify(id),
        }
    }

    #[cold]
    fn classify(&mut self, id: u32) -> u16 {
        let f = lexical_flags(self.cx.name_bytes(id));
        let i = id as usize;
        if i >= self.ninfo.len() {
            self.ninfo.resize(i + 1, 0);
        }
        self.ninfo[i] = f;
        f
    }

    fn set_info(&mut self, id: u32, bits: u16) {
        let f = self.info(id);
        self.ninfo[id as usize] = f | bits;
    }

    /// The name an imported symbol has where it is defined (`from m import a as b`: `b` -> `a`).
    #[inline]
    fn orig(&self, name: u32, info: u16) -> u32 {
        if info & N_SYMALIAS != 0 { self.bound.get(&name).copied().unwrap_or(name) } else { name }
    }

    /// The module behind a name written through the alias of an import (`from m import a as
    /// b`: `b` -> `m`), else `NONE`: what a reference to `a` made under the name `b` is
    /// qualified by, since no import of the file binds the name `a` itself.
    #[inline]
    fn alias_module(&self, name: u32, info: u16) -> u32 {
        if info & N_SYMALIAS != 0 { self.sym_target.get(&name).copied().unwrap_or(NONE) } else { NONE }
    }

    /// Append the name `name` denotes where it is defined: `orig`, or `TARGET|orig` when it is
    /// written through the alias of an import.
    fn push_orig(&mut self, name: u32, info: u16, buf: &mut Vec<u8>) {
        let m = self.alias_module(name, info);
        if m != NONE {
            buf.extend_from_slice(self.cx.name_bytes(m));
            buf.push(b'|');
        }
        let o = self.orig(name, info);
        buf.extend_from_slice(self.cx.name_bytes(o));
    }

    /// `push_orig`, interned: the type name behind `name`.
    fn orig_type(&mut self, name: u32, info: u16) -> u32 {
        if info & N_SYMALIAS == 0 {
            return name;
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        self.push_orig(name, info, &mut buf);
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// True for a builtin that nothing in this file redefines or imports.
    #[inline]
    fn is_stopped(info: u16) -> bool {
        info & N_STOP != 0 && info & (N_FILEDEF | N_SYM) == 0
    }

    // ----- locals -----

    #[inline]
    fn declare(&mut self, name: u32, state: u32) {
        self.set_info(name, N_LOCAL);
        self.cx.declare(name, state);
    }

    /// `None`: not a local. `Some(NONE)`: a local of unknown type. `Some(t)`: a local of type path `t`.
    #[inline]
    fn local_ty(&mut self, name: u32) -> Option<u32> {
        if self.info(name) & N_LOCAL == 0 {
            return None;
        }
        match self.cx.local_type(name) {
            None | Some(GLOBAL) => None,
            Some(v) if v >= UNBOUND => Some(NONE),
            Some(v) => Some(v & !STICKY),
        }
    }

    fn is_global(&mut self, name: u32) -> bool {
        self.info(name) & N_LOCAL != 0 && self.cx.local_type(name) == Some(GLOBAL)
    }

    /// Record an assignment to local `name`. A declared type is final; an inferred one holds
    /// only while every assignment seen so far agrees on it (`None` says nothing).
    fn bind_local(&mut self, name: u32, ty: u32, declared: bool) {
        let cur = if self.info(name) & N_LOCAL != 0 { self.cx.local_type(name) } else { None };
        let new = if declared {
            if ty == NONE || ty == NEUTRAL { NONE } else { ty | STICKY }
        } else {
            match cur {
                Some(v) if v < GLOBAL && v & STICKY != 0 => return,
                Some(NONE) => return,
                None | Some(UNBOUND) | Some(GLOBAL) => ty,
                Some(v) => {
                    if ty == NEUTRAL || ty == v {
                        return;
                    }
                    NONE
                }
            }
        };
        self.declare(name, new);
    }

    /// A binding that is never a definition: loop / `with` / `except` targets, walrus, captures.
    fn bind_plain(&mut self, n: Node<'t>, name: u32, ty: u32, walrus: bool) {
        if self.comp_depth == 0 || walrus {
            if self.is_global(name) {
                return self.cx.add_ref(name, RefKind::Write, line_of(n), NONE, 255);
            }
        }
        if self.comp_depth == 0 {
            match self.cur.kind {
                FK::Module if self.members.contains_key(&mkey(NONE, name)) => {
                    return self.cx.add_ref(name, RefKind::Write, line_of(n), NONE, 255);
                }
                FK::Class if self.members.contains_key(&mkey(self.cur.def, name)) => {
                    return self.cx.add_ref(name, RefKind::MemberWrite, line_of(n), self.cur.class_name, 255);
                }
                _ => {}
            }
        } else if walrus {
            self.walrus.push(name);
        }
        self.declare(name, if ty == NEUTRAL { NONE } else { ty });
    }

    /// A module-level definition takes its name back from a same-named loop variable.
    fn unshadow(&mut self, name: u32) {
        if self.info(name) & N_LOCAL != 0 && self.cx.local_type(name).is_some() {
            self.cx.declare(name, GLOBAL);
        }
    }

    // ----- frames -----

    /// Enter the body of a function or lambda. `base` is where its queued parameters start.
    fn enter(&mut self, def: u32, recv: u32, recv_cls: bool, base: usize) {
        let outer = self.cur;
        let mut f = Frame { kind: FK::Function, def: if def != NONE { def } else { outer.def }, class_def: NONE, class_name: NONE, recv: NONE, flags: 0 };
        match outer.kind {
            FK::Class => {
                if recv != NONE {
                    f.class_def = outer.def;
                    f.class_name = outer.class_name;
                    f.recv = recv;
                    if recv_cls {
                        f.flags |= F_RECV_CLS;
                    }
                }
            }
            FK::Function => {
                // A closure sees the method's `self`, unless one of its parameters hides it.
                f.class_def = outer.class_def;
                f.class_name = outer.class_name;
                f.recv = outer.recv;
                f.flags = outer.flags & F_RECV_CLS;
                if f.recv != NONE && self.params[base.min(self.params.len())..].iter().any(|p| p.0 == f.recv) {
                    f.recv = NONE;
                }
            }
            FK::Module => {}
        }
        self.frames.push(outer);
        self.cur = f;
    }

    fn leave(&mut self) {
        if let Some(f) = self.frames.pop() {
            self.cur = f;
        }
    }

    fn declare_params(&mut self, base: usize) {
        for i in base..self.params.len() {
            let (p, st) = self.params[i];
            self.declare(p, st);
        }
        self.params.truncate(base);
    }

    // ----- traversal -----

    /// Visit every child of the cursor's node. Depth is accounted for in `visit`, through which
    /// all recursion passes.
    fn children(&mut self, c: &mut TreeCursor<'t>) {
        if c.goto_first_child() {
            loop {
                self.visit(c);
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
    }

    fn visit(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        let k = self.k(n);
        match k {
            K::Identifier => return self.identifier(n),
            K::Skip
            | K::Comment
            | K::Integer
            | K::Float
            | K::True
            | K::False
            | K::None
            | K::Ellipsis
            | K::PassStatement
            | K::GlobalStatement
            | K::DottedName
            | K::AliasedImport
            | K::RelativeImport
            | K::ImportPrefix
            | K::WildcardImport
            | K::Parameters
            | K::TypedParameter
            | K::DefaultParameter
            | K::TypedDefaultParameter
            | K::DictionarySplatPattern => return,
            _ => {}
        }
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        match k {
            K::Attribute => {
                self.attr_access(n, c, Use::Read);
            }
            K::Call => self.call(n, c),
            K::Assignment => self.assignment(n, c),
            K::AugmentedAssignment => self.augmented(n, c),
            K::NamedExpression => self.named_expression(n, c),
            K::Lambda => self.lambda(n, c, NONE),
            K::FunctionDefinition => self.function(n, n, 0, c),
            K::ClassDefinition => self.class(n, n, c),
            K::DecoratedDefinition => self.decorated(n, c),
            K::Decorator => self.decorator(c),
            K::ImportStatement => self.import_statement(c),
            K::ImportFromStatement => self.import_from(n, c, false),
            K::FutureImportStatement => self.import_from(n, c, true),
            K::ForStatement | K::ForInClause => self.for_loop(n, c),
            K::ExceptClause => self.except_clause(c),
            K::AsPattern => self.as_pattern(c, false),
            K::ListComprehension | K::DictionaryComprehension | K::SetComprehension | K::GeneratorExpression => self.comprehension(n, c),
            K::KeywordArgument => self.keyword_argument(n, c),
            K::ConditionalExpression => self.conditional(c),
            K::String => self.string(n, c),
            K::Type | K::GenericType | K::UnionType | K::ConstrainedType | K::MemberType | K::SplatType | K::TypeParameter => self.type_expr(c),
            K::TypeAliasStatement => self.type_alias(n, c),
            K::CasePattern => self.pattern(c),
            K::Error => self.error(c),
            _ => self.children(c),
        }
        self.cx.depth -= 1;
    }

    /// Tokens that could not be placed surface as identifiers directly under ERROR nodes: they
    /// are noise. Anything structured below is still worth walking.
    fn error(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.facts.parse_errors += 1;
        if c.goto_first_child() {
            loop {
                if self.k(c.node()) != K::Identifier {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
    }

    // ----- pre-scans -----

    /// Names the module defines, known before any body is walked: a function may use a class
    /// or a function that the file only defines further down. Cursor on a statement container.
    fn prescan_module(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= 64 {
            return;
        }
        self.cx.depth += 1;
        let t = self.t;
        if c.goto_first_child() {
            loop {
                let n = c.node();
                match self.k(n) {
                    K::FunctionDefinition => self.predefine(n, N_FILEDEF),
                    K::ClassDefinition => self.predefine(n, N_FILEDEF | N_FILECLASS),
                    K::DecoratedDefinition => {
                        if let Some(d) = child(n, t.f_definition) {
                            match self.k(d) {
                                K::FunctionDefinition => self.predefine(d, N_FILEDEF),
                                K::ClassDefinition => self.predefine(d, N_FILEDEF | N_FILECLASS),
                                _ => {}
                            }
                        }
                    }
                    K::ExpressionStatement => {
                        if let Some(l) = n.named_child(0).filter(|a| self.k(*a) == K::Assignment).and_then(|a| child(a, t.f_left)) {
                            if self.k(l) == K::Identifier {
                                let name = self.cx.name(l);
                                self.set_info(name, N_FILEDEF);
                            }
                        }
                    }
                    K::Container | K::ForStatement | K::ExceptClause => self.prescan_module(c),
                    _ => {}
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.cx.depth -= 1;
    }

    fn predefine(&mut self, def: Node<'t>, bits: u16) {
        if let Some(nm) = child(def, self.t.f_name) {
            let name = self.cx.name(nm);
            self.set_info(name, bits);
        }
    }

    /// Hoist the names a function body binds: Python locals are function-wide, so a closure or
    /// a loop may read one above the statement that assigns it. Only statements are scanned
    /// (never expressions, nested functions or classes). Cursor on a statement container.
    fn prescan(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        let t = self.t;
        if c.goto_first_child() {
            loop {
                let n = c.node();
                match self.k(n) {
                    K::ExpressionStatement => {
                        let mut a = n.named_child(0);
                        for _ in 0..16 {
                            let Some(x) = a.filter(|x| matches!(self.k(*x), K::Assignment | K::AugmentedAssignment)) else { break };
                            let right = child(x, t.f_right);
                            if let Some(l) = child(x, t.f_left) {
                                // `f = lambda: ...` defines a nested function, not a variable.
                                let nested_fn = self.k(l) == K::Identifier && self.lambda_of(right).is_some();
                                if !nested_fn {
                                    self.prescan_target(l, 0);
                                }
                            }
                            a = right;
                        }
                    }
                    K::ForStatement => {
                        if let Some(l) = child(n, t.f_left) {
                            self.prescan_target(l, 0);
                        }
                        self.prescan(c);
                    }
                    K::AsPattern => {
                        if let Some(al) = child(n, t.f_alias) {
                            self.prescan_target(al, 0);
                        }
                    }
                    K::Container | K::ExceptClause | K::Error => self.prescan(c),
                    K::GlobalStatement => {
                        let mut i = 0;
                        while let Some(id) = n.named_child(i) {
                            if self.k(id) == K::Identifier {
                                let name = self.cx.name(id);
                                self.tmp.push(name);
                            }
                            i += 1;
                        }
                    }
                    _ => {}
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.cx.depth -= 1;
    }

    /// Names bound by `:=` anywhere in a function body (comprehensions included: they bind in
    /// the function). Only run for bodies whose text contains `:=`. Cursor on the body.
    fn prescan_walrus(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let mut depth = 0u32;
        loop {
            let n = c.node();
            let descend = match self.k(n) {
                K::NamedExpression => {
                    if let Some(nm) = child(n, t.f_name).filter(|x| self.k(*x) == K::Identifier && x.start_byte() < x.end_byte()) {
                        let name = self.cx.name(nm);
                        self.declare(name, UNBOUND);
                    }
                    true
                }
                // other scopes, and leaves that cannot hold one
                K::FunctionDefinition | K::ClassDefinition | K::DecoratedDefinition | K::Lambda if depth > 0 => false,
                K::Identifier | K::Comment | K::Skip | K::Integer | K::Float => false,
                _ => true,
            };
            if descend && depth < MAX_DEPTH && c.goto_first_child() {
                depth += 1;
                continue;
            }
            loop {
                if depth == 0 {
                    return;
                }
                if c.goto_next_sibling() {
                    break;
                }
                c.goto_parent();
                depth -= 1;
            }
        }
    }

    fn prescan_target(&mut self, n: Node<'t>, depth: u32) {
        match self.k(n) {
            K::Identifier => {
                let name = self.cx.name(n);
                self.declare(name, UNBOUND);
            }
            K::PatternList
            | K::TuplePattern
            | K::ListPattern
            | K::Tuple
            | K::List
            | K::ListSplatPattern
            | K::ListSplat
            | K::AsPatternTarget
            | K::ParenthesizedExpression
                if depth < 8 =>
            {
                let mut i = 0;
                while let Some(ch) = n.named_child(i) {
                    self.prescan_target(ch, depth + 1);
                    i += 1;
                }
            }
            _ => {}
        }
    }

    // ----- identifiers -----

    #[inline]
    fn identifier(&mut self, n: Node<'t>) {
        if n.start_byte() == n.end_byte() {
            // MISSING node inserted by error recovery
            self.cx.facts.parse_errors += 1;
            return;
        }
        let name = self.cx.name(n);
        let info = self.info(name);
        if info & N_SPECIAL == 0 && self.cur.kind != FK::Class {
            return self.cx.add_ref(name, RefKind::Read, line_of(n), NONE, 255);
        }
        self.identifier_slow(n, name, info)
    }

    fn bare(&mut self, name: u32, info: u16) -> Bare {
        if info & N_LOCAL != 0 && self.local_ty(name).is_some() {
            return Bare::Local;
        }
        // In a class body a bare name is first of all an attribute of that class.
        if self.cur.kind == FK::Class && self.members.contains_key(&mkey(self.cur.def, name)) {
            return Bare::Member;
        }
        if info & N_ALIAS != 0 || Self::is_stopped(info) {
            return Bare::Silent;
        }
        Bare::Global(self.orig(name, info), self.alias_module(name, info))
    }

    /// Reference to a bare name: `plain` for a global, `member` for a class attribute.
    fn bare_ref(&mut self, n: Node<'t>, name: u32, info: u16, plain: RefKind, member: RefKind) {
        match self.bare(name, info) {
            Bare::Local | Bare::Silent => {}
            Bare::Member => self.cx.add_ref(name, member, line_of(n), self.cur.class_name, 255),
            Bare::Global(o, q) => self.cx.add_ref(o, plain, line_of(n), q, 255),
        }
    }

    fn identifier_slow(&mut self, n: Node<'t>, name: u32, info: u16) {
        self.bare_ref(n, name, info, RefKind::Read, RefKind::MemberRead);
    }

    /// A non-local name that is assigned without being (re)defined: `x += 1`, `x[i] = v`.
    fn write_ident(&mut self, n: Node<'t>, name: u32) {
        let info = self.info(name);
        self.bare_ref(n, name, info, RefKind::Write, RefKind::MemberWrite);
    }

    // ----- receivers -----

    /// Classify the receiver expression `e` and intern its qualifier.
    fn recv_of(&mut self, e: Node<'t>) -> (Rk, u32) {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let rk = self.recv(e, &mut buf, 0);
        let q = if rk != Rk::Unknown && !buf.is_empty() && buf.len() <= MAX_PATH { self.cx.intern(&buf) } else { NONE };
        self.cx.s.buf = buf;
        if q == NONE { (Rk::Unknown, NONE) } else { (rk, q) }
    }

    fn recv(&mut self, e: Node<'t>, buf: &mut Vec<u8>, depth: u32) -> Rk {
        if depth > 6 {
            return Rk::Unknown;
        }
        let t = self.t;
        let lit: &[u8] = match self.k(e) {
            K::Identifier => return self.recv_ident(e, buf),
            K::Attribute => {
                let (Some(obj), Some(at)) = (child(e, t.f_object), child(e, t.f_attribute)) else { return Rk::Unknown };
                let r = self.recv(obj, buf, depth + 1);
                let name = self.cx.text(at);
                if r == Rk::Module && !self.cap_modules.is_empty() && is_classlike(name) {
                    // `import xml.etree.ElementTree`: the capitalised component is a module
                    let len = buf.len();
                    buf.push(b'.');
                    buf.extend_from_slice(name);
                    if self
                        .cap_modules
                        .iter()
                        .any(|&m| self.cx.name_bytes(m).starts_with(buf) && matches!(self.cx.name_bytes(m).get(buf.len()), None | Some(b'.')))
                    {
                        return Rk::Module;
                    }
                    buf.truncate(len);
                }
                return recv_step(r, name, buf);
            }
            K::Call => return self.recv_call(e, buf, depth),
            // (the grammar reads `[*a.b()]` as `(*a).b()`: look through the splat; the value of
            // `await f()` has the declared result type of `f`)
            K::ParenthesizedExpression | K::ListSplat | K::ListSplatPattern | K::Await => {
                return match e.named_child(0) {
                    Some(x) => self.recv(x, buf, depth + 1),
                    None => Rk::Unknown,
                };
            }
            K::String | K::ConcatenatedString => {
                if string_is_bytes(self.cx.text(e)) {
                    b"bytes"
                } else {
                    b"str"
                }
            }
            K::Integer => b"int",
            K::Float => b"float",
            K::True | K::False => b"bool",
            K::List | K::ListComprehension => b"list",
            K::Dictionary | K::DictionaryComprehension => b"dict",
            K::Set | K::SetComprehension => b"set",
            K::Tuple => b"tuple",
            _ => return Rk::Unknown,
        };
        buf.extend_from_slice(lit);
        Rk::Path
    }

    fn recv_ident(&mut self, e: Node<'t>, buf: &mut Vec<u8>) -> Rk {
        let name = self.cx.name(e);
        self.recv_name(name, buf)
    }

    /// What the bare name `name` is when used as a receiver; its path goes to `buf`.
    fn recv_name(&mut self, name: u32, buf: &mut Vec<u8>) -> Rk {
        let info = self.info(name);
        if info & N_LOCAL != 0 {
            match self.cx.local_type(name) {
                None | Some(GLOBAL) => {}
                Some(v) if v >= UNBOUND => return Rk::Unknown,
                Some(v) => {
                    buf.extend_from_slice(self.cx.name_bytes(v & !STICKY));
                    return Rk::Path;
                }
            }
        }
        // In a class body a bare name is an attribute of the class being defined.
        if self.cur.kind == FK::Class && self.cur.class_name != NONE && self.members.contains_key(&mkey(self.cur.def, name)) {
            buf.extend_from_slice(self.cx.name_bytes(self.cur.class_name));
            buf.extend_from_slice(b"::");
            buf.extend_from_slice(self.cx.name_bytes(name));
            return Rk::Path;
        }
        if info & N_ALIAS != 0 {
            let target = self.bound.get(&name).copied().unwrap_or(name);
            buf.extend_from_slice(self.cx.name_bytes(target));
            return Rk::Module;
        }
        if info & N_SYM != 0 {
            let o = self.orig(name, info);
            let oinfo = self.info(o);
            if oinfo & (N_UPPER | N_CAP) == N_UPPER {
                // an imported ALL_CAPS constant: an object, never a module or a class
                let m = self.alias_module(name, info);
                if m != NONE {
                    buf.extend_from_slice(self.cx.name_bytes(m));
                    buf.push(b'|');
                }
                buf.push(b'$');
                buf.extend_from_slice(self.cx.name_bytes(o));
                return Rk::Path;
            }
            if oinfo & N_CAP != 0 {
                self.push_orig(name, info, buf);
                return Rk::Type;
            }
            // A module, or an object of the module it was imported from: `TARGET.name`, the
            // path `import TARGET.name` would have named.
            if let Some(&target) = self.sym_target.get(&name) {
                buf.extend_from_slice(self.cx.name_bytes(target));
                if buf.last() != Some(&b'.') {
                    buf.push(b'.');
                }
            }
            buf.extend_from_slice(self.cx.name_bytes(o));
            return Rk::Sym;
        }
        if info & (N_FILECLASS | N_CAP | N_BTYPE) != 0 {
            buf.extend_from_slice(self.cx.name_bytes(name));
            return Rk::Type;
        }
        if Self::is_stopped(info) {
            return Rk::Unknown;
        }
        buf.push(b'$');
        buf.extend_from_slice(self.cx.name_bytes(name));
        Rk::Path
    }

    /// Type of a call result: an instance for `Class(...)`, `f()` for a plain function.
    fn recv_call(&mut self, e: Node<'t>, buf: &mut Vec<u8>, depth: u32) -> Rk {
        let t = self.t;
        let Some(f) = child(e, t.f_function) else { return Rk::Unknown };
        match self.k(f) {
            K::Identifier => {
                let text = self.cx.text(f);
                let name = self.cx.name(f);
                let info = self.info(name);
                if info & N_LOCAL != 0 && self.local_ty(name).is_some() {
                    // `cls(...)` in a classmethod builds an instance of the class
                    if name == self.cur.recv && self.cur.flags & F_RECV_CLS != 0 && self.cur.class_name != NONE {
                        buf.extend_from_slice(self.cx.name_bytes(self.cur.class_name));
                        return Rk::Path;
                    }
                    return Rk::Unknown;
                }
                if info & N_ALIAS != 0 {
                    return Rk::Unknown;
                }
                let first_arg = || child(e, t.f_arguments).and_then(|a| a.named_child(0));
                if text == b"super" && Self::is_stopped(info) {
                    // `super()` / `super(Class, self)`: a strict ancestor of the class
                    let explicit = first_arg().filter(|a| self.k(*a) == K::Identifier).map(|a| self.cx.name(a));
                    let cls = explicit.unwrap_or(self.cur.class_name);
                    if cls == NONE {
                        return Rk::Unknown;
                    }
                    buf.push(b'^');
                    buf.extend_from_slice(self.cx.name_bytes(cls));
                    return Rk::Path;
                }
                if text == b"cast" {
                    // `cast(T, x)` is a `T`
                    let ty = first_arg().map_or(NONE, |a| self.type_name_of(a, 0));
                    if ty == NONE {
                        return Rk::Unknown;
                    }
                    buf.extend_from_slice(self.cx.name_bytes(ty));
                    return Rk::Path;
                }
                if text == b"type" && Self::is_stopped(info) {
                    // `type(x).attr` looks `attr` up on the class of `x`
                    return match first_arg() {
                        Some(a) if self.recv(a, buf, depth + 1) == Rk::Path => Rk::Path,
                        _ => Rk::Unknown,
                    };
                }
                let o = self.orig(name, info);
                let oinfo = self.info(o);
                if (oinfo | info) & (N_CAP | N_BTYPE | N_FILECLASS) != 0 {
                    self.push_orig(name, info, buf);
                    return Rk::Path;
                }
                if Self::is_stopped(info) || oinfo & N_UPPER != 0 {
                    // a builtin, or an ALL_CAPS callable of unknown nature
                    return Rk::Unknown;
                }
                self.push_orig(name, info, buf);
                buf.extend_from_slice(b"()");
                Rk::Path
            }
            K::Attribute => {
                let (Some(obj), Some(at)) = (child(f, t.f_object), child(f, t.f_attribute)) else { return Rk::Unknown };
                let name = self.cx.text(at);
                if name.is_empty() {
                    return Rk::Unknown;
                }
                let r = self.recv(obj, buf, depth + 1);
                match r {
                    Rk::Unknown => Rk::Unknown,
                    // `x.__class__(...)` builds another `x`
                    Rk::Path if name == b"__class__" => Rk::Path,
                    // (`^Class` says where a member is looked up, not what the object is)
                    Rk::Path if buf.first() == Some(&b'^') => Rk::Unknown,
                    // `typing.cast(T, x)` is a `T`
                    Rk::Module if name == b"cast" => {
                        let ty = child(e, t.f_arguments).and_then(|a| a.named_child(0)).map_or(NONE, |a| self.type_name_of(a, 0));
                        if ty == NONE {
                            return Rk::Unknown;
                        }
                        buf.clear();
                        buf.extend_from_slice(self.cx.name_bytes(ty));
                        Rk::Path
                    }
                    // `mod.Class(...)` is a `TARGET|Class`, `mod.make(...)` a `TARGET|make()`
                    Rk::Module | Rk::Sym => {
                        buf.push(b'|');
                        buf.extend_from_slice(name);
                        if !is_classlike(name) {
                            buf.extend_from_slice(b"()");
                        }
                        Rk::Path
                    }
                    // `Outer.Inner(...)`
                    Rk::Type if is_classlike(name) => {
                        nested_type(buf, name);
                        Rk::Path
                    }
                    // `Class.make(...)`, `self.build(...)`, `a.b.get(...)`: the declared result
                    // of that method
                    Rk::Type | Rk::Path => {
                        buf.extend_from_slice(b"::");
                        buf.extend_from_slice(name);
                        buf.extend_from_slice(b"()");
                        Rk::Path
                    }
                }
            }
            // `type(x)(...)` builds another `x`
            K::Call => match self.type_of_arg(f) {
                Some(a) if self.recv(a, buf, depth + 1) == Rk::Path => Rk::Path,
                _ => Rk::Unknown,
            },
            _ => Rk::Unknown,
        }
    }

    /// The `x` of `type(x)`.
    fn type_of_arg(&mut self, call: Node<'t>) -> Option<Node<'t>> {
        let t = self.t;
        let f = child(call, t.f_function).filter(|f| self.k(*f) == K::Identifier && self.cx.text(*f) == b"type")?;
        let name = self.cx.name(f);
        let info = self.info(name);
        if !Self::is_stopped(info) || (info & N_LOCAL != 0 && self.local_ty(name).is_some()) {
            return None;
        }
        let args = child(call, t.f_arguments).filter(|a| a.named_child_count() == 1)?;
        args.named_child(0)
    }

    /// `x.__class__(...)` / `type(x)(...)` with `x` of a known class: a call of that class.
    fn own_class_call(&mut self, x: Node<'t>, at: Node<'t>) -> usize {
        let (rk, q) = self.recv_of(x);
        let ty = if rk == Rk::Path { self.simple(q) } else { NONE };
        if ty == NONE {
            return usize::MAX;
        }
        let (module, ty) = self.split_qualified(ty);
        if Self::is_stopped(self.info(ty)) {
            return usize::MAX;
        }
        let idx = self.cx.facts.refs.len();
        self.cx.add_ref(ty, RefKind::Call, line_of(at), module, 255);
        idx
    }

    /// `TARGET|Name` -> (`TARGET`, `Name`); a bare name -> (`NONE`, name).
    fn split_qualified(&mut self, ty: u32) -> (u32, u32) {
        let b = self.cx.facts.names.get(ty);
        let Some(i) = memchr::memchr(b'|', b) else { return (NONE, ty) };
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.extend_from_slice(b);
        let m = self.cx.intern(&buf[..i]);
        let n = self.cx.intern(&buf[i + 1..]);
        self.cx.s.buf = buf;
        (m, n)
    }

    /// The type `obj.name` names, with the module it is written through: `TARGET|Name` for
    /// `alias.Name`, `Name` for a class nested in a class of this file, `NONE` when `obj` is
    /// a value.
    fn attr_type(&mut self, obj: Node<'t>, name: &[u8]) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let r = self.recv(obj, &mut buf, 0);
        let id = if finish_type(r, &mut buf, name) && buf.len() <= MAX_PATH { self.cx.intern(&buf) } else { NONE };
        self.cx.s.buf = buf;
        id
    }

    /// Walk the dotted text `a.b.c` as a receiver.
    fn dotted_recv(&mut self, dotted: &[u8], buf: &mut Vec<u8>) -> Rk {
        let mut r = Rk::Unknown;
        for (i, part) in dotted.split(|&b| b == b'.').enumerate() {
            let part = part.trim_ascii();
            if part.is_empty() || i > 8 || !part.iter().all(|&b| b == b'_' || b.is_ascii_alphanumeric() || b >= 0x80) {
                return Rk::Unknown;
            }
            if i == 0 {
                let id = self.cx.intern(part);
                r = self.recv_name(id, buf);
            } else {
                r = recv_step(r, part, buf);
            }
            if r == Rk::Unknown {
                break;
            }
        }
        r
    }

    /// `attr_type` for a dotted prefix given as text (string annotations).
    fn dotted_type(&mut self, prefix: &[u8], name: &[u8]) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let r = self.dotted_recv(prefix, &mut buf);
        let id = if finish_type(r, &mut buf, name) && buf.len() <= MAX_PATH { self.cx.intern(&buf) } else { NONE };
        self.cx.s.buf = buf;
        id
    }

    /// Qualifier of a Type / Extends reference to `obj.Name`: the module `obj` stands for.
    fn type_qual(&mut self, rk: Rk, qual: u32) -> u32 {
        match rk {
            Rk::Module | Rk::Sym => qual,
            // `mod.Outer.Inner`: the module; `Outer.Inner` is looked up like a bare name
            Rk::Type => self.split_qualified(qual).0,
            _ => NONE,
        }
    }

    /// Type path of the value of `v`: `NEUTRAL` for `None`, `NONE` when unknown.
    fn infer(&mut self, v: Node<'t>) -> u32 {
        if self.k(v) == K::None {
            return NEUTRAL;
        }
        let (rk, q) = self.recv_of(v);
        if rk == Rk::Path { q } else { NONE }
    }

    /// A type path that is a plain type name (what `Def::type_name` can hold), else `NONE`.
    fn simple(&self, ty: u32) -> u32 {
        if ty == NONE || ty == NEUTRAL {
            return NONE;
        }
        let b = self.cx.name_bytes(ty);
        let name = match memchr::memchr(b'|', b) {
            Some(i) => &b[i + 1..],
            None => b,
        };
        if name.iter().any(|c| matches!(c, b':' | b'$' | b'(' | b'^' | b'.')) { NONE } else { ty }
    }

    /// Slot name `owner.field` (`.field` when the owner type is unknown).
    fn slot(&mut self, owner: u32, field: &[u8]) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        if owner != NONE {
            buf.extend_from_slice(self.cx.name_bytes(owner));
        }
        buf.push(b'.');
        buf.extend_from_slice(field);
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// `owner.field = value`: when `value` is a plain non-local name (a function, a constant),
    /// record that it is bound into the slot. Class-like names stay ordinary reads: a class
    /// stored in an attribute is a use of the type, not a callback.
    fn bind_value(&mut self, v: Node<'t>, owner: u32, field: &[u8]) -> bool {
        if self.k(v) != K::Identifier || v.start_byte() == v.end_byte() {
            return false;
        }
        let name = self.cx.name(v);
        let info = self.info(name);
        let Bare::Global(o, _) = self.bare(name, info) else { return false };
        if (self.info(o) | info) & (N_CAP | N_FILECLASS) != 0 {
            return false;
        }
        let slot = self.slot(owner, field);
        self.cx.add_ref(o, RefKind::Bind, line_of(v), slot, 255);
        true
    }

    // ----- attribute access and calls -----

    /// `obj.name` used as `usage`. Cursor on the attribute node. Returns the index of the
    /// reference emitted for `name` (`usize::MAX` if none).
    fn attr_access(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, usage: Use) -> usize {
        let t = self.t;
        let (Some(obj), Some(at)) = (child(n, t.f_object), child(n, t.f_attribute)) else {
            self.children(c);
            return usize::MAX;
        };
        let (rk, qual) = self.recv_of(obj);
        let idx = self.emit_member(at, rk, qual, usage);
        self.visit_object(obj, rk, c);
        idx
    }

    fn emit_member(&mut self, at: Node<'t>, rk: Rk, qual: u32, usage: Use) -> usize {
        let text = self.cx.text(at);
        if text.is_empty() || (usage != Use::Call && is_noise_attr(text)) {
            return usize::MAX;
        }
        let module_like = matches!(rk, Rk::Module | Rk::Sym);
        let kind = match usage {
            Use::Call if module_like || rk == Rk::Type => RefKind::Call,
            Use::Call => RefKind::MemberCall,
            Use::Read if module_like => RefKind::Read,
            // `Outer.Inner`: a nested class, not a field
            Use::Read if rk == Rk::Type && is_capwords(text) => RefKind::Read,
            Use::Read => RefKind::MemberRead,
            Use::Write if module_like => RefKind::Write,
            Use::Write => RefKind::MemberWrite,
        };
        let name = self.cx.name(at);
        let idx = self.cx.facts.refs.len();
        self.cx.add_ref(name, kind, line_of(at), qual, 255);
        idx
    }

    /// Walk the receiver of an attribute access. Cursor on the attribute node. The components
    /// of a module path are not references; a `from`-imported root is.
    fn visit_object(&mut self, obj: Node<'t>, rk: Rk, c: &mut TreeCursor<'t>) {
        match rk {
            Rk::Module => {}
            Rk::Sym => {
                let mut e = obj;
                for _ in 0..8 {
                    match self.k(e) {
                        K::Attribute => match child(e, self.t.f_object) {
                            Some(x) => e = x,
                            None => return,
                        },
                        K::Identifier => return self.identifier(e),
                        _ => return,
                    }
                }
            }
            _ => {
                if c.goto_first_child() {
                    loop {
                        if c.node() == obj {
                            self.visit(c);
                            break;
                        }
                        if !c.goto_next_sibling() {
                            break;
                        }
                    }
                    c.goto_parent();
                }
            }
        }
    }

    fn call(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let f = child(n, t.f_function);
        let mut idx = usize::MAX;
        let mut type_first = false;
        if !c.goto_first_child() {
            return;
        }
        loop {
            let ch = c.node();
            if Some(ch) == f {
                (idx, type_first) = self.callee(c, 0);
            } else {
                match self.k(ch) {
                    K::ArgumentList => {
                        let nargs = self.arguments(c, type_first);
                        if let Some(r) = self.cx.facts.refs.get_mut(idx) {
                            r.nargs = nargs;
                        }
                    }
                    K::GeneratorExpression => {
                        self.visit(c);
                        if let Some(r) = self.cx.facts.refs.get_mut(idx) {
                            r.nargs = 1;
                        }
                    }
                    _ => self.visit(c),
                }
            }
            if !c.goto_next_sibling() {
                break;
            }
        }
        c.goto_parent();
    }

    /// The callee of a call; cursor on it. Returns the index of the call reference and whether
    /// the first argument is a type (`cast(T, x)`).
    fn callee(&mut self, c: &mut TreeCursor<'t>, depth: u32) -> (usize, bool) {
        let t = self.t;
        let ch = c.node();
        match self.k(ch) {
            K::Identifier => (self.call_ident(ch), self.cx.text(ch) == b"cast"),
            K::Attribute => {
                let at = child(ch, t.f_attribute);
                match (child(ch, t.f_object), at) {
                    (Some(obj), Some(a)) if self.cx.text(a) == b"__class__" => {
                        let idx = self.own_class_call(obj, a);
                        self.visit_object(obj, Rk::Unknown, c);
                        (idx, false)
                    }
                    _ => (self.attr_access(ch, c, Use::Call), at.is_some_and(|a| self.cx.text(a) == b"cast")),
                }
            }
            K::Call => {
                let idx = match self.type_of_arg(ch) {
                    Some(x) => self.own_class_call(x, ch),
                    None => usize::MAX,
                };
                self.visit(c);
                (idx, false)
            }
            // `(f)(x)`, and `*f(x)`, which the grammar reads as `(*f)(x)` in some argument lists
            K::ParenthesizedExpression | K::ListSplat | K::ListSplatPattern if depth < 4 => {
                let mut r = (usize::MAX, false);
                if c.goto_first_child() {
                    loop {
                        let x = c.node();
                        if x.is_named() && !matches!(self.k(x), K::Comment | K::Skip) {
                            r = self.callee(c, depth + 1);
                        }
                        if !c.goto_next_sibling() {
                            break;
                        }
                    }
                    c.goto_parent();
                }
                r
            }
            _ => {
                self.visit(c);
                (usize::MAX, false)
            }
        }
    }

    /// `name(...)`. Returns the index of the emitted call reference.
    fn call_ident(&mut self, f: Node<'t>) -> usize {
        if f.start_byte() == f.end_byte() {
            return usize::MAX;
        }
        let name = self.cx.name(f);
        let info = self.info(name);
        let line = line_of(f);
        let idx = self.cx.facts.refs.len();
        match self.bare(name, info) {
            // `cls(...)` in a classmethod instantiates the class; other locals are opaque callables
            Bare::Local if name == self.cur.recv && self.cur.flags & F_RECV_CLS != 0 && self.cur.class_name != NONE => {
                self.cx.add_ref(self.cur.class_name, RefKind::Call, line, NONE, 255);
            }
            Bare::Local | Bare::Silent => return usize::MAX,
            // a function of the class body called while the class is being built
            Bare::Member => self.cx.add_ref(name, RefKind::Call, line, self.cur.class_name, 255),
            Bare::Global(o, q) => self.cx.add_ref(o, RefKind::Call, line, q, 255),
        }
        idx
    }

    /// Cursor on an `argument_list`. Returns the number of arguments, 255 when `*` / `**`
    /// unpacking hides it. With `type_first` the first argument is a type (`cast(T, x)`).
    fn arguments(&mut self, c: &mut TreeCursor<'t>, type_first: bool) -> u8 {
        let mut count = 0u32;
        let mut splat = false;
        if c.goto_first_child() {
            loop {
                let a = c.node();
                match self.k(a) {
                    K::Comment | K::Skip => {}
                    K::ListSplat | K::DictionarySplat => {
                        splat = true;
                        self.visit(c);
                    }
                    _ if a.is_named() => {
                        if count == 0 && type_first {
                            self.type_expr(c);
                        } else {
                            self.visit(c);
                        }
                        count += 1;
                    }
                    _ => {}
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        if splat { 255 } else { count.min(254) as u8 }
    }

    /// `name=value`: the name is a parameter of the callee, not a reference.
    fn keyword_argument(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let name = child(n, self.t.f_name);
        if c.goto_first_child() {
            loop {
                if Some(c.node()) != name {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
    }

    /// `a if cond else b`: the condition runs first, and may bind (`:=`) what `a` reads.
    fn conditional(&mut self, c: &mut TreeCursor<'t>) {
        for pass in 0..2 {
            let mut nth = 0;
            if c.goto_first_child() {
                loop {
                    let ch = c.node();
                    if ch.is_named() && self.k(ch) != K::Comment {
                        if (nth == 1) == (pass == 0) {
                            self.visit(c);
                        }
                        nth += 1;
                    }
                    if !c.goto_next_sibling() {
                        break;
                    }
                }
                c.goto_parent();
            }
        }
    }

    /// Only f-strings contain code.
    fn string(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let s = n.start_byte();
        let head = &self.cx.src[s..(s + 3).min(self.cx.src.len())];
        let mut formatted = false;
        for &b in head {
            match b {
                b'f' | b'F' | b't' | b'T' => {
                    formatted = true;
                    break;
                }
                b'"' | b'\'' => break,
                _ => {}
            }
        }
        if !formatted {
            return;
        }
        if c.goto_first_child() {
            loop {
                if self.k(c.node()) == K::Interpolation {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
    }

    // ----- assignments -----

    #[inline(never)]
    fn assignment(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(left) = child(n, t.f_left) else { return self.children(c) };
        let tyn = child(n, t.f_type);
        let right = child(n, t.f_right);
        let sig_end = right.map_or(n.end_byte(), |r| r.start_byte());
        // The value behind a chain `a = b = v`.
        let mut value = right;
        for _ in 0..8 {
            match value {
                Some(v) if self.k(v) == K::Assignment => value = child(v, t.f_right),
                _ => break,
            }
        }
        let lk = self.k(left);
        let mut def = NONE; // definition whose initialiser is being walked
        let mut lambda_def = NONE; // the right-hand side is the body of this function
        let mut consumed = false; // the right-hand side was recorded as a Bind
        let mut exports = false;
        let mut type_rhs = false;
        let mut left_rk = Rk::Unknown;
        let mut pushed = false;
        match lk {
            K::Identifier if left.start_byte() < left.end_byte() => {
                let name = self.cx.name(left);
                let line = line_of(left);
                let lam = self.lambda_of(right);
                let is_lambda = lam.is_some();
                // An annotation that names no usable type (`Any`, bare `Final`) leaves it to the value.
                let declared = tyn.map(|x| self.type_name_of(x, 0)).filter(|ty| *ty != NONE);
                match self.cur.kind {
                    FK::Function => {
                        if self.is_global(name) {
                            self.cx.add_ref(name, RefKind::Write, line, NONE, 255);
                        } else if is_lambda && self.local_ty(name).is_none() {
                            lambda_def = self.lambda_def(name, DefKind::Function, n, lam);
                        } else {
                            match declared {
                                Some(ty) => self.bind_local(name, ty, true),
                                None => {
                                    let ty = value.map_or(NONE, |v| self.infer(v));
                                    self.bind_local(name, ty, false);
                                }
                            }
                        }
                    }
                    FK::Class => {
                        let key = mkey(self.cur.def, name);
                        let exists = self.members.contains_key(&key);
                        if is_lambda && !exists {
                            lambda_def = self.lambda_def(name, DefKind::Method, n, lam);
                        } else {
                            let ty = match declared {
                                Some(ty) => ty,
                                None => {
                                    let p = value.map_or(NONE, |v| self.infer(v));
                                    self.simple(p)
                                }
                            };
                            let kind = if value.is_some_and(|v| self.is_property_call(v)) { DefKind::Property } else { DefKind::Field };
                            def = self.field_def(left, name, kind, n, sig_end, ty);
                            if def != NONE {
                                let mut flags = 0;
                                if self.cx.is_header {
                                    flags |= dflag::DECL;
                                }
                                if is_lambda || tyn.is_some_and(|x| self.is_callable_type(x)) {
                                    flags |= dflag::FNPTR;
                                }
                                self.cx.def_mut(def).flags |= flags;
                            }
                            if def != NONE {
                                self.cx.push_def(def);
                                pushed = true;
                            }
                            if !is_lambda && tyn.is_none() {
                                if let Some(r) = right {
                                    consumed = self.bind_value(r, self.cur.class_name, self.cx.text(left));
                                }
                            }
                        }
                    }
                    FK::Module => {
                        if self.members.contains_key(&mkey(NONE, name)) {
                            self.cx.add_ref(name, RefKind::Write, line, NONE, 255);
                        } else if is_lambda {
                            lambda_def = self.lambda_def(name, DefKind::Function, n, lam);
                        } else {
                            let info = self.info(name);
                            let mut kind = if info & N_UPPER != 0 { DefKind::Constant } else { DefKind::Variable };
                            let mut ty = NONE;
                            match declared {
                                Some(dt) => {
                                    if dt != NONE && matches!(self.cx.name_bytes(dt), b"TypeAlias" | b"typing|TypeAlias" | b"typing_extensions|TypeAlias") {
                                        kind = DefKind::TypeAlias;
                                        type_rhs = true;
                                        ty = value.map_or(NONE, |v| self.type_name_of(v, 0));
                                    } else {
                                        ty = dt;
                                    }
                                }
                                None => {
                                    if let Some(v) = value {
                                        // `Handler = BaseHandler`: another name for a class
                                        let aliased = if info & N_CAP != 0 && right == value { self.class_ref(v) } else { NONE };
                                        if aliased != NONE {
                                            kind = DefKind::TypeAlias;
                                            ty = aliased;
                                        } else {
                                            let p = self.infer(v);
                                            ty = self.simple(p);
                                        }
                                    }
                                }
                            }
                            def = self.global_def(left, name, kind, n, sig_end, ty);
                            if def != NONE && self.cx.is_header {
                                self.cx.def_mut(def).flags |= dflag::DECL;
                            }
                        }
                        exports = self.cx.text(left) == b"__all__";
                    }
                }
            }
            K::Attribute => {
                (consumed, left_rk) = self.assign_attr(n, left, tyn, value, right, sig_end);
            }
            _ => {}
        }
        // Walk the parts in source order.
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if ch == left {
                    match lk {
                        K::Identifier => {}
                        K::Attribute => {
                            if let Some(obj) = child(ch, t.f_object) {
                                self.visit_object(obj, left_rk, c);
                            }
                        }
                        _ => {
                            let d = self.target(c, Tgt { how: How::Assign, stmt: Some(n), sig_end, ty: NONE });
                            if d != NONE {
                                def = d;
                            }
                        }
                    }
                    if def != NONE && !pushed {
                        self.cx.push_def(def);
                        pushed = true;
                    }
                } else if Some(ch) == tyn {
                    self.type_expr(c);
                } else if Some(ch) == right {
                    if lambda_def != NONE {
                        self.named_lambda(c, lambda_def);
                    } else if consumed {
                    } else if type_rhs {
                        self.type_expr(c);
                    } else if exports && matches!(self.k(ch), K::List | K::Tuple) {
                        self.collect_exports(c);
                    } else {
                        self.visit(c);
                    }
                } else {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        if pushed {
            self.cx.pop_def();
        }
    }

    /// `obj.attr = value`. Emits the write (or defines the field when `obj` is `self`) and the
    /// slot binding. Returns whether the value was consumed by a Bind, and what `obj` is.
    fn assign_attr(
        &mut self,
        n: Node<'t>,
        left: Node<'t>,
        tyn: Option<Node<'t>>,
        value: Option<Node<'t>>,
        right: Option<Node<'t>>,
        sig_end: usize,
    ) -> (bool, Rk) {
        let t = self.t;
        let (Some(obj), Some(at)) = (child(left, t.f_object), child(left, t.f_attribute)) else { return (false, Rk::Unknown) };
        let (rk, qual) = self.recv_of(obj);
        let text = self.cx.text(at);
        if text.is_empty() {
            return (false, rk);
        }
        // `self.x = v` in a method: the first assignment declares field `x` of the class.
        let on_self = rk == Rk::Path
            && self.cur.recv != NONE
            && self.cur.class_def != NONE
            && self.k(obj) == K::Identifier
            && self.cx.name(obj) == self.cur.recv
            && !is_noise_attr(text);
        let mut created = false;
        if on_self {
            let name = self.cx.name(at);
            let mut ty = tyn.map_or(NONE, |x| self.type_name_of(x, 0));
            if ty == NONE {
                let p = value.map_or(NONE, |v| self.infer(v));
                ty = self.simple(p);
            }
            let key = mkey(self.cur.class_def, name);
            match self.members.get(&key).copied() {
                Some(d) => self.upgrade_type(d, ty),
                None => {
                    let d = self.cx.def(name, DefKind::Field, n, sig_end);
                    let parent = self.cur.class_def;
                    let callable = right.is_some_and(|r| self.k(r) == K::Lambda) || tyn.is_some_and(|x| self.is_callable_type(x));
                    let dm = self.cx.def_mut(d);
                    dm.parent = parent;
                    dm.type_name = ty;
                    if callable {
                        dm.flags |= dflag::FNPTR;
                    }
                    self.members.insert(key, d);
                    created = true;
                }
            }
        }
        if !created {
            self.emit_member(at, rk, qual, Use::Write);
        }
        let mut consumed = false;
        if !matches!(rk, Rk::Module | Rk::Sym) {
            if let Some(r) = right {
                consumed = self.bind_value(r, qual, text);
            }
        }
        (consumed, rk)
    }

    /// Fill in the type of a field whose first assignment did not reveal it (`self.x = None`).
    fn upgrade_type(&mut self, d: u32, ty: u32) {
        if ty == NONE {
            return;
        }
        let dm = self.cx.def_mut(d);
        if dm.kind == DefKind::Field && dm.type_name == NONE {
            dm.type_name = ty;
        }
    }

    /// Module-level binding of `name`: the first one is the definition, later ones are writes.
    fn global_def(&mut self, id: Node<'t>, name: u32, kind: DefKind, stmt: Node<'t>, sig_end: usize, ty: u32) -> u32 {
        let key = mkey(NONE, name);
        if self.members.contains_key(&key) {
            self.cx.add_ref(name, RefKind::Write, line_of(id), NONE, 255);
            return NONE;
        }
        let d = self.cx.def(name, kind, stmt, sig_end);
        self.cx.def_mut(d).type_name = ty;
        self.members.insert(key, d);
        self.set_info(name, N_FILEDEF);
        self.unshadow(name);
        d
    }

    /// Class-level binding of `name`: the first one defines the attribute.
    fn field_def(&mut self, id: Node<'t>, name: u32, kind: DefKind, stmt: Node<'t>, sig_end: usize, ty: u32) -> u32 {
        let key = mkey(self.cur.def, name);
        if let Some(&d) = self.members.get(&key) {
            self.upgrade_type(d, ty);
            self.cx.add_ref(name, RefKind::MemberWrite, line_of(id), self.cur.class_name, 255);
            return NONE;
        }
        let d = self.cx.def(name, kind, stmt, sig_end);
        let parent = self.cur.def;
        let dm = self.cx.def_mut(d);
        dm.parent = parent;
        dm.type_name = ty;
        self.members.insert(key, d);
        d
    }

    /// The lambda an assignment binds directly: `lambda ...` or `(lambda ...)`.
    fn lambda_of(&self, v: Option<Node<'t>>) -> Option<Node<'t>> {
        let mut v = v?;
        for _ in 0..3 {
            match self.k(v) {
                K::Lambda => return Some(v),
                K::ParenthesizedExpression => v = v.named_child(0)?,
                _ => return None,
            }
        }
        None
    }

    /// Walk the right-hand side found by `lambda_of` as the body of function `def`.
    fn named_lambda(&mut self, c: &mut TreeCursor<'t>, def: u32) {
        let n = c.node();
        if self.k(n) == K::Lambda {
            return self.lambda(n, c, def);
        }
        if c.goto_first_child() {
            loop {
                if matches!(self.k(c.node()), K::Lambda | K::ParenthesizedExpression) {
                    self.named_lambda(c, def);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
    }

    /// `name = lambda ...`: a function called `name`.
    fn lambda_def(&mut self, name: u32, kind: DefKind, stmt: Node<'t>, lam: Option<Node<'t>>) -> u32 {
        let Some(lam) = lam else { return NONE };
        let sig_end = match child(lam, self.t.f_parameters) {
            Some(p) => p.end_byte(),
            None => (lam.start_byte() + 6).min(lam.end_byte()),
        };
        let d = self.cx.def(name, kind, stmt, sig_end);
        self.cx.def_mut(d).parent = self.cur.def;
        match self.cur.kind {
            FK::Class => {
                self.members.insert(mkey(self.cur.def, name), d);
            }
            FK::Module => {
                self.members.insert(mkey(NONE, name), d);
                self.set_info(name, N_FILEDEF);
                self.unshadow(name);
            }
            FK::Function => {}
        }
        d
    }

    /// The class a value expression names (`Base`, `pkg.Base`), or `NONE`.
    fn class_ref(&mut self, v: Node<'t>) -> u32 {
        match self.k(v) {
            K::Identifier => {
                let name = self.cx.name(v);
                let info = self.info(name);
                if info & N_LOCAL != 0 && self.local_ty(name).is_some() {
                    return NONE;
                }
                let o = self.orig(name, info);
                if self.info(o) & N_CAP != 0 { self.orig_type(name, info) } else { NONE }
            }
            K::Attribute => match (child(v, self.t.f_object), child(v, self.t.f_attribute)) {
                (Some(obj), Some(at)) if is_capwords(self.cx.text(at)) => self.attr_type(obj, self.cx.text(at)),
                _ => NONE,
            },
            _ => NONE,
        }
    }

    /// `x = property(getter, setter)`
    fn is_property_call(&self, v: Node<'t>) -> bool {
        self.k(v) == K::Call && child(v, self.t.f_function).is_some_and(|f| self.k(f) == K::Identifier && self.cx.text(f).ends_with(b"property"))
    }

    /// `Callable[...]` annotation: the attribute holds a function.
    fn is_callable_type(&self, ty: Node<'t>) -> bool {
        let text = self.cx.text(ty);
        text.starts_with(b"Callable") || text.starts_with(b"typing.Callable") || text.starts_with(b"Optional[Callable")
    }

    /// `__all__ = ["a", "b"]`. Cursor on the list / tuple.
    fn collect_exports(&mut self, c: &mut TreeCursor<'t>) {
        if c.goto_first_child() {
            loop {
                let s = c.node();
                if self.k(s) == K::String {
                    let inner = string_inner(self.cx.text(s));
                    if !inner.is_empty() && inner.len() < 200 {
                        let id = self.cx.intern(inner);
                        self.set_info(id, N_EXPORT);
                    }
                } else {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
    }

    fn augmented(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let left = child(n, self.t.f_left);
        let exports = self.cur.kind == FK::Module && left.is_some_and(|l| self.cx.text(l) == b"__all__");
        if !c.goto_first_child() {
            return;
        }
        loop {
            let ch = c.node();
            if Some(ch) == left {
                self.target(c, Tgt { how: How::Aug, stmt: None, sig_end: 0, ty: NONE });
            } else if exports && matches!(self.k(ch), K::List | K::Tuple) {
                self.collect_exports(c);
            } else {
                self.visit(c);
            }
            if !c.goto_next_sibling() {
                break;
            }
        }
        c.goto_parent();
    }

    /// `name := value`
    fn named_expression(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let (name, value) = (child(n, t.f_name), child(n, t.f_value));
        if c.goto_first_child() {
            loop {
                if Some(c.node()) != name {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        if let Some(nm) = name.filter(|x| self.k(*x) == K::Identifier && x.start_byte() < x.end_byte()) {
            let ty = value.map_or(NONE, |v| self.infer(v));
            let id = self.cx.name(nm);
            self.bind_plain(nm, id, ty, true);
        }
    }

    /// A node in assignment-target position. Returns the first definition it creates.
    fn target(&mut self, c: &mut TreeCursor<'t>, tg: Tgt<'t>) -> u32 {
        let n = c.node();
        let k = self.k(n);
        match k {
            K::Identifier => self.target_ident(n, tg),
            K::Attribute => {
                match (tg.how, tg.stmt, child(n, self.t.f_object)) {
                    // `self.a, self.b = ...`
                    (How::Assign, Some(stmt), Some(obj)) => {
                        let (_, rk) = self.assign_attr(stmt, n, None, None, None, tg.sig_end);
                        self.visit_object(obj, rk, c);
                    }
                    _ => {
                        self.attr_access(n, c, Use::Write);
                    }
                }
                NONE
            }
            K::Subscript => {
                self.target_subscript(c);
                NONE
            }
            K::PatternList
            | K::TuplePattern
            | K::ListPattern
            | K::Tuple
            | K::List
            | K::ListSplatPattern
            | K::ListSplat
            | K::AsPatternTarget
            | K::ParenthesizedExpression => {
                if self.cx.depth >= MAX_DEPTH {
                    return NONE;
                }
                self.cx.depth += 1;
                // Unpacking loses the type of the value; plain wrappers keep it.
                let inner = if matches!(k, K::AsPatternTarget | K::ParenthesizedExpression) { tg } else { Tgt { ty: NONE, ..tg } };
                let mut first = NONE;
                if c.goto_first_child() {
                    loop {
                        let d = self.target(c, inner);
                        if first == NONE {
                            first = d;
                        }
                        if !c.goto_next_sibling() {
                            break;
                        }
                    }
                    c.goto_parent();
                }
                self.cx.depth -= 1;
                first
            }
            K::Comment => NONE,
            _ => {
                self.visit(c);
                NONE
            }
        }
    }

    fn target_ident(&mut self, n: Node<'t>, tg: Tgt<'t>) -> u32 {
        if n.start_byte() == n.end_byte() {
            return NONE;
        }
        let name = self.cx.name(n);
        match tg.how {
            How::Bind => self.bind_plain(n, name, tg.ty, false),
            How::Aug => self.write_ident(n, name),
            How::Assign => {
                let Some(stmt) = tg.stmt else { return NONE };
                match self.cur.kind {
                    FK::Function => {
                        if self.is_global(name) {
                            self.cx.add_ref(name, RefKind::Write, line_of(n), NONE, 255);
                        } else {
                            self.bind_local(name, tg.ty, false);
                        }
                    }
                    FK::Class => return self.field_def(n, name, DefKind::Field, stmt, tg.sig_end, NONE),
                    FK::Module => {
                        let kind = if self.info(name) & N_UPPER != 0 { DefKind::Constant } else { DefKind::Variable };
                        return self.global_def(n, name, kind, stmt, tg.sig_end, NONE);
                    }
                }
            }
        }
        NONE
    }

    /// `base[i] = v` mutates what `base` holds: a write to it, not a rebinding. Cursor on the
    /// subscript.
    fn target_subscript(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        let n = c.node();
        let value = child(n, self.t.f_value);
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if Some(ch) == value {
                    match self.k(ch) {
                        K::Identifier => {
                            if ch.start_byte() < ch.end_byte() {
                                let name = self.cx.name(ch);
                                self.write_ident(ch, name);
                            }
                        }
                        K::Attribute => {
                            self.attr_access(ch, c, Use::Write);
                        }
                        K::Subscript => self.target_subscript(c),
                        _ => self.visit(c),
                    }
                } else {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.cx.depth -= 1;
    }

    /// `for left in right: ...` and the `for` clause of a comprehension.
    fn for_loop(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let left = child(n, self.t.f_left);
        if !c.goto_first_child() {
            return;
        }
        loop {
            if Some(c.node()) == left {
                self.target(c, Tgt { how: How::Bind, stmt: None, sig_end: 0, ty: NONE });
            } else {
                self.visit(c);
            }
            if !c.goto_next_sibling() {
                break;
            }
        }
        c.goto_parent();
    }

    fn except_clause(&mut self, c: &mut TreeCursor<'t>) {
        if !c.goto_first_child() {
            return;
        }
        loop {
            if self.k(c.node()) == K::AsPattern {
                self.as_pattern(c, true);
            } else {
                self.visit(c);
            }
            if !c.goto_next_sibling() {
                break;
            }
        }
        c.goto_parent();
    }

    /// `expr as target` (`with`, `except`) or `pattern as name` (`match`). Cursor on the node.
    fn as_pattern(&mut self, c: &mut TreeCursor<'t>, is_except: bool) {
        let mut ty = NONE;
        let mut in_match = false;
        if !c.goto_first_child() {
            return;
        }
        loop {
            let ch = c.node();
            match self.k(ch) {
                K::CasePattern => {
                    in_match = true;
                    self.pattern(c);
                }
                K::AsPatternTarget => {
                    self.target(c, Tgt { how: How::Bind, stmt: None, sig_end: 0, ty });
                }
                K::Identifier if in_match => {
                    if ch.start_byte() < ch.end_byte() {
                        let name = self.cx.name(ch);
                        self.bind_plain(ch, name, NONE, false);
                    }
                }
                K::Comment => {}
                _ => {
                    // `except Error as e`: `e` is an `Error`
                    if is_except && ch.is_named() {
                        ty = self.class_name_of(ch);
                    }
                    self.visit(c);
                }
            }
            if !c.goto_next_sibling() {
                break;
            }
        }
        c.goto_parent();
    }

    /// Simple name of the class an expression names, without judging its spelling.
    fn class_name_of(&mut self, e: Node<'t>) -> u32 {
        match self.k(e) {
            K::Identifier if e.start_byte() < e.end_byte() => {
                let name = self.cx.name(e);
                let info = self.info(name);
                if info & N_LOCAL != 0 && self.local_ty(name).is_some() {
                    return NONE;
                }
                self.orig_type(name, info)
            }
            K::Attribute => match (child(e, self.t.f_object), child(e, self.t.f_attribute)) {
                (Some(obj), Some(at)) if at.start_byte() < at.end_byte() => self.attr_type(obj, self.cx.text(at)),
                _ => NONE,
            },
            _ => NONE,
        }
    }

    fn comprehension(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let body = child(n, self.t.f_body);
        self.cx.push_scope();
        self.comp_depth += 1;
        let mark = self.walrus.len();
        // The clauses first: they bind the names the element expression uses.
        if c.goto_first_child() {
            loop {
                if Some(c.node()) != body {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        if body.is_some() && c.goto_first_child() {
            loop {
                if Some(c.node()) == body {
                    self.visit(c);
                    break;
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.comp_depth -= 1;
        self.cx.pop_scope();
        // Names bound by `:=` belong to the scope around the comprehension.
        for i in mark..self.walrus.len() {
            let w = self.walrus[i];
            self.declare(w, NONE);
        }
        if self.comp_depth == 0 {
            self.walrus.truncate(mark);
        }
    }

    // ----- functions -----

    #[inline(never)]
    fn decorated(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let Some(inner) = child(n, self.t.f_definition) else { return self.children(c) };
        let mut df = 0u8;
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if self.k(ch) == K::Decorator {
                    df |= self.decorator_flags(ch);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        match self.k(inner) {
            K::FunctionDefinition => self.function(n, inner, df, c),
            K::ClassDefinition => self.class(n, inner, c),
            _ => self.children(c),
        }
    }

    fn decorator_flags(&self, dec: Node<'t>) -> u8 {
        let t = self.t;
        let Some(mut e) = dec.named_child(0) else { return 0 };
        if self.k(e) == K::Call {
            match child(e, t.f_function) {
                Some(f) => e = f,
                None => return 0,
            }
        }
        let (text, dotted) = match self.k(e) {
            K::Identifier => (self.cx.text(e), false),
            K::Attribute => match child(e, t.f_attribute) {
                Some(at) => (self.cx.text(at), true),
                None => return 0,
            },
            _ => return 0,
        };
        let mut f = 0;
        if text.ends_with(b"property") || (dotted && matches!(text, b"setter" | b"getter" | b"deleter")) {
            f |= D_PROPERTY;
        }
        if text.starts_with(b"abstract") {
            f |= D_ABSTRACT;
        }
        if text.ends_with(b"staticmethod") {
            f |= D_STATIC;
        }
        if text.ends_with(b"classmethod") {
            f |= D_CLASSMETHOD;
        }
        if text == b"overload" {
            f |= D_OVERLOAD;
        }
        f
    }

    /// `@expr`: an Annotation reference to the decorator; its arguments are ordinary code.
    /// Cursor on the decorator.
    fn decorator(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        if !c.goto_first_child() {
            return;
        }
        loop {
            let e = c.node();
            match self.k(e) {
                K::Identifier => self.annotation_ident(e),
                K::Attribute => self.annotation_attr(e, c),
                K::Call => {
                    let f = child(e, t.f_function);
                    if c.goto_first_child() {
                        loop {
                            let ch = c.node();
                            if Some(ch) == f {
                                match self.k(ch) {
                                    K::Identifier => self.annotation_ident(ch),
                                    K::Attribute => self.annotation_attr(ch, c),
                                    _ => self.visit(c),
                                }
                            } else {
                                self.visit(c);
                            }
                            if !c.goto_next_sibling() {
                                break;
                            }
                        }
                        c.goto_parent();
                    }
                }
                _ => self.visit(c),
            }
            if !c.goto_next_sibling() {
                break;
            }
        }
        c.goto_parent();
    }

    fn annotation_ident(&mut self, e: Node<'t>) {
        if e.start_byte() == e.end_byte() {
            return;
        }
        let name = self.cx.name(e);
        let info = self.info(name);
        self.bare_ref(e, name, info, RefKind::Annotation, RefKind::Annotation);
    }

    /// `@obj.name`. Cursor on the attribute.
    fn annotation_attr(&mut self, e: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let (Some(obj), Some(at)) = (child(e, t.f_object), child(e, t.f_attribute)) else { return self.children(c) };
        let text = self.cx.text(at);
        // `@x.setter` belongs to property `x`: a use of the property, not a decorator of its own.
        if matches!(text, b"setter" | b"getter" | b"deleter") && self.k(obj) == K::Identifier {
            return self.visit_object(obj, Rk::Unknown, c);
        }
        let (rk, qual) = self.recv_of(obj);
        if !text.is_empty() {
            let name = self.cx.name(at);
            self.cx.add_ref(name, RefKind::Annotation, line_of(at), qual, 255);
        }
        self.visit_object(obj, rk, c);
    }

    /// Keep short decorators in the signature; long ones (or ones with comments, which a
    /// one-line signature would garble) would push the `def` out of sight.
    fn trim_decorators(&mut self, d: u32, outer: Node<'t>, inner: Node<'t>, sig_end: usize) {
        let (os, is) = (outer.start_byte(), inner.start_byte());
        if is > os && (is - os > MAX_DECORATOR_SIG || self.cx.src[os..is].contains(&b'#')) {
            let dm = self.cx.def_mut(d);
            dm.sig_off = is as u32;
            dm.sig_len = sig_end.saturating_sub(is).min(u16::MAX as usize) as u16;
        }
    }

    /// `def` / `async def`. `outer` is the node the definition spans (the decorated definition
    /// when there are decorators); the cursor is on `outer`.
    #[inline(never)]
    fn function(&mut self, outer: Node<'t>, inner: Node<'t>, df: u8, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(nm) = child(inner, t.f_name).filter(|x| x.start_byte() < x.end_byte()) else { return self.children(c) };
        let params = child(inner, t.f_parameters);
        let ret = child(inner, t.f_return_type);
        let body = child(inner, t.f_body);
        let tparams = child(inner, t.f_type_parameters);
        let in_class = self.cur.kind == FK::Class;
        let name_text = self.cx.text(nm);
        let name = self.cx.name(nm);
        let kind = if !in_class {
            DefKind::Function
        } else if df & D_PROPERTY != 0 {
            DefKind::Property
        } else {
            DefKind::Method
        };
        let sig_end = ret.or(params).unwrap_or(nm).end_byte();
        let d = self.cx.def(name, kind, outer, sig_end);
        self.trim_decorators(d, outer, inner, sig_end);

        let mut flags = 0u8;
        if self.cx.src[inner.start_byte()..].starts_with(b"async") {
            flags |= dflag::ASYNC;
        }
        if name_text.starts_with(b"test_") || (in_class && self.cur.flags & F_TEST != 0) {
            flags |= dflag::TEST;
        }
        if df & D_ABSTRACT != 0 {
            flags |= dflag::ABSTRACT;
        }
        match body.map(|b| self.body_kind(b)) {
            Some(Body::Stub) => {
                if self.cx.is_header || df & (D_ABSTRACT | D_OVERLOAD) != 0 || (in_class && self.cur.flags & F_STUB != 0) {
                    flags |= dflag::DECL;
                }
            }
            Some(Body::Raises) => flags |= dflag::ABSTRACT,
            _ => {}
        }
        let parent = self.cur.def;
        {
            let dm = self.cx.def_mut(d);
            dm.parent = parent;
            dm.flags = flags;
        }
        match self.cur.kind {
            FK::Class => {
                self.members.entry(mkey(self.cur.def, name)).or_insert(d);
            }
            FK::Module => {
                self.members.entry(mkey(NONE, name)).or_insert(d);
                self.set_info(name, N_FILEDEF);
                self.unshadow(name);
            }
            FK::Function => {}
        }

        self.cx.push_def(d);
        if let Some(tp) = tparams {
            self.cx.push_scope();
            self.declare_type_params(tp);
        }
        if let Some(r) = ret {
            let ty = self.type_name_of(r, 0);
            self.cx.def_mut(d).type_name = ty;
        }
        let base = self.params.len();
        let method = in_class && df & D_STATIC == 0;
        let mut recv = NONE;
        let mut seen_params = params.is_none();
        let mut entered = false;
        let imports_mark = self.import_undo.len();

        // Decorators, then the parts of the definition in source order.
        let decorated = outer != inner;
        let mut descended = false;
        let mut at_inner = !decorated;
        if decorated && c.goto_first_child() {
            descended = true;
            loop {
                let ch = c.node();
                if ch == inner {
                    at_inner = true;
                    break;
                }
                if self.k(ch) == K::Decorator {
                    self.decorator(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
        }
        if at_inner && c.goto_first_child() {
            loop {
                let ch = c.node();
                if Some(ch) == params {
                    let class_name = if method { self.cur.class_name } else { NONE };
                    let cls_first = method && (df & D_CLASSMETHOD != 0 || matches!(name_text, b"__new__" | b"__init_subclass__" | b"__class_getitem__"));
                    let (amin, amax, r) = self.parameters(c, class_name, cls_first);
                    recv = r;
                    let dm = self.cx.def_mut(d);
                    dm.arity_min = amin;
                    dm.arity_max = amax;
                    // Calling the class calls `__init__`.
                    if in_class && name_text == b"__init__" && self.cur.def != NONE {
                        let cd = self.cx.def_mut(self.cur.def);
                        cd.arity_min = amin;
                        cd.arity_max = amax;
                    }
                    seen_params = true;
                } else if Some(ch) == body || (seen_params && self.k(ch) == K::Error) {
                    // The body, or a piece of it that error recovery left outside the block.
                    if !entered {
                        entered = true;
                        let recv_cls = recv != NONE
                            && (df & D_CLASSMETHOD != 0
                                || matches!(name_text, b"__new__" | b"__init_subclass__" | b"__class_getitem__")
                                || self.cx.name_bytes(recv) != b"self");
                        self.enter(d, recv, recv_cls, base);
                        self.cx.push_scope();
                    }
                    let gbase = self.tmp.len();
                    self.prescan(c);
                    if self.cx.src.get(ch.start_byte()..ch.end_byte()).is_some_and(|b| memchr::memmem::find(b, b":=").is_some()) {
                        self.prescan_walrus(c);
                    }
                    for i in gbase..self.tmp.len() {
                        let g = self.tmp[i];
                        self.declare(g, GLOBAL);
                    }
                    self.tmp.truncate(gbase);
                    self.declare_params(base);
                    if Some(ch) == body {
                        self.children(c);
                    } else {
                        self.visit(c);
                    }
                } else if matches!(self.k(ch), K::Type | K::TypeParameter) {
                    self.type_expr(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        if entered {
            self.restore_imports(imports_mark);
            self.cx.pop_scope();
            self.leave();
        }
        if descended {
            c.goto_parent();
        }
        self.params.truncate(base);
        if tparams.is_some() {
            self.cx.pop_scope();
        }
        self.cx.pop_def();
    }

    /// Cursor on `parameters` / `lambda_parameters`. Default values and annotations belong to
    /// the enclosing scope and are walked here; the names are queued in `self.params` until the
    /// function's own scope opens. Returns (arity_min, arity_max, receiver name).
    ///
    /// Arity counts every parameter a call can name (positional and keyword-only), without a
    /// leading `self` / `cls`; it matches call sites, where keyword arguments are counted too.
    /// With `class_name` set (a method), that leading parameter is an instance of the class.
    /// With `cls_first` (a classmethod, `__new__`) the leading parameter is the receiver
    /// whatever it is called (`klass`, `mcls`, `typ`).
    fn parameters(&mut self, c: &mut TreeCursor<'t>, class_name: u32, cls_first: bool) -> (u8, u8, u32) {
        let t = self.t;
        let (mut total, mut required) = (0u32, 0u32);
        let mut variadic = false;
        let mut first = true;
        let mut recv = NONE;
        if c.goto_first_child() {
            loop {
                let p = c.node();
                let k = self.k(p);
                // (name, has a default, annotation, 0 = plain / 1 = *args / 2 = **kwargs)
                let part: Option<(Option<Node<'t>>, bool, Option<Node<'t>>, u8)> = match k {
                    K::Identifier => Some((Some(p), false, None, 0)),
                    K::TypedParameter => {
                        let ty = child(p, t.f_type);
                        match p.named_child(0) {
                            Some(x) => match self.k(x) {
                                K::ListSplatPattern => Some((x.named_child(0), false, ty, 1)),
                                K::DictionarySplatPattern => Some((x.named_child(0), false, ty, 2)),
                                _ => Some((Some(x), false, ty, 0)),
                            },
                            None => None,
                        }
                    }
                    K::DefaultParameter => Some((child(p, t.f_name), true, None, 0)),
                    K::TypedDefaultParameter => Some((child(p, t.f_name), true, child(p, t.f_type), 0)),
                    K::ListSplatPattern => Some((p.named_child(0), false, None, 1)),
                    K::DictionarySplatPattern => Some((p.named_child(0), false, None, 2)),
                    K::TuplePattern => Some((Some(p), false, None, 0)),
                    _ => None,
                };
                if let Some((id, has_default, tyn, star)) = part {
                    let ident = id.filter(|x| self.k(*x) == K::Identifier && x.start_byte() < x.end_byte());
                    if star != 0 {
                        variadic = true;
                        first = false;
                        if let Some(x) = ident {
                            let name = self.cx.name(x);
                            // (inferred, not declared: `args = list(args)` rebinds the name)
                            let ty = self.cx.intern(if star == 1 { b"tuple" } else { b"dict" });
                            self.params.push((name, ty));
                        }
                    } else {
                        let is_recv = first && ident.is_some_and(|x| cls_first || is_receiver_name(self.cx.text(x)));
                        first = false;
                        match ident {
                            Some(x) => {
                                let name = self.cx.name(x);
                                let state = if is_recv && class_name != NONE {
                                    recv = name;
                                    class_name | STICKY
                                } else if is_recv {
                                    NONE
                                } else {
                                    match tyn.map(|ty| self.type_name_of(ty, 0)) {
                                        Some(ty) if ty != NONE => ty | STICKY,
                                        _ => NONE,
                                    }
                                };
                                self.params.push((name, state));
                            }
                            // Python 2 tuple parameter: `def f(a, (b, c))`
                            None => {
                                if let Some(x) = id {
                                    self.queue_pattern(x, 0);
                                }
                            }
                        }
                        if !is_recv {
                            total += 1;
                            if !has_default {
                                required += 1;
                            }
                        }
                    }
                    if matches!(k, K::TypedParameter | K::DefaultParameter | K::TypedDefaultParameter) {
                        let value = child(p, t.f_value);
                        if c.goto_first_child() {
                            loop {
                                let ch = c.node();
                                if self.k(ch) == K::Type {
                                    self.type_expr(c);
                                } else if Some(ch) == value {
                                    self.visit(c);
                                }
                                if !c.goto_next_sibling() {
                                    break;
                                }
                            }
                            c.goto_parent();
                        }
                    }
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        (required.min(254) as u8, if variadic { 255 } else { total.min(254) as u8 }, recv)
    }

    fn queue_pattern(&mut self, n: Node<'t>, depth: u32) {
        match self.k(n) {
            K::Identifier if n.start_byte() < n.end_byte() => {
                let name = self.cx.name(n);
                self.params.push((name, NONE));
            }
            K::TuplePattern | K::ListPattern | K::PatternList if depth < 8 => {
                let mut i = 0;
                while let Some(ch) = n.named_child(i) {
                    self.queue_pattern(ch, depth + 1);
                    i += 1;
                }
            }
            _ => {}
        }
    }

    /// `[T, *Ts, **P]` (PEP 695): the parameters are local type names.
    fn declare_type_params(&mut self, tp: Node<'t>) {
        let mut i = 0;
        while let Some(ty) = tp.named_child(i) {
            i += 1;
            let mut e = ty;
            for _ in 0..4 {
                match self.k(e) {
                    K::Type | K::ConstrainedType | K::SplatType => match e.named_child(0) {
                        Some(x) => e = x,
                        None => break,
                    },
                    _ => break,
                }
            }
            if self.k(e) == K::Identifier && e.start_byte() < e.end_byte() {
                let name = self.cx.name(e);
                self.declare(name, NONE);
            }
        }
    }

    fn body_kind(&self, body: Node<'t>) -> Body {
        // 0 = nothing yet, 1 = docstring, 2 = `pass` / `...`, 3 = `raise NotImplementedError`
        let mut state = 0u8;
        let mut i = 0;
        while let Some(st) = body.named_child(i) {
            i += 1;
            if i > 8 {
                return Body::Normal;
            }
            match self.k(st) {
                K::Comment => {}
                K::PassStatement if state <= 2 => state = 2,
                K::ExpressionStatement if state <= 2 => match st.named_child(0).map(|e| self.k(e)) {
                    Some(K::Ellipsis) => state = 2,
                    Some(K::String | K::ConcatenatedString) if state == 0 => state = 1,
                    _ => return Body::Normal,
                },
                K::RaiseStatement if state <= 1 => {
                    let raised = st.named_child(0).map(|e| if self.k(e) == K::Call { child(e, self.t.f_function).unwrap_or(e) } else { e });
                    if raised.is_some_and(|e| self.k(e) == K::Identifier && self.cx.text(e) == b"NotImplementedError") {
                        state = 3;
                    } else {
                        return Body::Normal;
                    }
                }
                _ => return Body::Normal,
            }
        }
        match state {
            1 | 2 => Body::Stub,
            3 => Body::Raises,
            _ => Body::Normal,
        }
    }

    /// `lambda params: body`; `def` is the function it defines when assigned to a name.
    fn lambda(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, def: u32) {
        let t = self.t;
        let body = child(n, t.f_body);
        let base = self.params.len();
        let class_name = if self.cur.kind == FK::Class { self.cur.class_name } else { NONE };
        let mut recv = NONE;
        if def != NONE {
            self.cx.push_def(def);
        }
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if self.k(ch) == K::Parameters {
                    let (amin, amax, r) = self.parameters(c, class_name, false);
                    recv = r;
                    if def != NONE {
                        let dm = self.cx.def_mut(def);
                        dm.arity_min = amin;
                        dm.arity_max = amax;
                    }
                } else if Some(ch) == body {
                    let recv_cls = recv != NONE && self.cx.name_bytes(recv) != b"self";
                    self.enter(def, recv, recv_cls, base);
                    self.cx.push_scope();
                    self.declare_params(base);
                    self.visit(c);
                    self.cx.pop_scope();
                    self.leave();
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.params.truncate(base);
        if def != NONE {
            self.cx.pop_def();
        }
    }

    // ----- classes -----

    #[inline(never)]
    fn class(&mut self, outer: Node<'t>, inner: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(nm) = child(inner, t.f_name).filter(|x| x.start_byte() < x.end_byte()) else { return self.children(c) };
        let supers = child(inner, t.f_superclasses);
        let tparams = child(inner, t.f_type_parameters);
        let body = child(inner, t.f_body);
        let name_text = self.cx.text(nm);
        let name = self.cx.name(nm);
        let sig_end = supers.or(tparams).unwrap_or(nm).end_byte();
        let d = self.cx.def(name, DefKind::Class, outer, sig_end);
        self.trim_decorators(d, outer, inner, sig_end);
        let parent = self.cur.def;
        {
            let dm = self.cx.def_mut(d);
            dm.parent = parent;
            // Constructor arguments are unknown until an `__init__` is seen.
            dm.arity_max = 255;
        }
        match self.cur.kind {
            FK::Class => {
                self.members.entry(mkey(self.cur.def, name)).or_insert(d);
            }
            FK::Module => {
                self.members.entry(mkey(NONE, name)).or_insert(d);
                self.set_info(name, N_FILEDEF);
                self.unshadow(name);
            }
            FK::Function => {}
        }
        self.set_info(name, N_FILECLASS);
        let mut cflags = if name_text.starts_with(b"Test") { F_TEST } else { 0 };

        self.cx.push_def(d);
        if let Some(tp) = tparams {
            self.cx.push_scope();
            self.declare_type_params(tp);
        }
        let decorated = outer != inner;
        let mut descended = false;
        let mut at_inner = !decorated;
        let mut entered = false;
        if decorated && c.goto_first_child() {
            descended = true;
            loop {
                let ch = c.node();
                if ch == inner {
                    at_inner = true;
                    break;
                }
                if self.k(ch) == K::Decorator {
                    self.decorator(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
        }
        if at_inner && c.goto_first_child() {
            loop {
                let ch = c.node();
                if Some(ch) == supers {
                    cflags |= self.superclasses(c);
                } else if Some(ch) == body || self.k(ch) == K::Error {
                    // The body, or a piece of it that error recovery left outside the block.
                    if !entered {
                        entered = true;
                        if cflags & F_TEST != 0 {
                            self.cx.def_mut(d).flags |= dflag::TEST;
                        }
                        self.frames.push(self.cur);
                        self.cur = Frame { kind: FK::Class, def: d, class_def: d, class_name: name, recv: NONE, flags: cflags };
                    }
                    if Some(ch) == body {
                        self.children(c);
                    } else {
                        self.visit(c);
                    }
                } else if self.k(ch) == K::TypeParameter {
                    self.type_expr(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        if entered {
            self.leave();
        }
        if descended {
            c.goto_parent();
        }
        if tparams.is_some() {
            self.cx.pop_scope();
        }
        self.cx.pop_def();
    }

    /// Base classes. Cursor on the `argument_list`. Returns frame flags for the class body.
    fn superclasses(&mut self, c: &mut TreeCursor<'t>) -> u8 {
        let t = self.t;
        let mut flags = 0u8;
        if !c.goto_first_child() {
            return 0;
        }
        loop {
            let b = c.node();
            match self.k(b) {
                K::Identifier | K::Attribute => flags |= self.base(b, c),
                K::Subscript => {
                    // `Base[T]`: the subscripted value is the base, the arguments are types
                    let value = child(b, t.f_value);
                    if c.goto_first_child() {
                        loop {
                            let x = c.node();
                            if Some(x) == value {
                                match self.k(x) {
                                    K::Identifier | K::Attribute => flags |= self.base(x, c),
                                    _ => self.visit(c),
                                }
                            } else {
                                self.type_expr(c);
                            }
                            if !c.goto_next_sibling() {
                                break;
                            }
                        }
                        c.goto_parent();
                    }
                }
                K::KeywordArgument => {
                    let (kn, kv) = (child(b, t.f_name), child(b, t.f_value));
                    let is_meta = kn.is_some_and(|x| self.cx.text(x) == b"metaclass");
                    if is_meta && kv.is_some_and(|v| self.cx.text(v).ends_with(b"ABCMeta")) {
                        flags |= F_STUB;
                    }
                    if c.goto_first_child() {
                        loop {
                            if Some(c.node()) == kv {
                                if is_meta {
                                    self.type_expr(c);
                                } else {
                                    self.visit(c);
                                }
                            }
                            if !c.goto_next_sibling() {
                                break;
                            }
                        }
                        c.goto_parent();
                    }
                }
                K::Comment => {}
                _ => self.visit(c),
            }
            if !c.goto_next_sibling() {
                break;
            }
        }
        c.goto_parent();
        flags
    }

    /// One base class (`Base` or `pkg.Base`): an Extends reference. Cursor on the node.
    fn base(&mut self, b: Node<'t>, c: &mut TreeCursor<'t>) -> u8 {
        let t = self.t;
        match self.k(b) {
            K::Identifier => {
                if b.start_byte() == b.end_byte() {
                    return 0;
                }
                let text = self.cx.text(b);
                let name = self.cx.name(b);
                let info = self.info(name);
                // (a class nested in the enclosing class body is still named plainly)
                match self.bare(name, info) {
                    Bare::Global(o, q) => self.cx.add_ref(o, RefKind::Extends, line_of(b), q, 255),
                    Bare::Member => self.cx.add_ref(name, RefKind::Extends, line_of(b), NONE, 255),
                    Bare::Local | Bare::Silent => {}
                }
                base_flags(text)
            }
            _ => {
                let (Some(obj), Some(at)) = (child(b, t.f_object), child(b, t.f_attribute)) else {
                    self.children(c);
                    return 0;
                };
                let text = self.cx.text(at);
                let (rk, qual) = self.recv_of(obj);
                if !text.is_empty() {
                    let q = self.type_qual(rk, qual);
                    let name = self.cx.name(at);
                    self.cx.add_ref(name, RefKind::Extends, line_of(at), q, 255);
                }
                self.visit_object(obj, rk, c);
                base_flags(text)
            }
        }
    }

    // ----- imports -----

    /// Text of a dotted name, without the whitespace Python allows around the dots.
    fn dotted(&mut self, dn: Node<'t>) -> u32 {
        let text = self.cx.text(dn);
        if text.iter().all(|&b| b == b'.' || b == b'_' || b.is_ascii_alphanumeric() || b >= 0x80) {
            return self.cx.intern(text);
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let mut i = 0;
        while let Some(id) = dn.named_child(i) {
            if self.k(id) == K::Identifier {
                if !buf.is_empty() {
                    buf.push(b'.');
                }
                buf.extend_from_slice(self.cx.text(id));
            }
            i += 1;
        }
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// Module named by a `from` clause, leading dots included (`..pkg.mod`).
    fn module_target(&mut self, m: Node<'t>) -> u32 {
        if self.k(m) != K::RelativeImport {
            return self.dotted(m);
        }
        let mut dots = 0usize;
        let mut rest = NONE;
        let mut i = 0;
        while let Some(ch) = m.named_child(i) {
            match self.k(ch) {
                K::ImportPrefix => dots += self.cx.text(ch).iter().filter(|&&b| b == b'.').count(),
                K::DottedName => rest = self.dotted(ch),
                _ => {}
            }
            i += 1;
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.resize(dots.min(64), b'.');
        if rest != NONE {
            buf.extend_from_slice(self.cx.name_bytes(rest));
        }
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// An import inside a function binds its names only there: remember what to restore.
    fn save_import(&mut self, name: u32, flags: u16) {
        if self.cur.kind == FK::Function {
            let old = self.bound.get(&name).copied().unwrap_or(NONE);
            let old_target = self.sym_target.get(&name).copied().unwrap_or(NONE);
            self.import_undo.push((name, flags & N_IMPORT, old, old_target));
        }
    }

    fn restore_imports(&mut self, mark: usize) {
        while self.import_undo.len() > mark {
            let Some((name, flags, old, old_target)) = self.import_undo.pop() else { break };
            let f = self.info(name);
            self.ninfo[name as usize] = (f & !N_IMPORT) | flags;
            if old == NONE {
                self.bound.remove(&name);
            } else {
                self.bound.insert(name, old);
            }
            if old_target == NONE {
                self.sym_target.remove(&name);
            } else {
                self.sym_target.insert(name, old_target);
            }
        }
    }

    fn bind_alias(&mut self, name: u32, target: u32) {
        let f = self.info(name);
        self.save_import(name, f);
        self.ninfo[name as usize] = (f & !N_IMPORT) | N_ALIAS;
        self.bound.insert(name, target);
    }

    fn bind_sym(&mut self, local: u32, orig: u32, target: u32) {
        let f = self.info(local);
        self.save_import(local, f);
        self.sym_target.insert(local, target);
        let mut nf = (f & !N_IMPORT) | N_SYM;
        if local != orig {
            nf |= N_SYMALIAS;
            self.bound.insert(local, orig);
        }
        self.ninfo[local as usize] = nf;
    }

    /// `import a.b.c`, `import a.b as x`. Cursor on the statement.
    fn import_statement(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        if !c.goto_first_child() {
            return;
        }
        loop {
            let ch = c.node();
            match self.k(ch) {
                K::DottedName => {
                    let target = self.dotted(ch);
                    self.cx.add_import(target, NONE, NONE, line_of(ch), 0);
                    if self.cx.name_bytes(target).split(|&b| b == b'.').skip(1).any(is_classlike) && !self.cap_modules.contains(&target) {
                        self.cap_modules.push(target);
                    }
                    // `import a.b.c` binds `a`: `a.b.c.f()` spells out the rest of the path.
                    if let Some(root) = ch.named_child(0).filter(|r| self.k(*r) == K::Identifier && r.start_byte() < r.end_byte()) {
                        let r = self.cx.name(root);
                        self.bind_alias(r, r);
                    }
                }
                K::AliasedImport => {
                    if let (Some(dn), Some(al)) = (child(ch, t.f_name), child(ch, t.f_alias)) {
                        let target = self.dotted(dn);
                        let alias = self.cx.name(al);
                        self.cx.add_import(target, NONE, alias, line_of(ch), 0);
                        self.bind_alias(alias, target);
                    }
                }
                _ => {}
            }
            if !c.goto_next_sibling() {
                break;
            }
        }
        c.goto_parent();
    }

    /// `from m import a, b as c`, `from m import *`, `from __future__ import x`.
    fn import_from(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, future: bool) {
        let t = self.t;
        let module = child(n, t.f_module_name);
        let target = match module {
            Some(m) => self.module_target(m),
            None if future => self.cx.intern(b"__future__"),
            None => return,
        };
        let flags = if future { iflag::SYSTEM } else { 0 };
        if !c.goto_first_child() {
            return;
        }
        loop {
            let ch = c.node();
            if Some(ch) != module {
                match self.k(ch) {
                    K::DottedName => {
                        let name = self.dotted(ch);
                        self.cx.add_import(target, name, NONE, line_of(ch), flags);
                        if !future {
                            self.cx.add_ref(name, RefKind::Import, line_of(ch), target, 255);
                            self.bind_sym(name, name, target);
                        }
                    }
                    K::AliasedImport => {
                        if let (Some(dn), Some(al)) = (child(ch, t.f_name), child(ch, t.f_alias)) {
                            let name = self.dotted(dn);
                            let alias = self.cx.name(al);
                            self.cx.add_import(target, name, if alias == name { NONE } else { alias }, line_of(ch), flags);
                            if !future {
                                self.cx.add_ref(name, RefKind::Import, line_of(ch), target, 255);
                                self.bind_sym(alias, name, target);
                            }
                        }
                    }
                    K::WildcardImport => self.cx.add_import(target, NONE, NONE, line_of(ch), flags | iflag::WILDCARD),
                    _ => {}
                }
            }
            if !c.goto_next_sibling() {
                break;
            }
        }
        c.goto_parent();
    }

    // ----- types -----

    fn type_children(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        if c.goto_first_child() {
            loop {
                self.type_expr(c);
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.cx.depth -= 1;
    }

    /// An expression in type position (annotation, generic argument, `cast`): the names in it
    /// are Type references.
    fn type_expr(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        let t = self.t;
        match self.k(n) {
            K::Identifier => self.type_ident(n),
            K::Type
            | K::TypeParameter
            | K::UnionType
            | K::ConstrainedType
            | K::SplatType
            | K::BinaryOperator
            | K::List
            | K::Tuple
            | K::ParenthesizedExpression => self.type_children(c),
            K::Attribute => self.type_attr(n, c),
            K::GenericType | K::Subscript => {
                // `Base[Args]`
                let base = if self.k(n) == K::Subscript { child(n, t.f_value) } else { n.child(0) };
                let special = base.map_or(0, |b| type_special(last_component(self.cx.text(b))));
                let mut nth = 0;
                if self.cx.depth >= MAX_DEPTH {
                    return;
                }
                self.cx.depth += 1;
                if c.goto_first_child() {
                    loop {
                        let ch = c.node();
                        if Some(ch) == base {
                            self.type_expr(c);
                        } else if self.k(ch) == K::TypeParameter {
                            match special {
                                TS_LITERAL => {}
                                TS_ANNOTATED => {
                                    // only the first argument is a type, the rest is metadata
                                    let mut first = true;
                                    if c.goto_first_child() {
                                        loop {
                                            if self.k(c.node()) == K::Type {
                                                if first {
                                                    self.type_expr(c);
                                                } else {
                                                    self.children(c);
                                                }
                                                first = false;
                                            }
                                            if !c.goto_next_sibling() {
                                                break;
                                            }
                                        }
                                        c.goto_parent();
                                    }
                                }
                                _ => self.type_expr(c),
                            }
                        } else if ch.is_named() && !matches!(self.k(ch), K::Comment | K::Skip) {
                            match special {
                                TS_LITERAL => {}
                                TS_ANNOTATED if nth > 0 => self.visit(c),
                                _ => self.type_expr(c),
                            }
                            nth += 1;
                        }
                        if !c.goto_next_sibling() {
                            break;
                        }
                    }
                    c.goto_parent();
                }
                self.cx.depth -= 1;
            }
            K::MemberType => {
                // `a.B` spelled as a type: the last identifier is the type
                let last = n.named_child(n.named_child_count().saturating_sub(1) as u32);
                if c.goto_first_child() {
                    loop {
                        let ch = c.node();
                        if Some(ch) == last && self.k(ch) == K::Identifier {
                            if ch.start_byte() < ch.end_byte() {
                                let name = self.cx.name(ch);
                                self.cx.add_ref(name, RefKind::Type, line_of(ch), NONE, 255);
                            }
                        } else if self.k(ch) == K::Type {
                            self.visit_type_prefix(c);
                        }
                        if !c.goto_next_sibling() {
                            break;
                        }
                    }
                    c.goto_parent();
                }
            }
            K::String => {
                if n.named_child_count() > 3 || n.named_child(1).is_some_and(|x| self.k(x) == K::Interpolation) {
                    self.string(n, c);
                } else {
                    self.type_string(n);
                }
            }
            K::Comment | K::None | K::Ellipsis | K::Integer | K::Float | K::True | K::False | K::Skip | K::ConcatenatedString => {}
            _ => self.visit(c),
        }
    }

    /// The `a` of a `member_type` `a.B`: an ordinary expression (usually a module).
    fn visit_type_prefix(&mut self, c: &mut TreeCursor<'t>) {
        if c.goto_first_child() {
            loop {
                if self.k(c.node()) != K::Identifier {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
    }

    fn type_ident(&mut self, n: Node<'t>) {
        if n.start_byte() == n.end_byte() {
            return;
        }
        let name = self.cx.name(n);
        self.type_ref(name, line_of(n));
    }

    fn type_ref(&mut self, name: u32, line: u32) {
        let info = self.info(name);
        match self.bare(name, info) {
            Bare::Global(o, q) => self.cx.add_ref(o, RefKind::Type, line, q, 255),
            // a class nested in the class being defined
            Bare::Member => self.cx.add_ref(name, RefKind::Type, line, NONE, 255),
            Bare::Local | Bare::Silent => {}
        }
    }

    /// `pkg.Type` in type position. Cursor on the attribute.
    fn type_attr(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let (Some(obj), Some(at)) = (child(n, t.f_object), child(n, t.f_attribute)) else { return self.children(c) };
        let (rk, qual) = self.recv_of(obj);
        if at.start_byte() < at.end_byte() {
            let q = self.type_qual(rk, qual);
            let name = self.cx.name(at);
            self.cx.add_ref(name, RefKind::Type, line_of(at), q, 255);
        }
        if rk == Rk::Type && self.k(obj) == K::Identifier {
            // `Outer.Inner`: `Outer` is used as a type too
            self.type_ident(obj);
        } else {
            self.visit_object(obj, rk, c);
        }
    }

    /// A forward reference: `"Foo"`, `"Optional[pkg.Foo]"`.
    fn type_string(&mut self, n: Node<'t>) {
        let inner = string_inner(self.cx.text(n));
        if !looks_like_type(inner) {
            return;
        }
        let line = line_of(n);
        let len = inner.len();
        let mut i = 0;
        while i < len {
            let b = inner[i];
            if b == b'_' || b.is_ascii_alphabetic() || b >= 0x80 {
                let s = i;
                while i < len && (inner[i] == b'_' || inner[i].is_ascii_alphanumeric() || inner[i] >= 0x80) {
                    i += 1;
                }
                let word = &inner[s..i];
                let qualifier = i < len && inner[i] == b'.';
                if !qualifier && !matches!(word, b"None" | b"True" | b"False") {
                    let id = self.cx.intern(word);
                    // `pkg.mod.Name`: the dotted prefix says which module
                    let mut ps = s;
                    while ps > 0 && (inner[ps - 1] == b'.' || inner[ps - 1] == b'_' || inner[ps - 1].is_ascii_alphanumeric() || inner[ps - 1] >= 0x80) {
                        ps -= 1;
                    }
                    if ps < s && inner[s - 1] == b'.' {
                        let mut buf = std::mem::take(&mut self.cx.s.buf);
                        buf.clear();
                        let rk = self.dotted_recv(&inner[ps..s - 1], &mut buf);
                        let q =
                            if matches!(rk, Rk::Module | Rk::Sym | Rk::Type) && !buf.is_empty() && buf.len() <= MAX_PATH { self.cx.intern(&buf) } else { NONE };
                        self.cx.s.buf = buf;
                        let q = if q != NONE { self.type_qual(rk, q) } else { NONE };
                        self.cx.add_ref(id, RefKind::Type, line, q, 255);
                    } else {
                        self.type_ref(id, line);
                    }
                }
                if word == b"Literal" {
                    // its arguments are values
                    let mut depth = 0i32;
                    while i < len {
                        match inner[i] {
                            b'[' => depth += 1,
                            b']' => {
                                depth -= 1;
                                if depth <= 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                }
            } else {
                i += 1;
            }
        }
    }

    /// Simple name of the type an annotation declares: `Foo` for `Foo`, `pkg.Foo`, `"Foo"`,
    /// `Optional[Foo]`, `Foo | None`; the container for other generics (`List[Foo]` -> `List`);
    /// `NONE` for `None`, `Any` and anything undecidable.
    fn type_name_of(&mut self, n: Node<'t>, depth: u32) -> u32 {
        if depth > 6 {
            return NONE;
        }
        let t = self.t;
        match self.k(n) {
            K::Type | K::ParenthesizedExpression => match n.named_child(0) {
                Some(x) => self.type_name_of(x, depth + 1),
                None => NONE,
            },
            K::Identifier => self.type_name_ident(self.cx.text(n)),
            K::Attribute => match (child(n, t.f_object), child(n, t.f_attribute)) {
                (Some(obj), Some(at)) => self.type_name_attr(obj, self.cx.text(at)),
                _ => NONE,
            },
            K::MemberType => {
                let last = n.named_child(n.named_child_count().saturating_sub(1) as u32).filter(|x| self.k(*x) == K::Identifier);
                // (the prefix is a `type` node wrapping an expression)
                let obj = n.named_child(0).and_then(|p| if self.k(p) == K::Type { p.named_child(0) } else { Some(p) });
                match (obj, last) {
                    (Some(obj), Some(x)) if obj != x => self.type_name_attr(obj, self.cx.text(x)),
                    _ => NONE,
                }
            }
            K::GenericType | K::Subscript => {
                let is_sub = self.k(n) == K::Subscript;
                let Some(base) = (if is_sub { child(n, t.f_value) } else { n.child(0) }) else { return NONE };
                let base_name = last_component(self.cx.text(base));
                // arguments: named children after the base
                let args = if is_sub { Some(n) } else { n.named_child(1).filter(|x| self.k(*x) == K::TypeParameter) };
                let skip = if is_sub { 1 } else { 0 };
                match type_special(base_name) {
                    TS_WRAPPER | TS_ANNOTATED => match args.and_then(|a| a.named_child(skip)) {
                        Some(x) => self.type_name_of(x, depth + 1),
                        None => NONE,
                    },
                    TS_UNION => {
                        // `Union[Foo, None]` is `Optional[Foo]`
                        let Some(a) = args else { return NONE };
                        let mut only = None;
                        let mut i = skip;
                        while let Some(x) = a.named_child(i) {
                            i += 1;
                            if self.k(x) == K::Comment || self.is_none_type(x) {
                                continue;
                            }
                            if only.is_some() {
                                return NONE;
                            }
                            only = Some(x);
                        }
                        match only {
                            Some(x) => self.type_name_of(x, depth + 1),
                            None => NONE,
                        }
                    }
                    _ if matches!(self.k(base), K::Attribute | K::MemberType | K::Type) => self.type_name_of(base, depth + 1),
                    _ => self.type_name_ident(base_name),
                }
            }
            K::BinaryOperator | K::UnionType => {
                // `Foo | None`
                let (Some(l), Some(r)) = (n.named_child(0), n.named_child(1)) else { return NONE };
                if self.k(n) == K::BinaryOperator && !self.cx.src.get(l.end_byte()..r.start_byte()).is_some_and(|op| op.contains(&b'|')) {
                    return NONE;
                }
                match (self.is_none_type(l), self.is_none_type(r)) {
                    (false, true) => self.type_name_of(l, depth + 1),
                    (true, false) => self.type_name_of(r, depth + 1),
                    _ => NONE,
                }
            }
            K::String => {
                let inner = string_inner(self.cx.text(n));
                if looks_like_type(inner) { self.type_name_str(inner, 0) } else { NONE }
            }
            _ => NONE,
        }
    }

    fn is_none_type(&self, n: Node<'t>) -> bool {
        match self.k(n) {
            K::None => true,
            K::Type => n.named_child(0).is_some_and(|x| self.k(x) == K::None),
            _ => false,
        }
    }

    /// Name of the type `obj.name`: `TARGET|name` when `obj` is a module.
    fn type_name_attr(&mut self, obj: Node<'t>, name: &'a [u8]) -> u32 {
        if is_untyped_name(name) {
            return NONE;
        }
        self.attr_type(obj, name)
    }

    fn type_name_ident(&mut self, text: &[u8]) -> u32 {
        match text {
            // `Self` is the class being defined
            b"Self" if self.cur.class_name != NONE => self.cur.class_name,
            _ if is_untyped_name(text) => NONE,
            _ => {
                let name = self.cx.intern(text);
                let info = self.info(name);
                if info & N_LOCAL != 0 && self.local_ty(name).is_some() {
                    // a type parameter
                    return NONE;
                }
                self.orig_type(name, info)
            }
        }
    }

    /// `type_name_of` for the text of a string annotation.
    fn type_name_str(&mut self, s: &[u8], depth: u32) -> u32 {
        let s = s.trim_ascii();
        if s.is_empty() || depth > 3 {
            return NONE;
        }
        // `A | None`
        let mut parts = split_top(s, b'|');
        if let (Some(a), Some(b)) = (parts.next(), parts.next()) {
            if parts.next().is_some() {
                return NONE;
            }
            return match (a.trim_ascii() == b"None", b.trim_ascii() == b"None") {
                (false, true) => self.type_name_str(a, depth + 1),
                (true, false) => self.type_name_str(b, depth + 1),
                _ => NONE,
            };
        }
        let (base, args) = match s.iter().position(|&b| b == b'[') {
            Some(i) if s.ends_with(b"]") => (&s[..i], Some(&s[i + 1..s.len() - 1])),
            Some(_) => return NONE,
            None => (s, None),
        };
        let full = base.trim_ascii();
        let base = last_component(full);
        if base.is_empty() || !base.iter().all(|&b| b == b'_' || b.is_ascii_alphanumeric() || b >= 0x80) {
            return NONE;
        }
        // `pkg.mod` of `pkg.mod.Name`
        let prefix = if full.len() > base.len() { full[..full.len() - base.len()].trim_ascii().strip_suffix(b".") } else { None };
        match (type_special(base), args) {
            (TS_WRAPPER | TS_ANNOTATED, Some(a)) => match split_top(a, b',').next() {
                Some(first) => self.type_name_str(first, depth + 1),
                None => NONE,
            },
            (TS_UNION, Some(a)) => {
                let mut only = None;
                for p in split_top(a, b',') {
                    if p.trim_ascii() == b"None" {
                        continue;
                    }
                    if only.is_some() {
                        return NONE;
                    }
                    only = Some(p);
                }
                match only {
                    Some(p) => self.type_name_str(p, depth + 1),
                    None => NONE,
                }
            }
            _ => match prefix {
                Some(_) if is_untyped_name(base) => NONE,
                Some(pre) => self.dotted_type(pre, base),
                None => self.type_name_ident(base),
            },
        }
    }

    /// `type Alias = Target` (PEP 695)
    fn type_alias(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let (left, right) = (child(n, t.f_left), child(n, t.f_right));
        // `Alias` or `Alias[T]`
        let mut nm = left;
        for _ in 0..3 {
            match nm {
                Some(x) if matches!(self.k(x), K::Type | K::GenericType) => nm = x.named_child(0),
                _ => break,
            }
        }
        let Some(nm) = nm.filter(|x| self.k(*x) == K::Identifier && x.start_byte() < x.end_byte()) else {
            return self.misparsed_type_statement(c);
        };
        let name = self.cx.name(nm);
        let sig_end = right.map_or(n.end_byte(), |r| r.start_byte());
        let d = self.cx.def(name, DefKind::TypeAlias, n, sig_end);
        let ty = right.map_or(NONE, |r| self.type_name_of(r, 0));
        let parent = self.cur.def;
        {
            let dm = self.cx.def_mut(d);
            dm.parent = parent;
            dm.type_name = ty;
        }
        if self.cur.kind == FK::Module {
            self.members.entry(mkey(NONE, name)).or_insert(d);
            self.set_info(name, N_FILEDEF);
        }
        self.cx.push_def(d);
        // type parameters of a generic alias are local names
        self.cx.push_scope();
        if let Some(tp) = left.and_then(|l| l.named_child(0)).filter(|g| self.k(*g) == K::GenericType).and_then(|g| g.named_child(1)) {
            self.declare_type_params(tp);
        }
        if c.goto_first_child() {
            loop {
                if Some(c.node()) == right {
                    self.type_expr(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.cx.pop_scope();
        self.cx.pop_def();
    }

    /// The grammar reads `type(x).attr = value` as a type alias whose name is `(x).attr`: it is
    /// an assignment to an attribute of the class of `x`. Cursor on the statement.
    fn misparsed_type_statement(&mut self, c: &mut TreeCursor<'t>) {
        let mut first = true;
        if c.goto_first_child() {
            loop {
                if self.k(c.node()) == K::Type {
                    // step inside the `type` wrapper: both sides are ordinary expressions
                    if c.goto_first_child() {
                        loop {
                            let e = c.node();
                            if first && self.k(e) == K::Attribute {
                                self.attr_access(e, c, Use::Write);
                            } else {
                                self.visit(c);
                            }
                            if !c.goto_next_sibling() {
                                break;
                            }
                        }
                        c.goto_parent();
                    }
                    first = false;
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
    }

    // ----- match patterns -----

    /// A `case` pattern: bare names capture (locals), dotted names and class patterns refer.
    fn pattern(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        let k = self.k(n);
        match k {
            K::DottedName => self.pattern_name(n, false),
            K::SplatPattern | K::Identifier => {
                let id = if k == K::Identifier { Some(n) } else { n.named_child(0) };
                if let Some(id) = id.filter(|x| self.k(*x) == K::Identifier && x.start_byte() < x.end_byte()) {
                    let name = self.cx.name(id);
                    self.bind_plain(id, name, NONE, false);
                }
            }
            K::AsPattern => self.as_pattern(c, false),
            K::CasePattern | K::UnionPattern | K::ListPattern | K::TuplePattern | K::DictPattern | K::ClassPattern | K::KeywordPattern => {
                if self.cx.depth >= MAX_DEPTH {
                    return;
                }
                self.cx.depth += 1;
                let mut first = true;
                if c.goto_first_child() {
                    loop {
                        let ch = c.node();
                        if ch.is_named() {
                            match (k, self.k(ch), first) {
                                // `Point(x=0)`: the class being matched
                                (K::ClassPattern, K::DottedName, true) => self.pattern_name(ch, true),
                                // `x=...` names an attribute of the subject
                                (K::KeywordPattern, K::Identifier, true) => {}
                                _ => self.pattern(c),
                            }
                            first = false;
                        }
                        if !c.goto_next_sibling() {
                            break;
                        }
                    }
                    c.goto_parent();
                }
                self.cx.depth -= 1;
            }
            _ => self.visit(c),
        }
    }

    /// A dotted name in a pattern: `x` captures, `Color.RED` is a value, `Point` in
    /// `Point(...)` a class.
    fn pattern_name(&mut self, n: Node<'t>, is_class: bool) {
        let count = n.named_child_count() as u32;
        let Some(root) = n.named_child(0).filter(|x| self.k(*x) == K::Identifier && x.start_byte() < x.end_byte()) else { return };
        if count == 1 {
            if is_class {
                return self.type_ident(root);
            }
            let name = self.cx.name(root);
            return self.bind_plain(root, name, NONE, false);
        }
        let Some(last) = n.named_child(count - 1).filter(|x| self.k(*x) == K::Identifier && x.start_byte() < x.end_byte()) else { return };
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let mut rk = self.recv_ident(root, &mut buf);
        for i in 1..count.saturating_sub(1).min(8) {
            if let Some(mid) = n.named_child(i).filter(|x| self.k(*x) == K::Identifier) {
                // `A.B.C`: `B` is read off `A` (unless it is a component of a module path)
                if !matches!(rk, Rk::Module | Rk::Sym) {
                    let q = if rk != Rk::Unknown && !buf.is_empty() { self.cx.intern(&buf) } else { NONE };
                    self.emit_member(mid, if q == NONE { Rk::Unknown } else { rk }, q, Use::Read);
                }
                rk = recv_step(rk, self.cx.text(mid), &mut buf);
            }
        }
        let qual = if rk != Rk::Unknown && !buf.is_empty() { self.cx.intern(&buf) } else { NONE };
        self.cx.s.buf = buf;
        if is_class {
            let q = if qual != NONE { self.type_qual(rk, qual) } else { NONE };
            let name = self.cx.name(last);
            self.cx.add_ref(name, RefKind::Type, line_of(last), q, 255);
        } else {
            self.emit_member(last, if qual == NONE { Rk::Unknown } else { rk }, qual, Use::Read);
        }
        if rk != Rk::Module {
            self.identifier(root);
        }
    }

    fn finish(&mut self) {
        // Names listed in `__all__` are the module's public API.
        let ninfo = &self.ninfo;
        let exported = |name: u32| ninfo.get(name as usize).is_some_and(|f| f & N_EXPORT != 0);
        for d in self.cx.facts.defs.iter_mut() {
            if d.parent == NONE && exported(d.name) {
                d.flags |= dflag::EXPORTED;
            }
        }
        // A package's `__init__` re-exports what it imports from its own modules; anywhere
        // else an import listed in `__all__` is a re-export.
        let init = self.cx.path.ends_with("__init__.py") || self.cx.path.ends_with("__init__.pyi");
        let names = &self.cx.facts.names;
        for im in self.cx.facts.imports.iter_mut() {
            let local = if im.alias != NONE { im.alias } else { im.name };
            let relative = names.get(im.target).first() == Some(&b'.');
            if (local != NONE && exported(local)) || (init && (relative || im.flags & iflag::WILDCARD != 0)) {
                im.flags |= iflag::REEXPORT;
            }
        }
    }
}

/// Extend a receiver by one attribute.
fn recv_step(r: Rk, name: &[u8], buf: &mut Vec<u8>) -> Rk {
    match r {
        Rk::Module | Rk::Sym => {
            if is_classlike(name) {
                // `pkg.Class`: from here on the receiver is that class of that module
                buf.push(b'|');
                buf.extend_from_slice(name);
                Rk::Type
            } else if is_constant_name(name) {
                // `pkg.REGISTRY`: an object of that module, never a submodule
                buf.extend_from_slice(b"|$");
                buf.extend_from_slice(name);
                Rk::Path
            } else {
                buf.push(b'.');
                buf.extend_from_slice(name);
                r
            }
        }
        Rk::Type => {
            if is_classlike(name) {
                nested_type(buf, name);
                Rk::Type
            } else {
                buf.extend_from_slice(b"::");
                buf.extend_from_slice(name);
                Rk::Path
            }
        }
        // `super().attr`: `^Class` says where `attr` is looked up, not what it is
        Rk::Path if buf.first() == Some(&b'^') => Rk::Unknown,
        Rk::Path => {
            match name {
                // `x.__class__.attr` looks `attr` up where `x.attr` does
                b"__class__" => {}
                b"__dict__" => {
                    buf.clear();
                    buf.extend_from_slice(b"dict");
                }
                _ => {
                    buf.extend_from_slice(b"::");
                    buf.extend_from_slice(name);
                }
            }
            Rk::Path
        }
        Rk::Unknown => Rk::Unknown,
    }
}

/// `Outer.Inner`: a class nested in a class is named plainly; the module `Outer` was written
/// through stays.
fn nested_type(buf: &mut Vec<u8>, name: &[u8]) {
    let keep = memchr::memchr(b'|', buf).map_or(0, |i| i + 1);
    buf.truncate(keep);
    buf.extend_from_slice(name);
}

/// Turn the receiver `r` (in `buf`) of a name in type position into the name of that type:
/// `TARGET|Name` through a module, `Name` for a nested class. A root no import of the file
/// binds stands for itself (`foo.Bar` is `foo|Bar`). False when the receiver is a value.
fn finish_type(r: Rk, buf: &mut Vec<u8>, name: &[u8]) -> bool {
    match r {
        Rk::Module | Rk::Sym => {
            buf.push(b'|');
            buf.extend_from_slice(name);
            true
        }
        Rk::Type => {
            nested_type(buf, name);
            true
        }
        Rk::Path if buf.first() == Some(&b'$') && !buf.iter().any(|&b| b == b'|' || b == b'(' || b == b':') => {
            buf.remove(0);
            buf.push(b'|');
            buf.extend_from_slice(name);
            true
        }
        _ => false,
    }
}

/// Conventional names of the receiver parameter of a method, classmethod or metaclass method.
fn is_receiver_name(p: &[u8]) -> bool {
    matches!(p, b"self" | b"cls" | b"mcs" | b"metacls")
}

fn base_flags(name: &[u8]) -> u8 {
    let mut f = 0;
    if matches!(name, b"Protocol" | b"ABC") {
        f |= F_STUB;
    }
    if name.ends_with(b"TestCase") {
        f |= F_TEST;
    }
    f
}

const TS_WRAPPER: u8 = 1;
const TS_UNION: u8 = 2;
const TS_LITERAL: u8 = 3;
const TS_ANNOTATED: u8 = 4;

/// Generic "types" that are transparent or are not types at all.
fn type_special(base: &[u8]) -> u8 {
    match base {
        b"Optional" | b"ClassVar" | b"Final" | b"Required" | b"NotRequired" | b"ReadOnly" | b"InitVar" | b"Type" | b"type" => TS_WRAPPER,
        b"Union" => TS_UNION,
        b"Literal" => TS_LITERAL,
        b"Annotated" => TS_ANNOTATED,
        _ => 0,
    }
}

/// A name in type position that names no usable type: `Any`, a bare wrapper, a type variable.
fn is_untyped_name(text: &[u8]) -> bool {
    matches!(text, b"" | b"None" | b"Any" | b"object" | b"Final" | b"ClassVar") || looks_like_typevar(text)
}

/// `T`, `_T`, `KT`, `T_co`, `T1`, `AnyStr`: conventional type-variable names. A value of such a
/// "type" can be anything, so it must not be taken for a class.
fn looks_like_typevar(s: &[u8]) -> bool {
    let mut t = s;
    while let Some((&b'_', rest)) = t.split_first() {
        t = rest;
    }
    for suffix in [b"_co".as_slice(), b"_contra".as_slice()] {
        if let Some(head) = t.strip_suffix(suffix) {
            t = head;
        }
    }
    match t {
        b"AnyStr" => true,
        [a] => a.is_ascii_uppercase(),
        [a, b] => a.is_ascii_uppercase() && (*b == b'T' || b.is_ascii_digit()),
        _ => false,
    }
}

/// `c` of `a.b.c`.
fn last_component(s: &[u8]) -> &[u8] {
    match s.iter().rposition(|&b| b == b'.') {
        Some(i) => s[i + 1..].trim_ascii(),
        None => s,
    }
}

/// Split at `sep` outside brackets.
fn split_top(s: &[u8], sep: u8) -> impl Iterator<Item = &[u8]> {
    let mut depth = 0i32;
    s.split(move |&b| {
        match b {
            b'[' | b'(' => depth += 1,
            b']' | b')' => depth -= 1,
            _ => {}
        }
        b == sep && depth == 0
    })
}

/// Contents of a string literal: prefix letters and quotes removed.
fn string_inner(t: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < t.len() && t[i].is_ascii_alphabetic() {
        i += 1;
    }
    let t = &t[i..];
    for q in [b"\"\"\"".as_slice(), b"'''".as_slice()] {
        if t.len() >= 6 && t.starts_with(q) && t.ends_with(q) {
            return &t[3..t.len() - 3];
        }
    }
    if t.len() >= 2 && (t[0] == b'"' || t[0] == b'\'') && t[t.len() - 1] == t[0] {
        return &t[1..t.len() - 1];
    }
    b""
}

fn string_is_bytes(t: &[u8]) -> bool {
    t.iter().take(2).take_while(|b| b.is_ascii_alphabetic()).any(|&b| b == b'b' || b == b'B')
}

/// Could this string be a type expression (a forward reference) rather than prose?
fn looks_like_type(s: &[u8]) -> bool {
    if s.is_empty() || s.len() > 200 {
        return false;
    }
    let mut word_then_space = false;
    let mut in_word = false;
    for &b in s {
        if b == b'_' || b.is_ascii_alphanumeric() || b >= 0x80 {
            // two words separated only by blanks: prose
            if word_then_space {
                return false;
            }
            in_word = true;
        } else {
            match b {
                b' ' => {
                    if in_word {
                        word_then_space = true;
                    }
                }
                b'.' | b'[' | b']' | b',' | b'|' | b'\'' | b'"' => word_then_space = false,
                _ => return false,
            }
            in_word = false;
        }
    }
    true
}

/// Universal attributes: reading them says nothing about the receiver's class.
fn is_noise_attr(a: &[u8]) -> bool {
    matches!(a, b"__class__" | b"__name__" | b"__dict__" | b"__doc__" | b"__module__" | b"__qualname__")
}

/// `Foo`, `HTTPServer`, `_Private`: first letter upper case, some lower case after it.
fn is_capwords(s: &[u8]) -> bool {
    let mut t = s;
    while let Some((&b'_', rest)) = t.split_first() {
        t = rest;
    }
    t.first().is_some_and(|c| c.is_ascii_uppercase()) && t.iter().any(|b| b.is_ascii_lowercase())
}

/// A name that reads as a class: CapWords, or one capital letter (`Q`, `F`).
fn is_classlike(s: &[u8]) -> bool {
    let mut t = s;
    while let Some((&b'_', rest)) = t.split_first() {
        t = rest;
    }
    match t {
        [c] => c.is_ascii_uppercase(),
        [c, ..] => c.is_ascii_uppercase() && t.iter().any(|b| b.is_ascii_lowercase()),
        [] => false,
    }
}

/// `MAX_SIZE`, `DEBUG`: at least two characters, upper case throughout.
fn is_constant_name(s: &[u8]) -> bool {
    let mut t = s;
    while let Some((&b'_', rest)) = t.split_first() {
        t = rest;
    }
    t.len() >= 2 && t[0].is_ascii_uppercase() && !t.iter().any(|b| b.is_ascii_lowercase())
}

fn lexical_flags(s: &[u8]) -> u16 {
    let mut f = N_SEEN;
    let mut t = s;
    while let Some((&b'_', rest)) = t.split_first() {
        t = rest;
    }
    match t.first() {
        Some(c) if c.is_ascii_uppercase() => {
            if t.iter().any(|b| b.is_ascii_lowercase()) {
                f |= N_CAP;
            } else {
                f |= N_UPPER;
                // `Q`, `F`, `A`: a one-letter name that is called or dotted is a class
                if t.len() == 1 {
                    f |= N_CAP;
                }
            }
        }
        _ => {}
    }
    if is_builtin(s) {
        f |= N_STOP;
    }
    if is_builtin_type(s) {
        f |= N_BTYPE;
    }
    f
}

fn is_builtin_type(w: &[u8]) -> bool {
    matches!(
        w,
        b"str"
            | b"int"
            | b"float"
            | b"bool"
            | b"dict"
            | b"list"
            | b"set"
            | b"tuple"
            | b"bytes"
            | b"object"
            | b"type"
            | b"frozenset"
            | b"bytearray"
            | b"complex"
    )
}

/// The stop list: builtins whose bare uses are not references.
fn is_builtin(w: &[u8]) -> bool {
    matches!(
        w,
        b"print"
            | b"len"
            | b"range"
            | b"str"
            | b"int"
            | b"float"
            | b"bool"
            | b"dict"
            | b"list"
            | b"set"
            | b"tuple"
            | b"bytes"
            | b"object"
            | b"type"
            | b"isinstance"
            | b"issubclass"
            | b"getattr"
            | b"setattr"
            | b"hasattr"
            | b"delattr"
            | b"super"
            | b"open"
            | b"enumerate"
            | b"zip"
            | b"map"
            | b"filter"
            | b"sorted"
            | b"reversed"
            | b"min"
            | b"max"
            | b"sum"
            | b"any"
            | b"all"
            | b"repr"
            | b"id"
            | b"iter"
            | b"next"
            | b"callable"
            | b"vars"
            | b"hash"
            | b"abs"
            | b"round"
            | b"True"
            | b"False"
            | b"None"
            | b"self"
            | b"cls"
            | b"__name__"
            | b"__file__"
            | b"frozenset"
            | b"bytearray"
            | b"complex"
            | b"memoryview"
            | b"slice"
            | b"divmod"
            | b"pow"
            | b"chr"
            | b"ord"
            | b"hex"
            | b"oct"
            | b"bin"
            | b"format"
            | b"dir"
            | b"globals"
            | b"locals"
            | b"input"
            | b"eval"
            | b"exec"
            | b"compile"
            | b"ascii"
            | b"__import__"
            | b"__doc__"
            | b"__debug__"
            | b"__class__"
            | b"__builtins__"
            | b"__package__"
            | b"__spec__"
            | b"__loader__"
            | b"__path__"
            | b"__dict__"
            | b"property"
            | b"staticmethod"
            | b"classmethod"
            | b"breakpoint"
            | b"aiter"
            | b"anext"
            | b"NotImplemented"
            | b"Ellipsis"
            | b"Exception"
            | b"BaseException"
            | b"ValueError"
            | b"TypeError"
            | b"KeyError"
            | b"IndexError"
            | b"AttributeError"
            | b"RuntimeError"
            | b"NotImplementedError"
            | b"StopIteration"
            | b"StopAsyncIteration"
            | b"AssertionError"
            | b"ImportError"
            | b"ModuleNotFoundError"
            | b"OSError"
            | b"IOError"
            | b"NameError"
            | b"LookupError"
            | b"ArithmeticError"
            | b"ZeroDivisionError"
            | b"OverflowError"
            | b"UnicodeError"
            | b"UnicodeDecodeError"
            | b"UnicodeEncodeError"
            | b"FileNotFoundError"
            | b"FileExistsError"
            | b"PermissionError"
            | b"TimeoutError"
            | b"ConnectionError"
            | b"EOFError"
            | b"MemoryError"
            | b"RecursionError"
            | b"SystemExit"
            | b"KeyboardInterrupt"
            | b"GeneratorExit"
            | b"SyntaxError"
            | b"IndentationError"
            | b"UnboundLocalError"
            | b"UnicodeTranslateError"
            | b"FloatingPointError"
            | b"BufferError"
            | b"ReferenceError"
            | b"SystemError"
            | b"EnvironmentError"
            | b"BlockingIOError"
            | b"ChildProcessError"
            | b"BrokenPipeError"
            | b"ConnectionAbortedError"
            | b"ConnectionRefusedError"
            | b"ConnectionResetError"
            | b"InterruptedError"
            | b"IsADirectoryError"
            | b"NotADirectoryError"
            | b"ProcessLookupError"
            | b"ExceptionGroup"
            | b"BaseExceptionGroup"
            | b"SyntaxWarning"
            | b"ImportWarning"
            | b"UnicodeWarning"
            | b"BytesWarning"
            | b"EncodingWarning"
            | b"Warning"
            | b"UserWarning"
            | b"DeprecationWarning"
            | b"PendingDeprecationWarning"
            | b"RuntimeWarning"
            | b"FutureWarning"
            | b"ResourceWarning"
    )
}
