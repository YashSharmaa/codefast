//! C# extractor (tree-sitter-c-sharp).
//!
//! C# is close to Java: every name that is not a local is a member of some type, reached
//! through an explicit receiver (`obj.F()`, `Type.F()`, `this.f`) or through the implicit `this`
//! and the lexically enclosing types (`F()`, `count`). Unlike Java, a file is not a class, the
//! namespace a file declares says nothing about its directory, and types, namespaces,
//! properties and methods are all written in PascalCase, so the spelling of a name tells little.
//! The walker therefore tracks:
//!   * locals with their declared types (parameters, local variables, `foreach`, `using`,
//!     `catch`, pattern, `out var`, lambda, tuple and LINQ range variables), so that `x.Foo()`
//!     carries the receiver type,
//!   * the members each enclosing type declares in this file (collected when its body is
//!     entered, because members may be used before they are declared), so that a bare `count`
//!     or `svc.Go()` is attributed to the type that declares `count` / `svc`,
//!   * the type parameters in scope, which stand for their constraint and never produce
//!     references,
//!   * the namespaces the file knows of (its own, its `using` directives and their prefixes) and
//!     the types it declares, which tell a namespace prefix (`System.IO.File`) from an outer
//!     type (`JsonReader.State`).
//!
//! Conventions the resolver can rely on:
//!   * Definitions. A namespace is a `Namespace` named with its full dotted name (`A.B` for
//!     `namespace B` inside `namespace A`); a file-scoped namespace spans the rest of the file.
//!     Types and namespaces have the namespace around them as parent. Classes and records are
//!     `Class`, structs and record structs `Struct`, delegates `Class` with `FNPTR`. Constructors
//!     are methods named after their type, destructors `~T`, operators `operator+`, conversions
//!     `operator T`, indexers properties named `this`. Positional record parameters are
//!     properties of the record. Events are fields (or properties, with accessors) with `FNPTR`.
//!     `public`, written or implied (interface members, enum members), is `EXPORTED`; `private`,
//!     written or implied for members of classes and structs, is `LOCAL` unless a type around
//!     the member is `partial` (its other parts live in other files). An extension method keeps
//!     its `this` parameter in the signature and leaves it out of the arity. Test methods carry
//!     `[Test]`, `[Fact]`, `[Theory]`, `[TestMethod]` or `[TestCase]`.
//!   * Qualifiers. `F()` is a `Call` qualified by the type that declares `F` in this file, else
//!     by the innermost enclosing type `C` (the method may be inherited, belong to an outer
//!     type, or come in through `using static`). A call of a local function is unqualified.
//!     `this.F()` is a `MemberCall C`, `base.F()` a `MemberCall ^C`, `x.F()` a `MemberCall T` for
//!     a local of type `T`, `m.F()` a `MemberCall D::m` where `D` declares the member `m` in this
//!     file, else `C::m`. `Type.F()` is a `Call Type`. Invoking a delegate field or event
//!     (`_cb()`, `Changed?.Invoke(..)`) is a `MemberCall` of the field on its owner.
//!   * Receiver type paths follow the Java extractor: a call result is the segment `::m()` on
//!     the path of its receiver, a member access the segment `::m`, and a `var` is typed by the
//!     path of its initializer. A method declared to return `Task<T>` or `ValueTask<T>` is
//!     recorded with the result type `T`: the result is nearly always awaited.
//!   * Type names. A type written after a namespace keeps it as a module qualifier:
//!     `System.IO.Stream` is `System.IO|Stream`; one written after an outer type is the path
//!     `Outer::Inner`. A `using A = N.T;` alias stands for `N|T`. The reference of a qualified
//!     type names its last segment and is qualified by the namespace (`N`) or the simple name of
//!     the outer type. `string` and `object` are `String` and `Object`.
//!   * Imports. `using N;` is target `N`, wildcard; `using static N.T;` target `N.T`, wildcard,
//!     with an `Import` reference of `T` qualified by `N`; `using A = N.T;` target `N`, name `T`,
//!     alias `A`. `global using` adds `REEXPORT`: the directive holds for every file of the
//!     project. A directive naming `System` or a namespace below it is `SYSTEM`.
//!   * Callbacks. A method group handed to a call (`list.ForEach(Print)`) is an `Arg` of that
//!     call, one stored into a field, property or event (`Changed += OnChanged`,
//!     `cb = this.Run`) a `Bind` into the slot `Owner.member`, both next to the `MemberRead` of
//!     the method. Attributes are `Annotation` references named after their class
//!     (`[Serializable]` is `SerializableAttribute`).

use crate::extract::*;
use crate::facts::*;
use crate::lang::Lang;
use std::marker::PhantomData;
use std::sync::OnceLock;
use tree_sitter::{Node, Tree, TreeCursor};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    /// Anonymous tokens: never entered.
    Token,
    /// Named nodes without special handling: recurse into the children.
    Other,
    /// Named nodes that cannot contain references.
    Skip,
    Comment,
    Error,
    CompilationUnit,
    Identifier,
    GenericName,
    QualifiedName,
    AliasQualifiedName,
    PredefinedType,
    ImplicitType,
    ArrayType,
    WrapperType,
    TupleType,
    TypeArgumentList,
    TypeParameterList,
    ConstraintsClause,
    UsingDirective,
    NamespaceDecl,
    FileNamespaceDecl,
    DeclarationList,
    ClassDecl,
    StructDecl,
    RecordDecl,
    InterfaceDecl,
    EnumDecl,
    DelegateDecl,
    EnumMemberList,
    EnumMember,
    BaseList,
    PrimaryCtorBase,
    FieldDecl,
    EventFieldDecl,
    EventDecl,
    PropertyDecl,
    IndexerDecl,
    AccessorList,
    Accessor,
    MethodDecl,
    CtorDecl,
    DtorDecl,
    OperatorDecl,
    ConversionDecl,
    LocalFunction,
    CtorInit,
    Modifier,
    AttributeList,
    Attribute,
    AttributeArgList,
    AttributeArg,
    ParameterList,
    BracketedParameterList,
    Parameter,
    ImplicitParameter,
    VariableDeclaration,
    VariableDeclarator,
    Block,
    Scoped,
    Foreach,
    CatchDecl,
    LabeledStatement,
    Goto,
    DeclarationPattern,
    RecursivePattern,
    VarPattern,
    DeclarationExpression,
    Designation,
    TuplePattern,
    ConstantPattern,
    ListPattern,
    Subpattern,
    PropertyPatternClause,
    Invocation,
    MemberAccess,
    ConditionalAccess,
    MemberBinding,
    ElementAccess,
    ObjectCreation,
    ImplicitObjectCreation,
    AnonymousObjectCreation,
    InitializerExpression,
    WithExpression,
    WithInitializer,
    Assignment,
    BinaryExpression,
    PrefixUnary,
    PostfixUnary,
    Lambda,
    AnonymousMethod,
    CastExpression,
    AsExpression,
    IsExpression,
    Parenthesized,
    Await,
    ArgumentList,
    Argument,
    TupleExpression,
    StringLiteral,
    Interpolated,
    QueryExpression,
    FromClause,
    JoinClause,
    JoinInto,
    LetClause,
    Preproc,
    ExplicitIface,
}

struct Tables {
    kinds: KindTable<K>,
    f_name: u16,
    f_body: u16,
    f_type: u16,
    f_returns: u16,
    f_parameters: u16,
    f_value: u16,
    f_expression: u16,
    f_function: u16,
    f_arguments: u16,
    f_condition: u16,
    f_left: u16,
    f_right: u16,
    f_operator: u16,
    f_qualifier: u16,
    f_accessors: u16,
    f_initializer: u16,
    f_alias: u16,
    kw_this: u16,
    kw_base: u16,
    kw_global: u16,
    kw_static: u16,
    kw_struct: u16,
    op_assign: u16,
    op_add_assign: u16,
    op_sub_assign: u16,
    op_coalesce_assign: u16,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let lang: tree_sitter::Language = tree_sitter_c_sharp::LANGUAGE.into();
        let special: &[(&str, K)] = &[
            ("ERROR", K::Error),
            ("compilation_unit", K::CompilationUnit),
            ("identifier", K::Identifier),
            ("generic_name", K::GenericName),
            ("qualified_name", K::QualifiedName),
            ("alias_qualified_name", K::AliasQualifiedName),
            ("predefined_type", K::PredefinedType),
            ("implicit_type", K::ImplicitType),
            ("array_type", K::ArrayType),
            ("nullable_type", K::WrapperType),
            ("pointer_type", K::WrapperType),
            ("ref_type", K::WrapperType),
            ("scoped_type", K::WrapperType),
            ("tuple_type", K::TupleType),
            ("type_argument_list", K::TypeArgumentList),
            ("type_parameter_list", K::TypeParameterList),
            ("type_parameter_constraints_clause", K::ConstraintsClause),
            ("using_directive", K::UsingDirective),
            ("namespace_declaration", K::NamespaceDecl),
            ("file_scoped_namespace_declaration", K::FileNamespaceDecl),
            ("declaration_list", K::DeclarationList),
            ("class_declaration", K::ClassDecl),
            ("struct_declaration", K::StructDecl),
            ("record_declaration", K::RecordDecl),
            ("interface_declaration", K::InterfaceDecl),
            ("enum_declaration", K::EnumDecl),
            ("delegate_declaration", K::DelegateDecl),
            ("enum_member_declaration_list", K::EnumMemberList),
            ("enum_member_declaration", K::EnumMember),
            ("base_list", K::BaseList),
            ("primary_constructor_base_type", K::PrimaryCtorBase),
            ("field_declaration", K::FieldDecl),
            ("event_field_declaration", K::EventFieldDecl),
            ("event_declaration", K::EventDecl),
            ("property_declaration", K::PropertyDecl),
            ("indexer_declaration", K::IndexerDecl),
            ("accessor_list", K::AccessorList),
            ("accessor_declaration", K::Accessor),
            ("method_declaration", K::MethodDecl),
            ("constructor_declaration", K::CtorDecl),
            ("destructor_declaration", K::DtorDecl),
            ("operator_declaration", K::OperatorDecl),
            ("conversion_operator_declaration", K::ConversionDecl),
            ("local_function_statement", K::LocalFunction),
            ("constructor_initializer", K::CtorInit),
            ("modifier", K::Modifier),
            ("attribute_list", K::AttributeList),
            ("attribute", K::Attribute),
            ("attribute_argument_list", K::AttributeArgList),
            ("attribute_argument", K::AttributeArg),
            ("parameter_list", K::ParameterList),
            ("bracketed_parameter_list", K::BracketedParameterList),
            ("parameter", K::Parameter),
            ("implicit_parameter", K::ImplicitParameter),
            ("variable_declaration", K::VariableDeclaration),
            ("variable_declarator", K::VariableDeclarator),
            ("block", K::Block),
            ("for_statement", K::Scoped),
            ("using_statement", K::Scoped),
            ("fixed_statement", K::Scoped),
            ("switch_section", K::Scoped),
            ("switch_expression_arm", K::Scoped),
            ("catch_clause", K::Scoped),
            ("foreach_statement", K::Foreach),
            ("catch_declaration", K::CatchDecl),
            ("labeled_statement", K::LabeledStatement),
            ("goto_statement", K::Goto),
            ("declaration_pattern", K::DeclarationPattern),
            ("recursive_pattern", K::RecursivePattern),
            ("var_pattern", K::VarPattern),
            ("declaration_expression", K::DeclarationExpression),
            ("parenthesized_variable_designation", K::Designation),
            ("tuple_pattern", K::TuplePattern),
            ("constant_pattern", K::ConstantPattern),
            ("list_pattern", K::ListPattern),
            ("subpattern", K::Subpattern),
            ("property_pattern_clause", K::PropertyPatternClause),
            ("invocation_expression", K::Invocation),
            ("member_access_expression", K::MemberAccess),
            ("conditional_access_expression", K::ConditionalAccess),
            ("member_binding_expression", K::MemberBinding),
            ("element_access_expression", K::ElementAccess),
            ("object_creation_expression", K::ObjectCreation),
            ("implicit_object_creation_expression", K::ImplicitObjectCreation),
            ("anonymous_object_creation_expression", K::AnonymousObjectCreation),
            ("initializer_expression", K::InitializerExpression),
            ("with_expression", K::WithExpression),
            ("with_initializer", K::WithInitializer),
            ("assignment_expression", K::Assignment),
            ("binary_expression", K::BinaryExpression),
            ("prefix_unary_expression", K::PrefixUnary),
            ("postfix_unary_expression", K::PostfixUnary),
            ("lambda_expression", K::Lambda),
            ("anonymous_method_expression", K::AnonymousMethod),
            ("cast_expression", K::CastExpression),
            ("as_expression", K::AsExpression),
            ("is_expression", K::IsExpression),
            ("parenthesized_expression", K::Parenthesized),
            ("await_expression", K::Await),
            ("argument_list", K::ArgumentList),
            ("bracketed_argument_list", K::ArgumentList),
            ("argument", K::Argument),
            ("tuple_expression", K::TupleExpression),
            ("string_literal", K::StringLiteral),
            ("verbatim_string_literal", K::StringLiteral),
            ("raw_string_literal", K::StringLiteral),
            ("interpolated_string_expression", K::Interpolated),
            ("query_expression", K::QueryExpression),
            ("from_clause", K::FromClause),
            ("join_clause", K::JoinClause),
            ("join_into_clause", K::JoinInto),
            ("let_clause", K::LetClause),
            ("preproc_if", K::Preproc),
            ("preproc_elif", K::Preproc),
            ("preproc_if_in_attribute_list", K::Preproc),
            ("explicit_interface_specifier", K::ExplicitIface),
            ("comment", K::Comment),
            // leaves and subtrees that never contain references
            ("character_literal", K::Skip),
            ("integer_literal", K::Skip),
            ("real_literal", K::Skip),
            ("boolean_literal", K::Skip),
            ("null_literal", K::Skip),
            ("discard", K::Skip),
            ("break_statement", K::Skip),
            ("continue_statement", K::Skip),
            ("empty_statement", K::Skip),
            ("extern_alias_directive", K::Skip),
            ("attribute_target_specifier", K::Skip),
            ("interpolation_format_clause", K::Skip),
            ("string_content", K::Skip),
            ("shebang_directive", K::Skip),
            ("preproc_define", K::Skip),
            ("preproc_undef", K::Skip),
            ("preproc_region", K::Skip),
            ("preproc_endregion", K::Skip),
            ("preproc_pragma", K::Skip),
            ("preproc_nullable", K::Skip),
            ("preproc_line", K::Skip),
            ("preproc_error", K::Skip),
            ("preproc_warning", K::Skip),
            ("type_parameter", K::Skip),
            ("constructor_constraint", K::Skip),
        ];
        // Every other named kind recurses into its children; anonymous tokens keep the default
        // and are never entered.
        let mut pairs: Vec<(&str, K)> = special.to_vec();
        for id in 0..lang.node_kind_count() {
            let id = id as u16;
            if !lang.node_kind_is_named(id) {
                continue;
            }
            if let Some(name) = lang.node_kind_for_id(id)
                && !pairs.iter().any(|(s, _)| *s == name)
            {
                pairs.push((name, K::Other));
            }
        }
        let kinds = KindTable::build(&lang, K::Token, &pairs);
        Tables {
            kinds,
            f_name: field(&lang, "name"),
            f_body: field(&lang, "body"),
            f_type: field(&lang, "type"),
            f_returns: field(&lang, "returns"),
            f_parameters: field(&lang, "parameters"),
            f_value: field(&lang, "value"),
            f_expression: field(&lang, "expression"),
            f_function: field(&lang, "function"),
            f_arguments: field(&lang, "arguments"),
            f_condition: field(&lang, "condition"),
            f_left: field(&lang, "left"),
            f_right: field(&lang, "right"),
            f_operator: field(&lang, "operator"),
            f_qualifier: field(&lang, "qualifier"),
            f_accessors: field(&lang, "accessors"),
            f_initializer: field(&lang, "initializer"),
            f_alias: field(&lang, "alias"),
            kw_this: lang.id_for_node_kind("this", false),
            kw_base: lang.id_for_node_kind("base", false),
            kw_global: lang.id_for_node_kind("global", false),
            kw_static: lang.id_for_node_kind("static", false),
            kw_struct: lang.id_for_node_kind("struct", false),
            op_assign: lang.id_for_node_kind("=", false),
            op_add_assign: lang.id_for_node_kind("+=", false),
            op_sub_assign: lang.id_for_node_kind("-=", false),
            op_coalesce_assign: lang.id_for_node_kind("??=", false),
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Scope entries
// ---------------------------------------------------------------------------------------------
//
// The value stored per name in a scope packs what the name denotes:
//   bit 31      set for a member of an enclosing type declared in this file, clear for a local
//   bits 25-30  for members: index of the declaring type in `W::classes`
//   bit 24      for members: the member is a method (a call of it is a call, a read of it a
//               method group)
//   bits 22-23  number of array dimensions of the declared type (saturating at 3)
//   bits 0-21   name id of the declared type path + 1, or 0 when unknown / primitive

const B_MEMBER: u32 = 1 << 31;
const B_LEVEL_SHIFT: u32 = 25;
const B_MAX_LEVEL: usize = 62;
const B_METHOD: u32 = 1 << 24;
const B_DIMS_SHIFT: u32 = 22;
const B_TY_MASK: u32 = (1 << B_DIMS_SHIFT) - 1;

#[inline]
fn enc_local(ty: u32, dims: u8) -> u32 {
    let t = if ty >= B_TY_MASK - 1 { 0 } else { ty + 1 };
    ((dims.min(3) as u32) << B_DIMS_SHIFT) | t
}

#[inline]
fn enc_member(level: usize, ty: u32, dims: u8, method: bool) -> u32 {
    B_MEMBER | ((level.min(B_MAX_LEVEL) as u32) << B_LEVEL_SHIFT) | if method { B_METHOD } else { 0 } | enc_local(ty, dims)
}

#[inline]
fn dec_level(v: u32) -> usize {
    ((v >> B_LEVEL_SHIFT) & 63) as usize
}

#[inline]
fn dec_dims(v: u32) -> u8 {
    ((v >> B_DIMS_SHIFT) & 3) as u8
}

#[inline]
fn dec_ty(v: u32) -> u32 {
    let t = v & B_TY_MASK;
    if t == 0 { NONE } else { t - 1 }
}

#[inline]
fn is_member(v: u32) -> bool {
    v & B_MEMBER != 0
}

#[inline]
fn is_method(v: u32) -> bool {
    v & (B_MEMBER | B_METHOD) == B_MEMBER | B_METHOD
}

// ---------------------------------------------------------------------------------------------
// Walker state
// ---------------------------------------------------------------------------------------------

/// Members of the type are implicitly public (interface).
const CF_IFACE: u8 = 1;
/// The type, or one around it, is `partial`: its private members are visible in other files.
const CF_PARTIAL: u8 = 2;
/// Members without a written access modifier are private (class, struct, record).
const CF_PRIVATE_DEFAULT: u8 = 4;

#[derive(Clone, Copy)]
struct Class {
    /// The type's own simple name: the qualifier of unqualified calls and of `this`.
    own: u32,
    /// Simple name of the base class (the first entry of the base list when it is a class),
    /// for `base(..)`.
    sup: u32,
    /// `^own`, interned on first use.
    sup_q: u32,
    flags: u8,
}

const M_PUBLIC: u16 = 1;
const M_PRIVATE: u16 = 2;
const M_ACCESS: u16 = 4;
const M_ABSTRACT: u16 = 8;
const M_STATIC: u16 = 16;
const M_ASYNC: u16 = 32;
const M_EXTERN: u16 = 64;
const M_PARTIAL: u16 = 128;
const M_CONST: u16 = 256;
const M_TEST: u16 = 512;

/// What the modifiers of a declaration say, and where its signature starts once leading
/// attributes are skipped.
#[derive(Clone, Copy)]
struct Mods {
    bits: u16,
    sig_start: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MK {
    Method,
    Ctor,
    Dtor,
    Operator,
    Conversion,
    Local,
    Delegate,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RK {
    /// A value whose static type path is `base` (+ `::fld`), `NONE` when unknown.
    Val,
    /// `this`.
    This,
    /// A type: `base` is its type path.
    Type,
    /// A namespace prefix: `base` is the dotted name so far.
    Pkg,
    Super,
}

/// What a receiver expression denotes.
#[derive(Clone, Copy)]
struct Recv {
    k: RK,
    base: u32,
    /// Member still to be appended to `base` (the path is only built when it is needed).
    fld: u32,
    /// The pending member is a method: the segment is `::fld()`, the declared result of a call.
    call: bool,
    dims: u8,
}

impl Recv {
    const UNKNOWN: Recv = Recv { k: RK::Val, base: NONE, fld: NONE, call: false, dims: 0 };
    const SUPER: Recv = Recv { k: RK::Super, base: NONE, fld: NONE, call: false, dims: 0 };

    #[inline]
    fn val(ty: u32, dims: u8) -> Recv {
        Recv { k: RK::Val, base: ty, fld: NONE, call: false, dims }
    }

    #[inline]
    fn ty(path: u32) -> Recv {
        Recv { k: RK::Type, base: path, fld: NONE, call: false, dims: 0 }
    }

    #[inline]
    fn member(base: u32, fld: u32, call: bool) -> Recv {
        if base == NONE { Recv::UNKNOWN } else { Recv { k: RK::Val, base, fld, call, dims: 0 } }
    }
}

/// `using A = N.T;`: the alias, the type path it stands for (`N|T`), and the namespace and
/// simple name of that type.
#[derive(Clone, Copy)]
struct Alias {
    alias: u32,
    path: u32,
    module: u32,
    name: u32,
}

struct W<'a, 't> {
    cx: Ctx<'a>,
    t: &'static Tables,
    /// Enclosing type bodies, outermost first.
    classes: Vec<Class>,
    /// Type parameters in scope with the type path of their first constraint (`NONE` when
    /// unconstrained).
    tparams: Vec<(u32, u32)>,
    /// Names of the types this file declares, at any depth, and whether each is an interface.
    types: Vec<(u32, bool)>,
    /// Namespaces this file names: its own, those of its `using` directives, `System`. A
    /// prefix of one of them is a namespace too.
    namespaces: Vec<u32>,
    aliases: Vec<Alias>,
    /// Full names of the enclosing namespaces, innermost last.
    ns_stack: Vec<u32>,
    /// Local functions visible from the current block.
    local_fns: Vec<u32>,
    /// Receiver of the innermost `a?.b`: what a `.b` binding applies to.
    binding: Recv,
    /// Declared type that a target-typed `new()` creates, while an initializer of a variable,
    /// field or property of that type is walked.
    target_ty: u32,
    /// Type whose members an object initializer (`new T { A = 1 }`) or a property pattern
    /// (`is T { A: 1 }`) names.
    init_ty: u32,
    string_id: u32,
    _t: PhantomData<&'t ()>,
}

pub fn extract(_lang: Lang, src: &[u8], tree: &Tree, path: &str, facts: &mut FileFacts, scratch: &mut Scratch) {
    // `#if` inside a statement or a declaration (`#if X else if (a) {..} #endif else {..}`) is
    // beyond the grammar, and the error it causes can swallow the rest of a type. Such files are
    // parsed again without the directives.
    let alt = if tree.root_node().has_error() && memchr::memmem::find(src, b"#if").is_some() { reparse_conditionals(src, tree) } else { None };
    walk(src, alt.as_ref().unwrap_or(tree), path, facts, scratch);
}

fn walk(src: &[u8], tree: &Tree, path: &str, facts: &mut FileFacts, scratch: &mut Scratch) {
    let cx = Ctx::new(src, path, facts, scratch);
    let mut w = W {
        cx,
        t: tables(),
        classes: Vec::new(),
        tparams: Vec::new(),
        types: Vec::new(),
        namespaces: Vec::new(),
        aliases: Vec::new(),
        ns_stack: Vec::new(),
        local_fns: Vec::new(),
        binding: Recv::UNKNOWN,
        target_ty: NONE,
        init_ty: NONE,
        string_id: NONE,
        _t: PhantomData,
    };
    let mut c = tree.walk();
    w.visit(&mut c);
}

impl<'a, 't> W<'a, 't> {
    #[inline]
    fn k(&self, n: Node<'t>) -> K {
        self.t.kinds.get(n.kind_id())
    }

    /// Call `f` with the cursor on each child of the cursor's node, respecting `MAX_DEPTH`.
    #[inline]
    fn each(&mut self, c: &mut TreeCursor<'t>, mut f: impl FnMut(&mut Self, &mut TreeCursor<'t>)) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        if c.goto_first_child() {
            loop {
                f(self, c);
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.cx.depth -= 1;
    }

    fn children(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| w.visit(c));
    }

    /// Visit every child except tokens, comments and the node starting at byte `skip` (the
    /// declared name, which is not a reference).
    fn children_except(&mut self, c: &mut TreeCursor<'t>, skip: usize) {
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if ch.start_byte() == skip => {}
                _ => w.visit(c),
            }
        });
    }

