//! Go extractor (tree-sitter-go).
//!
//! Go code is linked through packages, and almost every call site goes through a selector:
//! `pkg.Func()`, `recv.Method()`, `s.client.Get()`. What this extractor adds to plain definitions
//! and call sites:
//!   * a per-file map of import aliases, so that `v1.Pod`, `klog.Infof(...)` and `cache.Store`
//!     carry the import path they resolve through,
//!   * the type of every local that syntax can tell (receiver, parameters, `var x T`, `x := T{}`,
//!     `x := &T{}`, `x := new(T)`, `x := f()`, range and type-switch bindings), block scope by
//!     block scope, so that member accesses carry a receiver type path the resolver can follow
//!     through field types,
//!   * functions bound into struct fields (`cobra.Command{RunE: run}`, `s.handler = s.serve`),
//!     the Go spelling of an operations table,
//!   * the `var _ Iface = (*Impl)(nil)` assertion as an explicit "implements" fact.
//!
//! Type names: pointers, slices, arrays, maps (their value type), channels and type arguments are
//! stripped. A type of another package keeps its package as `IMPORT_PATH|Name`, where the import
//! path is the one the package name is bound to in this file. A slice or map has no members of its
//! own, so `xs[i].f` and `for _, x := range xs { x.f }` can only mean the element type; a channel
//! yields its elements, also to the single variable of `for v := range ch`.

use crate::extract::*;
use crate::facts::*;
use crate::lang::Lang;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::OnceLock;
use tree_sitter::{InputEdit, Node, Parser, Point, Tree, TreeCursor};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    Other,
    Skip,
    Comment,
    Error,
    Identifier,
    TypeIdentifier,
    FieldIdentifier,
    PackageClause,
    ImportDeclaration,
    ImportSpec,
    FunctionDeclaration,
    MethodDeclaration,
    TypeDeclaration,
    TypeSpec,
    VarDeclaration,
    ConstDeclaration,
    VarSpecList,
    VarSpec,
    ConstSpec,
    ParameterList,
    ParameterDeclaration,
    VariadicParameterDeclaration,
    TypeParameterList,
    TypeParameterDeclaration,
    StructType,
    FieldDeclarationList,
    FieldDeclaration,
    InterfaceType,
    MethodElem,
    TypeElem,
    QualifiedType,
    GenericType,
    PointerType,
    ParenthesizedType,
    SliceType,
    ArrayType,
    MapType,
    ChannelType,
    FunctionType,
    Block,
    Scoped,
    ShortVarDeclaration,
    AssignmentStatement,
    IncStatement,
    RangeClause,
    ReceiveStatement,
    TypeSwitch,
    TypeCase,
    CallExpression,
    SelectorExpression,
    IndexExpression,
    SliceExpression,
    UnaryExpression,
    ParenthesizedExpression,
    TypeAssertion,
    TypeConversion,
    TypeInstantiation,
    CompositeLiteral,
    LiteralValue,
    LiteralElement,
    KeyedElement,
    FuncLiteral,
}

struct Tables {
    kinds: KindTable<K>,
    f_name: u16,
    f_type: u16,
    f_body: u16,
    f_parameters: u16,
    f_result: u16,
    f_receiver: u16,
    f_type_arguments: u16,
    f_path: u16,
    f_package: u16,
    f_operand: u16,
    f_field: u16,
    f_function: u16,
    f_arguments: u16,
    f_left: u16,
    f_right: u16,
    f_operator: u16,
    f_value: u16,
    f_key: u16,
    f_element: u16,
    f_alias: u16,
    /// Anonymous tokens that decide what a statement means.
    s_define: u16,
    s_assign: u16,
    s_star: u16,
    s_amp: u16,
    s_arrow: u16,
    s_lparen: u16,
    s_lbrace: u16,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let lang: tree_sitter::Language = tree_sitter_go::LANGUAGE.into();
        let kinds = KindTable::build(
            &lang,
            K::Other,
            &[
                ("ERROR", K::Error),
                ("comment", K::Comment),
                ("identifier", K::Identifier),
                ("type_identifier", K::TypeIdentifier),
                ("field_identifier", K::FieldIdentifier),
                ("package_clause", K::PackageClause),
                ("import_declaration", K::ImportDeclaration),
                ("import_spec", K::ImportSpec),
                ("function_declaration", K::FunctionDeclaration),
                ("method_declaration", K::MethodDeclaration),
                ("type_declaration", K::TypeDeclaration),
                ("type_spec", K::TypeSpec),
                ("type_alias", K::TypeSpec),
                ("var_declaration", K::VarDeclaration),
                ("const_declaration", K::ConstDeclaration),
                ("var_spec_list", K::VarSpecList),
                ("var_spec", K::VarSpec),
                ("const_spec", K::ConstSpec),
                ("parameter_list", K::ParameterList),
                ("parameter_declaration", K::ParameterDeclaration),
                ("variadic_parameter_declaration", K::VariadicParameterDeclaration),
                ("type_parameter_list", K::TypeParameterList),
                ("type_parameter_declaration", K::TypeParameterDeclaration),
                ("struct_type", K::StructType),
                ("field_declaration_list", K::FieldDeclarationList),
                ("field_declaration", K::FieldDeclaration),
                ("interface_type", K::InterfaceType),
                ("method_elem", K::MethodElem),
                ("type_elem", K::TypeElem),
                ("qualified_type", K::QualifiedType),
                ("generic_type", K::GenericType),
                ("pointer_type", K::PointerType),
                ("parenthesized_type", K::ParenthesizedType),
                ("slice_type", K::SliceType),
                ("array_type", K::ArrayType),
                ("implicit_length_array_type", K::ArrayType),
                ("map_type", K::MapType),
                ("channel_type", K::ChannelType),
                ("function_type", K::FunctionType),
                ("block", K::Block),
                // statements that open an implicit block
                ("if_statement", K::Scoped),
                ("for_statement", K::Scoped),
                ("expression_switch_statement", K::Scoped),
                ("expression_case", K::Scoped),
                ("default_case", K::Scoped),
                ("communication_case", K::Scoped),
                ("short_var_declaration", K::ShortVarDeclaration),
                ("assignment_statement", K::AssignmentStatement),
                ("inc_statement", K::IncStatement),
                ("dec_statement", K::IncStatement),
                ("range_clause", K::RangeClause),
                ("receive_statement", K::ReceiveStatement),
                ("type_switch_statement", K::TypeSwitch),
                ("type_case", K::TypeCase),
                ("call_expression", K::CallExpression),
                ("selector_expression", K::SelectorExpression),
                ("index_expression", K::IndexExpression),
                ("slice_expression", K::SliceExpression),
                ("unary_expression", K::UnaryExpression),
                ("parenthesized_expression", K::ParenthesizedExpression),
                ("type_assertion_expression", K::TypeAssertion),
                ("type_conversion_expression", K::TypeConversion),
                ("type_instantiation_expression", K::TypeInstantiation),
                ("composite_literal", K::CompositeLiteral),
                ("literal_value", K::LiteralValue),
                ("literal_element", K::LiteralElement),
                ("keyed_element", K::KeyedElement),
                ("func_literal", K::FuncLiteral),
                // leaves and subtrees that never contain references
                ("interpreted_string_literal", K::Skip),
                ("raw_string_literal", K::Skip),
                ("int_literal", K::Skip),
                ("float_literal", K::Skip),
                ("imaginary_literal", K::Skip),
                ("rune_literal", K::Skip),
                ("nil", K::Skip),
                ("true", K::Skip),
                ("false", K::Skip),
                ("iota", K::Skip),
                ("label_name", K::Skip),
                ("package_identifier", K::Skip),
                ("blank_identifier", K::Skip),
                ("dot", K::Skip),
                ("break_statement", K::Skip),
                ("continue_statement", K::Skip),
                ("goto_statement", K::Skip),
                ("fallthrough_statement", K::Skip),
                ("empty_statement", K::Skip),
            ],
        );
        Tables {
            kinds,
            f_name: field(&lang, "name"),
            f_type: field(&lang, "type"),
            f_body: field(&lang, "body"),
            f_parameters: field(&lang, "parameters"),
            f_result: field(&lang, "result"),
            f_receiver: field(&lang, "receiver"),
            f_type_arguments: field(&lang, "type_arguments"),
            f_path: field(&lang, "path"),
            f_package: field(&lang, "package"),
            f_operand: field(&lang, "operand"),
            f_field: field(&lang, "field"),
            f_function: field(&lang, "function"),
            f_arguments: field(&lang, "arguments"),
            f_left: field(&lang, "left"),
            f_right: field(&lang, "right"),
            f_operator: field(&lang, "operator"),
            f_value: field(&lang, "value"),
            f_key: field(&lang, "key"),
            f_element: field(&lang, "element"),
            f_alias: field(&lang, "alias"),
            s_define: lang.id_for_node_kind(":=", false),
            s_assign: lang.id_for_node_kind("=", false),
            s_star: lang.id_for_node_kind("*", false),
            s_amp: lang.id_for_node_kind("&", false),
            s_arrow: lang.id_for_node_kind("<-", false),
            s_lparen: lang.id_for_node_kind("(", false),
            s_lbrace: lang.id_for_node_kind("{", false),
        }
    })
}

// What the top-level declarations of this file say about a name. Collected before the walk, so
// that uses may precede declarations.
/// A type.
const F_TYPE: u8 = 1;
/// A type whose underlying type is a map, slice or array: keys of its literals are not fields.
const F_ELEMS: u8 = 2;
/// A method (of any receiver).
const F_METHOD: u8 = 4;
/// A function, variable or constant.
const F_VALUE: u8 = 8;

// How a package name was bound to an import path, weakest first.
/// A spelling convention for a path element that is not an identifier (`go-restful`, `yaml.v3`),
/// or the version element of a path that may be an API version (`k8s.io/api/autoscaling/v2`).
const A_GUESS: u8 = 1;
/// The element before a major version (`k8s.io/klog/v2` -> `klog`).
const A_VERSIONED: u8 = 2;
/// The last path element (`net/http` -> `http`).
const A_LAST: u8 = 3;
/// Written in the import spec (`handler "example.com/cli/ui"`).
const A_EXPLICIT: u8 = 4;

/// Longest receiver expression (in nested operands) and type path (in bytes) that is followed.
const MAX_PATH_DEPTH: u32 = 40;
const MAX_PATH_LEN: usize = 1024;

/// How a name is used where it stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Use {
    Read,
    Call,
}

/// What is known about the type of a composite literal body written without its type
/// (`[]T{{...}}`, `map[K]V{k: {...}}`).
#[derive(Clone, Copy)]
enum Elem<'t> {
    Unknown,
    /// The type as written.
    Type(Node<'t>),
    /// A struct whose fields belong to this type path.
    Fields(u32),
}

/// What the keys and elements of a `{ ... }` literal body are.
#[derive(Clone, Copy)]
enum Lit<'t> {
    /// A named type, taken to be a struct: keys are its fields.
    Fields(u32),
    /// An anonymous struct: its fields cannot be referred to from anywhere else.
    Anon,
    /// Array or slice: keys are indices, elements have the given type.
    Elems(Elem<'t>),
    /// Map: key type and value type.
    Map(Elem<'t>, Elem<'t>),
    Unknown,
}

struct W<'a, 't> {
    cx: Ctx<'a>,
    t: &'static Tables,
    /// Nesting of function bodies (declarations and literals): inside one, declarations are locals.
    fn_depth: u32,
    /// Set while the left-hand side of an assignment is visited: `writes` is only consulted then.
    lhs_depth: u32,
    /// Some node of the current top-level declaration is in `handled`.
    any_marks: bool,
    /// The callee just visited is a method expression (`T.Method(recv, x)`): the first argument
    /// of the call is the receiver and does not count.
    recv_arg: bool,
    has_error: bool,
    is_test: bool,
    /// A top-level declaration of this file reuses a predeclared name (`min`, `max`, `copy`...).
    shadowed: bool,
    /// Flags of the names declared at the top level of this file, by name id.
    top: Vec<u8>,
    /// Import path bound to a package name in this file, by name id. A name that two imports
    /// claim with equal right is bound to itself: a package, but of an unknown path.
    alias: Vec<u32>,
    /// How each binding in `alias` was made (`A_*`), by name id: a stronger one replaces it.
    alias_rank: Vec<u8>,
    /// Definition that owns the fields of the struct type being visited; `NONE` = no field defs.
    owner: u32,
    /// Named interface whose method elements are being visited, or `NONE`.
    iface: u32,
    /// Type that an iota-style `const` spec without type and value repeats.
    const_type: u32,
    /// `var _ Iface = (*Impl)(nil)`: (index of the Implements ref, name of the implementing type).
    impls: Vec<(u32, u32)>,
    /// (name, type path, is a channel) of names collected for declaration: parameters until the
    /// signature has been visited, the names of a `var` spec until its values have been.
    params: Vec<(u32, u32, bool)>,
    /// The anonymous struct types of locals: field name and field type, by `type - ANON`.
    anon: Vec<Vec<(u32, u32)>>,
    /// Names declared in each open scope, innermost last, each with what `decl_scope` and
    /// `decl_chan` said about the name before: leaving the scope restores it.
    scope_names: Vec<(u32, u32, bool)>,
    scope_starts: Vec<u32>,
    /// Serial numbers of the open scopes, innermost last; every scope gets a fresh one.
    scope_serials: Vec<u32>,
    next_serial: u32,
    /// By name id: serial of the scope whose declaration of the name is visible (0: none).
    /// `x, err := f()` redeclares `x` only when the innermost scope does not have it yet.
    decl_scope: Vec<u32>,
    /// By name id: the visible declaration has a channel type, so that `for v := range name`
    /// binds an element and not an index or key.
    decl_chan: Vec<bool>,
    _tree: PhantomData<Node<'t>>,
}

