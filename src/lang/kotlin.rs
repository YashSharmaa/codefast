//! Kotlin extractor (tree-sitter-kotlin-ng).
//!
//! Kotlin is close to Java in what a name can denote, with three differences that shape this
//! walker: a file holds top-level functions and properties next to its classes, a function may
//! extend a type it is not declared in (`fun Order.describe()`), and every class may carry a
//! companion object whose members are written like static members (`Order.create()`). The
//! walker tracks:
//!   * locals with their declared or inferred types (parameters, `val` / `var`, lambda
//!     parameters and the implicit `it`, destructuring, `for`, `catch`, `when` subjects), so that
//!     `x.foo()` carries the receiver type,
//!   * the members each enclosing class declares in this file (properties, primary constructor
//!     properties, enum entries and methods, companion members included), collected when its
//!     body is entered because members may be used before they are declared,
//!   * what the file declares and imports at top level, so that a bare `foo()` is told apart
//!     as a call of a top-level function, a member of the class around it, or a constructor,
//!   * the implicit receivers: the class, the receiver of an extension function, and the
//!     receiver of a scope function lambda (`x.apply { .. }`, `with(x) { .. }`) when its type
//!     is known.
//!
//! Conventions the resolver can rely on (the same shapes as the Java extractor, so that Kotlin
//! and Java link to each other):
//!   * Definitions. The package is a `Namespace` named `a.b.c`, the first definition of its file.
//!     Top-level functions are `Function`s without parent, top-level properties `Variable`s
//!     (`Constant` for `const val`). An extension function is a `Method` whose container is the
//!     receiver type by name (`qual`), like a Go method. Classes, objects and companion objects
//!     are `Class`es, interfaces and annotation classes `Interface`s, enum classes `Enum`s with
//!     `EnumMember`s. Members of a companion object belong to the class around it: `parent` is
//!     that class, so that `Order.create()` finds them. Properties are `Field`s, or `Property`s
//!     when they have accessors, extend a type or live in an interface. Primary constructor
//!     parameters declared `val` / `var` are fields; the class carries the arity of its primary
//!     constructor; secondary constructors are methods named after their class. `private` is
//!     `LOCAL`, the default `public` is `EXPORTED`, `internal` and `protected` are neither.
//!   * Calls. A bare `foo()` is a `Call` qualified by the class around it that declares `foo` in
//!     this file, unqualified when the file declares or imports a top-level `foo`, else qualified
//!     by the innermost implicit receiver (the method may be inherited or an extension). `Foo(..)`
//!     with a name that is a type is a `New`. `x.foo()` / `x?.foo()` is a `MemberCall` on the
//!     type path of `x`, `Type.foo()` a `Call` qualified by the type (an object or a companion
//!     member), `super.foo()` a `MemberCall ^C`. A trailing lambda is an argument.
//!   * Supertypes. One with a constructor call is a class: `Extends`, plus the call of its
//!     constructor (`Call B` qualified `B`, as Java's `super(..)`; `New B` for an object
//!     expression). One without is `Implements` (`Extends` between interfaces).
//!   * Callable references. `T::foo`, `x::foo`, `this::foo` and `::foo` naming a member are a
//!     `MemberRead` of `foo` qualified like a receiver, `::foo` naming a top-level or local
//!     function a `Read`, `::Foo` a `New`. Handed directly to a call it is also an `Arg` of that
//!     call, stored directly into a property or a member a `Bind`.
//!   * Lambdas. A trailing lambda passed to a function that is not known to take a plain one
//!     (`forEach`, `map`, `let`, ..) may have a receiver of its own (`headers { append(..) }`):
//!     in it an unqualified name that no class of the file declares is left unqualified.
//!   * Imports. `import a.b.C` is target `a.b`, name `C`. A member of an object or a companion
//!     imported by name (`import a.b.C.foo`) makes `foo()` a call qualified `a.b|C`.

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
    SourceFile,
    Identifier,
    UserType,
    NullableType,
    NonNullableType,
    ParenthesizedType,
    FunctionType,
    TypeArguments,
    TypeModifiers,
    TypeParameters,
    TypeParameter,
    TypeConstraints,
    PackageHeader,
    Import,
    QualifiedIdentifier,
    FileAnnotation,
    ClassDecl,
    ObjectDecl,
    CompanionObject,
    ObjectLiteral,
    ClassBody,
    EnumEntry,
    PrimaryConstructor,
    ClassParameters,
    ClassParameter,
    SecondaryConstructor,
    ConstructorDelegationCall,
    AnonymousInitializer,
    DelegationSpecifiers,
    DelegationSpecifier,
    ConstructorInvocation,
    ExplicitDelegation,
    FunctionDecl,
    FunctionValueParameters,
    Parameter,
    ParameterModifiers,
    FunctionBody,
    PropertyDecl,
    VariableDeclaration,
    MultiVariableDeclaration,
    PropertyDelegate,
    Getter,
    Setter,
    Modifiers,
    Annotation,
    TypeAlias,
    Block,
    CallExpr,
    NavigationExpr,
    ValueArguments,
    ValueArgument,
    AnnotatedLambda,
    LambdaLiteral,
    LambdaParameters,
    AnonymousFunction,
    CallableReference,
    This,
    Super,
    Assignment,
    BinaryExpr,
    InfixExpr,
    UnaryExpr,
    AsExpr,
    Parenthesized,
    IndexExpr,
    ForStmt,
    CatchBlock,
    WhenExpr,
    WhenSubject,
    ReturnExpr,
    LabeledExpr,
    StringLiteral,
    Label,
}

struct Tables {
    kinds: KindTable<K>,
    f_name: u16,
    f_type: u16,
    f_label: u16,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let lang: tree_sitter::Language = tree_sitter_kotlin_ng::LANGUAGE.into();
        let special: &[(&str, K)] = &[
            ("ERROR", K::Error),
            ("source_file", K::SourceFile),
            ("identifier", K::Identifier),
            ("user_type", K::UserType),
            ("nullable_type", K::NullableType),
            ("non_nullable_type", K::NonNullableType),
            ("parenthesized_type", K::ParenthesizedType),
            ("function_type", K::FunctionType),
            ("type_arguments", K::TypeArguments),
            ("type_modifiers", K::TypeModifiers),
            ("type_parameters", K::TypeParameters),
            ("type_parameter", K::TypeParameter),
            ("type_constraint", K::TypeParameter),
            ("type_constraints", K::TypeConstraints),
            ("package_header", K::PackageHeader),
            ("import", K::Import),
            ("qualified_identifier", K::QualifiedIdentifier),
            ("file_annotation", K::FileAnnotation),
            ("class_declaration", K::ClassDecl),
            ("object_declaration", K::ObjectDecl),
            ("companion_object", K::CompanionObject),
            ("object_literal", K::ObjectLiteral),
            ("class_body", K::ClassBody),
            ("enum_class_body", K::ClassBody),
            ("enum_entry", K::EnumEntry),
            ("primary_constructor", K::PrimaryConstructor),
            ("class_parameters", K::ClassParameters),
            ("class_parameter", K::ClassParameter),
            ("secondary_constructor", K::SecondaryConstructor),
            ("constructor_delegation_call", K::ConstructorDelegationCall),
            ("anonymous_initializer", K::AnonymousInitializer),
            ("delegation_specifiers", K::DelegationSpecifiers),
            ("delegation_specifier", K::DelegationSpecifier),
            ("constructor_invocation", K::ConstructorInvocation),
            ("explicit_delegation", K::ExplicitDelegation),
            ("function_declaration", K::FunctionDecl),
            ("function_value_parameters", K::FunctionValueParameters),
            ("parameter", K::Parameter),
            ("parameter_modifiers", K::ParameterModifiers),
            ("function_body", K::FunctionBody),
            ("property_declaration", K::PropertyDecl),
            ("variable_declaration", K::VariableDeclaration),
            ("multi_variable_declaration", K::MultiVariableDeclaration),
            ("property_delegate", K::PropertyDelegate),
            ("getter", K::Getter),
            ("setter", K::Setter),
            ("modifiers", K::Modifiers),
            ("annotation", K::Annotation),
            ("type_alias", K::TypeAlias),
            ("block", K::Block),
            ("call_expression", K::CallExpr),
            ("navigation_expression", K::NavigationExpr),
            ("value_arguments", K::ValueArguments),
            ("value_argument", K::ValueArgument),
            ("annotated_lambda", K::AnnotatedLambda),
            ("lambda_literal", K::LambdaLiteral),
            ("lambda_parameters", K::LambdaParameters),
            ("anonymous_function", K::AnonymousFunction),
            ("callable_reference", K::CallableReference),
            ("this_expression", K::This),
            ("super_expression", K::Super),
            ("assignment", K::Assignment),
            ("binary_expression", K::BinaryExpr),
            ("infix_expression", K::InfixExpr),
            ("unary_expression", K::UnaryExpr),
            ("as_expression", K::AsExpr),
            ("parenthesized_expression", K::Parenthesized),
            ("index_expression", K::IndexExpr),
            ("for_statement", K::ForStmt),
            ("catch_block", K::CatchBlock),
            ("when_expression", K::WhenExpr),
            ("when_subject", K::WhenSubject),
            ("return_expression", K::ReturnExpr),
            ("labeled_expression", K::LabeledExpr),
            ("string_literal", K::StringLiteral),
            ("multiline_string_literal", K::StringLiteral),
            ("label", K::Label),
            ("line_comment", K::Comment),
            ("block_comment", K::Comment),
            // leaves and subtrees that never contain references
            ("number_literal", K::Skip),
            ("float_literal", K::Skip),
            ("character_literal", K::Skip),
            ("string_content", K::Skip),
            ("escape_sequence", K::Skip),
            ("shebang", K::Skip),
            ("use_site_target", K::Skip),
            ("visibility_modifier", K::Skip),
            ("class_modifier", K::Skip),
            ("function_modifier", K::Skip),
            ("inheritance_modifier", K::Skip),
            ("member_modifier", K::Skip),
            ("parameter_modifier", K::Skip),
            ("platform_modifier", K::Skip),
            ("property_modifier", K::Skip),
            ("reification_modifier", K::Skip),
            ("variance_modifier", K::Skip),
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
        Tables { kinds, f_name: field(&lang, "name"), f_type: field(&lang, "type"), f_label: field(&lang, "label") }
    })
}

// ---------------------------------------------------------------------------------------------
// Scope entries
// ---------------------------------------------------------------------------------------------
//
// The value stored per name in a scope packs what the name denotes:
//   bit 31      set for a member of an enclosing class declared in this file, clear for a local
//   bits 25-30  for members: index of the declaring class in `W::classes`
//   bit 24      a local function (called, not read)
//   bits 0-23   name id of the declared type path + 1, or 0 when unknown

const B_MEMBER: u32 = 1 << 31;
const B_LEVEL_SHIFT: u32 = 25;
const B_MAX_LEVEL: usize = 62;
const B_FN: u32 = 1 << 24;
const B_TY_MASK: u32 = B_FN - 1;

#[inline]
fn enc_local(ty: u32) -> u32 {
    if ty >= B_TY_MASK - 1 { 0 } else { ty + 1 }
}

#[inline]
fn enc_member(level: usize, ty: u32) -> u32 {
    B_MEMBER | ((level.min(B_MAX_LEVEL) as u32) << B_LEVEL_SHIFT) | enc_local(ty)
}

#[inline]
fn dec_level(v: u32) -> usize {
    ((v >> B_LEVEL_SHIFT) & 63) as usize
}

#[inline]
fn dec_ty(v: u32) -> u32 {
    let t = v & B_TY_MASK;
    if t == 0 { NONE } else { t - 1 }
}

// ---------------------------------------------------------------------------------------------
// Per-name knowledge of the file, indexed by file-local name id
// ---------------------------------------------------------------------------------------------

/// A top-level function of this file (not an extension).
const N_TOPFN: u16 = 1;
/// A top-level property of this file.
const N_TOPVAL: u16 = 1 << 1;
/// A type declared in this file or imported by name.
const N_TYPE: u16 = 1 << 2;
/// A top-level function or property imported by name.
const N_IMPVAL: u16 = 1 << 3;
/// The local name of an aliased import (`import a.b.C as D`).
const N_ALIAS: u16 = 1 << 4;
/// A member of an object or a companion imported by name (`import a.b.C.foo`).
const N_STATIC: u16 = 1 << 5;
/// The name of a companion object declared in this file.
const N_COMPANION: u16 = 1 << 6;

// ---------------------------------------------------------------------------------------------
// Walker state
// ---------------------------------------------------------------------------------------------

/// Members are implicitly abstract when they have no body (interface).
const CF_IFACE: u8 = 1;
/// Object expression or enum entry body: no name of its own.
const CF_ANON: u8 = 2;
/// An implicit receiver that is not a class of this file: the receiver of an extension
/// function or of a scope function lambda. Its members are not known here.
const CF_RECV: u8 = 4;
/// Annotation class: its constructor parameters are its elements.
const CF_ANNOT: u8 = 8;
/// A trailing lambda passed to a function this file does not know: it may have a receiver of
/// its own (`headers { append(..) }`), so an unqualified name in it that no class around it
/// declares has an unknown receiver.
const CF_UNKNOWN: u8 = 16;

#[derive(Clone, Copy)]
struct Class {
    /// Qualifier for unqualified calls and members that are not declared in this file: the
    /// innermost named class, or the type of the implicit receiver.
    name: u32,
    /// The class's own simple name; `NONE` for anonymous bodies and receiver scopes.
    own: u32,
    /// Static type of `this`.
    this_ty: u32,
    /// Type path of the superclass (the supertype written with a constructor call).
    sup: u32,
    /// `^name`, interned on first use.
    sup_q: u32,
    flags: u8,
    /// Range of the names of the functions the body declares in `W::methods`.
    methods: (u32, u32),
    /// Range of the primary constructor parameters in `W::params`.
    params: (u32, u32),
}

impl Class {
    fn recv(ty: u32) -> Class {
        Class { name: ty, own: NONE, this_ty: ty, sup: NONE, sup_q: NONE, flags: CF_RECV, methods: (0, 0), params: (0, 0) }
    }

    fn unknown() -> Class {
        Class { flags: CF_RECV | CF_UNKNOWN, ..Class::recv(NONE) }
    }
}

const M_PRIVATE: u32 = 1;
const M_PROTECTED: u32 = 1 << 1;
const M_INTERNAL: u32 = 1 << 2;
const M_ABSTRACT: u32 = 1 << 3;
const M_SUSPEND: u32 = 1 << 4;
const M_EXPECT: u32 = 1 << 5;
const M_EXTERNAL: u32 = 1 << 6;
const M_CONST: u32 = 1 << 7;
const M_ENUM: u32 = 1 << 8;
const M_ANNOT: u32 = 1 << 9;
const M_SEALED: u32 = 1 << 10;
const M_TEST: u32 = 1 << 11;