    fn scoped(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        self.children(c);
        self.cx.pop_scope();
    }

    /// Visit the node under the cursor. A node attached as `type` or `returns` is a type
    /// wherever it occurs (casts, `typeof`, `default`, results, lambda results).
    fn visit(&mut self, c: &mut TreeCursor<'t>) {
        if is_field(c, self.t.f_type) || is_field(c, self.t.f_returns) {
            return self.type_ref(c, RefKind::Type, 255);
        }
        self.visit_node(c);
    }

    fn visit_node(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        match self.k(n) {
            K::Token | K::Skip | K::Comment | K::StringLiteral | K::Modifier | K::PredefinedType | K::ImplicitType => {}
            K::Identifier => self.identifier(n, false),
            K::GenericName => {
                self.generic_value(c);
            }
            K::QualifiedName | K::AliasQualifiedName | K::ArrayType | K::WrapperType | K::TupleType => self.type_ref(c, RefKind::Type, 255),
            K::TypeArgumentList => self.type_args(c),
            K::Invocation => {
                self.invocation(n, c);
            }
            K::MemberAccess => {
                self.member_access(c, false);
            }
            K::ConditionalAccess => {
                self.conditional_access(c);
            }
            K::MemberBinding => {
                self.member_binding(n, false);
            }
            K::ElementAccess => {
                self.recv(c);
            }
            K::ObjectCreation => {
                self.object_creation(n, c);
            }
            K::ImplicitObjectCreation => {
                self.implicit_creation(n, c);
            }
            K::AnonymousObjectCreation => self.anonymous_object(c),
            K::InitializerExpression => self.initializer_expression(c),
            K::WithExpression => self.with_expression(c),
            K::Assignment => self.assignment(n, c),
            K::BinaryExpression => self.binary(c),
            K::PrefixUnary | K::PostfixUnary => self.unary(n, c),
            K::Lambda | K::AnonymousMethod => self.lambda(c),
            K::AsExpression | K::IsExpression => self.as_is(c),
            K::ConstantPattern => self.constant_pattern(c),
            K::DeclarationPattern | K::DeclarationExpression => self.declaration_pattern(n, c),
            K::RecursivePattern => self.recursive_pattern(n, c),
            K::VarPattern | K::Designation | K::TuplePattern | K::ListPattern => self.designation(c),
            K::Subpattern => self.subpattern(c),
            K::VariableDeclaration => self.variable_declaration(n, c),
            K::VariableDeclarator => self.declarator(c, NONE, 0),
            K::Block => self.block(c),
            K::Scoped => self.scoped(c),
            K::Foreach => self.foreach(n, c),
            K::CatchDecl => self.catch_decl(n, c),
            K::LabeledStatement => self.labeled(c),
            K::Goto => {}
            K::QueryExpression => self.query(c),
            K::FromClause => self.query_from(n, c),
            K::JoinClause | K::LetClause | K::JoinInto => self.range_clause(c),
            K::Argument => {
                self.argument(c, NONE);
            }
            K::ArgumentList => {
                self.arguments(c, NONE);
            }
            K::AttributeList => self.attribute_list(c),
            K::Attribute => self.attribute(n, c),
            K::Preproc => self.preproc(c),
            K::UsingDirective => self.using(n),
            K::NamespaceDecl => self.namespace(n, c),
            K::FileNamespaceDecl => {}
            K::CompilationUnit => self.compilation_unit(n, c),
            K::ClassDecl => self.type_decl(n, c, DefKind::Class, 0),
            K::StructDecl => self.type_decl(n, c, DefKind::Struct, 0),
            K::RecordDecl => {
                let structural = (0..n.child_count().min(16)).any(|i| n.child(i).is_some_and(|x| x.kind_id() == self.t.kw_struct));
                self.type_decl(n, c, if structural { DefKind::Struct } else { DefKind::Class }, 0)
            }
            K::InterfaceDecl => self.type_decl(n, c, DefKind::Interface, CF_IFACE),
            K::EnumDecl => self.type_decl(n, c, DefKind::Enum, 0),
            K::DelegateDecl => self.method(n, c, MK::Delegate),
            K::EnumMember => self.enum_member(n, c),
            K::FieldDecl | K::EventFieldDecl => self.field_declaration(n, c),
            K::PropertyDecl | K::EventDecl | K::IndexerDecl => self.property(n, c),
            K::MethodDecl => self.method(n, c, MK::Method),
            K::CtorDecl => self.method(n, c, MK::Ctor),
            K::DtorDecl => self.method(n, c, MK::Dtor),
            K::OperatorDecl => self.method(n, c, MK::Operator),
            K::ConversionDecl => self.method(n, c, MK::Conversion),
            K::LocalFunction => self.method(n, c, MK::Local),
            K::ParameterList | K::BracketedParameterList => {
                self.params(c);
            }
            K::Parameter => self.param(n, c),
            K::ImplicitParameter => {
                let name = self.cx.name(n);
                self.cx.declare(name, enc_local(NONE, 0));
            }
            K::ExplicitIface => self.each(c, |w, c| {
                if !matches!(w.k(c.node()), K::Token | K::Comment) {
                    w.type_ref(c, RefKind::Type, 255);
                }
            }),
            K::ConstraintsClause => self.constraints(c),
            K::Error => self.error(c),
            _ => self.children(c),
        }
    }

    // ----- small helpers -----

    #[inline]
    fn class_name(&self) -> u32 {
        self.classes.last().map_or(NONE, |c| c.own)
    }

    #[inline]
    fn owner_at(&self, level: usize) -> u32 {
        self.classes.get(level).map_or(NONE, |c| c.own)
    }

    fn string_id(&mut self) -> u32 {
        if self.string_id == NONE {
            self.string_id = self.cx.intern(b"String");
        }
        self.string_id
    }