pub fn extract(lang: Lang, src: &[u8], tree: &Tree, path: &str, facts: &mut FileFacts, scratch: &mut Scratch) {
    let _ = lang;
    let reparsed = if tree.root_node().has_error() { reparse_new_expr(src, tree) } else { None };
    let tree = reparsed.as_ref().unwrap_or(tree);
    let cx = Ctx::new(src, path, facts, scratch);
    let root = tree.root_node();
    let mut w = W {
        cx,
        t: tables(),
        fn_depth: 0,
        lhs_depth: 0,
        any_marks: false,
        recv_arg: false,
        has_error: root.has_error(),
        is_test: path.ends_with("_test.go"),
        shadowed: false,
        top: Vec::new(),
        alias: Vec::new(),
        alias_rank: Vec::new(),
        owner: NONE,
        iface: NONE,
        const_type: NONE,
        impls: Vec::new(),
        params: Vec::new(),
        anon: Vec::new(),
        scope_names: Vec::new(),
        scope_starts: Vec::new(),
        scope_serials: Vec::new(),
        next_serial: 0,
        decl_scope: Vec::new(),
        decl_chan: Vec::new(),
        _tree: PhantomData,
    };
    w.prescan(root);
    let mut c = tree.walk();
    w.file(&mut c);
    w.finish();
}

impl<'a, 't> W<'a, 't> {
    #[inline]
    fn k(&self, n: Node<'t>) -> K {
        self.t.kinds.get(n.kind_id())
    }

    /// First named child that is not a comment.
    fn inner(&self, n: Node<'t>) -> Option<Node<'t>> {
        let cnt = n.named_child_count().min(8) as u32;
        (0..cnt).filter_map(|i| n.named_child(i)).find(|x| self.k(*x) != K::Comment)
    }

    #[inline]
    fn top_flags(&self, name: u32) -> u8 {
        self.top.get(name as usize).copied().unwrap_or(0)
    }

    /// Import path that package name `name` stands for in this file.
    #[inline]
    fn alias_of(&self, name: u32) -> Option<u32> {
        match self.alias.get(name as usize) {
            Some(&p) if p != NONE => Some(p),
            _ => None,
        }
    }

    /// A predeclared identifier that no top-level declaration of this file redefines.
    #[inline]
    fn is_builtin(&mut self, text: &[u8]) -> bool {
        is_predeclared(text)
            && !(self.shadowed && {
                let id = self.cx.intern(text);
                self.top_flags(id) != 0
            })
    }

    // ----- local scopes -----

    fn push_scope(&mut self) {
        self.cx.push_scope();
        self.scope_starts.push(self.scope_names.len() as u32);
        self.next_serial += 1;
        self.scope_serials.push(self.next_serial);
    }

    fn pop_scope(&mut self) {
        self.cx.pop_scope();
        self.scope_serials.pop();
        if let Some(start) = self.scope_starts.pop() {
            // Innermost first: a name declared twice in the scope gets back what it had before.
            for i in (start as usize..self.scope_names.len()).rev() {
                let (name, scope, chan) = self.scope_names[i];
                self.decl_scope[name as usize] = scope;
                self.decl_chan[name as usize] = chan;
            }
            self.scope_names.truncate(start as usize);
        }
    }

    fn declare(&mut self, name: u32, ty: u32) {
        self.declare_as(name, ty, false);
    }

    fn declare_as(&mut self, name: u32, ty: u32, chan: bool) {
        let Some(&serial) = self.scope_serials.last() else { return };
        self.cx.declare(name, ty);
        let i = name as usize;
        if i >= self.decl_scope.len() {
            self.decl_scope.resize(i + 1, 0);
            self.decl_chan.resize(i + 1, false);
        }
        self.scope_names.push((name, self.decl_scope[i], self.decl_chan[i]));
        self.decl_scope[i] = serial;
        self.decl_chan[i] = chan;
    }

    /// The innermost scope declares `name`.
    fn declared_here(&self, name: u32) -> bool {
        match (self.scope_serials.last(), self.decl_scope.get(name as usize)) {
            (Some(&serial), Some(&s)) => s == serial,
            _ => false,
        }
    }

    /// The visible local `name` was declared with a channel type.
    fn is_chan_local(&self, name: u32) -> bool {
        self.decl_chan.get(name as usize).copied().unwrap_or(false)
    }

    /// The type is a channel type as written (`chan T`, `<-chan T`, in parentheses or not).
    fn is_chan_type(&self, mut ty: Node<'t>) -> bool {
        for _ in 0..4 {
            match self.k(ty) {
                K::ChannelType => return true,
                K::ParenthesizedType => match self.inner(ty) {
                    Some(x) => ty = x,
                    None => return false,
                },
                _ => return false,
            }
        }
        false
    }

    /// The expression yields a channel as far as syntax shows: a channel-typed local, possibly
    /// in parentheses, or `make(chan T)`.
    fn is_chan_expr(&mut self, mut e: Node<'t>) -> bool {
        let t = self.t;
        for _ in 0..4 {
            match self.k(e) {
                K::Identifier => {
                    let text = self.cx.text(e);
                    if text.is_empty() {
                        return false;
                    }
                    let id = self.cx.intern(text);
                    return self.cx.is_local(id) && self.is_chan_local(id);
                }
                K::ParenthesizedExpression => match self.inner(e) {
                    Some(x) => e = x,
                    None => return false,
                },
                K::CallExpression => {
                    let Some(f) = child(e, t.f_function).filter(|f| self.k(*f) == K::Identifier) else { return false };
                    if self.cx.text(f) != b"make" || !self.is_builtin(b"make") {
                        return false;
                    }
                    return child(e, t.f_arguments).and_then(|a| self.inner(a)).is_some_and(|ty| self.is_chan_type(ty));
                }
                _ => return false,
            }
        }
        false
    }

    // ----- traversal -----

    fn file(&mut self, c: &mut TreeCursor<'t>) {
        if c.node().is_error() {
            self.cx.facts.parse_errors += 1;
        }
        each_child(c, |c| {
            self.visit(c);
            // Marks and write targets never outlive a top-level declaration.
            if self.any_marks {
                self.cx.s.handled.clear();
                self.any_marks = false;
            }
            if self.cx.s.writes.len() > 4096 {
                self.cx.s.writes.clear();
            }
        });
    }

    fn children(&mut self, c: &mut TreeCursor<'t>) {
        each_child(c, |c| self.visit(c));
    }

    /// Children in a fresh block scope.
    fn scoped(&mut self, c: &mut TreeCursor<'t>) {
        self.push_scope();
        self.children(c);
        self.pop_scope();
    }

    /// Children of a declaration whose direct identifier children are the declared names.
    fn skip_names(&mut self, c: &mut TreeCursor<'t>) {
        each_child(c, |c| {
            if self.k(c.node()) != K::Identifier {
                self.visit(c);
            }
        });
    }

    /// Every handler recurses through here, so the depth guard covers hand-written child loops too.
    fn visit(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        let n = c.node();
        match self.k(n) {
            K::Skip | K::Comment | K::FieldIdentifier | K::ImportDeclaration | K::ImportSpec => {}
            K::Identifier => self.identifier(n),
            K::TypeIdentifier => self.type_ref(n, RefKind::Type),
            K::QualifiedType => self.qualified_type(n, RefKind::Type),
            K::SelectorExpression => {
                self.selector(n, c, Use::Read);
            }
            K::CallExpression => self.call_expression(n, c),
            K::CompositeLiteral => self.composite_literal(n, c),
            K::LiteralValue => self.literal_value(c, Elem::Unknown),
            K::ShortVarDeclaration => self.short_var_declaration(n, c),
            K::AssignmentStatement => self.assignment(n, c),
            K::IncStatement => self.inc_statement(n, c),
            K::Block | K::Scoped | K::TypeCase => self.scoped(c),
            K::RangeClause => self.range_or_receive(n, c, 1),
            K::ReceiveStatement => self.range_or_receive(n, c, 0),
            K::TypeSwitch => self.type_switch(c),
            K::FuncLiteral => self.func_literal(c),
            K::TypeConversion => self.type_conversion(n, c),
            K::TypeInstantiation => self.type_instantiation(c),
            K::ParameterList => {
                self.param_list(c, false);
            }
            K::ParameterDeclaration | K::VariadicParameterDeclaration | K::TypeParameterDeclaration => self.skip_names(c),
            K::FieldDeclaration => self.field_declaration(n, c),
            K::InterfaceType => {
                // Anonymous interface: its methods are nobody's members.
                let saved = std::mem::replace(&mut self.iface, NONE);
                self.children(c);
                self.iface = saved;
            }
            K::FunctionType => {
                // A struct or interface written inside a signature belongs to no declaration.
                let saved = (std::mem::replace(&mut self.owner, NONE), std::mem::replace(&mut self.iface, NONE));
                self.children(c);
                (self.owner, self.iface) = saved;
            }
            K::MethodElem => self.method_elem(n, c),
            K::TypeElem => self.type_elem(n, c),
            K::FunctionDeclaration => self.function(n, c, false),
            K::MethodDeclaration => self.function(n, c, true),
            K::TypeDeclaration => self.type_declaration(n, c),
            K::TypeSpec => self.type_spec(n, c, n),
            K::VarDeclaration => self.var_declaration(n, c, DefKind::Variable),
            K::ConstDeclaration => self.var_declaration(n, c, DefKind::Constant),
            K::VarSpec => self.var_spec(n, c, n, DefKind::Variable),
            K::ConstSpec => self.var_spec(n, c, n, DefKind::Constant),
            K::PackageClause => self.package_clause(n),
            K::Error => {
                // Tokens the parser could not place surface as identifiers directly under ERROR
                // nodes: whether they declare or use a name is unknown, so they are skipped.
                self.cx.facts.parse_errors += 1;
                self.skip_names(c);
            }
            K::Other => {
                if self.has_error && n.is_missing() {
                    self.cx.facts.parse_errors += 1;
                }
                self.children(c);
            }
            _ => self.children(c),
        }
        self.cx.depth -= 1;
    }

    // ----- pre-scan: imports and the names this file declares -----

    fn flag_top(&mut self, name: Node<'t>, flags: u8) {
        let text = self.cx.text(name);
        if text.is_empty() || text == b"_" {
            return;
        }
        let id = self.cx.intern(text) as usize;
        if id >= self.top.len() {
            self.top.resize(id + 1, 0);
        }
        self.top[id] |= flags;
        if flags & (F_TYPE | F_VALUE) != 0 && is_predeclared(text) {
            self.shadowed = true;
        }
    }

    fn prescan(&mut self, root: Node<'t>) {
        let t = self.t;
        let mut c = root.walk();
        each_child(&mut c, |c| {
            let n = c.node();
            match self.k(n) {
                K::ImportDeclaration => self.import_declaration(c, 0),
                K::FunctionDeclaration | K::MethodDeclaration => {
                    if let Some(nm) = child(n, t.f_name) {
                        self.flag_top(nm, if self.k(n) == K::MethodDeclaration { F_METHOD } else { F_VALUE });
                    }
                }
                K::TypeDeclaration | K::VarDeclaration | K::ConstDeclaration => self.prescan_specs(c, 0),
                _ => {}
            }
        });
        let n0 = self.cx.facts.names.len();
        self.alias.resize(n0, NONE);
        self.top.resize(n0, 0);
    }

    fn prescan_specs(&mut self, c: &mut TreeCursor<'t>, depth: u32) {
        let t = self.t;
        each_child(c, |c| {
            let n = c.node();
            match self.k(n) {
                K::TypeSpec => {
                    if let Some(nm) = child(n, t.f_name) {
                        let elems = child(n, t.f_type).is_some_and(|ty| matches!(self.k(ty), K::MapType | K::SliceType | K::ArrayType));
                        self.flag_top(nm, if elems { F_TYPE | F_ELEMS } else { F_TYPE });
                    }
                }
                K::VarSpec | K::ConstSpec => each_child(c, |c| {
                    let ch = c.node();
                    if self.k(ch) == K::Identifier {
                        self.flag_top(ch, F_VALUE);
                    }
                }),
                K::VarSpecList if depth == 0 => self.prescan_specs(c, 1),
                _ => {}
            }
        });
    }

    fn import_declaration(&mut self, c: &mut TreeCursor<'t>, depth: u32) {
        each_child(c, |c| {
            let n = c.node();
            match self.k(n) {
                K::ImportSpec => self.import_spec(n),
                // import_spec_list
                K::Other if depth == 0 && n.is_named() => self.import_declaration(c, 1),
                _ => {}
            }
        });
    }

    fn import_spec(&mut self, spec: Node<'t>) {
        let t = self.t;
        let Some(pn) = child(spec, t.f_path) else { return };
        let raw = self.cx.text(pn);
        if raw.len() < 3 {
            return;
        }
        let p = &raw[1..raw.len() - 1];
        let target = self.cx.intern(p);
        let mut flags = if is_std_import(p) { iflag::SYSTEM } else { 0 };
        let mut alias = NONE;
        match child(spec, t.f_name).map(|nm| self.cx.text(nm)) {
            Some(b".") => flags |= iflag::WILDCARD,
            Some(b"_") => alias = self.cx.intern(b"_"),
            Some(a) if !a.is_empty() => {
                alias = self.cx.intern(a);
                self.set_alias(alias, target, A_EXPLICIT);
            }
            _ => self.default_aliases(p, target),
        }
        self.cx.add_import(target, NONE, alias, line_of(spec), flags);
    }

    /// Bind a package name to an import path. Whatever the order of the imports, an alias
    /// written in the file beats every inferred name, and a likelier inference a less likely
    /// one. Two inferences of equal strength for different paths leave the name a package of
    /// unknown path (bound to itself): one of the two packages is not named after its path, and
    /// which one cannot be seen from here.
    fn set_alias(&mut self, name: u32, target: u32, rank: u8) {
        let i = name as usize;
        if i >= self.alias.len() {
            self.alias.resize(i + 1, NONE);
        }
        if i >= self.alias_rank.len() {
            self.alias_rank.resize(i + 1, 0);
        }
        let cur = self.alias_rank[i];
        if rank > cur {
            self.alias[i] = target;
            self.alias_rank[i] = rank;
        } else if rank == cur && self.alias[i] != target && rank != A_EXPLICIT {
            // (Two explicit aliases of one name do not compile: the first stays.)
            self.alias[i] = name;
        }
    }

    fn infer_alias(&mut self, name: &[u8], target: u32, rank: u8) {
        if is_ident(name) && !is_keyword(name) {
            let id = self.cx.intern(name);
            self.set_alias(id, target, rank);
        }
    }

    /// Package names an import without alias may be used under. The package name is declared in
    /// the imported package, which this file cannot see, so the conventions are registered as
    /// candidates and the one the file actually uses is the one that matches:
    ///   * the last path element (`net/http` -> `http`),
    ///   * the element before a major version (`k8s.io/klog/v2` -> `klog`); `v2` itself stays a
    ///     candidate because API packages are named after their version (`k8s.io/api/core/v1`),
    ///   * for an element that is not an identifier (`go-restful`, `yaml.v3`, `mount-utils`): its
    ///     words without a `go` / `golang` prefix, a `go` suffix or a `.vN` suffix: the first word
    ///     (what goimports assumes), all words joined, joined by `_`, and the last word.
    fn default_aliases(&mut self, p: &[u8], target: u32) {
        let (dir, base) = match memchr::memrchr(b'/', p) {
            Some(i) => (&p[..i], &p[i + 1..]),
            None => (&p[..0], p),
        };
        let version = is_version(base) && !dir.is_empty();
        let elem = if version {
            match memchr::memrchr(b'/', dir) {
                Some(i) => &dir[i + 1..],
                None => dir,
            }
        } else {
            base
        };
        let mut rank = A_LAST;
        if version {
            if matches!(base, b"v0" | b"v1") {
                // Never a module major version: the directory is the package.
                self.infer_alias(base, target, A_LAST);
                self.infer_alias(elem, target, A_GUESS);
                return;
            }
            self.infer_alias(base, target, A_GUESS);
            rank = A_VERSIONED;
        }
        if is_ident(elem) {
            self.infer_alias(elem, target, rank);
            return;
        }
        let mut words: [&[u8]; 8] = [b""; 8];
        let mut n = 0;
        for w in elem.split(|&b| !is_word_byte(b)) {
            if !w.is_empty() && n < words.len() {
                words[n] = w;
                n += 1;
            }
        }
        let mut words = &words[..n];
        if words.len() > 1 && matches!(words[0], b"go" | b"golang") {
            words = &words[1..];
        }
        while words.len() > 1 && (words[words.len() - 1] == b"go" || is_version(words[words.len() - 1])) {
            words = &words[..words.len() - 1];
        }
        let Some(first) = words.first() else { return };
        self.infer_alias(first, target, A_GUESS);
        if words.len() > 1 {
            for sep in [&b""[..], b"_"] {
                let mut buf = std::mem::take(&mut self.cx.s.buf);
                buf.clear();
                for (i, w) in words.iter().enumerate() {
                    if i > 0 {
                        buf.extend_from_slice(sep);
                    }
                    buf.extend_from_slice(w);
                }
                self.infer_alias(&buf, target, A_GUESS);
                self.cx.s.buf = buf;
            }
            self.infer_alias(words[words.len() - 1], target, A_GUESS);
        }
    }

    // ----- leaves -----

    fn identifier(&mut self, n: Node<'t>) {
        let text = self.cx.text(n);
        if text.is_empty() || self.is_builtin(text) {
            return;
        }
        if self.any_marks && self.cx.is_marked(n) {
            return;
        }
        let name = self.cx.intern(text);
        if self.cx.is_local(name) {
            return;
        }
        let kind = if self.lhs_depth > 0 && self.cx.s.writes.contains(&(n.start_byte() as u32)) { RefKind::Write } else { RefKind::Read };
        self.cx.add_ref(name, kind, line_of(n), NONE, 255);
    }

    /// A type name in a type position: `Type`, `New` (literal), `Extends` (embedding) or
    /// `Implements` (assertion).
    fn type_ref(&mut self, n: Node<'t>, kind: RefKind) {
        let text = self.cx.text(n);
        if text.is_empty() || self.is_builtin(text) {
            return;
        }
        let name = self.cx.intern(text);
        // Type parameters, and locals that shadow a type.
        if self.cx.is_local(name) {
            return;
        }
        self.cx.add_ref(name, kind, line_of(n), NONE, 255);
    }

    /// `pkg.Type`: the qualifier is the import path the package name is bound to.
    fn qualified_type(&mut self, n: Node<'t>, kind: RefKind) {
        let t = self.t;
        let Some(nm) = child(n, t.f_name).filter(|x| x.end_byte() > x.start_byte()) else { return };
        let name = self.cx.name(nm);
        let q = match child(n, t.f_package) {
            Some(p) => {
                let pid = self.cx.name(p);
                if let Some(lt) = self.cx.local_type(pid) {
                    // `x.f` with a local `x`: a value where the grammar expects a type
                    // (`fs[x.f]()`, `new(x.f)`).
                    return self.cx.add_ref(name, RefKind::MemberRead, line_of(nm), named(lt), 255);
                }
                self.alias_of(pid).unwrap_or(NONE)
            }
            None => NONE,
        };
        self.cx.add_ref(name, kind, line_of(nm), q, 255);
    }

    /// A named type (`T`, `pkg.T`, `T[A]`) referenced as `kind`; any other type is visited.
    fn named_type(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: RefKind) {
        match self.k(n) {
            K::TypeIdentifier => self.type_ref(n, kind),
            K::QualifiedType => self.qualified_type(n, kind),
            K::GenericType => each_child(c, |c| {
                let ch = c.node();
                match self.k(ch) {
                    K::TypeIdentifier => self.type_ref(ch, kind),
                    K::QualifiedType => self.qualified_type(ch, kind),
                    _ => self.visit(c),
                }
            }),
            _ => self.visit(c),
        }
    }

    // ----- type names -----

    /// Name of a type: pointers, slices, arrays, maps (value type), channels, parentheses and
    /// type arguments stripped. A type of another package is `IMPORT_PATH|Name`. `NONE` for
    /// predeclared types, type parameters, function types and anonymous structs / interfaces.
    fn type_name_of(&mut self, mut ty: Node<'t>) -> u32 {
        let t = self.t;
        for _ in 0..16 {
            let next = match self.k(ty) {
                K::TypeIdentifier => {
                    let text = self.cx.text(ty);
                    if text.is_empty() || self.is_builtin(text) {
                        return NONE;
                    }
                    let id = self.cx.intern(text);
                    return if self.cx.is_local(id) { NONE } else { id };
                }
                K::QualifiedType => {
                    let Some(nm) = child(ty, t.f_name).filter(|nm| nm.end_byte() > nm.start_byte()) else { return NONE };
                    let Some(p) = child(ty, t.f_package) else { return self.cx.name(nm) };
                    let pid = self.cx.name(p);
                    // `x.f` with a local `x` is a value the parser took for a type (`new(x.f)`).
                    if self.cx.is_local(pid) {
                        return NONE;
                    }
                    let text = self.cx.text(nm);
                    return self.module_qualified(pid, text);
                }
                K::GenericType => child(ty, t.f_type),
                K::PointerType | K::ParenthesizedType => self.inner(ty),
                K::SliceType | K::ArrayType => child(ty, t.f_element),
                K::MapType | K::ChannelType => child(ty, t.f_value),
                _ => None,
            };
            match next {
                Some(x) => ty = x,
                None => return NONE,
            }
        }
        NONE
    }

    /// The import path package name `pkg` stands for (the package name itself when no import of
    /// this file is known under it), followed by `|`: the module prefix of a qualified name.
    fn push_module(&self, buf: &mut Vec<u8>, pkg: u32) {
        let id = self.alias_of(pkg).unwrap_or(pkg);
        buf.extend_from_slice(self.cx.name_bytes(id));
        buf.push(b'|');
    }

    /// `IMPORT_PATH|name`, interned: a name of another package.
    fn module_qualified(&mut self, pkg: u32, name: &[u8]) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        let keep = buf.len();
        self.push_module(&mut buf, pkg);
        buf.extend_from_slice(name);
        let id = self.cx.intern(&buf[keep..]);
        buf.truncate(keep);
        self.cx.s.buf = buf;
        id
    }