/// What the modifiers of a declaration say, and where its signature starts once leading
/// annotations are skipped.
#[derive(Clone, Copy)]
struct Mods {
    bits: u32,
    sig_start: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Top,
    Member,
    Local,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RK {
    /// A value whose static type path is `base` (+ `::fld`), `NONE` when unknown.
    Val,
    /// `this`.
    This,
    /// A type name: `base` is its type path.
    Type,
    /// A chain of lower-case names that is either a member path or a package prefix; the
    /// references emitted for it so far start at `mark` and are dropped if a type name follows.
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
    mark: u32,
}

impl Recv {
    const UNKNOWN: Recv = Recv { k: RK::Val, base: NONE, fld: NONE, call: false, mark: 0 };
    const SUPER: Recv = Recv { k: RK::Super, base: NONE, fld: NONE, call: false, mark: 0 };

    #[inline]
    fn val(ty: u32) -> Recv {
        Recv { k: RK::Val, base: ty, fld: NONE, call: false, mark: 0 }
    }

    #[inline]
    fn ty(name: u32) -> Recv {
        Recv { k: RK::Type, base: name, fld: NONE, call: false, mark: 0 }
    }

    #[inline]
    fn member(base: u32, fld: u32, call: bool) -> Recv {
        if base == NONE { Recv::UNKNOWN } else { Recv { k: RK::Val, base, fld, call, mark: 0 } }
    }
}

/// What a lambda passed to a call gets: the type of `it` (`let`, `also`) or of `this` (`apply`,
/// `run`, `with`) as a type path (`NONE` when unknown), and whether it is known to have no
/// receiver of its own (`forEach`, `map`, a lambda that is not a trailing argument).
#[derive(Clone, Copy)]
struct LamCtx {
    it: u32,
    this: u32,
    plain: bool,
}

impl LamCtx {
    const NONE: LamCtx = LamCtx { it: NONE, this: NONE, plain: false };
    const PLAIN: LamCtx = LamCtx { it: NONE, this: NONE, plain: true };

    fn after(name: &[u8]) -> LamCtx {
        if is_plain_lambda_fn(name) { LamCtx::PLAIN } else { LamCtx::NONE }
    }
}

/// The parts of a function declaration, read by position: the grammar names only `name`.
struct FnParts<'t> {
    name: Option<Node<'t>>,
    recv: Option<Node<'t>>,
    ret: Option<Node<'t>>,
    body: Option<Node<'t>>,
}

/// The parts of a property declaration.
struct PropParts<'t> {
    recv: Option<Node<'t>>,
    var: Option<Node<'t>>,
    multi: Option<Node<'t>>,
    init: Option<Node<'t>>,
    accessors: bool,
    delegate: bool,
    sig_end: usize,
}

struct W<'a, 't> {
    cx: Ctx<'a>,
    t: &'static Tables,
    /// Enclosing class bodies and implicit receivers, outermost first.
    classes: Vec<Class>,
    /// Names of the functions the enclosing class bodies declare.
    methods: Vec<u32>,
    /// Primary constructor parameters of the enclosing classes: (name, encoded local type,
    /// declared as a property).
    params: Vec<(u32, u32, bool)>,
    /// Type parameters in scope with the type of their bound (`NONE` when unbounded).
    tparams: Vec<(u32, u32)>,
    /// `N_*` bits per file-local name id.
    info: Vec<u16>,
    /// Aliased imports: (local name, target, imported name).
    aliases: Vec<(u32, u32, u32)>,
    /// Members of objects and companions imported by name: (name, type path of the owner, here
    /// `a.b|C`). The path is `NONE` when two imports bind the name to different owners.
    statics: Vec<(u32, u32)>,
    /// Receiver for the next lambda literal, set by the scope function call that takes it.
    lam: LamCtx,
    string_id: u32,
    _t: PhantomData<&'t ()>,
}