    /// Intern `prefix` + `a` + `sep` + `b`, reusing the scratch buffer.
    fn joined(&mut self, prefix: &[u8], a: u32, sep: &[u8], b: u32) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.extend_from_slice(prefix);
        if a != NONE {
            buf.extend_from_slice(self.cx.name_bytes(a));
        }
        buf.extend_from_slice(sep);
        if b != NONE {
            buf.extend_from_slice(self.cx.name_bytes(b));
        }
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// Type path `base::fld`, or `base::fld()` for the declared result of a method. Paths are
    /// cut off after a few segments: a longer chain says little and every segment multiplies
    /// the candidates at query time.
    fn path(&mut self, base: u32, fld: u32, call: bool) -> u32 {
        let b = self.cx.name_bytes(base);
        if b.len() > 160 || b.windows(2).filter(|w| w == b"::").count() >= 6 {
            return NONE;
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.extend_from_slice(self.cx.name_bytes(base));
        buf.extend_from_slice(b"::");
        buf.extend_from_slice(self.cx.name_bytes(fld));
        if call {
            buf.extend_from_slice(b"()");
        }
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// The simple name at the end of a type path: `Inner` of `Outer::Inner`, `Stream` of
    /// `System.IO|Stream`.
    fn simple_of(&mut self, ty: u32) -> u32 {
        if ty == NONE {
            return NONE;
        }
        let b = self.cx.name_bytes(ty);
        match b.iter().rposition(|&x| x == b':' || x == b'|') {
            Some(i) => {
                let mut buf = std::mem::take(&mut self.cx.s.buf);
                buf.clear();
                buf.extend_from_slice(&self.cx.name_bytes(ty)[i + 1..]);
                let id = self.cx.intern(&buf);
                self.cx.s.buf = buf;
                id
            }
            None => ty,
        }
    }

    /// What a reference to the type at the end of a path is qualified by: the namespace of
    /// `N|T`, the simple name of the outer type of `Outer::T`, nothing for a bare `T`.
    fn qual_of_path(&mut self, ty: u32) -> u32 {
        if ty == NONE {
            return NONE;
        }
        let b = self.cx.name_bytes(ty);
        if let Some(i) = b.windows(2).rposition(|w| w == b"::") {
            let mut buf = std::mem::take(&mut self.cx.s.buf);
            buf.clear();
            let head = &self.cx.name_bytes(ty)[..i];
            let from = head.iter().rposition(|&x| x == b':' || x == b'|').map_or(0, |j| j + 1);
            buf.extend_from_slice(&head[from..]);
            let id = if buf.is_empty() { NONE } else { self.cx.intern(&buf) };
            self.cx.s.buf = buf;
            return id;
        }
        match memchr::memchr(b'|', b) {
            Some(i) => {
                let mut buf = std::mem::take(&mut self.cx.s.buf);
                buf.clear();
                buf.extend_from_slice(&self.cx.name_bytes(ty)[..i]);
                let id = self.cx.intern(&buf);
                self.cx.s.buf = buf;
                id
            }
            None => NONE,
        }
    }

    /// The qualifier a receiver contributes to a member reference.
    fn qual(&mut self, r: Recv) -> u32 {
        match r.k {
            RK::Super => self.super_qual(),
            RK::Type | RK::Pkg => r.base,
            RK::This => self.class_name(),
            // Members of an array (`Length`) are not members of its element type.
            _ if r.dims > 0 || r.base == NONE => NONE,
            _ if r.fld == NONE => r.base,
            _ => self.path(r.base, r.fld, r.call),
        }
    }

    /// `^Class`: a strict ancestor of the enclosing type.
    fn super_qual(&mut self) -> u32 {
        let Some(cl) = self.classes.last().copied() else { return NONE };
        if cl.sup_q != NONE || cl.own == NONE {
            return cl.sup_q;
        }
        let q = self.joined(b"^", cl.own, b"", NONE);
        if let Some(cl) = self.classes.last_mut() {
            cl.sup_q = q;
        }
        q
    }

    /// A dotted name (`a.b.c`) as written, without embedded whitespace or comments.
    fn dotted(&mut self, n: Node<'t>) -> u32 {
        let text = self.cx.text(n);
        if !text.iter().any(|&b| b <= b' ' || b == b'/') {
            return self.cx.intern(text);
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        push_dotted(text, &mut buf);
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// The name of a declaration, unless the parser had to invent it (a zero-width MISSING
    /// node).
    fn decl_name(&self, n: Node<'t>) -> Option<Node<'t>> {
        child(n, self.t.f_name).filter(|x| x.end_byte() > x.start_byte())
    }

    /// End of a declaration without its trailing `;`.
    fn before_semicolon(&self, n: Node<'t>) -> usize {
        let e = n.end_byte();
        if e > n.start_byte() && self.cx.src.get(e - 1) == Some(&b';') { e - 1 } else { e }
    }

    /// Modifier keywords and attributes of a declaration.
    fn modifiers(&self, n: Node<'t>) -> Mods {
        let mut m = Mods { bits: 0, sig_start: n.start_byte() };
        let mut sig: Option<usize> = None;
        for i in 0..n.child_count() {
            let Some(ch) = n.child(i) else { break };
            match self.k(ch) {
                K::AttributeList => {
                    if self.has_test_attribute(ch) {
                        m.bits |= M_TEST;
                    }
                }
                K::Comment | K::Preproc => {}
                K::Modifier => {
                    if sig.is_none() {
                        sig = Some(ch.start_byte());
                    }
                    m.bits |= match self.cx.text(ch) {
                        b"public" => M_PUBLIC,
                        b"private" => M_PRIVATE,
                        b"protected" | b"internal" | b"file" => M_ACCESS,
                        b"abstract" => M_ABSTRACT,
                        b"static" => M_STATIC,
                        b"async" => M_ASYNC,
                        b"extern" => M_EXTERN,
                        b"partial" => M_PARTIAL,
                        b"const" => M_CONST,
                        _ => 0,
                    };
                }
                _ => {
                    if sig.is_none() {
                        sig = Some(ch.start_byte());
                    }
                    break;
                }
            }
        }
        if let Some(s) = sig {
            m.sig_start = s;
        }
        m
    }

    fn has_test_attribute(&self, list: Node<'t>) -> bool {
        for i in 0..list.named_child_count() as u32 {
            let Some(a) = list.named_child(i) else { break };
            if self.k(a) != K::Attribute {
                continue;
            }
            let Some(nm) = child(a, self.t.f_name) else { continue };
            let last = self.last_name(nm);
            let text = self.cx.text(last);
            let text = text.strip_suffix(b"Attribute").unwrap_or(text);
            if matches!(text, b"Test" | b"Fact" | b"Theory" | b"TestMethod" | b"DataTestMethod" | b"TestCase" | b"TestCaseSource" | b"SkippableFact") {
                return true;
            }
        }
        false
    }

    /// The identifier at the end of a possibly qualified or generic name.
    fn last_name(&self, mut n: Node<'t>) -> Node<'t> {
        for _ in 0..8 {
            match self.k(n) {
                K::QualifiedName | K::AliasQualifiedName => match child(n, self.t.f_name) {
                    Some(x) => n = x,
                    None => break,
                },
                K::GenericName => match n.named_child(0) {
                    Some(x) => n = x,
                    None => break,
                },
                _ => break,
            }
        }
        n
    }

    /// Set the container, flags and signature start of a freshly created definition.
    fn finish_def(&mut self, d: u32, parent: u32, flags: u8, sig_start: usize, sig_end: usize) {
        let dm = self.cx.def_mut(d);
        dm.parent = parent;
        dm.flags |= flags;
        if sig_start > dm.sig_off as usize {
            dm.sig_off = sig_start as u32;
            dm.sig_len = sig_end.saturating_sub(sig_start).min(u16::MAX as usize) as u16;
        }
    }

    /// Visibility flags of a member of the innermost type.
    fn visibility(&self, m: Mods) -> u8 {
        let cf = self.classes.last().map_or(0, |c| c.flags);
        if m.bits & M_PUBLIC != 0 || (cf & CF_IFACE != 0 && m.bits & (M_PRIVATE | M_ACCESS) == 0) {
            dflag::EXPORTED
        } else if cf & CF_PARTIAL == 0 && (m.bits & M_PRIVATE != 0 || (cf & CF_PRIVATE_DEFAULT != 0 && m.bits & M_ACCESS == 0)) {
            dflag::LOCAL
        } else {
            0
        }
    }

    // ----- types -----

    /// Is the dotted name a namespace this file names, or a prefix of one?
    fn ns_known(&self, text: &[u8]) -> bool {
        if text.is_empty() {
            return false;
        }
        if text == b"System" || text == b"Microsoft" {
            return true;
        }
        self.namespaces.iter().any(|&k| {
            let kb = self.cx.name_bytes(k);
            kb == text || (kb.len() > text.len() && kb.starts_with(text) && kb[text.len()] == b'.')
        })
    }

    #[inline]
    fn known_type(&self, name: u32) -> bool {
        self.types.iter().any(|t| t.0 == name)
    }

    #[inline]
    fn alias_of(&self, name: u32) -> Option<Alias> {
        if self.aliases.is_empty() {
            return None;
        }
        self.aliases.iter().find(|a| a.alias == name).copied()
    }

    /// The type parameter `name` in scope, innermost first.
    #[inline]
    fn tparam(&self, name: u32) -> Option<(u32, u32)> {
        if self.tparams.is_empty() {
            return None;
        }
        self.tparams.iter().rev().find(|p| p.0 == name).copied()
    }

    /// Name and array dimensions of a declared type: generic arguments stripped, a type
    /// parameter replaced by its constraint, an alias by what it stands for. A qualified name
    /// keeps its qualifier as a type path (`System.IO|Stream`, `Outer::Inner`). With `unwrap`,
    /// `Task<T>` and `ValueTask<T>` (a method result) are `T`.
    fn type_name(&mut self, mut ty: Node<'t>, unwrap: bool) -> (u32, u8) {
        let t = self.t;
        let mut dims = 0u8;
        for _ in 0..8 {
            match self.k(ty) {
                K::Identifier => {
                    let text = self.cx.text(ty);
                    if text.is_empty() || text == b"dynamic" || text == b"var" {
                        return (NONE, 0);
                    }
                    let id = self.cx.intern(text);
                    if let Some((_, bound)) = self.tparam(id) {
                        return (bound, dims);
                    }
                    if let Some(a) = self.alias_of(id) {
                        return (a.path, dims);
                    }
                    return (id, dims);
                }
                K::GenericName => {
                    let Some(head) = ty.named_child(0) else { break };
                    if unwrap
                        && matches!(self.cx.text(head), b"Task" | b"ValueTask")
                        && let Some(arg) = ty.named_child(1).and_then(|a| if a.named_child_count() == 1 { a.named_child(0) } else { None })
                    {
                        ty = arg;
                        continue;
                    }
                    if self.cx.text(head) == b"Nullable"
                        && let Some(arg) = ty.named_child(1).and_then(|a| a.named_child(0))
                    {
                        ty = arg;
                        continue;
                    }
                    let id = self.cx.name(head);
                    if let Some(a) = self.alias_of(id) {
                        return (a.path, dims);
                    }
                    return (id, dims);
                }
                K::QualifiedName => {
                    let mut buf = std::mem::take(&mut self.cx.s.buf);
                    buf.clear();
                    let ok = self.qualified_path(ty, &mut buf);
                    let id = if ok && !buf.is_empty() { self.cx.intern(&buf) } else { NONE };
                    self.cx.s.buf = buf;
                    return (id, dims);
                }
                K::AliasQualifiedName => match child(ty, t.f_name) {
                    Some(x) => ty = x,
                    None => break,
                },
                K::WrapperType => match child(ty, t.f_type) {
                    Some(x) => ty = x,
                    None => break,
                },
                K::ArrayType => {
                    dims = dims.saturating_add(1);
                    match child(ty, t.f_type) {
                        Some(x) => ty = x,
                        None => break,
                    }
                }
                K::PredefinedType => {
                    return match self.cx.text(ty) {
                        b"string" => (self.string_id(), dims),
                        b"object" => (self.cx.intern(b"Object"), dims),
                        _ => (NONE, dims),
                    };
                }
                _ => break,
            }
        }
        (NONE, dims)
    }

    /// Write the type path of a qualified type name into `buf`: `N|T`, `Outer::Inner`,
    /// `N|Outer::Inner`. False when the name has a shape that cannot be told.
    fn qualified_path(&mut self, n: Node<'t>, buf: &mut Vec<u8>) -> bool {
        let t = self.t;
        let (Some(q), Some(last)) = (child(n, t.f_qualifier), child(n, t.f_name)) else { return false };
        let last = self.last_name(last);
        if self.k(last) != K::Identifier || last.end_byte() == last.start_byte() {
            return false;
        }
        match self.classify(q, 1) {
            Scope::Namespace => {
                push_dotted(self.cx.text(q), buf);
                if buf.starts_with(b"global::") {
                    buf.drain(..8);
                }
                buf.push(b'|');
            }
            Scope::Type => {
                let (p, _) = self.type_name(q, false);
                if p == NONE {
                    return false;
                }
                buf.extend_from_slice(self.cx.name_bytes(p));
                buf.extend_from_slice(b"::");
            }
        }
        buf.extend_from_slice(self.cx.text(last));
        true
    }

    /// Is the qualifier of a qualified type name a namespace or a type? `after` is the number
    /// of segments that follow it. A name this file declares as a type, an alias, a type
    /// parameter or a generic name is a type; a namespace this file names is a namespace. An
    /// unknown root followed by two or more segments starts a namespace (`Foo.Bar.Baz`), one
    /// followed by a single segment is an outer type (`JsonReader.State`).
    fn classify(&mut self, q: Node<'t>, after: u32) -> Scope {
        match self.k(q) {
            K::Identifier => {
                let text = self.cx.text(q);
                let id = self.cx.intern(text);
                if self.known_type(id) || self.alias_of(id).is_some() || self.tparam(id).is_some() {
                    Scope::Type
                } else if self.ns_known(text) || after >= 2 {
                    Scope::Namespace
                } else {
                    Scope::Type
                }
            }
            K::AliasQualifiedName => Scope::Namespace,
            K::QualifiedName => {
                let mut buf = std::mem::take(&mut self.cx.s.buf);
                buf.clear();
                push_dotted(self.cx.text(q), &mut buf);
                let known = self.ns_known(buf.strip_prefix(b"global::").unwrap_or(&buf));
                self.cx.s.buf = buf;
                if known {
                    return Scope::Namespace;
                }
                let (Some(inner), Some(last)) = (child(q, self.t.f_qualifier), child(q, self.t.f_name)) else { return Scope::Type };
                if self.k(last) != K::Identifier {
                    return Scope::Type;
                }
                let id = self.cx.name(last);
                match self.classify(inner, after + 1) {
                    Scope::Namespace if !self.known_type(id) => Scope::Namespace,
                    _ => Scope::Type,
                }
            }
            _ => Scope::Type,
        }
    }

    /// Reference to a type by its simple name. Type parameters and the stop list emit nothing
    /// (a name written with a qualifier is never a type parameter); an alias is the type it
    /// stands for.
    fn type_ident(&mut self, n: Node<'t>, kind: RefKind, qual: u32, nargs: u8) {
        let text = self.cx.text(n);
        if text.is_empty() || is_stop(text) {
            return;
        }
        let name = self.cx.intern(text);
        if qual == NONE {
            if self.tparam(name).is_some() {
                return;
            }
            if let Some(a) = self.alias_of(name) {
                if !is_stop(self.cx.name_bytes(a.name)) {
                    self.cx.add_ref(a.name, kind, line_of(n), a.module, nargs);
                }
                return;
            }
        }
        self.cx.add_ref(name, kind, line_of(n), qual, nargs);
    }

    /// References of a type written at the cursor: the head with `kind` (`Type`, `New`,
    /// `Extends`, ...), type arguments, qualifiers and array ranks as plain uses.
    fn type_ref(&mut self, c: &mut TreeCursor<'t>, kind: RefKind, nargs: u8) {
        let n = c.node();
        match self.k(n) {
            K::Identifier => self.type_ident(n, kind, NONE, nargs),
            K::GenericName => self.each(c, |w, c| match w.k(c.node()) {
                K::Identifier => w.type_ident(c.node(), kind, NONE, nargs),
                K::TypeArgumentList => w.type_args(c),
                _ => {}
            }),
            K::QualifiedName => self.qualified_type(c, kind, nargs),
            K::AliasQualifiedName => {
                let f_name = self.t.f_name;
                self.each(c, |w, c| {
                    if is_field(c, f_name) {
                        w.type_ref(c, kind, nargs);
                    }
                })
            }
            K::ArrayType | K::WrapperType => {
                let f_type = self.t.f_type;
                let inner = if self.k(n) == K::ArrayType { RefKind::Type } else { kind };
                self.each(c, |w, c| {
                    if is_field(c, f_type) {
                        w.type_ref(c, inner, nargs);
                    } else if !matches!(w.k(c.node()), K::Token | K::Comment) {
                        w.visit(c);
                    }
                })
            }
            K::TupleType => {
                let f_type = self.t.f_type;
                self.each(c, |w, c| {
                    w.each(c, |w, c| {
                        if is_field(c, f_type) {
                            w.type_ref(c, RefKind::Type, 255);
                        }
                    })
                })
            }
            K::PredefinedType | K::ImplicitType | K::Token | K::Comment => {}
            _ => self.visit_node(c),
        }
    }

    fn type_args(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            if !matches!(w.k(c.node()), K::Token | K::Comment) {
                w.type_ref(c, RefKind::Type, 255);
            }
        });
    }

    /// `N.T` / `Outer.Inner`: the last segment is the type, qualified by the namespace or the
    /// outer type it is written with.
    fn qualified_type(&mut self, c: &mut TreeCursor<'t>, kind: RefKind, nargs: u8) {
        let n = c.node();
        let t = self.t;
        let Some(q) = child(n, t.f_qualifier) else { return };
        let scope = self.classify(q, 1);
        let mut qual = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            if is_field(c, t.f_qualifier) {
                qual = match scope {
                    Scope::Namespace => {
                        let id = w.dotted(ch);
                        let b = w.cx.name_bytes(id);
                        match b.strip_prefix(b"global::") {
                            Some(rest) => {
                                let rest = rest.to_owned();
                                w.cx.intern(&rest)
                            }
                            None => id,
                        }
                    }
                    Scope::Type => {
                        w.type_ref(c, RefKind::Type, 255);
                        let (p, _) = w.type_name(ch, false);
                        w.simple_of(p)
                    }
                };
            } else if is_field(c, t.f_name) {
                match w.k(ch) {
                    K::Identifier => w.type_ident(ch, kind, qual, nargs),
                    K::GenericName => {
                        let q = qual;
                        w.each(c, |w, c| match w.k(c.node()) {
                            K::Identifier => w.type_ident(c.node(), kind, q, nargs),
                            K::TypeArgumentList => w.type_args(c),
                            _ => {}
                        })
                    }
                    _ => {}
                }
            }
        });
    }

    /// Declare the type parameters of a type, method or delegate. A parameter stands for its
    /// first type constraint, so that `where T : IWriter ... t.Write(x)` is a call on `IWriter`.
    fn declare_tparams(&mut self, n: Node<'t>) {
        let t = self.t;
        let mark = self.tparams.len();
        for i in 0..n.child_count() {
            let Some(ch) = n.child(i) else { break };
            match self.k(ch) {
                K::TypeParameterList => {
                    for j in 0..ch.named_child_count() as u32 {
                        let Some(p) = ch.named_child(j) else { break };
                        if let Some(nm) = child(p, t.f_name) {
                            let name = self.cx.name(nm);
                            self.tparams.push((name, NONE));
                        }
                    }
                }
                K::ConstraintsClause => {
                    let mut target = NONE;
                    for j in 0..ch.named_child_count() as u32 {
                        let Some(x) = ch.named_child(j) else { break };
                        match self.k(x) {
                            K::Identifier if target == NONE => target = self.cx.name(x),
                            K::Other => {
                                let Some(ty) = child(x, t.f_type) else { continue };
                                let (bound, _) = self.type_name(ty, false);
                                if let Some(p) = self.tparams[mark..].iter_mut().find(|p| p.0 == target && p.1 == NONE) {
                                    p.1 = bound;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// `where T : A, new()`: the constraints are references, `T` is not.
    fn constraints(&mut self, c: &mut TreeCursor<'t>) {
        let f_type = self.t.f_type;
        self.each(c, |w, c| {
            if matches!(w.k(c.node()), K::Token | K::Comment | K::Identifier) {
                return;
            }
            w.each(c, |w, c| {
                if is_field(c, f_type) {
                    w.type_ref(c, RefKind::Type, 255);
                }
            });
        });
    }

    // ----- file level -----

    fn compilation_unit(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        self.prescan(n, 0);
        // Top-level statements declare locals at file level.
        self.cx.push_scope();
        let mut file_ns = 0usize;
        let end_line = end_line_of(n);
        self.each(c, |w, c| {
            let ch = c.node();
            if w.k(ch) == K::FileNamespaceDecl {
                // `namespace A.B;` holds for the rest of the file.
                if let Some(nm) = w.decl_name(ch) {
                    let name = w.dotted(nm);
                    let sig_end = w.before_semicolon(ch);
                    let parent = w.cx.encl();
                    let d = w.cx.def(name, DefKind::Namespace, ch, sig_end);
                    w.finish_def(d, parent, 0, ch.start_byte(), sig_end);
                    w.cx.def_mut(d).end_line = end_line;
                    w.cx.push_def(d);
                    w.ns_stack.push(name);
                    file_ns += 1;
                }
            } else {
                w.visit(c);
            }
        });
        for _ in 0..file_ns {
            self.cx.pop_def();
            self.ns_stack.pop();
        }
        self.cx.pop_scope();
    }

    /// Before the walk: the types this file declares, the namespaces it names and its
    /// `using` aliases, which decide how qualified names read wherever they occur.
    fn prescan(&mut self, n: Node<'t>, depth: u32) {
        if depth > 12 {
            return;
        }
        let t = self.t;
        for i in 0..n.named_child_count() as u32 {
            let Some(ch) = n.named_child(i) else { break };
            match self.k(ch) {
                K::NamespaceDecl | K::FileNamespaceDecl => {
                    if let Some(nm) = self.decl_name(ch) {
                        let id = self.dotted(nm);
                        // A nested namespace is named after the ones around it.
                        let full = match self.ns_stack.last().copied() {
                            Some(outer) if self.k(ch) == K::NamespaceDecl && depth > 0 => self.joined(b"", outer, b".", id),
                            _ => id,
                        };
                        if !self.namespaces.contains(&full) {
                            self.namespaces.push(full);
                        }
                        if self.k(ch) == K::NamespaceDecl
                            && let Some(body) = child(ch, t.f_body)
                        {
                            self.ns_stack.push(full);
                            self.prescan(body, depth + 1);
                            self.ns_stack.pop();
                        }
                    }
                }
                K::UsingDirective => self.prescan_using(ch),
                K::ClassDecl | K::StructDecl | K::RecordDecl | K::InterfaceDecl | K::EnumDecl | K::DelegateDecl => {
                    if let Some(nm) = self.decl_name(ch) {
                        let id = self.cx.name(nm);
                        let iface = self.k(ch) == K::InterfaceDecl;
                        if !self.types.iter().any(|t| t.0 == id) {
                            self.types.push((id, iface));
                        }
                    }
                    if let Some(body) = child(ch, t.f_body) {
                        self.prescan(body, depth + 1);
                    }
                }
                K::Preproc | K::DeclarationList => self.prescan(ch, depth + 1),
                _ => {}
            }
        }
    }

    fn prescan_using(&mut self, n: Node<'t>) {
        let t = self.t;
        let alias = child(n, t.f_name);
        let mut is_static = false;
        let mut target = None;
        for i in 0..n.child_count() {
            let Some(ch) = n.child(i) else { break };
            if ch.kind_id() == t.kw_static {
                is_static = true;
            } else if ch.is_named() && Some(ch) != alias && !matches!(self.k(ch), K::Comment) {
                target = Some(ch);
            }
        }
        let Some(target) = target else { return };
        match (alias, self.k(target)) {
            (Some(a), _) => {
                // `using A = N.T;`: the namespace part is a namespace, the whole a type.
                let alias = self.cx.name(a);
                let (module, name) = self.split_target(target);
                if name == NONE {
                    return;
                }
                if module != NONE && !self.namespaces.contains(&module) {
                    self.namespaces.push(module);
                }
                let path = if module == NONE { name } else { self.joined(b"", module, b"|", name) };
                self.aliases.push(Alias { alias, path, module, name });
            }
            (None, K::Identifier | K::QualifiedName | K::AliasQualifiedName) => {
                let (module, _) = self.split_target(target);
                let id = if is_static { module } else { self.dotted(target) };
                if id != NONE && !self.namespaces.contains(&id) {
                    self.namespaces.push(id);
                }
            }
            _ => {}
        }
    }

    /// `N.T` (or `N.T<A>`) as (namespace `N`, simple name `T`). `N` is `NONE` for a bare name.
    fn split_target(&mut self, n: Node<'t>) -> (u32, u32) {
        let t = self.t;
        match self.k(n) {
            K::QualifiedName | K::AliasQualifiedName => {
                let module = match child(n, t.f_qualifier).or_else(|| child(n, t.f_alias)) {
                    Some(q) if self.k(n) == K::QualifiedName => {
                        let id = self.dotted(q);
                        let b = self.cx.name_bytes(id);
                        match b.strip_prefix(b"global::") {
                            Some(rest) => {
                                let rest = rest.to_owned();
                                self.cx.intern(&rest)
                            }
                            None => id,
                        }
                    }
                    _ => NONE,
                };
                let name = child(n, t.f_name).map(|x| self.last_name(x)).filter(|x| x.end_byte() > x.start_byte());
                (module, name.map_or(NONE, |x| self.cx.name(x)))
            }
            K::GenericName => (NONE, n.named_child(0).map_or(NONE, |x| self.cx.name(x))),
            K::Identifier => (NONE, self.cx.name(n)),
            _ => (NONE, NONE),
        }
    }

    fn using(&mut self, n: Node<'t>) {
        let t = self.t;
        let alias = child(n, t.f_name);
        let mut is_static = false;
        let mut global = false;
        let mut target = None;
        for i in 0..n.child_count() {
            let Some(ch) = n.child(i) else { break };
            let id = ch.kind_id();
            if id == t.kw_static {
                is_static = true;
            } else if id == t.kw_global {
                global = true;
            } else if ch.is_named() && Some(ch) != alias && !matches!(self.k(ch), K::Comment) {
                target = Some(ch);
            }
        }
        let Some(target) = target.filter(|x| x.end_byte() > x.start_byte()) else { return };
        let line = line_of(n);
        let text = self.cx.text(target);
        let sys = if text == b"System" || text.starts_with(b"System.") || text.starts_with(b"global::System") { iflag::SYSTEM } else { 0 };
        let flags = sys | if global { iflag::REEXPORT } else { 0 };
        if let Some(a) = alias {
            let (module, name) = self.split_target(target);
            if name == NONE {
                return;
            }
            let alias = self.cx.name(a);
            let module_or_empty = if module == NONE { self.cx.intern(b"") } else { module };
            self.cx.add_import(module_or_empty, name, alias, line, flags);
            self.cx.add_ref(name, RefKind::Import, line_of(target), module, 255);
            // Type arguments of an aliased generic type.
            if let Some(last) = child(target, t.f_name).or(Some(target)) {
                self.alias_type_args(last);
            }
            return;
        }
        let dotted = self.dotted(target);
        let id = match self.cx.name_bytes(dotted).strip_prefix(b"global::") {
            Some(rest) => {
                let rest = rest.to_owned();
                self.cx.intern(&rest)
            }
            None => dotted,
        };
        self.cx.add_import(id, NONE, NONE, line, iflag::WILDCARD | flags);
        if is_static {
            // `using static N.T;` uses the type `T` of namespace `N`.
            let (module, name) = self.split_target(target);
            if name != NONE {
                self.cx.add_ref(name, RefKind::Import, line, module, 255);
            }
        }
    }

    /// Type references for the arguments of `using A = List<B>;`.
    fn alias_type_args(&mut self, n: Node<'t>) {
        if self.k(n) != K::GenericName {
            return;
        }
        let Some(args) = n.named_child(1) else { return };
        for i in 0..args.named_child_count() as u32 {
            let Some(a) = args.named_child(i) else { break };
            let ty = self.last_name(a);
            if self.k(ty) == K::Identifier {
                self.type_ident(ty, RefKind::Type, NONE, 255);
            }
        }
    }

    fn namespace(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(nm) = self.decl_name(n) else { return self.children(c) };
        let id = self.dotted(nm);
        let name = match self.ns_stack.last().copied() {
            Some(outer) => self.joined(b"", outer, b".", id),
            None => id,
        };
        let body = child(n, t.f_body);
        let sig_end = body.map_or(n.end_byte(), |b| b.start_byte());
        let parent = self.cx.encl();
        let d = self.cx.def(name, DefKind::Namespace, n, sig_end);
        self.finish_def(d, parent, 0, n.start_byte(), sig_end);
        self.cx.push_def(d);
        self.ns_stack.push(name);
        let fb = t.f_body;
        self.each(c, |w, c| {
            if is_field(c, fb) {
                w.children(c);
            }
        });
        self.ns_stack.pop();
        self.cx.pop_def();
    }

    // ----- type declarations -----

    fn type_decl(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: DefKind, cf: u8) {
        let t = self.t;
        let Some(nm) = self.decl_name(n) else { return self.children(c) };
        let m = self.modifiers(n);
        let body = child(n, t.f_body);
        let sig_end = body.map_or(self.before_semicolon(n), |b| b.start_byte());
        let name = self.cx.name(nm);
        let parent = self.cx.encl();
        let mut flags = self.visibility(m);
        if m.bits & M_ABSTRACT != 0 {
            flags |= dflag::ABSTRACT;
        }
        let outer_partial = self.classes.last().is_some_and(|cl| cl.flags & CF_PARTIAL != 0);
        let d = self.cx.def(name, kind, n, sig_end);
        self.finish_def(d, parent, flags, m.sig_start, sig_end);
        self.cx.push_def(d);
        let tp_mark = self.tparams.len();
        self.declare_tparams(n);
        let mut cls = Class {
            own: name,
            sup: NONE,
            sup_q: NONE,
            flags: cf
                | if m.bits & M_PARTIAL != 0 || outer_partial { CF_PARTIAL } else { 0 }
                | if matches!(kind, DefKind::Class | DefKind::Struct) { CF_PRIVATE_DEFAULT } else { 0 },
        };
        // The primary constructor: record parameters are properties, the parameters of a class
        // or struct are in scope in the whole body.
        self.cx.push_scope();
        let record = self.k(n) == K::RecordDecl;
        let nm_at = nm.start_byte();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment | K::Modifier => {}
                K::BaseList => cls.sup = w.base_list(c, kind),
                K::ParameterList if record => w.record_params(c),
                K::ParameterList => {
                    w.params(c);
                }
                K::DeclarationList | K::EnumMemberList => w.class_body(c, cls, ch),
                K::TypeParameterList => {}
                _ if ch.start_byte() == nm_at => {}
                _ => w.visit(c),
            }
        });
        self.cx.pop_scope();
        self.tparams.truncate(tp_mark);
        self.cx.pop_def();
    }

    /// `: Base(args), IA, IB`: one `Extends` or `Implements` per listed type. The first entry
    /// of a class or record is its base class unless it is an interface (declared as one in
    /// this file, or named `IName`); every other entry, and every entry of a struct, is an
    /// interface; an interface extends its entries. Returns the simple name of the base class.
    fn base_list(&mut self, c: &mut TreeCursor<'t>, kind: DefKind) -> u32 {
        let mut sup = NONE;
        let mut idx = 0;
        let mut last_head = NONE;
        let f_type = self.t.f_type;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::ArgumentList => {
                    // `: Base(x)` runs a constructor of the base class.
                    let nargs = w.arguments(c, last_head);
                    if last_head != NONE && !is_stop(w.cx.name_bytes(last_head)) {
                        let line = line_of(ch);
                        w.cx.add_ref(last_head, RefKind::Call, line, last_head, nargs);
                    }
                }
                K::PrimaryCtorBase => {
                    let mut head = NONE;
                    w.each(c, |w, c| {
                        let x = c.node();
                        if is_field(c, f_type) {
                            head = w.type_name(x, false).0;
                            head = w.simple_of(head);
                            w.type_ref(c, RefKind::Extends, 255);
                        } else if w.k(x) == K::ArgumentList {
                            let nargs = w.arguments(c, head);
                            if head != NONE && !is_stop(w.cx.name_bytes(head)) {
                                w.cx.add_ref(head, RefKind::Call, line_of(x), head, nargs);
                            }
                        }
                    });
                    if idx == 0 {
                        sup = head;
                    }
                    idx += 1;
                }
                _ => {
                    let (path, _) = w.type_name(ch, false);
                    let head = w.simple_of(path);
                    let iface = head != NONE && {
                        let hb = w.cx.name_bytes(head);
                        w.types.iter().any(|t| t.0 == head && t.1) || (hb.len() > 1 && hb[0] == b'I' && hb[1].is_ascii_uppercase())
                    };
                    let k = match kind {
                        DefKind::Interface => RefKind::Extends,
                        DefKind::Class if idx == 0 && !iface => RefKind::Extends,
                        DefKind::Enum => RefKind::Type,
                        _ => RefKind::Implements,
                    };
                    if k == RefKind::Extends && kind == DefKind::Class {
                        sup = head;
                    }
                    last_head = head;
                    w.type_ref(c, k, 255);
                    idx += 1;
                }
            }
        });
        sup
    }

    /// Enter a type body: its members become visible, then the body is walked.
    fn class_body(&mut self, c: &mut TreeCursor<'t>, cls: Class, body: Node<'t>) {
        let level = self.classes.len();
        self.classes.push(cls);
        self.cx.push_scope();
        self.declare_members(body, level, cls.own, 0);
        self.children(c);
        self.cx.pop_scope();
        self.classes.pop();
    }

    /// Members may be used before their declaration: declare them all when the body is
    /// entered.
    fn declare_members(&mut self, body: Node<'t>, level: usize, own: u32, depth: u32) {
        let t = self.t;
        for i in 0..body.named_child_count() as u32 {
            let Some(ch) = body.named_child(i) else { break };
            match self.k(ch) {
                K::FieldDecl | K::EventFieldDecl => {
                    for j in 0..ch.named_child_count() as u32 {
                        let Some(vd) = ch.named_child(j) else { break };
                        if self.k(vd) != K::VariableDeclaration {
                            continue;
                        }
                        let (ty, dims) = child(vd, t.f_type).map_or((NONE, 0), |x| self.type_name(x, false));
                        for k in 0..vd.named_child_count() as u32 {
                            let Some(d) = vd.named_child(k) else { break };
                            if self.k(d) != K::VariableDeclarator {
                                continue;
                            }
                            if let Some(nm) = child(d, t.f_name) {
                                let name = self.cx.name(nm);
                                self.cx.declare(name, enc_member(level, ty, dims, false));
                            }
                        }
                    }
                }
                K::PropertyDecl | K::EventDecl => {
                    if let Some(nm) = self.decl_name(ch) {
                        let (ty, dims) = child(ch, t.f_type).map_or((NONE, 0), |x| self.type_name(x, false));
                        let name = self.cx.name(nm);
                        self.cx.declare(name, enc_member(level, ty, dims, false));
                    }
                }
                K::MethodDecl => {
                    if let Some(nm) = self.decl_name(ch) {
                        let name = self.cx.name(nm);
                        // A field of the same name (rare) wins: it is what a bare read denotes.
                        if self.cx.local_type(name).is_none_or(|v| !is_member(v) || dec_level(v) != level) {
                            let ty = child(ch, t.f_returns).map_or(NONE, |x| self.type_name(x, true).0);
                            self.cx.declare(name, enc_member(level, ty, 0, true));
                        }
                    }
                }
                K::EnumMember => {
                    if let Some(nm) = self.decl_name(ch) {
                        let name = self.cx.name(nm);
                        self.cx.declare(name, enc_member(level, own, 0, false));
                    }
                }
                K::Preproc if depth < 4 => self.declare_members(ch, level, own, depth + 1),
                _ => {}
            }
        }
    }

    /// `record R(int A, string B)`: every parameter is a public property of the record.
    fn record_params(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let level = self.classes.len();
        self.each(c, |w, c| {
            let ch = c.node();
            if w.k(ch) != K::Parameter {
                return;
            }
            let Some(nm) = w.decl_name(ch) else { return w.children(c) };
            let (ty, dims) = child(ch, t.f_type).map_or((NONE, 0), |x| w.type_name(x, false));
            let name = w.cx.name(nm);
            let parent = w.cx.encl();
            let m = w.modifiers(ch);
            let d = w.cx.def(name, DefKind::Property, ch, ch.end_byte());
            w.finish_def(d, parent, dflag::EXPORTED, m.sig_start, ch.end_byte());
            w.cx.def_mut(d).type_name = ty;
            // Visible in the body as members of the record (one level further in).
            w.cx.declare(name, enc_member(level, ty, dims, false));
            w.cx.push_def(d);
            w.children_except(c, nm.start_byte());
            w.cx.pop_def();
        });
    }

    fn enum_member(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let Some(nm) = self.decl_name(n) else { return self.children(c) };
        let m = self.modifiers(n);
        let sig_end = child(n, self.t.f_value).map_or(n.end_byte(), |v| self.trim_assign(v.start_byte()));
        let name = self.cx.name(nm);
        let parent = self.cx.encl();
        let own = self.class_name();
        let d = self.cx.def(name, DefKind::EnumMember, n, sig_end);
        self.finish_def(d, parent, dflag::EXPORTED, m.sig_start, sig_end);
        self.cx.def_mut(d).type_name = own;
        self.cx.push_def(d);
        self.children_except(c, nm.start_byte());
        self.cx.pop_def();
    }

    // ----- members -----

    /// Does a declared type hold a callable: a delegate declared in this file, `Func<..>`,
    /// `Action<..>`, `EventHandler<..>`, `Predicate<..>`?
    fn callable_type(&self, ty: Option<Node<'t>>) -> bool {
        let Some(ty) = ty else { return false };
        let head = self.last_name(ty);
        let text = self.cx.text(head);
        matches!(text, b"Func" | b"Action" | b"EventHandler" | b"Predicate" | b"Comparison" | b"Converter" | b"Delegate")
            || (self.k(head) == K::Identifier && self.cx.text(head).ends_with(b"Handler"))
    }

    fn field_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let m = self.modifiers(n);
        let event = self.k(n) == K::EventFieldDecl;
        let flags = self.visibility(m);
        let parent = self.cx.encl();
        let owner = self.class_name();
        let kind = if m.bits & M_CONST != 0 { DefKind::Constant } else { DefKind::Field };
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::VariableDeclaration => {
                    let ty_node = child(ch, t.f_type);
                    let (ty, _) = ty_node.map_or((NONE, 0), |x| w.type_name(x, false));
                    let callable = event || w.callable_type(ty_node);
                    w.each(c, |w, c| {
                        let d = c.node();
                        match w.k(d) {
                            K::VariableDeclarator => {
                                let Some(nm) = w.decl_name(d) else { return w.children(c) };
                                let val = w.declarator_value(d);
                                let sig_end = val.map_or(d.end_byte(), |v| v.start_byte());
                                let sig_end = w.trim_assign(sig_end);
                                let name = w.cx.name(nm);
                                let def = w.cx.def(name, kind, n, sig_end);
                                let fnptr = callable || val.is_some_and(|v| matches!(w.k(v), K::Lambda | K::AnonymousMethod));
                                w.finish_def(def, parent, if fnptr { flags | dflag::FNPTR } else { flags }, m.sig_start, sig_end);
                                w.cx.def_mut(def).type_name = ty;
                                w.cx.push_def(def);
                                let saved = std::mem::replace(&mut w.target_ty, ty);
                                w.initializer_slot(c, nm.start_byte(), owner, name);
                                w.target_ty = saved;
                                w.cx.pop_def();
                            }
                            K::Token | K::Comment => {}
                            _ if is_field(c, t.f_type) => w.type_ref(c, RefKind::Type, 255),
                            _ => w.visit(c),
                        }
                    });
                }
                K::Token | K::Comment | K::Modifier => {}
                _ => w.visit(c),
            }
        });
    }

