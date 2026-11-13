//! Ruby extractor (tree-sitter-ruby).
//!
//! Ruby declares nothing, so the walker reconstructs what a reader infers from the code:
//!   * a bare identifier is a local variable once an assignment, a parameter or a pattern has
//!     bound it earlier in the same scope, and a call of a method of `self` with no arguments
//!     otherwise. The grammar does not tell the two apart, so locals are tracked in source
//!     order the way Ruby's parser does it: `def`, `class` and `module` open a fresh scope,
//!     blocks and lambdas see the scope around them;
//!   * a call without a receiver inside a class or module is a call on `self`: a `MemberCall`
//!     qualified by the enclosing namespace path, exactly as `self.m` is. Outside any class it
//!     is a plain `Call`. A call on a constant (`Foo.m`, `A::B.m`) is a `MemberCall` on the
//!     value `$Foo` / `$A::B`: Ruby has no static methods, and the `$` root resolves classes,
//!     modules and constant objects alike;
//!   * `def self.m` and the methods of `class << self` belong to the class like instance
//!     methods do. Reopened classes are separate definitions with the same name;
//!   * class macros that define methods by name are expanded when that is exact:
//!     `attr_*` (properties), `define_method(:m)`, `delegate :a, to: :b`, `scope :m`,
//!     `alias` / `alias_method` (a field bound to the original method, the form the resolver
//!     follows for aliases), `Struct.new` / `Data.define` assigned to a constant (a struct with
//!     fields), and Rails `test "name" do`;
//!   * a symbol handed to a callback macro (`before_action :authenticate`, `validate :check`,
//!     `if: :ready?`) is an `Arg` of the macro plus a `MemberRead` of the method on `self`,
//!     which is how the resolver ties a callback to the method it names;
//!   * receivers are typed from `T.new`, `self`, constants, literals and instance variables
//!     whose first assignment is a `T.new`. An inferred type holds while every assignment
//!     agrees on it.
//!
//! Qualifiers (`Ref::qual`):
//!   * `$A::B`: the class, module or constant written `A::B` (a call on it, or on `self` inside
//!     it); `$A::B::@x` for a call on its instance variable `@x`; `^$A::B` for `super`;
//!   * `A`: the namespace a constant is written in, on `Type`, `New`, `Extends` and
//!     `Implements` (`A::B.new` is `New B` qualified by `A`);
//!   * `$A::B.slot` on the `Bind` of an alias, the macro's name on an `Arg`.
//!
//! `include`, `prepend` and `extend` of a module are `Implements`: the resolver walks
//! `Extends` and `Implements` alike when it looks for inherited members, and the superclass
//! stays the one `Extends` of a class.
//!
//! Arity counts positional parameters, plus one for keyword parameters taken together (a call
//! passes its keyword arguments as one trailing hash). A splat makes it variadic. `nargs`
//! counts positional arguments plus one when keyword arguments are present. Blocks do not
//! count on either side.

use crate::extract::*;
use crate::facts::*;
use crate::lang::Lang;
use rustc_hash::FxHashMap;
use std::sync::OnceLock;
use tree_sitter::{Node, Tree, TreeCursor};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    Other,
    Skip,
    Error,
    Identifier,
    Constant,
    ScopeResolution,
    InstanceVariable,
    ClassVariable,
    GlobalVariable,
    SelfK,
    Super,
    Call,
    ArgumentList,
    BlockArgument,
    SplatArgument,
    HashSplatArgument,
    ForwardArgument,
    Pair,
    HashKeySymbol,
    Method,
    SingletonMethod,
    Class,
    Module,
    SingletonClass,
    Superclass,
    MethodParameters,
    BlockParameters,
    LambdaParameters,
    OptionalParameter,
    KeywordParameter,
    SplatParameter,
    HashSplatParameter,
    BlockParameter,
    DestructuredParameter,
    ForwardParameter,
    Block,
    DoBlock,
    Lambda,
    Assignment,
    OpAssignment,
    LeftAssignmentList,
    DestructuredLeftAssignment,
    RestAssignment,
    Alias,
    Undef,
    For,
    Rescue,
    ExceptionVariable,
    InClause,
    MatchPattern,
    TestPattern,
    AsPattern,
    KeywordPattern,
    VariableReferencePattern,
    ExpressionReferencePattern,
    Str,
    Regex,
    SimpleSymbol,
    DelimitedSymbol,
    WordArray,
    Interpolation,
    StringContent,
    Integer,
    Float,
    Array,
    Hash,
    Nil,
    ParenthesizedStatements,
    Unary,
    BodyStatement,
    ElementReference,
    Setter,
    Operator,
}

struct Tables {
    kinds: KindTable<K>,
    f_name: u16,
    f_body: u16,
    f_parameters: u16,
    f_superclass: u16,
    f_receiver: u16,
    f_method: u16,
    f_arguments: u16,
    f_block: u16,
    f_left: u16,
    f_right: u16,
    f_scope: u16,
    f_value: u16,
    f_object: u16,
    f_alias: u16,
    f_pattern: u16,
    f_key: u16,
    f_operator: u16,
    f_locals: u16,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let lang: tree_sitter::Language = tree_sitter_ruby::LANGUAGE.into();
        let pairs: &[(&str, K)] = &[
            ("ERROR", K::Error),
            ("identifier", K::Identifier),
            ("constant", K::Constant),
            ("scope_resolution", K::ScopeResolution),
            ("instance_variable", K::InstanceVariable),
            ("class_variable", K::ClassVariable),
            ("global_variable", K::GlobalVariable),
            ("self", K::SelfK),
            ("super", K::Super),
            ("call", K::Call),
            ("argument_list", K::ArgumentList),
            ("block_argument", K::BlockArgument),
            ("splat_argument", K::SplatArgument),
            ("hash_splat_argument", K::HashSplatArgument),
            ("forward_argument", K::ForwardArgument),
            ("pair", K::Pair),
            ("hash_key_symbol", K::HashKeySymbol),
            ("method", K::Method),
            ("singleton_method", K::SingletonMethod),
            ("class", K::Class),
            ("module", K::Module),
            ("singleton_class", K::SingletonClass),
            ("superclass", K::Superclass),
            ("method_parameters", K::MethodParameters),
            ("block_parameters", K::BlockParameters),
            ("lambda_parameters", K::LambdaParameters),
            ("optional_parameter", K::OptionalParameter),
            ("keyword_parameter", K::KeywordParameter),
            ("splat_parameter", K::SplatParameter),
            ("hash_splat_parameter", K::HashSplatParameter),
            ("block_parameter", K::BlockParameter),
            ("destructured_parameter", K::DestructuredParameter),
            ("forward_parameter", K::ForwardParameter),
            ("hash_splat_nil", K::Skip),
            ("block", K::Block),
            ("do_block", K::DoBlock),
            ("lambda", K::Lambda),
            ("assignment", K::Assignment),
            ("operator_assignment", K::OpAssignment),
            ("left_assignment_list", K::LeftAssignmentList),
            ("destructured_left_assignment", K::DestructuredLeftAssignment),
            ("rest_assignment", K::RestAssignment),
            ("alias", K::Alias),
            ("undef", K::Undef),
            ("for", K::For),
            ("rescue", K::Rescue),
            ("exception_variable", K::ExceptionVariable),
            ("in_clause", K::InClause),
            ("match_pattern", K::MatchPattern),
            ("test_pattern", K::TestPattern),
            ("as_pattern", K::AsPattern),
            ("keyword_pattern", K::KeywordPattern),
            ("variable_reference_pattern", K::VariableReferencePattern),
            ("expression_reference_pattern", K::ExpressionReferencePattern),
            ("string", K::Str),
            ("chained_string", K::Str),
            ("heredoc_body", K::Str),
            ("subshell", K::Str),
            ("bare_string", K::Str),
            ("bare_symbol", K::Str),
            ("regex", K::Regex),
            ("simple_symbol", K::SimpleSymbol),
            ("delimited_symbol", K::DelimitedSymbol),
            ("string_array", K::WordArray),
            ("symbol_array", K::WordArray),
            ("interpolation", K::Interpolation),
            ("string_content", K::StringContent),
            ("integer", K::Integer),
            ("float", K::Float),
            ("array", K::Array),
            ("hash", K::Hash),
            ("nil", K::Nil),
            ("parenthesized_statements", K::ParenthesizedStatements),
            ("unary", K::Unary),
            ("body_statement", K::BodyStatement),
            ("element_reference", K::ElementReference),
            ("setter", K::Setter),
            ("operator", K::Operator),
            // leaves that never hold a reference
            ("comment", K::Skip),
            ("heredoc_beginning", K::Skip),
            ("heredoc_content", K::Skip),
            ("heredoc_end", K::Skip),
            ("escape_sequence", K::Skip),
            ("character", K::Skip),
            ("complex", K::Skip),
            ("rational", K::Skip),
            ("true", K::Skip),
            ("false", K::Skip),
            ("encoding", K::Skip),
            ("file", K::Skip),
            ("line", K::Skip),
            ("uninterpreted", K::Skip),
            ("empty_statement", K::Skip),
            ("redo", K::Skip),
            ("retry", K::Skip),
            ("hash_splat_nil", K::Skip),
        ];
        let kinds = KindTable::build(&lang, K::Other, pairs);
        Tables {
            kinds,
            f_name: field(&lang, "name"),
            f_body: field(&lang, "body"),
            f_parameters: field(&lang, "parameters"),
            f_superclass: field(&lang, "superclass"),
            f_receiver: field(&lang, "receiver"),
            f_method: field(&lang, "method"),
            f_arguments: field(&lang, "arguments"),
            f_block: field(&lang, "block"),
            f_left: field(&lang, "left"),
            f_right: field(&lang, "right"),
            f_scope: field(&lang, "scope"),
            f_value: field(&lang, "value"),
            f_object: field(&lang, "object"),
            f_alias: field(&lang, "alias"),
            f_pattern: field(&lang, "pattern"),
            f_key: field(&lang, "key"),
            f_operator: field(&lang, "operator"),
            f_locals: field(&lang, "locals"),
        }
    })
}

// ----- per-name knowledge, indexed by file-local name id -----

const N_SEEN: u8 = 1;
/// Kernel and Module methods whose bare calls are not references (`puts`, `raise`, `attr_reader`).
const N_STOP: u8 = 2;

/// A local whose only assignments so far were `nil`: says nothing about its type yet.
const NEUTRAL: u32 = NONE - 1;

/// Longest receiver type path worth recording.
const MAX_PATH: usize = 160;

#[derive(Clone, Copy, PartialEq, Eq)]
enum At {
    /// File level (and blocks there: RSpec `describe` bodies, rake tasks).
    Top,
    /// A class or module body, `class << self`, and the blocks written in them.
    Class,
    /// A method body.
    Method,
}

/// What encloses the code being walked.
#[derive(Clone, Copy)]
struct Frame {
    at: At,
    /// The class or module that owns new definitions and instance variables (`NONE` at top level).
    owner: u32,
    /// Receiver path of `self` here: `$A::B` for the class or module `owner`, `NONE` outside.
    selfq: u32,
    /// Name of the enclosing method, for `super` (`NONE` outside methods).
    method: u32,
    /// After a bare `private` in this class body.
    private: bool,
    /// Inside a block without parameters: `it` and `_1` .. `_9` are its parameters.
    implicit: bool,
    /// Inside `def self.included(base)` and its siblings: the name of `base`, whose `extend X`
    /// mixes `X` into the class that includes this module.
    hook: u32,
}

/// How a receiver expression is known.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Rk {
    Unknown,
    /// A constant path, the class or module object itself: `Foo`, `A::B`.
    Const,
    /// A value with a known type path.
    Path,
}