pub fn extract(_lang: Lang, src: &[u8], tree: &Tree, path: &str, facts: &mut FileFacts, scratch: &mut Scratch) {
    let cx = Ctx::new(src, path, facts, scratch);
    let mut w = W {
        cx,
        t: tables(),
        classes: Vec::new(),
        methods: Vec::new(),
        params: Vec::new(),
        tparams: Vec::new(),
        info: Vec::new(),
        aliases: Vec::new(),
        statics: Vec::new(),
        lam: LamCtx::NONE,
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

    /// Visit every child except tokens, comments and the node starting at byte `skip`.
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

    fn visit(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        match self.k(n) {
            K::Token | K::Skip | K::Comment | K::This | K::Super | K::QualifiedIdentifier | K::Label => {}
            K::SourceFile => self.source_file(c),
            K::Identifier => self.identifier(n, false),
            K::UserType => self.type_ref(c, RefKind::Type, 255),
            K::TypeParameter => self.type_parameter(c),
            K::PackageHeader => self.package(n),
            K::Import => self.import(n),
            K::Annotation | K::FileAnnotation => self.annotation(c),
            K::ClassDecl => self.class_decl(n, c),
            K::ObjectDecl => self.object_decl(n, c),
            K::CompanionObject => self.companion(n, c),
            K::ObjectLiteral => {
                self.object_literal(c);
            }
            K::FunctionDecl => self.function(n, c, Place::Local),
            K::PropertyDecl => self.property(n, c, Place::Local),
            K::TypeAlias => self.type_alias(n, c),
            K::CallExpr => {
                self.call(c, 0);
            }
            K::NavigationExpr => {
                self.navigation(c, false, false);
            }
            K::CallableReference => {
                self.callable_ref(c);
            }
            K::Assignment => self.assignment(c),
            K::BinaryExpr => self.binary(c),
            K::UnaryExpr => self.unary(c),
            K::InfixExpr => {
                self.infix(c);
            }
            K::AnnotatedLambda => self.lambda_arg(c, LamCtx::NONE),
            K::LambdaLiteral => self.lambda(c, LamCtx::PLAIN),
            K::AnonymousFunction => self.anon_function(c),
            K::Block => self.scoped(c),
            K::ForStmt => self.for_stmt(c),
            K::CatchBlock => self.catch_block(c),
            K::WhenExpr => self.when_expr(c),
            K::ReturnExpr => self.return_expr(n, c),
            K::LabeledExpr => self.labeled(c),
            K::ValueArguments => {
                self.arguments(c, NONE);
            }
            K::FunctionValueParameters => {
                self.params(c);
            }
            K::Parameter | K::VariableDeclaration => self.var_decl(c, NONE),
            K::Error => self.error(c),
            _ => self.children(c),
        }
    }

    // ----- small helpers -----

    /// The text of a node; a name in backticks (`` `in` ``, `` `my test` ``) without them.
    #[inline]
    fn tx(&self, n: Node<'t>) -> &'a [u8] {
        let t = self.cx.text(n);
        if t.len() > 2 && t[0] == b'`' && t[t.len() - 1] == b'`' { &t[1..t.len() - 1] } else { t }
    }

    /// Intern the name a node spells.
    #[inline]
    fn nm(&mut self, n: Node<'t>) -> u32 {
        let t = self.tx(n);
        self.cx.intern(t)
    }

    #[inline]
    fn info(&self, id: u32) -> u16 {
        self.info.get(id as usize).copied().unwrap_or(0)
    }

    fn set_info(&mut self, id: u32, bits: u16) {
        if id == NONE {
            return;
        }
        let i = id as usize;
        if self.info.len() <= i {
            self.info.resize(i + 1, 0);
        }
        self.info[i] |= bits;
    }

    #[inline]
    fn class_name(&self) -> u32 {
        self.classes.last().map_or(NONE, |c| c.name)
    }

    /// Qualifier for a member declared by the enclosing class at `level` (`NONE` for members of
    /// an anonymous body, which has no name).
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
        if base == NONE || fld == NONE {
            return NONE;
        }
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

    /// The simple name at the end of a type path: `Inner` of `Outer::Inner`, `List` of
    /// `java.util|List`.
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

    /// The qualifier a receiver contributes to a member reference.
    fn qual(&mut self, r: Recv) -> u32 {
        match r.k {
            RK::Super => self.super_qual(),
            RK::Type => r.base,
            _ if r.base == NONE => NONE,
            _ if r.fld == NONE => r.base,
            _ => self.path(r.base, r.fld, r.call),
        }
    }

    /// The type path a receiver denotes as a value (`NONE` for a type, `super` or nothing).
    fn value_ty(&mut self, r: Recv) -> u32 {
        match r.k {
            RK::Val | RK::This | RK::Pkg => self.qual(r),
            _ => NONE,
        }
    }

    /// A type a declaration can be given from its initializer: a constructed or cast type, not
    /// a call result or a member path.
    fn plain_type(&self, r: Recv) -> u32 {
        if !matches!(r.k, RK::Val | RK::Type) || r.fld != NONE || r.base == NONE {
            return NONE;
        }
        let b = self.cx.name_bytes(r.base);
        if b.first() == Some(&b'$') || b.contains(&b'(') { NONE } else { r.base }
    }

    /// `^Class`: a strict ancestor of the enclosing class. The super of an object expression is
    /// the type it was created from.
    fn super_qual(&mut self) -> u32 {
        let Some(i) = self.classes.iter().rposition(|cl| cl.flags & CF_RECV == 0) else { return NONE };
        let cl = self.classes[i];
        if cl.flags & CF_ANON != 0 {
            return cl.this_ty;
        }
        if cl.sup_q != NONE || cl.own == NONE {
            return cl.sup_q;
        }
        let q = self.joined(b"^", cl.own, b"", NONE);
        self.classes[i].sup_q = q;
        q
    }

    /// Qualifier of `this.name`: the type of `this`, except for a member the anonymous body
    /// declares itself, which no name can qualify.
    fn this_member_qual(&self, name: u32) -> u32 {
        let Some(i) = self.classes.iter().rposition(|cl| cl.flags & CF_UNKNOWN == 0) else { return NONE };
        let cl = &self.classes[i];
        if cl.flags & CF_ANON != 0
            && let Some(v) = self.cx.local_type(name)
            && v & B_MEMBER != 0
            && dec_level(v) == i
        {
            return NONE;
        }
        cl.this_ty
    }

    #[inline]
    fn alias_of(&self, name: u32) -> Option<(u32, u32)> {
        if self.info(name) & N_ALIAS == 0 {
            return None;
        }
        self.aliases.iter().rev().find(|a| a.0 == name).map(|a| (a.1, a.2))
    }

    #[inline]
    fn static_owner_of(&self, name: u32) -> u32 {
        if self.info(name) & N_STATIC == 0 {
            return NONE;
        }
        self.statics.iter().find(|x| x.0 == name).map_or(NONE, |x| x.1)
    }

    /// The class around the reference that declares a function `name` in this file: its name,
    /// or `NONE` for an anonymous body (the call is then unqualified). `None` when no class of
    /// this file around the reference declares one.
    fn method_owner(&self, name: u32) -> Option<u32> {
        for cl in self.classes.iter().rev() {
            if cl.flags & CF_RECV != 0 {
                continue;
            }
            let (a, b) = (cl.methods.0 as usize, cl.methods.1 as usize);
            if self.methods.get(a..b).is_some_and(|m| m.contains(&name)) {
                return Some(cl.own);
            }
        }
        None
    }

    /// The qualifier of a bare call or callable reference `name`: the class around it that
    /// declares it, nothing for a local or top-level function, else the innermost implicit
    /// receiver (an inherited member, an extension, or a top-level function of another file:
    /// the resolver looks there when the receiver has no such member).
    fn bare_qual(&self, name: u32) -> u32 {
        if let Some(v) = self.cx.local_type(name)
            && v & B_MEMBER == 0
        {
            return NONE;
        }
        let owner = self.static_owner_of(name);
        if owner != NONE {
            return owner;
        }
        if let Some(q) = self.method_owner(name) {
            return q;
        }
        if self.info(name) & (N_TOPFN | N_IMPVAL) != 0 {
            return NONE;
        }
        self.class_name()
    }

    /// The name of a declaration, unless the parser had to invent it (a zero-width MISSING node).
    fn decl_name(&self, n: Node<'t>) -> Option<Node<'t>> {
        child(n, self.t.f_name).filter(|x| x.end_byte() > x.start_byte())
    }

    /// The first identifier among the direct children of `n`.
    fn first_ident(&self, n: Node<'t>) -> Option<Node<'t>> {
        for i in 0..n.child_count().min(12) {
            let ch = n.child(i)?;
            if self.k(ch) == K::Identifier && ch.end_byte() > ch.start_byte() {
                return Some(ch);
            }
        }
        None
    }

    /// The first direct child of `n` that is a type.
    fn first_type(&self, n: Node<'t>) -> Option<Node<'t>> {
        (0..n.child_count().min(12)).filter_map(|i| n.child(i)).find(|&ch| self.is_type(ch))
    }

    #[inline]
    fn is_type(&self, n: Node<'t>) -> bool {
        matches!(self.k(n), K::UserType | K::NullableType | K::NonNullableType | K::ParenthesizedType | K::FunctionType)
    }

    /// Text of the first anonymous token among the direct children of `n` that is one of the
    /// navigation operators, or the empty string.
    fn nav_op(&self, n: Node<'t>) -> &'a [u8] {
        for i in 0..n.child_count().min(6) {
            let Some(ch) = n.child(i) else { break };
            if self.k(ch) == K::Token {
                let t = self.tx(ch);
                if matches!(t, b"." | b"?." | b"::") {
                    return t;
                }
            }
        }
        b""
    }

    /// Does a direct child token of `n` read `kw`?
    fn has_token(&self, n: Node<'t>, kw: &[u8]) -> bool {
        (0..n.child_count()).filter_map(|i| n.child(i)).any(|ch| self.k(ch) == K::Token && self.tx(ch) == kw)
    }

    // ----- modifiers -----

    fn modifiers(&self, n: Node<'t>) -> Mods {
        let mut m = Mods { bits: 0, sig_start: n.start_byte() };
        let Some(mods) = n.child(0).filter(|x| self.k(*x) == K::Modifiers) else { return m };
        let mut sig: Option<usize> = None;
        for i in 0..mods.child_count() {
            let Some(ch) = mods.child(i) else { break };
            match self.k(ch) {
                K::Annotation => {
                    if self.is_test_annotation(ch) {
                        m.bits |= M_TEST;
                    }
                }
                K::Comment => {}
                _ => {
                    if sig.is_none() {
                        sig = Some(ch.start_byte());
                    }
                    m.bits |= match self.tx(ch) {
                        b"private" => M_PRIVATE,
                        b"protected" => M_PROTECTED,
                        b"internal" => M_INTERNAL,
                        b"abstract" => M_ABSTRACT,
                        b"suspend" => M_SUSPEND,
                        b"expect" => M_EXPECT,
                        b"external" => M_EXTERNAL,
                        b"const" => M_CONST,
                        b"enum" => M_ENUM,
                        b"annotation" => M_ANNOT,
                        b"sealed" => M_SEALED,
                        _ => 0,
                    };
                }
            }
        }
        m.sig_start = match sig {
            Some(s) => s,
            None => {
                // Only annotations: the signature starts at whatever follows them.
                let mut s = mods.end_byte();
                for i in 1..n.child_count().min(8) {
                    let Some(ch) = n.child(i) else { break };
                    if self.k(ch) != K::Comment {
                        s = ch.start_byte();
                        break;
                    }
                }
                s
            }
        };
        m
    }

    fn is_test_annotation(&self, ann: Node<'t>) -> bool {
        let mut ty = None;
        for i in 0..ann.child_count() {
            let Some(ch) = ann.child(i) else { break };
            match self.k(ch) {
                K::UserType => ty = Some(ch),
                K::ConstructorInvocation => ty = self.first_type(ch),
                _ => {}
            }
        }
        let Some(ty) = ty.filter(|t| self.k(*t) == K::UserType) else { return false };
        let mut last = None;
        for i in 0..ty.child_count() {
            if let Some(ch) = ty.child(i).filter(|x| self.k(*x) == K::Identifier) {
                last = Some(ch);
            }
        }
        last.is_some_and(|l| matches!(self.tx(l), b"Test" | b"ParameterizedTest" | b"RepeatedTest" | b"TestFactory" | b"TestTemplate"))
    }

    fn visibility(m: Mods) -> u8 {
        if m.bits & M_PRIVATE != 0 {
            dflag::LOCAL
        } else if m.bits & (M_PROTECTED | M_INTERNAL) != 0 {
            0
        } else {
            dflag::EXPORTED
        }
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

    // ----- types -----

    /// The identifiers of a `user_type` (`a.b.Outer<T>.Inner`), in order. Zero when there are
    /// more than eight.
    fn segs(&self, n: Node<'t>, out: &mut [Node<'t>; 8]) -> usize {
        let mut k = 0;
        for i in 0..n.child_count() {
            let Some(ch) = n.child(i) else { break };
            if self.k(ch) == K::Identifier && ch.end_byte() > ch.start_byte() {
                if k == 8 {
                    return 0;
                }
                out[k] = ch;
                k += 1;
            }
        }
        k
    }

    /// How many leading segments of a qualified type name are its package: lower-case names
    /// that are not types of this file. When no segment is capitalised the last one is the type.
    fn pkg_len(&mut self, segs: &[Node<'t>]) -> usize {
        let mut p = 0;
        for s in segs {
            let text = self.tx(*s);
            if starts_upper(text) {
                break;
            }
            let id = self.cx.intern(text);
            if self.info(id) & (N_TYPE | N_ALIAS) != 0 {
                break;
            }
            p += 1;
        }
        p.min(segs.len().saturating_sub(1))
    }

    /// The package prefix of a qualified name, from the start of `first` to the end of `last`,
    /// as written without whitespace or comments.
    fn dotted_range(&mut self, first: Node<'t>, last: Node<'t>) -> u32 {
        let text = &self.cx.src[first.start_byte()..last.end_byte().max(first.start_byte())];
        if !text.iter().any(|&b| b <= b' ' || b == b'/') {
            return self.cx.intern(text);
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        push_clean(text, &mut buf);
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// A dotted name (`a.b.c`) as written, without embedded whitespace or comments.
    fn dotted(&mut self, n: Node<'t>) -> u32 {
        self.dotted_range(n, n)
    }

    /// The type path of a declared type: generics and nullability stripped, a type parameter
    /// replaced by its bound, an aliased import replaced by what it names. A qualified name keeps
    /// its qualifier: a package becomes a module qualifier (`java.util.List` is `java.util|List`),
    /// an outer type a root (`Map.Entry` is `Map::Entry`).
    fn type_name(&mut self, mut ty: Node<'t>) -> u32 {
        for _ in 0..8 {
            match self.k(ty) {
                K::UserType => return self.user_type_name(ty),
                K::NullableType | K::NonNullableType | K::ParenthesizedType => match self.first_type(ty) {
                    Some(x) => ty = x,
                    None => break,
                },
                _ => break,
            }
        }
        NONE
    }

    fn user_type_name(&mut self, n: Node<'t>) -> u32 {
        let mut segs = [n; 8];
        let k = self.segs(n, &mut segs);
        if k == 0 {
            return NONE;
        }
        if k == 1 {
            let id = self.nm(segs[0]);
            if let Some((_, bound)) = self.tparam(id) {
                return bound;
            }
            if let Some((target, real)) = self.alias_of(id) {
                return self.joined(b"", target, b"|", real);
            }
            return id;
        }
        let p = self.pkg_len(&segs[..k]);
        let pkg = if p > 0 { self.dotted_range(segs[0], segs[p - 1]) } else { NONE };
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        if pkg != NONE {
            buf.extend_from_slice(self.cx.name_bytes(pkg));
            buf.push(b'|');
        }
        for (i, s) in segs[p..k].iter().enumerate() {
            if i > 0 {
                buf.extend_from_slice(b"::");
            }
            buf.extend_from_slice(self.tx(*s));
        }
        let id = if buf.len() <= 200 { self.cx.intern(&buf) } else { NONE };
        self.cx.s.buf = buf;
        id
    }

    /// The type parameter `name` in scope, innermost first.
    #[inline]
    fn tparam(&self, name: u32) -> Option<(u32, u32)> {
        if self.tparams.is_empty() {
            return None;
        }
        self.tparams.iter().rev().find(|p| p.0 == name).copied()
    }

    /// Declare the type parameters of a class or function, each standing for its first bound
    /// (`<T : Writeable>`, or `where T : Writeable`), so that `t.writeTo(out)` is a call on it.
    fn declare_tparams(&mut self, n: Node<'t>) {
        for i in 0..n.child_count() {
            let Some(ch) = n.child(i) else { break };
            match self.k(ch) {
                K::TypeParameters => {
                    for j in 0..ch.child_count() {
                        let Some(p) = ch.child(j) else { break };
                        if self.k(p) != K::TypeParameter {
                            continue;
                        }
                        let Some(nm) = self.first_ident(p) else { continue };
                        let name = self.nm(nm);
                        let bound = self.first_type(p).map_or(NONE, |b| self.type_name(b));
                        self.tparams.push((name, bound));
                    }
                }
                K::TypeConstraints => {
                    for j in 0..ch.child_count() {
                        let Some(p) = ch.child(j) else { break };
                        if self.k(p) != K::TypeParameter {
                            continue;
                        }
                        let (Some(nm), Some(b)) = (self.first_ident(p), self.first_type(p)) else { continue };
                        let name = self.nm(nm);
                        let bound = self.type_name(b);
                        if let Some(tp) = self.tparams.iter_mut().rev().find(|t| t.0 == name)
                            && tp.1 == NONE
                        {
                            tp.1 = bound;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// `T : A` in a type parameter list or a `where` clause: the bound is a reference, `T` is not.
    fn type_parameter(&mut self, c: &mut TreeCursor<'t>) {
        let mut named = false;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Identifier if !named => named = true,
                _ => w.visit(c),
            }
        });
    }

    /// References for a `user_type`: the last segment with `kind`, outer types as `Type`, each
    /// qualified by what it is written with (the package, or the simple name of the outer type).
    /// Type parameters and the stop list emit nothing; type arguments are `Type` references.
    fn type_ref(&mut self, c: &mut TreeCursor<'t>, kind: RefKind, nargs: u8) {
        let n = c.node();
        let mut segs = [n; 8];
        let k = self.segs(n, &mut segs);
        if k == 1 {
            let text = self.tx(segs[0]);
            if !is_stop_type(text) && !(kind == RefKind::Annotation && is_stop_annotation(text)) {
                let id = self.cx.intern(text);
                let line = line_of(segs[0]);
                if let Some((target, real)) = self.alias_of(id) {
                    self.cx.add_ref(real, kind, line, target, nargs);
                } else if self.tparam(id).is_none() {
                    self.cx.add_ref(id, kind, line, NONE, nargs);
                }
            }
        } else if k > 1 {
            let p = self.pkg_len(&segs[..k]);
            let mut q = if p > 0 { self.dotted_range(segs[0], segs[p - 1]) } else { NONE };
            for i in p..k {
                let text = self.tx(segs[i]);
                let id = self.cx.intern(text);
                let kd = if i + 1 == k { kind } else { RefKind::Type };
                if !is_stop_type(text) && !(kd == RefKind::Annotation && is_stop_annotation(text)) {
                    self.cx.add_ref(id, kd, line_of(segs[i]), q, if i + 1 == k { nargs } else { 255 });
                }
                q = id;
            }
        }
        self.each(c, |w, c| {
            if matches!(w.k(c.node()), K::TypeArguments | K::TypeModifiers) {
                w.visit(c);
            }
        });
    }

    /// Emit the reference for the head of a type with `kind` (`Extends`, `New`, ...); type
    /// arguments, annotations and the parts of function types are plain `Type` references.
    fn type_head(&mut self, c: &mut TreeCursor<'t>, kind: RefKind, nargs: u8) {
        match self.k(c.node()) {
            K::UserType => self.type_ref(c, kind, nargs),
            K::NullableType | K::NonNullableType | K::ParenthesizedType => {
                self.each(c, |w, c| if w.is_type(c.node()) { w.type_head(c, kind, nargs) } else { w.visit(c) })
            }
            _ => self.visit(c),
        }
    }

    // ----- file level -----

    fn source_file(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| w.note_top(c.node()));
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::FunctionDecl => w.function(ch, c, Place::Top),
                K::PropertyDecl => w.property(ch, c, Place::Top),
                _ => w.visit(c),
            }
        });
    }

    /// Remember what a top-level declaration makes visible in the whole file: functions and
    /// properties may be used before they are declared.
    fn note_top(&mut self, n: Node<'t>) {
        match self.k(n) {
            K::FunctionDecl => {
                let p = self.fn_parts(n);
                if let (Some(nm), None) = (p.name, p.recv) {
                    let id = self.nm(nm);
                    self.set_info(id, N_TOPFN);
                }
            }
            K::PropertyDecl => {
                let p = self.prop_parts(n);
                if p.recv.is_some() {
                    return;
                }
                if let Some(v) = p.var.and_then(|v| self.first_ident(v)) {
                    let id = self.nm(v);
                    self.set_info(id, N_TOPVAL);
                }
            }
            K::ClassDecl | K::ObjectDecl => {
                if let Some(nm) = self.decl_name(n) {
                    let id = self.nm(nm);
                    self.set_info(id, N_TYPE);
                }
            }
            K::TypeAlias => {
                if let Some(nm) = child(n, self.t.f_type) {
                    let id = self.nm(nm);
                    self.set_info(id, N_TYPE);
                }
            }
            _ => {}
        }
    }

    fn package(&mut self, n: Node<'t>) {
        let Some(q) = (0..n.child_count()).filter_map(|i| n.child(i)).find(|x| matches!(self.k(*x), K::QualifiedIdentifier | K::Identifier)) else {
            return;
        };
        if q.end_byte() == q.start_byte() {
            return;
        }
        let name = self.dotted(q);
        self.cx.def(name, DefKind::Namespace, n, n.end_byte());
    }

    fn import(&mut self, n: Node<'t>) {
        let mut q = None;
        let mut alias = None;
        let mut wildcard = false;
        for i in 0..n.child_count() {
            let Some(ch) = n.child(i) else { break };
            match self.k(ch) {
                K::QualifiedIdentifier if q.is_none() => q = Some(ch),
                K::Identifier if q.is_none() => q = Some(ch),
                K::Identifier => alias = Some(ch),
                K::Token if self.tx(ch) == b"*" => wildcard = true,
                _ => {}
            }
        }
        let Some(q) = q.filter(|x| x.end_byte() > x.start_byte()) else { return };
        let mut segs = [q; 8];
        let k = if self.k(q) == K::Identifier { 1 } else { self.segs(q, &mut segs) };
        if k == 0 {
            return;
        }
        let line = line_of(n);
        let text = self.tx(q);
        let sys = if text.starts_with(b"kotlin.") || text.starts_with(b"java.") || text.starts_with(b"javax.") || text.starts_with(b"jdk.") {
            iflag::SYSTEM
        } else {
            0
        };
        if wildcard {
            let target = self.dotted(q);
            self.cx.add_import(target, NONE, NONE, line, iflag::WILDCARD | sys);
            // `import a.b.Obj.*` uses the object `Obj` of package `a.b`.
            if k > 1 && starts_upper(self.tx(segs[k - 1])) {
                let scope = self.dotted_range(segs[0], segs[k - 2]);
                let name = self.nm(segs[k - 1]);
                self.cx.add_ref(name, RefKind::Import, line_of(segs[k - 1]), scope, 255);
            }
            return;
        }
        let last = segs[k - 1];
        let name = self.nm(last);
        let target = if k > 1 { self.dotted_range(segs[0], segs[k - 2]) } else { self.cx.intern(b"") };
        let local = match alias.filter(|a| a.end_byte() > a.start_byte()) {
            Some(a) => {
                let id = self.nm(a);
                self.cx.add_import(target, name, id, line, sys);
                self.aliases.push((id, target, name));
                self.set_info(id, N_ALIAS);
                id
            }
            None => {
                self.cx.add_import(target, name, NONE, line, sys);
                name
            }
        };
        self.cx.add_ref(name, RefKind::Import, line_of(last), if k > 1 { target } else { NONE }, 255);
        let upper = starts_upper(self.tx(last));
        let owner_at = segs[..k - 1].iter().position(|s| starts_upper(self.tx(*s)));
        match owner_at {
            // A member of an object or companion (`a.b.C.foo`, `a.b.C.Companion.foo`), unless
            // what is imported is itself a nested type.
            Some(at) if !type_like(self.tx(last)) => {
                let owner = self.static_owner(&segs[..k - 1], at);
                match self.statics.iter_mut().find(|x| x.0 == local) {
                    Some(x) if x.1 != owner => x.1 = NONE,
                    Some(_) => {}
                    None => self.statics.push((local, owner)),
                }
                self.set_info(local, N_STATIC);
            }
            _ if upper => self.set_info(local, N_TYPE),
            _ => self.set_info(local, N_IMPVAL),
        }
    }

    /// Type path of the object or class named by the segments of an import before the member:
    /// `a.b.Outer.Inner` is `a.b|Outer::Inner`. A `Companion` segment is dropped: the members of
    /// a companion belong to the class around it.
    fn static_owner(&mut self, segs: &[Node<'t>], at: usize) -> u32 {
        let pkg = if at > 0 { self.dotted_range(segs[0], segs[at - 1]) } else { NONE };
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        if pkg != NONE {
            buf.extend_from_slice(self.cx.name_bytes(pkg));
            buf.push(b'|');
        }
        let mut first = true;
        for s in &segs[at..] {
            let t = self.tx(*s);
            if t == b"Companion" {
                continue;
            }
            if !first {
                buf.extend_from_slice(b"::");
            }
            first = false;
            buf.extend_from_slice(t);
        }
        let id = if !first && buf.len() <= 200 { self.cx.intern(&buf) } else { NONE };
        self.cx.s.buf = buf;
        id
    }

    // ----- annotations -----

    /// `@A`, `@a.b.A(x, key = y)`, `@file:A`, `@get:A`. A named argument writes the element of
    /// the annotation class.
    fn annotation(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::UserType => w.type_ref(c, RefKind::Annotation, 255),
                K::ConstructorInvocation => {
                    let owner = w.first_type(ch).map_or(NONE, |t| w.type_name(t));
                    w.each(c, |w, c| match w.k(c.node()) {
                        K::UserType => w.type_ref(c, RefKind::Annotation, 255),
                        K::ValueArguments => w.annotation_args(c, owner),
                        _ => w.visit(c),
                    });
                }
                K::Token | K::Comment | K::Skip => {}
                _ => w.visit(c),
            }
        });
    }

    fn annotation_args(&mut self, c: &mut TreeCursor<'t>, owner: u32) {
        self.each(c, |w, c| {
            if w.k(c.node()) != K::ValueArgument {
                return;
            }
            w.each(c, |w, c| {
                let ch = c.node();
                match w.k(ch) {
                    K::Identifier if w.is_arg_name(ch) => {
                        let name = w.nm(ch);
                        w.cx.add_ref(name, RefKind::MemberWrite, line_of(ch), owner, 255);
                    }
                    _ => w.visit(c),
                }
            });
        });
    }

    /// Is the identifier the name of a named argument (`name = value`)?
    fn is_arg_name(&self, n: Node<'t>) -> bool {
        n.next_sibling().is_some_and(|x| self.k(x) == K::Token && self.tx(x) == b"=")
    }

    // ----- type declarations -----

    fn class_decl(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let Some(nm) = self.decl_name(n) else { return self.children(c) };
        let m = self.modifiers(n);
        let iface = self.has_token(n, b"interface");
        let (kind, cf) = if iface {
            (DefKind::Interface, CF_IFACE)
        } else if m.bits & M_ENUM != 0 {
            (DefKind::Enum, 0)
        } else if m.bits & M_ANNOT != 0 {
            (DefKind::Interface, CF_ANNOT)
        } else {
            (DefKind::Class, 0)
        };
        self.type_decl(n, c, nm, m, kind, cf);
    }

    fn object_decl(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let Some(nm) = self.decl_name(n) else { return self.children(c) };
        let m = self.modifiers(n);
        self.type_decl(n, c, nm, m, DefKind::Class, 0);
    }

    fn type_decl(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, nm: Node<'t>, m: Mods, kind: DefKind, cf: u8) {
        let mut body = None;
        let mut primary = false;
        for i in 0..n.child_count() {
            let Some(ch) = n.child(i) else { break };
            match self.k(ch) {
                K::ClassBody => body = Some(ch),
                K::PrimaryConstructor => primary = true,
                _ => {}
            }
        }
        let sig_end = body.map_or(n.end_byte(), |b| b.start_byte());
        let name = self.nm(nm);
        self.set_info(name, N_TYPE);
        let parent = self.cx.encl();
        let mut flags = Self::visibility(m);
        if m.bits & (M_ABSTRACT | M_SEALED) != 0 {
            flags |= dflag::ABSTRACT;
        }
        if m.bits & M_EXPECT != 0 {
            flags |= dflag::DECL;
        }
        let d = self.cx.def(name, kind, n, sig_end);
        self.finish_def(d, parent, flags, m.sig_start, sig_end);
        self.cx.push_def(d);
        let tp_mark = self.tparams.len();
        self.declare_tparams(n);
        let params_mark = self.params.len() as u32;
        // Without a primary constructor a superclass is written without a call; the secondary
        // constructors call it with `super(..)`.
        let first_is_class = !primary && cf & CF_IFACE == 0 && body.is_some_and(|b| self.delegates_to_super(b));
        let mut cls = Class { name, own: name, this_ty: name, sup: NONE, sup_q: NONE, flags: cf, methods: (0, 0), params: (params_mark, params_mark) };
        let nm_at = nm.start_byte();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if ch.start_byte() == nm_at => {}
                K::PrimaryConstructor => {
                    let (lo, hi) = w.primary_ctor(c, cf);
                    let dm = w.cx.def_mut(d);
                    dm.arity_min = lo;
                    dm.arity_max = hi;
                }
                K::DelegationSpecifiers => {
                    w.cx.push_scope();
                    for i in params_mark..w.params.len() as u32 {
                        let (pn, pt, _) = w.params[i as usize];
                        w.cx.declare(pn, pt);
                    }
                    cls.sup = w.supertypes(c, cf & CF_IFACE != 0, false, first_is_class);
                    w.cx.pop_scope();
                }
                K::ClassBody => {
                    cls.params.1 = w.params.len() as u32;
                    w.class_body(c, cls);
                }
                _ => w.visit(c),
            }
        });
        self.params.truncate(params_mark as usize);
        self.tparams.truncate(tp_mark);
        self.cx.pop_def();
    }

    /// Does a secondary constructor in the class body delegate to `super(..)`?
    fn delegates_to_super(&self, body: Node<'t>) -> bool {
        (0..body.child_count()).filter_map(|i| body.child(i)).any(|ch| {
            self.k(ch) == K::SecondaryConstructor
                && (0..ch.child_count()).filter_map(|i| ch.child(i)).any(|d| self.k(d) == K::ConstructorDelegationCall && self.has_token(d, b"super"))
        })
    }

    /// The supertype list. Returns the type path of the superclass. `object_expr`: an object
    /// expression, where a constructor call creates an instance (`New`).
    fn supertypes(&mut self, c: &mut TreeCursor<'t>, iface: bool, object_expr: bool, first_is_class: bool) -> u32 {
        let mut sup = NONE;
        let mut first = true;
        self.each(c, |w, c| {
            if w.k(c.node()) != K::DelegationSpecifier {
                return;
            }
            let is_first = std::mem::take(&mut first);
            w.each(c, |w, c| {
                let ch = c.node();
                match w.k(ch) {
                    K::ConstructorInvocation => {
                        let ty = w.first_type(ch).map_or(NONE, |t| w.type_name(t));
                        if sup == NONE {
                            sup = ty;
                        }
                        let from = w.cx.facts.refs.len();
                        let mut nargs = 255u8;
                        let simple = w.simple_of(ty);
                        w.each(c, |w, c| match w.k(c.node()) {
                            K::UserType | K::NullableType | K::ParenthesizedType => w.type_head(c, RefKind::Extends, 255),
                            K::ValueArguments => nargs = w.arguments(c, simple).0,
                            _ => w.visit(c),
                        });
                        let ext = w.cx.facts.refs.get(from..).and_then(|rs| rs.iter().find(|r| r.kind == RefKind::Extends)).copied();
                        if let Some(e) = ext {
                            if object_expr {
                                w.cx.add_ref(e.name, RefKind::New, e.line, e.qual, nargs);
                            } else if ty != NONE {
                                // The constructor of the superclass runs, as with Java's `super(..)`.
                                w.cx.add_ref(e.name, RefKind::Call, e.line, ty, nargs);
                            }
                        }
                    }
                    K::UserType | K::NullableType | K::ParenthesizedType => {
                        let kind = if iface || (is_first && first_is_class) { RefKind::Extends } else { RefKind::Implements };
                        if kind == RefKind::Extends && sup == NONE {
                            sup = w.type_name(ch);
                        }
                        w.type_head(c, kind, 255);
                    }
                    K::ExplicitDelegation => w.each(c, |w, c| if w.is_type(c.node()) { w.type_head(c, RefKind::Implements, 255) } else { w.visit(c) }),
                    _ => w.visit(c),
                }
            });
        });
        sup
    }

    /// The primary constructor: parameters declared `val` / `var` are fields of the class, the
    /// others are visible to the initializers. Returns the arity.
    fn primary_ctor(&mut self, c: &mut TreeCursor<'t>, cf: u8) -> (u8, u8) {
        let mut count = 0u32;
        let mut defaults = 0u32;
        self.each(c, |w, c| match w.k(c.node()) {
            K::ClassParameters => w.each(c, |w, c| {
                let ch = c.node();
                if w.k(ch) == K::ClassParameter {
                    count += 1;
                    if w.class_param(ch, c, cf) {
                        defaults += 1;
                    }
                }
            }),
            K::Token | K::Comment => {}
            _ => w.visit(c),
        });
        let count = count.min(254);
        (count.saturating_sub(defaults) as u8, count as u8)
    }

    /// One primary constructor parameter. Returns whether it has a default.
    fn class_param(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, cf: u8) -> bool {
        let nm = self.first_ident(n);
        let ty_node = self.first_type(n);
        let ty = ty_node.map_or(NONE, |t| self.type_name(t));
        let prop = self.has_token(n, b"val") || self.has_token(n, b"var");
        let has_default = self.has_token(n, b"=");
        let Some(nm) = nm else {
            self.children(c);
            return has_default;
        };
        let name = self.nm(nm);
        let mut d = NONE;
        if prop {
            let m = self.modifiers(n);
            let sig_end = ty_node.map_or(nm.end_byte(), |t| t.end_byte());
            let parent = self.cx.encl();
            let kind = if cf & CF_ANNOT != 0 { DefKind::Property } else { DefKind::Field };
            let fnptr = ty_node.is_some_and(|t| self.is_fn_type(t));
            d = self.cx.def(name, kind, n, sig_end);
            self.finish_def(d, parent, Self::visibility(m) | if fnptr { dflag::FNPTR } else { 0 }, m.sig_start, sig_end);
            self.cx.def_mut(d).type_name = ty;
            self.cx.push_def(d);
        }
        self.params.push((name, enc_local(ty), prop));
        let nm_at = nm.start_byte();
        self.children_except(c, nm_at);
        if d != NONE {
            self.cx.pop_def();
        }
        has_default
    }

    fn is_fn_type(&self, mut t: Node<'t>) -> bool {
        for _ in 0..4 {
            match self.k(t) {
                K::FunctionType => return true,
                K::NullableType | K::ParenthesizedType => match self.first_type(t) {
                    Some(x) => t = x,
                    None => return false,
                },
                _ => return false,
            }
        }
        false
    }

    /// Enter a class body: its members become visible, then the body is walked.
    fn class_body(&mut self, c: &mut TreeCursor<'t>, cls: Class) {
        let level = self.classes.len();
        self.classes.push(cls);
        self.cx.push_scope();
        for i in cls.params.0..cls.params.1 {
            let (name, ty, prop) = self.params[i as usize];
            if prop {
                self.cx.declare(name, enc_member(level, dec_ty(ty)));
            }
        }
        let methods_mark = self.methods.len();
        let n = c.node();
        self.declare_members(n, level, cls.own);
        if let Some(cl) = self.classes.last_mut() {
            cl.methods = (methods_mark as u32, self.methods.len() as u32);
        }
        self.members(c);
        self.methods.truncate(methods_mark);
        self.cx.pop_scope();
        self.classes.pop();
    }

    /// Properties, enum entries and functions may be used before their declaration: declare them
    /// all when the body is entered. A companion's members are members of the class.
    fn declare_members(&mut self, body: Node<'t>, level: usize, own: u32) {
        for i in 0..body.child_count() {
            let Some(ch) = body.child(i) else { break };
            match self.k(ch) {
                K::PropertyDecl => {
                    let p = self.prop_parts(ch);
                    if p.recv.is_some() {
                        continue;
                    }
                    if let Some(v) = p.var
                        && let Some(nm) = self.first_ident(v)
                    {
                        let ty = match self.first_type(v) {
                            Some(t) => self.type_name(t),
                            None => p.init.map_or(NONE, |x| self.quick_type(x)),
                        };
                        let name = self.nm(nm);
                        self.cx.declare(name, enc_member(level, ty));
                    }
                }
                K::FunctionDecl => {
                    let p = self.fn_parts(ch);
                    if let (Some(nm), None) = (p.name, p.recv) {
                        let name = self.nm(nm);
                        self.methods.push(name);
                    }
                }
                K::EnumEntry => {
                    if let Some(nm) = self.first_ident(ch) {
                        let name = self.nm(nm);
                        self.cx.declare(name, enc_member(level, own));
                    }
                }
                K::CompanionObject => {
                    if let Some(nm) = self.decl_name(ch) {
                        let name = self.nm(nm);
                        self.set_info(name, N_COMPANION);
                    }
                    if let Some(b) = (0..ch.child_count()).filter_map(|j| ch.child(j)).find(|x| self.k(*x) == K::ClassBody) {
                        self.declare_members(b, level, own);
                    }
                }
                K::ClassDecl | K::ObjectDecl => {
                    if let Some(nm) = self.decl_name(ch) {
                        let name = self.nm(nm);
                        self.set_info(name, N_TYPE);
                    }
                }
                _ => {}
            }
        }
    }

    /// The type an initializer obviously has, without walking it: a constructor call of a type
    /// or a cast.
    fn quick_type(&mut self, x: Node<'t>) -> u32 {
        match self.k(x) {
            K::CallExpr => {
                let Some(f) = x.child(0).filter(|f| self.k(*f) == K::Identifier) else { return NONE };
                let text = self.tx(f);
                let id = self.cx.intern(text);
                if (type_like(text) || self.info(id) & N_TYPE != 0) && self.info(id) & (N_TOPFN | N_IMPVAL) == 0 && !is_stop_type(text) {
                    match self.alias_of(id) {
                        Some((target, real)) => self.joined(b"", target, b"|", real),
                        None => id,
                    }
                } else {
                    NONE
                }
            }
            K::AsExpr => (0..x.child_count()).filter_map(|i| x.child(i)).filter(|t| self.is_type(*t)).last().map_or(NONE, |t| self.type_name(t)),
            _ => NONE,
        }
    }

    /// The members of a class body (or of a companion's body, which shares the class's scope).
    fn members(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::PropertyDecl => w.property(ch, c, Place::Member),
                K::FunctionDecl => w.function(ch, c, Place::Member),
                K::SecondaryConstructor => w.secondary_ctor(ch, c),
                K::AnonymousInitializer => {
                    w.cx.push_scope();
                    w.declare_ctor_params();
                    w.children(c);
                    w.cx.pop_scope();
                }
                K::CompanionObject => w.companion(ch, c),
                K::EnumEntry => w.enum_entry(ch, c),
                _ => w.visit(c),
            }
        });
    }

    /// Make the primary constructor parameters of the innermost class visible (initializers).
    fn declare_ctor_params(&mut self) {
        let Some(cl) = self.classes.iter().rev().find(|cl| cl.flags & CF_RECV == 0).copied() else { return };
        for i in cl.params.0..cl.params.1 {
            if let Some(&(name, ty, _)) = self.params.get(i as usize) {
                self.cx.declare(name, ty);
            }
        }
    }

    /// `companion object [Name] : Supers { .. }`: a definition of its own, whose members belong
    /// to the class around it.
    fn companion(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        if !self.classes.last().is_some_and(|cl| cl.flags & (CF_RECV | CF_ANON) == 0) {
            return self.children(c);
        }
        let m = self.modifiers(n);
        let nm = self.decl_name(n);
        let name = match nm {
            Some(x) => self.nm(x),
            None => self.cx.intern(b"Companion"),
        };
        let body = (0..n.child_count()).filter_map(|i| n.child(i)).find(|x| self.k(*x) == K::ClassBody);
        let sig_end = body.map_or(n.end_byte(), |b| b.start_byte());
        let parent = self.cx.encl();
        let d = self.cx.def(name, DefKind::Class, n, sig_end);
        self.finish_def(d, parent, Self::visibility(m), m.sig_start, sig_end);
        self.cx.push_def(d);
        let mut pushed = true;
        let nm_at = nm.map_or(usize::MAX, |x| x.start_byte());
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if ch.start_byte() == nm_at => {}
                K::DelegationSpecifiers => {
                    w.supertypes(c, false, false, false);
                }
                K::ClassBody => {
                    w.cx.pop_def();
                    pushed = false;
                    w.members(c);
                }
                _ => w.visit(c),
            }
        });
        if pushed {
            self.cx.pop_def();
        }
    }

    /// `object : Supers { .. }`. Returns the type of the object: its first supertype.
    fn object_literal(&mut self, c: &mut TreeCursor<'t>) -> Recv {
        let n = c.node();
        let first = (0..n.child_count())
            .filter_map(|i| n.child(i))
            .find(|x| self.k(*x) == K::DelegationSpecifiers)
            .and_then(|ds| ds.named_child(0))
            .and_then(|spec| (0..spec.child_count()).filter_map(|i| spec.child(i)).find(|x| self.is_type(*x) || self.k(*x) == K::ConstructorInvocation));
        let ty = match first {
            Some(x) if self.k(x) == K::ConstructorInvocation => self.first_type(x).map_or(NONE, |t| self.type_name(t)),
            Some(x) => self.type_name(x),
            None => NONE,
        };
        let name = self.class_name();
        self.each(c, |w, c| match w.k(c.node()) {
            K::DelegationSpecifiers => {
                w.supertypes(c, false, true, false);
            }
            K::ClassBody => {
                let cls = Class { name, own: NONE, this_ty: ty, sup: ty, sup_q: NONE, flags: CF_ANON, methods: (0, 0), params: (0, 0) };
                w.class_body(c, cls);
            }
            _ => w.visit(c),
        });
        Recv::val(ty)
    }

    fn enum_entry(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let Some(nm) = self.first_ident(n) else { return self.children(c) };
        let m = self.modifiers(n);
        let body = (0..n.child_count()).filter_map(|i| n.child(i)).find(|x| self.k(*x) == K::ClassBody);
        let sig_end = body.map_or(n.end_byte(), |b| b.start_byte());
        let name = self.nm(nm);
        let parent = self.cx.encl();
        let own = self.classes.last().map_or(NONE, |cl| cl.own);
        let d = self.cx.def(name, DefKind::EnumMember, n, sig_end);
        self.finish_def(d, parent, dflag::EXPORTED, m.sig_start, sig_end);
        self.cx.def_mut(d).type_name = own;
        self.cx.push_def(d);
        let nm_at = nm.start_byte();
        let line = line_of(nm);
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if ch.start_byte() == nm_at => {}
                K::ValueArguments => {
                    // `RED(1)` runs a constructor of the enum.
                    let nargs = w.arguments(c, own).0;
                    if own != NONE {
                        w.cx.add_ref(own, RefKind::New, line, NONE, nargs);
                    }
                }
                // An entry with a body is an anonymous subclass of the enum.
                K::ClassBody => {
                    w.class_body(c, Class { name: own, own: NONE, this_ty: own, sup: own, sup_q: NONE, flags: CF_ANON, methods: (0, 0), params: (0, 0) })
                }
                _ => w.visit(c),
            }
        });
        self.cx.pop_def();
    }

    fn secondary_ctor(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let Some(cls) = self.classes.last().copied().filter(|cl| cl.own != NONE) else { return self.children(c) };
        let m = self.modifiers(n);
        let body = (0..n.child_count()).filter_map(|i| n.child(i)).find(|x| self.k(*x) == K::Block);
        let sig_end = body.map_or(n.end_byte(), |b| b.start_byte());
        let parent = self.cx.encl();
        let d = self.cx.def(cls.own, DefKind::Method, n, sig_end);
        self.finish_def(d, parent, Self::visibility(m), m.sig_start, sig_end);
        self.cx.push_def(d);
        self.cx.push_scope();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::FunctionValueParameters => {
                    let (lo, hi) = w.params(c);
                    let dm = w.cx.def_mut(d);
                    dm.arity_min = lo;
                    dm.arity_max = hi;
                }
                K::ConstructorDelegationCall => {
                    // `this(..)` / `super(..)`: a constructor of the class or of its superclass.
                    let target = if w.has_token(ch, b"super") { cls.sup } else { cls.own };
                    let line = line_of(ch);
                    let simple = w.simple_of(target);
                    let mut nargs = 255u8;
                    w.each(c, |w, c| {
                        if w.k(c.node()) == K::ValueArguments {
                            nargs = w.arguments(c, simple).0;
                        }
                    });
                    if target != NONE && !is_stop_type(w.cx.name_bytes(simple)) {
                        w.cx.add_ref(simple, RefKind::Call, line, target, nargs);
                    }
                }
                // The body shares the scope of the parameters.
                K::Block => w.children(c),
                _ => w.visit(c),
            }
        });
        self.cx.pop_scope();
        self.cx.pop_def();
    }

    fn type_alias(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let Some(nm) = child(n, self.t.f_type).filter(|x| x.end_byte() > x.start_byte()) else { return self.children(c) };
        let m = self.modifiers(n);
        let name = self.nm(nm);
        let parent = self.cx.encl();
        let tp_mark = self.tparams.len();
        self.declare_tparams(n);
        let nm_at = nm.start_byte();
        let aliased = (0..n.child_count()).filter_map(|i| n.child(i)).filter(|x| self.is_type(*x) && x.start_byte() != nm_at).last();
        let ty = aliased.map_or(NONE, |t| self.type_name(t));
        let d = self.cx.def(name, DefKind::TypeAlias, n, n.end_byte());
        self.finish_def(d, parent, Self::visibility(m), m.sig_start, n.end_byte());
        self.cx.def_mut(d).type_name = ty;
        self.cx.push_def(d);
        self.children_except(c, nm_at);
        self.cx.pop_def();
        self.tparams.truncate(tp_mark);
    }

    // ----- functions -----

    fn fn_parts(&self, n: Node<'t>) -> FnParts<'t> {
        let name = self.decl_name(n);
        let nm_at = name.map_or(usize::MAX, |x| x.start_byte());
        let mut p = FnParts { name, recv: None, ret: None, body: None };
        for i in 0..n.child_count() {
            let Some(ch) = n.child(i) else { break };
            match self.k(ch) {
                K::FunctionBody => p.body = Some(ch),
                _ if self.is_type(ch) => {
                    if ch.start_byte() < nm_at {
                        p.recv = Some(ch);
                    } else {
                        p.ret = Some(ch);
                    }
                }
                _ => {}
            }
        }
        p
    }

    fn function(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, place: Place) {
        let p = self.fn_parts(n);
        let Some(nm) = p.name else { return self.children(c) };
        let m = self.modifiers(n);
        let text = self.tx(nm);
        let name = self.cx.intern(text);
        let cls = self.classes.last().copied().filter(|cl| cl.flags & CF_RECV == 0);
        let iface = place == Place::Member && cls.is_some_and(|cl| cl.flags & CF_IFACE != 0);
        let parent = self.cx.encl();
        let tp_mark = self.tparams.len();
        self.declare_tparams(n);
        let recv_ty = p.recv.map_or(NONE, |r| self.type_name(r));
        let kind = match place {
            Place::Member => DefKind::Method,
            Place::Top if recv_ty != NONE => DefKind::Method,
            _ => DefKind::Function,
        };
        let mut flags = if place == Place::Local { 0 } else { Self::visibility(m) };
        if p.body.is_none() || m.bits & M_EXPECT != 0 {
            flags |= dflag::DECL;
            if m.bits & M_ABSTRACT != 0 || (iface && m.bits & M_EXTERNAL == 0) {
                flags |= dflag::ABSTRACT;
            }
        } else if m.bits & M_ABSTRACT != 0 {
            flags |= dflag::ABSTRACT;
        }
        if m.bits & M_SUSPEND != 0 {
            flags |= dflag::ASYNC;
        }
        if m.bits & M_TEST != 0 {
            flags |= dflag::TEST;
        }
        if place == Place::Local {
            // A local function can be called from where it is declared on, and recursively.
            self.cx.declare(name, B_FN);
        }
        let sig_end = p.body.map_or(n.end_byte(), |b| b.start_byte());
        // An extension is a member of its receiver type, known by name. One declared in a class
        // or an object (`companion object { fun String.encodeUtf8() }`) is called the same way,
        // `"x".encodeUtf8()`, so it is recorded the same way, not under the class.
        let ext = place != Place::Local && recv_ty != NONE;
        let d = self.cx.def(name, kind, n, sig_end);
        self.finish_def(d, if ext { NONE } else { parent }, flags, m.sig_start, sig_end);
        if ext {
            let q = self.simple_of(recv_ty);
            self.cx.def_mut(d).qual = q;
        }
        let ret = p.ret.map_or(NONE, |t| self.type_name(t));
        self.cx.def_mut(d).type_name = ret;
        self.cx.push_def(d);
        self.cx.push_scope();
        let pushed = recv_ty != NONE;
        if pushed {
            self.classes.push(Class::recv(recv_ty));
        }
        let nm_at = nm.start_byte();
        let mut inferred = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if ch.start_byte() == nm_at => {}
                K::FunctionValueParameters => {
                    let (lo, hi) = w.params(c);
                    let dm = w.cx.def_mut(d);
                    dm.arity_min = lo;
                    dm.arity_max = hi;
                }
                K::FunctionBody => {
                    let r = w.function_body(c);
                    if ret == NONE {
                        inferred = w.plain_type(r);
                    }
                }
                _ => w.visit(c),
            }
        });
        if ret == NONE && inferred != NONE {
            self.cx.def_mut(d).type_name = inferred;
        }
        if pushed {
            self.classes.pop();
        }
        self.cx.pop_scope();
        self.tparams.truncate(tp_mark);
        self.cx.pop_def();
    }

    /// A block body, or `= expression`, which is walked as a receiver: what it denotes is the
    /// result of the function when no type is written.
    fn function_body(&mut self, c: &mut TreeCursor<'t>) -> Recv {
        let mut r = Recv::UNKNOWN;
        self.each(c, |w, c| match w.k(c.node()) {
            K::Token | K::Comment => {}
            K::Block => w.children(c),
            _ => r = w.recv(c),
        });
        r
    }

    /// Declare the parameters under the cursor (`function_value_parameters`) in the current
    /// scope. Returns the arity: parameters with a default are optional, `vararg` is variadic.
    fn params(&mut self, c: &mut TreeCursor<'t>) -> (u8, u8) {
        let mut count = 0u32;
        let mut defaults = 0u32;
        let mut vararg = false;
        let mut pending_vararg = false;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::ParameterModifiers => {
                    if w.has_vararg(ch) {
                        pending_vararg = true;
                    }
                    w.visit(c);
                }
                K::Parameter => {
                    count += 1;
                    w.var_decl(c, NONE);
                    if std::mem::take(&mut pending_vararg) {
                        vararg = true;
                        // `vararg xs: T` is an array of `T`: its members are not those of `T`.
                        if let Some(nm) = w.first_ident(ch) {
                            let name = w.nm(nm);
                            w.cx.declare(name, 0);
                        }
                    }
                }
                K::Token => {
                    if w.tx(ch) == b"=" {
                        defaults += 1;
                    }
                }
                K::Comment => {}
                _ => w.visit(c),
            }
        });
        let count = count.min(254);
        let min = count.saturating_sub(defaults).saturating_sub(vararg as u32);
        (min as u8, if vararg { 255 } else { count as u8 })
    }

    fn has_vararg(&self, mods: Node<'t>) -> bool {
        (0..mods.child_count()).filter_map(|i| mods.child(i)).any(|x| self.tx(x) == b"vararg")
    }

    /// A parameter or a variable declaration (`name: T`): declare the name with its type (or
    /// `fallback` when none is written) and visit the type.
    fn var_decl(&mut self, c: &mut TreeCursor<'t>, fallback: u32) {
        let n = c.node();
        let nm = self.first_ident(n);
        if let Some(nm) = nm {
            let ty = self.first_type(n).map_or(fallback, |t| self.type_name(t));
            let name = self.nm(nm);
            self.cx.declare(name, enc_local(ty));
        }
        let nm_at = nm.map_or(usize::MAX, |x| x.start_byte());
        self.children_except(c, nm_at);
    }

    fn anon_function(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        self.each(c, |w, c| match w.k(c.node()) {
            K::FunctionBody => {
                w.function_body(c);
            }
            _ => w.visit(c),
        });
        self.cx.pop_scope();
    }

    // ----- properties -----

    fn prop_parts(&self, n: Node<'t>) -> PropParts<'t> {
        let mut p = PropParts { recv: None, var: None, multi: None, init: None, accessors: false, delegate: false, sig_end: n.end_byte() };
        let mut eq = false;
        let mut decl_seen = false;
        for i in 0..n.child_count() {
            let Some(ch) = n.child(i) else { break };
            match self.k(ch) {
                K::Token => {
                    if self.tx(ch) == b"=" {
                        eq = true;
                    }
                }
                K::Comment | K::Modifiers | K::TypeParameters => {}
                K::VariableDeclaration => {
                    p.var = Some(ch);
                    p.sig_end = ch.end_byte();
                    decl_seen = true;
                }
                K::MultiVariableDeclaration => {
                    p.multi = Some(ch);
                    p.sig_end = ch.end_byte();
                    decl_seen = true;
                }
                K::TypeConstraints => p.sig_end = ch.end_byte(),
                K::PropertyDelegate => p.delegate = true,
                K::Getter | K::Setter => p.accessors = true,
                _ if !decl_seen && self.is_type(ch) => p.recv = Some(ch),
                _ if eq && p.init.is_none() => p.init = Some(ch),
                _ => {}
            }
        }
        p
    }

    fn property(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, place: Place) {
        let p = self.prop_parts(n);
        if place == Place::Local {
            return self.local_property(c, &p);
        }
        let Some(var) = p.var else { return self.local_property(c, &p) };
        let Some(nm) = self.first_ident(var) else { return self.children(c) };
        let m = self.modifiers(n);
        let name = self.nm(nm);
        let cls = self.classes.last().copied().filter(|cl| cl.flags & CF_RECV == 0);
        let iface = place == Place::Member && cls.is_some_and(|cl| cl.flags & CF_IFACE != 0);
        let tp_mark = self.tparams.len();
        self.declare_tparams(n);
        let recv_ty = p.recv.map_or(NONE, |r| self.type_name(r));
        let declared = self.first_type(var);
        let kind = match place {
            Place::Member if p.accessors || recv_ty != NONE || iface => DefKind::Property,
            Place::Member => DefKind::Field,
            _ if recv_ty != NONE => DefKind::Property,
            _ if m.bits & M_CONST != 0 => DefKind::Constant,
            _ => DefKind::Variable,
        };
        let mut flags = Self::visibility(m);
        let callable = declared.is_some_and(|t| self.is_fn_type(t))
            || p.init.is_some_and(|x| matches!(self.k(x), K::LambdaLiteral | K::AnonymousFunction | K::CallableReference) || self.is_member_ref(x));
        if callable {
            flags |= dflag::FNPTR;
        }
        let bodiless = p.init.is_none() && !p.accessors && !p.delegate;
        if m.bits & M_ABSTRACT != 0 || (iface && bodiless) {
            flags |= dflag::DECL | dflag::ABSTRACT;
        } else if m.bits & M_EXPECT != 0 {
            flags |= dflag::DECL;
        }
        let parent = self.cx.encl();
        let d = self.cx.def(name, kind, n, p.sig_end);
        self.finish_def(d, parent, flags, m.sig_start, p.sig_end);
        if place == Place::Top && recv_ty != NONE {
            let q = self.simple_of(recv_ty);
            self.cx.def_mut(d).qual = q;
        }
        let declared_ty = declared.map_or(NONE, |t| self.type_name(t));
        self.cx.def_mut(d).type_name = declared_ty;
        self.cx.push_def(d);
        let owner = cls.map_or(NONE, |cl| cl.own);
        let init_at = p.init.map_or(usize::MAX, |x| x.start_byte());
        let mut inferred = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::VariableDeclaration => w.var_types(c),
                K::Getter | K::Setter => w.accessor(c, recv_ty),
                K::PropertyDelegate => {
                    w.cx.push_scope();
                    if place == Place::Member {
                        w.declare_ctor_params();
                    }
                    w.children(c);
                    w.cx.pop_scope();
                }
                _ if ch.start_byte() == init_at => {
                    w.cx.push_scope();
                    if place == Place::Member {
                        w.declare_ctor_params();
                    }
                    if matches!(w.k(ch), K::CallableReference) || w.is_member_ref(ch) {
                        // A function stored into the property: a binding into its slot.
                        let (f, line) = w.callable_ref(c);
                        if f != NONE {
                            let slot = w.joined(b"", owner, b".", name);
                            w.cx.add_ref(f, RefKind::Bind, line, slot, 255);
                        }
                    } else {
                        let r = w.recv(c);
                        inferred = w.plain_type(r);
                    }
                    w.cx.pop_scope();
                }
                _ => w.visit(c),
            }
        });
        if declared_ty == NONE && inferred != NONE {
            self.cx.def_mut(d).type_name = inferred;
        }
        self.cx.pop_def();
        self.tparams.truncate(tp_mark);
    }

    /// `x::foo`, `this::foo`, `Type::foo` (parsed as a navigation).
    fn is_member_ref(&self, n: Node<'t>) -> bool {
        self.k(n) == K::NavigationExpr && self.nav_op(n) == b"::"
    }

    /// The types written in a variable declaration (not its name).
    fn var_types(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| match w.k(c.node()) {
            K::Identifier | K::Token | K::Comment => {}
            _ => w.visit(c),
        });
    }

    /// `get() = ..` / `set(value) { .. }`. `field` is the backing field and the setter's
    /// parameter a local; an extension property's accessors have its receiver as `this`.
    fn accessor(&mut self, c: &mut TreeCursor<'t>, recv_ty: u32) {
        self.cx.push_scope();
        let field = self.cx.intern(b"field");
        self.cx.declare(field, 0);
        if recv_ty != NONE {
            self.classes.push(Class::recv(recv_ty));
        }
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Identifier => {
                    let name = w.nm(ch);
                    w.cx.declare(name, 0);
                }
                K::Token | K::Comment => {}
                _ => w.visit(c),
            }
        });
        if recv_ty != NONE {
            self.classes.pop();
        }
        self.cx.pop_scope();
    }

    /// A local `val` / `var`: typed by its declaration or by what its initializer denotes. The
    /// names are declared after the initializer is walked (they are not in scope there).
    fn local_property(&mut self, c: &mut TreeCursor<'t>, p: &PropParts<'t>) {
        let init_at = p.init.map_or(usize::MAX, |x| x.start_byte());
        let mut init_ty = NONE;
        let mut bound: Option<(u32, u32)> = None;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::VariableDeclaration | K::MultiVariableDeclaration => w.decl_types(c),
                _ if ch.start_byte() == init_at => {
                    if matches!(w.k(ch), K::CallableReference) || w.is_member_ref(ch) {
                        let (f, line) = w.callable_ref(c);
                        if f != NONE {
                            bound = Some((f, line));
                        }
                    } else {
                        let r = w.recv(c);
                        init_ty = match r.k {
                            RK::Type => r.base,
                            _ => w.value_ty(r),
                        };
                    }
                }
                _ => w.visit(c),
            }
        });
        if let Some(v) = p.var
            && let Some(nm) = self.first_ident(v)
        {
            let ty = self.first_type(v).map_or(init_ty, |t| self.type_name(t));
            let name = self.nm(nm);
            if let Some((f, line)) = bound {
                let slot = self.joined(b".", name, b"", NONE);
                self.cx.add_ref(f, RefKind::Bind, line, slot, 255);
            }
            self.cx.declare(name, enc_local(ty));
        }
        if let Some(mv) = p.multi {
            for i in 0..mv.child_count() {
                let Some(v) = mv.child(i).filter(|x| self.k(*x) == K::VariableDeclaration) else { continue };
                if let Some(nm) = self.first_ident(v) {
                    let ty = self.first_type(v).map_or(NONE, |t| self.type_name(t));
                    let name = self.nm(nm);
                    self.cx.declare(name, enc_local(ty));
                }
            }
        }
    }

    /// The types of the variables a declaration lists, without declaring them.
    fn decl_types(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| match w.k(c.node()) {
            K::VariableDeclaration => w.var_types(c),
            K::Identifier | K::Token | K::Comment => {}
            _ => w.visit(c),
        });
    }

    // ----- statements with bindings -----

    fn for_stmt(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        let mut vars: Option<Node<'t>> = None;
        let mut after_in = false;
        let mut declared = false;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Comment | K::Label => {}
                K::Token => {
                    if w.tx(ch) == b"in" {
                        after_in = true;
                    }
                }
                K::VariableDeclaration | K::MultiVariableDeclaration if !after_in => {
                    vars = Some(ch);
                    w.decl_types(c);
                }
                _ => {
                    w.visit(c);
                    // The variables are in scope in the body only, not in the iterated expression.
                    if after_in && !declared {
                        declared = true;
                        if let Some(v) = vars {
                            w.declare_vars(v);
                        }
                    }
                }
            }
        });
        self.cx.pop_scope();
    }

    /// Declare the names of a variable declaration or a destructuring one, with written types.
    fn declare_vars(&mut self, v: Node<'t>) {
        if self.k(v) == K::VariableDeclaration {
            if let Some(nm) = self.first_ident(v) {
                let ty = self.first_type(v).map_or(NONE, |t| self.type_name(t));
                let name = self.nm(nm);
                self.cx.declare(name, enc_local(ty));
            }
            return;
        }
        for i in 0..v.child_count() {
            if let Some(x) = v.child(i).filter(|x| self.k(*x) == K::VariableDeclaration) {
                self.declare_vars(x);
            }
        }
    }

    fn catch_block(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        let n = c.node();
        let nm = self.first_ident(n);
        if let Some(nm) = nm {
            let ty = self.first_type(n).map_or(NONE, |t| self.type_name(t));
            let name = self.nm(nm);
            self.cx.declare(name, enc_local(ty));
        }
        self.children_except(c, nm.map_or(usize::MAX, |x| x.start_byte()));
        self.cx.pop_scope();
    }

    fn when_expr(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::WhenSubject => {
                    // `when (val s = x)`: `s` has the type of `x`.
                    let var = (0..ch.child_count()).filter_map(|i| ch.child(i)).find(|x| w.k(*x) == K::VariableDeclaration);
                    let mut ty = NONE;
                    let var_at = var.map_or(usize::MAX, |v| v.start_byte());
                    w.each(c, |w, c| {
                        let x = c.node();
                        match w.k(x) {
                            K::Token | K::Comment => {}
                            K::VariableDeclaration => w.var_types(c),
                            _ if x.start_byte() == var_at => {}
                            _ if var.is_some() => {
                                let r = w.recv(c);
                                ty = w.value_ty(r);
                            }
                            _ => w.visit(c),
                        }
                    });
                    if let Some(v) = var
                        && let Some(nm) = w.first_ident(v)
                    {
                        let ty = w.first_type(v).map_or(ty, |t| w.type_name(t));
                        let name = w.nm(nm);
                        w.cx.declare(name, enc_local(ty));
                    }
                }
                _ => w.visit(c),
            }
        });
        self.cx.pop_scope();
    }

    /// `return@label x`: the label is not a reference.
    fn return_expr(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let label = child(n, self.t.f_label).map_or(usize::MAX, |l| l.start_byte());
        self.children_except(c, label);
    }

    /// `break@outer`, `continue@outer`: the label names a loop. Other labels mark an expression.
    fn labeled(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        let jump = n.child(0).is_some_and(|l| self.k(l) == K::Label && matches!(self.tx(l), b"break@" | b"continue@"));
        if jump {
            return;
        }
        self.children(c);
    }

    // ----- lambdas -----

    /// `{ .. }` passed to a call (possibly with a label or annotations).
    fn lambda_arg(&mut self, c: &mut TreeCursor<'t>, ctx: LamCtx) {
        self.each(c, |w, c| match w.k(c.node()) {
            K::LambdaLiteral => w.lambda(c, ctx),
            K::Label | K::Token | K::Comment => {}
            _ => w.visit(c),
        });
    }

    fn lambda(&mut self, c: &mut TreeCursor<'t>, ctx: LamCtx) {
        let n = c.node();
        self.cx.push_scope();
        let explicit = (0..n.child_count().min(3)).filter_map(|i| n.child(i)).any(|x| self.k(x) == K::LambdaParameters);
        if !explicit {
            let it = self.cx.intern(b"it");
            self.cx.declare(it, enc_local(ctx.it));
        }
        let pushed = ctx.this != NONE || !ctx.plain;
        if ctx.this != NONE {
            self.classes.push(Class::recv(ctx.this));
        } else if pushed {
            self.classes.push(Class::unknown());
        }
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::LambdaParameters => {
                    let mut first = true;
                    w.each(c, |w, c| match w.k(c.node()) {
                        K::VariableDeclaration => {
                            let fb = if std::mem::take(&mut first) { ctx.it } else { NONE };
                            w.var_decl(c, fb);
                        }
                        K::MultiVariableDeclaration => {
                            first = false;
                            let v = c.node();
                            w.decl_types(c);
                            w.declare_vars(v);
                        }
                        K::Token | K::Comment => {}
                        _ => w.visit(c),
                    });
                }
                _ => w.visit(c),
            }
        });
        if pushed {
            self.classes.pop();
        }
        self.cx.pop_scope();
    }

    // ----- expressions -----

    /// A bare name in an expression. Locals emit nothing; a member of a class around it that is
    /// declared in this file is a member read of that class; a top-level property of this file
    /// or an imported one a plain read; a constant-style or capitalised name (an object, an enum
    /// class, a constant of another file) a plain read too; anything else a member of the
    /// innermost implicit receiver (inherited), or a plain read outside any class.
    fn identifier(&mut self, n: Node<'t>, write: bool) {
        let text = self.tx(n);
        if text.is_empty() || is_literal(text) {
            return;
        }
        let name = self.cx.intern(text);
        let line = line_of(n);
        let (mk, pk) = if write { (RefKind::MemberWrite, RefKind::Write) } else { (RefKind::MemberRead, RefKind::Read) };
        match self.cx.local_type(name) {
            Some(v) if v & B_MEMBER == 0 => {}
            Some(v) => {
                let q = self.owner_at(dec_level(v));
                self.cx.add_ref(name, mk, line, q, 255);
            }
            None => {
                if let Some((target, real)) = self.alias_of(name) {
                    return self.cx.add_ref(real, pk, line, target, 255);
                }
                let owner = self.static_owner_of(name);
                if owner != NONE {
                    return self.cx.add_ref(name, mk, line, owner, 255);
                }
                if self.tparam(name).is_some() {
                    return;
                }
                if self.top_value(name, text) || self.classes.is_empty() {
                    if !(starts_upper(text) && is_stop_type(text)) {
                        self.cx.add_ref(name, pk, line, NONE, 255);
                    }
                } else {
                    let q = self.class_name();
                    self.cx.add_ref(name, mk, line, q, 255);
                }
            }
        }
    }

    /// A bare name used as a receiver or qualifier (`x` in `x.f`, `x.m()`, `x::m`).
    fn recv_ident(&mut self, n: Node<'t>) -> Recv {
        let text = self.tx(n);
        if text.is_empty() || is_literal(text) {
            return Recv::UNKNOWN;
        }
        let name = self.cx.intern(text);
        let line = line_of(n);
        if let Some(v) = self.cx.local_type(name) {
            if v & B_MEMBER == 0 {
                return if v & B_FN != 0 { Recv::UNKNOWN } else { Recv::val(dec_ty(v)) };
            }
            let own = self.owner_at(dec_level(v));
            self.cx.add_ref(name, RefKind::MemberRead, line, own, 255);
            // A member of an anonymous body has no owner to name: fall back on its declared type.
            return if own != NONE { Recv { k: RK::Val, base: own, fld: name, call: false, mark: 0 } } else { Recv::val(dec_ty(v)) };
        }
        if self.tparam(name).is_some() {
            return Recv::UNKNOWN;
        }
        if let Some((target, real)) = self.alias_of(name) {
            if starts_upper(self.cx.name_bytes(real)) {
                self.cx.add_ref(real, RefKind::Type, line, target, 255);
                return Recv::ty(self.joined(b"", target, b"|", real));
            }
            self.cx.add_ref(real, RefKind::Read, line, target, 255);
            return Recv::val(self.joined(b"", target, b"|$", real));
        }
        let owner = self.static_owner_of(name);
        if owner != NONE {
            self.cx.add_ref(name, RefKind::MemberRead, line, owner, 255);
            return Recv { k: RK::Val, base: owner, fld: name, call: false, mark: 0 };
        }
        let info = self.info(name);
        if info & N_TYPE != 0 || type_like(text) {
            if !is_stop_type(text) {
                self.cx.add_ref(name, RefKind::Type, line, NONE, 255);
            }
            return Recv::ty(name);
        }
        if self.top_value(name, text) {
            // A top-level property, or a constant of another file or class (`DEFAULT.copy()`).
            self.cx.add_ref(name, RefKind::Read, line, NONE, 255);
            return Recv::val(self.joined(b"$", name, b"", NONE));
        }
        // Not declared in this file: an inherited member, a top-level property of the package,
        // or the first segment of a package name (`java.util.UUID.randomUUID()`).
        let mark = self.cx.facts.refs.len() as u32;
        if self.classes.is_empty() {
            self.cx.add_ref(name, RefKind::Read, line, NONE, 255);
            let base = self.joined(b"$", name, b"", NONE);
            return Recv { k: RK::Pkg, base, fld: NONE, call: false, mark };
        }
        let q = self.class_name();
        self.cx.add_ref(name, RefKind::MemberRead, line, q, 255);
        Recv { k: RK::Pkg, base: q, fld: if q == NONE { NONE } else { name }, call: false, mark }
    }

    /// Walk the expression under the cursor as a receiver: emit its references and return what
    /// it denotes.
    fn recv(&mut self, c: &mut TreeCursor<'t>) -> Recv {
        let n = c.node();
        match self.k(n) {
            K::Identifier => self.recv_ident(n),
            K::This => self.this_recv(n),
            K::Super => self.super_recv(c),
            K::NavigationExpr => self.navigation(c, false, true),
            K::CallExpr => self.call(c, 0),
            K::ObjectLiteral => self.object_literal(c),
            K::Parenthesized => {
                let mut r = Recv::UNKNOWN;
                self.each(c, |w, c| {
                    if !matches!(w.k(c.node()), K::Token | K::Comment) {
                        r = w.recv(c);
                    }
                });
                r
            }
            K::AsExpr => {
                let mut ty = NONE;
                let mut first = true;
                self.each(c, |w, c| {
                    let ch = c.node();
                    match w.k(ch) {
                        K::Token | K::Comment => {}
                        _ if std::mem::take(&mut first) => w.visit(c),
                        _ if w.is_type(ch) => {
                            ty = w.type_name(ch);
                            w.visit(c);
                        }
                        _ => w.visit(c),
                    }
                });
                Recv::val(ty)
            }
            K::UnaryExpr if self.has_token(n, b"!!") => {
                let mut r = Recv::UNKNOWN;
                self.each(c, |w, c| {
                    if !matches!(w.k(c.node()), K::Token | K::Comment) {
                        r = w.recv(c);
                    }
                });
                r
            }
            K::StringLiteral => {
                self.children(c);
                Recv::val(self.string_id())
            }
            _ => {
                self.visit(c);
                Recv::UNKNOWN
            }
        }
    }

    /// `this`, or `this@Outer` for the instance of an enclosing class.
    fn this_recv(&mut self, n: Node<'t>) -> Recv {
        match self.first_ident(n) {
            Some(l) => {
                let id = self.nm(l);
                if self.classes.iter().any(|cl| cl.own == id) { Recv::val(id) } else { Recv::UNKNOWN }
            }
            // In a lambda that may have a receiver of its own `this` is taken to be the class.
            None => {
                let base = self.classes.iter().rev().find(|cl| cl.flags & CF_UNKNOWN == 0).map_or(NONE, |cl| cl.this_ty);
                Recv { k: RK::This, base, fld: NONE, call: false, mark: 0 }
            }
        }
    }

    /// `super`, or `super<T>` naming the supertype whose member is meant.
    fn super_recv(&mut self, c: &mut TreeCursor<'t>) -> Recv {
        let n = c.node();
        match self.first_type(n) {
            Some(t) => {
                let ty = self.type_name(t);
                self.children(c);
                Recv::val(ty)
            }
            None => Recv::SUPER,
        }
    }

    /// `object.field` / `object?.field`, or the callable reference `object::field`. Emits the
    /// member reference and returns what the whole access denotes. Only an access that is
    /// itself a qualifier (`as_recv`) can name a nested type or a package.
    fn navigation(&mut self, c: &mut TreeCursor<'t>, write: bool, as_recv: bool) -> Recv {
        let n = c.node();
        if self.nav_op(n) == b"::" {
            self.callable_ref(c);
            return Recv::UNKNOWN;
        }
        let mut r = Recv::UNKNOWN;
        let mut first = true;
        let mut field = None;
        let mut object = None;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if std::mem::take(&mut first) => {
                    object = Some(ch);
                    r = w.recv(c);
                }
                K::Identifier => field = Some(ch),
                _ => w.visit(c),
            }
        });
        let Some(f) = field.filter(|f| f.end_byte() > f.start_byte()) else { return Recv::UNKNOWN };
        let text = self.tx(f);
        let name = self.cx.intern(text);
        let line = line_of(f);
        let kind = if write { RefKind::MemberWrite } else { RefKind::MemberRead };
        match r.k {
            RK::Type if as_recv && self.is_companion(name) => r,
            // (`Unit.B.toBytes()`: a one-letter name after a type is a constant, not a nested type.)
            RK::Type if as_recv && text.len() > 1 && type_like(text) => {
                // A nested type used in an expression: `Outer.Inner.CONST`, `Outer.Inner::class`.
                if !is_stop_type(text) {
                    let outer = self.simple_of(r.base);
                    self.cx.add_ref(name, RefKind::Type, line, outer, 255);
                }
                Recv::ty(self.path(r.base, name, false))
            }
            RK::Type => {
                self.cx.add_ref(name, kind, line, r.base, 255);
                Recv { k: RK::Val, base: r.base, fld: name, call: false, mark: 0 }
            }
            // (Within a package name any capitalised segment is a type: `java.util.UUID`.)
            RK::Pkg if as_recv && starts_upper(text) => {
                // `java.util.UUID`: the chain so far was a package name, not member reads. The
                // type keeps its package: `java.util|UUID`.
                self.cx.facts.refs.truncate(r.mark as usize);
                let q = object.map_or(NONE, |o| self.dotted(o));
                if !is_stop_type(text) {
                    self.cx.add_ref(name, RefKind::Type, line, q, 255);
                }
                Recv::ty(if q == NONE { name } else { self.joined(b"", q, b"|", name) })
            }
            RK::Super => {
                let q = self.super_qual();
                self.cx.add_ref(name, kind, line, q, 255);
                Recv::UNKNOWN
            }
            _ => {
                let q = if r.k == RK::This { self.this_member_qual(name) } else { self.qual(r) };
                self.cx.add_ref(name, kind, line, q, 255);
                let k = if r.k == RK::Pkg && text.first().is_some_and(|b| b.is_ascii_lowercase()) { RK::Pkg } else { RK::Val };
                Recv { k, base: q, fld: if q == NONE { NONE } else { name }, call: false, mark: r.mark }
            }
        }
    }

    /// Does a bare value name that no scope declares denote a top-level property rather than a
    /// member of the implicit receiver? Members of the receivers shadow imports: an imported
    /// lower-case name (usually an extension function, `import okio.buffer`) read inside a class
    /// or an extension is the receiver's property. Capitalised names are objects, classes and
    /// constants.
    fn top_value(&self, name: u32, text: &[u8]) -> bool {
        let info = self.info(name);
        info & (N_TOPVAL | N_TOPFN | N_TYPE) != 0 || starts_upper(text) || (info & N_IMPVAL != 0 && self.classes.is_empty())
    }

    #[inline]
    fn is_companion(&self, name: u32) -> bool {
        self.cx.name_bytes(name) == b"Companion" || self.info(name) & N_COMPANION != 0
    }

    /// A call. `extra`: arguments of an outer call that this one is the callee of (a trailing
    /// lambda after the parenthesised arguments, `f(a) { .. }`). Emits the call reference and
    /// returns what the call denotes as a receiver.
    fn call(&mut self, c: &mut TreeCursor<'t>, extra: u8) -> Recv {
        let n = c.node();
        let mut callee = None;
        let mut args = None;
        let mut lambda = false;
        for i in 0..n.child_count() {
            let Some(ch) = n.child(i) else { break };
            match self.k(ch) {
                K::Token | K::Comment | K::TypeArguments => {}
                K::ValueArguments => args = Some(ch),
                K::AnnotatedLambda => lambda = true,
                _ if callee.is_none() => callee = Some(ch),
                _ => {}
            }
        }
        let Some(callee) = callee else {
            self.children(c);
            return Recv::UNKNOWN;
        };
        // `f(a) { .. }` nests the call with the lambda around the call with the arguments.
        if args.is_none()
            && lambda
            && self.k(callee) == K::CallExpr
            && !(0..callee.child_count()).filter_map(|i| callee.child(i)).any(|x| self.k(x) == K::AnnotatedLambda)
        {
            let mut out = Recv::UNKNOWN;
            let mut ctx = LamCtx::NONE;
            self.each(c, |w, c| {
                let ch = c.node();
                match w.k(ch) {
                    K::Token | K::Comment => {}
                    K::CallExpr if ch.start_byte() == callee.start_byte() => {
                        out = w.call(c, extra.saturating_add(1));
                        ctx = std::mem::replace(&mut w.lam, LamCtx::NONE);
                    }
                    K::AnnotatedLambda => w.lambda_arg(c, ctx),
                    _ => w.visit(c),
                }
            });
            return out;
        }
        let mut nargs: u32 = extra as u32 + lambda as u32;
        let mut broken = false;
        if let Some(a) = args {
            for i in 0..a.child_count() {
                let Some(ch) = a.child(i) else { break };
                match self.k(ch) {
                    K::ValueArgument => nargs += 1,
                    K::Error => broken = true,
                    _ => {}
                }
            }
        }
        let nargs = if broken { 255 } else { nargs.min(254) as u8 };
        // `x.apply { .. }` takes nothing but the lambda; `x.apply(y)` is some other `apply`.
        let lambda_only = args.is_none() && lambda && extra == 0;
        let mut out = Recv::UNKNOWN;
        let mut ctx = LamCtx::NONE;
        let mut callee_name = NONE;
        let mut with = false;
        let callee_at = callee.start_byte();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if ch.start_byte() == callee_at => {
                    let (r, cx, nm) = w.callee(c, nargs, lambda_only);
                    out = r;
                    ctx = cx;
                    callee_name = nm;
                    with = nm != NONE && w.cx.name_bytes(nm) == b"with" && w.k(ch) == K::Identifier && w.info(nm) & N_TOPFN == 0;
                }
                K::ValueArguments => {
                    let (_, first) = w.arguments(c, callee_name);
                    if with {
                        // `with(x) { .. }`: `x` is the receiver of the lambda.
                        ctx.this = first;
                    }
                }
                K::AnnotatedLambda => w.lambda_arg(c, std::mem::replace(&mut ctx, LamCtx::NONE)),
                _ => w.visit(c),
            }
        });
        // The receiver of a lambda that is still to come (`f(a) { .. }`).
        self.lam = if lambda { LamCtx::NONE } else { ctx };
        out
    }

    /// The callee of a call: emits the call reference. Returns what the call denotes, the
    /// receiver its lambda gets, and the called name.
    fn callee(&mut self, c: &mut TreeCursor<'t>, nargs: u8, lambda_only: bool) -> (Recv, LamCtx, u32) {
        let n = c.node();
        match self.k(n) {
            K::Identifier => self.bare_call(n, nargs),
            K::NavigationExpr if self.nav_op(n) != b"::" => self.member_call(c, nargs, lambda_only),
            _ => {
                self.recv(c);
                (Recv::UNKNOWN, LamCtx::NONE, NONE)
            }
        }
    }

    /// `foo(..)`: a local function, a constructor, a function imported or declared at top level,
    /// a method of a class around the call, or a member of the implicit receiver.
    fn bare_call(&mut self, n: Node<'t>, nargs: u8) -> (Recv, LamCtx, u32) {
        let text = self.tx(n);
        if text.is_empty() {
            return (Recv::UNKNOWN, LamCtx::NONE, NONE);
        }
        let name = self.cx.intern(text);
        let line = line_of(n);
        if let Some(v) = self.cx.local_type(name) {
            if v & B_MEMBER != 0 {
                // A property holding a function.
                let q = self.owner_at(dec_level(v));
                self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
                return (Recv::UNKNOWN, LamCtx::NONE, name);
            }
            if v & B_FN != 0 {
                self.cx.add_ref(name, RefKind::Call, line, NONE, nargs);
                return (Recv::val(self.joined(b"", name, b"()", NONE)), LamCtx::NONE, name);
            }
            // A local holding a function: nothing to link.
            return (Recv::UNKNOWN, LamCtx::NONE, name);
        }
        if let Some((target, real)) = self.alias_of(name) {
            if starts_upper(self.cx.name_bytes(real)) {
                self.cx.add_ref(real, RefKind::New, line, target, nargs);
                return (Recv::val(self.joined(b"", target, b"|", real)), LamCtx::NONE, real);
            }
            self.cx.add_ref(real, RefKind::Call, line, target, nargs);
            return (Recv::val(self.joined(b"", target, b"|", real)), LamCtx::NONE, real);
        }
        let info = self.info(name);
        let declared = info & (N_TOPFN | N_IMPVAL | N_STATIC) != 0 || self.method_owner(name).is_some();
        if !declared && is_stop_call(text) {
            return (Recv::UNKNOWN, LamCtx::after(text), name);
        }
        if !declared && (info & N_TYPE != 0 || type_like(text)) {
            if !is_stop_type(text) {
                self.cx.add_ref(name, RefKind::New, line, NONE, nargs);
            }
            return (Recv::val(name), LamCtx::NONE, name);
        }
        // In a lambda whose receiver is not known the qualifier is `NONE`: the name is looked up
        // where a bare name is (the file, its imports, its package), which finds a top-level
        // function as well as a member of a builder the lambda may be run on.
        let q = self.bare_qual(name);
        self.cx.add_ref(name, RefKind::Call, line, q, nargs);
        let r = if q == NONE { Recv::val(self.joined(b"", name, b"()", NONE)) } else { Recv::member(q, name, true) };
        (r, LamCtx::after(text), name)
    }

    /// `recv.foo(..)` / `recv?.foo(..)`.
    fn member_call(&mut self, c: &mut TreeCursor<'t>, nargs: u8, lambda_only: bool) -> (Recv, LamCtx, u32) {
        let mut r = Recv::UNKNOWN;
        let mut first = true;
        let mut field = None;
        let mut object = None;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if std::mem::take(&mut first) => {
                    object = Some(ch);
                    r = w.recv(c);
                }
                K::Identifier => field = Some(ch),
                _ => w.visit(c),
            }
        });
        let Some(f) = field.filter(|f| f.end_byte() > f.start_byte()) else { return (Recv::UNKNOWN, LamCtx::NONE, NONE) };
        let text = self.tx(f);
        let name = self.cx.intern(text);
        let line = line_of(f);
        // Scope functions: the lambda gets the receiver as `it` or as `this`; `also` and `apply`
        // return the receiver itself.
        match text {
            _ if !lambda_only => {}
            b"let" | b"also" | b"takeIf" | b"takeUnless" | b"use" => {
                let ty = self.value_ty(r);
                let out = if matches!(text, b"let" | b"use") { Recv::UNKNOWN } else { Recv::val(ty) };
                return (out, LamCtx { it: ty, this: NONE, plain: true }, name);
            }
            b"apply" | b"run" => {
                let ty = self.value_ty(r);
                let out = if text == b"apply" { Recv::val(ty) } else { Recv::UNKNOWN };
                return (out, LamCtx { it: NONE, this: ty, plain: false }, name);
            }
            _ => {}
        }
        let q = match r.k {
            RK::Type if self.is_companion(name) => return (r, LamCtx::NONE, name),
            RK::Type if type_like(text) => {
                // `Outer.Inner(..)`: a nested class is constructed.
                let outer = self.simple_of(r.base);
                if !is_stop_type(text) {
                    self.cx.add_ref(name, RefKind::New, line, outer, nargs);
                }
                return (Recv::val(self.path(r.base, name, false)), LamCtx::NONE, name);
            }
            RK::Type => {
                self.cx.add_ref(name, RefKind::Call, line, r.base, nargs);
                r.base
            }
            RK::Pkg if starts_upper(text) => {
                // `java.util.ArrayList()`, `a.b.Foo()`: a class written with its package.
                self.cx.facts.refs.truncate(r.mark as usize);
                let q = object.map_or(NONE, |o| self.dotted(o));
                if !is_stop_type(text) {
                    self.cx.add_ref(name, RefKind::New, line, q, nargs);
                }
                return (Recv::val(if q == NONE { name } else { self.joined(b"", q, b"|", name) }), LamCtx::NONE, name);
            }
            RK::Super => {
                let q = self.super_qual();
                self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
                // The result is declared by an ancestor; the lookup starts at the class itself
                // (an override keeps the result type) and walks up from there.
                self.classes.last().map_or(NONE, |cl| cl.this_ty)
            }
            RK::This => {
                let q = self.this_member_qual(name);
                self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
                q
            }
            RK::Val if type_like(text) && !is_stop_type(text) => {
                // `order.Line(3)`: an inner class constructed through an instance of the outer
                // one, as Java's `order.new Line(3)`.
                let outer = self.qual(r);
                self.cx.add_ref(name, RefKind::New, line, outer, nargs);
                return (Recv::val(if outer == NONE { name } else { self.path(outer, name, false) }), LamCtx::NONE, name);
            }
            _ => {
                let q = self.qual(r);
                self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
                q
            }
        };
        (Recv::member(q, name, true), LamCtx::after(text), name)
    }

    /// Walk the argument list of a call of `callee` and count the arguments. A callable
    /// reference handed over directly is a callback registration: besides its own reference it
    /// is recorded as an argument of `callee`. Returns the count (unknown when the list did not
    /// parse) and the type of the first argument.
    fn arguments(&mut self, c: &mut TreeCursor<'t>, callee: u32) -> (u8, u32) {
        let mut n = 0u32;
        let mut broken = false;
        let mut first_ty = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::ValueArgument => {
                    n += 1;
                    let first = n == 1;
                    w.each(c, |w, c| {
                        let x = c.node();
                        match w.k(x) {
                            K::Token | K::Comment => {}
                            // The name of a named argument is no reference.
                            K::Identifier if w.is_arg_name(x) => {}
                            K::CallableReference => {
                                let (f, line) = w.callable_ref(c);
                                if f != NONE && callee != NONE {
                                    w.cx.add_ref(f, RefKind::Arg, line, callee, 255);
                                }
                            }
                            K::NavigationExpr if w.is_member_ref(x) => {
                                let (f, line) = w.callable_ref(c);
                                if f != NONE && callee != NONE {
                                    w.cx.add_ref(f, RefKind::Arg, line, callee, 255);
                                }
                            }
                            _ => {
                                let r = w.recv(c);
                                if first {
                                    first_ty = w.value_ty(r);
                                }
                            }
                        }
                    });
                }
                K::Error => {
                    broken = true;
                    w.visit(c);
                }
                _ => w.visit(c),
            }
        });
        (if broken { 255 } else { n.min(254) as u8 }, first_ty)
    }

    /// `::foo`, `T::foo`, `x::foo`, `this::foo`, `::Foo` (a constructor), `T::class`. Returns the
    /// referenced function or property (`NONE` for constructors and class literals) and its line.
    fn callable_ref(&mut self, c: &mut TreeCursor<'t>) -> (u32, u32) {
        let n = c.node();
        let mut q = NONE;
        let mut has_recv = false;
        let mut target = None;
        let navigation = self.k(n) == K::NavigationExpr;
        let mut first = true;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if navigation && std::mem::take(&mut first) => {
                    has_recv = true;
                    let r = w.recv(c);
                    q = match r.k {
                        RK::Type => r.base,
                        RK::This => r.base,
                        _ => w.qual(r),
                    };
                }
                K::Identifier => target = Some(ch),
                _ if w.is_type(ch) => {
                    has_recv = true;
                    q = w.type_name(ch);
                    w.visit(c);
                }
                _ => w.visit(c),
            }
        });
        let Some(t) = target.filter(|t| t.end_byte() > t.start_byte()) else { return (NONE, 0) };
        let text = self.tx(t);
        if text == b"class" {
            return (NONE, 0);
        }
        let name = self.cx.intern(text);
        let line = line_of(t);
        if !has_recv {
            let info = self.info(name);
            if (info & N_TYPE != 0 || type_like(text)) && info & (N_TOPFN | N_IMPVAL) == 0 && self.method_owner(name).is_none() {
                // `::Foo` instantiates `Foo`.
                if !is_stop_type(text) {
                    match self.alias_of(name) {
                        Some((target, real)) => self.cx.add_ref(real, RefKind::New, line, target, 255),
                        None => self.cx.add_ref(name, RefKind::New, line, NONE, 255),
                    }
                }
                return (NONE, 0);
            }
            if is_stop_call(text) && info & (N_TOPFN | N_IMPVAL) == 0 && self.method_owner(name).is_none() {
                return (NONE, 0);
            }
            q = self.bare_qual(name);
        }
        // A member used as a value is a member read, as `obj.method` would be: the resolver
        // pairs it with the `Arg` or `Bind` of the same line. A top-level function is a read.
        let kind = if q == NONE && !has_recv { RefKind::Read } else { RefKind::MemberRead };
        self.cx.add_ref(name, kind, line, q, 255);
        (name, line)
    }

    fn assignment(&mut self, c: &mut TreeCursor<'t>) {
        let mut idx = 0;
        let mut slot: Option<(u32, u32, bool)> = None;
        self.each(c, |w, c| {
            let ch = c.node();
            let k = w.k(ch);
            if k == K::Token || k == K::Comment {
                return;
            }
            idx += 1;
            if idx == 1 {
                let from = w.cx.facts.refs.len();
                w.lvalue(c);
                if matches!(k, K::Identifier | K::NavigationExpr) {
                    let last = w.cx.facts.refs.get(from..).and_then(|rs| rs.last());
                    slot = last.filter(|r| matches!(r.kind, RefKind::MemberWrite | RefKind::Write)).map(|r| (r.qual, r.name, r.kind == RefKind::Write));
                }
            } else if k == K::CallableReference || w.is_member_ref(ch) {
                // A function stored into a member or a variable: a binding into that slot.
                let (f, line) = w.callable_ref(c);
                if let (Some((owner, m, plain)), true) = (slot, f != NONE) {
                    let s = if plain { w.joined(b".", m, b"", NONE) } else { w.joined(b"", owner, b".", m) };
                    w.cx.add_ref(f, RefKind::Bind, line, s, 255);
                }
            } else {
                w.visit(c);
            }
        });
    }

    /// An assignment target. Storing into an element (`a[i] = v`) counts as a write of `a`, as in
    /// the Java extractor.
    fn lvalue(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        match self.k(n) {
            K::Identifier => self.identifier(n, true),
            K::NavigationExpr => {
                self.navigation(c, true, false);
            }
            K::IndexExpr => {
                let mut first = true;
                self.each(c, |w, c| {
                    let k = w.k(c.node());
                    if k == K::Token || k == K::Comment {
                        return;
                    }
                    if std::mem::take(&mut first) { w.lvalue(c) } else { w.visit(c) }
                });
            }
            K::Parenthesized => self.each(c, |w, c| {
                if !matches!(w.k(c.node()), K::Token | K::Comment) {
                    w.lvalue(c);
                }
            }),
            _ => self.visit(c),
        }
    }

    fn unary(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        if self.has_token(n, b"++") || self.has_token(n, b"--") {
            return self.each(c, |w, c| {
                if !matches!(w.k(c.node()), K::Token | K::Comment) {
                    w.lvalue(c);
                }
            });
        }
        self.children(c);
    }

    /// `a op b` with a named infix function: a call of `op` on `a` with one argument. The
    /// standard library's (`to`, `until`, `shl`, ..) emit nothing.
    fn infix(&mut self, c: &mut TreeCursor<'t>) -> Recv {
        let mut idx = 0;
        let mut r = Recv::UNKNOWN;
        let mut out = Recv::UNKNOWN;
        self.each(c, |w, c| {
            let ch = c.node();
            let k = w.k(ch);
            if k == K::Token || k == K::Comment {
                return;
            }
            idx += 1;
            match idx {
                1 => r = w.recv(c),
                2 if k == K::Identifier => {
                    let text = w.tx(ch);
                    if text.is_empty() || is_stop_infix(text) {
                        return;
                    }
                    let name = w.cx.intern(text);
                    let q = match r.k {
                        RK::Val | RK::This | RK::Pkg => w.qual(r),
                        _ => NONE,
                    };
                    w.cx.add_ref(name, RefKind::MemberCall, line_of(ch), q, 1);
                    out = Recv::member(q, name, true);
                }
                _ => w.visit(c),
            }
        });
        out
    }

    /// `a + b + c + ...` nests to the left, one level per operand. Long concatenations would
    /// run into the depth guard, so the left spine is walked with a loop instead of recursion.
    fn binary(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        let mut levels = 0u32;
        while self.k(c.node()) == K::BinaryExpr && c.goto_first_child() {
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

    /// Names directly under an ERROR node are fragments the parser could not place (sometimes
    /// keywords read as names): noise, not references.
    fn error(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.facts.parse_errors += 1;
        self.each(c, |w, c| {
            if w.k(c.node()) != K::Identifier {
                w.visit(c);
            }
        });
    }
}

/// Append `text` without whitespace and comments.
fn push_clean(text: &[u8], buf: &mut Vec<u8>) {
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
        if b > b' ' && b != b'`' {
            buf.push(b);
        }
        i += 1;
    }
}