    /// The initializer of a variable declarator: the named child after the name.
    fn declarator_value(&self, d: Node<'t>) -> Option<Node<'t>> {
        let nm_at = child(d, self.t.f_name).map_or(usize::MAX, |x| x.start_byte());
        for i in 0..d.named_child_count() as u32 {
            let Some(ch) = d.named_child(i) else { break };
            if ch.start_byte() != nm_at && !matches!(self.k(ch), K::Comment | K::ArgumentList | K::TuplePattern) {
                return Some(ch);
            }
        }
        None
    }

    /// End of a signature that stops before an initializer: before the `=`.
    fn trim_assign(&self, mut end: usize) -> usize {
        let src = self.cx.src;
        while end > 0 && src.get(end - 1).is_some_and(|b| b.is_ascii_whitespace()) {
            end -= 1;
        }
        if end > 0 && src.get(end - 1) == Some(&b'=') {
            end -= 1;
            while end > 0 && src.get(end - 1).is_some_and(|b| b.is_ascii_whitespace()) {
                end -= 1;
            }
        }
        end
    }

    /// Walk the children of a field declarator (the cursor's node) except its name. A method
    /// group stored directly into the field is a binding into the slot `Owner.field`.
    fn initializer_slot(&mut self, c: &mut TreeCursor<'t>, nm_at: usize, owner: u32, field: u32) {
        self.cx.push_scope();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if ch.start_byte() == nm_at => {}
                _ => {
                    let from = w.cx.facts.refs.len();
                    w.visit(c);
                    if let Some(m) = w.method_group(ch, from) {
                        let slot = w.joined(b"", owner, b".", field);
                        w.cx.add_ref(m, RefKind::Bind, line_of(ch), slot, 255);
                    }
                }
            }
        });
        self.cx.pop_scope();
    }

    fn property(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let indexer = self.k(n) == K::IndexerDecl;
        let event = self.k(n) == K::EventDecl;
        let nm = if indexer { None } else { self.decl_name(n) };
        if !indexer && nm.is_none() {
            return self.children(c);
        }
        let m = self.modifiers(n);
        let ty_node = child(n, t.f_type);
        let (ty, _) = ty_node.map_or((NONE, 0), |x| self.type_name(x, false));
        let accessors = child(n, t.f_accessors);
        let value = child(n, t.f_value);
        let iface = self.classes.last().is_some_and(|cl| cl.flags & CF_IFACE != 0);
        // An accessor list without a body anywhere is part of the signature (`{ get; set; }`).
        let bodiless = accessors.is_some_and(|a| (0..a.named_child_count() as u32).all(|i| a.named_child(i).is_none_or(|x| child(x, t.f_body).is_none())));
        let expr_body = value.filter(|v| self.cx.src.get(v.start_byte()..v.start_byte() + 2) == Some(b"=>"));
        let sig_end = match (accessors, expr_body) {
            (Some(a), _) if bodiless => a.end_byte(),
            (Some(a), _) => a.start_byte(),
            (None, Some(v)) => v.start_byte(),
            _ => self.before_semicolon(n),
        };
        let sig_end = self.trim_assign(sig_end);
        let name = match nm {
            Some(x) => self.cx.name(x),
            None => self.cx.intern(b"this"),
        };
        let parent = self.cx.encl();
        let mut flags = self.visibility(m);
        if bodiless && expr_body.is_none() && (iface || m.bits & (M_ABSTRACT | M_EXTERN) != 0) {
            flags |= dflag::DECL;
            if iface || m.bits & M_ABSTRACT != 0 {
                flags |= dflag::ABSTRACT;
            }
        } else if m.bits & M_ABSTRACT != 0 {
            flags |= dflag::ABSTRACT;
        }
        if event || self.callable_type(ty_node) {
            flags |= dflag::FNPTR;
        }
        let d = self.cx.def(name, DefKind::Property, n, sig_end);
        self.finish_def(d, parent, flags, m.sig_start, sig_end);
        self.cx.def_mut(d).type_name = ty;
        self.cx.push_def(d);
        self.cx.push_scope();
        let nm_at = nm.map_or(usize::MAX, |x| x.start_byte());
        let owner = self.class_name();
        let (fa, fv, fp) = (t.f_accessors, t.f_value, t.f_parameters);
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment | K::Modifier => {}
                _ if ch.start_byte() == nm_at => {}
                _ if is_field(c, fa) => w.accessors(c),
                _ if is_field(c, fv) => {
                    let saved = std::mem::replace(&mut w.target_ty, ty);
                    let from = w.cx.facts.refs.len();
                    w.visit(c);
                    if expr_body.is_none()
                        && let Some(mth) = w.method_group(ch, from)
                    {
                        let slot = w.joined(b"", owner, b".", name);
                        w.cx.add_ref(mth, RefKind::Bind, line_of(ch), slot, 255);
                    }
                    w.target_ty = saved;
                }
                _ if is_field(c, fp) => {
                    let (min, max) = w.params(c);
                    let dm = w.cx.def_mut(d);
                    dm.arity_min = min;
                    dm.arity_max = max;
                }
                _ if is_field(c, t.f_type) => w.type_ref(c, RefKind::Type, 255),
                _ => w.visit(c),
            }
        });
        self.cx.pop_scope();
        self.cx.pop_def();
    }

    /// `{ get { .. } set { .. } }`: the bodies belong to the property; setters and event
    /// accessors see the implicit parameter `value`.
    fn accessors(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            let ch = c.node();
            if w.k(ch) != K::Accessor {
                return;
            }
            w.cx.push_scope();
            let value = w.cx.intern(b"value");
            w.cx.declare(value, enc_local(NONE, 0));
            w.each(c, |w, c| {
                let x = c.node();
                match w.k(x) {
                    K::Token | K::Comment | K::Modifier | K::Identifier => {}
                    _ => w.visit(c),
                }
            });
            w.cx.pop_scope();
        });
    }

    fn method(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, mk: MK) {
        let t = self.t;
        let nm = self.decl_name(n);
        if nm.is_none() && !matches!(mk, MK::Operator | MK::Conversion) {
            return self.children(c);
        }
        let m = self.modifiers(n);
        let body = child(n, t.f_body);
        let sig_end = match body {
            Some(b) => b.start_byte(),
            None => self.before_semicolon(n),
        };
        let name = match mk {
            MK::Dtor => {
                let own = nm.map_or(NONE, |x| self.cx.name(x));
                self.joined(b"~", own, b"", NONE)
            }
            MK::Operator => match child(n, t.f_operator) {
                Some(op) => {
                    let mut buf = std::mem::take(&mut self.cx.s.buf);
                    buf.clear();
                    buf.extend_from_slice(b"operator");
                    buf.extend_from_slice(self.cx.text(op));
                    let id = self.cx.intern(&buf);
                    self.cx.s.buf = buf;
                    id
                }
                None => return self.children(c),
            },
            MK::Conversion => {
                let ty = child(n, t.f_type).map_or(NONE, |x| self.dotted(x));
                self.joined(b"operator ", ty, b"", NONE)
            }
            _ => self.cx.name(nm.unwrap_or(n)),
        };
        let parent = self.cx.encl();
        let iface = self.classes.last().is_some_and(|cl| cl.flags & CF_IFACE != 0);
        // A destructor is never called by name and takes no access modifier.
        let mut flags = if matches!(mk, MK::Local | MK::Dtor) {
            0
        } else if mk == MK::Delegate {
            self.visibility(m) | dflag::FNPTR
        } else {
            // An explicit interface implementation is reached through the interface only.
            let explicit = (0..n.named_child_count() as u32).any(|i| n.named_child(i).is_some_and(|x| self.k(x) == K::ExplicitIface));
            if explicit { 0 } else { self.visibility(m) }
        };
        if body.is_none() && mk != MK::Delegate {
            flags |= dflag::DECL;
            if m.bits & M_ABSTRACT != 0 || (iface && m.bits & (M_EXTERN | M_STATIC) == 0) {
                flags |= dflag::ABSTRACT;
            }
        }
        if m.bits & M_ASYNC != 0 {
            flags |= dflag::ASYNC;
        }
        if m.bits & M_TEST != 0 && mk == MK::Method {
            flags |= dflag::TEST;
        }
        let kind = match mk {
            MK::Local => DefKind::Function,
            MK::Delegate => DefKind::Class,
            _ if self.classes.is_empty() => DefKind::Function,
            _ => DefKind::Method,
        };
        let d = self.cx.def(name, kind, n, sig_end);
        self.finish_def(d, parent, flags, m.sig_start, sig_end);
        if mk == MK::Local {
            self.local_fns.push(name);
        }
        self.cx.push_def(d);
        self.cx.push_scope();
        let tp_mark = self.tparams.len();
        self.declare_tparams(n);
        let ret = match mk {
            MK::Method | MK::Local | MK::Delegate | MK::Operator => {
                child(n, t.f_returns).or_else(|| child(n, t.f_type)).map_or(NONE, |x| self.type_name(x, true).0)
            }
            MK::Conversion => child(n, t.f_type).map_or(NONE, |x| self.type_name(x, false).0),
            _ => NONE,
        };
        self.cx.def_mut(d).type_name = ret;
        let nm_at = if matches!(mk, MK::Operator | MK::Conversion) { usize::MAX } else { nm.map_or(usize::MAX, |x| x.start_byte()) };
        let fn_mark = self.local_fns.len();
        let (fp, fb) = (t.f_parameters, t.f_body);
        let cls = self.classes.last().copied();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment | K::Modifier | K::TypeParameterList => {}
                _ if ch.start_byte() == nm_at => {}
                _ if is_field(c, fp) => {
                    let (min, max) = w.params(c);
                    let dm = w.cx.def_mut(d);
                    dm.arity_min = min;
                    dm.arity_max = max;
                }
                // The body shares the scope of the parameters.
                _ if is_field(c, fb) => {
                    if w.k(ch) == K::Block {
                        w.block_inner(c);
                    } else {
                        w.children(c);
                    }
                }
                K::CtorInit => w.ctor_init(c, cls),
                _ if is_field(c, t.f_returns) || is_field(c, t.f_type) => w.type_ref(c, RefKind::Type, 255),
                _ => w.visit(c),
            }
        });
        self.local_fns.truncate(fn_mark);
        self.tparams.truncate(tp_mark);
        self.cx.pop_scope();
        self.cx.pop_def();
    }

    /// `: base(..)` / `: this(..)`: a call of a constructor of the base class or of the type.
    fn ctor_init(&mut self, c: &mut TreeCursor<'t>, cls: Option<Class>) {
        let t = self.t;
        let mut target = NONE;
        let mut line = 0;
        let mut nargs = 255u8;
        self.each(c, |w, c| {
            let ch = c.node();
            let id = ch.kind_id();
            if id == t.kw_this {
                target = cls.map_or(NONE, |cl| cl.own);
                line = line_of(ch);
            } else if id == t.kw_base {
                target = cls.map_or(NONE, |cl| cl.sup);
                line = line_of(ch);
            } else if w.k(ch) == K::ArgumentList {
                nargs = w.arguments(c, target);
            }
        });
        if target != NONE && !is_stop(self.cx.name_bytes(target)) {
            self.cx.add_ref(target, RefKind::Call, line, target, nargs);
        }
    }

    // ----- parameters and locals -----

    /// Declare the parameters under the cursor (a `parameter_list` or
    /// `bracketed_parameter_list`) in the current scope. Returns the arity: parameters with a
    /// default value are optional, `params` makes the list variadic, and the `this` parameter
    /// of an extension method is the receiver.
    fn params(&mut self, c: &mut TreeCursor<'t>) -> (u8, u8) {
        let t = self.t;
        let mut count = 0u32;
        let mut optional = 0u32;
        let mut variadic = false;
        let n = c.node();
        // `params T[] xs` is not wrapped in a `parameter` node: its type and name hang off the
        // list itself.
        let loose_name = child(n, t.f_name);
        if let Some(nm) = loose_name {
            variadic = true;
            count += 1;
            let (ty, dims) = child(n, t.f_type).map_or((NONE, 0), |x| self.type_name(x, false));
            let name = self.cx.name(nm);
            self.cx.declare(name, enc_local(ty, dims));
        }
        let loose_at = loose_name.map_or(usize::MAX, |x| x.start_byte());
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Parameter => {
                    let (receiver, has_default, params) = w.param_mods(ch);
                    if !receiver {
                        count += 1;
                    }
                    if has_default {
                        optional += 1;
                    }
                    if params {
                        variadic = true;
                    }
                    w.param(ch, c);
                }
                _ if ch.start_byte() == loose_at => {}
                _ if is_field(c, t.f_type) => w.type_ref(c, RefKind::Type, 255),
                K::AttributeList => w.visit(c),
                _ => {}
            }
        });
        let count = count.min(254) as u8;
        let min = count.saturating_sub(optional.min(254) as u8);
        if variadic { (min.saturating_sub(1), 255) } else { (min, count) }
    }

    /// (`this` receiver, has a default value, `params`) for a parameter.
    fn param_mods(&self, p: Node<'t>) -> (bool, bool, bool) {
        let mut receiver = false;
        let mut params = false;
        let mut has_default = false;
        let nm_end = child(p, self.t.f_name).map_or(usize::MAX, |x| x.end_byte());
        for i in 0..p.child_count() {
            let Some(ch) = p.child(i) else { break };
            match self.k(ch) {
                K::Modifier => match self.cx.text(ch) {
                    b"this" => receiver = true,
                    b"params" => params = true,
                    _ => {}
                },
                K::Token if ch.start_byte() >= nm_end && self.cx.text(ch) == b"=" => has_default = true,
                _ => {}
            }
        }
        (receiver, has_default, params)
    }

    fn param(&mut self, p: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let nm = child(p, t.f_name).filter(|x| x.end_byte() > x.start_byte());
        if let Some(nm) = nm {
            let (ty, dims) = child(p, t.f_type).map_or((NONE, 0), |x| self.type_name(x, false));
            let name = self.cx.name(nm);
            self.cx.declare(name, enc_local(ty, dims));
        }
        self.children_except(c, nm.map_or(usize::MAX, |x| x.start_byte()));
    }

    /// A block: local functions declared anywhere in it are visible from its start.
    fn block(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        self.block_inner(c);
        self.cx.pop_scope();
    }

    fn block_inner(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        let mark = self.local_fns.len();
        for i in 0..n.named_child_count() as u32 {
            let Some(ch) = n.named_child(i) else { break };
            if self.k(ch) == K::LocalFunction
                && let Some(nm) = self.decl_name(ch)
            {
                let name = self.cx.name(nm);
                self.local_fns.push(name);
            }
        }
        self.children(c);
        self.local_fns.truncate(mark);
    }

    /// `T a = x, b;` / `var a = x;`.
    fn variable_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let ty_node = child(n, t.f_type);
        let var = ty_node.is_some_and(|x| self.k(x) == K::ImplicitType || (self.k(x) == K::Identifier && self.cx.text(x) == b"var"));
        let (ty, dims) = if var { (NONE, 0) } else { ty_node.map_or((NONE, 0), |x| self.type_name(x, false)) };
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::VariableDeclarator => {
                    if var {
                        w.var_declarator(c);
                    } else {
                        w.declarator(c, ty, dims);
                    }
                }
                _ if is_field(c, t.f_type) => w.type_ref(c, RefKind::Type, 255),
                _ => w.visit(c),
            }
        });
    }

    /// `name = init` with a declared type. The name is in scope in its own initializer (it is
    /// declared first), and a target-typed `new()` there creates the declared type.
    fn declarator(&mut self, c: &mut TreeCursor<'t>, ty: u32, dims: u8) {
        let d = c.node();
        let nm = child(d, self.t.f_name).filter(|x| x.end_byte() > x.start_byte());
        if let Some(nm) = nm {
            let name = self.cx.name(nm);
            self.cx.declare(name, enc_local(ty, dims));
        }
        let saved = std::mem::replace(&mut self.target_ty, if dims == 0 { ty } else { NONE });
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if nm.is_some_and(|x| x.start_byte() == ch.start_byte()) => {}
                K::TuplePattern => w.designation(c),
                _ => w.visit(c),
            }
        });
        self.target_ty = saved;
    }

    /// `var name = init`: typed by whatever the initializer denotes; `var (a, b) = init`
    /// declares untyped locals.
    fn var_declarator(&mut self, c: &mut TreeCursor<'t>) {
        let d = c.node();
        let nm = child(d, self.t.f_name).filter(|x| x.end_byte() > x.start_byte());
        let nm_at = nm.map_or(usize::MAX, |x| x.start_byte());
        let mut out = (NONE, 0u8);
        let saved = std::mem::replace(&mut self.target_ty, NONE);
        let mut seen_value = false;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if ch.start_byte() == nm_at => {}
                K::TuplePattern => w.designation(c),
                K::ArgumentList => w.visit(c),
                _ if !seen_value => {
                    seen_value = true;
                    let r = w.recv(c);
                    out = match r.k {
                        RK::Val | RK::This => (w.qual(r), r.dims),
                        _ => (NONE, 0),
                    };
                    // `qual` hides the element type of an array: keep it with its dimensions.
                    if r.dims > 0 && r.k == RK::Val {
                        out.0 = if r.fld == NONE { r.base } else { w.path(r.base, r.fld, r.call) };
                    }
                }
                _ => w.visit(c),
            }
        });
        self.target_ty = saved;
        if let Some(nm) = nm {
            let name = self.cx.name(nm);
            let v = if out.0 == NONE { enc_local(NONE, 0) } else { enc_local(out.0, out.1) };
            self.cx.declare(name, v);
        }
    }

    fn foreach(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        self.cx.push_scope();
        let ty_node = child(n, t.f_type);
        let var = ty_node.is_some_and(|x| self.k(x) == K::ImplicitType);
        let (ty, dims) = if var { (NONE, 0) } else { ty_node.map_or((NONE, 0), |x| self.type_name(x, false)) };
        let left = child(n, t.f_left);
        let right_at = child(n, t.f_right).map_or(usize::MAX, |x| x.start_byte());
        let (fl, ft) = (t.f_left, t.f_type);
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if is_field(c, fl) => {}
                _ if is_field(c, ft) => w.type_ref(c, RefKind::Type, 255),
                _ => {
                    let at = ch.start_byte();
                    w.visit(c);
                    // The variable is in scope in the body only, not in the iterated expression.
                    if at == right_at
                        && let Some(l) = left
                    {
                        w.declare_left(l, ty, dims);
                    }
                }
            }
        });
        self.cx.pop_scope();
    }

    /// The variable part of a `foreach`: a name, a tuple of names, a declaration.
    fn declare_left(&mut self, l: Node<'t>, ty: u32, dims: u8) {
        match self.k(l) {
            K::Identifier => {
                let name = self.cx.name(l);
                self.cx.declare(name, enc_local(ty, dims));
            }
            _ => self.declare_names(l, 0),
        }
    }

    /// Declare every identifier under a designation (`(a, (b, c))`) as an untyped local.
    fn declare_names(&mut self, n: Node<'t>, depth: u32) {
        if depth > 8 {
            return;
        }
        for i in 0..n.named_child_count() as u32 {
            let Some(ch) = n.named_child(i) else { break };
            match self.k(ch) {
                K::Identifier => {
                    let name = self.cx.name(ch);
                    self.cx.declare(name, enc_local(NONE, 0));
                }
                K::Designation | K::TuplePattern | K::DeclarationExpression | K::Argument | K::TupleExpression | K::VarPattern => {
                    self.declare_names(ch, depth + 1)
                }
                _ => {}
            }
        }
    }

    fn catch_decl(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let nm = child(n, t.f_name);
        let ty = child(n, t.f_type).map_or(NONE, |x| self.type_name(x, false).0);
        if let Some(nm) = nm {
            let name = self.cx.name(nm);
            self.cx.declare(name, enc_local(ty, 0));
        }
        self.children_except(c, nm.map_or(usize::MAX, |x| x.start_byte()));
    }

    fn lambda(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        let saved = std::mem::replace(&mut self.target_ty, NONE);
        self.children(c);
        self.target_ty = saved;
        self.cx.pop_scope();
    }

    /// `T x` in a pattern or `out T x` / `out var x` in an argument: the binding stays visible
    /// until the enclosing scope ends, which covers both `if (o is T t) { t.. }` and
    /// `if (!(o is T t)) return; t..`.
    fn declaration_pattern(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let ty_node = child(n, t.f_type);
        let (ty, dims) = match ty_node {
            Some(x) if self.k(x) != K::ImplicitType => self.type_name(x, false),
            _ => (NONE, 0),
        };
        let nm = child(n, t.f_name);
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if is_field(c, t.f_type) => w.type_ref(c, RefKind::Type, 255),
                _ if nm.is_some_and(|x| x.start_byte() == ch.start_byte()) => {}
                K::Designation => w.designation(c),
                _ => w.visit(c),
            }
        });
        if let Some(nm) = nm {
            let name = self.cx.name(nm);
            self.cx.declare(name, enc_local(ty, dims));
        }
    }

    /// `T { A: 1, B.C: 2 } name`, `(x, y) name`: the property names are members of the type.
    fn recursive_pattern(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let ty_node = child(n, t.f_type);
        let (ty, dims) = ty_node.map_or((NONE, 0), |x| self.type_name(x, false));
        let nm = child(n, t.f_name);
        let saved = std::mem::replace(&mut self.init_ty, ty);
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if is_field(c, t.f_type) => w.type_ref(c, RefKind::Type, 255),
                _ if nm.is_some_and(|x| x.start_byte() == ch.start_byte()) => {}
                K::Designation => w.designation(c),
                K::PropertyPatternClause => w.children(c),
                _ => {
                    // Positional subpatterns are not property names.
                    let inner = std::mem::replace(&mut w.init_ty, NONE);
                    w.visit(c);
                    w.init_ty = inner;
                }
            }
        });
        self.init_ty = saved;
        if let Some(nm) = nm {
            let name = self.cx.name(nm);
            self.cx.declare(name, enc_local(ty, dims));
        }
    }

    /// `A: pattern` in a property pattern: `A` is a member of the type the pattern matches.
    fn subpattern(&mut self, c: &mut TreeCursor<'t>) {
        let mut first = true;
        let ty = self.init_ty;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::Identifier if std::mem::take(&mut first) && ty != NONE || (first && ch.next_sibling().is_some()) => {
                    first = false;
                    let name = w.cx.name(ch);
                    w.cx.add_ref(name, RefKind::MemberRead, line_of(ch), ty, 255);
                }
                _ => {
                    first = false;
                    let inner = std::mem::replace(&mut w.init_ty, NONE);
                    w.visit(c);
                    w.init_ty = inner;
                }
            }
        });
    }

    /// Every name under a `var` pattern or a deconstruction designation is a new local.
    fn designation(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        if matches!(self.k(n), K::ListPattern) {
            let nm = child(n, self.t.f_name);
            self.each(c, |w, c| {
                let ch = c.node();
                if nm.is_some_and(|x| x.start_byte() == ch.start_byte()) || matches!(w.k(ch), K::Token | K::Comment) {
                    return;
                }
                w.visit(c);
            });
            if let Some(nm) = nm {
                let name = self.cx.name(nm);
                self.cx.declare(name, enc_local(NONE, 0));
            }
            return;
        }
        self.declare_names(n, 0);
    }

    /// `is X` with a bare name: a type (`is Foo`) or a constant in scope.
    fn constant_pattern(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Identifier => {
                    let text = w.cx.text(ch);
                    let id = w.cx.intern(text);
                    if w.cx.local_type(id).is_none() && (type_like(text) || w.known_type(id)) && !w.ns_known(text) {
                        w.type_ident(ch, RefKind::Type, NONE, 255);
                    } else {
                        w.identifier(ch, false);
                    }
                }
                _ => w.visit(c),
            }
        });
    }

    fn as_is(&mut self, c: &mut TreeCursor<'t>) {
        let f_right = self.t.f_right;
        self.each(c, |w, c| {
            if is_field(c, f_right) {
                w.type_ref(c, RefKind::Type, 255);
            } else if !matches!(w.k(c.node()), K::Token | K::Comment) {
                w.visit(c);
            }
        });
    }

    fn labeled(&mut self, c: &mut TreeCursor<'t>) {
        let mut first = true;
        self.each(c, |w, c| {
            let k = w.k(c.node());
            if k == K::Token || k == K::Comment {
                return;
            }
            // The label itself is not a reference.
            if std::mem::take(&mut first) && k == K::Identifier {
                return;
            }
            w.visit(c);
        });
    }

    /// A LINQ query: its range variables are locals of the query.
    fn query(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                // `.. into g select g.Key`: the continuation variable.
                K::Identifier => {
                    let name = w.cx.name(ch);
                    w.cx.declare(name, enc_local(NONE, 0));
                }
                _ => w.visit(c),
            }
        });
        self.cx.pop_scope();
    }

    /// `from T x in source`: the source is walked before `x` is declared.
    fn query_from(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let nm = child(n, t.f_name);
        let ty = child(n, t.f_type).map_or(NONE, |x| self.type_name(x, false).0);
        self.children_except(c, nm.map_or(usize::MAX, |x| x.start_byte()));
        if let Some(nm) = nm {
            let name = self.cx.name(nm);
            self.cx.declare(name, enc_local(ty, 0));
        }
    }

    /// `join x in s on a equals b`, `let x = e`, `into g`: the first name is a new range
    /// variable.
    fn range_clause(&mut self, c: &mut TreeCursor<'t>) {
        let mut first = true;
        let ft = self.t.f_type;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if is_field(c, ft) => w.type_ref(c, RefKind::Type, 255),
                K::Identifier if std::mem::take(&mut first) => {
                    let name = w.cx.name(ch);
                    w.cx.declare(name, enc_local(NONE, 0));
                }
                _ => {
                    first = false;
                    w.visit(c);
                }
            }
        });
    }

    /// `#if COND`: the condition names preprocessor symbols, not code.
    fn preproc(&mut self, c: &mut TreeCursor<'t>) {
        let f_condition = self.t.f_condition;
        self.each(c, |w, c| {
            if !is_field(c, f_condition) {
                w.visit(c);
            }
        });
    }

    // ----- attributes -----

    fn attribute_list(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Attribute => w.attribute(ch, c),
                K::Preproc => w.preproc(c),
                _ => {}
            }
        });
    }

    /// `[Name(args, Prop = v)]`: an annotation by the attribute class (`NameAttribute`), and a
    /// write of each named property of it.
    fn attribute(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(nm) = child(n, t.f_name) else { return };
        let last = self.last_name(nm);
        let text = self.cx.text(last);
        if text.is_empty() {
            return;
        }
        let stem = text.strip_suffix(b"Attribute").unwrap_or(text);
        let mut owner = NONE;
        if !is_stop_attribute(stem) {
            let name = if text.ends_with(b"Attribute") {
                self.cx.intern(text)
            } else {
                let mut buf = std::mem::take(&mut self.cx.s.buf);
                buf.clear();
                buf.extend_from_slice(text);
                buf.extend_from_slice(b"Attribute");
                let id = self.cx.intern(&buf);
                self.cx.s.buf = buf;
                id
            };
            let qual = match self.k(nm) {
                K::QualifiedName => match child(nm, t.f_qualifier) {
                    Some(q) => match self.classify(q, 1) {
                        Scope::Namespace => self.dotted(q),
                        Scope::Type => {
                            let (p, _) = self.type_name(q, false);
                            self.simple_of(p)
                        }
                    },
                    None => NONE,
                },
                _ => NONE,
            };
            self.cx.add_ref(name, RefKind::Annotation, line_of(last), qual, 255);
            owner = if qual == NONE { name } else { self.joined(b"", qual, b"|", name) };
        }
        self.each(c, |w, c| {
            if w.k(c.node()) != K::AttributeArgList {
                return;
            }
            w.each(c, |w, c| {
                let ch = c.node();
                if w.k(ch) != K::AttributeArg {
                    return;
                }
                let key = child(ch, t.f_name);
                if let (Some(k), true) = (key, owner != NONE) {
                    let name = w.cx.name(k);
                    w.cx.add_ref(name, RefKind::MemberWrite, line_of(k), owner, 255);
                }
                w.children_except(c, key.map_or(usize::MAX, |k| k.start_byte()));
            });
        });
    }

    // ----- expressions -----

    /// A bare name in an expression. Locals emit nothing; anything else is a member of the type
    /// that declares it in this file, or of the enclosing type (inherited, or imported with
    /// `using static`).
    fn identifier(&mut self, n: Node<'t>, write: bool) {
        if n.end_byte() == n.start_byte() {
            return;
        }
        let name = self.cx.name(n);
        let qual = match self.cx.local_type(name) {
            Some(v) if !is_member(v) => return,
            Some(v) => self.owner_at(dec_level(v)),
            None => self.class_name(),
        };
        let kind = if write { RefKind::MemberWrite } else { RefKind::MemberRead };
        self.cx.add_ref(name, kind, line_of(n), qual, 255);
    }

    /// `Name<T>` used as a value (a generic method group): the name, then its type arguments.
    fn generic_value(&mut self, c: &mut TreeCursor<'t>) -> Recv {
        let mut r = Recv::UNKNOWN;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Identifier => {
                    let text = w.cx.text(ch);
                    let id = w.cx.intern(text);
                    if w.cx.local_type(id).is_none() && (type_like(text) || w.known_type(id)) {
                        // `List<int>.Empty`, `Comparer<T>.Default`: a generic type.
                        w.type_ident(ch, RefKind::Type, NONE, 255);
                        r = Recv::ty(w.alias_of(id).map_or(id, |a| a.path));
                    } else {
                        w.identifier(ch, false);
                    }
                }
                K::TypeArgumentList => w.type_args(c),
                _ => {}
            }
        });
        r
    }

    /// A bare name used as a receiver or qualifier (`x` in `x.F`, `x.M()`).
    fn recv_ident(&mut self, n: Node<'t>) -> Recv {
        let text = self.cx.text(n);
        if text.is_empty() {
            return Recv::UNKNOWN;
        }
        let name = self.cx.intern(text);
        let line = line_of(n);
        if let Some(v) = self.cx.local_type(name) {
            let (ty, dims) = (dec_ty(v), dec_dims(v));
            if !is_member(v) {
                return Recv::val(ty, dims);
            }
            let own = self.owner_at(dec_level(v));
            self.cx.add_ref(name, RefKind::MemberRead, line, own, 255);
            return if own != NONE { Recv { k: RK::Val, base: own, fld: name, call: false, dims } } else { Recv::val(ty, dims) };
        }
        if let Some(a) = self.alias_of(name) {
            if !is_stop(self.cx.name_bytes(a.name)) {
                self.cx.add_ref(a.name, RefKind::Type, line, a.module, 255);
            }
            return Recv::ty(a.path);
        }
        if let Some((_, bound)) = self.tparam(name) {
            // `T.Create()` on a type parameter (static abstract members): its constraint.
            return if bound == NONE { Recv::UNKNOWN } else { Recv::ty(bound) };
        }
        if self.known_type(name) {
            self.cx.add_ref(name, RefKind::Type, line, NONE, 255);
            return Recv::ty(name);
        }
        if self.ns_known(text) {
            return Recv { k: RK::Pkg, base: name, fld: NONE, call: false, dims: 0 };
        }
        if type_like(text) {
            let q = self.class_name();
            if is_stop(text) || q == NONE {
                return Recv::ty(name);
            }
            // A type of another file, or a member inherited from one: C# looks the name up as
            // a member first. The path `C::Name` says so; the resolver falls back on the type
            // `Name` when `C` has no such member.
            self.cx.add_ref(name, RefKind::Type, line, NONE, 255);
            self.cx.add_ref(name, RefKind::MemberRead, line, q, 255);
            return Recv::member(q, name, false);
        }
        // Not declared in this file: an inherited member.
        let q = self.class_name();
        self.cx.add_ref(name, RefKind::MemberRead, line, q, 255);
        Recv::member(q, name, false)
    }

    /// Walk the expression under the cursor as a receiver: emit its references and return what
    /// it denotes.
    fn recv(&mut self, c: &mut TreeCursor<'t>) -> Recv {
        let n = c.node();
        let t = self.t;
        let id = n.kind_id();
        if id == t.kw_this {
            return Recv { k: RK::This, base: self.class_name(), fld: NONE, call: false, dims: 0 };
        }
        if id == t.kw_base {
            return Recv::SUPER;
        }
        match self.k(n) {
            K::Identifier => self.recv_ident(n),
            K::GenericName => self.generic_value(c),
            K::MemberAccess => self.member_access(c, false),
            K::Invocation => self.invocation(n, c),
            K::ConditionalAccess => self.conditional_access(c),
            K::MemberBinding => self.member_binding(n, false),
            K::ObjectCreation => {
                let ty = self.object_creation(n, c);
                Recv::val(ty, 0)
            }
            K::ImplicitObjectCreation => {
                let ty = self.implicit_creation(n, c);
                Recv::val(ty, 0)
            }
            K::Parenthesized | K::Await => {
                let mut r = Recv::UNKNOWN;
                self.each(c, |w, c| {
                    if !matches!(w.k(c.node()), K::Token | K::Comment) {
                        r = w.recv(c);
                    }
                });
                r
            }
            K::PostfixUnary if self.cx.src.get(n.end_byte().wrapping_sub(1)) == Some(&b'!') => {
                // `x!`: the null-forgiving operator changes nothing about `x`.
                let mut r = Recv::UNKNOWN;
                self.each(c, |w, c| {
                    if !matches!(w.k(c.node()), K::Token | K::Comment) {
                        r = w.recv(c);
                    }
                });
                r
            }
            K::CastExpression => {
                let (ty, dims) = child(n, t.f_type).map_or((NONE, 0), |x| self.type_name(x, false));
                self.children(c);
                Recv::val(ty, dims)
            }
            K::AsExpression => {
                let (ty, dims) = child(n, t.f_right).map_or((NONE, 0), |x| self.type_name(x, false));
                self.as_is(c);
                Recv::val(ty, dims)
            }
            K::ElementAccess => {
                let mut r = Recv::UNKNOWN;
                let fe = t.f_expression;
                self.each(c, |w, c| {
                    if is_field(c, fe) {
                        r = w.recv(c);
                    } else if !matches!(w.k(c.node()), K::Token | K::Comment) {
                        w.visit(c);
                    }
                });
                // An element of an array: one dimension less. Indexers of other types return
                // something the receiver's type declares, which is not tracked.
                match r.k {
                    RK::Val | RK::This if r.dims > 0 => Recv { dims: r.dims - 1, ..r },
                    _ => Recv::UNKNOWN,
                }
            }
            K::StringLiteral => Recv::val(self.string_id(), 0),
            K::Interpolated => {
                self.children(c);
                Recv::val(self.string_id(), 0)
            }
            K::PredefinedType => match self.cx.text(n) {
                b"string" => Recv::ty(self.string_id()),
                text => {
                    let id = self.cx.intern(text);
                    Recv::ty(id)
                }
            },
            K::QualifiedName => {
                let (p, _) = self.type_name(n, false);
                self.type_ref(c, RefKind::Type, 255);
                if p == NONE { Recv::UNKNOWN } else { Recv::ty(p) }
            }
            K::AliasQualifiedName => {
                // `global::System`: the root of a namespace or type name.
                let mut r = Recv::UNKNOWN;
                let f_name = t.f_name;
                self.each(c, |w, c| {
                    if is_field(c, f_name) {
                        let x = c.node();
                        let text = w.cx.text(x);
                        let id = w.cx.intern(text);
                        r = if w.known_type(id) || !w.ns_known(text) && type_like(text) {
                            w.type_ident(x, RefKind::Type, NONE, 255);
                            Recv::ty(id)
                        } else {
                            Recv { k: RK::Pkg, base: id, fld: NONE, call: false, dims: 0 }
                        };
                    }
                });
                r
            }
            _ => {
                self.visit(c);
                Recv::UNKNOWN
            }
        }
    }

    /// The name of a member access or binding: `F` or `F<T>` (the type arguments are walked).
    fn member_name(&mut self, c: &mut TreeCursor<'t>) -> Option<Node<'t>> {
        let n = c.node();
        match self.k(n) {
            K::Identifier => Some(n),
            K::GenericName => {
                let head = n.named_child(0);
                self.each(c, |w, c| {
                    if w.k(c.node()) == K::TypeArgumentList {
                        w.type_args(c);
                    }
                });
                head.filter(|h| self.k(*h) == K::Identifier)
            }
            _ => None,
        }
    }

    /// `object.Member`. Emits the member reference and returns what the whole access denotes.
    fn member_access(&mut self, c: &mut TreeCursor<'t>, write: bool) -> Recv {
        let t = self.t;
        let mut r = Recv::UNKNOWN;
        let mut f: Option<Node<'t>> = None;
        let (fe, fname) = (t.f_expression, t.f_name);
        self.each(c, |w, c| {
            if is_field(c, fe) {
                r = w.recv(c);
            } else if is_field(c, fname) {
                f = w.member_name(c);
            }
        });
        let Some(f) = f else { return Recv::UNKNOWN };
        self.member_of(r, f, write)
    }

    /// The reference of member `f` of receiver `r`, and what the access denotes.
    fn member_of(&mut self, r: Recv, f: Node<'t>, write: bool) -> Recv {
        let text = self.cx.text(f);
        if text.is_empty() {
            return Recv::UNKNOWN;
        }
        let name = self.cx.intern(text);
        let line = line_of(f);
        let kind = if write { RefKind::MemberWrite } else { RefKind::MemberRead };
        match r.k {
            RK::Pkg => {
                // `System.IO.File`: a namespace as long as the file knows one by that name, then
                // the type, qualified by the namespace.
                let dotted = self.joined(b"", r.base, b".", name);
                if self.ns_known(self.cx.name_bytes(dotted)) && !self.known_type(name) {
                    return Recv { k: RK::Pkg, base: dotted, fld: NONE, call: false, dims: 0 };
                }
                if !is_stop(text) {
                    self.cx.add_ref(name, RefKind::Type, line, r.base, 255);
                }
                Recv::ty(self.joined(b"", r.base, b"|", name))
            }
            RK::Type if self.known_type(name) && !write => {
                // A nested type this file declares: `Outer.Inner.Create()`.
                let outer = self.simple_of(r.base);
                self.cx.add_ref(name, RefKind::Type, line, outer, 255);
                Recv::ty(self.path(r.base, name, false))
            }
            RK::Type => {
                self.cx.add_ref(name, kind, line, r.base, 255);
                Recv::member(r.base, name, false)
            }
            RK::Super => {
                let q = self.super_qual();
                self.cx.add_ref(name, kind, line, q, 255);
                let own = self.class_name();
                Recv::member(own, name, false)
            }
            _ => {
                let q = self.qual(r);
                self.cx.add_ref(name, kind, line, q, 255);
                Recv::member(q, name, false)
            }
        }
    }

    /// `a?.b`, `a?.M()`, `a?[i]`: the bindings inside apply to the receiver `a`.
    fn conditional_access(&mut self, c: &mut TreeCursor<'t>) -> Recv {
        let fc = self.t.f_condition;
        let mut out = Recv::UNKNOWN;
        let saved = self.binding;
        self.each(c, |w, c| {
            let ch = c.node();
            if is_field(c, fc) {
                w.binding = w.recv(c);
            } else if !matches!(w.k(ch), K::Token | K::Comment) {
                out = w.recv(c);
            }
        });
        self.binding = saved;
        out
    }

    /// `.b` inside `a?.b`.
    fn member_binding(&mut self, n: Node<'t>, write: bool) -> Recv {
        let Some(nm) = child(n, self.t.f_name) else { return Recv::UNKNOWN };
        let nm = self.last_name(nm);
        let r = self.binding;
        self.member_of(r, nm, write)
    }

    /// A call. Emits the call reference and returns what the call denotes as a receiver: the
    /// declared result of the method, looked up on the receiver's type at query time.
    fn invocation(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) -> Recv {
        let t = self.t;
        let Some(func) = child(n, t.f_function) else {
            self.children(c);
            return Recv::UNKNOWN;
        };
        let (ff, fa) = (t.f_function, t.f_arguments);
        // What the callee is: (receiver or None for a bare name, name node).
        let mut callee: Option<(Option<Recv>, Node<'t>)> = None;
        let mut nargs = 255u8;
        let mut nameof = false;
        self.each(c, |w, c| {
            let ch = c.node();
            if is_field(c, ff) {
                callee = w.callee(c, func);
                if let Some((None, nm)) = callee
                    && w.cx.text(nm) == b"nameof"
                {
                    nameof = true;
                }
            } else if is_field(c, fa) {
                let name = callee.map_or(NONE, |(_, nm)| w.cx.name(nm));
                if nameof {
                    w.nameof_args(c);
                } else {
                    nargs = w.arguments(c, name);
                }
            } else if !matches!(w.k(ch), K::Token | K::Comment) {
                w.visit(c);
            }
        });
        if nameof {
            return Recv::val(self.string_id(), 0);
        }
        let Some((r, nm)) = callee else { return Recv::UNKNOWN };
        let name = self.cx.name(nm);
        let line = line_of(nm);
        let owner = match r {
            None => {
                if self.local_fns.contains(&name) {
                    self.cx.add_ref(name, RefKind::Call, line, NONE, nargs);
                    let root = self.joined(b"", name, b"()", NONE);
                    return Recv::val(root, 0);
                }
                match self.cx.local_type(name) {
                    // A local holding a delegate.
                    Some(v) if !is_member(v) => return Recv::UNKNOWN,
                    // A field, property or event holding a delegate: a call through the slot.
                    Some(v) if !is_method(v) => {
                        let q = self.owner_at(dec_level(v));
                        self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
                        return Recv::UNKNOWN;
                    }
                    Some(v) => {
                        let q = self.owner_at(dec_level(v));
                        self.cx.add_ref(name, RefKind::Call, line, q, nargs);
                        q
                    }
                    None => {
                        let q = self.class_name();
                        self.cx.add_ref(name, RefKind::Call, line, q, nargs);
                        q
                    }
                }
            }
            // `_cb.Invoke(..)`, `Changed?.Invoke(..)`: a call through the delegate slot.
            Some(r) if self.cx.text(nm) == b"Invoke" && r.k == RK::Val && r.fld != NONE && !r.call => {
                self.cx.add_ref(r.fld, RefKind::MemberCall, line, r.base, nargs);
                return Recv::UNKNOWN;
            }
            Some(r) if r.k == RK::Type || r.k == RK::Pkg => {
                self.cx.add_ref(name, RefKind::Call, line, r.base, nargs);
                r.base
            }
            Some(r) if r.k == RK::Super => {
                let q = self.super_qual();
                self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
                // The result is declared by an ancestor; the lookup starts at the type itself
                // (an override keeps the result type) and walks up from there.
                self.class_name()
            }
            Some(r) => {
                let q = self.qual(r);
                self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
                q
            }
        };
        if owner == NONE { Recv::UNKNOWN } else { Recv { k: RK::Val, base: owner, fld: name, call: true, dims: 0 } }
    }

    /// The callee of a call (the cursor is on the `function` child): the receiver it is called
    /// on (`None` for a bare name) and its name. `None` when the callee is no name (a call of a
    /// call result, of a lambda).
    fn callee(&mut self, c: &mut TreeCursor<'t>, func: Node<'t>) -> Option<(Option<Recv>, Node<'t>)> {
        let t = self.t;
        match self.k(func) {
            K::Identifier => Some((None, func)),
            K::GenericName => {
                let head = func.named_child(0).filter(|h| self.k(*h) == K::Identifier)?;
                self.each(c, |w, c| {
                    if w.k(c.node()) == K::TypeArgumentList {
                        w.type_args(c);
                    }
                });
                Some((None, head))
            }
            K::MemberAccess => {
                let mut r = Recv::UNKNOWN;
                let mut f = None;
                let (fe, fname) = (t.f_expression, t.f_name);
                self.each(c, |w, c| {
                    if is_field(c, fe) {
                        r = w.recv(c);
                    } else if is_field(c, fname) {
                        f = w.member_name(c);
                    }
                });
                f.map(|f| (Some(r), f))
            }
            K::ConditionalAccess => {
                // `a?.M(..)`: the binding names the method.
                let fc = t.f_condition;
                let saved = self.binding;
                let mut out = None;
                self.each(c, |w, c| {
                    let ch = c.node();
                    if is_field(c, fc) {
                        w.binding = w.recv(c);
                    } else if w.k(ch) == K::MemberBinding {
                        let r = w.binding;
                        let nm = child(ch, t.f_name);
                        out = nm.and_then(|x| {
                            let x = w.last_name(x);
                            if w.k(x) == K::Identifier { Some((Some(r), x)) } else { None }
                        });
                        w.each(c, |w, c| {
                            if w.k(c.node()) == K::GenericName {
                                w.each(c, |w, c| {
                                    if w.k(c.node()) == K::TypeArgumentList {
                                        w.type_args(c);
                                    }
                                });
                            }
                        });
                    } else if !matches!(w.k(ch), K::Token | K::Comment) {
                        // `a?.b.M()`: the member access chain hangs off the binding.
                        if w.k(ch) == K::MemberAccess {
                            out = w.callee(c, ch);
                        } else {
                            w.visit(c);
                        }
                    }
                });
                self.binding = saved;
                out
            }
            K::MemberBinding => {
                let nm = child(func, t.f_name).map(|x| self.last_name(x)).filter(|x| self.k(*x) == K::Identifier)?;
                Some((Some(self.binding), nm))
            }
            _ => {
                self.visit(c);
                None
            }
        }
    }

    /// `nameof(x.Y)`: the operand is read, not evaluated; a bare type name is a type.
    fn nameof_args(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            if w.k(c.node()) != K::Argument {
                return;
            }
            w.each(c, |w, c| {
                let x = c.node();
                match w.k(x) {
                    K::Identifier => {
                        let text = w.cx.text(x);
                        let id = w.cx.intern(text);
                        if w.cx.local_type(id).is_none() && w.known_type(id) {
                            w.type_ident(x, RefKind::Type, NONE, 255);
                        } else {
                            w.identifier(x, false);
                        }
                    }
                    K::Token | K::Comment => {}
                    _ => w.visit(c),
                }
            });
        });
    }

    /// Walk the argument list of a call of `callee` and count the arguments. The count is
    /// unknown when the list did not parse. A method group handed over directly is a callback
    /// registration: besides its own reference it is recorded as an argument of `callee`.
    fn arguments(&mut self, c: &mut TreeCursor<'t>, callee: u32) -> u8 {
        let mut n = 0u32;
        let mut broken = false;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::Error => {
                    broken = true;
                    w.visit(c);
                }
                K::Argument => {
                    n += 1;
                    w.argument(c, callee);
                }
                _ => {
                    n += 1;
                    w.visit(c);
                }
            }
        });
        if broken { 255 } else { n.min(254) as u8 }
    }

    fn argument(&mut self, c: &mut TreeCursor<'t>, callee: u32) {
        let key = child(c.node(), self.t.f_name);
        let key_at = key.map_or(usize::MAX, |k| k.start_byte());
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment | K::Modifier => {}
                // `name: value`: the parameter name is not a reference.
                _ if ch.start_byte() == key_at => {}
                _ => {
                    let from = w.cx.facts.refs.len();
                    w.visit(c);
                    if callee != NONE
                        && let Some(m) = w.method_group(ch, from)
                    {
                        w.cx.add_ref(m, RefKind::Arg, line_of(ch), callee, 255);
                    }
                }
            }
        });
    }

    /// When the expression `e` (whose references start at `from`) is a method group, its name:
    /// a bare name that is a method of an enclosing type, or a member access (`this.Run`,
    /// `T.Parse`, `x.Handle`) or an unknown bare name spelled like a method.
    fn method_group(&self, e: Node<'t>, from: usize) -> Option<u32> {
        match self.k(e) {
            K::Identifier | K::MemberAccess | K::GenericName => {}
            _ => return None,
        }
        let r = self.cx.facts.refs.get(from..)?.last()?;
        if r.kind != RefKind::MemberRead || r.line != line_of(self.last_name(child(e, self.t.f_name).unwrap_or(e))) {
            return None;
        }
        let text = self.cx.name_bytes(r.name);
        match self.cx.local_type(r.name) {
            Some(v) if self.k(e) != K::MemberAccess => is_method(v).then_some(r.name),
            _ if starts_upper(text) => Some(r.name),
            _ => None,
        }
    }

    /// `new T(..) { .. }`. Returns the type path of `T`.
    fn object_creation(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) -> u32 {
        let t = self.t;
        let ty_node = child(n, t.f_type);
        let ty = ty_node.map_or(NONE, |x| self.type_name(x, false).0);
        let mut at: Option<usize> = None;
        let (ft, fa, fi) = (t.f_type, t.f_arguments, t.f_initializer);
        self.each(c, |w, c| {
            let ch = c.node();
            if is_field(c, ft) {
                let from = w.cx.facts.refs.len();
                w.type_ref(c, RefKind::New, 255);
                at = w.cx.facts.refs.get(from..).and_then(|rs| rs.iter().position(|r| r.kind == RefKind::New)).map(|i| from + i);
            } else if is_field(c, fa) {
                let callee = w.simple_of(ty);
                let nargs = w.arguments(c, callee);
                if let Some(r) = at.and_then(|i| w.cx.facts.refs.get_mut(i)) {
                    r.nargs = nargs;
                }
            } else if is_field(c, fi) {
                let saved = std::mem::replace(&mut w.init_ty, ty);
                w.initializer_expression(c);
                w.init_ty = saved;
            } else if !matches!(w.k(ch), K::Token | K::Comment) {
                w.visit(c);
            }
        });
        if at.is_some_and(|i| self.cx.facts.refs.get(i).is_some_and(|r| r.nargs == 255)) && child(n, t.f_arguments).is_none() {
            // `new T { .. }` runs the parameterless constructor.
            if let Some(r) = at.and_then(|i| self.cx.facts.refs.get_mut(i)) {
                r.nargs = 0;
            }
        }
        ty
    }

    /// `new(..)`: the type the context declares, when it is visible.
    fn implicit_creation(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) -> u32 {
        let ty = self.target_ty;
        let mut nargs = 255u8;
        let line = line_of(n);
        let callee = self.simple_of(ty);
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::ArgumentList => nargs = w.arguments(c, callee),
                K::InitializerExpression => {
                    let saved = std::mem::replace(&mut w.init_ty, ty);
                    w.initializer_expression(c);
                    w.init_ty = saved;
                }
                K::Token | K::Comment => {}
                _ => w.visit(c),
            }
        });
        if ty != NONE && !is_stop(self.cx.name_bytes(callee)) {
            let q = self.qual_of_path(ty);
            self.cx.add_ref(callee, RefKind::New, line, q, nargs);
        }
        ty
    }

    /// `{ A = 1, B = { .. }, [k] = v }` after `new T`: the assigned names are members of `T`.
    /// A collection initializer (`{ 1, 2 }`) is a list of plain expressions.
    fn initializer_expression(&mut self, c: &mut TreeCursor<'t>) {
        let ty = std::mem::replace(&mut self.init_ty, NONE);
        let target = std::mem::replace(&mut self.target_ty, NONE);
        let fl = self.t.f_left;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::Assignment => {
                    let left = child(ch, fl);
                    match left {
                        Some(l) if w.k(l) == K::Identifier => {
                            let name = w.cx.name(l);
                            let line = line_of(l);
                            w.cx.add_ref(name, RefKind::MemberWrite, line, ty, 255);
                            // The value: a nested initializer applies to the member's type, which
                            // is not known here.
                            let mut first = true;
                            w.each(c, |w, c| {
                                let x = c.node();
                                if matches!(w.k(x), K::Token | K::Comment) {
                                    return;
                                }
                                if std::mem::take(&mut first) {
                                    return;
                                }
                                let from = w.cx.facts.refs.len();
                                w.visit(c);
                                if let Some(m) = w.method_group(x, from) {
                                    let slot = w.joined(b"", ty, b".", name);
                                    let slot = if ty == NONE { w.joined(b".", name, b"", NONE) } else { slot };
                                    w.cx.add_ref(m, RefKind::Bind, line_of(x), slot, 255);
                                }
                            });
                        }
                        _ => w.visit(c),
                    }
                }
                _ => w.visit(c),
            }
        });
        self.target_ty = target;
    }

    /// `new { A = x, B }`: the property names are new, the values are expressions.
    fn anonymous_object(&mut self, c: &mut TreeCursor<'t>) {
        let fl = self.t.f_left;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::Assignment => w.each(c, |w, c| {
                    if !is_field(c, fl) && !matches!(w.k(c.node()), K::Token | K::Comment) {
                        w.visit(c);
                    }
                }),
                _ => w.visit(c),
            }
        });
    }

    /// `x with { A = 1 }`: the assigned names are members of the type of `x`.
    fn with_expression(&mut self, c: &mut TreeCursor<'t>) {
        let mut ty = NONE;
        let mut first = true;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if std::mem::take(&mut first) => {
                    let r = w.recv(c);
                    if matches!(r.k, RK::Val | RK::This) {
                        ty = w.qual(r);
                    }
                }
                K::WithInitializer => {
                    let mut name_seen = false;
                    w.each(c, |w, c| {
                        let x = c.node();
                        match w.k(x) {
                            K::Token | K::Comment => {}
                            K::Identifier if !std::mem::take(&mut name_seen) => {
                                let name = w.cx.name(x);
                                w.cx.add_ref(name, RefKind::MemberWrite, line_of(x), ty, 255);
                                name_seen = true;
                            }
                            _ => {
                                name_seen = false;
                                w.visit(c);
                            }
                        }
                    });
                }
                _ => w.visit(c),
            }
        });
    }

    fn assignment(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let op = child(n, t.f_operator).map_or(0, |o| o.kind_id());
        let binds = op == t.op_assign || op == t.op_add_assign || op == t.op_sub_assign || op == t.op_coalesce_assign;
        let mut slot: Option<(u32, u32)> = None;
        let mut target = NONE;
        let (fl, fr) = (t.f_left, t.f_right);
        self.each(c, |w, c| {
            let ch = c.node();
            if is_field(c, fl) {
                let from = w.cx.facts.refs.len();
                // The declared type of the target, for `x = new()`.
                if w.k(ch) == K::Identifier {
                    let name = w.cx.name(ch);
                    if let Some(v) = w.cx.local_type(name)
                        && dec_dims(v) == 0
                    {
                        target = dec_ty(v);
                    }
                }
                w.lvalue(c);
                if matches!(w.k(ch), K::Identifier | K::MemberAccess | K::MemberBinding) {
                    let last = w.cx.facts.refs.get(from..).and_then(|rs| rs.last());
                    slot = last.filter(|r| r.kind == RefKind::MemberWrite).map(|r| (r.qual, r.name));
                }
            } else if is_field(c, fr) {
                let from = w.cx.facts.refs.len();
                let saved = std::mem::replace(&mut w.target_ty, target);
                w.visit(c);
                w.target_ty = saved;
                if binds && let Some(m) = w.method_group(ch, from) {
                    // A method stored into a member, or added to an event: a binding into the
                    // slot `Owner.member`.
                    let s = match slot {
                        Some((owner, f)) if owner != NONE => Some(w.joined(b"", owner, b".", f)),
                        Some((_, f)) => Some(w.joined(b".", f, b"", NONE)),
                        None => None,
                    };
                    if let Some(s) = s {
                        w.cx.add_ref(m, RefKind::Bind, line_of(ch), s, 255);
                    }
                }
            }
        });
    }

    /// `a + b + c + ...` nests to the left, one level per operand. Long concatenations would
    /// run into the depth guard, so the left spine is walked with a loop instead of recursion.
    fn binary(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        let mut levels = 0u32;
        while self.k(c.node()) == K::BinaryExpression && c.goto_first_child() {
            levels += 1;
        }
        if levels == 0 {
            return;
        }
        self.cx.depth += 1;
        // The cursor is on the leftmost operand. Visit it and what follows it, then climb one
        // level and go on with the operator and right operand found there.
        'climb: loop {
            loop {
                self.visit(c);
                if !c.goto_next_sibling() {
                    break;
                }
            }
            loop {
                c.goto_parent();
                levels -= 1;
                if levels == 0 {
                    break 'climb;
                }
                if c.goto_next_sibling() {
                    break;
                }
            }
        }
        self.cx.depth -= 1;
    }

    /// `x++`, `--x` write their operand; other unary operators only read it.
    fn unary(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let text = self.cx.text(n);
        let incdec = text.starts_with(b"++") || text.starts_with(b"--") || text.ends_with(b"++") || text.ends_with(b"--");
        self.each(c, |w, c| {
            if matches!(w.k(c.node()), K::Token | K::Comment) {
                return;
            }
            if incdec { w.lvalue(c) } else { w.visit(c) }
        });
    }

    /// An assignment target. Storing into an element counts as a write of the collection, as
    /// in the Java extractor.
    fn lvalue(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        match self.k(n) {
            K::Identifier => self.identifier(n, true),
            K::MemberAccess => {
                self.member_access(c, true);
            }
            K::MemberBinding => {
                self.member_binding(n, true);
            }
            K::ElementAccess => {
                let fe = self.t.f_expression;
                self.each(c, |w, c| {
                    if is_field(c, fe) {
                        w.lvalue(c);
                    } else if !matches!(w.k(c.node()), K::Token | K::Comment) {
                        w.visit(c);
                    }
                });
            }
            K::Parenthesized | K::TupleExpression | K::Argument => self.each(c, |w, c| {
                if !matches!(w.k(c.node()), K::Token | K::Comment) {
                    w.lvalue(c);
                }
            }),
            _ => self.visit(c),
        }
    }

    /// Names directly under an ERROR node are fragments the parser could not place (sometimes
    /// keywords read as names): noise, not references.
    fn error(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.facts.parse_errors += 1;
        self.each(c, |w, c| {
            if !matches!(w.k(c.node()), K::Identifier) {
                w.visit(c);
            }
        });
    }
}