struct W<'a, 't> {
    cx: Ctx<'a>,
    t: &'static Tables,
    cur: Frame,
    frames: Vec<Frame>,
    ninfo: Vec<u8>,
    /// Bound locals, innermost last: (name, type path or `NONE` / `NEUTRAL`).
    locals: Vec<(u32, u32)>,
    /// Locals below this index belong to an enclosing `def`, `class` or `module`: invisible.
    floor: usize,
    /// Live bindings per name id: most identifiers are not locals, and this answers that at once.
    nlocal: Vec<u16>,
    /// (owner definition or `NONE`, name) -> definition: first assignments of instance variables
    /// and constants, methods (for `private :m`) and attributes.
    members: FxHashMap<u64, u32>,
    test_file: bool,
    /// Inside the operand of `defined?`: a bare name there is tested, not called.
    in_defined: u32,
    _t: std::marker::PhantomData<&'t ()>,
}

pub fn extract(_lang: Lang, src: &[u8], tree: &Tree, path: &str, facts: &mut FileFacts, scratch: &mut Scratch) {
    let cx = Ctx::new(src, path, facts, scratch);
    let mut w = W {
        cx,
        t: tables(),
        cur: Frame { at: At::Top, owner: NONE, selfq: NONE, method: NONE, private: false, implicit: false, hook: NONE },
        frames: Vec::new(),
        ninfo: Vec::new(),
        locals: Vec::new(),
        floor: 0,
        nlocal: Vec::new(),
        members: FxHashMap::default(),
        test_file: is_test_path(path),
        in_defined: 0,
        _t: std::marker::PhantomData,
    };
    let mut c = tree.walk();
    w.visit(&mut c);
    w.finish();
}

#[inline]
fn mkey(owner: u32, name: u32) -> u64 {
    ((owner as u64) << 32) | name as u64
}

/// Test code by Ruby's conventions: the `spec/` and `test/` trees (RSpec, Minitest) and
/// `*_spec.rb` / `*_test.rb` files.
fn is_test_path(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    let mut dirs = path.split('/');
    dirs.next_back();
    dirs.any(|c| c == "spec" || c == "test") || file.ends_with("_spec.rb") || file.ends_with("_test.rb")
}

impl<'a, 't> W<'a, 't> {
    #[inline]
    fn k(&self, n: Node<'t>) -> K {
        self.t.kinds.get(n.kind_id())
    }