#[inline]
fn starts_upper(b: &[u8]) -> bool {
    b.first().is_some_and(|c| c.is_ascii_uppercase())
}

/// Does the name look like a type (`Foo`, `IOUtils`, `T`) rather than a constant (`MAX_SIZE`,
/// `INSTANCE`), a variable or a package segment?
#[inline]
fn type_like(b: &[u8]) -> bool {
    match b.first() {
        Some(c) if c.is_ascii_uppercase() => b.len() == 1 || b[1..].iter().any(|x| x.is_ascii_lowercase()),
        _ => false,
    }
}

/// The grammar reads these literals as identifiers.
#[inline]
fn is_literal(b: &[u8]) -> bool {
    matches!(b, b"null" | b"true" | b"false")
}

/// Types that never produce a reference: the Kotlin built-ins and the collection interfaces
/// every file sees without an import.
fn is_stop_type(b: &[u8]) -> bool {
    matches!(
        b,
        b"Int"
            | b"Long"
            | b"Short"
            | b"Byte"
            | b"Double"
            | b"Float"
            | b"Boolean"
            | b"Char"
            | b"String"
            | b"Unit"
            | b"Any"
            | b"Nothing"
            | b"Number"
            | b"CharSequence"
            | b"Comparable"
            | b"Array"
            | b"IntArray"
            | b"LongArray"
            | b"ShortArray"
            | b"ByteArray"
            | b"CharArray"
            | b"FloatArray"
            | b"DoubleArray"
            | b"BooleanArray"
            | b"UInt"
            | b"ULong"
            | b"UShort"
            | b"UByte"
            | b"List"
            | b"MutableList"
            | b"Map"
            | b"MutableMap"
            | b"Set"
            | b"MutableSet"
            | b"Collection"
            | b"MutableCollection"
            | b"Iterable"
            | b"MutableIterable"
            | b"Iterator"
            | b"Sequence"
            | b"Pair"
            | b"Triple"
            | b"Lazy"
            | b"Object"
    )
}

