//! Java extractor (tree-sitter-java).
//!
//! Java has no free functions and no globals: every name that is not a local is a member of
//! some class, reached through an explicit receiver (`obj.f()`, `Type.f()`, `this.f`) or through
//! the implicit `this` and the lexically enclosing classes (`f()`, `count`). The walker
//! therefore tracks:
//!   * locals with their declared types (parameters, local variables, loop / catch / lambda /
//!     pattern / resource variables), so that `x.foo()` carries the receiver type,
//!   * the members each enclosing class declares in this file (collected when its body is
//!     entered, because members may be used before they are declared), so that a bare `count`
//!     or `svc.go()` is attributed to the class that declares `count` / `svc`,
//!   * the type parameters in scope, which stand for their bound and never produce references.
//!
//! Conventions the resolver can rely on:
//!   * Definitions. The package is a `Namespace` named `a.b.c`, the first definition of its
//!     file; top-level types have no parent. Records are `Class`, annotation types `Interface`,
//!     their elements `Property`, record components `Field`. Constructors are methods named
//!     after their class. Members of an anonymous class or of an enum constant body have the
//!     enclosing definition (method, field, enum constant) as parent; the `Extends T` reference
//!     next to the `New T` names their supertype. `private` is `LOCAL`; `public`, written or
//!     implied (interface members, enum constants, record components), is `EXPORTED`. A
//!     definition starts at its first annotation, its signature after the leading annotations.
//!   * Qualifiers. `f()` is a `Call` qualified by the innermost enclosing named class `C`: the
//!     method may be inherited, belong to an outer class or be imported statically (the import
//!     record has target = the class, name = the member). A method that the anonymous class
//!     around the call declares itself has no class name: the call is unqualified.
//!     `this.f()` is a `MemberCall C`, `super.f()` a `MemberCall ^C`, `x.f()` a `MemberCall T`
//!     for a local of type `T`, `m.f()` a `MemberCall D::m` where `D` is the enclosing class
//!     that declares the member `m` in this file, else `C`. `Type.f()` is a `Call Type`. Bare
//!     names and field accesses follow the same rules as `MemberRead` / `MemberWrite`. In an
//!     anonymous class `this` has the type the class was created from.
//!   * Receiver type paths. A call result is the segment `::m()` on the path of its receiver:
//!     `repo.find(id).getName()` is a `MemberCall Repo::find()`, `this.svc.client().send()` a
//!     `MemberCall C::svc::client()`, `Foo.of(x).build()` a `MemberCall Foo::of()`, and after an
//!     unqualified `make().go()` a `MemberCall C::make()`. A `var` is typed by the path of its
//!     initializer. Paths stop after six segments.
//!   * Type names (`type_name`, the roots of paths, the qualifier of a static call). A type
//!     written with its package keeps it as a module qualifier: `java.util.List` is
//!     `java.util|List`. Nested types are members of the type around them (`parent`), so a type
//!     written `Outer.Inner` is the path `Outer::Inner` (`a.b.Outer.Inner` is `a.b|Outer::Inner`).
//!     The `Type` / `New` / `Extends` / `Implements` reference of a qualified type names its
//!     last segment and is qualified by what it was written with: the package (`a.b`) or the
//!     simple name of the outer type (`Outer`).
//!   * `T::m` is a `Read` of `m` qualified like a receiver, `T::new` a `New`. Handed directly to
//!     a call it is also an `Arg` of that call; stored directly into a member, also a `Bind`.

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
    Program,
    Identifier,
    TypeIdentifier,
    ScopedIdentifier,
    ScopedTypeIdentifier,
    GenericType,
    ArrayType,
    AnnotatedType,
    This,
    Super,
    PackageDeclaration,
    ImportDeclaration,
    ModuleDeclaration,
    ClassDeclaration,
    InterfaceDeclaration,
    EnumDeclaration,
    RecordDeclaration,
    AnnotationTypeDeclaration,
    ClassBody,
    EnumBodyDeclarations,
    EnumConstant,
    FieldDeclaration,
    MethodDeclaration,
    ConstructorDeclaration,
    CompactConstructor,
    AnnotationElement,
    Modifiers,
    Annotation,
    AnnotationArgumentList,
    ElementValuePair,
    TypeParameter,
    TypeBound,
    Superclass,
    SuperInterfaces,
    ExtendsInterfaces,
    TypeList,
    FormalParameters,
    FormalParameter,
    SpreadParameter,
    ReceiverParameter,
    InferredParameters,
    VariableDeclarator,
    LocalVariableDeclaration,
    Block,
    ConstructorBody,
    Scoped,
    EnhancedFor,
    TryWithResources,
    Resource,
    CatchClause,
    CatchFormalParameter,
    CatchType,
    LabeledStatement,
    SwitchExpression,
    SwitchLabel,
    ExplicitConstructorInvocation,
    MethodInvocation,
    FieldAccess,
    ObjectCreation,
    ArrayCreation,
    MethodReference,
    Assignment,
    BinaryExpression,
    Update,
    Lambda,
    Instanceof,
    TypePattern,
    RecordPattern,
    CastExpression,
    Parenthesized,
    ArrayAccess,
    ArgumentList,
    TypeArguments,
    StringLiteral,
    ClassLiteral,
    Asterisk,
}