/// A better tree for a file whose conditional code (`#if`) did not parse: `src` parsed again
/// with the directive lines blanked, keeping every branch, or failing that with only the first
/// branch of every conditional. Blanking keeps every byte offset and line, so the new tree
/// describes `src` itself. `None` when neither has fewer errors than `tree`.
fn reparse_conditionals(src: &[u8], tree: &Tree) -> Option<Tree> {
    let before = error_count(tree);
    let all = parse_blanked(src, false)?;
    let n_all = error_count(&all);
    if n_all == 0 {
        return Some(all);
    }
    let first = parse_blanked(src, true);
    let n_first = first.as_ref().map_or(usize::MAX, error_count);
    if n_first < n_all && n_first < before {
        first
    } else if n_all < before {
        Some(all)
    } else {
        None
    }
}

/// Parse `src` with its preprocessor conditionals blanked: the directive lines, and with
/// `first_only` the `#elif` / `#else` branches too.
fn parse_blanked(src: &[u8], first_only: bool) -> Option<Tree> {
    thread_local! {
        static PARSER: std::cell::RefCell<Option<tree_sitter::Parser>> = const { std::cell::RefCell::new(None) };
    }
    let mut clean = src.to_vec();
    // One flag per open conditional: is its first branch over?
    let mut levels: Vec<bool> = Vec::new();
    let mut changed = false;
    let mut at = 0usize;
    while at < src.len() {
        let end = memchr::memchr(b'\n', &src[at..]).map_or(src.len(), |i| at + i);
        let line = &src[at..end];
        let text = line.trim_ascii_start();
        let directive = text.strip_prefix(b"#").map(|d| d.trim_ascii_start());
        let blank = match directive {
            Some(d) if d.starts_with(b"if") => {
                levels.push(false);
                true
            }
            Some(d) if d.starts_with(b"elif") || d.starts_with(b"else") => {
                if let Some(l) = levels.last_mut() {
                    *l = true;
                }
                true
            }
            Some(d) if d.starts_with(b"endif") => {
                levels.pop();
                true
            }
            _ => first_only && levels.iter().any(|&l| l),
        };
        if blank {
            for b in &mut clean[at..end] {
                if *b != b'\r' {
                    *b = b' ';
                }
            }
            changed = true;
        }
        at = end + 1;
    }
    if !changed {
        return None;
    }
    PARSER.with(|p| {
        let mut p = p.borrow_mut();
        if p.is_none() {
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(&tree_sitter_c_sharp::LANGUAGE.into()).ok()?;
            *p = Some(parser);
        }
        p.as_mut()?.parse(&clean, None)
    })
}