/// Annotations of the language and the JVM interop that say nothing about the code base.
fn is_stop_annotation(b: &[u8]) -> bool {
    matches!(
        b,
        b"Suppress"
            | b"JvmStatic"
            | b"JvmField"
            | b"JvmName"
            | b"JvmOverloads"
            | b"JvmInline"
            | b"JvmSynthetic"
            | b"JvmSuppressWildcards"
            | b"JvmWildcard"
            | b"JvmDefault"
            | b"JvmMultifileClass"
            | b"Throws"
            | b"Deprecated"
            | b"Volatile"
            | b"Transient"
            | b"Synchronized"
            | b"PublishedApi"
            | b"OptIn"
            | b"Target"
            | b"Retention"
            | b"MustBeDocumented"
            | b"Repeatable"
            | b"Override"
            | b"SuppressWarnings"
            | b"FunctionalInterface"
    )
}

/// Functions of the standard library called without a receiver.
fn is_stop_call(b: &[u8]) -> bool {
    matches!(
        b,
        b"println"
            | b"print"
            | b"listOf"
            | b"listOfNotNull"
            | b"mutableListOf"
            | b"arrayListOf"
            | b"emptyList"
            | b"emptyMap"
            | b"emptySet"
            | b"emptyArray"
            | b"emptySequence"
            | b"setOf"
            | b"mutableSetOf"
            | b"hashSetOf"
            | b"linkedSetOf"
            | b"mapOf"
            | b"mutableMapOf"
            | b"hashMapOf"
            | b"linkedMapOf"
            | b"arrayOf"
            | b"arrayOfNulls"
            | b"intArrayOf"
            | b"longArrayOf"
            | b"byteArrayOf"
            | b"charArrayOf"
            | b"booleanArrayOf"
            | b"doubleArrayOf"
            | b"floatArrayOf"
            | b"shortArrayOf"
            | b"sequenceOf"
            | b"require"
            | b"requireNotNull"
            | b"check"
            | b"checkNotNull"
            | b"error"
            | b"TODO"
            | b"repeat"
            | b"lazy"
            | b"run"
            | b"with"
            | b"apply"
            | b"let"
            | b"also"
            | b"takeIf"
            | b"takeUnless"
            | b"buildString"
            | b"buildList"
            | b"buildMap"
            | b"buildSet"
            | b"maxOf"
            | b"minOf"
            | b"assert"
            | b"synchronized"
    )
}