    /// The name without its module prefix: `Set` for `github.com/scylladb/go-set/strset|Set`.
    fn simple_name(&mut self, name: u32) -> u32 {
        let Some(i) = memchr::memrchr(b'|', self.cx.name_bytes(name)) else { return name };
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        let keep = buf.len();
        buf.extend_from_slice(&self.cx.name_bytes(name)[i + 1..]);
        let id = self.cx.intern(&buf[keep..]);
        buf.truncate(keep);
        self.cx.s.buf = buf;
        id
    }

    /// `IMPORT_PATH|name` for a type named through an import path id (`NONE`: the plain name).
    fn path_qualified(&mut self, path: u32, name: u32) -> u32 {
        if path == NONE || name == NONE {
            return name;
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        let keep = buf.len();
        buf.extend_from_slice(self.cx.name_bytes(path));
        buf.push(b'|');
        buf.extend_from_slice(self.cx.name_bytes(name));
        let id = self.cx.intern(&buf[keep..]);
        buf.truncate(keep);
        self.cx.s.buf = buf;
        id
    }

    /// Where the signature of a declaration of type `ty` ends: before the body of a struct or
    /// interface type, else after the type.
    fn type_sig_end(&self, ty: Node<'t>) -> usize {
        let body = match self.k(ty) {
            K::StructType => self.inner(ty).filter(|b| self.k(*b) == K::FieldDeclarationList),
            K::InterfaceType => ty.child(1).filter(|b| b.kind_id() == self.t.s_lbrace),
            _ => None,
        };
        body.map(|b| b.start_byte()).unwrap_or(ty.end_byte())
    }

    /// The `func(...)` type a type denotes directly.
    fn func_type(&self, mut ty: Node<'t>) -> Option<Node<'t>> {
        for _ in 0..4 {
            match self.k(ty) {
                K::FunctionType => return Some(ty),
                K::ParenthesizedType => ty = self.inner(ty)?,
                _ => return None,
            }
        }
        None
    }

    /// Type name of the first result: `(a T, err error)` -> `T`.
    fn result_type(&mut self, r: Node<'t>) -> u32 {
        let first = if self.k(r) == K::ParameterList { self.inner(r).and_then(|p| child(p, self.t.f_type)) } else { Some(r) };
        first.map(|x| self.type_name_of(x)).unwrap_or(NONE)
    }

    /// Arity and first result type of a `func(...)` type or a function literal.
    fn signature_info(&mut self, f: Node<'t>) -> (u8, u8, u32) {
        let t = self.t;
        let (mut n, mut variadic) = (0u32, false);
        if let Some(ps) = child(f, t.f_parameters) {
            for i in 0..ps.named_child_count().min(64) as u32 {
                let Some(p) = ps.named_child(i) else { break };
                match self.k(p) {
                    K::ParameterDeclaration => {
                        let names = (0..p.named_child_count().min(64) as u32).filter(|&j| p.named_child(j).is_some_and(|x| self.k(x) == K::Identifier)).count();
                        n += (names as u32).max(1);
                    }
                    K::VariadicParameterDeclaration => variadic = true,
                    _ => {}
                }
            }
        }
        let ret = child(f, t.f_result).map(|r| self.result_type(r)).unwrap_or(NONE);
        let n = n.min(254) as u8;
        (n, if variadic { 255 } else { n }, ret)
    }

    /// Type name of an expression whose type is spelled out: `T{}`, `&T{}`, `new(T)`,
    /// `(*T)(nil)`, `x.(T)`, `[]T{}`. `NONE` for anything that needs more than syntax.
    /// The type of a local as written, where an anonymous struct counts: `[]struct{ in T; want U }`
    /// is remembered with its fields, so that `tt.in.Method()` in a table-driven test has a
    /// receiver type. Everything else is `type_name_of`.
    fn local_type_of(&mut self, mut ty: Node<'t>) -> u32 {
        let t = self.t;
        for _ in 0..16 {
            let next = match self.k(ty) {
                K::PointerType | K::ParenthesizedType => self.inner(ty),
                K::SliceType | K::ArrayType => child(ty, t.f_element),
                K::MapType | K::ChannelType => child(ty, t.f_value),
                K::StructType => return self.anon_struct(ty),
                _ => return self.type_name_of(ty),
            };
            match next {
                Some(x) => ty = x,
                None => return NONE,
            }
        }
        NONE
    }

    fn anon_struct(&mut self, st: Node<'t>) -> u32 {
        let t = self.t;
        if self.anon.len() >= 4096 {
            return NONE;
        }
        let Some(body) = self.inner(st).filter(|b| self.k(*b) == K::FieldDeclarationList) else { return NONE };
        // Its place is taken before the fields are read: a field may be another such struct.
        let at = self.anon.len();
        self.anon.push(Vec::new());
        let mut fields: Vec<(u32, u32)> = Vec::new();
        for i in 0..body.named_child_count().min(256) as u32 {
            let Some(f) = body.named_child(i).filter(|f| self.k(*f) == K::FieldDeclaration) else { continue };
            let Some(fty) = child(f, t.f_type) else { continue };
            let ty = self.local_type_of(fty);
            let mut named = false;
            for j in 0..f.named_child_count().min(32) as u32 {
                let Some(nm) = f.named_child(j).filter(|x| self.k(*x) == K::FieldIdentifier) else { continue };
                named = true;
                let text = self.cx.text(nm);
                if !text.is_empty() {
                    let id = self.cx.intern(text);
                    fields.push((id, ty));
                }
            }
            // An embedded field is called after its type.
            if !named && ty != NONE && !is_anon(ty) {
                let id = self.simple_name(ty);
                fields.push((id, ty));
            }
        }
        self.anon[at] = fields;
        ANON + at as u32
    }

    /// The anonymous struct type an expression has, where locals say: the local itself, an
    /// element of it, a field of it that is such a struct again, a literal of one.
    fn anon_of_expr(&mut self, mut e: Node<'t>) -> Option<u32> {
        let t = self.t;
        for _ in 0..8 {
            e = match self.k(e) {
                K::Identifier => {
                    let text = self.cx.text(e);
                    if text.is_empty() {
                        return None;
                    }
                    let id = self.cx.intern(text);
                    return self.cx.local_type(id).filter(|&ty| is_anon(ty));
                }
                K::ParenthesizedExpression => self.inner(e)?,
                K::IndexExpression | K::SliceExpression => child(e, t.f_operand)?,
                K::UnaryExpression => {
                    let op = child(e, t.f_operator).map(|o| o.kind_id());
                    if op != Some(t.s_amp) && op != Some(t.s_star) {
                        return None;
                    }
                    child(e, t.f_operand)?
                }
                K::SelectorExpression => {
                    let (op, fl) = (child(e, t.f_operand)?, child(e, t.f_field)?);
                    let owner = self.anon_of_expr(op)?;
                    let field = self.cx.text(fl);
                    return self.anon_field(owner, field).filter(|&ty| is_anon(ty));
                }
                K::CompositeLiteral => {
                    let ty = child(e, t.f_type)?;
                    let id = self.local_type_of(ty);
                    return Some(id).filter(|&ty| is_anon(ty));
                }
                _ => return None,
            };
        }
        None
    }

    /// The type of field `name` of a local anonymous struct type.
    fn anon_field(&self, anon: u32, name: &[u8]) -> Option<u32> {
        let fields = self.anon.get((anon - ANON) as usize)?;
        fields.iter().find(|(f, _)| self.cx.name_bytes(*f) == name).map(|&(_, ty)| ty)
    }

    fn expr_type_name(&mut self, mut e: Node<'t>) -> u32 {
        let t = self.t;
        for _ in 0..6 {
            let next = match self.k(e) {
                K::CompositeLiteral | K::TypeAssertion => return child(e, t.f_type).map(|ty| self.type_name_of(ty)).unwrap_or(NONE),
                K::TypeConversion => {
                    return match child(e, t.f_type) {
                        // `F[T](x)` is more often a generic call than a conversion
                        Some(ty) if self.k(ty) != K::GenericType => self.type_name_of(ty),
                        _ => NONE,
                    };
                }
                K::ParenthesizedExpression => self.inner(e),
                K::UnaryExpression => {
                    let op = child(e, t.f_operator).map(|o| o.kind_id());
                    if op == Some(t.s_amp) || op == Some(t.s_star) { child(e, t.f_operand) } else { None }
                }
                K::CallExpression => {
                    let Some(f) = child(e, t.f_function) else { return NONE };
                    match self.k(f) {
                        K::Identifier => {
                            let text = self.cx.text(f);
                            if matches!(text, b"new" | b"make") && self.is_builtin(text) {
                                let Some(first) = child(e, t.f_arguments).and_then(|a| self.inner(a)) else { return NONE };
                                let id = self.type_name_of(first);
                                if id != NONE || text != b"new" {
                                    return id;
                                }
                                // `new(T{...})`
                                Some(first)
                            } else {
                                // conversion to a type of this file
                                let id = self.cx.intern(text);
                                return if self.top_flags(id) & F_TYPE != 0 && !self.cx.is_local(id) { id } else { NONE };
                            }
                        }
                        // `(*T)(nil)`
                        K::ParenthesizedExpression => {
                            return match self.pointer_conversion(f) {
                                Some((id, _, path)) => self.path_qualified(path, id),
                                None => NONE,
                            };
                        }
                        _ => return NONE,
                    }
                }
                _ => None,
            };
            match next {
                Some(x) => e = x,
                None => return NONE,
            }
        }
        NONE
    }

    /// `(*T)` / `(*pkg.T)` in callee position: a conversion to a pointer type. Returns the type
    /// name, the node that names it, and the import path for a qualified type.
    fn pointer_conversion(&mut self, f: Node<'t>) -> Option<(u32, Node<'t>, u32)> {
        let t = self.t;
        let mut e = f;
        let mut star = false;
        for _ in 0..6 {
            match self.k(e) {
                K::ParenthesizedExpression => e = self.inner(e)?,
                K::UnaryExpression => {
                    if child(e, t.f_operator).map(|o| o.kind_id()) != Some(t.s_star) {
                        return None;
                    }
                    star = true;
                    e = child(e, t.f_operand)?;
                }
                K::Identifier if star => {
                    let text = self.cx.text(e);
                    if text.is_empty() || self.is_builtin(text) {
                        return None;
                    }
                    let id = self.cx.intern(text);
                    return if self.cx.is_local(id) { None } else { Some((id, e, NONE)) };
                }
                K::SelectorExpression if star => {
                    let (op, fl) = (child(e, t.f_operand)?, child(e, t.f_field)?);
                    if self.k(op) != K::Identifier || fl.end_byte() == fl.start_byte() {
                        return None;
                    }
                    let oid = self.cx.name(op);
                    if self.cx.is_local(oid) {
                        return None;
                    }
                    let path = self.alias_of(oid)?;
                    return Some((self.cx.name(fl), fl, path));
                }
                _ => return None,
            }
        }
        None
    }

    // ----- type paths of expressions -----

    /// Type path of an expression used as a receiver or initialiser (`T`, `T::field`, `$global`,
    /// `fn()`), interned; `NONE` when syntax cannot tell.
    fn qual_of(&mut self, e: Node<'t>) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let ok = self.qual_path(e, &mut buf, 0);
        let q = if ok && !buf.is_empty() { self.cx.intern(&buf) } else { NONE };
        self.cx.s.buf = buf;
        q
    }

    fn push_name(&self, buf: &mut Vec<u8>, id: u32) -> bool {
        if id == NONE || is_anon(id) {
            return false;
        }
        buf.extend_from_slice(self.cx.name_bytes(id));
        true
    }

    fn qual_path(&mut self, e: Node<'t>, buf: &mut Vec<u8>, depth: u32) -> bool {
        // Builder chains run long: `cfg.WithA().WithB()...` with a dozen links is ordinary.
        if depth > MAX_PATH_DEPTH || buf.len() > MAX_PATH_LEN {
            return false;
        }
        let t = self.t;
        match self.k(e) {
            K::Identifier => {
                let text = self.cx.text(e);
                if text.is_empty() {
                    return false;
                }
                let name = self.cx.intern(text);
                if let Some(ty) = self.cx.local_type(name) {
                    return self.push_name(buf, ty);
                }
                if self.is_builtin(text) || self.alias_of(name).is_some() {
                    return false;
                }
                buf.push(b'$');
                buf.extend_from_slice(text);
                true
            }
            K::SelectorExpression => {
                let (Some(op), Some(fl)) = (child(e, t.f_operand), child(e, t.f_field)) else { return false };
                let ftext = self.cx.text(fl);
                if ftext.is_empty() {
                    return false;
                }
                if let Some(owner) = self.anon_of_expr(op) {
                    // `tt.want`: the field of a local anonymous struct, typed where it was written.
                    return match self.anon_field(owner, ftext) {
                        Some(ty) => self.push_name(buf, ty),
                        None => false,
                    };
                }
                if self.k(op) == K::Identifier {
                    let oname = self.cx.name(op);
                    if !self.cx.is_local(oname) && self.alias_of(oname).is_some() {
                        // `pkg.Var`: a variable of another package
                        self.push_module(buf, oname);
                        buf.push(b'$');
                        buf.extend_from_slice(ftext);
                        return true;
                    }
                }
                if !self.qual_path(op, buf, depth + 1) {
                    return false;
                }
                buf.extend_from_slice(b"::");
                buf.extend_from_slice(ftext);
                true
            }
            K::ParenthesizedExpression => match self.inner(e) {
                Some(x) => self.qual_path(x, buf, depth + 1),
                None => false,
            },
            K::UnaryExpression => {
                // `*p`, `&x` and `<-ch` keep the element type; arithmetic has no members.
                let op = child(e, t.f_operator).map(|o| o.kind_id());
                let keeps = op == Some(t.s_star) || op == Some(t.s_amp) || op == Some(t.s_arrow);
                match child(e, t.f_operand) {
                    Some(x) if keeps => self.qual_path(x, buf, depth + 1),
                    _ => false,
                }
            }
            K::IndexExpression | K::SliceExpression => match child(e, t.f_operand) {
                Some(x) => self.qual_path(x, buf, depth + 1),
                None => false,
            },
            K::CompositeLiteral | K::TypeAssertion | K::TypeConversion => {
                let id = self.expr_type_name(e);
                self.push_name(buf, id)
            }
            K::CallExpression => self.call_path(e, buf, depth),
            _ => false,
        }
    }

    /// Type path of a call result: `f()` for a function (the resolver reads its return type),
    /// the type itself for `new(T)`, `make([]T, n)` and conversions.
    fn call_path(&mut self, e: Node<'t>, buf: &mut Vec<u8>, depth: u32) -> bool {
        let t = self.t;
        let Some(mut f) = child(e, t.f_function) else { return false };
        // `f[T](x)`: explicit instantiation of a generic function
        if self.k(f) == K::IndexExpression {
            match child(f, t.f_operand) {
                Some(x) => f = x,
                None => return false,
            }
        }
        match self.k(f) {
            K::Identifier => {
                let text = self.cx.text(f);
                if text.is_empty() {
                    return false;
                }
                let name = self.cx.intern(text);
                if self.cx.is_local(name) {
                    return false;
                }
                if self.is_builtin(text) {
                    let first = child(e, t.f_arguments).and_then(|a| self.inner(a));
                    return match (text, first) {
                        (b"new" | b"make", Some(ty)) => {
                            let id = self.type_name_of(ty);
                            // `new(expr)` points to a copy of the value
                            self.push_name(buf, id) || (text == b"new" && self.qual_path(ty, buf, depth + 1))
                        }
                        (b"append", Some(x)) => self.qual_path(x, buf, depth + 1),
                        _ => false,
                    };
                }
                buf.extend_from_slice(text);
                // A type of this file: a conversion, not a call.
                if self.top_flags(name) & F_TYPE == 0 {
                    buf.extend_from_slice(b"()");
                }
                true
            }
            K::SelectorExpression => {
                let (Some(op), Some(fl)) = (child(f, t.f_operand), child(f, t.f_field)) else { return false };
                let ftext = self.cx.text(fl);
                if ftext.is_empty() {
                    return false;
                }
                if self.k(op) == K::Identifier {
                    let oname = self.cx.name(op);
                    if !self.cx.is_local(oname) && self.alias_of(oname).is_some() {
                        // `pkg.Func()`: the function's result, in the package the name stands for
                        self.push_module(buf, oname);
                        buf.extend_from_slice(ftext);
                        buf.extend_from_slice(b"()");
                        return true;
                    }
                }
                // `x.Method()`: the method's declared result, looked up on the receiver's type
                if !self.qual_path(op, buf, depth + 1) {
                    return false;
                }
                buf.extend_from_slice(b"::");
                buf.extend_from_slice(ftext);
                buf.extend_from_slice(b"()");
                true
            }
            K::ParenthesizedExpression => match self.pointer_conversion(f) {
                Some((id, _, path)) => {
                    let id = self.path_qualified(path, id);
                    self.push_name(buf, id)
                }
                None => false,
            },
            _ => false,
        }
    }

    /// `$name`: the type of a non-local variable, looked up at query time.
    fn dollar(&mut self, name: &[u8]) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.push(b'$');
        buf.extend_from_slice(name);
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// Slot name `qual.field` (or `.field` when the owner type is unknown).
    fn slot(&mut self, qual: u32, field: &[u8]) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        if qual != NONE {
            buf.extend_from_slice(self.cx.name_bytes(qual));
        }
        buf.push(b'.');
        buf.extend_from_slice(field);
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    // ----- definitions: package, functions -----

    fn package_clause(&mut self, n: Node<'t>) {
        let Some(nm) = self.inner(n).filter(|x| x.end_byte() > x.start_byte()) else { return };
        let name = self.cx.name(nm);
        self.cx.def(name, DefKind::Module, n, n.end_byte());
    }

    fn function(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, is_method: bool) {
        let t = self.t;
        let Some(name_node) = child(n, t.f_name).filter(|x| x.end_byte() > x.start_byte()) else { return self.skip_names(c) };
        let body = child(n, t.f_body);
        let sig_end = body.map(|b| b.start_byte()).unwrap_or(n.end_byte());
        let ntext = self.cx.text(name_node);
        let name = self.cx.intern(ntext);
        let d = self.cx.def(name, if is_method { DefKind::Method } else { DefKind::Function }, n, sig_end);
        let mut flags = 0u8;
        if is_exported(ntext) {
            flags |= dflag::EXPORTED;
        }
        if body.is_none() {
            flags |= dflag::DECL;
        }
        if !is_method && self.is_test && is_test_name(ntext) {
            flags |= dflag::TEST;
        }
        self.cx.push_def(d);
        self.push_scope();
        self.fn_depth += 1;
        let saved = (std::mem::replace(&mut self.owner, NONE), std::mem::replace(&mut self.iface, NONE));
        let p0 = self.params.len();
        let (mut arity, mut ret) = ((0u32, false), NONE);
        each_child(c, |c| {
            let ch = c.node();
            match self.k(ch) {
                // the name
                K::Identifier | K::FieldIdentifier => {}
                K::ParameterList => {
                    if is_field(c, t.f_receiver) {
                        self.receiver(ch, d);
                    } else if is_field(c, t.f_parameters) {
                        arity = self.param_list(c, true);
                    } else {
                        ret = self.result_type(ch);
                        self.param_list(c, true);
                    }
                }
                K::TypeParameterList => self.type_parameters(ch, c),
                K::Block => {
                    // Parameters and the body's own declarations share one scope.
                    self.declare_collected(p0);
                    self.children(c);
                }
                _ => {
                    if is_field(c, t.f_result) {
                        ret = self.type_name_of(ch);
                    }
                    self.visit(c);
                }
            }
        });
        self.params.truncate(p0);
        (self.owner, self.iface) = saved;
        self.fn_depth -= 1;
        self.pop_scope();
        self.cx.pop_def();
        let n_params = arity.0.min(254) as u8;
        let dm = self.cx.def_mut(d);
        dm.flags |= flags;
        dm.arity_min = n_params;
        dm.arity_max = if arity.1 { 255 } else { n_params };
        dm.type_name = ret;
    }

    /// `(s *Stack[T])`: the receiver type names the method's container, its type arguments are
    /// the method's type parameters, and `s` is a local of that type.
    fn receiver(&mut self, list: Node<'t>, d: u32) {
        let t = self.t;
        let Some(pd) = self.inner(list) else { return };
        let Some(mut ty) = child(pd, t.f_type) else { return };
        for _ in 0..4 {
            match self.k(ty) {
                K::PointerType | K::ParenthesizedType => match self.inner(ty) {
                    Some(x) => ty = x,
                    None => return,
                },
                _ => break,
            }
        }
        let (base, targs) = if self.k(ty) == K::GenericType { (child(ty, t.f_type), child(ty, t.f_type_arguments)) } else { (Some(ty), None) };
        let mut qual = NONE;
        if let Some(b) = base {
            match self.k(b) {
                K::TypeIdentifier if b.end_byte() > b.start_byte() => {
                    qual = self.cx.name(b);
                    self.type_ref(b, RefKind::Type);
                }
                K::QualifiedType => {
                    qual = self.type_name_of(b);
                    self.qualified_type(b, RefKind::Type);
                }
                _ => {}
            }
        }
        self.cx.def_mut(d).qual = qual;
        if let Some(ta) = targs {
            for i in 0..ta.named_child_count().min(32) as u32 {
                let Some(p) = ta.named_child(i).and_then(|te| self.inner(te)) else { continue };
                if self.k(p) == K::TypeIdentifier && p.end_byte() > p.start_byte() {
                    let id = self.cx.name(p);
                    self.declare(id, NONE);
                }
            }
        }
        if let Some(nm) = child(pd, t.f_name) {
            let text = self.cx.text(nm);
            if !text.is_empty() && text != b"_" {
                let id = self.cx.intern(text);
                self.params.push((id, qual, false));
            }
        }
    }

    /// Declare the type parameters, then visit their constraints (which may mention each other).
    fn type_parameters(&mut self, list: Node<'t>, c: &mut TreeCursor<'t>) {
        for i in 0..list.named_child_count().min(64) as u32 {
            let Some(d) = list.named_child(i) else { break };
            if self.k(d) != K::TypeParameterDeclaration {
                continue;
            }
            for j in 0..d.named_child_count().min(64) as u32 {
                let Some(x) = d.named_child(j) else { break };
                if self.k(x) == K::Identifier && x.end_byte() > x.start_byte() {
                    let id = self.cx.name(x);
                    self.declare(id, NONE);
                }
            }
        }
        self.children(c);
    }

    /// Visit the types of a parameter list. With `collect`, parameter names and their types are
    /// queued in `params`: they become locals once the whole signature has been visited, because
    /// a parameter may be named like the type of a later one. Returns (count, variadic).
    fn param_list(&mut self, c: &mut TreeCursor<'t>, collect: bool) -> (u32, bool) {
        let t = self.t;
        let list = c.node();
        let (mut count, mut variadic) = (0u32, false);
        // Whether the list names its parameters: all of them or none, says the language.
        let mut named: Option<bool> = None;
        // First of the names the grammar took for types, waiting for the type of their group.
        let mut stray = usize::MAX;
        each_child(c, |c| {
            let p = c.node();
            let pk = self.k(p);
            if pk != K::ParameterDeclaration && pk != K::VariadicParameterDeclaration {
                return self.visit(c);
            }
            let ty = child(p, t.f_type);
            let has_name = child(p, t.f_name).is_some();
            if !has_name && pk == K::ParameterDeclaration {
                if let Some(x) = ty.filter(|x| self.k(*x) == K::TypeIdentifier && x.end_byte() > x.start_byte()) {
                    // With many names in one group (`a, b, c, ..., j string`) the grammar splits
                    // the first ones off as parameters that are only a type.
                    let named = *named.get_or_insert_with(|| {
                        (0..list.named_child_count().min(255) as u32).filter_map(|i| list.named_child(i)).any(|q| child(q, t.f_name).is_some())
                    });
                    if named {
                        count += 1;
                        let text = self.cx.text(x);
                        if collect && text != b"_" {
                            if stray == usize::MAX {
                                stray = self.params.len();
                            }
                            let id = self.cx.intern(text);
                            self.params.push((id, NONE, false));
                        }
                        return;
                    }
                }
            }
            let tname = match ty {
                Some(ty) if collect => self.type_name_of(ty),
                _ => NONE,
            };
            let chan = collect && pk == K::ParameterDeclaration && ty.is_some_and(|ty| self.is_chan_type(ty));
            if has_name && stray != usize::MAX {
                for q in &mut self.params[stray..] {
                    (q.1, q.2) = (tname, chan);
                }
                stray = usize::MAX;
            }
            let mut names = 0u32;
            each_child(c, |c| {
                let ch = c.node();
                if self.k(ch) != K::Identifier {
                    return self.visit(c);
                }
                names += 1;
                let text = self.cx.text(ch);
                if collect && !text.is_empty() && text != b"_" {
                    let id = self.cx.intern(text);
                    self.params.push((id, tname, chan));
                }
            });
            if pk == K::VariadicParameterDeclaration {
                variadic = true;
            } else {
                count += names.max(1);
            }
        });
        (count, variadic)
    }

    /// Declare the names collected in `params` from index `p0` on.
    fn declare_collected(&mut self, p0: usize) {
        for i in p0..self.params.len() {
            let (name, ty, chan) = self.params[i];
            if name != NONE {
                self.declare_as(name, ty, chan);
            }
        }
        self.params.truncate(p0);
    }

    fn func_literal(&mut self, c: &mut TreeCursor<'t>) {
        self.push_scope();
        self.fn_depth += 1;
        let saved = (std::mem::replace(&mut self.owner, NONE), std::mem::replace(&mut self.iface, NONE));
        let p0 = self.params.len();
        each_child(c, |c| match self.k(c.node()) {
            K::ParameterList => {
                self.param_list(c, true);
            }
            K::Block => {
                self.declare_collected(p0);
                self.children(c);
            }
            _ => self.visit(c),
        });
        self.params.truncate(p0);
        (self.owner, self.iface) = saved;
        self.fn_depth -= 1;
        self.pop_scope();
    }

    // ----- definitions: types -----

    fn type_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        // A lone spec spans the declaration, so that its signature reads `type T struct`.
        let mut grouped = false;
        each_child(c, |c| {
            let ch = c.node();
            if ch.kind_id() == t.s_lparen {
                grouped = true;
            } else if self.k(ch) == K::TypeSpec {
                self.type_spec(ch, c, if grouped { ch } else { n });
            } else {
                self.visit(c);
            }
        });
    }

    fn type_spec(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, def_node: Node<'t>) {
        let t = self.t;
        let (Some(nm), Some(ty)) = (child(n, t.f_name).filter(|x| x.end_byte() > x.start_byte()), child(n, t.f_type)) else {
            return self.children(c);
        };
        let tk = self.k(ty);
        let (kind, sig_end) = match tk {
            K::StructType => (DefKind::Struct, self.type_sig_end(ty)),
            K::InterfaceType => (DefKind::Interface, self.type_sig_end(ty)),
            _ => (DefKind::TypeAlias, n.end_byte()),
        };
        let ntext = self.cx.text(nm);
        let name = self.cx.intern(ntext);
        let d = self.cx.def(name, kind, def_node, sig_end);
        // A type declared inside a function is visible in that function only.
        let (parent, local) = if self.fn_depth > 0 { (self.cx.encl(), dflag::LOCAL) } else { (NONE, 0) };
        {
            let dm = self.cx.def_mut(d);
            dm.parent = parent;
            dm.flags |= local | if is_exported(ntext) { dflag::EXPORTED } else { 0 };
        }
        self.cx.push_def(d);
        // Scope of the type parameters.
        self.push_scope();
        let saved = (std::mem::replace(&mut self.owner, d), std::mem::replace(&mut self.iface, NONE));
        each_child(c, |c| {
            let ch = c.node();
            if ch == nm {
                // the declared name
            } else if ch == ty {
                match tk {
                    // struct_type -> field_declaration_list -> field_declaration
                    K::StructType => self.children(c),
                    K::InterfaceType => {
                        self.iface = d;
                        self.children(c);
                        self.iface = NONE;
                    }
                    _ => {
                        let tn = self.type_name_of(ty);
                        self.cx.def_mut(d).type_name = tn;
                        self.visit(c);
                    }
                }
            } else if self.k(ch) == K::TypeParameterList {
                self.type_parameters(ch, c);
            } else {
                self.visit(c);
            }
        });
        (self.owner, self.iface) = saved;
        self.pop_scope();
        self.cx.pop_def();
    }

    fn field_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let ty = child(n, t.f_type);
        let owner = self.owner;
        if owner == NONE {
            // A struct type inside a function body or a signature: only the field types matter.
            return each_child(c, |c| {
                if Some(c.node()) == ty {
                    self.visit(c);
                }
            });
        }
        let mut tname = ty.map(|x| self.type_name_of(x)).unwrap_or(NONE);
        let (mut flags, mut amin, mut amax) = (if self.fn_depth > 0 { dflag::LOCAL } else { 0 }, 0u8, 0u8);
        if let Some(ft) = ty.and_then(|x| self.func_type(x)) {
            (amin, amax, tname) = self.signature_info(ft);
            flags |= dflag::FNPTR;
        }
        // The tag is not part of the signature.
        let sig_end = ty.map(|x| self.type_sig_end(x)).unwrap_or(n.end_byte());
        let mut first = NONE;
        let mut named = false;
        each_child(c, |c| {
            let ch = c.node();
            if self.k(ch) == K::FieldIdentifier {
                named = true;
                let text = self.cx.text(ch);
                if !text.is_empty() && text != b"_" {
                    let name = self.cx.intern(text);
                    let di = self.cx.def(name, DefKind::Field, n, sig_end);
                    let dm = self.cx.def_mut(di);
                    dm.parent = owner;
                    dm.type_name = tname;
                    dm.flags |= flags | if is_exported(text) { dflag::EXPORTED } else { 0 };
                    dm.arity_min = amin;
                    dm.arity_max = amax;
                    if first == NONE {
                        first = di;
                    }
                }
            } else if Some(ch) == ty {
                if named {
                    // Fields of an anonymous struct type belong to the field that declares it.
                    let saved = std::mem::replace(&mut self.owner, first);
                    self.visit(c);
                    self.owner = saved;
                } else {
                    // Embedded field: named after its type (without the package), which the
                    // struct extends.
                    if tname != NONE {
                        let fname = self.simple_name(tname);
                        let di = self.cx.def(fname, DefKind::Field, n, sig_end);
                        let exported = is_exported(self.cx.name_bytes(fname));
                        let dm = self.cx.def_mut(di);
                        dm.parent = owner;
                        dm.type_name = tname;
                        dm.flags |= flags | if exported { dflag::EXPORTED } else { 0 };
                    }
                    self.named_type(ch, c, RefKind::Extends);
                }
            }
        });
    }