    #[inline]
    fn text(&self, n: Node<'t>) -> &'a [u8] {
        self.cx.text(n)
    }

    // ----- names -----

    #[inline]
    fn info(&mut self, id: u32) -> u8 {
        match self.ninfo.get(id as usize) {
            Some(&f) if f != 0 => f,
            _ => self.classify(id),
        }
    }

    #[cold]
    fn classify(&mut self, id: u32) -> u8 {
        let mut f = N_SEEN;
        if is_builtin(self.cx.name_bytes(id)) {
            f |= N_STOP;
        }
        let i = id as usize;
        if i >= self.ninfo.len() {
            self.ninfo.resize(i + 1, 0);
        }
        self.ninfo[i] = f;
        f
    }

    // ----- locals -----

    fn local(&self, name: u32) -> Option<u32> {
        if self.nlocal.get(name as usize).copied().unwrap_or(0) == 0 {
            return None;
        }
        for &(n, t) in self.locals[self.floor..].iter().rev() {
            if n == name {
                return Some(if t == NEUTRAL { NONE } else { t });
            }
        }
        None
    }

    #[inline]
    fn is_local(&self, name: u32) -> bool {
        self.local(name).is_some()
    }

    /// Bind `name` in the innermost scope that already has it, else in the current one. A type
    /// holds while every assignment agrees on it (`nil` says nothing).
    fn declare(&mut self, name: u32, ty: u32) {
        let i = name as usize;
        if self.nlocal.get(i).copied().unwrap_or(0) > 0 {
            for j in (self.floor..self.locals.len()).rev() {
                if self.locals[j].0 == name {
                    let old = self.locals[j].1;
                    self.locals[j].1 = if old == NEUTRAL || old == ty {
                        ty
                    } else if ty == NEUTRAL {
                        old
                    } else {
                        NONE
                    };
                    return;
                }
            }
        }
        if i >= self.nlocal.len() {
            self.nlocal.resize(i + 1, 0);
        }
        self.nlocal[i] += 1;
        self.locals.push((name, ty));
    }

    fn close_scope(&mut self, mark: usize) {
        for j in mark..self.locals.len() {
            let n = self.locals[j].0 as usize;
            self.nlocal[n] -= 1;
        }
        self.locals.truncate(mark);
    }

    fn declare_node(&mut self, n: Node<'t>, ty: u32) {
        if n.start_byte() < n.end_byte() {
            let name = self.cx.name(n);
            self.declare(name, ty);
        }
    }

    /// `it` and `_1` .. `_9` inside a block that declares no parameters.
    fn implicit_param(&self, text: &[u8]) -> bool {
        self.cur.implicit && (text == b"it" || (text.len() == 2 && text[0] == b'_' && (b'1'..=b'9').contains(&text[1])))
    }

    // ----- frames -----

    fn push_frame(&mut self, f: Frame) {
        self.frames.push(self.cur);
        self.cur = f;
    }

    fn pop_frame(&mut self) {
        if let Some(f) = self.frames.pop() {
            self.cur = f;
        }
    }

    // ----- traversal -----

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

    /// Visit the child `target` of the cursor's node.
    fn visit_child(&mut self, c: &mut TreeCursor<'t>, target: Node<'t>) {
        if c.goto_first_child() {
            loop {
                if c.node() == target {
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

    fn visit(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        let k = self.k(n);
        match k {
            K::Identifier => return self.ident(n),
            K::Constant => return self.constant(n, RefKind::Type),
            K::InstanceVariable | K::ClassVariable => return self.ivar_read(n),
            K::GlobalVariable => {
                let name = self.cx.name(n);
                return self.cx.add_ref(name, RefKind::Read, line_of(n), NONE, 255);
            }
            K::Super => return self.zsuper(n),
            K::Skip
            | K::SelfK
            | K::HashKeySymbol
            | K::SimpleSymbol
            | K::StringContent
            | K::Integer
            | K::Float
            | K::Nil
            | K::Undef
            | K::ForwardArgument
            | K::Setter
            | K::Operator => return,
            _ => {}
        }
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        match k {
            K::Call => self.call(n, c),
            K::Method => self.method(n, c, false),
            K::SingletonMethod => self.method(n, c, true),
            K::Class => self.class(n, c),
            K::Module => self.module(n, c),
            K::SingletonClass => self.singleton_class(n, c),
            K::Assignment => self.assignment(n, c),
            K::OpAssignment => self.op_assignment(n, c),
            K::ScopeResolution => self.scope_resolution(n, c, RefKind::Type),
            K::Block | K::DoBlock => self.block(n, c),
            K::Lambda => self.lambda(n, c),
            K::Alias => self.alias(n),
            K::For => self.for_loop(n, c),
            K::Rescue => self.rescue(c),
            K::InClause => self.in_clause(n, c),
            K::MatchPattern | K::TestPattern => self.match_pattern(n, c),
            K::Pair => self.pair(n, c),
            K::Str | K::Regex | K::DelimitedSymbol | K::WordArray => self.interpolations(c),
            K::Unary => self.unary(n, c),
            K::BlockArgument => self.block_argument(n, c),
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
                if !matches!(self.k(c.node()), K::Identifier | K::Constant) {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
    }

    /// Only interpolations hold code in a string, symbol, regex or heredoc.
    fn interpolations(&mut self, c: &mut TreeCursor<'t>) {
        if c.goto_first_child() {
            loop {
                if matches!(self.k(c.node()), K::Interpolation | K::Str) {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
    }

    /// `&:name` calls `name` on every element, whose type is not known here.
    fn block_argument(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        match n.named_child(0) {
            Some(s) if self.k(s) == K::SimpleSymbol => {
                if let Some(text) = self.sym_text(s).filter(|t| is_method_name(t)) {
                    let name = self.cx.intern(text);
                    self.cx.add_ref(name, RefKind::MemberCall, line_of(s), NONE, 0);
                }
            }
            _ => self.children(c),
        }
    }

    /// `defined?(x)` asks whether `x` exists: a bare name there is not a call.
    fn unary(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let defined = child(n, self.t.f_operator).is_some_and(|o| self.text(o) == b"defined?");
        if defined {
            self.in_defined += 1;
        }
        self.children(c);
        if defined {
            self.in_defined -= 1;
        }
    }

    // ----- names and constants -----

    /// A bare identifier: a local, or a call of a method of `self` with no arguments.
    fn ident(&mut self, n: Node<'t>) {
        if n.start_byte() == n.end_byte() {
            self.cx.facts.parse_errors += 1;
            return;
        }
        let name = self.cx.name(n);
        if self.is_local(name) || self.in_defined > 0 {
            return;
        }
        let text = self.text(n);
        if self.implicit_param(text) {
            return;
        }
        if self.cur.at == At::Class && matches!(text, b"private" | b"public" | b"protected" | b"module_function") {
            // a visibility switch for the definitions that follow
            self.cur.private = text == b"private";
            return;
        }
        if self.info(name) & N_STOP != 0 {
            return;
        }
        self.self_call(name, line_of(n), 0, RefKind::MemberCall);
    }

    /// A call (or `kind`) of method `name` on `self`: a member reference on the enclosing class
    /// or module, a plain call outside any.
    fn self_call(&mut self, name: u32, line: u32, nargs: u8, kind: RefKind) {
        let q = self.cur.selfq;
        if q != NONE {
            // A known attribute of the class is read, not called.
            let kind = if kind == RefKind::MemberCall && nargs == 0 && self.is_attr(self.cur.owner, name) { RefKind::MemberRead } else { kind };
            self.cx.add_ref(name, kind, line, q, nargs);
        } else {
            let kind = match kind {
                RefKind::MemberCall => RefKind::Call,
                RefKind::MemberRead => RefKind::Read,
                k => k,
            };
            self.cx.add_ref(name, kind, line, NONE, nargs);
        }
    }

    fn is_attr(&self, owner: u32, name: u32) -> bool {
        owner != NONE && self.members.get(&mkey(owner, name)).is_some_and(|&d| self.cx.facts.defs[d as usize].kind == DefKind::Property)
    }

    /// A constant used as a value or a type. ALL_CAPS names are values, the rest name classes
    /// and modules.
    fn constant(&mut self, n: Node<'t>, kind: RefKind) {
        if n.start_byte() == n.end_byte() {
            return;
        }
        let text = self.text(n);
        if is_builtin_const(text) {
            return;
        }
        let name = self.cx.name(n);
        let kind = if kind == RefKind::Type && is_constant_name(text) { RefKind::Read } else { kind };
        self.cx.add_ref(name, kind, line_of(n), NONE, 255);
    }

    /// `A::B::C` used as `kind`: every namespace segment is a type use, the last one is `kind`
    /// qualified by the namespace it is written in. A scope that is not a constant path
    /// (`obj.class::X`) is walked as an expression.
    fn scope_resolution(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: RefKind) {
        let t = self.t;
        let Some(nm) = child(n, t.f_name) else { return self.children(c) };
        let scope = child(n, t.f_scope);
        let mut qual = NONE;
        if let Some(s) = scope {
            match self.k(s) {
                K::Constant | K::ScopeResolution if self.const_path_ok(s, 0) => {
                    self.const_prefix_refs(s);
                    qual = self.path_id(s, false);
                }
                _ => self.visit_child(c, s),
            }
        }
        if self.k(nm) != K::Constant || nm.start_byte() == nm.end_byte() {
            // `Foo::bar` is a method call written with `::`
            return;
        }
        let text = self.text(nm);
        if qual == NONE && is_builtin_const(text) {
            return;
        }
        let name = self.cx.name(nm);
        let kind = if kind == RefKind::Type && is_constant_name(text) { RefKind::Read } else { kind };
        self.cx.add_ref(name, kind, line_of(nm), qual, 255);
    }

    /// Type uses for the segments of a constant path `s`, each qualified by the namespace
    /// before it (`A::B` gives `A`, then `B` in `A`).
    fn const_prefix_refs(&mut self, s: Node<'t>) {
        let t = self.t;
        match self.k(s) {
            K::Constant => self.constant(s, RefKind::Type),
            K::ScopeResolution => {
                let Some(nm) = child(s, t.f_name) else { return };
                let mut qual = NONE;
                if let Some(sc) = child(s, t.f_scope) {
                    self.const_prefix_refs(sc);
                    qual = self.path_id(sc, false);
                }
                if nm.start_byte() < nm.end_byte() {
                    let name = self.cx.name(nm);
                    self.cx.add_ref(name, RefKind::Type, line_of(nm), qual, 255);
                }
            }
            _ => {}
        }
    }

    /// Is `n` a path of constants (`A`, `A::B`, `::A`)?
    fn const_path_ok(&self, n: Node<'t>, depth: u32) -> bool {
        match self.k(n) {
            K::Constant => n.start_byte() < n.end_byte(),
            K::ScopeResolution if depth < 12 => {
                child(n, self.t.f_name).is_some_and(|x| self.k(x) == K::Constant) && child(n, self.t.f_scope).is_none_or(|s| self.const_path_ok(s, depth + 1))
            }
            _ => false,
        }
    }

    /// Append the constant path `n` as written, without a leading `::`.
    fn push_path(&self, n: Node<'t>, buf: &mut Vec<u8>) {
        match self.k(n) {
            K::Constant => buf.extend_from_slice(self.text(n)),
            K::ScopeResolution => {
                if let Some(s) = child(n, self.t.f_scope) {
                    self.push_path(s, buf);
                    buf.extend_from_slice(b"::");
                }
                if let Some(nm) = child(n, self.t.f_name) {
                    buf.extend_from_slice(self.text(nm));
                }
            }
            _ => {}
        }
    }

    /// The constant path `n` interned, with a leading `$` when `value` (a receiver).
    fn path_id(&mut self, n: Node<'t>, value: bool) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        if value {
            buf.push(b'$');
        }
        self.push_path(n, &mut buf);
        let id = if buf.len() > usize::from(value) && buf.len() <= MAX_PATH { self.cx.intern(&buf) } else { NONE };
        self.cx.s.buf = buf;
        id
    }

    /// The last constant of a path and the namespace before it: `A::B::C` -> (`C`, `A::B`).
    fn split_const(&mut self, n: Node<'t>) -> Option<(Node<'t>, u32)> {
        match self.k(n) {
            K::Constant => Some((n, NONE)),
            K::ScopeResolution => {
                let nm = child(n, self.t.f_name).filter(|x| self.k(*x) == K::Constant && x.start_byte() < x.end_byte())?;
                let q = match child(n, self.t.f_scope) {
                    Some(s) => self.path_id(s, false),
                    None => NONE,
                };
                Some((nm, q))
            }
            _ => None,
        }
    }

    /// A constant path used in a definition or a type reference: refs for its namespace part,
    /// then (last name, namespace) of the whole.
    fn named_const(&mut self, n: Node<'t>) -> Option<(Node<'t>, u32)> {
        if !self.const_path_ok(n, 0) {
            return None;
        }
        if self.k(n) == K::ScopeResolution {
            if let Some(s) = child(n, self.t.f_scope) {
                self.const_prefix_refs(s);
            }
        }
        self.split_const(n)
    }

    /// A type reference (`Extends`, `Implements`, `New`) to the constant path `n`.
    fn type_ref(&mut self, n: Node<'t>, kind: RefKind, line: u32, nargs: u8) -> bool {
        let Some((nm, q)) = self.named_const(n) else { return false };
        if q == NONE && is_builtin_const(self.text(nm)) {
            return true;
        }
        let name = self.cx.name(nm);
        self.cx.add_ref(name, kind, line, q, nargs);
        true
    }

    // ----- instance variables -----

    fn ivar_read(&mut self, n: Node<'t>) {
        if self.cur.selfq == NONE || n.start_byte() == n.end_byte() {
            return;
        }
        let name = self.cx.name(n);
        self.cx.add_ref(name, RefKind::MemberRead, line_of(n), self.cur.selfq, 255);
    }

    /// `@x = v`: the first assignment in a class or module defines its field `@x`, later ones
    /// write it.
    fn ivar_write(&mut self, id: Node<'t>, stmt: Node<'t>, sig_end: usize, ty: u32) {
        let owner = self.cur.owner;
        if owner == NONE || id.start_byte() == id.end_byte() {
            return;
        }
        let name = self.cx.name(id);
        let key = mkey(owner, name);
        match self.members.get(&key).copied() {
            Some(d) => {
                let dm = self.cx.def_mut(d);
                if dm.kind == DefKind::Field && dm.type_name == NONE && ty != NONE {
                    dm.type_name = ty;
                }
                self.cx.add_ref(name, RefKind::MemberWrite, line_of(id), self.cur.selfq, 255);
            }
            None => {
                let d = self.new_def(name, DefKind::Field, stmt, sig_end, owner, NONE);
                self.cx.def_mut(d).type_name = ty;
                self.members.insert(key, d);
            }
        }
    }

    // ----- definitions -----

    fn new_def(&mut self, name: u32, kind: DefKind, node: Node<'t>, sig_end: usize, parent: u32, qual: u32) -> u32 {
        let d = self.cx.def(name, kind, node, sig_end);
        let test = self.test_file;
        let dm = self.cx.def_mut(d);
        dm.parent = parent;
        if parent == NONE {
            dm.qual = qual;
        }
        if test {
            dm.flags |= dflag::TEST;
        }
        d
    }

    /// The `$`-rooted path of a namespace defined as `written` inside the current one.
    fn nested_selfq(&mut self, written: Node<'t>) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let absolute = self.k(written) == K::ScopeResolution && child(written, self.t.f_scope).is_none();
        if self.cur.selfq != NONE && !absolute {
            buf.extend_from_slice(self.cx.name_bytes(self.cur.selfq));
            buf.extend_from_slice(b"::");
        } else {
            buf.push(b'$');
        }
        self.push_path(written, &mut buf);
        let id = if buf.len() <= MAX_PATH { self.cx.intern(&buf) } else { NONE };
        self.cx.s.buf = buf;
        id
    }

    /// The name node and container of a class or module name: `B` and `A` for `class A::B`
    /// (a container known by name only), the enclosing definition otherwise.
    fn def_name(&mut self, written: Node<'t>) -> Option<(Node<'t>, u32, u32)> {
        let t = self.t;
        match self.k(written) {
            K::Constant if written.start_byte() < written.end_byte() => Some((written, self.cur.owner, NONE)),
            K::ScopeResolution => {
                let nm = child(written, t.f_name).filter(|x| x.start_byte() < x.end_byte())?;
                match child(written, t.f_scope) {
                    // `class ::Foo`: a top-level class whatever encloses it
                    None => Some((nm, NONE, NONE)),
                    Some(s) => {
                        self.const_prefix_refs(s);
                        let container = match self.k(s) {
                            K::Constant => self.cx.name(s),
                            K::ScopeResolution => match child(s, t.f_name) {
                                Some(x) => self.cx.name(x),
                                None => NONE,
                            },
                            _ => NONE,
                        };
                        Some((nm, NONE, container))
                    }
                }
            }
            _ => None,
        }
    }

    fn class(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(written) = child(n, t.f_name) else { return self.children(c) };
        let sup = child(n, t.f_superclass);
        let body = child(n, t.f_body);
        let Some((nm, parent, container)) = self.def_name(written) else { return self.children(c) };
        let name = self.cx.name(nm);
        let sig_end = sup.unwrap_or(written).end_byte();
        let d = self.new_def(name, DefKind::Class, n, sig_end, parent, container);
        self.cx.def_mut(d).arity_max = 255;
        if self.name_is_test_class(nm) {
            self.cx.def_mut(d).flags |= dflag::TEST;
        }
        let selfq = self.nested_selfq(written);
        self.cx.push_def(d);
        if let Some(s) = sup {
            self.superclass(s, d, c);
        }
        self.body(d, selfq, body, c);
        self.cx.pop_def();
    }

    fn name_is_test_class(&self, nm: Node<'t>) -> bool {
        let s = self.text(nm);
        s.ends_with(b"Test") || s.ends_with(b"Spec")
    }

    /// `< Base`, `< A::Base`, `< Struct.new(:a, :b)`, `< ActiveRecord::Migration[7.1]`.
    fn superclass(&mut self, s: Node<'t>, d: u32, c: &mut TreeCursor<'t>) {
        let Some(e) = s.named_child(0) else { return };
        let line = line_of(e);
        let base = if self.k(e) == K::ElementReference { child(e, self.t.f_object).unwrap_or(e) } else { e };
        if self.type_ref(base, RefKind::Extends, line, 255) {
            return;
        }
        if self.struct_call(e).is_some() {
            // the anonymous struct class contributes its members
            self.struct_fields(e, d);
            return;
        }
        if c.goto_first_child() {
            loop {
                if c.node() == s {
                    self.children(c);
                    break;
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
    }

    fn module(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(written) = child(n, t.f_name) else { return self.children(c) };
        let body = child(n, t.f_body);
        let Some((nm, parent, container)) = self.def_name(written) else { return self.children(c) };
        let name = self.cx.name(nm);
        let d = self.new_def(name, DefKind::Module, n, written.end_byte(), parent, container);
        let selfq = self.nested_selfq(written);
        let outer = self.cur.owner;
        if self.text(nm) == b"ClassMethods" && self.cur.at == At::Class && outer != NONE && self.cx.facts.defs[outer as usize].kind == DefKind::Module {
            // `ActiveSupport::Concern` extends every class that includes the module with its
            // `ClassMethods`: the module mixes them in as far as lookup goes.
            let ns = self.cur.selfq;
            let ns = if ns == NONE { NONE } else { self.cx.intern(&self.cx.name_bytes(ns)[1..].to_vec()) };
            self.cx.facts.refs.push(Ref { name, qual: ns, line: line_of(nm), encl: outer, kind: RefKind::Implements, nargs: 255 });
        }
        self.cx.push_def(d);
        self.body(d, selfq, body, c);
        self.cx.pop_def();
    }

    /// Walk the body of class or module `d` with a scope of its own.
    fn body(&mut self, d: u32, selfq: u32, body: Option<Node<'t>>, c: &mut TreeCursor<'t>) {
        let Some(b) = body else { return };
        self.push_frame(Frame { at: At::Class, owner: d, selfq, method: NONE, private: false, implicit: false, hook: NONE });
        let (floor, mark) = (self.floor, self.locals.len());
        self.floor = mark;
        self.visit_child(c, b);
        self.close_scope(mark);
        self.floor = floor;
        self.pop_frame();
    }

    /// `class << self`: its methods are methods of the enclosing class.
    fn singleton_class(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let value = child(n, t.f_value);
        let body = child(n, t.f_body);
        if value.is_some_and(|v| self.k(v) != K::SelfK) {
            if let Some(v) = value {
                self.visit_child(c, v);
            }
        }
        let Some(b) = body else { return };
        let mut f = self.cur;
        f.at = At::Class;
        f.private = false;
        f.implicit = false;
        f.method = NONE;
        self.push_frame(f);
        let (floor, mark) = (self.floor, self.locals.len());
        self.floor = mark;
        self.visit_child(c, b);
        self.close_scope(mark);
        self.floor = floor;
        self.pop_frame();
    }

    fn method(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, singleton: bool) {
        let t = self.t;
        let Some(nm) = child(n, t.f_name).filter(|x| x.start_byte() < x.end_byte()) else { return self.children(c) };
        let params = child(n, t.f_parameters);
        let body = child(n, t.f_body);
        let name_text = self.text(nm);
        let name = self.cx.name(nm);
        let mut parent = self.cur.owner;
        let mut qual = NONE;
        let mut on_self = true;
        if singleton {
            if let Some(obj) = child(n, t.f_object) {
                match self.k(obj) {
                    K::SelfK => {}
                    // `def Foo.bar` outside Foo: a method of a class known by name
                    K::Constant | K::ScopeResolution => {
                        if let Some((cn, _)) = self.split_const(obj) {
                            let cname = self.cx.name(cn);
                            let own = parent != NONE && self.cx.facts.defs[parent as usize].name == cname;
                            if !own {
                                parent = NONE;
                                qual = cname;
                                on_self = false;
                            }
                        }
                    }
                    _ => {
                        self.visit_child(c, obj);
                        on_self = false;
                    }
                }
            }
        }
        let kind = if parent != NONE || qual != NONE { DefKind::Method } else { DefKind::Function };
        let sig_end = params.unwrap_or(nm).end_byte();
        let d = self.new_def(name, kind, n, sig_end, parent, qual);
        let mut flags = 0u8;
        if self.cur.at == At::Class && self.cur.private && !singleton {
            flags |= dflag::LOCAL;
        }
        if name_text.starts_with(b"test_") {
            flags |= dflag::TEST;
        }
        if body.is_some_and(|b| self.raises_not_implemented(b)) {
            flags |= dflag::ABSTRACT;
        }
        self.cx.def_mut(d).flags |= flags;
        if parent != NONE {
            self.members.insert(mkey(parent, name), d);
        }
        let hook = singleton && matches!(name_text, b"included" | b"extended" | b"prepended" | b"inherited");

        self.cx.push_def(d);
        let selfq = if on_self { self.cur.selfq } else { NONE };
        let owner = if on_self { self.cur.owner } else { NONE };
        self.push_frame(Frame { at: At::Method, owner, selfq, method: name, private: false, implicit: false, hook: NONE });
        let (floor, mark) = (self.floor, self.locals.len());
        self.floor = mark;
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if Some(ch) == params {
                    let (amin, amax, first) = self.parameters(c);
                    let dm = self.cx.def_mut(d);
                    dm.arity_min = amin;
                    dm.arity_max = amax;
                    if hook {
                        self.cur.hook = first;
                    }
                    // `T.new(..)` runs `initialize`: the class takes its parameters.
                    if name_text == b"initialize" && parent != NONE {
                        let pd = self.cx.def_mut(parent);
                        if pd.kind == DefKind::Class || pd.kind == DefKind::Struct {
                            pd.arity_min = amin;
                            pd.arity_max = amax;
                        }
                    }
                } else if Some(ch) == body {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.close_scope(mark);
        self.floor = floor;
        self.pop_frame();
        self.cx.pop_def();
    }

    /// A body that only raises `NotImplementedError`: an abstract method.
    fn raises_not_implemented(&self, body: Node<'t>) -> bool {
        let st = if self.k(body) == K::BodyStatement {
            if body.named_child_count() != 1 {
                return false;
            }
            match body.named_child(0) {
                Some(s) => s,
                None => return false,
            }
        } else {
            body
        };
        if self.k(st) != K::Call || child(st, self.t.f_receiver).is_some() {
            return false;
        }
        let Some(m) = child(st, self.t.f_method) else { return false };
        if !matches!(self.text(m), b"raise" | b"fail") {
            return false;
        }
        let Some(a) = child(st, self.t.f_arguments).and_then(|a| a.named_child(0)) else { return false };
        let e = if self.k(a) == K::Call { child(a, self.t.f_receiver).unwrap_or(a) } else { a };
        self.text(e).ends_with(b"NotImplementedError")
    }

    /// Cursor on `method_parameters`, `block_parameters` or `lambda_parameters`. Names are
    /// declared in order, defaults are walked once the parameters before them are bound.
    /// Returns (arity_min, arity_max, name of the first parameter).
    fn parameters(&mut self, c: &mut TreeCursor<'t>) -> (u8, u8, u32) {
        let t = self.t;
        let (mut req, mut opt) = (0u32, 0u32);
        let (mut kw_req, mut kw_any, mut variadic) = (false, false, false);
        let mut first = NONE;
        if c.goto_first_child() {
            loop {
                let p = c.node();
                match self.k(p) {
                    K::Identifier => {
                        if first == NONE {
                            first = self.cx.name(p);
                        }
                        self.declare_node(p, NONE);
                        // block-local variables (`|x; y|`) are not parameters
                        if !is_field(c, t.f_locals) {
                            req += 1;
                        }
                    }
                    K::DestructuredParameter => {
                        self.destructure(p, 0);
                        req += 1;
                    }
                    K::OptionalParameter => {
                        if let Some(v) = child(p, t.f_value) {
                            self.visit_child(c, v);
                        }
                        if let Some(x) = child(p, t.f_name) {
                            self.declare_node(x, NONE);
                        }
                        opt += 1;
                    }
                    K::KeywordParameter => {
                        let v = child(p, t.f_value);
                        if let Some(v) = v {
                            self.visit_child(c, v);
                            kw_any = true;
                        } else {
                            kw_req = true;
                        }
                        if let Some(x) = child(p, t.f_name) {
                            self.declare_node(x, NONE);
                        }
                    }
                    K::SplatParameter => {
                        variadic = true;
                        if let Some(x) = child(p, t.f_name) {
                            let ty = self.cx.intern(b"Array");
                            self.declare_node(x, ty);
                        }
                    }
                    K::HashSplatParameter => {
                        kw_any = true;
                        if let Some(x) = child(p, t.f_name) {
                            let ty = self.cx.intern(b"Hash");
                            self.declare_node(x, ty);
                        }
                    }
                    K::BlockParameter => {
                        if let Some(x) = child(p, t.f_name) {
                            self.declare_node(x, NONE);
                        }
                    }
                    K::ForwardParameter => variadic = true,
                    _ => {}
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        let kw = kw_req || kw_any;
        let min = req + kw_req as u32;
        let max = if variadic { 255 } else { (req + opt + kw as u32).min(254) };
        (min.min(254) as u8, max as u8, first)
    }

    fn destructure(&mut self, n: Node<'t>, depth: u32) {
        let mut i = 0;
        while let Some(ch) = n.named_child(i) {
            i += 1;
            match self.k(ch) {
                K::Identifier => self.declare_node(ch, NONE),
                K::SplatParameter | K::RestAssignment => {
                    if let Some(x) = child(ch, self.t.f_name).or_else(|| ch.named_child(0)) {
                        if self.k(x) == K::Identifier {
                            self.declare_node(x, NONE);
                        }
                    }
                }
                K::DestructuredParameter | K::DestructuredLeftAssignment if depth < 8 => self.destructure(ch, depth + 1),
                _ => {}
            }
        }
    }

    // ----- blocks -----

    /// A block or a lambda body: its own scope inside the current one.
    fn block(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let params = child(n, self.t.f_parameters);
        self.block_with(n, c, params, NONE);
    }

    /// Walk block `n`. `def`, when set, is the method the block is the body of
    /// (`define_method(:m) { .. }`).
    fn block_with(&mut self, _n: Node<'t>, c: &mut TreeCursor<'t>, params: Option<Node<'t>>, def: u32) -> (u8, u8) {
        let mut f = self.cur;
        f.implicit = params.is_none();
        if def != NONE {
            f.at = At::Method;
            f.method = self.cx.facts.defs[def as usize].name;
        }
        self.push_frame(f);
        let mark = self.locals.len();
        let mut arity = (0u8, 255u8);
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if Some(ch) == params {
                    let (a, b, _) = self.parameters(c);
                    arity = (a, b);
                } else {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.close_scope(mark);
        self.pop_frame();
        arity
    }

    /// `->(x) { .. }`: parameters on the lambda, the body a block without its own.
    fn lambda(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let params = child(n, t.f_parameters);
        let mut f = self.cur;
        f.implicit = params.is_none();
        self.push_frame(f);
        let mark = self.locals.len();
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if Some(ch) == params {
                    self.parameters(c);
                } else if matches!(self.k(ch), K::Block | K::DoBlock) {
                    // its parameters are the lambda's
                    self.children(c);
                } else {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.close_scope(mark);
        self.pop_frame();
    }

    // ----- receivers -----

    /// Classify receiver `e` and intern its type path.
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
            K::SelfK => {
                if self.cur.selfq == NONE {
                    return Rk::Unknown;
                }
                buf.extend_from_slice(self.cx.name_bytes(self.cur.selfq));
                return Rk::Path;
            }
            K::Identifier => {
                let name = self.cx.name(e);
                return match self.local(name) {
                    Some(ty) if ty != NONE => {
                        buf.extend_from_slice(self.cx.name_bytes(ty));
                        Rk::Path
                    }
                    Some(_) => Rk::Unknown,
                    // a call of a method of `self`
                    None if self.cur.selfq != NONE && !self.implicit_param(self.text(e)) && self.info(name) & N_STOP == 0 => {
                        buf.extend_from_slice(self.cx.name_bytes(self.cur.selfq));
                        buf.extend_from_slice(b"::");
                        buf.extend_from_slice(self.text(e));
                        Rk::Path
                    }
                    None => Rk::Unknown,
                };
            }
            K::Constant | K::ScopeResolution => {
                if !self.const_path_ok(e, 0) {
                    return Rk::Unknown;
                }
                buf.push(b'$');
                self.push_path(e, buf);
                return Rk::Const;
            }
            K::InstanceVariable | K::ClassVariable => {
                if self.cur.selfq == NONE {
                    return Rk::Unknown;
                }
                buf.extend_from_slice(self.cx.name_bytes(self.cur.selfq));
                buf.extend_from_slice(b"::");
                buf.extend_from_slice(self.text(e));
                return Rk::Path;
            }
            K::ParenthesizedStatements => {
                return match e.named_child(0) {
                    Some(x) if e.named_child_count() == 1 => self.recv(x, buf, depth + 1),
                    _ => Rk::Unknown,
                };
            }
            K::Call => return self.recv_call(e, buf, depth),
            K::Str => b"String",
            K::SimpleSymbol | K::DelimitedSymbol => b"Symbol",
            K::Integer => b"Integer",
            K::Float => b"Float",
            K::Array | K::WordArray => b"Array",
            K::Hash => b"Hash",
            K::Regex => b"Regexp",
            _ => {
                let _ = t;
                return Rk::Unknown;
            }
        };
        buf.extend_from_slice(lit);
        Rk::Path
    }

    /// Type of a call result: an instance for `T.new`, the class for `x.class`, the attribute
    /// path `P::m` for an argument-less `p.m` on a known `p`.
    fn recv_call(&mut self, e: Node<'t>, buf: &mut Vec<u8>, depth: u32) -> Rk {
        let t = self.t;
        let Some(m) = child(e, t.f_method) else { return Rk::Unknown };
        if self.k(m) != K::Identifier {
            return Rk::Unknown;
        }
        let mtext = self.text(m);
        let args = child(e, t.f_arguments);
        let Some(r) = child(e, t.f_receiver) else {
            // `new(..)` inside a class builds one of its own
            if mtext == b"new" && self.cur.selfq != NONE && self.cur.at != At::Top {
                buf.extend_from_slice(self.cx.name_bytes(self.cur.selfq));
                return Rk::Path;
            }
            return Rk::Unknown;
        };
        let rk = self.recv(r, buf, depth + 1);
        match rk {
            Rk::Unknown => Rk::Unknown,
            Rk::Const if mtext == b"new" => Rk::Path,
            Rk::Path | Rk::Const if mtext == b"class" && args.is_none() => Rk::Path,
            Rk::Path if args.is_none() && child(e, t.f_block).is_none() => {
                if matches!(mtext.last(), Some(b'?' | b'!')) {
                    return Rk::Unknown;
                }
                buf.extend_from_slice(b"::");
                buf.extend_from_slice(mtext);
                Rk::Path
            }
            _ => Rk::Unknown,
        }
    }

    /// Type path of the value of `v` (`NEUTRAL` for `nil`, `NONE` when unknown).
    fn infer(&mut self, v: Node<'t>) -> u32 {
        if self.k(v) == K::Nil {
            return NEUTRAL;
        }
        let (rk, q) = self.recv_of(v);
        if rk == Rk::Path { q } else { NONE }
    }

    /// A type path as a `Def::type_name`: `Foo` for `$Foo`, `$A::B` kept whole (the resolver
    /// walks it), nothing for paths through methods or variables.
    fn type_name_of(&mut self, ty: u32) -> u32 {
        if ty == NONE || ty == NEUTRAL {
            return NONE;
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.extend_from_slice(self.cx.name_bytes(ty));
        let id = match buf.strip_prefix(b"$") {
            // literals: `String`, `Array`, ...
            None => {
                if buf.iter().all(|c| c.is_ascii_alphanumeric()) {
                    ty
                } else {
                    NONE
                }
            }
            Some(rest) if rest.windows(2).any(|w| w == b"::") => {
                // only constant segments
                let ok = rest.split(|&c| c == b':').filter(|s| !s.is_empty()).all(|s| s.first().is_some_and(|c| c.is_ascii_uppercase()));
                if ok { ty } else { NONE }
            }
            Some(rest) => self.cx.intern(rest),
        };
        self.cx.s.buf = buf;
        id
    }

    // ----- calls -----

    /// Number of arguments in an argument list: positional ones plus one for keyword arguments
    /// taken together; 255 when a splat or `...` hides it. Block arguments do not count.
    fn count_args(&self, args: Option<Node<'t>>) -> u8 {
        let Some(a) = args else { return 0 };
        let (mut n, mut kw) = (0u32, false);
        let mut i = 0;
        while let Some(x) = a.named_child(i) {
            i += 1;
            match self.k(x) {
                K::Skip | K::BlockArgument => {}
                K::SplatArgument | K::ForwardArgument => return 255,
                K::Pair | K::HashSplatArgument => kw = true,
                _ => n += 1,
            }
        }
        (n + kw as u32).min(254) as u8
    }

    fn call(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(m) = child(n, t.f_method) else { return self.children(c) };
        let recv = child(n, t.f_receiver);
        let args = child(n, t.f_arguments);
        let line = line_of(m);
        let nargs = self.count_args(args);
        let mk = self.k(m);
        if mk == K::Super {
            self.super_call(m, nargs);
            return self.call_parts(n, c, recv, m, None);
        }
        if !matches!(mk, K::Identifier | K::Constant) || m.start_byte() == m.end_byte() {
            return self.children(c);
        }
        let mtext = self.text(m);
        let Some(r) = recv else {
            if self.bare_macro(n, m, mtext, args, c) {
                return;
            }
            let name = self.cx.name(m);
            if self.info(name) & N_STOP == 0 {
                if mtext == b"new" && self.cur.selfq != NONE && self.cur.at != At::Top {
                    // `new(..)` inside a class instantiates it
                    let d = self.cur.owner;
                    if d != NONE {
                        let cname = self.cx.facts.defs[d as usize].name;
                        self.cx.add_ref(cname, RefKind::New, line, NONE, nargs);
                    }
                } else {
                    self.self_call(name, line, nargs, RefKind::MemberCall);
                }
            }
            return self.call_parts(n, c, None, m, None);
        };
        // `Foo.new(..)` instantiates `Foo`; its namespace segments are type uses.
        if mtext == b"new" && self.const_path_ok(r, 0) {
            self.type_ref(r, RefKind::New, line, nargs);
            // (the receiver is the instantiated class, recorded above)
            return self.call_parts(n, c, None, m, None);
        }
        let (rk, q) = self.recv_of(r);
        if self.cur.hook != NONE && matches!(mtext, b"extend" | b"include" | b"prepend") && self.k(r) == K::Identifier && self.cx.name(r) == self.cur.hook {
            // `base.extend ClassMethods` in `self.included(base)`: a mixin of the module itself
            self.hook_mixins(args);
        }
        match mtext {
            b"send" | b"public_send" | b"__send__" if self.symbol_arg(args, 0).is_some() => {
                let sym = self.symbol_arg(args, 0).unwrap_or(m);
                let name = self.sym_name_id(sym);
                let rest = if nargs == 255 { 255 } else { nargs.saturating_sub(1) };
                self.cx.add_ref(name, RefKind::MemberCall, line, q, rest);
            }
            b"method" | b"instance_method" | b"public_method" if self.symbol_arg(args, 0).is_some() => {
                let sym = self.symbol_arg(args, 0).unwrap_or(m);
                let name = self.sym_name_id(sym);
                self.cx.add_ref(name, RefKind::MemberRead, line, q, 255);
            }
            // `x.class`, `x.is_a?(T)`: Object's own methods say nothing about the program
            _ if is_object_method(mtext) => {}
            _ => {
                let name = self.cx.name(m);
                let _ = rk;
                self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
            }
        }
        self.call_parts(n, c, Some(r), m, None)
    }

    /// Walk the receiver, arguments and block of call `n` (not the method name). With `def`,
    /// the block is the body of that method.
    fn call_parts(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, recv: Option<Node<'t>>, m: Node<'t>, def: Option<u32>) {
        let block = child(n, self.t.f_block);
        let receiver = child(n, self.t.f_receiver);
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if ch == m {
                } else if Some(ch) == block {
                    match def {
                        Some(d) => {
                            let params = child(ch, self.t.f_parameters);
                            self.cx.push_def(d);
                            let (a, b) = self.block_with(ch, c, params, d);
                            self.cx.pop_def();
                            let dm = self.cx.def_mut(d);
                            dm.arity_min = a;
                            dm.arity_max = b;
                        }
                        None => self.visit(c),
                    }
                } else if Some(ch) == receiver {
                    if recv.is_some() {
                        self.visit(c);
                    }
                } else if ch.is_named() {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
    }

    /// `super` and `super(..)` inside a method: the same method of a strict ancestor.
    fn super_call(&mut self, n: Node<'t>, nargs: u8) {
        let (method, q) = (self.cur.method, self.cur.selfq);
        if method == NONE || q == NONE {
            return;
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.push(b'^');
        buf.extend_from_slice(self.cx.name_bytes(q));
        let qual = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        self.cx.add_ref(method, RefKind::MemberCall, line_of(n), qual, nargs);
    }

    /// Bare `super`: it passes the method's own arguments on.
    fn zsuper(&mut self, n: Node<'t>) {
        self.super_call(n, 255);
    }

    // ----- symbols and literal strings -----

    /// The text a symbol or literal string stands for: `foo` for `:foo`, `:"foo"`, `'foo'`.
    fn sym_text(&self, n: Node<'t>) -> Option<&'a [u8]> {
        match self.k(n) {
            K::SimpleSymbol => {
                let t = self.text(n);
                (t.len() > 1).then(|| &t[1..])
            }
            K::DelimitedSymbol | K::Str => {
                if n.named_child_count() != 1 {
                    return None;
                }
                let ch = n.named_child(0)?;
                (self.k(ch) == K::StringContent).then(|| self.text(ch))
            }
            K::Identifier | K::Constant | K::Operator | K::Setter => Some(self.text(n)),
            _ => None,
        }
    }

    fn sym_name_id(&mut self, n: Node<'t>) -> u32 {
        match self.sym_text(n) {
            Some(s) if !s.is_empty() => self.cx.intern(s),
            _ => NONE,
        }
    }

    /// The `i`-th positional argument when it is a symbol or literal string (and that node).
    fn symbol_arg(&self, args: Option<Node<'t>>, i: usize) -> Option<Node<'t>> {
        let a = args?;
        let mut seen = 0;
        let mut j = 0;
        while let Some(x) = a.named_child(j) {
            j += 1;
            if matches!(self.k(x), K::Skip | K::Pair | K::BlockArgument | K::HashSplatArgument) {
                continue;
            }
            if seen == i {
                return self.sym_text(x).filter(|s| !s.is_empty() && is_method_name(s)).map(|_| x);
            }
            seen += 1;
        }
        None
    }

    /// The value of keyword argument `key` in an argument list.
    fn kwarg(&self, args: Option<Node<'t>>, key: &[u8]) -> Option<Node<'t>> {
        let a = args?;
        let mut j = 0;
        while let Some(x) = a.named_child(j) {
            j += 1;
            if self.k(x) != K::Pair {
                continue;
            }
            let k = child(x, self.t.f_key)?;
            let kt = match self.k(k) {
                K::HashKeySymbol => self.text(k),
                _ => match self.sym_text(k) {
                    Some(s) => s,
                    None => continue,
                },
            };
            if kt == key {
                return child(x, self.t.f_value);
            }
        }
        None
    }

    // ----- class macros -----

    /// Calls without a receiver that define, import, mix in or register something. Returns
    /// true when the call was fully handled.
    fn bare_macro(&mut self, n: Node<'t>, m: Node<'t>, mtext: &[u8], args: Option<Node<'t>>, c: &mut TreeCursor<'t>) -> bool {
        let line = line_of(m);
        match mtext {
            b"require" | b"require_relative" | b"load" | b"require_dependency" => {
                if let Some(s) = self.string_arg(args) {
                    let target = self.cx.name(s);
                    self.cx.add_import(target, NONE, NONE, line, 0);
                }
                self.call_parts(n, c, None, m, None);
                return true;
            }
            b"autoload" => {
                if let (Some(sym), Some(s)) = (self.symbol_arg(args, 0), self.string_arg_at(args, 1)) {
                    let name = self.sym_name_id(sym);
                    let target = self.cx.name(s);
                    self.cx.add_import(target, name, NONE, line, 0);
                    self.cx.add_ref(name, RefKind::Import, line, target, 255);
                }
                return true;
            }
            _ => {}
        }
        if self.cur.at != At::Class {
            if self.cur.at == At::Top && mtext == b"include" {
                // `include Foo` at the top level mixes into Object: a use of the module
                self.call_parts(n, c, None, m, None);
                return true;
            }
            if self.cur.at == At::Method && self.cur.selfq != NONE {
                // a class method that registers callbacks (`protect_from_forgery`)
                self.callbacks(m, mtext, args);
            }
            return self.method_ref_macro(n, m, mtext, args, c);
        }
        let owner = self.cur.owner;
        match mtext {
            // `included do .. end` (ActiveSupport::Concern) registers a block: it does not call
            // the module's own `self.included` hook.
            b"included" | b"extended" | b"prepended" | b"inherited" | b"class_methods" if child(n, self.t.f_block).is_some() => {
                self.call_parts(n, c, None, m, None);
                return true;
            }
            b"include" | b"extend" | b"prepend" | b"include_concern" => {
                self.mixins(args, owner, c, n, m);
                return true;
            }
            b"attr_reader"
            | b"attr_writer"
            | b"attr_accessor"
            | b"attr"
            | b"cattr_accessor"
            | b"mattr_accessor"
            | b"cattr_reader"
            | b"mattr_reader"
            | b"class_attribute"
            | b"attr_internal"
            | b"attr_internal_reader"
            | b"attr_internal_accessor" => {
                self.attrs(n, args, owner);
                return true;
            }
            b"private" | b"public" | b"protected" | b"module_function" | b"private_class_method" | b"public_class_method" | b"private_constant" => {
                let before = self.cx.facts.defs.len();
                self.call_parts(n, c, None, m, None);
                if matches!(mtext, b"private" | b"private_class_method") {
                    // `private def x`, `private attr_reader :x`, `private :x, :y`
                    for i in before..self.cx.facts.defs.len() {
                        let d = &mut self.cx.facts.defs[i];
                        if d.parent == owner && matches!(d.kind, DefKind::Method | DefKind::Property) {
                            d.flags |= dflag::LOCAL;
                        }
                    }
                    self.each_symbol(args, |w, s| {
                        let name = w.sym_name_id(s);
                        if let Some(&d) = w.members.get(&mkey(owner, name)) {
                            w.cx.def_mut(d).flags |= dflag::LOCAL;
                        }
                    });
                }
                return true;
            }
            b"alias_method" => {
                if let (Some(a), Some(b)) = (self.symbol_arg(args, 0), self.symbol_arg(args, 1)) {
                    self.alias_def(n, a, b);
                    return true;
                }
            }
            b"define_method" => {
                if let Some(s) = self.symbol_arg(args, 0) {
                    let name = self.sym_name_id(s);
                    let sig_end = args.map_or(m.end_byte(), |a| a.end_byte());
                    let d = self.new_def(name, DefKind::Method, n, sig_end, owner, NONE);
                    if self.cur.private {
                        self.cx.def_mut(d).flags |= dflag::LOCAL;
                    }
                    self.cx.def_mut(d).arity_max = 255;
                    self.members.insert(mkey(owner, name), d);
                    self.call_parts(n, c, None, m, Some(d));
                    return true;
                }
            }
            b"scope" => {
                if let Some(s) = self.symbol_arg(args, 0) {
                    let name = self.sym_name_id(s);
                    let d = self.new_def(name, DefKind::Method, n, s.end_byte(), owner, NONE);
                    self.cx.def_mut(d).arity_max = 255;
                    self.members.insert(mkey(owner, name), d);
                    self.cx.push_def(d);
                    self.call_parts(n, c, None, m, None);
                    self.cx.pop_def();
                    return true;
                }
            }
            b"delegate" => {
                if self.delegate(n, args) {
                    return true;
                }
            }
            b"test" if self.test_file => {
                if let Some(d) = self.test_block(n, args) {
                    self.call_parts(n, c, None, m, Some(d));
                    let dm = self.cx.def_mut(d);
                    dm.arity_min = 0;
                    dm.arity_max = 0;
                    return true;
                }
            }
            b"module" | b"class" => {}
            _ => {}
        }
        self.callbacks(m, mtext, args);
        self.method_ref_macro(n, m, mtext, args, c)
    }

    /// `before_action :authenticate, if: :ready?`: each symbol names a method of the class.
    fn callbacks(&mut self, m: Node<'t>, mtext: &[u8], args: Option<Node<'t>>) {
        if is_callback_macro(mtext) {
            let macro_name = self.cx.name(m);
            let skip = mtext.starts_with(b"skip_");
            self.each_symbol(args, |w, s| w.callback(s, macro_name, skip));
        }
        self.option_callbacks(m, mtext, args);
    }

    /// `send(:m, ..)`, `method(:m)` without a receiver: a call or a read of a method of `self`.
    fn method_ref_macro(&mut self, n: Node<'t>, m: Node<'t>, mtext: &[u8], args: Option<Node<'t>>, c: &mut TreeCursor<'t>) -> bool {
        let line = line_of(m);
        match mtext {
            b"send" | b"public_send" | b"__send__" => {
                let Some(s) = self.symbol_arg(args, 0) else { return false };
                let name = self.sym_name_id(s);
                let nargs = self.count_args(args);
                let rest = if nargs == 255 { 255 } else { nargs.saturating_sub(1) };
                self.self_call(name, line, rest, RefKind::MemberCall);
            }
            b"method" | b"instance_method" | b"public_method" => {
                let Some(s) = self.symbol_arg(args, 0) else { return false };
                let name = self.sym_name_id(s);
                self.self_call(name, line, 255, RefKind::MemberRead);
            }
            _ => return false,
        }
        self.call_parts(n, c, None, m, None);
        true
    }

    /// `if: :ready?`, `unless: :done?`, `with: :handler` on any class macro: methods of the
    /// class that the macro will call.
    fn option_callbacks(&mut self, m: Node<'t>, mtext: &[u8], args: Option<Node<'t>>) {
        let Some(a) = args else { return };
        let mut has_pair = false;
        let mut j = 0;
        while let Some(x) = a.named_child(j) {
            j += 1;
            has_pair |= self.k(x) == K::Pair;
        }
        if !has_pair {
            return;
        }
        let macro_name = self.cx.name(m);
        for key in [b"if".as_slice(), b"unless".as_slice(), b"with".as_slice()] {
            if key == b"with" && mtext != b"rescue_from" {
                continue;
            }
            let Some(v) = self.kwarg(args, key) else { continue };
            match self.k(v) {
                K::SimpleSymbol => self.callback(v, macro_name, false),
                K::Array => {
                    let mut j = 0;
                    while let Some(x) = v.named_child(j) {
                        j += 1;
                        if self.k(x) == K::SimpleSymbol {
                            self.callback(x, macro_name, false);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Symbol `s` handed to macro `macro_name` names a method of `self` that it will call.
    fn callback(&mut self, s: Node<'t>, macro_name: u32, skip_only: bool) {
        let Some(text) = self.sym_text(s).filter(|t| is_method_name(t)) else { return };
        let name = self.cx.intern(text);
        let line = line_of(s);
        if !skip_only {
            self.cx.add_ref(name, RefKind::Arg, line, macro_name, 255);
        }
        self.cx.add_ref(name, RefKind::MemberRead, line, self.cur.selfq, 255);
    }

    /// Run `f` on every positional symbol or string argument.
    fn each_symbol(&mut self, args: Option<Node<'t>>, mut f: impl FnMut(&mut Self, Node<'t>)) {
        let Some(a) = args else { return };
        let mut j = 0;
        while let Some(x) = a.named_child(j) {
            j += 1;
            match self.k(x) {
                K::SimpleSymbol | K::DelimitedSymbol | K::Str if self.sym_text(x).is_some() => f(self, x),
                _ => {}
            }
        }
    }

    fn string_arg(&self, args: Option<Node<'t>>) -> Option<Node<'t>> {
        self.string_arg_at(args, 0)
    }

    /// The content node of the `i`-th argument when it is a literal string.
    fn string_arg_at(&self, args: Option<Node<'t>>, i: u32) -> Option<Node<'t>> {
        let a = args?;
        let x = a.named_child(i)?;
        if self.k(x) != K::Str || x.named_child_count() != 1 {
            return None;
        }
        let ch = x.named_child(0)?;
        (self.k(ch) == K::StringContent && ch.start_byte() < ch.end_byte()).then_some(ch)
    }

    /// `include M`, `extend M`, `prepend M` in a class or module body.
    fn mixins(&mut self, args: Option<Node<'t>>, owner: u32, c: &mut TreeCursor<'t>, n: Node<'t>, m: Node<'t>) {
        let mut rest = false;
        if let Some(a) = args {
            let mut j = 0;
            while let Some(x) = a.named_child(j) {
                j += 1;
                if owner == NONE || !self.type_ref(x, RefKind::Implements, line_of(x), 255) {
                    rest = true;
                }
            }
        }
        if rest {
            // `extend self`, `include(*mods)`: walk what is not a constant
            if let Some(a) = args {
                if c.goto_first_child() {
                    loop {
                        if c.node() == a {
                            if c.goto_first_child() {
                                loop {
                                    let x = c.node();
                                    if x.is_named() && !self.const_path_ok(x, 0) {
                                        self.visit(c);
                                    }
                                    if !c.goto_next_sibling() {
                                        break;
                                    }
                                }
                                c.goto_parent();
                            }
                        }
                        if !c.goto_next_sibling() {
                            break;
                        }
                    }
                    c.goto_parent();
                }
            }
        }
        let _ = (n, m);
    }

    /// `base.extend X` inside `self.included(base)`: `X` is mixed into whatever includes the
    /// module, which is what an `Implements` of the module itself says.
    fn hook_mixins(&mut self, args: Option<Node<'t>>) {
        let Some(a) = args else { return };
        let owner = self.cur.owner;
        if owner == NONE {
            return;
        }
        let mut j = 0;
        while let Some(x) = a.named_child(j) {
            j += 1;
            if !self.const_path_ok(x, 0) {
                continue;
            }
            if let Some((nm, q)) = self.split_const(x) {
                let name = self.cx.name(nm);
                let line = line_of(x);
                self.cx.facts.refs.push(Ref { name, qual: q, line, encl: owner, kind: RefKind::Implements, nargs: 255 });
            }
        }
    }

    /// `attr_reader :a, :b` and friends: properties of the class.
    fn attrs(&mut self, n: Node<'t>, args: Option<Node<'t>>, owner: u32) {
        if owner == NONE {
            return;
        }
        let private = self.cur.private;
        self.each_symbol(args, |w, s| {
            let Some(text) = w.sym_text(s).filter(|t| is_method_name(t)) else { return };
            let name = w.cx.intern(text);
            let key = mkey(owner, name);
            if w.members.get(&key).is_some_and(|&d| w.cx.facts.defs[d as usize].kind == DefKind::Property) {
                return;
            }
            let d = w.new_def(name, DefKind::Property, n, s.end_byte(), owner, NONE);
            if private {
                w.cx.def_mut(d).flags |= dflag::LOCAL;
            }
            w.members.insert(key, d);
        });
    }

    /// `delegate :a, :b, to: :target` (with an optional `prefix:`): methods of the class that
    /// call `target` and pass the call on.
    fn delegate(&mut self, n: Node<'t>, args: Option<Node<'t>>) -> bool {
        let owner = self.cur.owner;
        let Some(to) = self.kwarg(args, b"to") else { return false };
        let to_name = match self.k(to) {
            K::SimpleSymbol | K::Str | K::DelimitedSymbol => self.sym_text(to),
            _ => None,
        };
        let prefix: Option<&'a [u8]> = match self.kwarg(args, b"prefix") {
            None => Some(b""),
            Some(p) => match self.k(p) {
                K::Skip if self.text(p) == b"true" => to_name,
                K::Skip if self.text(p) == b"false" => Some(b""),
                K::SimpleSymbol | K::Str => self.sym_text(p),
                _ => None,
            },
        };
        let Some(prefix) = prefix else { return false };
        let target = to_name.filter(|t| is_method_name(t) && !t.starts_with(b"@")).map(|t| self.cx.intern(t));
        let line = line_of(n);
        let sig_end = n.end_byte();
        self.each_symbol(args, |w, s| {
            let Some(text) = w.sym_text(s).filter(|t| is_method_name(t)) else { return };
            let mut buf = std::mem::take(&mut w.cx.s.buf);
            buf.clear();
            if !prefix.is_empty() {
                buf.extend_from_slice(prefix);
                buf.push(b'_');
            }
            buf.extend_from_slice(text);
            let name = w.cx.intern(&buf);
            w.cx.s.buf = buf;
            let d = w.new_def(name, DefKind::Method, n, sig_end, owner, NONE);
            w.cx.def_mut(d).arity_max = 255;
            w.members.insert(mkey(owner, name), d);
            if let Some(tg) = target {
                w.cx.push_def(d);
                w.self_call(tg, line, 0, RefKind::MemberCall);
                w.cx.pop_def();
            }
        });
        true
    }

    /// Rails `test "does a thing" do .. end`: the method `test_does_a_thing`.
    fn test_block(&mut self, n: Node<'t>, args: Option<Node<'t>>) -> Option<u32> {
        let s = self.string_arg(args)?;
        child(n, self.t.f_block)?;
        let text = self.text(s);
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.extend_from_slice(b"test_");
        let mut space = false;
        for &b in text {
            if b.is_ascii_whitespace() {
                space = true;
            } else {
                if space {
                    buf.push(b'_');
                    space = false;
                }
                buf.push(b);
            }
        }
        let name = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        let owner = self.cur.owner;
        let sig_end = args.map_or(n.end_byte(), |a| a.end_byte());
        let d = self.new_def(name, DefKind::Method, n, sig_end, owner, NONE);
        self.cx.def_mut(d).flags |= dflag::TEST;
        Some(d)
    }

    /// `alias new old` / `alias_method :new, :old`: a member `new` bound to the method `old`.
    fn alias_def(&mut self, n: Node<'t>, new: Node<'t>, old: Node<'t>) {
        let owner = self.cur.owner;
        let (Some(nt), Some(ot)) = (self.sym_text(new), self.sym_text(old)) else { return };
        if nt.is_empty() || ot.is_empty() || nt.starts_with(b"$") {
            return;
        }
        let name = self.cx.intern(nt);
        let orig = self.cx.intern(ot);
        let line = line_of(n);
        let d = self.new_def(name, if owner != NONE { DefKind::Field } else { DefKind::Variable }, n, n.end_byte(), owner, NONE);
        self.cx.def_mut(d).flags |= dflag::FNPTR;
        if owner != NONE {
            self.members.insert(mkey(owner, name), d);
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        if self.cur.selfq != NONE {
            buf.extend_from_slice(self.cx.name_bytes(self.cur.selfq));
        }
        buf.push(b'.');
        buf.extend_from_slice(nt);
        let slot = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        self.cx.push_def(d);
        self.cx.add_ref(orig, RefKind::Bind, line, slot, 255);
        self.self_call(orig, line, 255, RefKind::MemberRead);
        self.cx.pop_def();
    }

    fn alias(&mut self, n: Node<'t>) {
        let t = self.t;
        if let (Some(a), Some(b)) = (child(n, t.f_name), child(n, t.f_alias)) {
            self.alias_def(n, a, b);
        }
    }

    // ----- assignments -----

    fn assignment(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let (Some(left), right) = (child(n, t.f_left), child(n, t.f_right)) else { return self.children(c) };
        let sig_end = right.map_or(n.end_byte(), |r| r.start_byte());
        match self.k(left) {
            K::Identifier => {
                let ty = right.map_or(NONE, |r| self.infer(r));
                // Ruby binds the name as soon as it sees `x =`: `x = x` reads the new local.
                self.declare_node(left, ty);
                if let Some(r) = right {
                    self.visit_child(c, r);
                }
            }
            K::Constant | K::ScopeResolution => self.constant_assignment(n, left, right, sig_end, c),
            K::InstanceVariable | K::ClassVariable => {
                let ty = right.map_or(NONE, |r| self.infer(r));
                let ty = self.type_name_of(ty);
                self.ivar_write(left, n, sig_end, ty);
                if let Some(r) = right {
                    self.visit_child(c, r);
                }
            }
            K::GlobalVariable => {
                let name = self.cx.name(left);
                let line = line_of(left);
                if self.cur.at == At::Top && self.cur.owner == NONE && !self.members.contains_key(&mkey(NONE, name)) {
                    let d = self.new_def(name, DefKind::Variable, n, sig_end, NONE, NONE);
                    self.members.insert(mkey(NONE, name), d);
                } else {
                    self.cx.add_ref(name, RefKind::Write, line, NONE, 255);
                }
                if let Some(r) = right {
                    self.visit_child(c, r);
                }
            }
            K::Call => {
                self.member_write(left, c);
                if let Some(r) = right {
                    self.visit_child(c, r);
                }
            }
            K::LeftAssignmentList => {
                self.targets(left, c, 0);
                if let Some(r) = right {
                    self.visit_child(c, r);
                }
            }
            _ => self.children(c),
        }
    }

    /// `obj.attr = v`: a write of the attribute, then the receiver is walked.
    fn member_write(&mut self, left: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let (Some(m), recv) = (child(left, t.f_method), child(left, t.f_receiver)) else { return self.visit_child(c, left) };
        if self.k(m) != K::Identifier || m.start_byte() == m.end_byte() {
            return self.visit_child(c, left);
        }
        let q = match recv {
            Some(r) => self.recv_of(r).1,
            None => self.cur.selfq,
        };
        let name = self.cx.name(m);
        self.cx.add_ref(name, RefKind::MemberWrite, line_of(m), q, 255);
        if let Some(r) = recv {
            if c.goto_first_child() {
                loop {
                    if c.node() == left {
                        self.visit_child(c, r);
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

    /// Targets of a multiple assignment. Cursor on the assignment.
    fn targets(&mut self, list: Node<'t>, c: &mut TreeCursor<'t>, depth: u32) {
        let mut j = 0;
        while let Some(x) = list.named_child(j) {
            j += 1;
            match self.k(x) {
                K::Identifier => self.declare_node(x, NONE),
                K::InstanceVariable | K::ClassVariable => self.ivar_write(x, x, x.end_byte(), NONE),
                K::RestAssignment => {
                    if let Some(id) = x.named_child(0) {
                        if self.k(id) == K::Identifier {
                            self.declare_node(id, NONE);
                        }
                    }
                }
                K::DestructuredLeftAssignment if depth < 8 => self.targets(x, c, depth + 1),
                K::Call => {
                    let t = self.t;
                    if let Some(m) = child(x, t.f_method).filter(|m| self.k(*m) == K::Identifier) {
                        let q = match child(x, t.f_receiver) {
                            Some(r) => self.recv_of(r).1,
                            None => self.cur.selfq,
                        };
                        let name = self.cx.name(m);
                        self.cx.add_ref(name, RefKind::MemberWrite, line_of(m), q, 255);
                    }
                }
                _ => {}
            }
        }
    }

    /// `X = v` in a class, module or file: a constant, or a struct, class or module when the
    /// value builds one. The first assignment defines, later ones write.
    fn constant_assignment(&mut self, n: Node<'t>, left: Node<'t>, right: Option<Node<'t>>, sig_end: usize, c: &mut TreeCursor<'t>) {
        let owner = self.cur.owner;
        if self.k(left) == K::ScopeResolution {
            // `Foo::X = v` assigns a constant of another namespace
            self.scope_resolution(left, c, RefKind::Write);
            if let Some(r) = right {
                self.visit_child(c, r);
            }
            return;
        }
        let name = self.cx.name(left);
        let key = mkey(owner, name);
        if self.members.contains_key(&key) {
            self.cx.add_ref(name, RefKind::Write, line_of(left), NONE, 255);
            if let Some(r) = right {
                self.visit_child(c, r);
            }
            return;
        }
        if let Some(r) = right {
            if let Some(kind) = self.struct_call(r) {
                let d = self.new_def(name, kind, n, sig_end, owner, NONE);
                self.members.insert(key, d);
                self.cx.def_mut(d).arity_max = 255;
                self.struct_fields(r, d);
                return self.struct_body(n, r, d, c);
            }
            if let Some(kind) = self.class_new(r) {
                let d = self.new_def(name, kind, n, sig_end, owner, NONE);
                self.members.insert(key, d);
                self.cx.def_mut(d).arity_max = 255;
                self.cx.push_def(d);
                if let Some(base) = child(r, self.t.f_arguments).and_then(|a| a.named_child(0)) {
                    self.type_ref(base, RefKind::Extends, line_of(base), 255);
                }
                self.cx.pop_def();
                return self.struct_body(n, r, d, c);
            }
        }
        let d = self.new_def(name, DefKind::Constant, n, sig_end, owner, NONE);
        self.members.insert(key, d);
        if let Some(r) = right {
            // `Alias = Other::Class` stands for that class; `X = Foo.new` is a `Foo`
            let ty = if self.const_path_ok(r, 0) { self.path_id(r, true) } else { self.infer(r) };
            let ty = self.type_name_of(ty);
            self.cx.def_mut(d).type_name = ty;
            self.cx.push_def(d);
            self.visit_child(c, r);
            self.cx.pop_def();
        }
    }

    /// `Struct.new(..)` / `Data.define(..)`: what it defines.
    fn struct_call(&self, r: Node<'t>) -> Option<DefKind> {
        if self.k(r) != K::Call {
            return None;
        }
        let t = self.t;
        let recv = child(r, t.f_receiver).filter(|x| self.k(*x) == K::Constant)?;
        let m = child(r, t.f_method)?;
        match (self.text(recv), self.text(m)) {
            (b"Struct", b"new") | (b"Data", b"define") => Some(DefKind::Struct),
            _ => None,
        }
    }

    /// `Class.new(Base)` / `Module.new`.
    fn class_new(&self, r: Node<'t>) -> Option<DefKind> {
        if self.k(r) != K::Call {
            return None;
        }
        let t = self.t;
        let recv = child(r, t.f_receiver).filter(|x| self.k(*x) == K::Constant)?;
        let m = child(r, t.f_method)?;
        match (self.text(recv), self.text(m)) {
            (b"Class", b"new") => Some(DefKind::Class),
            (b"Module", b"new") => Some(DefKind::Module),
            _ => None,
        }
    }

    /// The members a `Struct.new(:a, :b)` call declares, as fields of `d`.
    fn struct_fields(&mut self, call: Node<'t>, d: u32) {
        let args = child(call, self.t.f_arguments);
        let mut n = 0u8;
        self.each_symbol(args, |w, s| {
            let Some(text) = w.sym_text(s).filter(|t| is_method_name(t)) else { return };
            let name = w.cx.intern(text);
            let f = w.new_def(name, DefKind::Field, s, s.end_byte(), d, NONE);
            w.members.insert(mkey(d, name), f);
            n = n.saturating_add(1);
        });
        let dm = self.cx.def_mut(d);
        if dm.kind == DefKind::Struct {
            dm.arity_min = 0;
            dm.arity_max = n.min(254);
        }
    }

    /// The block of `Struct.new(..) do .. end` / `Class.new do .. end` is the body of `d`.
    /// Cursor on the assignment `n`.
    fn struct_body(&mut self, n: Node<'t>, r: Node<'t>, d: u32, c: &mut TreeCursor<'t>) {
        let _ = n;
        let block = child(r, self.t.f_block);
        let mut selfq_buf = std::mem::take(&mut self.cx.s.buf);
        selfq_buf.clear();
        match self.cur.selfq {
            NONE => selfq_buf.push(b'$'),
            q => {
                selfq_buf.extend_from_slice(self.cx.name_bytes(q));
                selfq_buf.extend_from_slice(b"::");
            }
        }
        let dn = self.cx.facts.defs[d as usize].name;
        selfq_buf.extend_from_slice(self.cx.name_bytes(dn));
        let selfq = self.cx.intern(&selfq_buf);
        self.cx.s.buf = selfq_buf;
        let Some(b) = block else { return };
        if c.goto_first_child() {
            loop {
                if c.node() == r {
                    self.cx.push_def(d);
                    self.push_frame(Frame { at: At::Class, owner: d, selfq, method: NONE, private: false, implicit: false, hook: NONE });
                    let (floor, mark) = (self.floor, self.locals.len());
                    self.floor = mark;
                    if c.goto_first_child() {
                        loop {
                            if c.node() == b {
                                // the block's own body, with its parameters as locals
                                let params = child(b, self.t.f_parameters);
                                self.block_with(b, c, params, NONE);
                            }
                            if !c.goto_next_sibling() {
                                break;
                            }
                        }
                        c.goto_parent();
                    }
                    self.close_scope(mark);
                    self.floor = floor;
                    self.pop_frame();
                    self.cx.pop_def();
                    break;
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
    }

    /// `x += 1`, `@x ||= v`, `obj.attr ||= v`, `h[k] += 1`.
    fn op_assignment(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let (Some(left), right) = (child(n, t.f_left), child(n, t.f_right)) else { return self.children(c) };
        let sig_end = right.map_or(n.end_byte(), |r| r.start_byte());
        match self.k(left) {
            K::Identifier => {
                let ty = right.map_or(NONE, |r| self.infer(r));
                self.declare_node(left, ty);
            }
            K::InstanceVariable | K::ClassVariable => {
                let ty = right.map_or(NONE, |r| self.infer(r));
                let ty = self.type_name_of(ty);
                self.ivar_write(left, n, sig_end, ty);
            }
            K::Constant => {
                let name = self.cx.name(left);
                self.cx.add_ref(name, RefKind::Write, line_of(left), NONE, 255);
            }
            K::Call => self.member_write(left, c),
            _ => self.visit_child(c, left),
        }
        if let Some(r) = right {
            self.visit_child(c, r);
        }
    }

    // ----- control flow with bindings -----

    fn for_loop(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let pat = child(n, t.f_pattern);
        if let Some(p) = pat {
            match self.k(p) {
                K::Identifier => self.declare_node(p, NONE),
                K::LeftAssignmentList => self.targets(p, c, 0),
                _ => {}
            }
        }
        if c.goto_first_child() {
            loop {
                if Some(c.node()) != pat {
                    self.visit(c);
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
    }

    /// `rescue A, B => e`.
    fn rescue(&mut self, c: &mut TreeCursor<'t>) {
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if self.k(ch) == K::ExceptionVariable {
                    match ch.named_child(0) {
                        Some(x) if self.k(x) == K::Identifier => self.declare_node(x, NONE),
                        Some(x) if matches!(self.k(x), K::InstanceVariable | K::ClassVariable) => self.ivar_write(x, x, x.end_byte(), NONE),
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

    /// `in pattern if guard then body`: the pattern binds first.
    fn in_clause(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let pat = child(n, self.t.f_pattern);
        if c.goto_first_child() {
            loop {
                if Some(c.node()) == pat {
                    self.pattern(c, 0);
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

    /// `value => pattern`, `value in pattern`.
    fn match_pattern(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let pat = child(n, self.t.f_pattern);
        if c.goto_first_child() {
            loop {
                if Some(c.node()) == pat {
                    self.pattern(c, 0);
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

    /// A pattern: bare names bind, constants are type uses, `^x` and `^(expr)` are reads.
    fn pattern(&mut self, c: &mut TreeCursor<'t>, depth: u32) {
        let n = c.node();
        if depth > 64 {
            return;
        }
        let t = self.t;
        match self.k(n) {
            K::Identifier => self.declare_node(n, NONE),
            K::Constant => self.constant(n, RefKind::Type),
            K::ScopeResolution => self.scope_resolution(n, c, RefKind::Type),
            K::VariableReferencePattern | K::ExpressionReferencePattern => self.children(c),
            K::KeywordPattern => {
                match child(n, t.f_value) {
                    // `{name: pat}`
                    Some(v) => {
                        if c.goto_first_child() {
                            loop {
                                if c.node() == v {
                                    self.pattern(c, depth + 1);
                                }
                                if !c.goto_next_sibling() {
                                    break;
                                }
                            }
                            c.goto_parent();
                        }
                    }
                    // `{name:}` binds `name`
                    None => {
                        if let Some(k) = child(n, t.f_key) {
                            let text = self.text(k);
                            let text = text.strip_suffix(b":").unwrap_or(text);
                            if !text.is_empty() {
                                let name = self.cx.intern(text);
                                self.declare(name, NONE);
                            }
                        }
                    }
                }
            }
            K::SplatParameter | K::HashSplatParameter => {
                if let Some(x) = child(n, t.f_name) {
                    self.declare_node(x, NONE);
                }
            }
            K::Lambda | K::Call | K::Str | K::Regex | K::Interpolation => self.visit(c),
            K::Skip | K::Integer | K::Float | K::Nil | K::SimpleSymbol | K::HashKeySymbol => {}
            _ => {
                if c.goto_first_child() {
                    loop {
                        if c.node().is_named() {
                            self.pattern(c, depth + 1);
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

    /// `key: value`; the shorthand `key:` reads the local or method `key`.
    fn pair(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        if child(n, t.f_value).is_none() {
            if let Some(k) = child(n, t.f_key).filter(|k| self.k(*k) == K::HashKeySymbol && k.start_byte() < k.end_byte()) {
                let name = self.cx.name(k);
                if !self.is_local(name) && self.info(name) & N_STOP == 0 {
                    self.self_call(name, line_of(k), 0, RefKind::MemberCall);
                }
            }
            return;
        }
        self.children(c);
    }

    // ----- end of file -----

    /// An attribute whose instance variable has a known type has that type too.
    fn finish(&mut self) {
        let n = self.cx.facts.defs.len();
        for i in 0..n {
            let d = self.cx.facts.defs[i];
            if d.kind != DefKind::Property || d.type_name != NONE || d.parent == NONE {
                continue;
            }
            let mut buf = std::mem::take(&mut self.cx.s.buf);
            buf.clear();
            buf.push(b'@');
            buf.extend_from_slice(self.cx.name_bytes(d.name));
            let iv = self.cx.intern(&buf);
            self.cx.s.buf = buf;
            if let Some(&f) = self.members.get(&mkey(d.parent, iv)) {
                let ty = self.cx.facts.defs[f as usize].type_name;
                self.cx.facts.defs[i].type_name = ty;
            }
        }
    }
}

/// A name a method can have: `foo`, `foo?`, `foo!`, `foo=`, `Foo`.
fn is_method_name(s: &[u8]) -> bool {
    let body = s.strip_suffix(b"?").or_else(|| s.strip_suffix(b"!")).or_else(|| s.strip_suffix(b"=")).unwrap_or(s);
    !body.is_empty() && !body[0].is_ascii_digit() && body.iter().all(|&b| b == b'_' || b.is_ascii_alphanumeric() || b >= 0x80)
}

/// Methods every object has from `Object` and `Kernel`, which programs do not redefine.
fn is_object_method(m: &[u8]) -> bool {
    matches!(
        m,
        b"class"
            | b"is_a?"
            | b"kind_of?"
            | b"instance_of?"
            | b"respond_to?"
            | b"nil?"
            | b"frozen?"
            | b"freeze"
            | b"object_id"
            | b"singleton_class"
            | b"instance_variable_get"
            | b"instance_variable_set"
            | b"instance_variable_defined?"
            | b"send"
            | b"public_send"
            | b"__send__"
            | b"tap"
            | b"then"
            | b"itself"
            | b"extend"
            | b"include"
            | b"prepend"
    )
}

/// `MAX_SIZE`, `DEBUG`: at least two characters, upper case throughout.
fn is_constant_name(s: &[u8]) -> bool {
    s.len() >= 2 && s[0].is_ascii_uppercase() && !s.iter().any(|b| b.is_ascii_lowercase())
}

/// Macros whose symbol arguments name methods of the class that will be called back: Rails
/// controller, model, job and test callbacks.
fn is_callback_macro(m: &[u8]) -> bool {
    let m = m.strip_prefix(b"skip_").unwrap_or(m);
    let m = m.strip_prefix(b"prepend_").or_else(|| m.strip_prefix(b"append_")).unwrap_or(m);
    if let Some(rest) = m.strip_prefix(b"before_").or_else(|| m.strip_prefix(b"after_")).or_else(|| m.strip_prefix(b"around_")) {
        return !rest.is_empty();
    }
    matches!(m, b"validate" | b"setup" | b"teardown" | b"helper_method" | b"rescue_from" | b"after_commit" | b"before_commit")
}

/// Core classes and modules: using one is not a reference worth recording.
fn is_builtin_const(s: &[u8]) -> bool {
    matches!(
        s,
        b"String"
            | b"Integer"
            | b"Float"
            | b"Array"
            | b"Hash"
            | b"Symbol"
            | b"NilClass"
            | b"TrueClass"
            | b"FalseClass"
            | b"Object"
            | b"BasicObject"
            | b"Kernel"
            | b"Comparable"
            | b"Enumerable"
            | b"Numeric"
            | b"Proc"
            | b"Struct"
            | b"Class"
            | b"Module"
            | b"Exception"
            | b"StandardError"
            | b"RuntimeError"
            | b"ArgumentError"
            | b"TypeError"
            | b"NameError"
            | b"NoMethodError"
            | b"NotImplementedError"
            | b"KeyError"
            | b"IndexError"
            | b"RangeError"
            | b"IOError"
            | b"StopIteration"
            | b"Regexp"
            | b"Range"
            | b"Time"
            | b"File"
            | b"Dir"
            | b"IO"
            | b"Math"
            | b"Process"
            | b"Thread"
            | b"Mutex"
            | b"Set"
            | b"ENV"
            | b"ARGV"
            | b"STDOUT"
            | b"STDERR"
            | b"STDIN"
            | b"Rational"
            | b"Complex"
            | b"Data"
            | b"Method"
            | b"UnboundMethod"
            | b"Encoding"
            | b"Fiber"
            | b"ObjectSpace"
            | b"GC"
            | b"Signal"
            | b"Marshal"
            | b"Random"
    )
}

/// The stop list: Kernel and Module methods whose bare calls are not references.
fn is_builtin(w: &[u8]) -> bool {
    matches!(
        w,
        b"puts"
            | b"print"
            | b"p"
            | b"pp"
            | b"printf"
            | b"sprintf"
            | b"format"
            | b"raise"
            | b"fail"
            | b"require"
            | b"require_relative"
            | b"load"
            | b"autoload"
            | b"lambda"
            | b"proc"
            | b"loop"
            | b"block_given?"
            | b"binding"
            | b"caller"
            | b"caller_locations"
            | b"catch"
            | b"throw"
            | b"gets"
            | b"sleep"
            | b"rand"
            | b"srand"
            | b"exit"
            | b"exit!"
            | b"abort"
            | b"at_exit"
            | b"system"
            | b"exec"
            | b"spawn"
            | b"fork"
            | b"trap"
            | b"warn"
            | b"send"
            | b"public_send"
            | b"__send__"
            | b"__method__"
            | b"__dir__"
            | b"__callee__"
            | b"respond_to?"
            | b"is_a?"
            | b"kind_of?"
            | b"instance_of?"
            | b"nil?"
            | b"freeze"
            | b"frozen?"
            | b"tap"
            | b"then"
            | b"itself"
            | b"class"
            | b"singleton_class"
            | b"instance_variable_get"
            | b"instance_variable_set"
            | b"instance_variable_defined?"
            | b"instance_variables"
            | b"define_method"
            | b"method"
            | b"attr"
            | b"attr_reader"
            | b"attr_writer"
            | b"attr_accessor"
            | b"include"
            | b"extend"
            | b"prepend"
            | b"private"
            | b"public"
            | b"protected"
            | b"module_function"
            | b"private_constant"
            | b"private_class_method"
            | b"public_class_method"
            | b"alias_method"
            | b"Integer"
            | b"Float"
            | b"String"
            | b"Array"
            | b"Hash"
            | b"Rational"
            | b"Complex"
            | b"object_id"
            | b"instance_eval"
            | b"instance_exec"
            | b"class_eval"
            | b"module_eval"
            | b"class_exec"
            | b"const_get"
            | b"const_set"
            | b"const_defined?"
            | b"__FILE__"
    )
}