fn error_count(tree: &Tree) -> usize {
    let mut n = 0;
    let mut c = tree.walk();
    loop {
        let node = c.node();
        if node.is_error() || node.is_missing() {
            n += 1;
        }
        if node.has_error() && c.goto_first_child() {
            continue;
        }
        loop {
            if c.goto_next_sibling() {
                break;
            }
            if !c.goto_parent() {
                return n;
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Namespace,
    Type,
}

/// Append a dotted name as written, without embedded whitespace or comments.
fn push_dotted(text: &[u8], buf: &mut Vec<u8>) {
    if !text.iter().any(|&b| b <= b' ' || b == b'/') {
        buf.extend_from_slice(text);
        return;
    }
    let mut i = 0;
    while i < text.len() {
        let b = text[i];
        if b == b'/' && text.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < text.len() && !(text[i] == b'*' && text[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        if b == b'/' && text.get(i + 1) == Some(&b'/') {
            while i < text.len() && text[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b > b' ' {
            buf.push(b);
        }
        i += 1;
    }
}

#[inline]
fn starts_upper(b: &[u8]) -> bool {
    b.first().is_some_and(|c| c.is_ascii_uppercase())
}

/// Does the name look like a type (`Foo`, `IOUtils`, `T`) rather than a constant (`MAX_SIZE`)
/// or a variable?
#[inline]
fn type_like(b: &[u8]) -> bool {
    match b.first() {
        Some(c) if c.is_ascii_uppercase() => b.len() == 1 || b[1..].iter().any(|x| x.is_ascii_lowercase()),
        _ => false,
    }
}

/// Names that never produce a type reference: the `System` basics.
fn is_stop(b: &[u8]) -> bool {
    matches!(
        b,
        b"String"
            | b"Object"
            | b"Int32"
            | b"Int64"
            | b"Boolean"
            | b"Double"
            | b"Console"
            | b"Math"
            | b"Convert"
            | b"Type"
            | b"Func"
            | b"Action"
            | b"Task"
            | b"ValueTask"
            | b"Nullable"
            | b"StringBuilder"
            | b"Enumerable"
            | b"Array"
            | b"var"
            | b"dynamic"
            | b"nameof"
    )
}

/// Attributes of the platform that say nothing about the code's own types.
fn is_stop_attribute(b: &[u8]) -> bool {
    matches!(
        b,
        b"Serializable"
            | b"NonSerialized"
            | b"Obsolete"
            | b"Flags"
            | b"MethodImpl"
            | b"DebuggerStepThrough"
            | b"DebuggerDisplay"
            | b"DebuggerBrowsable"
            | b"DebuggerHidden"
            | b"EditorBrowsable"
            | b"CompilerGenerated"
            | b"Conditional"
            | b"ThreadStatic"
            | b"StructLayout"
            | b"AttributeUsage"
            | b"CLSCompliant"
            | b"ComVisible"
            | b"SuppressMessage"
            | b"NotNull"
            | b"MaybeNull"
            | b"NotNullWhen"
            | b"MaybeNullWhen"
            | b"AllowNull"
            | b"DoesNotReturn"
    )
}