    /// A method of an interface. Inside a named interface it is a definition without body.
    fn method_elem(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let iface = self.iface;
        let mut d = NONE;
        if iface != NONE {
            if let Some(nm) = child(n, t.f_name).filter(|x| x.end_byte() > x.start_byte()) {
                let text = self.cx.text(nm);
                let name = self.cx.intern(text);
                d = self.cx.def(name, DefKind::Method, n, n.end_byte());
                let local = if self.fn_depth > 0 { dflag::LOCAL } else { 0 };
                let dm = self.cx.def_mut(d);
                dm.parent = iface;
                dm.flags |= dflag::DECL | dflag::ABSTRACT | local | if is_exported(text) { dflag::EXPORTED } else { 0 };
                self.cx.push_def(d);
            }
        }
        let saved = (std::mem::replace(&mut self.owner, NONE), std::mem::replace(&mut self.iface, NONE));
        let (mut arity, mut ret) = ((0u32, false), NONE);
        each_child(c, |c| {
            let ch = c.node();
            match self.k(ch) {
                K::FieldIdentifier => {}
                K::ParameterList => {
                    if is_field(c, t.f_parameters) {
                        arity = self.param_list(c, false);
                    } else {
                        ret = self.result_type(ch);
                        self.param_list(c, false);
                    }
                }
                _ => {
                    if is_field(c, t.f_result) {
                        ret = self.type_name_of(ch);
                    }
                    self.visit(c);
                }
            }
        });
        (self.owner, self.iface) = saved;
        if d != NONE {
            self.cx.pop_def();
            let n_params = arity.0.min(254) as u8;
            let dm = self.cx.def_mut(d);
            dm.arity_min = n_params;
            dm.arity_max = if arity.1 { 255 } else { n_params };
            dm.type_name = ret;
        }
    }