/// Functions of the standard library whose lambda has no receiver of its own: in it, `this`
/// and unqualified names mean what they mean around the call.
fn is_plain_lambda_fn(b: &[u8]) -> bool {
    matches!(
        b,
        b"forEach"
            | b"forEachIndexed"
            | b"map"
            | b"mapNotNull"
            | b"mapIndexed"
            | b"flatMap"
            | b"filter"
            | b"filterNot"
            | b"filterIsInstance"
            | b"any"
            | b"all"
            | b"none"
            | b"count"
            | b"first"
            | b"firstOrNull"
            | b"last"
            | b"lastOrNull"
            | b"find"
            | b"single"
            | b"singleOrNull"
            | b"sortedBy"
            | b"sortedByDescending"
            | b"sortBy"
            | b"groupBy"
            | b"associate"
            | b"associateBy"
            | b"associateWith"
            | b"partition"
            | b"sumOf"
            | b"maxByOrNull"
            | b"minByOrNull"
            | b"fold"
            | b"reduce"
            | b"onEach"
            | b"takeWhile"
            | b"dropWhile"
            | b"removeAll"
            | b"removeIf"
            | b"retainAll"
            | b"getOrPut"
            | b"getOrElse"
            | b"computeIfAbsent"
            | b"let"
            | b"also"
            | b"takeIf"
            | b"takeUnless"
            | b"use"
            | b"repeat"
            | b"lazy"
            | b"require"
            | b"requireNotNull"
            | b"check"
            | b"checkNotNull"
            | b"synchronized"
            | b"thread"
            | b"assertFailsWith"
            | b"assertFails"
    )
}

/// Infix functions of the standard library.
fn is_stop_infix(b: &[u8]) -> bool {
    matches!(b, b"to" | b"until" | b"downTo" | b"step" | b"and" | b"or" | b"xor" | b"shl" | b"shr" | b"ushr")
}