struct Tables {
    kinds: KindTable<K>,
    f_name: u16,
    f_body: u16,
    f_type: u16,
    f_type_parameters: u16,
    f_declarator: u16,
    f_value: u16,
    f_dimensions: u16,
    f_field: u16,
    f_object: u16,
    f_scope: u16,
    f_element: u16,
    f_key: u16,
    f_right: u16,
    kw_public: u16,
    kw_private: u16,
    kw_abstract: u16,
    kw_native: u16,
    kw_static: u16,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let lang: tree_sitter::Language = tree_sitter_java::LANGUAGE.into();
        let special: &[(&str, K)] = &[
            ("ERROR", K::Error),
            ("program", K::Program),
            ("identifier", K::Identifier),
            ("type_identifier", K::TypeIdentifier),
            ("scoped_identifier", K::ScopedIdentifier),
            ("scoped_type_identifier", K::ScopedTypeIdentifier),
            ("generic_type", K::GenericType),
            ("array_type", K::ArrayType),
            ("annotated_type", K::AnnotatedType),
            ("this", K::This),
            ("super", K::Super),
            ("package_declaration", K::PackageDeclaration),
            ("import_declaration", K::ImportDeclaration),
            ("module_declaration", K::ModuleDeclaration),
            ("class_declaration", K::ClassDeclaration),
            ("interface_declaration", K::InterfaceDeclaration),
            ("enum_declaration", K::EnumDeclaration),
            ("record_declaration", K::RecordDeclaration),
            ("annotation_type_declaration", K::AnnotationTypeDeclaration),
            ("class_body", K::ClassBody),
            ("interface_body", K::ClassBody),
            ("enum_body", K::ClassBody),
            ("annotation_type_body", K::ClassBody),
            ("enum_body_declarations", K::EnumBodyDeclarations),
            ("enum_constant", K::EnumConstant),
            ("field_declaration", K::FieldDeclaration),
            ("constant_declaration", K::FieldDeclaration),
            ("method_declaration", K::MethodDeclaration),
            ("constructor_declaration", K::ConstructorDeclaration),
            ("compact_constructor_declaration", K::CompactConstructor),
            ("annotation_type_element_declaration", K::AnnotationElement),
            ("modifiers", K::Modifiers),
            ("marker_annotation", K::Annotation),
            ("annotation", K::Annotation),
            ("annotation_argument_list", K::AnnotationArgumentList),
            ("element_value_pair", K::ElementValuePair),
            ("type_parameter", K::TypeParameter),
            ("type_bound", K::TypeBound),
            ("superclass", K::Superclass),
            ("super_interfaces", K::SuperInterfaces),
            ("extends_interfaces", K::ExtendsInterfaces),
            ("type_list", K::TypeList),
            ("formal_parameters", K::FormalParameters),
            ("formal_parameter", K::FormalParameter),
            ("spread_parameter", K::SpreadParameter),
            ("receiver_parameter", K::ReceiverParameter),
            ("inferred_parameters", K::InferredParameters),
            ("variable_declarator", K::VariableDeclarator),
            ("local_variable_declaration", K::LocalVariableDeclaration),
            ("block", K::Block),
            ("constructor_body", K::ConstructorBody),
            ("switch_block", K::Scoped),
            ("switch_rule", K::Scoped),
            ("for_statement", K::Scoped),
            ("enhanced_for_statement", K::EnhancedFor),
            ("try_with_resources_statement", K::TryWithResources),
            ("resource", K::Resource),
            ("catch_clause", K::CatchClause),
            ("catch_formal_parameter", K::CatchFormalParameter),
            ("catch_type", K::CatchType),
            ("labeled_statement", K::LabeledStatement),
            ("switch_expression", K::SwitchExpression),
            ("switch_label", K::SwitchLabel),
            ("explicit_constructor_invocation", K::ExplicitConstructorInvocation),
            ("method_invocation", K::MethodInvocation),
            ("field_access", K::FieldAccess),
            ("object_creation_expression", K::ObjectCreation),
            ("array_creation_expression", K::ArrayCreation),
            ("method_reference", K::MethodReference),
            ("assignment_expression", K::Assignment),
            ("binary_expression", K::BinaryExpression),
            ("update_expression", K::Update),
            ("lambda_expression", K::Lambda),
            ("instanceof_expression", K::Instanceof),
            ("type_pattern", K::TypePattern),
            ("record_pattern_component", K::TypePattern),
            ("record_pattern", K::RecordPattern),
            ("cast_expression", K::CastExpression),
            ("parenthesized_expression", K::Parenthesized),
            ("array_access", K::ArrayAccess),
            ("argument_list", K::ArgumentList),
            ("type_arguments", K::TypeArguments),
            ("string_literal", K::StringLiteral),
            ("class_literal", K::ClassLiteral),
            ("asterisk", K::Asterisk),
            ("line_comment", K::Comment),
            ("block_comment", K::Comment),
            // leaves and subtrees that never contain references
            ("character_literal", K::Skip),
            ("decimal_integer_literal", K::Skip),
            ("hex_integer_literal", K::Skip),
            ("octal_integer_literal", K::Skip),
            ("binary_integer_literal", K::Skip),
            ("decimal_floating_point_literal", K::Skip),
            ("hex_floating_point_literal", K::Skip),
            ("true", K::Skip),
            ("false", K::Skip),
            ("null_literal", K::Skip),
            ("integral_type", K::Skip),
            ("floating_point_type", K::Skip),
            ("boolean_type", K::Skip),
            ("void_type", K::Skip),
            ("underscore_pattern", K::Skip),
            ("break_statement", K::Skip),
            ("continue_statement", K::Skip),
            ("module_body", K::Skip),
            ("requires_modifier", K::Skip),
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
            f_type_parameters: field(&lang, "type_parameters"),
            f_declarator: field(&lang, "declarator"),
            f_value: field(&lang, "value"),
            f_dimensions: field(&lang, "dimensions"),
            f_field: field(&lang, "field"),
            f_object: field(&lang, "object"),
            f_scope: field(&lang, "scope"),
            f_element: field(&lang, "element"),
            f_key: field(&lang, "key"),
            f_right: field(&lang, "right"),
            kw_public: lang.id_for_node_kind("public", false),
            kw_private: lang.id_for_node_kind("private", false),
            kw_abstract: lang.id_for_node_kind("abstract", false),
            kw_native: lang.id_for_node_kind("native", false),
            kw_static: lang.id_for_node_kind("static", false),
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Scope entries
// ---------------------------------------------------------------------------------------------
//
// The value stored per name in a scope packs what the name denotes:
//   bit 31      set for a member of an enclosing class declared in this file, clear for a local
//   bits 25-30  for members: index of the declaring class in `W::classes`
//   bits 23-24  number of array dimensions of the declared type (saturating at 3)
//   bits 0-22   name id of the declared type's simple name + 1, or 0 when unknown / primitive

const B_MEMBER: u32 = 1 << 31;
const B_LEVEL_SHIFT: u32 = 25;
const B_MAX_LEVEL: usize = 62;
const B_DIMS_SHIFT: u32 = 23;
const B_TY_MASK: u32 = (1 << B_DIMS_SHIFT) - 1;

#[inline]
fn enc_local(ty: u32, dims: u8) -> u32 {
    let t = if ty >= B_TY_MASK - 1 { 0 } else { ty + 1 };
    ((dims.min(3) as u32) << B_DIMS_SHIFT) | t
}

#[inline]
fn enc_member(level: usize, ty: u32, dims: u8) -> u32 {
    B_MEMBER | ((level.min(B_MAX_LEVEL) as u32) << B_LEVEL_SHIFT) | enc_local(ty, dims)
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

// ---------------------------------------------------------------------------------------------
// Walker state
// ---------------------------------------------------------------------------------------------

/// Members of the class are implicitly public (interface, annotation type).
const CF_IFACE: u8 = 1;
/// The class name marks a test class (`*Test`, `*Tests`, `*IT`).
const CF_TEST: u8 = 2;
/// Anonymous class body or enum constant body: no name of its own.
const CF_ANON: u8 = 4;

#[derive(Clone, Copy)]
struct Class {
    /// Qualifier for unqualified calls and members that are not declared in this file: the
    /// innermost enclosing *named* class.
    name: u32,
    /// The class's own simple name; `NONE` for anonymous bodies.
    own: u32,
    /// Static type of `this`: the class itself, or the supertype of an anonymous class.
    this_ty: u32,
    /// Simple name of the written superclass (`extends T`), for `super(...)`.
    sup: u32,
    /// `^name`, interned on first use.
    sup_q: u32,
    flags: u8,
    /// Range of this record's components in `W::comps`.
    comps: (u32, u32),
    /// For an anonymous body: range of the methods it declares in `W::anon_methods`.
    methods: (u32, u32),
}

const M_PUBLIC: u8 = 1;
const M_PRIVATE: u8 = 2;
const M_ABSTRACT: u8 = 4;
const M_NATIVE: u8 = 8;
const M_TEST: u8 = 16;
const M_STATIC: u8 = 32;

/// What the modifiers of a declaration say, and where its signature starts once leading
/// annotations are skipped.
#[derive(Clone, Copy)]
struct Mods {
    bits: u8,
    sig_start: usize,
}

/// What the declarators of one field declaration share.
struct FieldDecl<'t> {
    node: Node<'t>,
    parent: u32,
    flags: u8,
    ty: u32,
    sig_start: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MK {
    Method,
    Ctor,
    Compact,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RK {
    /// A value whose static type path is `base` (+ `::fld`), `NONE` when unknown.
    Val,
    /// `this`.
    This,
    /// A type name: `base` is its simple name.
    Type,
    /// A chain of lower-case names that is either a member path or a package prefix; the member
    /// reads emitted for it so far start at `mark` and are dropped if a type name follows.
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
    mark: u32,
}

impl Recv {
    const UNKNOWN: Recv = Recv { k: RK::Val, base: NONE, fld: NONE, call: false, dims: 0, mark: 0 };
    const SUPER: Recv = Recv { k: RK::Super, base: NONE, fld: NONE, call: false, dims: 0, mark: 0 };

    #[inline]
    fn val(ty: u32, dims: u8) -> Recv {
        Recv { k: RK::Val, base: ty, fld: NONE, call: false, dims, mark: 0 }
    }

    #[inline]
    fn ty(name: u32) -> Recv {
        Recv { k: RK::Type, base: name, fld: NONE, call: false, dims: 0, mark: 0 }
    }
}

struct W<'a, 't> {
    cx: Ctx<'a>,
    t: &'static Tables,
    /// Enclosing class bodies, outermost first.
    classes: Vec<Class>,
    /// Type parameters of the enclosing classes and methods with the simple name of their bound
    /// (`NONE` when unbounded). Those from `tp_base` on are in scope: a static member does not
    /// see the type parameters of the classes around it.
    tparams: Vec<(u32, u32)>,
    tp_base: usize,
    /// Components of the enclosing records: (name, encoded local type).
    comps: Vec<(u32, u32)>,
    /// Names of the methods the enclosing anonymous class bodies declare themselves.
    anon_methods: Vec<u32>,
    /// Type names known in this file that do not look like type names (`URL`, `xContent`).
    odd_types: Vec<u32>,
    /// Names bound by `import static a.b.C.name`: (name, type path of `C`, here `a.b|C`). The
    /// path is `NONE` when two static imports bind the name to different classes.
    static_imports: Vec<(u32, u32)>,
    /// Statically imported names that an enclosing class body declares as a method itself: the
    /// method of the class hides the import.
    shadowed: Vec<u32>,
    /// Type path of the selector of the innermost `switch`, for bare enum constants in labels.
    switch_q: u32,
    /// Type of a `case final T name` pattern whose binding `name` is still to come (see
    /// `type_pattern`).
    pending_final: u32,
    string_id: u32,
    class_id: u32,
    _t: PhantomData<&'t ()>,
}

pub fn extract(_lang: Lang, src: &[u8], tree: &Tree, path: &str, facts: &mut FileFacts, scratch: &mut Scratch) {
    let cx = Ctx::new(src, path, facts, scratch);
    let mut w = W {
        cx,
        t: tables(),
        classes: Vec::new(),
        tparams: Vec::new(),
        tp_base: 0,
        comps: Vec::new(),
        anon_methods: Vec::new(),
        odd_types: Vec::new(),
        static_imports: Vec::new(),
        shadowed: Vec::new(),
        switch_q: NONE,
        pending_final: NONE,
        string_id: NONE,
        class_id: NONE,
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

    fn visit(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        match self.k(n) {
            K::Token | K::Skip | K::Comment | K::This | K::Super | K::ScopedIdentifier | K::Asterisk | K::StringLiteral | K::InferredParameters => {}
            K::Identifier => self.identifier(n, false),
            K::TypeIdentifier => self.type_ident(n, RefKind::Type, NONE, 255),
            K::ScopedTypeIdentifier => self.scoped_type(c, RefKind::Type, 255),
            K::MethodInvocation => {
                self.method_invocation(n, c);
            }
            K::FieldAccess => {
                self.field_access(n, c, false, false);
            }
            K::ObjectCreation => {
                self.object_creation(n, c);
            }
            K::MethodReference => {
                self.method_reference(n, c);
            }
            K::Assignment => self.assignment(c),
            K::BinaryExpression => self.binary(c),
            K::Update => self.update(c),
            K::Lambda => self.lambda(c),
            K::Instanceof => self.instanceof(n, c),
            K::TypePattern => self.type_pattern(c),
            K::RecordPattern => self.record_pattern(c),
            K::LocalVariableDeclaration => self.local_variable(n, c),
            K::VariableDeclarator => self.local_declarator(n, c, NONE, 0, false),
            K::Block | K::ConstructorBody | K::Scoped => self.scoped(c),
            K::EnhancedFor => self.enhanced_for(n, c),
            K::TryWithResources => self.try_with_resources(c),
            K::Resource => self.resource(n, c),
            K::CatchClause => self.catch_clause(c),
            K::CatchFormalParameter => self.catch_param(n, c),
            K::LabeledStatement => self.labeled(c),
            K::SwitchExpression => self.switch_expression(c),
            K::SwitchLabel => self.switch_label(c),
            K::ExplicitConstructorInvocation => self.explicit_constructor_call(c),
            K::Annotation => self.annotation(n, c),
            K::ElementValuePair => self.element_pair(n, c, NONE),
            K::ClassDeclaration => self.type_decl(n, c, DefKind::Class, 0, false),
            K::InterfaceDeclaration => self.type_decl(n, c, DefKind::Interface, CF_IFACE, true),
            K::EnumDeclaration => self.type_decl(n, c, DefKind::Enum, 0, true),
            K::RecordDeclaration => self.type_decl(n, c, DefKind::Class, 0, true),
            K::AnnotationTypeDeclaration => self.type_decl(n, c, DefKind::Interface, CF_IFACE, true),
            K::EnumConstant => self.enum_constant(n, c),
            K::FieldDeclaration => self.field_declaration(n, c),
            K::MethodDeclaration => self.method(n, c, MK::Method),
            K::ConstructorDeclaration => self.method(n, c, MK::Ctor),
            K::CompactConstructor => self.method(n, c, MK::Compact),
            K::AnnotationElement => self.annotation_element(n, c),
            K::PackageDeclaration => self.package(n, c),
            K::ImportDeclaration => self.import(n),
            K::ModuleDeclaration => self.module(n),
            K::Program => self.program(c),
            K::TypeParameter => self.type_parameter(c),
            K::FormalParameters => {
                self.params(c);
            }
            K::FormalParameter => self.param(n, c, K::FormalParameter),
            K::SpreadParameter => self.param(n, c, K::SpreadParameter),
            K::ReceiverParameter => self.receiver_parameter(c),
            K::Error => self.error(c),
            _ => self.children(c),
        }
    }

    // ----- small helpers -----

    #[inline]
    fn class_name(&self) -> u32 {
        self.classes.last().map_or(NONE, |c| c.name)
    }

    /// Is `name` a method that an enclosing anonymous class body declares itself (looking no
    /// further out than the innermost named class)?
    fn anon_method(&self, name: u32) -> bool {
        for cl in self.classes.iter().rev() {
            if cl.flags & CF_ANON == 0 {
                break;
            }
            let (a, b) = (cl.methods.0 as usize, cl.methods.1 as usize);
            if self.anon_methods.get(a..b).is_some_and(|m| m.contains(&name)) {
                return true;
            }
        }
        false
    }

    /// Qualifier for a member declared by the enclosing class at `level` (`NONE` for members of
    /// an anonymous class, which has no name).
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

    fn class_id(&mut self) -> u32 {
        if self.class_id == NONE {
            self.class_id = self.cx.intern(b"Class");
        }
        self.class_id
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
            // Members of an array (`length`, `clone()`) are not members of its element type.
            _ if r.dims > 0 || r.base == NONE => NONE,
            _ if r.fld == NONE => r.base,
            _ => self.path(r.base, r.fld, r.call),
        }
    }

    /// `^Class`: a strict ancestor of the enclosing class. The super of an anonymous class is
    /// the type it was created from.
    fn super_qual(&mut self) -> u32 {
        let Some(cl) = self.classes.last().copied() else { return NONE };
        if cl.flags & CF_ANON != 0 {
            return cl.this_ty;
        }
        if cl.sup_q != NONE || cl.own == NONE {
            return cl.sup_q;
        }
        let q = self.joined(b"^", cl.own, b"", NONE);
        if let Some(cl) = self.classes.last_mut() {
            cl.sup_q = q;
        }
        q
    }

    /// Qualifier of `this.name`: the enclosing class, except for a member the anonymous class
    /// body declares itself, which no name can qualify.
    fn this_member_qual(&self, name: u32) -> u32 {
        let Some(cl) = self.classes.last() else { return NONE };
        if cl.flags & CF_ANON != 0
            && let Some(v) = self.cx.local_type(name)
            && v & B_MEMBER != 0
            && dec_level(v) == self.classes.len() - 1
        {
            return NONE;
        }
        cl.this_ty
    }

    /// The class a by-name static import binds `name` to, as a type path (`a.b|C`); `NONE` when
    /// no static import of the file names it.
    #[inline]
    fn static_owner_of(&self, name: u32) -> u32 {
        if self.static_imports.is_empty() {
            return NONE;
        }
        self.static_imports.iter().find(|x| x.0 == name).map_or(NONE, |x| x.1)
    }

    /// Type path of the class named by the scope of a static import: `a.b.Outer.Inner` is
    /// `a.b|Outer::Inner`. The package ends where the first capitalised segment starts; when
    /// no segment is capitalised the last one is the class.
    fn static_owner(&mut self, scope: Node<'t>) -> u32 {
        let dotted = self.dotted(scope);
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let text = self.cx.name_bytes(dotted);
        let mut cut = None;
        let mut off = 0;
        for seg in text.split(|&b| b == b'.') {
            if starts_upper(seg) {
                cut = Some(off);
                break;
            }
            off += seg.len() + 1;
        }
        let cut = cut.unwrap_or_else(|| text.iter().rposition(|&b| b == b'.').map_or(0, |i| i + 1));
        if cut > 0 {
            buf.extend_from_slice(&text[..cut - 1]);
            buf.push(b'|');
        }
        for (i, seg) in text[cut..].split(|&b| b == b'.').enumerate() {
            if i > 0 {
                buf.extend_from_slice(b"::");
            }
            buf.extend_from_slice(seg);
        }
        let ok = !buf.is_empty() && buf.len() <= 160 && buf.last() != Some(&b'|') && !buf.ends_with(b"::");
        let id = if ok { self.cx.intern(&buf) } else { NONE };
        self.cx.s.buf = buf;
        id
    }

    /// A dotted name (`a.b.c`) as written, without embedded whitespace or comments.
    fn dotted(&mut self, n: Node<'t>) -> u32 {
        let text = self.cx.text(n);
        if !text.iter().any(|&b| b <= b' ' || b == b'/') {
            return self.cx.intern(text);
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        self.push_dotted(n, &mut buf);
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// The name of a declaration or call, unless the parser had to invent it (a zero-width
    /// MISSING node).
    fn decl_name(&self, n: Node<'t>) -> Option<Node<'t>> {
        child(n, self.t.f_name).filter(|x| x.end_byte() > x.start_byte())
    }

    /// Type and first declarator of a field or local variable declaration, `[modifiers] type
    /// declarator, ..;`. Read by position: a lookup by field scans the children every time.
    fn decl_head(&self, n: Node<'t>) -> (Option<Node<'t>>, Option<Node<'t>>) {
        let at = if n.child(0).is_some_and(|x| self.k(x) == K::Modifiers) { 1 } else { 0 };
        match (n.child(at), n.child(at + 1)) {
            (Some(ty), Some(d)) if self.k(d) == K::VariableDeclarator && !matches!(self.k(ty), K::Token | K::Comment) => (Some(ty), Some(d)),
            _ => (child(n, self.t.f_type), child(n, self.t.f_declarator)),
        }
    }

    /// Name, initializer and extra array dimensions (`int a[]`) of a declarator. The usual
    /// shapes, `name` and `name = value`, are read by position.
    fn declarator_parts(&self, d: Node<'t>) -> (Option<Node<'t>>, Option<Node<'t>>, u8) {
        let t = self.t;
        match d.child_count() {
            1 => (d.child(0), None, 0),
            3 if d.child(1).is_some_and(|x| self.k(x) == K::Token) => (d.child(0), d.child(2), 0),
            _ => (child(d, t.f_name), child(d, t.f_value), child(d, t.f_dimensions).map_or(0, |x| self.count_dims(x))),
        }
    }

    /// End of a declaration without its trailing `;`.
    fn before_semicolon(&self, n: Node<'t>) -> usize {
        let e = n.end_byte();
        if e > n.start_byte() && self.cx.src.get(e - 1) == Some(&b';') { e - 1 } else { e }
    }

    /// Modifier keywords and annotations of a declaration.
    fn modifiers(&self, n: Node<'t>) -> Mods {
        let t = self.t;
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
                    let id = ch.kind_id();
                    if id == t.kw_public {
                        m.bits |= M_PUBLIC;
                    } else if id == t.kw_private {
                        m.bits |= M_PRIVATE;
                    } else if id == t.kw_abstract {
                        m.bits |= M_ABSTRACT;
                    } else if id == t.kw_native {
                        m.bits |= M_NATIVE;
                    } else if id == t.kw_static {
                        m.bits |= M_STATIC;
                    }
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
        let t = self.t;
        let Some(nm) = child(ann, t.f_name) else { return false };
        let simple = if self.k(nm) == K::ScopedIdentifier { child(nm, t.f_name) } else { Some(nm) };
        simple.is_some_and(|s| matches!(self.cx.text(s), b"Test" | b"ParameterizedTest" | b"RepeatedTest"))
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

    fn visibility(m: Mods, implicit_public: bool) -> u8 {
        if m.bits & M_PRIVATE != 0 {
            dflag::LOCAL
        } else if m.bits & M_PUBLIC != 0 || implicit_public {
            dflag::EXPORTED
        } else {
            0
        }
    }

    // ----- types -----

    /// Name and array dimensions of a declared type: generics stripped, a type parameter
    /// replaced by its bound. A qualified name keeps its qualifier as a type path: a package
    /// becomes a module qualifier (`java.util.List` is `java.util|List`), an outer type a root
    /// (`Outer.Inner` is `Outer::Inner`: nested types are members of the type around them).
    fn type_name(&mut self, mut ty: Node<'t>) -> (u32, u8) {
        let t = self.t;
        let mut dims = 0u8;
        for _ in 0..8 {
            match self.k(ty) {
                K::TypeIdentifier => {
                    let text = self.cx.text(ty);
                    if text == b"var" {
                        return (NONE, 0);
                    }
                    let id = self.cx.intern(text);
                    if let Some((_, bound)) = self.tparam(id) {
                        return (bound, dims);
                    }
                    return (id, dims);
                }
                K::GenericType => match ty.named_child(0) {
                    Some(x) => ty = x,
                    None => break,
                },
                K::ScopedTypeIdentifier => {
                    let mut buf = std::mem::take(&mut self.cx.s.buf);
                    buf.clear();
                    let ok = self.scoped_path(ty, &mut buf, 0);
                    let id = if ok && !buf.is_empty() { self.cx.intern(&buf) } else { NONE };
                    self.cx.s.buf = buf;
                    return (id, dims);
                }
                K::AnnotatedType => match ty.named_child((ty.named_child_count() as u32).wrapping_sub(1)) {
                    Some(x) => ty = x,
                    None => break,
                },
                K::ArrayType => {
                    dims = dims.saturating_add(child(ty, t.f_dimensions).map_or(1, |d| self.count_dims(d)));
                    match child(ty, t.f_element) {
                        Some(x) => ty = x,
                        None => break,
                    }
                }
                _ => break,
            }
        }
        (NONE, dims)
    }

    /// Write the type path of a qualified type name (`a.b.C`, `Outer.Inner`, `a.b.Outer.Inner`,
    /// `Outer<T>.Inner`) into `buf`. False when the name has a shape that cannot be told.
    fn scoped_path(&mut self, n: Node<'t>, buf: &mut Vec<u8>, depth: u32) -> bool {
        if depth > 6 {
            return false;
        }
        match self.k(n) {
            K::TypeIdentifier => {
                buf.extend_from_slice(self.cx.text(n));
                true
            }
            K::GenericType => match n.named_child(0) {
                Some(x) => self.scoped_path(x, buf, depth + 1),
                None => false,
            },
            K::AnnotatedType => match n.named_child((n.named_child_count() as u32).wrapping_sub(1)) {
                Some(x) => self.scoped_path(x, buf, depth + 1),
                None => false,
            },
            K::ScopedTypeIdentifier => {
                let count = n.named_child_count() as u32;
                let (Some(scope), Some(last)) = (n.named_child(0), n.named_child(count.wrapping_sub(1))) else { return false };
                if count < 2 || self.k(last) != K::TypeIdentifier || last.end_byte() == last.start_byte() {
                    return false;
                }
                if self.is_package(scope) {
                    self.push_dotted(scope, buf);
                    buf.push(b'|');
                } else {
                    if !self.scoped_path(scope, buf, depth + 1) {
                        return false;
                    }
                    buf.extend_from_slice(b"::");
                }
                buf.extend_from_slice(self.cx.text(last));
                true
            }
            _ => false,
        }
    }

    /// Is the scope of a qualified type name a package (`java.util`) rather than an outer type?
    /// Packages are written in lower case; a name known to be a type in this file is a type.
    fn is_package(&mut self, scope: Node<'t>) -> bool {
        match self.k(scope) {
            K::TypeIdentifier => {
                let text = self.cx.text(scope);
                !starts_upper(text) && {
                    let id = self.cx.intern(text);
                    !self.odd_types.contains(&id)
                }
            }
            K::ScopedTypeIdentifier => {
                let last = scope.named_child((scope.named_child_count() as u32).wrapping_sub(1));
                !last.is_some_and(|l| starts_upper(self.cx.text(l)))
            }
            _ => false,
        }
    }

    /// Append a dotted name as written, without embedded whitespace or comments.
    fn push_dotted(&self, n: Node<'t>, buf: &mut Vec<u8>) {
        let text = self.cx.text(n);
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

    fn count_dims(&self, d: Node<'t>) -> u8 {
        self.cx.text(d).iter().filter(|&&b| b == b'[').count().clamp(1, 3) as u8
    }

    #[inline]
    fn is_var(&self, ty: Node<'t>) -> bool {
        self.k(ty) == K::TypeIdentifier && self.cx.text(ty) == b"var"
    }

    /// Element type and dimensions of an array creation, `new T[n][]` or `new T[] { .. }`.
    fn infer(&mut self, v: Node<'t>) -> (u32, u8) {
        let t = self.t;
        let Some(ty_node) = child(v, t.f_type) else { return (NONE, 0) };
        let (ty, d) = self.type_name(ty_node);
        let from = ty_node.end_byte();
        let to = child(v, t.f_value).map_or(v.end_byte(), |x| x.start_byte()).max(from);
        // Brackets nested in a dimension expression (`new T[a[i]]`) are not dimensions.
        let mut depth = 0u32;
        let mut dims = 0u32;
        for &b in self.cx.src.get(from..to).unwrap_or(&[]).iter().take(256) {
            match b {
                b'[' => {
                    if depth == 0 {
                        dims += 1;
                    }
                    depth += 1;
                }
                b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        (ty, d.saturating_add(dims.clamp(1, 3) as u8))
    }

    /// Reference to a type by its simple name. Type parameters and the stop list emit nothing
    /// (a name written with a qualifier is never a type parameter).
    fn type_ident(&mut self, n: Node<'t>, kind: RefKind, qual: u32, nargs: u8) {
        let text = self.cx.text(n);
        if text.is_empty() || is_stop(text) {
            return;
        }
        let name = self.cx.intern(text);
        if qual == NONE && self.tparam(name).is_some() {
            return;
        }
        // A name used as a type is a type wherever it appears later in the file, also when
        // its spelling (`FST`, `xml`) would not say so.
        if !type_like(text) && !self.odd_types.contains(&name) {
            self.odd_types.push(name);
        }
        self.cx.add_ref(name, kind, line_of(n), qual, nargs);
    }

    /// Emit the reference for the head of a type with `kind` (`Extends`, `New`, ...); type
    /// arguments, annotations and outer types are plain `Type` references.
    fn type_head(&mut self, c: &mut TreeCursor<'t>, kind: RefKind, nargs: u8) {
        let n = c.node();
        match self.k(n) {
            K::TypeIdentifier => self.type_ident(n, kind, NONE, nargs),
            K::ScopedTypeIdentifier => self.scoped_type(c, kind, nargs),
            K::GenericType | K::AnnotatedType => self.each(c, |w, c| match w.k(c.node()) {
                K::TypeIdentifier | K::ScopedTypeIdentifier | K::GenericType | K::AnnotatedType => w.type_head(c, kind, nargs),
                _ => w.visit(c),
            }),
            _ => self.visit(c),
        }
    }

    /// `Outer.Inner` / `a.b.C`: the last segment is the type, qualified by the outer type or
    /// the package it is written with.
    fn scoped_type(&mut self, c: &mut TreeCursor<'t>, kind: RefKind, nargs: u8) {
        let mut q = NONE;
        let mut first = true;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if first => {
                    first = false;
                    q = w.type_scope(c);
                }
                K::TypeIdentifier => w.type_ident(ch, kind, q, nargs),
                _ => w.visit(c),
            }
        });
    }

    /// The scope part of a qualified type: an outer type (referenced, returns its name) or a
    /// package prefix (no reference, returns the dotted name).
    fn type_scope(&mut self, c: &mut TreeCursor<'t>) -> u32 {
        let n = c.node();
        match self.k(n) {
            K::TypeIdentifier => {
                let text = self.cx.text(n);
                let id = self.cx.intern(text);
                if starts_upper(text) || self.odd_types.contains(&id) {
                    self.type_ident(n, RefKind::Type, NONE, 255);
                }
                id
            }
            K::ScopedTypeIdentifier => {
                let last = n.named_child((n.named_child_count() as u32).wrapping_sub(1));
                match last {
                    Some(l) if starts_upper(self.cx.text(l)) => {
                        self.scoped_type(c, RefKind::Type, 255);
                        self.cx.name(l)
                    }
                    _ => self.dotted(n),
                }
            }
            K::GenericType => {
                let (id, _) = self.type_name(n);
                self.visit(c);
                self.simple_of(id)
            }
            _ => {
                self.visit(c);
                NONE
            }
        }
    }

    /// The type parameter `name` in scope, innermost first.
    #[inline]
    fn tparam(&self, name: u32) -> Option<(u32, u32)> {
        self.tparams.get(self.tp_base..)?.iter().rev().find(|p| p.0 == name).copied()
    }

    /// Declare the type parameters of a class or method. A parameter stands for its first
    /// bound, so that `<T extends Writeable> ... t.writeTo(out)` is a call on `Writeable`.
    fn declare_tparams(&mut self, tps: Node<'t>) {
        for i in 0..tps.named_child_count() as u32 {
            let Some(p) = tps.named_child(i) else { break };
            if self.k(p) != K::TypeParameter {
                continue;
            }
            let mut name = NONE;
            let mut bound = NONE;
            for j in 0..p.named_child_count() as u32 {
                let Some(x) = p.named_child(j) else { break };
                match self.k(x) {
                    K::TypeIdentifier if name == NONE => name = self.cx.name(x),
                    K::TypeBound => {
                        if let Some(b) = x.named_child(0) {
                            bound = self.type_name(b).0;
                        }
                    }
                    _ => {}
                }
            }
            if name != NONE {
                self.tparams.push((name, bound));
            }
        }
    }

    /// `T extends A & B` in a type parameter list: the bounds are references, `T` is not.
    fn type_parameter(&mut self, c: &mut TreeCursor<'t>) {
        let mut named = false;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::TypeIdentifier if !named => named = true,
                _ => w.visit(c),
            }
        });
    }

    // ----- file level -----

    fn program(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| w.note_odd_type(c.node()));
        self.children(c);
    }

    /// Remember a type declared in this file whose name would not be recognised as a type by
    /// its spelling (`URL`, `ES`), so that `URL.of()` is a static call and not a member call.
    fn note_odd_type(&mut self, n: Node<'t>) {
        if !matches!(self.k(n), K::ClassDeclaration | K::InterfaceDeclaration | K::EnumDeclaration | K::RecordDeclaration | K::AnnotationTypeDeclaration) {
            return;
        }
        if let Some(nm) = child(n, self.t.f_name)
            && !type_like(self.cx.text(nm))
        {
            let id = self.cx.name(nm);
            if !self.odd_types.contains(&id) {
                self.odd_types.push(id);
            }
        }
    }

    fn package(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let mut nm = None;
        let mut sig_start = n.start_byte();
        let mut seen_kw = false;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Annotation => w.visit(c),
                K::Identifier | K::ScopedIdentifier => nm = Some(ch),
                K::Token if !seen_kw => {
                    seen_kw = true;
                    sig_start = ch.start_byte();
                }
                _ => {}
            }
        });
        let Some(nm) = nm.filter(|x| x.end_byte() > x.start_byte()) else { return };
        let name = self.dotted(nm);
        let sig_end = self.before_semicolon(n);
        let d = self.cx.def(name, DefKind::Namespace, n, sig_end);
        self.finish_def(d, NONE, 0, sig_start, sig_end);
    }

    fn module(&mut self, n: Node<'t>) {
        let Some(nm) = self.decl_name(n) else { return };
        let name = self.dotted(nm);
        let sig_end = child(n, self.t.f_body).map_or(n.end_byte(), |b| b.start_byte());
        self.cx.def(name, DefKind::Module, n, sig_end);
    }

    fn import(&mut self, n: Node<'t>) {
        let t = self.t;
        let mut is_static = false;
        let mut wildcard = false;
        let mut nm = None;
        for i in 0..n.child_count() {
            let Some(ch) = n.child(i) else { break };
            match self.k(ch) {
                K::Identifier | K::ScopedIdentifier => nm = Some(ch),
                K::Asterisk => wildcard = true,
                K::Token if ch.kind_id() == t.kw_static => is_static = true,
                _ => {}
            }
        }
        let Some(nm) = nm else { return };
        // `import module java.base;` (Java 25) is not in the grammar: the pieces it leaves
        // behind (`module`, `base`) name no class or package.
        if n.has_error()
            && self.cx.text(nm).starts_with(b"module")
            && !self.cx.text(nm).get(6).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
        {
            self.cx.facts.parse_errors += 1;
            return;
        }
        let line = line_of(n);
        let text = self.cx.text(nm);
        let sys = if text.starts_with(b"java.") || text.starts_with(b"javax.") || text.starts_with(b"jdk.") { iflag::SYSTEM } else { 0 };
        let (scope, simple) = if self.k(nm) == K::ScopedIdentifier { (child(nm, t.f_scope), child(nm, t.f_name)) } else { (None, Some(nm)) };
        if wildcard {
            let target = self.dotted(nm);
            self.cx.add_import(target, NONE, NONE, line, iflag::WILDCARD | sys);
            // `import static a.b.C.*` uses the class `C` of package `a.b`.
            if let (true, Some(s), Some(x)) = (is_static, scope, simple.filter(|x| x.end_byte() > x.start_byte())) {
                let q = self.dotted(s);
                let name = self.cx.name(x);
                self.cx.add_ref(name, RefKind::Import, line_of(x), q, 255);
            }
            return;
        }
        let Some(simple) = simple.filter(|x| x.end_byte() > x.start_byte()) else { return };
        let name = self.cx.name(simple);
        let target = match scope {
            Some(s) => self.dotted(s),
            None => self.cx.intern(b""),
        };
        self.cx.add_import(target, name, NONE, line, sys);
        self.cx.add_ref(name, RefKind::Import, line_of(simple), if scope.is_some() { target } else { NONE }, 255);
        if let (true, Some(s)) = (is_static, scope) {
            // The name now stands for a static member of that class: `m()` is `C.m()`.
            let owner = self.static_owner(s);
            match self.static_imports.iter_mut().find(|x| x.0 == name) {
                Some(x) if x.1 != owner => x.1 = NONE,
                Some(_) => {}
                None => self.static_imports.push((name, owner)),
            }
        }
        if !is_static && !type_like(self.cx.text(simple)) && !self.odd_types.contains(&name) {
            self.odd_types.push(name);
        }
    }

    // ----- type declarations -----

    /// A class, interface, enum, record or annotation type. `is_static`: the kind of type is
    /// static wherever it is declared (everything but a plain class).
    fn type_decl(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: DefKind, cf: u8, is_static: bool) {
        let t = self.t;
        let Some(nm) = self.decl_name(n) else { return self.children(c) };
        let m = self.modifiers(n);
        let sig_end = child(n, t.f_body).map_or(n.end_byte(), |b| b.start_byte());
        let text = self.cx.text(nm);
        let name = self.cx.intern(text);
        let parent = self.cx.encl();
        let in_iface = self.classes.last().is_some_and(|cl| cl.flags & CF_IFACE != 0);
        let mut flags = Self::visibility(m, in_iface);
        if m.bits & M_ABSTRACT != 0 {
            flags |= dflag::ABSTRACT;
        }
        let d = self.cx.def(name, kind, n, sig_end);
        self.finish_def(d, parent, flags, m.sig_start, sig_end);
        self.cx.push_def(d);
        let (tp_mark, tp_base) = (self.tparams.len(), self.tp_base);
        if is_static || in_iface || m.bits & M_STATIC != 0 {
            self.tp_base = tp_mark;
        }
        if let Some(tp) = child(n, t.f_type_parameters) {
            self.declare_tparams(tp);
        }
        let comps_mark = self.comps.len() as u32;
        let test = text.ends_with(b"Tests") || text.ends_with(b"Test") || text.ends_with(b"IT");
        let mut cls = Class {
            name,
            own: name,
            this_ty: name,
            sup: NONE,
            sup_q: NONE,
            flags: cf | if test { CF_TEST } else { 0 },
            comps: (comps_mark, comps_mark),
            methods: (0, 0),
        };
        let nm_at = nm.start_byte();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::Superclass => cls.sup = w.heritage(c, RefKind::Extends),
                K::SuperInterfaces => {
                    w.heritage(c, RefKind::Implements);
                }
                K::ExtendsInterfaces => {
                    w.heritage(c, RefKind::Extends);
                }
                K::FormalParameters => w.record_components(c),
                K::ClassBody => {
                    cls.comps.1 = w.comps.len() as u32;
                    w.class_body(c, cls);
                }
                _ if ch.start_byte() == nm_at => {}
                _ => w.visit(c),
            }
        });
        self.comps.truncate(comps_mark as usize);
        self.tparams.truncate(tp_mark);
        self.tp_base = tp_base;
        self.cx.pop_def();
    }

    /// `extends T` / `implements A, B`: one reference of `kind` per listed type. Returns the
    /// simple name of the first one.
    fn heritage(&mut self, c: &mut TreeCursor<'t>, kind: RefKind) -> u32 {
        let mut first = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::TypeList => w.each(c, |w, c| {
                    if !matches!(w.k(c.node()), K::Token | K::Comment) {
                        w.type_head(c, kind, 255);
                    }
                }),
                _ => {
                    if first == NONE {
                        first = w.type_name(ch).0;
                    }
                    w.type_head(c, kind, 255);
                }
            }
        });
        first
    }

    /// Enter a class body: its members become visible, then the body is walked.
    fn class_body(&mut self, c: &mut TreeCursor<'t>, cls: Class) {
        let level = self.classes.len();
        self.classes.push(cls);
        self.cx.push_scope();
        for i in cls.comps.0..cls.comps.1 {
            if let Some(&(name, ty)) = self.comps.get(i as usize) {
                self.cx.declare(name, enc_member(level, dec_ty(ty), dec_dims(ty)));
            }
        }
        let methods_mark = self.anon_methods.len();
        let shadowed_mark = self.shadowed.len();
        self.declare_members(c, level, cls.own);
        if let Some(cl) = self.classes.last_mut() {
            cl.methods = (methods_mark as u32, self.anon_methods.len() as u32);
        }
        self.children(c);
        self.anon_methods.truncate(methods_mark);
        self.shadowed.truncate(shadowed_mark);
        self.cx.pop_scope();
        self.classes.pop();
    }

    /// Fields and enum constants may be used before their declaration: declare them all when
    /// the body is entered.
    fn declare_members(&mut self, c: &mut TreeCursor<'t>, level: usize, own: u32) {
        let t = self.t;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::FieldDeclaration => {
                    let (ty, dims) = w.decl_head(ch).0.map_or((NONE, 0), |x| w.type_name(x));
                    w.each(c, |w, c| {
                        let d = c.node();
                        if w.k(d) != K::VariableDeclarator {
                            return;
                        }
                        if let (Some(nm), _, extra) = w.declarator_parts(d) {
                            let name = w.cx.name(nm);
                            w.cx.declare(name, enc_member(level, ty, dims.saturating_add(extra)));
                        }
                    });
                }
                K::EnumConstant => {
                    if let Some(nm) = child(ch, t.f_name) {
                        let name = w.cx.name(nm);
                        w.cx.declare(name, enc_member(level, own, 0));
                    }
                }
                K::EnumBodyDeclarations => w.declare_members(c, level, own),
                // A method of an anonymous class has no class name a call could be qualified
                // with: remember it, so that calls of it from inside the body stay unqualified.
                // A method named like a static import hides that import inside the body.
                K::MethodDeclaration if own == NONE || !w.static_imports.is_empty() => {
                    if let Some(nm) = w.decl_name(ch) {
                        let name = w.cx.name(nm);
                        if own == NONE {
                            w.anon_methods.push(name);
                        }
                        if w.static_owner_of(name) != NONE {
                            w.shadowed.push(name);
                        }
                    }
                }
                _ => w.note_odd_type(ch),
            }
        });
    }

    /// Record header: every component is a field of the record.
    fn record_components(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            let ch = c.node();
            let k = w.k(ch);
            if k != K::FormalParameter && k != K::SpreadParameter {
                return;
            }
            let (nm, ty_node, extra) = w.param_parts(ch, k);
            let Some(nm) = nm.filter(|x| w.k(*x) == K::Identifier && x.end_byte() > x.start_byte()) else { return w.children(c) };
            let (ty, dims) = ty_node.map_or((NONE, 0), |x| w.type_name(x));
            let dims = dims.saturating_add(extra);
            let name = w.cx.name(nm);
            let m = w.modifiers(ch);
            let parent = w.cx.encl();
            let d = w.cx.def(name, DefKind::Field, ch, ch.end_byte());
            w.finish_def(d, parent, dflag::EXPORTED, m.sig_start, ch.end_byte());
            w.cx.def_mut(d).type_name = ty;
            w.comps.push((name, enc_local(ty, dims)));
            w.cx.push_def(d);
            w.param_children(c, nm.start_byte());
            w.cx.pop_def();
        });
    }

    fn enum_constant(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(nm) = self.decl_name(n) else { return self.children(c) };
        let m = self.modifiers(n);
        let sig_end = child(n, t.f_body).map_or(n.end_byte(), |b| b.start_byte());
        let name = self.cx.name(nm);
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
                K::ArgumentList => {
                    // `RED(1, "r")` runs a constructor of the enum.
                    let nargs = w.arguments(c, own);
                    if own != NONE {
                        w.cx.add_ref(own, RefKind::New, line, NONE, nargs);
                    }
                }
                // A constant with a body is an anonymous subclass of the enum.
                K::ClassBody => {
                    w.class_body(c, Class { name: own, own: NONE, this_ty: own, sup: own, sup_q: NONE, flags: CF_ANON, comps: (0, 0), methods: (0, 0) })
                }
                _ if ch.start_byte() == nm_at => {}
                _ => w.visit(c),
            }
        });
        self.cx.pop_def();
    }

    // ----- members -----

    fn field_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let (ty_node, first) = self.decl_head(n);
        let Some(first) = first else { return self.children(c) };
        let m = self.modifiers(n);
        let cls = self.classes.last().copied();
        let owner = cls.map_or(NONE, |cl| cl.own);
        let f = FieldDecl {
            node: n,
            parent: self.cx.encl(),
            flags: Self::visibility(m, cls.is_some_and(|cl| cl.flags & CF_IFACE != 0)),
            ty: ty_node.map_or(NONE, |x| self.type_name(x).0),
            sig_start: m.sig_start,
        };
        // The first field exists before the walk: the modifiers and the type belong to it.
        let (nm, val, _) = self.declarator_parts(first);
        let mut cur = self.field_def(&f, first, nm, val);
        self.cx.push_def(cur);
        let mut seen = 0;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::VariableDeclarator => {
                    let (nm, val, _) = w.declarator_parts(ch);
                    if seen > 0 {
                        w.cx.pop_def();
                        cur = w.field_def(&f, ch, nm, val);
                        w.cx.push_def(cur);
                    }
                    seen += 1;
                    if val.is_some() {
                        w.field_initializer(c, nm, owner);
                    }
                }
                _ => w.visit(c),
            }
        });
        self.cx.pop_def();
    }

    /// One field per declarator. The definition spans the whole declaration; the signature
    /// stops before the initializer.
    fn field_def(&mut self, f: &FieldDecl<'t>, decl: Node<'t>, nm: Option<Node<'t>>, val: Option<Node<'t>>) -> u32 {
        let Some(nm) = nm.filter(|x| self.k(*x) == K::Identifier && x.end_byte() > x.start_byte()) else { return NONE };
        let sig_end = val.map_or(decl.end_byte(), |v| v.start_byte());
        let name = self.cx.name(nm);
        let d = self.cx.def(name, DefKind::Field, f.node, sig_end);
        let callable = val.is_some_and(|v| matches!(self.k(v), K::Lambda | K::MethodReference));
        self.finish_def(d, f.parent, if callable { f.flags | dflag::FNPTR } else { f.flags }, f.sig_start, sig_end);
        self.cx.def_mut(d).type_name = f.ty;
        d
    }

    /// Walk the initializer of the field declarator under the cursor. A method reference stored
    /// directly into the field is also a binding into the slot `Class.field`.
    fn field_initializer(&mut self, c: &mut TreeCursor<'t>, nm: Option<Node<'t>>, owner: u32) {
        let nm_at = nm.map_or(usize::MAX, |x| x.start_byte());
        // Pattern variables of an `instanceof` in the initializer need somewhere to live.
        self.cx.push_scope();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::MethodReference => {
                    let (method, line) = w.method_reference(ch, c);
                    if let (Some(nm), true) = (nm, method != NONE) {
                        let f = w.cx.name(nm);
                        let slot = w.joined(b"", owner, b".", f);
                        w.cx.add_ref(method, RefKind::Bind, line, slot, 255);
                    }
                }
                _ if ch.start_byte() == nm_at => {}
                _ => w.visit(c),
            }
        });
        self.cx.pop_scope();
    }

    fn method(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, mk: MK) {
        let t = self.t;
        let Some(nm) = self.decl_name(n) else { return self.children(c) };
        let m = self.modifiers(n);
        let body = child(n, t.f_body);
        let sig_end = match body {
            Some(b) => b.start_byte(),
            None => self.before_semicolon(n),
        };
        let text = self.cx.text(nm);
        let name = self.cx.intern(text);
        let parent = self.cx.encl();
        let cls = self.classes.last().copied();
        let iface = cls.is_some_and(|cl| cl.flags & CF_IFACE != 0);
        let mut flags = Self::visibility(m, iface);
        if body.is_none() {
            flags |= dflag::DECL;
            if m.bits & M_ABSTRACT != 0 || (iface && m.bits & M_NATIVE == 0) {
                flags |= dflag::ABSTRACT;
            }
        }
        if mk == MK::Method && (m.bits & M_TEST != 0 || (text.starts_with(b"test") && cls.is_some_and(|cl| cl.flags & CF_TEST != 0))) {
            flags |= dflag::TEST;
        }
        let d = self.cx.def(name, DefKind::Method, n, sig_end);
        self.finish_def(d, parent, flags, m.sig_start, sig_end);
        self.cx.push_def(d);
        self.cx.push_scope();
        let (tp_mark, tp_base) = (self.tparams.len(), self.tp_base);
        if m.bits & M_STATIC != 0 {
            self.tp_base = tp_mark;
        }
        if let Some(tp) = child(n, t.f_type_parameters) {
            self.declare_tparams(tp);
        }
        if mk == MK::Method {
            let ty = child(n, t.f_type).map_or(NONE, |x| self.type_name(x).0);
            self.cx.def_mut(d).type_name = ty;
        }
        if mk == MK::Compact {
            // The compact constructor of a record takes the components as its parameters.
            if let Some(cl) = cls {
                let (a, b) = cl.comps;
                for i in a..b {
                    if let Some(&(cname, cty)) = self.comps.get(i as usize) {
                        self.cx.declare(cname, cty);
                    }
                }
                let arity = b.saturating_sub(a).min(254) as u8;
                let dm = self.cx.def_mut(d);
                dm.arity_min = arity;
                dm.arity_max = arity;
            }
        }
        let nm_at = nm.start_byte();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::FormalParameters => {
                    let (count, variadic) = w.params(c);
                    let dm = w.cx.def_mut(d);
                    dm.arity_min = if variadic { count.saturating_sub(1) } else { count };
                    dm.arity_max = if variadic { 255 } else { count };
                }
                // The body shares the scope of the parameters.
                K::Block | K::ConstructorBody => w.children(c),
                _ if ch.start_byte() == nm_at => {}
                _ => w.visit(c),
            }
        });
        self.tparams.truncate(tp_mark);
        self.tp_base = tp_base;
        self.cx.pop_scope();
        self.cx.pop_def();
    }

    /// `String value() default "";` in an annotation type: an accessor-like member that is set
    /// by name (`@Ann(value = ..)`) and read by a call (`ann.value()`).
    fn annotation_element(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(nm) = self.decl_name(n) else { return self.children(c) };
        let m = self.modifiers(n);
        let sig_end = self.before_semicolon(n);
        let name = self.cx.name(nm);
        let parent = self.cx.encl();
        let ty = child(n, t.f_type).map_or(NONE, |x| self.type_name(x).0);
        let d = self.cx.def(name, DefKind::Property, n, sig_end);
        self.finish_def(d, parent, dflag::EXPORTED, m.sig_start, sig_end);
        self.cx.def_mut(d).type_name = ty;
        self.cx.push_def(d);
        self.children_except(c, nm.start_byte());
        self.cx.pop_def();
    }

    // ----- parameters and locals -----

    /// Declare the parameters under the cursor (a `formal_parameters` node) in the current
    /// scope. Returns their number and whether the last one is variadic.
    fn params(&mut self, c: &mut TreeCursor<'t>) -> (u8, bool) {
        let mut count = 0u32;
        let mut variadic = false;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::FormalParameter => {
                    count += 1;
                    w.param(ch, c, K::FormalParameter);
                }
                K::SpreadParameter => {
                    count += 1;
                    variadic = true;
                    w.param(ch, c, K::SpreadParameter);
                }
                K::ReceiverParameter => w.receiver_parameter(c),
                _ => {}
            }
        });
        (count.min(254) as u8, variadic)
    }

    /// Name, type and extra array dimensions of a `formal_parameter` or `spread_parameter`.
    fn param_parts(&self, p: Node<'t>, k: K) -> (Option<Node<'t>>, Option<Node<'t>>, u8) {
        let t = self.t;
        if k == K::FormalParameter {
            // `[modifiers] type name` is read by position.
            let at = if p.child(0).is_some_and(|x| self.k(x) == K::Modifiers) { 1 } else { 0 };
            if p.child_count() == at + 2 {
                return (p.child(at + 1), p.child(at), 0);
            }
            return (child(p, t.f_name), child(p, t.f_type), child(p, t.f_dimensions).map_or(0, |d| self.count_dims(d)));
        }
        // `T... name`: the type has no field, the name sits in a variable_declarator.
        let mut nm = None;
        let mut ty = None;
        for i in 0..p.child_count() {
            let Some(ch) = p.child(i) else { break };
            match self.k(ch) {
                K::Token | K::Comment | K::Modifiers | K::Annotation => {}
                K::VariableDeclarator => nm = self.declarator_parts(ch).0,
                _ if ty.is_none() => ty = Some(ch),
                _ => {}
            }
        }
        (nm, ty, 1)
    }

    fn param(&mut self, p: Node<'t>, c: &mut TreeCursor<'t>, k: K) {
        let (nm, ty_node, extra) = self.param_parts(p, k);
        if let Some(nm) = nm.filter(|x| self.k(*x) == K::Identifier) {
            let (ty, dims) = ty_node.map_or((NONE, 0), |x| self.type_name(x));
            let name = self.cx.name(nm);
            self.cx.declare(name, enc_local(ty, dims.saturating_add(extra)));
        }
        self.param_children(c, nm.map_or(usize::MAX, |x| x.start_byte()));
    }

    /// Visit the modifiers and the type of a parameter, not its name.
    fn param_children(&mut self, c: &mut TreeCursor<'t>, nm_at: usize) {
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment | K::VariableDeclarator => {}
                _ if ch.start_byte() == nm_at => {}
                _ => w.visit(c),
            }
        });
    }

    /// `Outer this`: only the type is a reference.
    fn receiver_parameter(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            if w.k(c.node()) != K::Identifier {
                w.visit(c);
            }
        });
    }

    fn local_variable(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        if self.classes.is_empty() && !self.cx.in_scope() && self.cx.s.def_stack.is_empty() {
            // A variable declared outside any class or method: a field of the implicit class of
            // a compact source file.
            return self.field_declaration(n, c);
        }
        let ty_node = self.decl_head(n).0;
        let var = ty_node.is_some_and(|x| self.is_var(x));
        let (ty, dims) = ty_node.map_or((NONE, 0), |x| self.type_name(x));
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::VariableDeclarator => w.local_declarator(ch, c, ty, dims, var),
                _ => w.visit(c),
            }
        });
    }

    fn local_declarator(&mut self, d: Node<'t>, c: &mut TreeCursor<'t>, ty: u32, dims: u8, var: bool) {
        let (nm, val, extra) = self.declarator_parts(d);
        let nm = nm.filter(|x| self.k(*x) == K::Identifier);
        let nm_at = nm.map_or(usize::MAX, |x| x.start_byte());
        if let (true, Some(v), Some(nm)) = (var, val, nm) {
            // `var x = init`: the type is whatever the initializer denotes. The name is declared
            // after the initializer is walked (it is not in scope there).
            let (ty, dims) = self.initializer(c, v, nm_at);
            let name = self.cx.name(nm);
            self.cx.declare(name, enc_local(ty, dims));
            return;
        }
        if let Some(nm) = nm {
            let (ty, dims) = if var { (NONE, 0) } else { (ty, dims.saturating_add(extra)) };
            let name = self.cx.name(nm);
            self.cx.declare(name, enc_local(ty, dims));
        }
        if val.is_some() {
            self.children_except(c, nm_at);
        }
    }

    /// Walk the children of a declarator or resource (the cursor's node) whose initializer is
    /// `v`, and return the type path the initializer denotes: a constructed or cast type, the
    /// declared result of a call (`Repo::find()`), the type of a member (`Class::field`).
    fn initializer(&mut self, c: &mut TreeCursor<'t>, v: Node<'t>, nm_at: usize) -> (u32, u8) {
        let v_at = v.start_byte();
        let mut out = (NONE, 0u8);
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if ch.start_byte() == nm_at => {}
                K::ArrayCreation if ch.start_byte() == v_at => {
                    out = w.infer(ch);
                    w.visit(c);
                }
                _ if ch.start_byte() == v_at => {
                    let r = w.recv(c);
                    out = match r.k {
                        RK::Val | RK::This | RK::Pkg => (w.qual(r), r.dims),
                        _ => (NONE, 0),
                    };
                    // `qual` hides the element type of an array: keep it with its dimensions.
                    if r.dims > 0 && matches!(r.k, RK::Val | RK::Pkg) {
                        out.0 = if r.fld == NONE { r.base } else { w.path(r.base, r.fld, r.call) };
                    }
                }
                _ => w.visit(c),
            }
        });
        if out.0 == NONE { (NONE, 0) } else { out }
    }

    fn enhanced_for(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        self.cx.push_scope();
        let nm = child(n, t.f_name).filter(|x| self.k(*x) == K::Identifier);
        let nm_at = nm.map_or(usize::MAX, |x| x.start_byte());
        let val_at = child(n, t.f_value).map_or(usize::MAX, |x| x.start_byte());
        let (ty, mut dims) = child(n, t.f_type).map_or((NONE, 0), |x| self.type_name(x));
        if let Some(d) = child(n, t.f_dimensions) {
            dims = dims.saturating_add(self.count_dims(d));
        }
        let mut pending = nm;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if ch.start_byte() == nm_at => {}
                _ => {
                    let at = ch.start_byte();
                    w.visit(c);
                    // The variable is in scope in the body only, not in the iterated expression.
                    if at == val_at
                        && let Some(nm) = pending.take()
                    {
                        let name = w.cx.name(nm);
                        w.cx.declare(name, enc_local(ty, dims));
                    }
                }
            }
        });
        self.cx.pop_scope();
    }

    fn try_with_resources(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        let mut open = true;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                // The resources are visible in the body, not in `catch` / `finally`.
                K::Block if open => {
                    w.children(c);
                    w.cx.pop_scope();
                    open = false;
                }
                _ => w.visit(c),
            }
        });
        if open {
            self.cx.pop_scope();
        }
    }

    fn resource(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(nm) = child(n, t.f_name) else { return self.children(c) };
        if self.k(nm) == K::Identifier {
            let ty_node = child(n, t.f_type);
            if let (Some(x), Some(v)) = (ty_node, child(n, t.f_value))
                && self.is_var(x)
            {
                // `try (var out = open())`: typed by its initializer. The `var` itself is no type.
                let (ty, dims) = self.initializer(c, v, nm.start_byte());
                let name = self.cx.name(nm);
                self.cx.declare(name, enc_local(ty, dims));
                return;
            }
            let (ty, dims) = ty_node.map_or((NONE, 0), |x| self.type_name(x));
            let name = self.cx.name(nm);
            self.cx.declare(name, enc_local(ty, dims));
        }
        self.children_except(c, nm.start_byte());
    }

    fn catch_clause(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::Block => w.children(c),
                _ => w.visit(c),
            }
        });
        self.cx.pop_scope();
    }

    fn catch_param(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let nm = child(n, self.t.f_name);
        let mut ty = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::CatchType => {
                    // `A | B e`: no single static type.
                    if ch.named_child_count() == 1 {
                        ty = ch.named_child(0).map_or(NONE, |x| w.type_name(x).0);
                    }
                    w.children(c);
                }
                K::Modifiers => w.children(c),
                _ => {}
            }
        });
        if let Some(nm) = nm.filter(|x| self.k(*x) == K::Identifier) {
            let name = self.cx.name(nm);
            self.cx.declare(name, enc_local(ty, 0));
        }
    }

    fn lambda(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        let mut first = true;
        self.each(c, |w, c| {
            let ch = c.node();
            let k = w.k(ch);
            if k == K::Token || k == K::Comment {
                return;
            }
            if first {
                first = false;
                match k {
                    K::Identifier => {
                        let name = w.cx.name(ch);
                        w.cx.declare(name, enc_local(NONE, 0));
                    }
                    K::InferredParameters => w.each(c, |w, c| {
                        if w.k(c.node()) == K::Identifier {
                            let name = w.cx.name(c.node());
                            w.cx.declare(name, enc_local(NONE, 0));
                        }
                    }),
                    K::FormalParameters => {
                        w.params(c);
                    }
                    _ => w.visit(c),
                }
            } else if k == K::Block {
                w.children(c);
            } else {
                w.visit(c);
            }
        });
        self.cx.pop_scope();
    }

    /// `x instanceof T t`: the binding stays visible until the enclosing scope ends, which
    /// covers both `if (x instanceof T t) { t.. }` and `if (!(x instanceof T t)) return; t..`.
    fn instanceof(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let nm = child(n, t.f_name).filter(|x| self.k(*x) == K::Identifier);
        self.children_except(c, nm.map_or(usize::MAX, |x| x.start_byte()));
        if let Some(nm) = nm {
            let (ty, dims) = child(n, t.f_right).map_or((NONE, 0), |x| self.type_name(x));
            let name = self.cx.name(nm);
            self.cx.declare(name, enc_local(ty, dims));
        }
    }

    /// `T name` in a type pattern or a record pattern component.
    fn type_pattern(&mut self, c: &mut TreeCursor<'t>) {
        let mut ty = (NONE, 0);
        let mut after_final = false;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                // `case final T name` is read as the type `final`, the binding `T` and a stray
                // `name`: `T` is the type, and the name that follows in an ERROR node the binding.
                K::TypeIdentifier if w.cx.text(ch) == b"final" => after_final = true,
                K::Identifier if after_final => {
                    w.type_ident(ch, RefKind::Type, NONE, 255);
                    w.pending_final = w.cx.name(ch);
                }
                K::Identifier => {
                    let name = w.cx.name(ch);
                    w.cx.declare(name, enc_local(ty.0, ty.1));
                }
                K::RecordPattern => w.record_pattern(c),
                _ => {
                    ty = w.type_name(ch);
                    w.visit(c);
                }
            }
        });
    }

    /// `Point(int x, var y)`: the record type is written as a plain identifier.
    fn record_pattern(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Identifier => w.type_ident(ch, RefKind::Type, NONE, 255),
                _ => w.visit(c),
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

    fn switch_expression(&mut self, c: &mut TreeCursor<'t>) {
        let saved = self.switch_q;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::Parenthesized => {
                    // A selector declared in this file has a known type; otherwise its path.
                    let declared = ch.named_child(0).filter(|x| w.k(*x) == K::Identifier).and_then(|x| {
                        let name = w.cx.name(x);
                        w.cx.local_type(name).map(|v| if dec_dims(v) > 0 { NONE } else { dec_ty(v) })
                    });
                    let r = w.recv(c);
                    let q = match (declared, r.k) {
                        (Some(ty), _) => ty,
                        (None, RK::Val | RK::Pkg | RK::This) => w.qual(r),
                        _ => NONE,
                    };
                    // Labels of a switch over a string or a boxed number are not enum constants.
                    w.switch_q = if q != NONE && is_stop(w.cx.name_bytes(q)) { NONE } else { q };
                }
                _ => w.visit(c),
            }
        });
        self.switch_q = saved;
    }

    fn switch_label(&mut self, c: &mut TreeCursor<'t>) {
        self.pending_final = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Identifier => w.case_constant(ch),
                // No call can be a case label: this is a record pattern the grammar cannot parse.
                K::MethodInvocation => w.qualified_record_pattern(ch, c),
                K::Lambda => w.label_lambda(c),
                _ => w.visit(c),
            }
        });
    }

    /// `case X -> p -> body` is parsed as the label `X -> p` followed by `body`, and
    /// `case X -> c ? p -> a : q -> body` as the label `X -> c ? p -> a : q`. `X` is the case
    /// constant; the last name of the label is the parameter of the lambda that ends the rule.
    fn label_lambda(&mut self, c: &mut TreeCursor<'t>) {
        let mut first = true;
        self.each(c, |w, c| {
            let ch = c.node();
            let k = w.k(ch);
            if k == K::Token || k == K::Comment {
                return;
            }
            if std::mem::take(&mut first) {
                if k == K::Identifier {
                    w.case_constant(ch)
                } else {
                    w.visit(c)
                }
                return;
            }
            let mut last = ch;
            for _ in 0..16 {
                match last.named_child((last.named_child_count() as u32).wrapping_sub(1)) {
                    Some(x) if w.k(last) != K::Identifier => last = x,
                    _ => break,
                }
            }
            let param_at = if w.k(last) == K::Identifier { last.start_byte() } else { usize::MAX };
            w.label_lambda_body(c, param_at);
        });
    }

    /// Walk the body of a misparsed label lambda: an ordinary expression, except for the name
    /// at byte `param_at`, which declares the parameter of the lambda the rule returns.
    fn label_lambda_body(&mut self, c: &mut TreeCursor<'t>, param_at: usize) {
        let n = c.node();
        if n.end_byte() <= param_at || n.start_byte() > param_at {
            return self.visit(c);
        }
        if self.k(n) == K::Identifier {
            let name = self.cx.name(n);
            self.cx.declare(name, enc_local(NONE, 0));
            return;
        }
        self.each(c, |w, c| w.label_lambda_body(c, param_at));
    }

    /// `case Outer.Rec(T a, var b)`: the grammar only knows record patterns with a simple type
    /// name and parses a qualified one as a call whose arguments are `ERROR(type) name` pairs.
    /// Recover the type reference and the bindings.
    fn qualified_record_pattern(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let Some(nm) = self.decl_name(n) else { return self.children(c) };
        let nm_at = nm.start_byte();
        let mut q = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::ArgumentList => w.pattern_components(c),
                _ if ch.start_byte() == nm_at => w.type_ident(ch, RefKind::Type, q, 255),
                _ if ch.start_byte() < nm_at => {
                    let r = w.recv(c);
                    if r.k == RK::Type {
                        q = w.simple_of(r.base);
                    }
                }
                _ => w.visit(c),
            }
        });
    }

    fn pattern_components(&mut self, c: &mut TreeCursor<'t>) {
        let mut ty = (NONE, 0u8);
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::Error => {
                    // The type of the component that follows.
                    w.cx.facts.parse_errors += 1;
                    ty = ch.named_child(0).map_or((NONE, 0), |x| w.type_name(x));
                    w.children(c);
                }
                K::Identifier => {
                    let name = w.cx.name(ch);
                    w.cx.declare(name, enc_local(ty.0, ty.1));
                    ty = (NONE, 0);
                }
                K::MethodInvocation => w.qualified_record_pattern(ch, c),
                _ => w.pattern_leftovers(c),
            }
        });
    }

    /// Whatever else the parser made of a pattern component (`Box<T> b` reads as comparisons):
    /// names that look like types are types, the others are bindings.
    fn pattern_leftovers(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        match self.k(n) {
            K::Token | K::Comment | K::Skip | K::StringLiteral => {}
            K::Identifier => {
                let text = self.cx.text(n);
                if type_like(text) {
                    self.type_ident(n, RefKind::Type, NONE, 255);
                } else if !text.is_empty() {
                    let name = self.cx.intern(text);
                    self.cx.declare(name, enc_local(NONE, 0));
                }
            }
            K::TypeIdentifier | K::ScopedTypeIdentifier | K::GenericType | K::ArrayType | K::AnnotatedType => self.visit(c),
            K::MethodInvocation => self.qualified_record_pattern(n, c),
            K::Error => {
                self.cx.facts.parse_errors += 1;
                self.each(c, |w, c| w.pattern_leftovers(c));
            }
            _ => self.each(c, |w, c| w.pattern_leftovers(c)),
        }
    }

    /// A bare name in `case RED:` is a constant of the selector's enum type when that type is
    /// known; otherwise a constant in lexical scope, or unknown.
    fn case_constant(&mut self, n: Node<'t>) {
        let text = self.cx.text(n);
        if text.is_empty() || text == b"default" {
            return;
        }
        let name = self.cx.intern(text);
        let line = line_of(n);
        if self.switch_q != NONE {
            self.cx.add_ref(name, RefKind::MemberRead, line, self.switch_q, 255);
            return;
        }
        match self.cx.local_type(name) {
            Some(v) if v & B_MEMBER == 0 => {}
            Some(v) => {
                let q = self.owner_at(dec_level(v));
                self.cx.add_ref(name, RefKind::MemberRead, line, q, 255);
            }
            None => self.cx.add_ref(name, RefKind::MemberRead, line, NONE, 255),
        }
    }

    // ----- annotations -----

    fn annotation(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(nm) = child(n, t.f_name) else { return };
        let (simple, scope) = if self.k(nm) == K::ScopedIdentifier { (child(nm, t.f_name), child(nm, t.f_scope)) } else { (Some(nm), None) };
        let Some(simple) = simple else { return };
        let text = self.cx.text(simple);
        if text.is_empty() || is_stop(text) {
            return;
        }
        let name = self.cx.intern(text);
        let qual = match scope {
            Some(s) => self.name_scope(s, 0),
            None => NONE,
        };
        self.cx.add_ref(name, RefKind::Annotation, line_of(simple), qual, 255);
        // The annotation type as a type path, for its elements: `a.b|Marker`, `Outer::Marker`.
        let owner = match scope {
            Some(s) if qual != NONE => {
                let last = if self.k(s) == K::ScopedIdentifier { child(s, t.f_name).unwrap_or(s) } else { s };
                if starts_upper(self.cx.text(last)) { self.path(qual, name, false) } else { self.joined(b"", qual, b"|", name) }
            }
            _ => name,
        };
        self.each(c, |w, c| {
            if w.k(c.node()) != K::AnnotationArgumentList {
                return;
            }
            w.each(c, |w, c| {
                let ch = c.node();
                match w.k(ch) {
                    K::ElementValuePair => w.element_pair(ch, c, owner),
                    _ => w.visit(c),
                }
            });
        });
    }

    /// The scope of a qualified annotation name: an outer type (referenced, returns its name)
    /// or a package (returns the dotted name).
    fn name_scope(&mut self, s: Node<'t>, depth: u32) -> u32 {
        let t = self.t;
        let scoped = self.k(s) == K::ScopedIdentifier;
        let last = if scoped { child(s, t.f_name).unwrap_or(s) } else { s };
        let text = self.cx.text(last);
        if !starts_upper(text) || depth > 8 {
            return self.dotted(s);
        }
        let id = self.cx.intern(text);
        if !is_stop(text) {
            let q = match child(s, t.f_scope) {
                Some(x) if scoped => self.name_scope(x, depth + 1),
                _ => NONE,
            };
            self.cx.add_ref(id, RefKind::Type, line_of(last), q, 255);
        }
        id
    }

    /// `key = value` in an annotation: a write of the element `key` of the annotation type.
    fn element_pair(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, ann: u32) {
        let key = child(n, self.t.f_key);
        if let Some(k) = key.filter(|k| k.end_byte() > k.start_byte()) {
            let name = self.cx.name(k);
            self.cx.add_ref(name, RefKind::MemberWrite, line_of(k), ann, 255);
        }
        self.children_except(c, key.map_or(usize::MAX, |k| k.start_byte()));
    }

    // ----- expressions -----

    /// A bare name in an expression. Locals emit nothing; anything else is a member of the
    /// class that declares it in this file, or of the enclosing class (inherited, or imported
    /// statically). Outside any class it is a member of the implicit class of a compact source
    /// file, which has no name to qualify it with.
    fn identifier(&mut self, n: Node<'t>, write: bool) {
        if n.end_byte() == n.start_byte() {
            return;
        }
        let name = self.cx.name(n);
        let qual = match self.cx.local_type(name) {
            Some(v) if v & B_MEMBER == 0 => return,
            Some(v) => self.owner_at(dec_level(v)),
            // (`import static a.b.C.NAME`: a member of `C`, as if written `a.b.C.NAME`)
            None => match self.static_owner_of(name) {
                NONE => self.class_name(),
                owner => owner,
            },
        };
        let kind = if write { RefKind::MemberWrite } else { RefKind::MemberRead };
        self.cx.add_ref(name, kind, line_of(n), qual, 255);
    }

    /// A bare name used as a receiver or qualifier (`x` in `x.f`, `x.m()`, `x::m`).
    fn recv_ident(&mut self, n: Node<'t>) -> Recv {
        let text = self.cx.text(n);
        if text.is_empty() {
            return Recv::UNKNOWN;
        }
        let name = self.cx.intern(text);
        let line = line_of(n);
        if let Some(v) = self.cx.local_type(name) {
            let (ty, dims) = (dec_ty(v), dec_dims(v));
            if v & B_MEMBER == 0 {
                return Recv::val(ty, dims);
            }
            let own = self.owner_at(dec_level(v));
            self.cx.add_ref(name, RefKind::MemberRead, line, own, 255);
            // A member of an anonymous class has no owner to name: fall back on its declared type.
            return if own != NONE { Recv { k: RK::Val, base: own, fld: name, call: false, dims, mark: 0 } } else { Recv::val(ty, dims) };
        }
        if type_like(text) || (!self.odd_types.is_empty() && self.odd_types.contains(&name)) {
            if !is_stop(text) && self.tparam(name).is_none() {
                self.cx.add_ref(name, RefKind::Type, line, NONE, 255);
            }
            return Recv::ty(name);
        }
        // Not declared in this file: an inherited or statically imported member, or the first
        // segment of a package name (`java.util.List.of()`).
        let mark = self.cx.facts.refs.len() as u32;
        let owner = self.static_owner_of(name);
        if owner != NONE {
            // `import static a.b.C.INSTANCE`: a member of `C`, never a package segment.
            self.cx.add_ref(name, RefKind::MemberRead, line, owner, 255);
            return Recv { k: RK::Val, base: owner, fld: name, call: false, dims: 0, mark };
        }
        let k = if text.first().is_some_and(|b| b.is_ascii_lowercase()) { RK::Pkg } else { RK::Val };
        let q = self.class_name();
        self.cx.add_ref(name, RefKind::MemberRead, line, q, 255);
        Recv { k, base: q, fld: if q == NONE { NONE } else { name }, call: false, dims: 0, mark }
    }

    /// Walk the expression under the cursor as a receiver: emit its references and return what
    /// it denotes.
    fn recv(&mut self, c: &mut TreeCursor<'t>) -> Recv {
        let n = c.node();
        match self.k(n) {
            K::Identifier => self.recv_ident(n),
            K::This => Recv { k: RK::This, base: self.classes.last().map_or(NONE, |cl| cl.this_ty), fld: NONE, call: false, dims: 0, mark: 0 },
            K::Super => Recv::SUPER,
            K::FieldAccess => self.field_access(n, c, false, true),
            K::MethodInvocation => self.method_invocation(n, c),
            K::ObjectCreation => {
                let ty = self.object_creation(n, c);
                Recv::val(ty, 0)
            }
            K::Parenthesized => {
                let mut r = Recv::UNKNOWN;
                self.each(c, |w, c| {
                    if !matches!(w.k(c.node()), K::Token | K::Comment) {
                        r = w.recv(c);
                    }
                });
                r
            }
            K::CastExpression => {
                let (ty, dims) = child(n, self.t.f_type).map_or((NONE, 0), |x| self.type_name(x));
                self.children(c);
                Recv::val(ty, dims)
            }
            K::ArrayAccess => {
                let mut r = Recv::UNKNOWN;
                let mut first = true;
                self.each(c, |w, c| {
                    let k = w.k(c.node());
                    if k == K::Token || k == K::Comment {
                        return;
                    }
                    if std::mem::take(&mut first) { r = w.recv(c) } else { w.visit(c) }
                });
                // An element of the array: one dimension less. A member whose declaration is not
                // in this file keeps its path: field types are recorded by element type.
                match r.k {
                    RK::Val | RK::Pkg | RK::This => Recv { k: RK::Val, dims: r.dims.saturating_sub(1), ..r },
                    _ => Recv::UNKNOWN,
                }
            }
            K::StringLiteral => Recv::val(self.string_id(), 0),
            K::ClassLiteral => {
                self.children(c);
                Recv::val(self.class_id(), 0)
            }
            _ => {
                self.visit(c);
                Recv::UNKNOWN
            }
        }
    }

    /// `object.field`. Emits the member reference and returns what the whole access denotes.
    /// Only an access that is itself a qualifier (`as_recv`) can name a type or a package: as a
    /// value, `Property.NodeScope` and `Double.NaN` are members however they are spelled.
    fn field_access(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, write: bool, as_recv: bool) -> Recv {
        let t = self.t;
        let mut r = Recv::UNKNOWN;
        let mut first = true;
        self.each(c, |w, c| {
            // The object is the first child; `.`, `super` and the field name follow.
            if std::mem::take(&mut first) {
                r = w.recv(c);
            }
        });
        let Some(f) = child(n, t.f_field) else { return Recv::UNKNOWN };
        if self.k(f) != K::Identifier {
            // `Outer.this`
            return if r.k == RK::Type { Recv::val(r.base, 0) } else { Recv::UNKNOWN };
        }
        let text = self.cx.text(f);
        if text.is_empty() {
            return Recv::UNKNOWN;
        }
        let name = self.cx.intern(text);
        let line = line_of(f);
        let kind = if write { RefKind::MemberWrite } else { RefKind::MemberRead };
        match r.k {
            // (`Unit.B.toBytes()`: a one-letter name after a type is a constant, not a nested type.)
            RK::Type if as_recv && text.len() > 1 && type_like(text) => {
                // A nested type used in an expression: `Outer.Inner.CONST`, `Outer.Inner::new`.
                // It is a member of the outer type: the path `Outer::Inner`.
                if !is_stop(text) {
                    let outer = self.simple_of(r.base);
                    self.cx.add_ref(name, RefKind::Type, line, outer, 255);
                }
                Recv::ty(self.path(r.base, name, false))
            }
            RK::Type => {
                self.cx.add_ref(name, kind, line, r.base, 255);
                Recv { k: RK::Val, base: r.base, fld: name, call: false, dims: 0, mark: 0 }
            }
            RK::Pkg if as_recv && type_like(text) => {
                // `java.util.Objects`: the chain so far was a package name, not member reads.
                // The type keeps its package: `java.util|Objects`.
                self.cx.facts.refs.truncate(r.mark as usize);
                let q = child(n, t.f_object).map_or(NONE, |o| self.dotted(o));
                if !is_stop(text) {
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
                Recv { k, base: q, fld: if q == NONE { NONE } else { name }, call: false, dims: 0, mark: r.mark }
            }
        }
    }

    /// A method call. Emits the call reference and returns what the call denotes as a receiver:
    /// the declared result of the method, looked up on the receiver's type at query time
    /// (`repo.find(id)` with `repo: Repo` is `Repo::find()`).
    fn method_invocation(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) -> Recv {
        let Some(nm) = self.decl_name(n) else {
            self.children(c);
            return Recv::UNKNOWN;
        };
        let nm_at = nm.start_byte();
        let name = self.cx.name(nm);
        let mut r: Option<Recv> = None;
        let mut outer_super = false;
        let mut nargs = 255u8;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::ArgumentList => nargs = w.arguments(c, name),
                K::TypeArguments => w.children(c),
                K::Super => {
                    if r.is_some() {
                        outer_super = true;
                    } else {
                        r = Some(Recv::SUPER);
                    }
                }
                _ if ch.start_byte() == nm_at => {}
                _ if r.is_none() && ch.start_byte() < nm_at => r = Some(w.recv(c)),
                _ => w.visit(c),
            }
        });
        let line = line_of(nm);
        // `owner`: the type path the method is looked up on for the result of the call.
        let owner = match r {
            // Implicit `this`: the method belongs to the enclosing class, one of its ancestors
            // or outer classes, or is imported statically.
            None if self.anon_method(name) => {
                // A method of the anonymous class around the call: no class name qualifies it.
                // Its result is that of a function called without a receiver, `name()`.
                self.cx.add_ref(name, RefKind::Call, line, NONE, nargs);
                let root = self.joined(b"", name, b"()", NONE);
                return Recv::val(root, 0);
            }
            None => {
                // `import static a.b.C.m` makes `m()` the static call `a.b.C.m()`, unless a
                // class around the call declares a method `m` itself.
                let q = match self.static_owner_of(name) {
                    owner if owner != NONE && !self.shadowed.contains(&name) => owner,
                    _ => self.class_name(),
                };
                self.cx.add_ref(name, RefKind::Call, line, q, nargs);
                q
            }
            Some(r) if r.k == RK::Type && !outer_super => {
                self.cx.add_ref(name, RefKind::Call, line, r.base, nargs);
                r.base
            }
            Some(r) if r.k == RK::Type => {
                // `Outer.super.m()` runs an ancestor of the enclosing class `Outer`;
                // `Iface.super.m()` runs the default method of the interface itself.
                let q = if self.classes.iter().any(|cl| cl.own == r.base) { self.joined(b"^", r.base, b"", NONE) } else { r.base };
                self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
                r.base
            }
            Some(r) if r.k == RK::Super => {
                let q = self.super_qual();
                self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
                // The result is declared by an ancestor; the lookup starts at the class itself
                // (an override keeps the result type) and walks up from there.
                self.classes.last().map_or(NONE, |cl| cl.this_ty)
            }
            Some(r) => {
                let q = self.qual(r);
                self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
                q
            }
        };
        if owner == NONE { Recv::UNKNOWN } else { Recv { k: RK::Val, base: owner, fld: name, call: true, dims: 0, mark: 0 } }
    }

    /// Walk the argument list of a call of `callee` and count the arguments (comments are named
    /// nodes too). The count is unknown when the list did not parse. A method reference handed
    /// over directly is a callback registration: besides its own reference it is recorded as an
    /// argument of `callee`.
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
                K::MethodReference => {
                    n += 1;
                    let (method, line) = w.method_reference(ch, c);
                    if method != NONE && callee != NONE {
                        w.cx.add_ref(method, RefKind::Arg, line, callee, 255);
                    }
                }
                _ => {
                    n += 1;
                    w.visit(c);
                }
            }
        });
        if broken { 255 } else { n.min(254) as u8 }
    }

    /// `new T(..)`, possibly with an anonymous class body. Returns the type path of `T`.
    fn object_creation(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) -> u32 {
        let ty_node = child(n, self.t.f_type);
        let ty_at = ty_node.map_or(usize::MAX, |x| x.start_byte());
        let ty = ty_node.map_or(NONE, |x| self.type_name(x).0);
        let mut result = ty;
        let mut at: Option<usize> = None;
        let mut outer = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::ArgumentList => {
                    let callee = w.simple_of(ty);
                    let nargs = w.arguments(c, callee);
                    if let Some(r) = at.and_then(|i| w.cx.facts.refs.get_mut(i)) {
                        r.nargs = nargs;
                    }
                }
                K::Annotation | K::TypeArguments => w.visit(c),
                // `outer.new Inner()`: `Inner` is a member class of the type of `outer`.
                _ if ch.start_byte() < ty_at => {
                    let r = w.recv(c);
                    if r.k != RK::Type {
                        outer = w.qual(r);
                        if outer != NONE && ty != NONE && !w.cx.name_bytes(ty).contains(&b':') && !w.cx.name_bytes(ty).contains(&b'|') {
                            result = w.path(outer, ty, false);
                        }
                    }
                }
                K::ClassBody => {
                    // An anonymous class is also a subtype of `T`.
                    if let Some(r) = at.and_then(|i| w.cx.facts.refs.get(i)).copied() {
                        w.cx.add_ref(r.name, RefKind::Extends, r.line, r.qual, 255);
                    }
                    let name = w.class_name();
                    w.class_body(c, Class { name, own: NONE, this_ty: ty, sup: ty, sup_q: NONE, flags: CF_ANON, comps: (0, 0), methods: (0, 0) });
                }
                _ if ch.start_byte() == ty_at => {
                    let from = w.cx.facts.refs.len();
                    w.type_head(c, RefKind::New, 255);
                    at = w.cx.facts.refs.get(from..).and_then(|rs| rs.iter().position(|r| r.kind == RefKind::New)).map(|i| from + i);
                    if let Some(r) = at.and_then(|i| w.cx.facts.refs.get_mut(i)).filter(|r| r.qual == NONE) {
                        r.qual = outer;
                    }
                }
                _ => w.visit(c),
            }
        });
        result
    }

    /// `T::m`, `expr::m`, `this::m`, `T::new`. Returns the referenced method (`NONE` for
    /// constructors) and its line.
    fn method_reference(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) -> (u32, u32) {
        let last = n.child(n.child_count().wrapping_sub(1));
        let method = last.filter(|x| self.k(*x) == K::Identifier && x.end_byte() > x.start_byte());
        let m_at = method.map_or(usize::MAX, |x| x.start_byte());
        let mut q = NONE;
        let mut qualifier = true;
        self.each(c, |w, c| {
            let ch = c.node();
            let k = w.k(ch);
            if k == K::Token || k == K::Comment || ch.start_byte() == m_at {
                return;
            }
            if !std::mem::take(&mut qualifier) {
                // explicit type arguments
                return w.visit(c);
            }
            match k {
                // `T::new` instantiates `T`.
                K::Identifier if method.is_none() => {
                    let name = w.cx.name(ch);
                    if w.cx.local_type(name).is_none() {
                        w.type_ident(ch, RefKind::New, NONE, 255);
                    } else {
                        w.identifier(ch, false);
                    }
                }
                // `Outer.super::m` reads as the type `Outer.super`: a method of an ancestor of
                // the enclosing class `Outer`, or the default method of the interface `Outer`.
                K::ScopedTypeIdentifier
                    if method.is_some()
                        && ch.named_child_count() == 2
                        && ch.named_child(1).is_some_and(|x| w.cx.text(x) == b"super")
                        && ch.named_child(0).is_some_and(|x| w.k(x) == K::TypeIdentifier) =>
                {
                    if let Some(o) = ch.named_child(0) {
                        let id = w.cx.name(o);
                        w.type_ident(o, RefKind::Type, NONE, 255);
                        q = if w.classes.iter().any(|cl| cl.own == id) { w.joined(b"^", id, b"", NONE) } else { id };
                    }
                }
                K::TypeIdentifier | K::ScopedTypeIdentifier | K::GenericType | K::AnnotatedType => {
                    if method.is_none() {
                        w.type_head(c, RefKind::New, 255);
                    } else {
                        q = w.type_name(ch).0;
                        w.visit(c);
                    }
                }
                K::ArrayType => w.visit(c),
                _ => {
                    let from = w.cx.facts.refs.len();
                    let r = w.recv(c);
                    if method.is_some() {
                        q = w.qual(r);
                    } else if r.k == RK::Type {
                        // `Outer.Inner::new`: the type reference just emitted is the instantiation.
                        let last = w.cx.facts.refs.get_mut(from..).and_then(|rs| rs.last_mut());
                        if let Some(x) = last.filter(|x| x.kind == RefKind::Type) {
                            x.kind = RefKind::New;
                        }
                    }
                }
            }
        });
        match method {
            Some(m) => {
                let name = self.cx.name(m);
                let line = line_of(m);
                self.cx.add_ref(name, RefKind::Read, line, q, 255);
                (name, line)
            }
            None => (NONE, 0),
        }
    }

    fn assignment(&mut self, c: &mut TreeCursor<'t>) {
        let mut idx = 0;
        let mut slot: Option<(u32, u32)> = None;
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
                if matches!(k, K::Identifier | K::FieldAccess) {
                    let last = w.cx.facts.refs.get(from..).and_then(|rs| rs.last());
                    slot = last.filter(|r| r.kind == RefKind::MemberWrite).map(|r| (r.qual, r.name));
                }
            } else if k == K::MethodReference {
                // A method stored into a member: also a binding into the slot `Owner.member`.
                let (method, line) = w.method_reference(ch, c);
                if let (Some((owner, f)), true) = (slot, method != NONE) {
                    let s = w.joined(b"", owner, b".", f);
                    w.cx.add_ref(method, RefKind::Bind, line, s, 255);
                }
            } else {
                w.visit(c);
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

    fn update(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            if !matches!(w.k(c.node()), K::Token | K::Comment) {
                w.lvalue(c);
            }
        });
    }

    /// An assignment target. Storing into an array element counts as a write of the array, as
    /// in the C extractor.
    fn lvalue(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        match self.k(n) {
            K::Identifier => self.identifier(n, true),
            K::FieldAccess => {
                self.field_access(n, c, true, false);
            }
            K::ArrayAccess => {
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

    /// `this(..)` / `super(..)` at the start of a constructor: a call of a constructor of the
    /// class or of its written superclass.
    fn explicit_constructor_call(&mut self, c: &mut TreeCursor<'t>) {
        let mut target = NONE;
        let mut line = 0;
        let mut nargs = 255u8;
        let cls = self.classes.last().copied();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::This => {
                    target = cls.map_or(NONE, |cl| cl.own);
                    line = line_of(ch);
                }
                K::Super => {
                    target = cls.map_or(NONE, |cl| cl.sup);
                    line = line_of(ch);
                }
                K::ArgumentList => {
                    let callee = w.simple_of(target);
                    nargs = w.arguments(c, callee);
                }
                _ => w.visit(c),
            }
        });
        if target != NONE {
            // The constructor is named after its class; the class is written as a type path.
            let name = self.simple_of(target);
            if !is_stop(self.cx.name_bytes(name)) {
                self.cx.add_ref(name, RefKind::Call, line, target, nargs);
            }
        }
    }

    /// Names directly under an ERROR node are fragments the parser could not place (sometimes
    /// keywords read as names): noise, not references.
    fn error(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.facts.parse_errors += 1;
        let pending = std::mem::replace(&mut self.pending_final, NONE);
        if pending != NONE {
            // The binding of a `case final T name` pattern.
            let n = c.node();
            if let (1, Some(x)) = (n.named_child_count(), n.named_child(0))
                && self.k(x) == K::Identifier
                && x.end_byte() > x.start_byte()
            {
                let name = self.cx.name(x);
                self.cx.declare(name, enc_local(pending, 0));
                return;
            }
        }
        self.each(c, |w, c| {
            if !matches!(w.k(c.node()), K::Identifier | K::TypeIdentifier) {
                w.visit(c);
            }
        });
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

/// Names that never produce a reference: the `java.lang` basics, the built-in annotations and
/// `var`.
fn is_stop(b: &[u8]) -> bool {
    matches!(
        b,
        b"String"
            | b"Object"
            | b"Integer"
            | b"Long"
            | b"Double"
            | b"Float"
            | b"Boolean"
            | b"Character"
            | b"Byte"
            | b"Short"
            | b"Void"
            | b"Number"
            | b"Math"
            | b"System"
            | b"Class"
            | b"StringBuilder"
            | b"Iterable"
            | b"Comparable"
            | b"Runnable"
            | b"Thread"
            | b"Override"
            | b"Deprecated"
            | b"SuppressWarnings"
            | b"FunctionalInterface"
            | b"SafeVarargs"
            | b"var"
    )
}