    /// An element of an interface body: a lone named type embeds that interface.
    fn type_elem(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let saved = std::mem::replace(&mut self.iface, NONE);
        let embedded = saved != NONE
            && n.named_child_count() == 1
            && self.inner(n).is_some_and(|x| matches!(self.k(x), K::TypeIdentifier | K::QualifiedType | K::GenericType));
        if embedded {
            each_child(c, |c| {
                let ch = c.node();
                if ch.is_named() {
                    self.named_type(ch, c, RefKind::Extends);
                }
            });
        } else {
            self.children(c);
        }
        self.iface = saved;
    }

    // ----- definitions: variables and constants -----

    fn var_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: DefKind) {
        let t = self.t;
        self.const_type = NONE;
        // A lone spec spans the declaration, so that its signature reads `var x T`.
        let mut grouped = false;
        each_child(c, |c| {
            let ch = c.node();
            match self.k(ch) {
                K::VarSpec | K::ConstSpec => self.var_spec(ch, c, if grouped { ch } else { n }, kind),
                K::VarSpecList => each_child(c, |c| {
                    let s = c.node();
                    if self.k(s) == K::VarSpec {
                        self.var_spec(s, c, s, kind);
                    } else {
                        self.visit(c);
                    }
                }),
                _ => {
                    grouped |= ch.kind_id() == t.s_lparen;
                    self.visit(c);
                }
            }
        });
    }

    fn var_spec(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, def_node: Node<'t>, kind: DefKind) {
        let t = self.t;
        let ty = child(n, t.f_type);
        let value = child(n, t.f_value);
        if self.fn_depth > 0 {
            return self.local_spec(c, ty, value);
        }
        let single = value.filter(|v| v.named_child_count() == 1).and_then(|v| v.named_child(0));
        let mut tname = ty.map(|x| self.type_name_of(x)).unwrap_or(NONE);
        let (mut flags, mut amin, mut amax) = (0u8, 0u8, 0u8);
        let callable = match (ty, single) {
            (Some(x), _) => self.func_type(x),
            (None, Some(v)) if self.k(v) == K::FuncLiteral => Some(v),
            _ => None,
        };
        if let Some(f) = callable {
            (amin, amax, tname) = self.signature_info(f);
            flags |= dflag::FNPTR;
        } else if let (None, Some(v)) = (ty, single) {
            tname = self.expr_type_name(v);
        }
        if kind == DefKind::Constant {
            // `B` after `A Kind = iota` repeats the type and the expression.
            if ty.is_none() && value.is_none() {
                tname = self.const_type;
            } else {
                self.const_type = tname;
            }
        }
        let sig_end = if kind == DefKind::Constant {
            // A constant is its value: keep it in the signature, up to the end of the line.
            let text = self.cx.text(n);
            n.start_byte() + memchr::memchr(b'\n', text).unwrap_or(text.len()).min(200)
        } else {
            // Stop before the `=`: at the declared type, or else at the last name.
            match ty {
                Some(x) => self.type_sig_end(x),
                None => {
                    let names = (0..n.named_child_count().min(64) as u32)
                        .map_while(|i| n.named_child(i))
                        .take_while(|x| matches!(self.k(*x), K::Identifier | K::Comment));
                    names.filter(|x| self.k(*x) == K::Identifier).last().map(|x| x.end_byte()).unwrap_or(n.end_byte())
                }
            }
        };
        let implements = if kind == DefKind::Variable { self.asserted_impl(n, ty, single) } else { NONE };
        let mut first = NONE;
        each_child(c, |c| {
            let ch = c.node();
            if self.k(ch) == K::Identifier {
                let text = self.cx.text(ch);
                if !text.is_empty() && text != b"_" {
                    let name = self.cx.intern(text);
                    let di = self.cx.def(name, kind, def_node, sig_end);
                    let dm = self.cx.def_mut(di);
                    dm.type_name = tname;
                    dm.flags |= flags | if is_exported(text) { dflag::EXPORTED } else { 0 };
                    dm.arity_min = amin;
                    dm.arity_max = amax;
                    if first == NONE {
                        first = di;
                        self.cx.push_def(di);
                    }
                }
                return;
            }
            // Fields of an anonymous struct type belong to the variable.
            let saved = std::mem::replace(&mut self.owner, first);
            if Some(ch) == ty && implements != NONE {
                let at = self.cx.facts.refs.len();
                self.named_type(ch, c, RefKind::Implements);
                if self.cx.facts.refs.get(at).is_some_and(|r| r.kind == RefKind::Implements) {
                    self.impls.push((at as u32, implements));
                }
            } else {
                self.visit(c);
            }
            self.owner = saved;
        });
        if first != NONE {
            self.cx.pop_def();
        }
    }

    /// `var _ Iface = (*Impl)(nil)` declares that `Impl`, a type of this file, implements `Iface`.
    /// Returns the name of `Impl`, or `NONE` when the spec is not such an assertion.
    fn asserted_impl(&mut self, spec: Node<'t>, ty: Option<Node<'t>>, value: Option<Node<'t>>) -> u32 {
        let (Some(ty), Some(v)) = (ty, value) else { return NONE };
        let blank = spec.named_child(0).is_some_and(|x| self.k(x) == K::Identifier && self.cx.text(x) == b"_");
        let lone = spec.named_child(1).is_none_or(|x| self.k(x) != K::Identifier);
        if !blank || !lone || !matches!(self.k(ty), K::TypeIdentifier | K::QualifiedType | K::GenericType) {
            return NONE;
        }
        let id = self.expr_type_name(v);
        if id != NONE && self.top_flags(id) & F_TYPE != 0 { id } else { NONE }
    }

    /// `var` / `const` inside a function: the names are locals, typed from the declared type or
    /// from the initialiser. References on the right are visited before the names exist.
    fn local_spec(&mut self, c: &mut TreeCursor<'t>, ty: Option<Node<'t>>, value: Option<Node<'t>>) {
        let p0 = self.params.len();
        each_child(c, |c| {
            let ch = c.node();
            if self.k(ch) != K::Identifier {
                return self.visit(c);
            }
            // A blank keeps its place: values pair up with names by position.
            let text = self.cx.text(ch);
            let id = if text.is_empty() || text == b"_" { NONE } else { self.cx.intern(text) };
            self.params.push((id, NONE, false));
        });
        let n = self.params.len() - p0;
        let declared = ty.map(|x| (self.local_type_of(x), self.is_chan_type(x)));
        for i in 0..n {
            let tp = match declared {
                Some(d) => d,
                None => self.infer(value, n, i),
            };
            (self.params[p0 + i].1, self.params[p0 + i].2) = tp;
        }
        self.declare_collected(p0);
    }

    /// Type path of the `i`-th of `n` names initialised from the expression list `values`:
    /// pairwise for `a, b := x, y`, the first result for `a, err := f()`, `v, ok := m[k]`,
    /// `v, ok := x.(T)` and `v, ok := <-ch`.
    fn infer(&mut self, values: Option<Node<'t>>, n: usize, i: usize) -> (u32, bool) {
        let Some(v) = values else { return (NONE, false) };
        let nv = v.named_child_count();
        let e = if nv == n {
            // A comment inside the list would shift the pairing.
            if n > 1 && (0..nv.min(16) as u32).any(|j| v.named_child(j).is_some_and(|x| self.k(x) == K::Comment)) {
                return (NONE, false);
            }
            v.named_child(i as u32)
        } else if nv == 1 && i == 0 {
            v.named_child(0)
        } else {
            // `a, b, err := f()`: a later name is the result at its place. The resolver reads
            // the type off the function's signature.
            return match v.named_child(0).filter(|e| nv == 1 && n > 1 && self.k(*e) == K::CallExpression) {
                Some(e) => (self.suffixed(e, format!("#{i}").as_bytes()), false),
                None => (NONE, false),
            };
        };
        match e {
            // Only a value that stands alone is the channel itself: `v, ok := <-ch` is not.
            Some(e) if self.k(e) != K::Comment => match self.anon_of_expr(e) {
                Some(anon) => (anon, false),
                None => (self.qual_of(e), nv == n && self.is_chan_expr(e)),
            },
            _ => (NONE, false),
        }
    }

    /// The type path of a call with a pick after it (`f()#1`, `f()<-`), or `NONE` when the
    /// call has no path that ends in the call itself (a conversion, `make`, `new`).
    fn suffixed(&mut self, call: Node<'t>, suffix: &[u8]) -> u32 {
        let q = self.qual_of(call);
        if q == NONE || !self.cx.name_bytes(q).ends_with(b"()") {
            return NONE;
        }
        let mut buf = self.cx.name_bytes(q).to_vec();
        buf.extend_from_slice(suffix);
        self.cx.intern(&buf)
    }

    // ----- statements that declare locals -----

    /// `a, b := x, y`
    fn short_var_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let left = child(n, t.f_left);
        let right = child(n, t.f_right);
        // The right-hand side is evaluated before the new names exist: `path := path.Join(a, b)`.
        each_child(c, |c| {
            if Some(c.node()) != left {
                self.visit(c);
            }
        });
        let Some(left) = left else { return };
        let n = left.named_child_count();
        let mut types = [(NONE, false); 8];
        for (i, slot) in types.iter_mut().enumerate().take(n) {
            *slot = self.infer(right, n, i);
        }
        for i in 0..n.min(64) {
            let Some(l) = left.named_child(i as u32) else { break };
            let text = self.cx.text(l);
            if self.k(l) != K::Identifier || text.is_empty() || text == b"_" {
                continue;
            }
            let id = self.cx.intern(text);
            // A name the innermost scope already has is assigned, not declared: it keeps its type.
            if self.declared_here(id) && self.cx.local_type(id).is_some_and(|ty| ty != NONE) {
                continue;
            }
            let (ty, chan) = if i < types.len() { types[i] } else { (NONE, false) };
            self.declare_as(id, ty, chan);
        }
    }

    /// `k, v := range xs` (the second name is an element of `xs`) and, in a `select`,
    /// `v, ok := <-ch` (the first is). With `=` instead of `:=` the names are assigned.
    fn range_or_receive(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, elem_at: usize) {
        let t = self.t;
        let left = child(n, t.f_left);
        let right = child(n, t.f_right);
        let define = (0..n.child_count().min(4)).any(|i| n.child(i).is_some_and(|x| x.kind_id() == t.s_define));
        if let (Some(l), false) = (left, define) {
            self.write_targets(l, None, false);
        }
        each_child(c, |c| {
            if Some(c.node()) != left {
                self.visit(c);
            } else if !define {
                self.lhs_depth += 1;
                self.visit(c);
                self.lhs_depth -= 1;
            }
        });
        if let (Some(l), true) = (left, define) {
            let mut elem = match right {
                Some(r) => match self.anon_of_expr(r) {
                    Some(anon) => anon,
                    None => self.qual_of(r),
                },
                None => NONE,
            };
            // Ranging over a channel yields its elements as the first (and only) value.
            let mut elem_at = if elem_at == 1 && elem != NONE && right.is_some_and(|r| self.is_chan_expr(r)) { 0 } else { elem_at };
            // One name over the result of a call: an element if the call returns a channel or
            // an iterator, an index or a key otherwise. Which it is stands in the function's
            // signature, so the path says "what it yields" and the resolver looks.
            if elem_at == 1 && l.named_child_count() == 1 {
                if let Some(r) = right.filter(|r| self.k(*r) == K::CallExpression) {
                    let yielded = self.suffixed(r, b"<-");
                    if yielded != NONE {
                        elem = yielded;
                        elem_at = 0;
                    }
                }
            }
            for i in 0..l.named_child_count().min(4) {
                let Some(x) = l.named_child(i as u32) else { break };
                let text = self.cx.text(x);
                if self.k(x) == K::Identifier && !text.is_empty() && text != b"_" {
                    let id = self.cx.intern(text);
                    self.declare(id, if i == elem_at { elem } else { NONE });
                }
            }
        }
    }

    /// `switch v := x.(type)`: in a case with a single type, `v` has that type.
    fn type_switch(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        self.push_scope();
        let mut alias = NONE;
        let mut subject = NONE;
        each_child(c, |c| {
            let ch = c.node();
            if is_field(c, t.f_alias) {
                if let Some(x) = self.inner(ch).filter(|x| self.k(*x) == K::Identifier) {
                    let text = self.cx.text(x);
                    if !text.is_empty() && text != b"_" {
                        alias = self.cx.intern(text);
                    }
                }
            } else if is_field(c, t.f_value) {
                subject = self.qual_of(ch);
                self.visit(c);
            } else if matches!(self.k(ch), K::TypeCase | K::Scoped) {
                // `case T:` or `default:`
                self.push_scope();
                if alias != NONE {
                    let ty = self.case_type(ch).unwrap_or(subject);
                    self.declare(alias, ty);
                }
                self.children(c);
                self.pop_scope();
            } else {
                self.visit(c);
            }
        });
        self.pop_scope();
    }

    /// The type a type-switch binding has in a case: the case's only type. With several types,
    /// `nil` or `default` it keeps the type of the switched value (`None`).
    fn case_type(&mut self, case: Node<'t>) -> Option<u32> {
        if self.k(case) != K::TypeCase {
            return None;
        }
        let mut only = None;
        for i in 0..case.named_child_count().min(32) as u32 {
            let x = case.named_child(i)?;
            // comments and the statement list
            if matches!(self.k(x), K::Comment | K::Other) {
                continue;
            }
            if only.is_some() {
                return None;
            }
            only = Some(x);
        }
        let x = only.filter(|x| self.cx.text(*x) != b"nil")?;
        Some(self.type_name_of(x))
    }

    // ----- assignments -----

    fn lvalue_target(&self, mut l: Node<'t>) -> Node<'t> {
        let t = self.t;
        for _ in 0..8 {
            let next = match self.k(l) {
                K::ParenthesizedExpression => self.inner(l),
                K::IndexExpression => child(l, t.f_operand),
                K::UnaryExpression if child(l, t.f_operator).map(|o| o.kind_id()) == Some(t.s_star) => child(l, t.f_operand),
                _ => None,
            };
            match next {
                Some(x) => l = x,
                None => break,
            }
        }
        l
    }

    /// Record the assignment targets of the expression list `left`, and bind the values stored
    /// into members (`x.f = handler`) when `bind` is set and the lists pair up.
    fn write_targets(&mut self, left: Node<'t>, right: Option<Node<'t>>, bind: bool) {
        let t = self.t;
        let ln = left.named_child_count();
        let pairs = bind && right.is_some_and(|r| r.named_child_count() == ln);
        for i in 0..ln.min(32) as u32 {
            let Some(l) = left.named_child(i) else { break };
            let target = self.lvalue_target(l);
            match self.k(target) {
                K::Identifier => {
                    self.cx.s.writes.insert(target.start_byte() as u32);
                }
                K::SelectorExpression => {
                    let (Some(op), Some(fl)) = (child(target, t.f_operand), child(target, t.f_field)) else { continue };
                    self.cx.s.writes.insert(fl.start_byte() as u32);
                    if !pairs {
                        continue;
                    }
                    let Some(v) = right.and_then(|r| r.named_child(i)) else { continue };
                    // `pkg.Var = f` writes a variable, not a member.
                    if self.k(op) == K::Identifier {
                        let oid = self.cx.name(op);
                        if !self.cx.is_local(oid) && self.alias_of(oid).is_some() {
                            continue;
                        }
                    }
                    let q = self.qual_of(op);
                    self.bind(v, q, self.cx.text(fl));
                }
                _ => {}
            }
        }
    }

    /// Bind value `v` into slot `qual.field` when it names something that can be looked up: a
    /// non-local identifier, `pkg.Name`, or a method value `x.method` whose method this file
    /// declares. A plain identifier is thereby accounted for; a qualified one keeps its own
    /// reference, which carries the import path or the receiver type.
    fn bind(&mut self, v: Node<'t>, qual: u32, field: &[u8]) {
        let t = self.t;
        let (name, at) = match self.k(v) {
            K::Identifier => {
                let text = self.cx.text(v);
                if text.is_empty() || self.is_builtin(text) {
                    return;
                }
                let id = self.cx.intern(text);
                if self.cx.is_local(id) {
                    return;
                }
                self.cx.mark(v);
                self.any_marks = true;
                (id, v)
            }
            K::SelectorExpression => {
                let (Some(op), Some(fl)) = (child(v, t.f_operand), child(v, t.f_field)) else { return };
                if fl.end_byte() == fl.start_byte() {
                    return;
                }
                let fid = self.cx.name(fl);
                let package = self.k(op) == K::Identifier && {
                    let oid = self.cx.name(op);
                    !self.cx.is_local(oid) && self.alias_of(oid).is_some()
                };
                if !package && self.top_flags(fid) & F_METHOD == 0 {
                    return;
                }
                (fid, fl)
            }
            _ => return,
        };
        let slot = self.slot(qual, field);
        self.cx.add_ref(name, RefKind::Bind, line_of(at), slot, 255);
    }

    fn assignment(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let left = child(n, t.f_left);
        if let Some(l) = left {
            let plain = child(n, t.f_operator).is_none_or(|o| o.kind_id() == t.s_assign);
            self.write_targets(l, child(n, t.f_right), plain);
        }
        each_child(c, |c| {
            if Some(c.node()) == left {
                self.lhs_depth += 1;
                self.visit(c);
                self.lhs_depth -= 1;
            } else {
                self.visit(c);
            }
        });
    }

    /// `x++` / `x--`
    fn inc_statement(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        if let Some(e) = self.inner(n) {
            let target = self.lvalue_target(e);
            let written = match self.k(target) {
                K::Identifier => Some(target),
                K::SelectorExpression => child(target, self.t.f_field),
                _ => None,
            };
            if let Some(x) = written {
                self.cx.s.writes.insert(x.start_byte() as u32);
            }
        }
        self.lhs_depth += 1;
        self.children(c);
        self.lhs_depth -= 1;
    }

    // ----- expressions -----

    fn member_ref(&mut self, name: u32, usage: Use, written: bool, line: u32, qual: u32) {
        let kind = match usage {
            Use::Call => RefKind::MemberCall,
            Use::Read if written => RefKind::MemberWrite,
            Use::Read => RefKind::MemberRead,
        };
        self.cx.add_ref(name, kind, line, qual, 255);
    }

    /// `operand.field`. Emits the reference for `field` first and returns its index (for a call,
    /// the caller fills in the argument count), or `NONE` when nothing was emitted.
    fn selector(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, usage: Use) -> u32 {
        let t = self.t;
        let (Some(op), Some(fl)) = (child(n, t.f_operand), child(n, t.f_field)) else {
            self.children(c);
            return NONE;
        };
        let ftext = self.cx.text(fl);
        if ftext.is_empty() {
            self.children(c);
            return NONE;
        }
        let fname = self.cx.intern(ftext);
        let line = line_of(fl);
        let written = self.lhs_depth > 0 && self.cx.s.writes.contains(&(fl.start_byte() as u32));
        let ri = self.cx.facts.refs.len() as u32;
        if self.k(op) != K::Identifier {
            // `(*T).Method`, `(*pkg.T).Method`: a method expression. Only a name this file
            // declares as a variable makes it a dereference instead, and so does a write.
            if self.k(op) == K::ParenthesizedExpression && !written {
                if let Some((tname, at, path)) = self.pointer_conversion(op) {
                    if path != NONE || self.top_flags(tname) & (F_TYPE | F_VALUE) != F_VALUE {
                        let kind = if usage == Use::Call { RefKind::Call } else { RefKind::Read };
                        let q = self.path_qualified(path, tname);
                        self.cx.add_ref(fname, kind, line, q, 255);
                        self.cx.add_ref(tname, RefKind::Type, line_of(at), path, 255);
                        self.recv_arg = usage == Use::Call;
                        return ri;
                    }
                }
            }
            let q = self.qual_of(op);
            self.member_ref(fname, usage, written, line, q);
            // the operand; the field identifier is a leaf that is skipped
            self.children(c);
            return ri;
        }
        let otext = self.cx.text(op);
        let oname = self.cx.intern(otext);
        if let Some(ty) = self.cx.local_type(oname) {
            self.member_ref(fname, usage, written, line, named(ty));
            return ri;
        }
        if let Some(path) = self.alias_of(oname) {
            // `pkg.Name`: a name of another package, qualified by its import path.
            let kind = match usage {
                Use::Call => RefKind::Call,
                Use::Read if written => RefKind::Write,
                Use::Read => RefKind::Read,
            };
            self.cx.add_ref(fname, kind, line, path, 255);
            return ri;
        }
        if otext.is_empty() || self.is_builtin(otext) {
            self.member_ref(fname, usage, written, line, NONE);
            return ri;
        }
        if self.top_flags(oname) & F_TYPE != 0 {
            // `T.Method`: a method expression of a type of this file.
            let kind = if usage == Use::Call { RefKind::Call } else { RefKind::Read };
            self.cx.add_ref(fname, kind, line, oname, 255);
            self.cx.add_ref(oname, RefKind::Type, line_of(op), NONE, 255);
            self.recv_arg = usage == Use::Call;
            return ri;
        }
        // A package-level variable (of this package: other packages need a qualifier).
        let q = self.dollar(otext);
        self.member_ref(fname, usage, written, line, q);
        self.cx.add_ref(oname, RefKind::Read, line_of(op), NONE, 255);
        ri
    }

    fn call_expression(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(f) = child(n, t.f_function) else { return self.children(c) };
        // `new(T)` instantiates T.
        let is_new = self.k(f) == K::Identifier && self.cx.text(f) == b"new" && self.is_builtin(b"new");
        // The call reference: its argument count is filled in once the arguments have been seen.
        let mut ri = NONE;
        let mut nargs = 255u8;
        let mut recv_arg = false;
        each_child(c, |c| {
            if c.node() == f {
                self.recv_arg = false;
                ri = self.callee(c);
                recv_arg = std::mem::take(&mut self.recv_arg);
            } else if is_field(c, t.f_arguments) {
                nargs = self.arguments(c, is_new);
            } else {
                self.visit(c);
            }
        });
        if recv_arg && nargs != 255 {
            nargs = nargs.saturating_sub(1);
        }
        if let Some(r) = self.cx.facts.refs.get_mut(ri as usize).filter(|r| r.kind.is_call()) {
            r.nargs = nargs;
        }
    }

    /// The callee of a call, cursor on it. Emits the call reference, visits whatever else the
    /// callee expression contains, and returns the index of the reference (`NONE` if none).
    fn callee(&mut self, c: &mut TreeCursor<'t>) -> u32 {
        let t = self.t;
        let f = c.node();
        match self.k(f) {
            K::Identifier => {
                // `f(x)`: a call, or a conversion when `f` is a type of this file.
                let text = self.cx.text(f);
                if text.is_empty() || self.is_builtin(text) {
                    return NONE;
                }
                let name = self.cx.intern(text);
                if self.cx.is_local(name) {
                    return NONE;
                }
                if self.top_flags(name) & F_TYPE != 0 {
                    self.cx.add_ref(name, RefKind::Type, line_of(f), NONE, 255);
                    return NONE;
                }
                let ri = self.cx.facts.refs.len() as u32;
                self.cx.add_ref(name, RefKind::Call, line_of(f), NONE, 255);
                ri
            }
            K::SelectorExpression => self.selector(f, c, Use::Call),
            K::ParenthesizedExpression | K::IndexExpression if self.cx.depth < MAX_DEPTH => {
                // `(*T)(x)` converts to a pointer type.
                if let Some((name, at, path)) = self.pointer_conversion(f) {
                    self.cx.add_ref(name, RefKind::Type, line_of(at), path, 255);
                    return NONE;
                }
                // `(f)(x)` calls `f`. `f[T](x)` instantiates a generic function and `s.fs[i](x)`
                // calls an element of a member: either way the operand is what is called.
                let index = self.k(f) == K::IndexExpression;
                let mut ri = NONE;
                self.cx.depth += 1;
                each_child(c, |c| {
                    let callable = matches!(self.k(c.node()), K::Identifier | K::SelectorExpression | K::ParenthesizedExpression);
                    if callable && (!index || is_field(c, t.f_operand)) {
                        ri = self.callee(c);
                    } else {
                        self.visit(c);
                    }
                });
                self.cx.depth -= 1;
                ri
            }
            _ => {
                self.visit(c);
                NONE
            }
        }
    }

    /// Visit an argument list and count the arguments. A single argument that is itself a call
    /// may spread several results (`f(g())`): the count is then unknown.
    fn arguments(&mut self, c: &mut TreeCursor<'t>, is_new: bool) -> u8 {
        let mut count = 0u32;
        let mut spreads = false;
        each_child(c, |c| {
            let a = c.node();
            let ak = self.k(a);
            if ak == K::Comment || (ak == K::Other && !a.is_named()) {
                return;
            }
            if count == 0 {
                spreads = ak == K::CallExpression && !self.single_valued(a);
            }
            if is_new && count == 0 {
                self.new_operand(a, c);
            } else {
                self.visit(c);
            }
            count += 1;
        });
        if count == 1 && spreads { 255 } else { count.min(254) as u8 }
    }

    /// A call known to yield exactly one value: a builtin, or a conversion to a predeclared type
    /// or to a type of this file.
    fn single_valued(&mut self, call: Node<'t>) -> bool {
        let Some(f) = child(call, self.t.f_function).filter(|f| self.k(*f) == K::Identifier) else { return false };
        let text = self.cx.text(f);
        if self.is_builtin(text) {
            return true;
        }
        let id = self.cx.intern(text);
        self.top_flags(id) & F_TYPE != 0 && !self.cx.is_local(id)
    }

    /// The operand of `new(...)`. Since Go 1.26 it may be a value, and a plain or qualified name
    /// reads the same either way: what this file declares decides, and a name it does not
    /// declare is recorded as a type use, which a type and a constant both accept.
    fn new_operand(&mut self, a: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        match self.k(a) {
            K::TypeIdentifier => {
                let text = self.cx.text(a);
                if text.is_empty() || self.is_builtin(text) {
                    return;
                }
                let name = self.cx.intern(text);
                if self.cx.is_local(name) {
                    return;
                }
                let fl = self.top_flags(name);
                let kind = if fl & F_TYPE != 0 {
                    RefKind::New
                } else if fl & F_VALUE != 0 {
                    RefKind::Read
                } else {
                    RefKind::Type
                };
                self.cx.add_ref(name, kind, line_of(a), NONE, 255);
            }
            K::QualifiedType => {
                let (Some(p), Some(nm)) = (child(a, t.f_package), child(a, t.f_name)) else { return };
                if nm.end_byte() == nm.start_byte() || p.end_byte() == p.start_byte() {
                    return;
                }
                let pid = self.cx.name(p);
                if self.cx.is_local(pid) || self.alias_of(pid).is_some() {
                    // `new(x.field)` reads a member; `new(pkg.Name)` is a type use
                    return self.qualified_type(a, RefKind::Type);
                }
                let name = self.cx.name(nm);
                let q = self.dollar(self.cx.text(p));
                self.cx.add_ref(name, RefKind::MemberRead, line_of(nm), q, 255);
                self.cx.add_ref(pid, RefKind::Read, line_of(p), NONE, 255);
            }
            K::GenericType => self.named_type(a, c, RefKind::New),
            _ => self.visit(c),
        }
    }

    /// `T(x)` where the parser saw a type. With a generic type it is ambiguous between a
    /// conversion (`Set[string](x)`), a generic call (`Map[int, string](xs)`) and a call through
    /// an indexed value (`fs[i](x)`): the base decides.
    fn type_conversion(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(ty) = child(n, t.f_type).filter(|ty| self.k(*ty) == K::GenericType) else { return self.children(c) };
        let Some(base) = child(ty, t.f_type) else { return self.children(c) };
        // The "type arguments" of a value are index expressions.
        let mut values = false;
        match self.k(base) {
            K::TypeIdentifier => {
                let text = self.cx.text(base);
                if !text.is_empty() && !self.is_builtin(text) {
                    let name = self.cx.intern(text);
                    if self.cx.is_local(name) {
                        values = true;
                    } else if self.top_flags(name) & F_TYPE != 0 {
                        self.cx.add_ref(name, RefKind::Type, line_of(base), NONE, 255);
                    } else {
                        self.cx.add_ref(name, RefKind::Call, line_of(base), NONE, 1);
                    }
                }
            }
            K::QualifiedType => {
                let (Some(p), Some(nm)) = (child(base, t.f_package), child(base, t.f_name)) else { return self.children(c) };
                if nm.end_byte() == nm.start_byte() || p.end_byte() == p.start_byte() {
                    return self.children(c);
                }
                let pid = self.cx.name(p);
                let name = self.cx.name(nm);
                if let Some(lt) = self.cx.local_type(pid) {
                    // `x.fs[i](v)`
                    values = true;
                    self.cx.add_ref(name, RefKind::MemberCall, line_of(nm), named(lt), 1);
                } else if let Some(path) = self.alias_of(pid) {
                    self.cx.add_ref(name, RefKind::Call, line_of(nm), path, 1);
                } else {
                    values = true;
                    let q = self.dollar(self.cx.text(p));
                    self.cx.add_ref(name, RefKind::MemberCall, line_of(nm), q, 1);
                    self.cx.add_ref(pid, RefKind::Read, line_of(p), NONE, 255);
                }
            }
            _ => return self.children(c),
        }
        each_child(c, |c| {
            if c.node() != ty {
                return self.visit(c);
            }
            // generic_type: the base is done, the arguments remain
            each_child(c, |c| {
                if c.node() == base {
                } else if values {
                    self.index_values(c);
                } else {
                    self.visit(c);
                }
            });
        });
    }

    /// Identifiers the parser took for type arguments but which index a value: plain reads.
    fn index_values(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        if self.k(n) == K::TypeIdentifier {
            return self.identifier(n);
        }
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        each_child(c, |c| self.index_values(c));
        self.cx.depth -= 1;
    }

    /// `f[int, string]` as a value: a generic function, not a type.
    fn type_instantiation(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        each_child(c, |c| {
            let ch = c.node();
            if is_field(c, t.f_type) && self.k(ch) == K::TypeIdentifier {
                self.identifier(ch);
            } else {
                self.visit(c);
            }
        });
    }

    // ----- composite literals -----

    fn composite_literal(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let ty = child(n, self.t.f_type);
        each_child(c, |c| {
            let ch = c.node();
            if Some(ch) == ty {
                // `T{...}` instantiates T; `[]T{...}` and `map[K]V{...}` only mention types.
                self.named_type(ch, c, RefKind::New);
            } else if self.k(ch) == K::LiteralValue {
                self.literal_value(c, ty.map(Elem::Type).unwrap_or(Elem::Unknown));
            } else {
                self.visit(c);
            }
        });
    }

    fn lit_mode(&mut self, of: Elem<'t>) -> Lit<'t> {
        let t = self.t;
        let mut ty = match of {
            Elem::Unknown => return Lit::Unknown,
            Elem::Fields(q) => return Lit::Fields(q),
            Elem::Type(ty) => ty,
        };
        let elem = |x: Option<Node<'t>>| x.map(Elem::Type).unwrap_or(Elem::Unknown);
        for _ in 0..4 {
            match self.k(ty) {
                K::PointerType | K::ParenthesizedType => match self.inner(ty) {
                    Some(x) => ty = x,
                    None => return Lit::Unknown,
                },
                K::TypeIdentifier | K::QualifiedType | K::GenericType => {
                    let q = self.type_name_of(ty);
                    // A map / slice type of this file: its keys are values, and its elements are
                    // what the name resolves to through the type's definition.
                    if q != NONE && self.k(ty) != K::QualifiedType && self.top_flags(q) & F_ELEMS != 0 {
                        return Lit::Elems(Elem::Fields(q));
                    }
                    return Lit::Fields(q);
                }
                K::SliceType | K::ArrayType => return Lit::Elems(elem(child(ty, t.f_element))),
                K::MapType => return Lit::Map(elem(child(ty, t.f_key)), elem(child(ty, t.f_value))),
                K::StructType => return Lit::Anon,
                _ => return Lit::Unknown,
            }
        }
        Lit::Unknown
    }

    /// The body of a composite literal of type `of`. Cursor on the literal_value.
    fn literal_value(&mut self, c: &mut TreeCursor<'t>, of: Elem<'t>) {
        let mode = self.lit_mode(of);
        // Type of an element written without its type.
        let elem = match mode {
            Lit::Elems(e) => e,
            Lit::Map(_, v) => v,
            // A named slice or map type declared elsewhere: its elements have the type the name
            // resolves to.
            Lit::Fields(q) => Elem::Fields(q),
            Lit::Anon | Lit::Unknown => Elem::Unknown,
        };
        each_child(c, |c| match self.k(c.node()) {
            K::KeyedElement => self.keyed_element(c, mode, elem),
            K::LiteralElement => self.literal_element(c, elem),
            _ => self.visit(c),
        });
    }

    /// An element: an expression, or a nested literal body of type `nested`.
    fn literal_element(&mut self, c: &mut TreeCursor<'t>, nested: Elem<'t>) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        each_child(c, |c| {
            if self.k(c.node()) == K::LiteralValue {
                self.literal_value(c, nested);
            } else {
                self.visit(c);
            }
        });
        self.cx.depth -= 1;
    }

    /// `key: value`. In a struct literal the key is a field: a member write, and a binding when
    /// the value names a function or another value that can be looked up.
    fn keyed_element(&mut self, c: &mut TreeCursor<'t>, mode: Lit<'t>, elem: Elem<'t>) {
        let t = self.t;
        let n = c.node();
        let key = child(n, t.f_key);
        let value = child(n, t.f_value);
        let field = key.and_then(|k| self.inner(k)).filter(|x| self.k(*x) == K::Identifier && x.end_byte() > x.start_byte());
        // Owner of the field a key names: a type path, or `NONE` when the literal's type is unknown.
        let owner = match mode {
            Lit::Fields(q) => Some(q),
            Lit::Unknown => Some(NONE),
            _ => None,
        };
        let key_is_field = match (field, owner) {
            (Some(f), Some(q)) => {
                let ftext = self.cx.text(f);
                let name = self.cx.intern(ftext);
                self.cx.add_ref(name, RefKind::MemberWrite, line_of(f), q, 255);
                if let Some(v) = value.and_then(|v| self.inner(v)) {
                    self.bind(v, q, ftext);
                }
                true
            }
            (Some(_), None) => matches!(mode, Lit::Anon),
            _ => false,
        };
        let key_type = match mode {
            Lit::Map(k, _) => k,
            _ => Elem::Unknown,
        };
        each_child(c, |c| {
            let ch = c.node();
            if Some(ch) == key {
                if !key_is_field {
                    self.literal_element(c, key_type);
                }
            } else if Some(ch) == value {
                self.literal_element(c, elem);
            } else {
                self.visit(c);
            }
        });
    }

    // ----- wrap-up -----

    /// Attach each `var _ Iface = (*Impl)(nil)` assertion to the definition of `Impl`.
    fn finish(&mut self) {
        for i in 0..self.impls.len() {
            let (ri, ty) = self.impls[i];
            let def = self.cx.facts.defs.iter().position(|d| d.name == ty && d.parent == NONE && d.kind.is_type() && d.flags & dflag::LOCAL == 0);
            let r = &mut self.cx.facts.refs[ri as usize];
            match def {
                Some(d) => r.encl = d as u32,
                None => r.kind = RefKind::Type,
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Go 1.26 `new(expr)`
// ---------------------------------------------------------------------------------------------
//
// Since Go 1.26 `new` takes an expression (`new(int32(80))`, `new("x")`, `new(f())`). The grammar
// still insists on a type there, and its error recovery is not local: string contents surface as
// identifiers and the following statements are swallowed. Such a file is parsed again with the
// `new` of every call that cannot be `new(Type)` spelled as an ordinary identifier of the same
// length, which keeps every byte offset; node text is always read from the original source.

thread_local! {
    static REPARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

/// Local types at and above this are anonymous struct types of the function being read
/// (`W::anon`), not names. They never leave the extractor.
const ANON: u32 = 0xF000_0000;

fn is_anon(ty: u32) -> bool {
    (ANON..NONE).contains(&ty)
}

/// A local's type as a qualifier: the name it has, or none for an anonymous struct.
fn named(ty: u32) -> u32 {
    if is_anon(ty) { NONE } else { ty }
}

fn reparse_new_expr(src: &[u8], old: &Tree) -> Option<Tree> {
    // The patched source and the old tree with the patches applied as edits: everything the
    // edits do not touch is reused, so the second parse costs a fraction of the first.
    let mut patched: Option<(Vec<u8>, Tree)> = None;
    let (mut row, mut line_start, mut scanned) = (0usize, 0usize, 0usize);
    for i in memchr::memmem::find_iter(src, b"new(") {
        if i > 0 && (is_word_byte(src[i - 1]) || src[i - 1] == b'.') {
            continue;
        }
        if !new_takes_expression(&src[i + 4..]) {
            continue;
        }
        let at = i + 2;
        for nl in memchr::memchr_iter(b'\n', &src[scanned..at]) {
            row += 1;
            line_start = scanned + nl + 1;
        }
        scanned = at;
        let (buf, tree) = patched.get_or_insert_with(|| (src.to_vec(), old.clone()));
        buf[at] = b'W';
        let (start, end) = (Point::new(row, at - line_start), Point::new(row, at + 1 - line_start));
        tree.edit(&InputEdit {
            start_byte: at,
            old_end_byte: at + 1,
            new_end_byte: at + 1,
            start_position: start,
            old_end_position: end,
            new_end_position: end,
        });
    }
    let (patched, edited) = patched?;
    let tree = REPARSER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let mut p = Parser::new();
            p.set_language(&tree_sitter_go::LANGUAGE.into()).ok()?;
            *slot = Some(p);
        }
        slot.as_mut()?.parse(&patched, Some(&edited))
    })?;
    let better = !tree.root_node().has_error() || error_count(tree.root_node()) < error_count(old.root_node());
    better.then_some(tree)
}

/// Whether the text after `new(` starts an argument that can only be an expression. `T`, `pkg.T`,
/// `*T`, `[]T`, `map[K]V` and the like are left alone: as types they parse.
fn new_takes_expression(s: &[u8]) -> bool {
    let skip_ws = |mut i: usize| {
        while s.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        i
    };
    let mut i = skip_ws(0);
    // pointer, array and slice prefixes of a type
    loop {
        match s.get(i) {
            Some(b'*') => i += 1,
            Some(b'[') => {
                let mut depth = 0u32;
                loop {
                    match s.get(i) {
                        Some(b'[') => depth += 1,
                        Some(b']') => depth -= 1,
                        Some(b'\n') | None => return false,
                        _ => {}
                    }
                    i += 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            _ => break,
        }
        i = skip_ws(i);
    }
    // a dotted name
    let start = i;
    let mut first_end = i;
    let mut segments = 0;
    loop {
        let j = i;
        while s.get(i).is_some_and(|&b| is_word_byte(b)) {
            i += 1;
        }
        if i == j {
            break;
        }
        if s[j].is_ascii_digit() {
            return true;
        }
        segments += 1;
        if segments == 1 {
            first_end = i;
        }
        if s.get(i) == Some(&b'.') && s.get(i + 1).is_some_and(|&b| is_word_byte(b)) {
            i += 1;
        } else {
            break;
        }
    }
    if segments == 0 {
        // A literal, an operator or a parenthesis; `<-chan T` is a type.
        return !matches!(s.get(i), Some(b'<') | Some(b')') | None);
    }
    if segments == 1 && matches!(&s[start..first_end], b"func" | b"struct" | b"interface" | b"chan" | b"map") {
        return false;
    }
    match s.get(skip_ws(i)) {
        // `T` and `pkg.T` parse as types whatever they are; `a.b.c` can only be a value
        Some(b')') => segments > 2,
        None => false,
        // `f(x)`, `T{...}`, `x + y`, `xs[i]`
        _ => true,
    }
}

/// Number of ERROR and MISSING nodes of a tree. Only subtrees that contain one are entered.
fn error_count(root: Node) -> usize {
    let mut count = 0;
    let mut c = root.walk();
    loop {
        let n = c.node();
        if n.is_error() || n.is_missing() {
            count += 1;
        }
        if n.has_error() && c.goto_first_child() {
            continue;
        }
        loop {
            if c.goto_next_sibling() {
                break;
            }
            if !c.goto_parent() {
                return count;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------------------------------

#[inline]
fn is_word_byte(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric() || b >= 0x80
}

/// Predeclared identifiers (builtin functions, types, values) and the blank identifier: they
/// never produce references.
fn is_predeclared(w: &[u8]) -> bool {
    match w.len() {
        1 => w == b"_",
        3 => matches!(w, b"cap" | b"len" | b"max" | b"min" | b"new" | b"int" | b"any" | b"nil"),
        4 => matches!(w, b"copy" | b"imag" | b"make" | b"real" | b"bool" | b"byte" | b"int8" | b"rune" | b"uint" | b"true" | b"iota"),
        5 => matches!(w, b"clear" | b"close" | b"panic" | b"print" | b"error" | b"int16" | b"int32" | b"int64" | b"uint8" | b"false"),
        6 => matches!(w, b"append" | b"delete" | b"string" | b"uint16" | b"uint32" | b"uint64"),
        7 => matches!(w, b"complex" | b"println" | b"recover" | b"float32" | b"float64" | b"uintptr"),
        9 => w == b"complex64",
        10 => matches!(w, b"complex128" | b"comparable"),
        _ => false,
    }
}

fn is_keyword(w: &[u8]) -> bool {
    matches!(
        w,
        b"break"
            | b"default"
            | b"func"
            | b"interface"
            | b"select"
            | b"case"
            | b"defer"
            | b"go"
            | b"map"
            | b"struct"
            | b"chan"
            | b"else"
            | b"goto"
            | b"package"
            | b"switch"
            | b"const"
            | b"fallthrough"
            | b"if"
            | b"range"
            | b"type"
            | b"continue"
            | b"for"
            | b"import"
            | b"return"
            | b"var"
    )
}

fn is_ident(w: &[u8]) -> bool {
    w.first().is_some_and(|b| !b.is_ascii_digit()) && w.iter().all(|&b| is_word_byte(b))
}

/// `v2`, `v22`: a major-version path element.
fn is_version(w: &[u8]) -> bool {
    w.len() > 1 && w[0] == b'v' && w[1..].iter().all(|b| b.is_ascii_digit())
}

/// Standard-library import paths have no dot in their first element (`net/http`, `C`).
fn is_std_import(p: &[u8]) -> bool {
    let first = p.split(|&b| b == b'/').next().unwrap_or(p);
    !first.contains(&b'.')
}

/// Exported names start with an upper-case letter.
fn is_exported(name: &[u8]) -> bool {
    match name.first() {
        Some(b) if b.is_ascii() => b.is_ascii_uppercase(),
        Some(_) => std::str::from_utf8(name).ok().and_then(|s| s.chars().next()).is_some_and(|ch| ch.is_uppercase()),
        None => false,
    }
}

/// `go test` runs `TestXxx`, `BenchmarkXxx`, `FuzzXxx` and `ExampleXxx`, where `Xxx` does not
/// start with a lower-case letter.
fn is_test_name(name: &[u8]) -> bool {
    for prefix in [&b"Test"[..], b"Benchmark", b"Fuzz", b"Example"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return rest.first().is_none_or(|b| !b.is_ascii_lowercase());
        }
    }
    false
}
