//! PHP extractor (tree-sitter-php, the `php` dialect: text outside `<?php ... ?>` is skipped).
//!
//! PHP resolves every class, function and constant name at compile time from the namespace the
//! code is in and the `use` imports above it. The walker does the part of that resolution that
//! a file can do alone and leaves the rest to the resolver:
//!   * a name written bare (`User`, `helper()`) stays bare: the resolver looks it up in the file,
//!     through the `use` imports, in the file's namespace and, for functions and constants, in
//!     the global namespace, which is where PHP falls back to;
//!   * a qualified name is turned into its fully qualified namespace. `\A\B\C` is in `A\B`;
//!     `B\C` is in `A\B` when `use A\B;` imports `B`, else in `NS\B` inside namespace `NS`;
//!     `namespace\C` is in the current namespace. The namespace becomes the module qualifier:
//!     `Call` / `New` / `Type` qual=`A\B`, and type paths and `type_name` read `A\B|C`. The
//!     global namespace is written `\`.
//!
//! Variables always start with `$`, so they never produce references. They are tracked only to
//! type receivers: from parameter types (or `@param`), `$x = new T`, `$x = T::make()`,
//! `$x = $this->f()`, `$x = f()`, copies of typed variables, `catch (T $e)`, and inline
//! `/** @var T $x */` comments. A type holds while every assignment agrees on it.
//!
//! Conventions the resolver can rely on:
//!   * Definitions. `namespace A\B;` is a `Namespace` named `A\B`, the first definition of its
//!     file. Classes, interfaces, traits, enums, functions and constants have no parent: PHP
//!     declares them globally wherever the declaration sits (a function inside a function or an
//!     `if (!function_exists(..))` block included). Members (methods, properties as `Field`,
//!     class constants as `Field`, enum cases as `EnumMember`, promoted constructor parameters
//!     as `Field`) have their class as parent. Members of an anonymous class have the enclosing
//!     definition as parent; the `Extends T` next to them names their supertype. Property and
//!     constant names are written without `$`. `define('X', ..)` is a `Constant` `X`.
//!   * Visibility: `private` is `LOCAL` (except in traits, whose private methods are called from
//!     the classes that use them), `public` or none is `EXPORTED`, `protected` is neither.
//!     Top-level declarations are `EXPORTED`.
//!   * Receivers. `$this->m()` is a `MemberCall C`, `$x->m()` a `MemberCall T` for a variable of
//!     type `T`, `$this->repo->m()` a `MemberCall C::repo`, `$x->a()->b()` a `MemberCall T::a()`.
//!     `T::m()`, `self::m()` and `static::m()` are a `Call` qualified by the class, `parent::m()`
//!     a `MemberCall ^C`. `T::$p` and `T::CONST` are `MemberRead`s of `p` / `CONST` qualified by
//!     `T`, and `T::CASE->m()` is a call on `T` (a class constant holding an object is an enum
//!     case). A return type `static` or `self` is the class itself.
//!   * `use TraitName;` inside a class is an `Extends` reference: a trait contributes members to
//!     the class the way a base class or a Python mixin does, and the resolver walks `Extends`
//!     and `Implements` alike when it looks for a member. `implements` is `Implements`, an
//!     interface's `extends` is `Extends`.
//!   * Callables. `[$this, 'm']`, `[T::class, 'm']` and `$this->m(...)` are a `MemberRead` of `m`
//!     (a method used as a value), `f(...)` a `Read` of `f`. Passed directly to a call they are
//!     also an `Arg` of the callee; stored directly into a member, also a `Bind` into the slot
//!     `Owner.member`.
//!
//! Functions and classes are case-insensitive in PHP. Names are matched as written: code nearly
//! always uses the declared case.

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
    Name,
    QualifiedName,
    RelativeName,
    VariableName,
    RelativeScope,
    NamespaceDefinition,
    NamespaceUse,
    NamespaceUseClause,
    NamespaceUseGroup,
    NamespaceName,
    ClassDecl,
    InterfaceDecl,
    TraitDecl,
    EnumDecl,
    AnonClass,
    DeclList,
    BaseClause,
    InterfaceClause,
    TraitUse,
    UseList,
    UseAs,
    UseInsteadOf,
    ConstDecl,
    ConstElement,
    PropertyDecl,
    PropertyElement,
    PropertyHookList,
    PropertyHook,
    EnumCase,
    MethodDecl,
    FunctionDef,
    FormalParams,
    SimpleParam,
    VariadicParam,
    PromotionParam,
    AnonFunction,
    AnonUseClause,
    ArrowFunction,
    AttributeList,
    AttributeGroup,
    Attribute,
    NamedType,
    OptionalType,
    UnionType,
    IntersectionType,
    DnfType,
    PrimitiveType,
    TypeList,
    FunctionCall,
    MemberCall,
    ScopedCall,
    MemberAccess,
    ScopedPropertyAccess,
    ClassConstantAccess,
    ObjectCreation,
    Arguments,
    Argument,
    VariadicPlaceholder,
    VariadicUnpacking,
    Assignment,
    AugmentedAssignment,
    Update,
    Binary,
    Unary,
    Foreach,
    Pair,
    ByRef,
    ListLiteral,
    CatchClause,
    GlobalDecl,
    StaticVarDecl,
    Include,
    ExpressionStatement,
    ArrayCreation,
    ArrayElement,
    String,
    StringContent,
    Parenthesized,
    Clone,
    Subscript,
    Yield,
    Null,
    VisibilityModifier,
    AbstractModifier,
    StaticModifier,
    VarModifier,
    CompoundStatement,
    Program,
}

struct Tables {
    kinds: KindTable<K>,
    f_name: u16,
    f_body: u16,
    f_type: u16,
    f_return_type: u16,
    f_object: u16,
    f_arguments: u16,
    f_scope: u16,
    f_function: u16,
    f_left: u16,
    f_right: u16,
    f_operator: u16,
    f_argument: u16,
    f_alias: u16,
    f_default_value: u16,
    f_value: u16,
    f_visibility: u16,
    kw_instanceof: u16,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let lang: tree_sitter::Language = tree_sitter_php::LANGUAGE_PHP.into();
        let special: &[(&str, K)] = &[
            ("ERROR", K::Error),
            ("program", K::Program),
            ("comment", K::Comment),
            ("name", K::Name),
            ("qualified_name", K::QualifiedName),
            ("relative_name", K::RelativeName),
            ("variable_name", K::VariableName),
            ("relative_scope", K::RelativeScope),
            ("namespace_definition", K::NamespaceDefinition),
            ("namespace_use_declaration", K::NamespaceUse),
            ("namespace_use_clause", K::NamespaceUseClause),
            ("namespace_use_group", K::NamespaceUseGroup),
            ("namespace_name", K::NamespaceName),
            ("class_declaration", K::ClassDecl),
            ("interface_declaration", K::InterfaceDecl),
            ("trait_declaration", K::TraitDecl),
            ("enum_declaration", K::EnumDecl),
            ("anonymous_class", K::AnonClass),
            ("declaration_list", K::DeclList),
            ("enum_declaration_list", K::DeclList),
            ("base_clause", K::BaseClause),
            ("class_interface_clause", K::InterfaceClause),
            ("use_declaration", K::TraitUse),
            ("use_list", K::UseList),
            ("use_as_clause", K::UseAs),
            ("use_instead_of_clause", K::UseInsteadOf),
            ("const_declaration", K::ConstDecl),
            ("const_element", K::ConstElement),
            ("property_declaration", K::PropertyDecl),
            ("property_element", K::PropertyElement),
            ("property_hook_list", K::PropertyHookList),
            ("property_hook", K::PropertyHook),
            ("enum_case", K::EnumCase),
            ("method_declaration", K::MethodDecl),
            ("function_definition", K::FunctionDef),
            ("formal_parameters", K::FormalParams),
            ("simple_parameter", K::SimpleParam),
            ("variadic_parameter", K::VariadicParam),
            ("property_promotion_parameter", K::PromotionParam),
            ("anonymous_function", K::AnonFunction),
            ("anonymous_function_use_clause", K::AnonUseClause),
            ("arrow_function", K::ArrowFunction),
            ("attribute_list", K::AttributeList),
            ("attribute_group", K::AttributeGroup),
            ("attribute", K::Attribute),
            ("named_type", K::NamedType),
            ("optional_type", K::OptionalType),
            ("union_type", K::UnionType),
            ("intersection_type", K::IntersectionType),
            ("disjunctive_normal_form_type", K::DnfType),
            ("primitive_type", K::PrimitiveType),
            ("type_list", K::TypeList),
            ("function_call_expression", K::FunctionCall),
            ("member_call_expression", K::MemberCall),
            ("nullsafe_member_call_expression", K::MemberCall),
            ("scoped_call_expression", K::ScopedCall),
            ("member_access_expression", K::MemberAccess),
            ("nullsafe_member_access_expression", K::MemberAccess),
            ("scoped_property_access_expression", K::ScopedPropertyAccess),
            ("class_constant_access_expression", K::ClassConstantAccess),
            ("object_creation_expression", K::ObjectCreation),
            ("arguments", K::Arguments),
            ("argument", K::Argument),
            ("variadic_placeholder", K::VariadicPlaceholder),
            ("variadic_unpacking", K::VariadicUnpacking),
            ("assignment_expression", K::Assignment),
            ("reference_assignment_expression", K::Assignment),
            ("augmented_assignment_expression", K::AugmentedAssignment),
            ("update_expression", K::Update),
            ("binary_expression", K::Binary),
            ("unary_op_expression", K::Unary),
            ("foreach_statement", K::Foreach),
            ("pair", K::Pair),
            ("by_ref", K::ByRef),
            ("list_literal", K::ListLiteral),
            ("catch_clause", K::CatchClause),
            ("global_declaration", K::GlobalDecl),
            ("static_variable_declaration", K::StaticVarDecl),
            ("include_expression", K::Include),
            ("include_once_expression", K::Include),
            ("require_expression", K::Include),
            ("require_once_expression", K::Include),
            ("expression_statement", K::ExpressionStatement),
            ("array_creation_expression", K::ArrayCreation),
            ("array_element_initializer", K::ArrayElement),
            ("string", K::String),
            ("string_content", K::StringContent),
            ("parenthesized_expression", K::Parenthesized),
            ("clone_expression", K::Clone),
            ("subscript_expression", K::Subscript),
            ("yield_expression", K::Yield),
            ("null", K::Null),
            ("visibility_modifier", K::VisibilityModifier),
            ("abstract_modifier", K::AbstractModifier),
            ("static_modifier", K::StaticModifier),
            ("var_modifier", K::VarModifier),
            ("compound_statement", K::CompoundStatement),
            // leaves and subtrees that never contain references
            ("php_tag", K::Skip),
            ("php_end_tag", K::Skip),
            ("text", K::Skip),
            ("text_interpolation", K::Skip),
            ("integer", K::Skip),
            ("float", K::Skip),
            ("boolean", K::Skip),
            ("nowdoc", K::Skip),
            ("escape_sequence", K::Skip),
            ("cast_type", K::Skip),
            ("declare_directive", K::Skip),
            ("named_label_statement", K::Skip),
            ("goto_statement", K::Skip),
            ("break_statement", K::Skip),
            ("continue_statement", K::Skip),
            ("final_modifier", K::Skip),
            ("readonly_modifier", K::Skip),
            ("reference_modifier", K::Skip),
            ("bottom_type", K::Skip),
            ("operation", K::Skip),
            ("heredoc_start", K::Skip),
            ("heredoc_end", K::Skip),
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
            f_return_type: field(&lang, "return_type"),
            f_object: field(&lang, "object"),
            f_arguments: field(&lang, "arguments"),
            f_scope: field(&lang, "scope"),
            f_function: field(&lang, "function"),
            f_left: field(&lang, "left"),
            f_right: field(&lang, "right"),
            f_operator: field(&lang, "operator"),
            f_argument: field(&lang, "argument"),
            f_alias: field(&lang, "alias"),
            f_default_value: field(&lang, "default_value"),
            f_value: field(&lang, "value"),
            f_visibility: field(&lang, "visibility"),
            kw_instanceof: lang.id_for_node_kind("instanceof", false),
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Walker state
// ---------------------------------------------------------------------------------------------

/// Interface body: members are public and abstract.
const CF_IFACE: u8 = 1;
/// Trait body: private members are reachable from the classes that use the trait.
const CF_TRAIT: u8 = 2;
/// The class name marks a test class (`*Test`, `*Tests`, `*TestCase`).
const CF_TEST: u8 = 4;
/// Anonymous class body: no name of its own.
const CF_ANON: u8 = 8;

#[derive(Clone, Copy)]
struct Class {
    /// The class's own name; `NONE` for an anonymous class.
    own: u32,
    /// Static type of `$this`, `self` and `static`: the class, or the supertype of an anonymous
    /// class.
    this_ty: u32,
    /// Type path of the written superclass (`extends T`), for `parent`.
    sup: u32,
    /// `^own`, interned on first use.
    sup_q: u32,
    flags: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RK {
    /// A value whose static type path is `base` (+ `::fld`), `NONE` when unknown.
    Val,
    /// A class used as a scope (`T::`, `self::`): `base` is its type path.
    Type,
    /// `parent::`.
    Parent,
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
}

impl Recv {
    const UNKNOWN: Recv = Recv { k: RK::Val, base: NONE, fld: NONE, call: false };

    #[inline]
    fn val(ty: u32) -> Recv {
        Recv { k: RK::Val, base: ty, fld: NONE, call: false }
    }

    #[inline]
    fn member(base: u32, fld: u32, call: bool) -> Recv {
        if base == NONE || fld == NONE { Recv::UNKNOWN } else { Recv { k: RK::Val, base, fld, call } }
    }
}

/// A class, function or constant name as PHP resolves it in this file: `module` is the fully
/// qualified namespace for a qualified name (`\` for the global one), `NONE` for a bare name.
#[derive(Clone, Copy)]
struct QN {
    name: u32,
    module: u32,
}

/// A variable that was declared but has no type yet (`$x = null` before `$x = new T`).
const UNSET: u32 = NONE - 1;

/// What the modifiers of a member say.
const M_PRIVATE: u8 = 1;
const M_PROTECTED: u8 = 2;
const M_ABSTRACT: u8 = 4;
const M_TEST: u8 = 8;

struct W<'a, 't> {
    cx: Ctx<'a>,
    t: &'static Tables,
    /// Enclosing class bodies, outermost first.
    classes: Vec<Class>,
    /// The current namespace (`NONE` for the global one).
    ns: u32,
    /// Class-kind `use` imports of the current namespace: (local name, fully qualified name).
    /// They resolve the first segment of a qualified name.
    aliases: Vec<(u32, u32)>,
    /// Variables of the enclosing functions with their type paths; those from `vbase` on
    /// belong to the innermost function (a closure sees only what its `use` clause brings in).
    vars: Vec<(u32, u32)>,
    vbase: usize,
    /// Variables of the innermost function typed by an `instanceof` test (from `nbase` on).
    narrowed: Vec<u32>,
    nbase: usize,
    /// The `instanceof` about to be visited is the operand of a `!`.
    negated: bool,
    /// The last `/** ... */` comment seen: a docblock for the declaration that follows it.
    doc: Option<Node<'t>>,
    /// Name of the function or method that the last first-class callable or callable array
    /// denoted (`f(...)`, `[$this, 'm']`), for the `Arg` / `Bind` reference around it.
    last_callable: u32,
    /// Set when a `yield` is seen: the enclosing function is a generator.
    saw_yield: bool,
    /// `\`, the global namespace.
    global_id: u32,
    _t: PhantomData<&'t ()>,
}

pub fn extract(_lang: Lang, src: &[u8], tree: &Tree, path: &str, facts: &mut FileFacts, scratch: &mut Scratch) {
    let cx = Ctx::new(src, path, facts, scratch);
    let mut w = W {
        cx,
        t: tables(),
        classes: Vec::new(),
        ns: NONE,
        aliases: Vec::new(),
        vars: Vec::new(),
        vbase: 0,
        narrowed: Vec::new(),
        nbase: 0,
        negated: false,
        doc: None,
        last_callable: NONE,
        saw_yield: false,
        global_id: NONE,
        _t: PhantomData,
    };
    w.global_id = w.cx.intern(b"\\");
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

    /// Visit the children except the one attached through `skip_field`.
    fn children_except(&mut self, c: &mut TreeCursor<'t>, skip_field: u16) {
        self.each(c, |w, c| {
            if !is_field(c, skip_field) {
                w.visit(c);
            }
        });
    }

    fn visit(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        let k = self.k(n);
        if k == K::Comment {
            if self.cx.text(n).starts_with(b"/**") {
                self.doc = Some(n);
            }
            return;
        }
        if k == K::Token {
            return;
        }
        match k {
            K::Skip | K::VariableName | K::String | K::StringContent | K::Null | K::PrimitiveType | K::VariadicPlaceholder => {}
            K::VisibilityModifier | K::AbstractModifier | K::StaticModifier | K::VarModifier | K::RelativeScope | K::NamespaceName => {}
            K::Name | K::QualifiedName | K::RelativeName => self.constant(n),
            K::Program => self.children(c),
            K::NamespaceDefinition => self.namespace_def(n, c),
            K::NamespaceUse => self.use_decl(n),
            K::ClassDecl => self.class_decl(n, c, DefKind::Class, 0),
            K::InterfaceDecl => self.class_decl(n, c, DefKind::Interface, CF_IFACE),
            K::TraitDecl => self.class_decl(n, c, DefKind::Trait, CF_TRAIT),
            K::EnumDecl => self.class_decl(n, c, DefKind::Enum, 0),
            K::FunctionDef => self.function(n, c),
            K::MethodDecl => self.method(n, c),
            K::PropertyDecl => self.property_decl(n, c),
            K::ConstDecl => self.const_decl(n, c),
            K::EnumCase => self.enum_case(n, c),
            K::TraitUse => self.trait_use(c),
            K::AttributeList | K::AttributeGroup => self.children(c),
            K::Attribute => self.attribute(n, c),
            K::FunctionCall => {
                self.function_call(n, c);
            }
            K::MemberCall => {
                self.member_call(n, c);
            }
            K::ScopedCall => {
                self.scoped_call(n, c);
            }
            K::MemberAccess => {
                self.member_access(n, c, false);
            }
            K::ScopedPropertyAccess => {
                self.scoped_prop(n, c, false);
            }
            K::ClassConstantAccess => {
                self.class_const(n, c);
            }
            K::ObjectCreation => {
                self.object_creation(n, c);
            }
            K::ArrayCreation => {
                self.array(n, c);
            }
            K::Assignment => self.assignment(n, c),
            K::AugmentedAssignment | K::Update => self.update(c),
            K::Binary => self.binary(c),
            K::Unary => {
                // `! $x instanceof T`
                let arg = child(n, self.t.f_argument);
                let not = child(n, self.t.f_operator).is_some_and(|o| self.cx.text(o) == b"!");
                self.each(c, |w, c| {
                    if arg.is_some_and(|a| a.start_byte() == c.node().start_byte() && w.k(a) == K::Binary) {
                        w.negated = not && w.is_instanceof(c.node());
                    }
                    w.visit(c);
                });
            }
            K::AnonFunction => self.closure(n, c),
            K::ArrowFunction => self.arrow(n, c),
            K::Foreach => self.foreach(n, c),
            K::CatchClause => self.catch_clause(n, c),
            K::GlobalDecl => self.declare_targets(c),
            K::StaticVarDecl => self.static_var(n, c),
            K::Include => self.include(n, c),
            K::ExpressionStatement => self.statement(n, c),
            K::NamedType | K::OptionalType | K::UnionType | K::IntersectionType | K::DnfType | K::TypeList => self.type_refs(c),
            K::Arguments => {
                self.arguments(c, NONE);
            }
            K::Argument => {
                self.argument(c, NONE);
            }
            K::Yield => {
                self.saw_yield = true;
                self.children(c);
            }
            K::Error => self.error(c),
            _ => self.children(c),
        }
    }

    // ----- small helpers -----

    #[inline]
    fn this_ty(&self) -> u32 {
        self.classes.last().map_or(NONE, |c| c.this_ty)
    }

    /// Intern `a` + `sep` + `b`, reusing the scratch buffer.
    fn joined(&mut self, a: u32, sep: &[u8], b: u32) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
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

    /// The qualifier a receiver contributes to a member reference.
    fn qual(&mut self, r: Recv) -> u32 {
        match r.k {
            RK::Parent => self.super_qual(),
            RK::Type => r.base,
            _ if r.base == NONE => NONE,
            _ if r.fld == NONE => r.base,
            _ => self.path(r.base, r.fld, r.call),
        }
    }

    /// `^Class`: a strict ancestor of the enclosing class. In an anonymous class, its written
    /// supertype.
    fn super_qual(&mut self) -> u32 {
        let Some(cl) = self.classes.last().copied() else { return NONE };
        if cl.flags & CF_ANON != 0 {
            return cl.sup;
        }
        if cl.sup_q != NONE || cl.own == NONE {
            return cl.sup_q;
        }
        let q = self.joined(NONE, b"^", cl.own);
        if let Some(cl) = self.classes.last_mut() {
            cl.sup_q = q;
        }
        q
    }

    /// The type path of a resolved class name: `Name` or `A\B|Name`.
    fn ty_path(&mut self, q: QN) -> u32 {
        if q.name == NONE {
            NONE
        } else if q.module == NONE {
            q.name
        } else {
            self.joined(q.module, b"|", q.name)
        }
    }

    /// The type path of the declared result of function `q`: `f()` or `A\B|f()`.
    fn fn_result(&mut self, q: QN) -> u32 {
        if q.name == NONE {
            return NONE;
        }
        let root = self.joined(q.name, b"()", NONE);
        if q.module == NONE { root } else { self.joined(q.module, b"|", root) }
    }

    /// End of a declaration without its trailing `;`.
    fn before_semicolon(&self, n: Node<'t>) -> usize {
        let e = n.end_byte();
        if e > n.start_byte() && self.cx.src.get(e - 1) == Some(&b';') { e - 1 } else { e }
    }

    /// Byte offset before `= value`: the signature of an initialised declaration ends there.
    fn before_initializer(&self, start: usize, value_at: usize) -> usize {
        let mut e = value_at;
        let src = self.cx.src;
        while e > start && src[e - 1].is_ascii_whitespace() {
            e -= 1;
        }
        if e > start && src[e - 1] == b'=' {
            e -= 1;
        }
        while e > start && src[e - 1].is_ascii_whitespace() {
            e -= 1;
        }
        e
    }

    /// Where the signature of a declaration starts: after its attributes.
    fn sig_start(&self, n: Node<'t>) -> usize {
        for i in 0..n.child_count().min(16) {
            let Some(ch) = n.child(i) else { break };
            if !matches!(self.k(ch), K::AttributeList | K::Comment) {
                return ch.start_byte();
            }
        }
        n.start_byte()
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

    /// The docblock right in front of `n` (only blank space between them), if any.
    fn doc_of(&mut self, n: Node<'t>) -> Option<&'a [u8]> {
        let d = self.doc.take()?;
        let (e, s) = (d.end_byte(), n.start_byte());
        if e > s || !self.cx.src[e..s].iter().all(|b| b.is_ascii_whitespace()) {
            return None;
        }
        Some(self.cx.text(d))
    }

    // ----- variables -----

    /// Type of a variable of the innermost function; `None` when it was never assigned.
    #[inline]
    fn var(&self, name: u32) -> Option<u32> {
        self.vars.get(self.vbase..)?.iter().rev().find(|v| v.0 == name).map(|v| if v.1 == UNSET { NONE } else { v.1 })
    }

    /// Record an assignment of a value of type `ty`. A variable keeps a type only while every
    /// assignment agrees on it.
    fn assign_var(&mut self, name: u32, ty: u32) {
        let base = self.vbase;
        match self.vars[base..].iter_mut().rev().find(|v| v.0 == name) {
            Some(v) if v.1 == UNSET => v.1 = ty,
            Some(v) if ty != UNSET && v.1 != ty => v.1 = NONE,
            Some(_) => {}
            None => self.vars.push((name, ty)),
        }
    }

    /// Declare a variable with a known type, overriding what was inferred (`@var`, parameters).
    fn set_var(&mut self, name: u32, ty: u32) {
        let base = self.vbase;
        match self.vars[base..].iter_mut().rev().find(|v| v.0 == name) {
            Some(v) => v.1 = ty,
            None => self.vars.push((name, ty)),
        }
    }

    /// The name of a `$variable` node, without the `$`.
    fn var_name(&mut self, n: Node<'t>) -> Option<u32> {
        if self.k(n) != K::VariableName {
            return None;
        }
        let nm = n.named_child(0).filter(|x| self.k(*x) == K::Name && x.end_byte() > x.start_byte())?;
        Some(self.cx.name(nm))
    }

    // ----- names -----

    /// Resolve a written class / function / constant name (`Foo`, `A\Foo`, `\A\Foo`,
    /// `namespace\Foo`) the way PHP does for the current file.
    fn resolve_name(&mut self, text: &[u8]) -> QN {
        let (fq, t) = match text.strip_prefix(b"\\") {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        if t.is_empty() {
            return QN { name: NONE, module: NONE };
        }
        let (relative, t) = match t.strip_prefix(b"namespace\\") {
            Some(rest) if !fq => (true, rest),
            _ => (false, t),
        };
        let Some(cut) = t.iter().rposition(|&b| b == b'\\') else {
            let name = self.cx.intern(t);
            // `\Foo` in a namespace names the global `Foo`; in the global namespace it is `Foo`.
            let module = if relative {
                self.ns
            } else if fq && self.ns != NONE {
                self.global_id
            } else {
                NONE
            };
            return QN { name, module };
        };
        let (prefix, last) = (&t[..cut], &t[cut + 1..]);
        if last.is_empty() || prefix.is_empty() {
            return QN { name: NONE, module: NONE };
        }
        let name = self.cx.intern(last);
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        if fq {
            buf.extend_from_slice(prefix);
        } else {
            let first_end = prefix.iter().position(|&b| b == b'\\').unwrap_or(prefix.len());
            let first = &prefix[..first_end];
            let full = if relative { None } else { self.aliases.iter().rev().find(|a| self.cx.name_bytes(a.0) == first).map(|a| a.1) };
            match full {
                Some(f) => {
                    buf.extend_from_slice(self.cx.name_bytes(f));
                    buf.extend_from_slice(&prefix[first_end..]);
                }
                None => {
                    if self.ns != NONE {
                        buf.extend_from_slice(self.cx.name_bytes(self.ns));
                        buf.push(b'\\');
                    }
                    buf.extend_from_slice(prefix);
                }
            }
        }
        let module = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        QN { name, module }
    }

    /// Resolve a name node.
    fn qn(&mut self, n: Node<'t>) -> QN {
        let text = self.cx.text(n);
        if n.end_byte() == n.start_byte() {
            return QN { name: NONE, module: NONE };
        }
        if text.iter().any(|b| b.is_ascii_whitespace() || *b == b'/' || *b == b'#') {
            // A name broken over lines or around a comment: the segments are its `name` nodes.
            let mut buf: Vec<u8> = Vec::with_capacity(text.len());
            if text.first() == Some(&b'\\') {
                buf.push(b'\\');
            }
            push_segments(self, n, &mut buf, 0);
            return self.resolve_name(&buf);
        }
        self.resolve_name(text)
    }

    /// Does an unqualified class name written here denote a class of the global namespace?
    fn global_class(&self, q: QN) -> bool {
        if q.module == self.global_id {
            return true;
        }
        if q.module != NONE {
            return false;
        }
        if self.ns == NONE {
            return true;
        }
        let name = self.cx.name_bytes(q.name);
        self.aliases.iter().rev().find(|a| self.cx.name_bytes(a.0) == name).is_some_and(|a| !self.cx.name_bytes(a.1).contains(&b'\\'))
    }

    /// The class a `self`, `static` or `parent` written as a name stands for: `Some(path)`, or
    /// `None` when the name is an ordinary class name.
    fn relative_class(&self, n: Node<'t>) -> Option<u32> {
        match self.cx.text(n) {
            b"self" | b"static" => Some(self.this_ty()),
            b"parent" => Some(self.classes.last().map_or(NONE, |c| c.sup)),
            _ => None,
        }
    }

    /// The type path a class name node denotes (`self`, `static` and `parent` included).
    fn class_path(&mut self, n: Node<'t>) -> u32 {
        if let Some(p) = self.relative_class(n) {
            return p;
        }
        let q = self.qn(n);
        self.ty_path(q)
    }

    /// Reference of `kind` to the class a name node denotes. `self`, `static` and `parent` emit
    /// nothing as types; instantiated, they name the class.
    fn type_ref(&mut self, n: Node<'t>, kind: RefKind, nargs: u8) -> QN {
        if !matches!(self.k(n), K::Name | K::QualifiedName | K::RelativeName) {
            return QN { name: NONE, module: NONE };
        }
        if let Some(p) = self.relative_class(n) {
            if kind == RefKind::New && p != NONE {
                // The class is written as a type path: its simple name is the reference.
                let b = self.cx.name_bytes(p);
                let (name, module) = match b.iter().position(|&x| x == b'|') {
                    Some(i) => {
                        let (m, s) = (b[..i].to_vec(), b[i + 1..].to_vec());
                        (self.cx.intern(&s), self.cx.intern(&m))
                    }
                    None => (p, NONE),
                };
                self.cx.add_ref(name, kind, line_of(n), module, nargs);
                return QN { name, module };
            }
            return QN { name: NONE, module: NONE };
        }
        let q = self.qn(n);
        if q.name == NONE {
            return q;
        }
        let b = self.cx.name_bytes(q.name);
        if is_reserved_type(b) || (is_stop_type(b) && self.global_class(q)) {
            return QN { name: NONE, module: q.module };
        }
        self.cx.add_ref(q.name, kind, line_of(n), q.module, nargs);
        q
    }

    // ----- types -----

    /// The type path a declared type names: a single class, possibly nullable. Unions of
    /// several classes and primitive types give `NONE`.
    fn decl_type(&mut self, t: Node<'t>) -> u32 {
        match self.k(t) {
            K::NamedType => match t.named_child(0) {
                Some(n) if matches!(self.k(n), K::Name | K::QualifiedName | K::RelativeName) => {
                    let p = self.class_path(n);
                    if p == NONE || is_reserved_type(self.cx.name_bytes(p)) { NONE } else { p }
                }
                _ => NONE,
            },
            K::OptionalType => t.named_child(0).map_or(NONE, |x| self.decl_type(x)),
            K::UnionType => {
                let mut found = NONE;
                for i in 0..t.named_child_count() as u32 {
                    let Some(x) = t.named_child(i) else { break };
                    match self.k(x) {
                        K::PrimitiveType if self.cx.text(x) == b"null" => {}
                        K::NamedType | K::OptionalType if found == NONE => {
                            found = self.decl_type(x);
                            if found == NONE {
                                return NONE;
                            }
                        }
                        _ => return NONE,
                    }
                }
                found
            }
            _ => NONE,
        }
    }

    /// A type written in a docblock (`?Foo`, `Foo|null`, `\A\Foo`, `Collection<int, User>`,
    /// `static`): the class it names, or `NONE`. Arrays and unions of classes give `NONE`.
    fn doc_type(&mut self, raw: &'a [u8]) -> u32 {
        let mut one: Option<&'a [u8]> = None;
        let mut depth = 0u32;
        let mut start = 0usize;
        let mut parts = 0;
        let mut end = raw.len();
        for (i, &b) in raw.iter().enumerate() {
            match b {
                b'<' | b'{' | b'(' => {
                    if depth == 0 && end == raw.len() {
                        end = i;
                    }
                    depth += 1;
                }
                b'>' | b'}' | b')' => depth = depth.saturating_sub(1),
                b'|' if depth == 0 => {
                    let part = &raw[start..end.min(i)];
                    if !matches!(part, b"null" | b"" | b"false" | b"void") {
                        parts += 1;
                        one = Some(part);
                    }
                    start = i + 1;
                    end = raw.len();
                }
                _ => {}
            }
        }
        let last = &raw[start.min(raw.len())..end.min(raw.len())];
        if !matches!(last, b"null" | b"" | b"false" | b"void") {
            parts += 1;
            one = Some(last);
        }
        if parts != 1 {
            return NONE;
        }
        let Some(t) = one else { return NONE };
        let t = t.strip_prefix(b"?").unwrap_or(t);
        if t.is_empty() || t.ends_with(b"]") || !t.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'\\' || b >= 0x80) {
            return if matches!(t, b"$this") { self.this_ty() } else { NONE };
        }
        if t[0].is_ascii_digit() {
            return NONE;
        }
        match t {
            b"static" | b"self" => return self.this_ty(),
            b"parent" => return self.classes.last().map_or(NONE, |c| c.sup),
            _ => {}
        }
        if is_reserved_type(t) || is_doc_pseudo_type(t) {
            return NONE;
        }
        let q = self.resolve_name(t);
        let b = self.cx.name_bytes(q.name);
        if q.name == NONE || (is_stop_type(b) && self.global_class(q)) {
            return NONE;
        }
        self.ty_path(q)
    }

    /// `Type` references for every class a type expression names.
    fn type_refs(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        match self.k(n) {
            K::NamedType => {
                if let Some(x) = n.named_child(0) {
                    self.type_ref(x, RefKind::Type, 255);
                }
            }
            K::OptionalType | K::UnionType | K::IntersectionType | K::DnfType | K::TypeList => self.each(c, |w, c| {
                if !matches!(w.k(c.node()), K::Token | K::Comment) {
                    w.type_refs(c);
                }
            }),
            K::Name | K::QualifiedName | K::RelativeName => {
                self.type_ref(n, RefKind::Type, 255);
            }
            _ => {}
        }
    }

    // ----- namespaces and imports -----

    fn namespace_def(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let body = child(n, t.f_body);
        let nm = child(n, t.f_name).filter(|x| x.end_byte() > x.start_byte());
        let id = match nm {
            Some(x) => {
                let text = self.cx.text(x);
                if text.iter().any(|b| b.is_ascii_whitespace() || *b == b'/' || *b == b'#') {
                    let mut buf: Vec<u8> = Vec::with_capacity(text.len());
                    push_segments(self, x, &mut buf, 0);
                    self.cx.intern(&buf)
                } else {
                    self.cx.intern(text)
                }
            }
            None => NONE,
        };
        if id != NONE {
            let sig_end = match body {
                Some(b) => b.start_byte(),
                None => self.before_semicolon(n),
            };
            self.cx.def(id, DefKind::Namespace, n, sig_end);
        }
        // Imports belong to the namespace they are written in.
        self.aliases.clear();
        match body {
            Some(_) => {
                let saved = self.ns;
                self.ns = id;
                self.each(c, |w, c| {
                    if is_field(c, w.t.f_body) {
                        w.children(c);
                    }
                });
                self.ns = saved;
                self.aliases.clear();
            }
            None => self.ns = id,
        }
    }

    /// `use A\B;`, `use A\B as C, D;`, `use function A\f;`, `use const A\X;`, `use A\{B, C as D,
    /// function f};`.
    fn use_decl(&mut self, n: Node<'t>) {
        let t = self.t;
        let kind = child(n, t.f_type).map_or(UseKind::Class, |x| use_kind(self.cx.text(x)));
        let mut prefix: Option<Node<'t>> = None;
        for i in 0..n.child_count() {
            let Some(ch) = n.child(i) else { break };
            match self.k(ch) {
                K::NamespaceName => prefix = Some(ch),
                K::NamespaceUseClause => self.use_clause(ch, None, kind),
                K::NamespaceUseGroup => {
                    for j in 0..ch.named_child_count() as u32 {
                        let Some(cl) = ch.named_child(j) else { break };
                        if self.k(cl) == K::NamespaceUseClause {
                            self.use_clause(cl, prefix, kind);
                        }
                    }
                }
                K::Error => self.cx.facts.parse_errors += 1,
                _ => {}
            }
        }
    }

    fn use_clause(&mut self, cl: Node<'t>, prefix: Option<Node<'t>>, kind: UseKind) {
        let t = self.t;
        let kind = child(cl, t.f_type).map_or(kind, |x| use_kind(self.cx.text(x)));
        let alias = child(cl, t.f_alias).filter(|x| x.end_byte() > x.start_byte());
        let mut nm = None;
        for i in 0..cl.named_child_count() as u32 {
            let Some(x) = cl.named_child(i) else { break };
            if matches!(self.k(x), K::Name | K::QualifiedName) && alias.is_none_or(|a| a.start_byte() != x.start_byte()) {
                nm = Some(x);
                break;
            }
        }
        let Some(nm) = nm.filter(|x| x.end_byte() > x.start_byte()) else { return };
        // The fully qualified name, without the leading `\`.
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        if let Some(p) = prefix {
            push_clean(self.cx.text(p), &mut buf);
            buf.push(b'\\');
        }
        push_clean(self.cx.text(nm), &mut buf);
        while buf.first() == Some(&b'\\') {
            buf.remove(0);
        }
        let full = self.cx.intern(&buf);
        let (target, name) = match buf.iter().rposition(|&b| b == b'\\') {
            Some(i) => {
                let target = self.cx.intern(&buf[..i]);
                (target, self.cx.intern(&buf[i + 1..]))
            }
            None => (self.global_id, full),
        };
        self.cx.s.buf = buf;
        if self.cx.name_bytes(name).is_empty() {
            return;
        }
        let alias_id = alias.map_or(NONE, |a| self.cx.name(a));
        let alias_id = if alias_id == name { NONE } else { alias_id };
        let sys = if target == self.global_id && kind == UseKind::Class && is_stop_type(self.cx.name_bytes(name)) { iflag::SYSTEM } else { 0 };
        let line = line_of(cl);
        self.cx.add_import(target, name, alias_id, line, sys);
        self.cx.add_ref(name, RefKind::Import, line, target, 255);
        if kind == UseKind::Class {
            self.aliases.push((if alias_id == NONE { name } else { alias_id }, full));
        }
    }

    /// `require 'x.php'`, `include_once __DIR__ . '/x.php'`: a file import when the path is
    /// written out. A path built on `__DIR__` (or `dirname(__FILE__)`) is relative to the file.
    fn include(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let line = line_of(n);
        if let Some(e) = n.named_child(0) {
            let mut buf = std::mem::take(&mut self.cx.s.buf);
            buf.clear();
            let ok = match self.k(e) {
                K::String => self.literal(e).is_some_and(|s| {
                    buf.extend_from_slice(s);
                    true
                }),
                K::Parenthesized => e.named_child(0).filter(|x| self.k(*x) == K::String).and_then(|x| self.literal(x)).is_some_and(|s| {
                    buf.extend_from_slice(s);
                    true
                }),
                K::Binary => {
                    let (l, r) = (child(e, self.t.f_left), child(e, self.t.f_right));
                    let here = l.is_some_and(|l| matches!(self.cx.text(l), b"__DIR__" | b"dirname(__FILE__)"));
                    match (here, r.filter(|x| self.k(*x) == K::String).and_then(|x| self.literal(x))) {
                        (true, Some(s)) if s.starts_with(b"/") => {
                            buf.push(b'.');
                            buf.extend_from_slice(s);
                            true
                        }
                        _ => false,
                    }
                }
                _ => false,
            };
            if ok && !buf.is_empty() && !buf.contains(&b'\n') {
                let target = self.cx.intern(&buf);
                self.cx.add_import(target, NONE, NONE, line, 0);
            }
            self.cx.s.buf = buf;
        }
        self.children(c);
    }

    /// The text of a string literal without escapes or interpolation.
    fn literal(&self, s: Node<'t>) -> Option<&'a [u8]> {
        if self.k(s) != K::String {
            return None;
        }
        match s.named_child_count() {
            0 => Some(b""),
            1 => s.named_child(0).filter(|x| self.k(*x) == K::StringContent).map(|x| self.cx.text(x)),
            _ => None,
        }
    }

    // ----- type declarations -----

    /// A class, interface, trait or enum.
    fn class_decl(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: DefKind, cf: u8) {
        let t = self.t;
        self.doc = None;
        let Some(nm) = child(n, t.f_name).filter(|x| x.end_byte() > x.start_byte()) else { return self.children(c) };
        let body = child(n, t.f_body);
        let sig_end = body.map_or(n.end_byte(), |b| b.start_byte());
        let sig_start = self.sig_start(n);
        let text = self.cx.text(nm);
        let name = self.cx.intern(text);
        let mut flags = dflag::EXPORTED;
        for i in 0..n.child_count().min(16) {
            if let Some(ch) = n.child(i)
                && self.k(ch) == K::AbstractModifier
            {
                flags |= dflag::ABSTRACT;
            }
        }
        let d = self.cx.def(name, kind, n, sig_end);
        self.finish_def(d, NONE, flags, sig_start, sig_end);
        self.cx.push_def(d);
        let test = text.ends_with(b"Test") || text.ends_with(b"Tests") || text.ends_with(b"TestCase");
        let mut cls = Class { own: name, this_ty: name, sup: NONE, sup_q: NONE, flags: cf | if test { CF_TEST } else { 0 } };
        let iface = cf & CF_IFACE != 0;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::BaseClause => {
                    let first = w.heritage(c, RefKind::Extends);
                    if !iface {
                        cls.sup = first;
                    }
                }
                K::InterfaceClause => {
                    w.heritage(c, RefKind::Implements);
                }
                K::DeclList => w.class_body(c, cls),
                _ if ch.start_byte() == nm.start_byte() => {}
                _ => w.visit(c),
            }
        });
        self.cx.pop_def();
    }

    /// `extends A`, `implements A, B`: one reference of `kind` per listed type. Returns the
    /// type path of the first one.
    fn heritage(&mut self, c: &mut TreeCursor<'t>, kind: RefKind) -> u32 {
        let mut first = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Name | K::QualifiedName | K::RelativeName => {
                    let q = w.type_ref(ch, kind, 255);
                    if first == NONE && q.name != NONE {
                        first = w.ty_path(q);
                    }
                }
                K::Token | K::Comment => {}
                _ => w.visit(c),
            }
        });
        first
    }

    /// Enter a class body.
    fn class_body(&mut self, c: &mut TreeCursor<'t>, cls: Class) {
        self.classes.push(cls);
        self.doc = None;
        self.children(c);
        self.classes.pop();
    }

    /// `new class(..) extends B implements I { .. }`. Returns the type of the object.
    fn anon_class(&mut self, c: &mut TreeCursor<'t>) -> u32 {
        let mut sup = NONE;
        let mut ifaces_first = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::Arguments => {
                    w.arguments(c, NONE);
                }
                K::BaseClause => sup = w.heritage(c, RefKind::Extends),
                K::InterfaceClause => ifaces_first = w.heritage(c, RefKind::Implements),
                K::DeclList => {
                    let this_ty = if sup != NONE { sup } else { ifaces_first };
                    w.class_body(c, Class { own: NONE, this_ty, sup, sup_q: NONE, flags: CF_ANON });
                }
                _ => w.visit(c),
            }
        });
        if sup != NONE { sup } else { ifaces_first }
    }

    // ----- members -----

    /// Modifier keywords of a member (and whether an attribute marks it as a test).
    fn modifiers(&self, n: Node<'t>) -> u8 {
        let mut bits = 0u8;
        for i in 0..n.child_count().min(16) {
            let Some(ch) = n.child(i) else { break };
            match self.k(ch) {
                K::VisibilityModifier => match self.cx.text(ch) {
                    b"private" => bits |= M_PRIVATE,
                    b"protected" => bits |= M_PROTECTED,
                    _ => {}
                },
                K::AbstractModifier => bits |= M_ABSTRACT,
                K::AttributeList if self.has_test_attribute(ch) => bits |= M_TEST,
                _ => {}
            }
        }
        bits
    }

    fn has_test_attribute(&self, list: Node<'t>) -> bool {
        for i in 0..list.named_child_count() as u32 {
            let Some(g) = list.named_child(i) else { break };
            for j in 0..g.named_child_count() as u32 {
                let Some(a) = g.named_child(j) else { break };
                if let Some(nm) = a.named_child(0) {
                    let text = self.cx.text(nm);
                    let simple = text.rsplit(|&b| b == b'\\').next().unwrap_or(text);
                    if simple == b"Test" {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Visibility flags of a member of the innermost class.
    fn visibility(&self, bits: u8) -> u8 {
        let cls = self.classes.last();
        if cls.is_some_and(|c| c.flags & CF_IFACE != 0) {
            return dflag::EXPORTED;
        }
        if bits & M_PRIVATE != 0 {
            // A private member of a trait becomes a member of every class that uses it.
            if cls.is_some_and(|c| c.flags & CF_TRAIT != 0) { 0 } else { dflag::LOCAL }
        } else if bits & M_PROTECTED != 0 {
            0
        } else {
            dflag::EXPORTED
        }
    }

    fn method(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let doc = self.doc_of(n);
        let Some(nm) = child(n, t.f_name).filter(|x| x.end_byte() > x.start_byte()) else { return self.children(c) };
        let bits = self.modifiers(n);
        let body = child(n, t.f_body);
        let sig_end = match body {
            Some(b) => b.start_byte(),
            None => self.before_semicolon(n),
        };
        let sig_start = self.sig_start(n);
        let text = self.cx.text(nm);
        let name = self.cx.intern(text);
        let parent = self.cx.encl();
        let cls = self.classes.last().copied();
        let iface = cls.is_some_and(|cl| cl.flags & CF_IFACE != 0);
        let mut flags = self.visibility(bits);
        if body.is_none() {
            flags |= dflag::DECL;
            if bits & M_ABSTRACT != 0 || iface {
                flags |= dflag::ABSTRACT;
            }
        }
        let doc_test = doc.is_some_and(|d| has_tag(d, b"@test"));
        if bits & M_TEST != 0 || doc_test || (text.starts_with(b"test") && cls.is_some_and(|cl| cl.flags & CF_TEST != 0)) {
            flags |= dflag::TEST;
        }
        let d = self.cx.def(name, DefKind::Method, n, sig_end);
        self.finish_def(d, parent, flags, sig_start, sig_end);
        let ret = child(n, t.f_return_type).map_or(NONE, |x| self.decl_type(x));
        let ret = if ret == NONE { doc.and_then(|d| tag_value(d, b"@return")).map_or(NONE, |v| self.doc_type(v)) } else { ret };
        self.cx.def_mut(d).type_name = ret;
        self.callable_body(c, d, parent, doc);
    }

    fn function(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let doc = self.doc_of(n);
        let Some(nm) = child(n, t.f_name).filter(|x| x.end_byte() > x.start_byte()) else { return self.children(c) };
        let body = child(n, t.f_body);
        let sig_end = body.map_or(n.end_byte(), |b| b.start_byte());
        let sig_start = self.sig_start(n);
        let name = self.cx.name(nm);
        // PHP declares a function globally, wherever its declaration sits.
        let d = self.cx.def(name, DefKind::Function, n, sig_end);
        self.finish_def(d, NONE, dflag::EXPORTED, sig_start, sig_end);
        let ret = child(n, t.f_return_type).map_or(NONE, |x| self.decl_type(x));
        let ret = if ret == NONE { doc.and_then(|d| tag_value(d, b"@return")).map_or(NONE, |v| self.doc_type(v)) } else { ret };
        self.cx.def_mut(d).type_name = ret;
        self.callable_body(c, d, NONE, doc);
    }

    /// Parameters and body of a function or method `d` declared in container `owner`.
    fn callable_body(&mut self, c: &mut TreeCursor<'t>, d: u32, owner: u32, doc: Option<&'a [u8]>) {
        let t = self.t;
        self.cx.push_def(d);
        let (vmark, vbase) = (self.vars.len(), self.vbase);
        let (nmark, nbase) = (self.narrowed.len(), self.nbase);
        self.vbase = vmark;
        self.nbase = nmark;
        let saw_yield = std::mem::replace(&mut self.saw_yield, false);
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if is_field(c, t.f_name) => {}
                K::FormalParams => {
                    let (min, max) = w.params(c, owner, doc);
                    let dm = w.cx.def_mut(d);
                    dm.arity_min = min;
                    dm.arity_max = max;
                }
                _ if is_field(c, t.f_body) => {
                    w.doc = None;
                    w.children(c);
                }
                _ => w.visit(c),
            }
        });
        if self.saw_yield {
            self.cx.def_mut(d).flags |= dflag::ASYNC;
        }
        self.saw_yield = saw_yield;
        self.vars.truncate(vmark);
        self.vbase = vbase;
        self.narrowed.truncate(nmark);
        self.nbase = nbase;
        self.cx.pop_def();
    }

    /// Declare the parameters under the cursor. Promoted constructor parameters are also fields
    /// of `owner`. Returns the arity.
    fn params(&mut self, c: &mut TreeCursor<'t>, owner: u32, doc: Option<&'a [u8]>) -> (u8, u8) {
        let t = self.t;
        let mut required = 0u32;
        let mut total = 0u32;
        let mut variadic = false;
        self.each(c, |w, c| {
            let p = c.node();
            let k = w.k(p);
            if !matches!(k, K::SimpleParam | K::VariadicParam | K::PromotionParam) {
                if k != K::Token && k != K::Comment {
                    w.visit(c);
                }
                return;
            }
            total += 1;
            let has_default = child(p, t.f_default_value).is_some();
            if k == K::VariadicParam {
                variadic = true;
            } else if !has_default {
                required += 1;
            }
            let nm_node = child(p, t.f_name);
            let name = nm_node.and_then(|x| w.var_name(x));
            let mut ty = child(p, t.f_type).map_or(NONE, |x| w.decl_type(x));
            if ty == NONE
                && let (Some(d), Some(nm)) = (doc, nm_node)
            {
                ty = param_doc_type(d, w.cx.text(nm)).map_or(NONE, |v| w.doc_type(v));
            }
            let field_def = if k == K::PromotionParam && owner != NONE {
                name.map(|nmid| {
                    let vis = child(p, t.f_visibility).map_or(0, |v| match w.cx.text(v) {
                        b"private" => M_PRIVATE,
                        b"protected" => M_PROTECTED,
                        _ => 0,
                    });
                    let sig_end = match child(p, t.f_default_value) {
                        Some(v) => w.before_initializer(p.start_byte(), v.start_byte()),
                        None => p.end_byte(),
                    };
                    let sig_start = w.sig_start(p);
                    let flags = w.visibility(vis);
                    let d = w.cx.def(nmid, DefKind::Field, p, sig_end);
                    w.finish_def(d, owner, flags, sig_start, sig_end);
                    w.cx.def_mut(d).type_name = ty;
                    d
                })
            } else {
                None
            };
            if let Some(d) = field_def {
                w.cx.push_def(d);
            }
            w.each(c, |w, c| {
                let ch = c.node();
                match w.k(ch) {
                    K::Token | K::Comment | K::VariableName => {}
                    _ => w.visit(c),
                }
            });
            if field_def.is_some() {
                w.cx.pop_def();
            }
            if let Some(nmid) = name {
                w.set_var(nmid, if ty == NONE { UNSET } else { ty });
            }
        });
        let total = total.min(254) as u8;
        let required = required.min(254) as u8;
        (required, if variadic { 255 } else { total })
    }

    /// `public ?Foo $a = null, $b;` and the PHP 8.4 hooks of a property.
    fn property_decl(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let doc = self.doc_of(n);
        let bits = self.modifiers(n);
        let flags = self.visibility(bits);
        let parent = self.cx.encl();
        let sig_start = self.sig_start(n);
        let ty_node = child(n, t.f_type);
        let mut ty = ty_node.map_or(NONE, |x| self.decl_type(x));
        let mut callable = ty_node.is_some_and(|x| matches!(self.cx.text(x), b"Closure" | b"?Closure" | b"\\Closure" | b"?\\Closure"));
        if ty == NONE
            && ty_node.is_none()
            && let Some(v) = doc.and_then(|d| tag_value(d, b"@var"))
        {
            callable = matches!(v, b"callable" | b"Closure" | b"\\Closure" | b"?callable" | b"?Closure" | b"callable|null" | b"Closure|null");
            ty = self.doc_type(v);
        }
        if callable {
            ty = NONE;
        }
        let flags = if callable { flags | dflag::FNPTR } else { flags };
        let field_def = |w: &mut Self, el: Node<'t>| -> u32 {
            let Some(nm) = child(el, t.f_name).and_then(|x| w.var_name(x)) else { return NONE };
            let sig_end = match child(el, t.f_default_value) {
                Some(v) => w.before_initializer(sig_start, v.start_byte()),
                None => el.end_byte(),
            };
            let d = w.cx.def(nm, DefKind::Field, n, sig_end);
            w.finish_def(d, parent, flags, sig_start, sig_end);
            w.cx.def_mut(d).type_name = ty;
            d
        };
        // The first field exists before the walk: the modifiers and the type belong to it.
        let first = (0..n.named_child_count() as u32).filter_map(|i| n.named_child(i)).find(|x| self.k(*x) == K::PropertyElement);
        let mut cur = first.map_or(NONE, |el| field_def(self, el));
        let first_at = first.map_or(usize::MAX, |el| el.start_byte());
        if cur != NONE {
            self.cx.push_def(cur);
        }
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::PropertyElement => {
                    if ch.start_byte() != first_at {
                        let d = field_def(w, ch);
                        if d == NONE {
                            return w.children(c);
                        }
                        if cur != NONE {
                            w.cx.pop_def();
                        }
                        cur = d;
                        w.cx.push_def(d);
                    }
                    w.children_except(c, t.f_name);
                }
                K::PropertyHookList => w.each(c, |w, c| {
                    if w.k(c.node()) == K::PropertyHook {
                        w.property_hook(c);
                    }
                }),
                _ => w.visit(c),
            }
        });
        if cur != NONE {
            self.cx.pop_def();
        }
    }

    /// `get => ..` / `set(T $value) { .. }`: a body of its own, with `$value` for `set`.
    fn property_hook(&mut self, c: &mut TreeCursor<'t>) {
        let (vmark, vbase) = (self.vars.len(), self.vbase);
        self.vbase = vmark;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment | K::Name => {}
                K::FormalParams => {
                    w.params(c, NONE, None);
                }
                _ => w.visit(c),
            }
        });
        self.vars.truncate(vmark);
        self.vbase = vbase;
    }

    /// `const A = 1, B = 2;`: class constants are fields of their class, top-level ones global
    /// constants.
    fn const_decl(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        self.doc = None;
        let in_class = !self.classes.is_empty();
        let bits = self.modifiers(n);
        let (kind, flags, parent) = if in_class { (DefKind::Field, self.visibility(bits), self.cx.encl()) } else { (DefKind::Constant, dflag::EXPORTED, NONE) };
        let sig_start = self.sig_start(n);
        let ty = child(n, t.f_type).map_or(NONE, |x| self.decl_type(x));
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::ConstElement => {
                    let mut nm = None;
                    let mut val = None;
                    for i in 0..ch.named_child_count() as u32 {
                        let Some(x) = ch.named_child(i) else { break };
                        if w.k(x) == K::Comment {
                            continue;
                        }
                        if nm.is_none() && w.k(x) == K::Name {
                            nm = Some(x);
                        } else if val.is_none() {
                            val = Some(x);
                        }
                    }
                    let Some(nm) = nm.filter(|x| x.end_byte() > x.start_byte()) else { return w.children(c) };
                    let name = w.cx.name(nm);
                    let sig_end = match val {
                        Some(v) => w.before_initializer(sig_start, v.start_byte()),
                        None => ch.end_byte(),
                    };
                    let d = w.cx.def(name, kind, n, sig_end);
                    w.finish_def(d, parent, flags, sig_start, sig_end);
                    w.cx.def_mut(d).type_name = ty;
                    w.cx.push_def(d);
                    let nm_at = nm.start_byte();
                    w.each(c, |w, c| {
                        if c.node().start_byte() != nm_at {
                            w.visit(c);
                        }
                    });
                    w.cx.pop_def();
                }
                _ => w.visit(c),
            }
        });
    }

    fn enum_case(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        self.doc = None;
        let Some(nm) = child(n, t.f_name).filter(|x| x.end_byte() > x.start_byte()) else { return self.children(c) };
        let name = self.cx.name(nm);
        let parent = self.cx.encl();
        let own = self.classes.last().map_or(NONE, |cl| cl.own);
        let sig_start = self.sig_start(n);
        let sig_end = match child(n, t.f_value) {
            Some(v) => self.before_initializer(sig_start, v.start_byte()),
            None => self.before_semicolon(n),
        };
        let d = self.cx.def(name, DefKind::EnumMember, n, sig_end);
        self.finish_def(d, parent, dflag::EXPORTED, sig_start, sig_end);
        self.cx.def_mut(d).type_name = own;
        self.cx.push_def(d);
        self.children_except(c, t.f_name);
        self.cx.pop_def();
    }

    /// `use A, B { A::m insteadof B; B::m as n; }` inside a class: the class takes the members
    /// of the traits (see the module comment for why this is `Extends`).
    fn trait_use(&mut self, c: &mut TreeCursor<'t>) {
        self.doc = None;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Name | K::QualifiedName | K::RelativeName => {
                    w.type_ref(ch, RefKind::Extends, 255);
                }
                K::UseList => w.each(c, |w, c| {
                    let cl = c.node();
                    match w.k(cl) {
                        // `T::m insteadof U`: `U` names a trait; `T::m as [visibility] n`: `n` is
                        // the new name of the method, not a reference.
                        K::UseInsteadOf | K::UseAs => {
                            let insteadof = w.k(cl) == K::UseInsteadOf;
                            w.each(c, |w, c| {
                                let x = c.node();
                                match w.k(x) {
                                    K::ClassConstantAccess => {
                                        w.class_const(x, c);
                                    }
                                    K::Name | K::QualifiedName if insteadof => {
                                        w.type_ref(x, RefKind::Type, 255);
                                    }
                                    _ => {}
                                }
                            });
                        }
                        _ => {}
                    }
                }),
                _ => {}
            }
        });
    }

    /// `#[Attr(args)]`.
    fn attribute(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let mut first = true;
        let line = line_of(n);
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Name | K::QualifiedName | K::RelativeName if first => {
                    first = false;
                    let q = w.qn(ch);
                    if q.name != NONE && !(is_stop_attribute(w.cx.name_bytes(q.name)) && w.global_class(q)) {
                        w.cx.add_ref(q.name, RefKind::Annotation, line, q.module, 255);
                    }
                }
                K::Arguments => {
                    w.arguments(c, NONE);
                }
                K::Token | K::Comment => {}
                _ => w.visit(c),
            }
        });
    }

    // ----- expressions -----

    /// A bare or qualified name used as a value: a global constant.
    fn constant(&mut self, n: Node<'t>) {
        let q = self.qn(n);
        if q.name == NONE {
            return;
        }
        if q.module == NONE && is_stop_const(self.cx.name_bytes(q.name)) {
            return;
        }
        self.cx.add_ref(q.name, RefKind::Read, line_of(n), q.module, 255);
    }

    /// Walk the expression under the cursor as a receiver: emit its references and return what
    /// it denotes.
    fn recv(&mut self, c: &mut TreeCursor<'t>) -> Recv {
        let n = c.node();
        match self.k(n) {
            K::VariableName => {
                let text = self.cx.text(n);
                if text == b"$this" {
                    return Recv::val(self.this_ty());
                }
                match self.var_name(n).and_then(|v| self.var(v)) {
                    Some(t) => Recv::val(t),
                    None => Recv::UNKNOWN,
                }
            }
            K::MemberAccess => self.member_access(n, c, false),
            K::MemberCall => self.member_call(n, c),
            K::ScopedCall => self.scoped_call(n, c),
            K::ScopedPropertyAccess => self.scoped_prop(n, c, false),
            K::ClassConstantAccess => self.class_const(n, c),
            K::FunctionCall => self.function_call(n, c),
            K::ObjectCreation => Recv::val(self.object_creation(n, c)),
            K::Parenthesized | K::Clone => {
                let mut r = Recv::UNKNOWN;
                self.each(c, |w, c| {
                    if !matches!(w.k(c.node()), K::Token | K::Comment) {
                        r = w.recv(c);
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

    /// The receiver at the start of a scoped expression (`T::`, `self::`, `parent::`, `$x::`).
    /// A class written by name is also a `Type` reference.
    fn scope_recv(&mut self, c: &mut TreeCursor<'t>) -> Recv {
        let n = c.node();
        match self.k(n) {
            K::RelativeScope => match self.cx.text(n) {
                b"parent" => Recv { k: RK::Parent, base: NONE, fld: NONE, call: false },
                _ => Recv { k: RK::Type, base: self.this_ty(), fld: NONE, call: false },
            },
            K::Name | K::QualifiedName | K::RelativeName => {
                if let Some(p) = self.relative_class(n) {
                    return Recv { k: RK::Type, base: p, fld: NONE, call: false };
                }
                let q = self.type_ref(n, RefKind::Type, 255);
                if q.name == NONE {
                    // A builtin class (`DateTime::createFromFormat`): nothing of this repository.
                    return Recv { k: RK::Type, base: NONE, fld: NONE, call: false };
                }
                let p = self.ty_path(q);
                Recv { k: RK::Type, base: p, fld: NONE, call: false }
            }
            _ => self.recv(c),
        }
    }

    /// Does the argument list hold only `...` (a first-class callable, `f(...)`)?
    fn is_fcc(&self, args: Option<Node<'t>>) -> bool {
        args.is_some_and(|a| a.named_child_count() == 1 && a.named_child(0).is_some_and(|x| self.k(x) == K::VariadicPlaceholder))
    }

    /// `f(..)`, `A\f(..)`, `$fn(..)`, `define('X', ..)`, `f(...)`.
    fn function_call(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) -> Recv {
        let t = self.t;
        let f = child(n, t.f_function);
        let args = child(n, t.f_arguments);
        let Some(f) = f.filter(|x| matches!(self.k(*x), K::Name | K::QualifiedName | K::RelativeName)) else {
            // A call through a variable or an expression: nothing is named.
            self.each(c, |w, c| {
                let ch = c.node();
                match w.k(ch) {
                    K::Arguments => {
                        w.arguments(c, NONE);
                    }
                    K::Token | K::Comment => {}
                    _ => {
                        w.recv(c);
                    }
                }
            });
            return Recv::UNKNOWN;
        };
        let q = self.qn(f);
        if q.name == NONE {
            self.children_except(c, t.f_function);
            return Recv::UNKNOWN;
        }
        let text = self.cx.name_bytes(q.name);
        let bare = q.module == NONE || q.module == self.global_id;
        if bare && text == b"define" {
            self.define(n, c);
            return Recv::UNKNOWN;
        }
        let stop = bare && is_stop_fn(text);
        let fcc = self.is_fcc(args);
        let mut nargs = 255u8;
        // A builtin gets no reference of its own, but the callables handed to it are still its
        // arguments (`array_map([$this, 'm'], ..)`).
        self.each(c, |w, c| {
            if is_field(c, t.f_arguments) {
                nargs = w.arguments(c, q.name);
            }
        });
        if stop {
            return Recv::UNKNOWN;
        }
        let line = line_of(f);
        if fcc {
            self.cx.add_ref(q.name, RefKind::Read, line, q.module, 255);
            self.last_callable = q.name;
            return Recv::UNKNOWN;
        }
        self.cx.add_ref(q.name, RefKind::Call, line, q.module, nargs);
        Recv::val(self.fn_result(q))
    }

    /// `define('NAME', value)`: a global constant.
    fn define(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let name_lit = child(n, t.f_arguments)
            .and_then(|a| a.named_child(0))
            .filter(|a| self.k(*a) == K::Argument)
            .and_then(|a| a.named_child(0))
            .and_then(|s| self.literal(s))
            .filter(|s| !s.is_empty() && s.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'\\'));
        let d = name_lit.map(|s| {
            // `define('A\B', ..)` defines `B` in namespace `A`; the namespace is not recorded.
            let simple = s.rsplit(|&b| b == b'\\').next().unwrap_or(s);
            let name = self.cx.intern(simple);
            let d = self.cx.def(name, DefKind::Constant, n, n.end_byte());
            self.finish_def(d, NONE, dflag::EXPORTED, n.start_byte(), n.end_byte());
            d
        });
        if let Some(d) = d {
            self.cx.push_def(d);
        }
        self.each(c, |w, c| {
            if is_field(c, t.f_arguments) {
                w.arguments(c, NONE);
            }
        });
        if d.is_some() {
            self.cx.pop_def();
        }
    }

    /// `$x->m(..)`, `$x?->m(..)`, `$x->m(...)`. Returns the declared result of the method on
    /// the receiver's type.
    fn member_call(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) -> Recv {
        let t = self.t;
        let nm = child(n, t.f_name).filter(|x| self.k(*x) == K::Name && x.end_byte() > x.start_byte());
        let name = nm.map_or(NONE, |x| self.cx.name(x));
        let fcc = self.is_fcc(child(n, t.f_arguments));
        let mut r = Recv::UNKNOWN;
        let mut nargs = 255u8;
        self.each(c, |w, c| {
            let ch = c.node();
            if is_field(c, t.f_object) {
                r = w.recv(c);
            } else if is_field(c, t.f_arguments) {
                nargs = w.arguments(c, name);
            } else if is_field(c, t.f_name) {
                // `$x->$m()`, `$x->{$m}()`: a dynamic name.
                if nm.is_none() && !matches!(w.k(ch), K::VariableName) {
                    w.visit(c);
                }
            }
        });
        if name == NONE {
            return Recv::UNKNOWN;
        }
        let q = self.qual(r);
        let line = line_of(nm.unwrap_or(n));
        if fcc {
            self.cx.add_ref(name, RefKind::MemberRead, line, q, 255);
            self.last_callable = name;
            return Recv::UNKNOWN;
        }
        self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
        Recv::member(q, name, true)
    }

    /// `T::m(..)`, `self::m(..)`, `static::m(..)`, `parent::m(..)`, `$x::m(..)`.
    fn scoped_call(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) -> Recv {
        let t = self.t;
        let nm = child(n, t.f_name).filter(|x| self.k(*x) == K::Name && x.end_byte() > x.start_byte());
        let name = nm.map_or(NONE, |x| self.cx.name(x));
        let fcc = self.is_fcc(child(n, t.f_arguments));
        let mut r = Recv::UNKNOWN;
        let mut nargs = 255u8;
        self.each(c, |w, c| {
            let ch = c.node();
            if is_field(c, t.f_scope) {
                r = w.scope_recv(c);
            } else if is_field(c, t.f_arguments) {
                nargs = w.arguments(c, name);
            } else if is_field(c, t.f_name) && nm.is_none() && !matches!(w.k(ch), K::VariableName) {
                w.visit(c);
            }
        });
        if name == NONE {
            return Recv::UNKNOWN;
        }
        let line = line_of(nm.unwrap_or(n));
        let q = self.qual(r);
        if r.k == RK::Type && q == NONE {
            return Recv::UNKNOWN;
        }
        if fcc {
            self.cx.add_ref(name, RefKind::MemberRead, line, q, 255);
            self.last_callable = name;
            return Recv::UNKNOWN;
        }
        match r.k {
            // `T::m()`, `self::m()`: a static call, or a call on `$this` written through the class.
            RK::Type => {
                self.cx.add_ref(name, RefKind::Call, line, q, nargs);
                Recv::member(q, name, true)
            }
            RK::Parent => {
                self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
                // The result is declared by an ancestor; the lookup starts at the class itself
                // (an override keeps the result type) and walks up from there.
                Recv::member(self.this_ty(), name, true)
            }
            RK::Val => {
                self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
                Recv::member(q, name, true)
            }
        }
    }

    /// `$x->p`, `$x?->p`. Emits the member reference and returns what the access denotes.
    fn member_access(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, write: bool) -> Recv {
        let t = self.t;
        let nm = child(n, t.f_name).filter(|x| self.k(*x) == K::Name && x.end_byte() > x.start_byte());
        let mut r = Recv::UNKNOWN;
        self.each(c, |w, c| {
            let ch = c.node();
            if is_field(c, t.f_object) {
                r = w.recv(c);
            } else if is_field(c, t.f_name) && nm.is_none() && !matches!(w.k(ch), K::VariableName) {
                w.visit(c);
            }
        });
        let Some(nm) = nm else { return Recv::UNKNOWN };
        let name = self.cx.name(nm);
        let q = self.qual(r);
        let kind = if write { RefKind::MemberWrite } else { RefKind::MemberRead };
        self.cx.add_ref(name, kind, line_of(nm), q, 255);
        Recv::member(q, name, false)
    }

    /// `T::$p`, `self::$p`, `parent::$p`.
    fn scoped_prop(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, write: bool) -> Recv {
        let t = self.t;
        let nm = child(n, t.f_name).filter(|x| self.k(*x) == K::VariableName);
        let mut r = Recv::UNKNOWN;
        self.each(c, |w, c| {
            if is_field(c, t.f_scope) {
                r = w.scope_recv(c);
            } else if is_field(c, t.f_name) && nm.is_none() {
                w.visit(c);
            }
        });
        let Some(name) = nm.and_then(|x| self.var_name(x)) else { return Recv::UNKNOWN };
        let q = self.qual(r);
        if r.k == RK::Type && q == NONE {
            return Recv::UNKNOWN;
        }
        let kind = if write { RefKind::MemberWrite } else { RefKind::MemberRead };
        self.cx.add_ref(name, kind, line_of(n), q, 255);
        if r.k == RK::Parent {
            return Recv::member(self.this_ty(), name, false);
        }
        Recv::member(q, name, false)
    }

    /// `T::CONST`, `self::CONST`, `T::Case`, `T::class`. A constant used as a receiver is an
    /// enum case: an object of the enum itself.
    fn class_const(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) -> Recv {
        let last = n.named_child((n.named_child_count() as u32).wrapping_sub(1));
        let nm = last.filter(|x| self.k(*x) == K::Name && x.end_byte() > x.start_byte() && n.named_child_count() >= 2);
        let nm_at = nm.map_or(usize::MAX, |x| x.start_byte());
        let mut r = Recv::UNKNOWN;
        let mut first = true;
        let class_lit = nm.is_some_and(|x| self.cx.text(x) == b"class");
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if ch.start_byte() == nm_at => {}
                _ if first => {
                    first = false;
                    if class_lit && matches!(w.k(ch), K::VariableName) {
                        return;
                    }
                    r = w.scope_recv(c);
                }
                _ => w.visit(c),
            }
        });
        let Some(nm) = nm else { return Recv::UNKNOWN };
        if class_lit {
            // `T::class` is the name of the class as a string; the `Type` reference is all.
            return Recv::UNKNOWN;
        }
        let q = self.qual(r);
        if r.k == RK::Type && q == NONE {
            return Recv::UNKNOWN;
        }
        let name = self.cx.name(nm);
        self.cx.add_ref(name, RefKind::MemberRead, line_of(nm), q, 255);
        match r.k {
            RK::Type => Recv::val(q),
            RK::Parent => Recv::val(self.classes.last().map_or(NONE, |c| c.sup)),
            RK::Val => Recv::UNKNOWN,
        }
    }

    /// `new T(..)`, `new self`, `new class(..) {..}`, `new $cls`. Returns the type path of the
    /// object.
    fn object_creation(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) -> u32 {
        let mut cls: Option<Node<'t>> = None;
        let mut ty = NONE;
        let mut nargs = 255u8;
        let mut has_args = false;
        let mut callee = NONE;
        // The class comes first; its simple name is the callee of the arguments.
        for i in 0..n.named_child_count() as u32 {
            let Some(x) = n.named_child(i) else { break };
            if matches!(self.k(x), K::Name | K::QualifiedName | K::RelativeName) {
                cls = Some(x);
                break;
            }
        }
        if let Some(x) = cls {
            ty = self.class_path(x);
            callee = match self.relative_class(x) {
                Some(_) => NONE,
                None => self.qn(x).name,
            };
        }
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                K::Arguments => {
                    has_args = true;
                    nargs = w.arguments(c, callee);
                }
                K::AnonClass => ty = w.anon_class(c),
                _ if cls.is_some_and(|x| x.start_byte() == ch.start_byte()) => {}
                _ => {
                    w.recv(c);
                }
            }
        });
        if let Some(x) = cls {
            self.type_ref(x, RefKind::New, if has_args { nargs } else { 0 });
        }
        ty
    }

    /// Walk an argument list of a call of `callee` and count the arguments. A spread argument
    /// makes the count unknown. A callable handed over directly is also an `Arg` of `callee`.
    fn arguments(&mut self, c: &mut TreeCursor<'t>, callee: u32) -> u8 {
        let mut n = 0u32;
        let mut unknown = false;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment | K::VariadicPlaceholder => {}
                K::Argument => {
                    n += 1;
                    if w.argument(c, callee) {
                        unknown = true;
                    }
                }
                K::Error => {
                    unknown = true;
                    w.visit(c);
                }
                _ => {
                    n += 1;
                    w.visit(c);
                }
            }
        });
        if unknown { 255 } else { n.min(254) as u8 }
    }

    /// One argument. Returns true for a spread (`...$args`).
    fn argument(&mut self, c: &mut TreeCursor<'t>, callee: u32) -> bool {
        let t = self.t;
        let mut spread = false;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                // a named argument's label
                _ if is_field(c, t.f_name) => {}
                K::VariadicUnpacking => {
                    spread = true;
                    w.children(c);
                }
                k => {
                    w.last_callable = NONE;
                    let callable_shape = matches!(k, K::ArrayCreation | K::FunctionCall | K::MemberCall | K::ScopedCall);
                    w.visit(c);
                    let m = std::mem::replace(&mut w.last_callable, NONE);
                    if callable_shape && m != NONE && callee != NONE {
                        w.cx.add_ref(m, RefKind::Arg, line_of(ch), callee, 255);
                    }
                }
            }
        });
        spread
    }

    /// An array literal. `[$this, 'm']`, `[self::class, 'm']` and `[T::class, 'm']` name a
    /// method: a `MemberRead` of it, remembered in `last_callable`.
    fn array(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        if n.named_child_count() == 2
            && let (Some(a), Some(b)) = (n.named_child(0), n.named_child(1))
            && self.k(a) == K::ArrayElement
            && self.k(b) == K::ArrayElement
            && a.named_child_count() == 1
            && b.named_child_count() == 1
            && let (Some(obj), Some(m)) = (a.named_child(0), b.named_child(0))
            && let Some(method) = self.literal(m).filter(|s| is_ident(s))
        {
            let q = match self.k(obj) {
                K::VariableName if self.cx.text(obj) == b"$this" => Some(self.this_ty()),
                K::ClassConstantAccess if obj.named_child_count() == 2 && obj.named_child(1).is_some_and(|x| self.cx.text(x) == b"class") => {
                    obj.named_child(0).and_then(|s| match self.k(s) {
                        K::RelativeScope => (self.cx.text(s) != b"parent").then(|| self.this_ty()),
                        K::Name | K::QualifiedName | K::RelativeName => {
                            let q = self.type_ref(s, RefKind::Type, 255);
                            (q.name != NONE).then(|| self.ty_path(q))
                        }
                        _ => None,
                    })
                }
                _ => None,
            };
            if let Some(q) = q {
                let name = self.cx.intern(method);
                self.cx.add_ref(name, RefKind::MemberRead, line_of(m), q, 255);
                self.last_callable = name;
                return;
            }
        }
        self.children(c);
    }

    /// `$x = v`, `$this->p = v`, `[$a, $b] = v`, `$x = &$y`. The value is walked first: it is
    /// evaluated before the target is assigned, and it types a variable target.
    fn assignment(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let right = child(n, t.f_right);
        let null_value = right.is_some_and(|r| self.k(r) == K::Null);
        let mut ty = NONE;
        let mut bound = NONE;
        self.each(c, |w, c| {
            if is_field(c, t.f_right) {
                w.last_callable = NONE;
                let k = w.k(c.node());
                let callable_shape = matches!(k, K::ArrayCreation | K::FunctionCall | K::MemberCall | K::ScopedCall);
                let r = w.recv(c);
                if callable_shape {
                    bound = std::mem::replace(&mut w.last_callable, NONE);
                }
                ty = w.qual(r);
                if r.k != RK::Val {
                    ty = NONE;
                }
            }
        });
        self.each(c, |w, c| {
            if !is_field(c, t.f_left) {
                return;
            }
            let ch = c.node();
            match w.k(ch) {
                K::VariableName => {
                    if let Some(v) = w.var_name(ch) {
                        w.assign_var(v, if null_value { UNSET } else { ty });
                    }
                }
                _ => {
                    let from = w.cx.facts.refs.len();
                    w.lvalue(c);
                    if bound != NONE
                        && let Some(slot) = w.cx.facts.refs.get(from..).and_then(|rs| rs.last()).filter(|r| r.kind == RefKind::MemberWrite).copied()
                    {
                        // A method stored into a member: a binding into the slot `Owner.member`.
                        let s = w.joined(slot.qual, b".", slot.name);
                        w.cx.add_ref(bound, RefKind::Bind, slot.line, s, 255);
                    }
                }
            }
        });
    }

    /// `$x += 1`, `$this->n++`: the target is written.
    fn update(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let mut first = true;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if is_field(c, t.f_operator) => {}
                _ if std::mem::take(&mut first) => w.lvalue(c),
                _ => w.visit(c),
            }
        });
    }

    /// An assignment target. Storing into an element of a member counts as a write of the
    /// member, as in the C extractor.
    fn lvalue(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        match self.k(n) {
            K::VariableName => {
                if let Some(v) = self.var_name(n) {
                    self.assign_var(v, NONE);
                }
            }
            K::MemberAccess => {
                self.member_access(n, c, true);
            }
            K::ScopedPropertyAccess => {
                self.scoped_prop(n, c, true);
            }
            K::Subscript => {
                let mut first = true;
                self.each(c, |w, c| {
                    let k = w.k(c.node());
                    if k == K::Token || k == K::Comment {
                        return;
                    }
                    if std::mem::take(&mut first) {
                        if k == K::VariableName {
                            return;
                        }
                        w.lvalue(c)
                    } else {
                        w.visit(c)
                    }
                });
            }
            K::ListLiteral | K::ArrayCreation => self.declare_targets(c),
            K::ByRef => self.declare_targets(c),
            _ => self.visit(c),
        }
    }

    /// Variables bound by a destructuring target, `global` or `foreach` (`[$a, 'k' => $b]`,
    /// `list($a, $b)`, `&$v`): declared without a type. Other targets are written.
    fn declare_targets(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        match self.k(n) {
            K::VariableName => {
                if let Some(v) = self.var_name(n) {
                    self.assign_var(v, NONE);
                }
            }
            K::ListLiteral | K::ArrayCreation | K::ArrayElement | K::ByRef | K::GlobalDecl => self.each(c, |w, c| {
                if !matches!(w.k(c.node()), K::Token | K::Comment) {
                    w.declare_targets(c);
                }
            }),
            K::Pair => {
                // `'key' => $v`: the key is a value, the rest a target.
                let mut first = true;
                self.each(c, |w, c| {
                    if matches!(w.k(c.node()), K::Token | K::Comment) {
                        return;
                    }
                    if std::mem::take(&mut first) { w.visit(c) } else { w.declare_targets(c) }
                });
            }
            _ => self.lvalue(c),
        }
    }

    /// `a . b . c ...` nests to the left, one level per operand. Long concatenations would run
    /// into the depth guard, so the left spine is walked with a loop instead of recursion. The
    /// right operand of `instanceof` is a class.
    fn binary(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        if self.is_instanceof(c.node()) {
            return self.instanceof(c);
        }
        let mut levels = 0u32;
        while self.k(c.node()) == K::Binary && !self.is_instanceof(c.node()) && c.goto_first_child() {
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

    fn is_instanceof(&self, n: Node<'t>) -> bool {
        child(n, self.t.f_operator).is_some_and(|o| o.kind_id() == self.t.kw_instanceof)
    }

    /// `$x instanceof T`. The variable takes `T` from here on: the test nearly always guards the
    /// code that uses it as a `T` (`if ($v instanceof T) $v->m()`, `if (! $v instanceof T)
    /// return;`). A declared type or a constructed one is kept: it holds after the test too. A
    /// negated test only types a variable that has no type yet, so that in
    /// `$v instanceof A && ! $v instanceof B` the variable stays an `A`.
    fn instanceof(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let n = c.node();
        let var = child(n, t.f_left).and_then(|l| self.var_name(l));
        let negated = std::mem::replace(&mut self.negated, false);
        let mut ty = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            if is_field(c, t.f_right) && matches!(w.k(ch), K::Name | K::QualifiedName | K::RelativeName) {
                ty = match w.relative_class(ch) {
                    Some(p) => p,
                    None => {
                        let q = w.type_ref(ch, RefKind::Type, 255);
                        if q.name != NONE { w.ty_path(q) } else { NONE }
                    }
                };
            } else if !matches!(w.k(ch), K::Token | K::Comment) && !is_field(c, t.f_operator) {
                w.visit(c);
            }
        });
        if let Some(v) = var
            && ty != NONE
        {
            let narrowed = self.narrowed.get(self.nbase..).is_some_and(|ns| ns.contains(&v));
            let cur = self.var(v);
            let weak = cur.is_none_or(|x| x == NONE || self.cx.name_bytes(x).ends_with(b"()"));
            if (!negated && (weak || narrowed)) || (!narrowed && cur.is_none_or(|x| x == NONE)) {
                self.set_var(v, ty);
                if !narrowed {
                    self.narrowed.push(v);
                }
            }
        }
    }

    /// `function (..) use ($a, &$b) { .. }`: a body of its own that sees only its parameters
    /// and the variables its `use` clause brings in.
    fn closure(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let (vmark, vbase) = (self.vars.len(), self.vbase);
        // The types of the captured variables, read before the scope changes.
        for i in 0..n.named_child_count() as u32 {
            let Some(u) = n.named_child(i) else { break };
            if self.k(u) != K::AnonUseClause {
                continue;
            }
            for j in 0..u.named_child_count() as u32 {
                let Some(mut v) = u.named_child(j) else { break };
                if self.k(v) == K::ByRef
                    && let Some(x) = v.named_child(0)
                {
                    v = x;
                }
                if let Some(name) = self.var_name(v) {
                    let ty = self.var(name).unwrap_or(NONE);
                    self.vars.push((name, if ty == NONE { UNSET } else { ty }));
                }
            }
        }
        self.vbase = vmark;
        let (nmark, nbase) = (self.narrowed.len(), self.nbase);
        self.nbase = nmark;
        let saw_yield = std::mem::replace(&mut self.saw_yield, false);
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment | K::AnonUseClause | K::StaticModifier => {}
                K::FormalParams => {
                    w.params(c, NONE, None);
                }
                _ => w.visit(c),
            }
        });
        self.saw_yield = saw_yield;
        self.vars.truncate(vmark);
        self.vbase = vbase;
        self.narrowed.truncate(nmark);
        self.nbase = nbase;
    }

    /// `fn (..) => expr` captures the enclosing variables by value.
    fn arrow(&mut self, _n: Node<'t>, c: &mut TreeCursor<'t>) {
        let vmark = self.vars.len();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment | K::StaticModifier => {}
                K::FormalParams => {
                    w.params(c, NONE, None);
                }
                _ => w.visit(c),
            }
        });
        self.vars.truncate(vmark);
    }

    /// `foreach ($xs as $k => $v)`: the iterated value, then the bindings, then the body.
    fn foreach(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        if let Some(d) = self.doc_of(n) {
            self.inline_var_doc(d);
        }
        let mut idx = 0;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Token | K::Comment => {}
                _ if is_field(c, t.f_body) => w.visit(c),
                _ => {
                    idx += 1;
                    if idx == 1 {
                        w.visit(c);
                    } else {
                        w.declare_targets(c);
                    }
                }
            }
        });
    }

    /// `catch (A | B $e)`: the types are references; `$e` has the type when there is one.
    fn catch_clause(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let ty_node = child(n, t.f_type);
        let ty = match ty_node {
            Some(tl) if tl.named_child_count() == 1 => tl.named_child(0).map_or(NONE, |x| self.decl_type(x)),
            _ => NONE,
        };
        self.each(c, |w, c| {
            let ch = c.node();
            if is_field(c, t.f_name) {
                if let Some(v) = w.var_name(ch) {
                    w.assign_var(v, ty);
                }
            } else if !matches!(w.k(ch), K::Token | K::Comment) {
                w.visit(c);
            }
        });
    }

    /// `static $x = v;` in a function.
    fn static_var(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let mut ty = NONE;
        self.each(c, |w, c| {
            if is_field(c, t.f_value) {
                let r = w.recv(c);
                ty = if r.k == RK::Val { w.qual(r) } else { NONE };
            }
        });
        if let Some(v) = child(n, t.f_name).and_then(|x| self.var_name(x)) {
            self.assign_var(v, ty);
        }
    }

    /// An expression statement, possibly typed by an inline `/** @var T $x */` before it.
    fn statement(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let doc = self.doc_of(n);
        if let Some(d) = doc {
            self.inline_var_doc(d);
        }
        self.children(c);
        // The comment is more precise than what the value says (`$x = $this->locate()`).
        if let Some(d) = doc {
            self.inline_var_doc(d);
        }
        // `/** @var T */ $x = make();`: the comment types the assigned variable.
        if let Some(v) = doc.and_then(|d| tag_value(d, b"@var"))
            && let Some(e) = n.named_child(0).filter(|e| self.k(*e) == K::Assignment)
            && let Some(var) = child(e, self.t.f_left).and_then(|l| self.var_name(l))
            && doc.is_some_and(|d| doc_var_name(d).is_none())
        {
            let ty = self.doc_type(v);
            if ty != NONE {
                self.set_var(var, ty);
            }
        }
    }

    /// `/** @var T $x */`: declare `$x` with type `T`.
    fn inline_var_doc(&mut self, d: &'a [u8]) {
        let (Some(v), Some(name)) = (tag_value(d, b"@var"), doc_var_name(d)) else { return };
        let ty = self.doc_type(v);
        if ty != NONE {
            let id = self.cx.intern(name);
            self.set_var(id, ty);
        }
    }

    /// Names directly under an ERROR node are fragments the parser could not place (sometimes
    /// keywords read as names): noise, not references.
    fn error(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.facts.parse_errors += 1;
        self.each(c, |w, c| {
            if !matches!(w.k(c.node()), K::Name | K::QualifiedName | K::RelativeName) {
                w.visit(c);
            }
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UseKind {
    Class,
    Function,
    Const,
}

fn use_kind(b: &[u8]) -> UseKind {
    match b {
        b"function" => UseKind::Function,
        b"const" => UseKind::Const,
        _ => UseKind::Class,
    }
}

/// Append the `name` segments of a namespace name or qualified name, separated by `\`, leaving
/// out whitespace and comments.
fn push_segments<'t>(w: &W<'_, 't>, n: Node<'t>, buf: &mut Vec<u8>, depth: u32) {
    if depth > 8 {
        return;
    }
    for i in 0..n.named_child_count() as u32 {
        let Some(x) = n.named_child(i) else { break };
        match w.k(x) {
            K::Name => {
                if !buf.is_empty() && buf.last() != Some(&b'\\') {
                    buf.push(b'\\');
                }
                buf.extend_from_slice(w.cx.text(x));
            }
            K::NamespaceName => push_segments(w, x, buf, depth + 1),
            _ => {}
        }
    }
}

/// Append a name as written, without whitespace.
fn push_clean(text: &[u8], buf: &mut Vec<u8>) {
    buf.extend(text.iter().copied().filter(|b| !b.is_ascii_whitespace()));
}

fn is_ident(s: &[u8]) -> bool {
    !s.is_empty() && !s[0].is_ascii_digit() && s.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80)
}

// ----- docblocks -----

/// Does the docblock carry the tag (`@test`) as a whole word?
fn has_tag(doc: &[u8], tag: &[u8]) -> bool {
    let mut from = 0;
    while let Some(i) = memchr::memmem::find(&doc[from..], tag) {
        let at = from + i + tag.len();
        if doc.get(at).is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'-' && *b != b'_') {
            return true;
        }
        from = at;
    }
    false
}

/// The type written after a tag (`@var Foo`, `@return ?Bar`): the first word after it.
fn tag_value<'d>(doc: &'d [u8], tag: &[u8]) -> Option<&'d [u8]> {
    let mut from = 0;
    while let Some(i) = memchr::memmem::find(&doc[from..], tag) {
        let mut at = from + i + tag.len();
        from = at;
        if doc.get(at).is_some_and(|b| !b.is_ascii_whitespace()) {
            continue;
        }
        while at < doc.len() && (doc[at] == b' ' || doc[at] == b'\t') {
            at += 1;
        }
        let start = at;
        // A type may hold spaces inside generic arguments: `array<int, Foo>`.
        let mut depth = 0u32;
        while at < doc.len() {
            match doc[at] {
                b'<' | b'{' | b'(' => depth += 1,
                b'>' | b'}' | b')' => depth = depth.saturating_sub(1),
                b if b.is_ascii_whitespace() && depth == 0 => break,
                b'*' if depth == 0 && doc.get(at + 1) == Some(&b'/') => break,
                _ => {}
            }
            at += 1;
        }
        if at > start {
            return Some(&doc[start..at]);
        }
    }
    None
}

/// The variable an inline `@var T $x` names, without the `$`.
fn doc_var_name(doc: &[u8]) -> Option<&[u8]> {
    let i = memchr::memmem::find(doc, b"@var")?;
    let rest = &doc[i + 4..];
    let mut words = rest.split(|b| b.is_ascii_whitespace()).filter(|w| !w.is_empty());
    let _ty = words.next()?;
    let name = words.next()?.strip_prefix(b"$")?;
    let end = name.iter().position(|&b| !(b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80)).unwrap_or(name.len());
    (end > 0).then(|| &name[..end])
}

/// The type a `@param T $name` line gives the parameter written `$name`.
fn param_doc_type<'d>(doc: &'d [u8], var: &[u8]) -> Option<&'d [u8]> {
    let mut from = 0;
    while let Some(i) = memchr::memmem::find(&doc[from..], b"@param") {
        let at = from + i + 6;
        from = at;
        let line_end = memchr::memchr(b'\n', &doc[at..]).map_or(doc.len(), |e| at + e);
        let line = &doc[at..line_end];
        let mut words = line.split(|b| b.is_ascii_whitespace()).filter(|w| !w.is_empty());
        let (Some(ty), Some(name)) = (words.next(), words.next()) else { continue };
        let name = name.strip_prefix(b"...").unwrap_or(name);
        if name == var {
            return Some(ty);
        }
    }
    None
}

// ----- stop lists -----

/// Names in type position that are keywords or pseudo-types, not classes.
fn is_reserved_type(b: &[u8]) -> bool {
    matches!(
        b,
        b"self"
            | b"static"
            | b"parent"
            | b"array"
            | b"callable"
            | b"bool"
            | b"int"
            | b"float"
            | b"string"
            | b"iterable"
            | b"object"
            | b"mixed"
            | b"void"
            | b"null"
            | b"never"
            | b"false"
            | b"true"
            | b"resource"
            | b"double"
            | b"integer"
            | b"boolean"
    )
}

/// Docblock types that name no class (PHPStan and Psalm extensions included).
fn is_doc_pseudo_type(b: &[u8]) -> bool {
    matches!(
        b,
        b"list"
            | b"scalar"
            | b"numeric"
            | b"number"
            | b"class-string"
            | b"callable-string"
            | b"non-empty-string"
            | b"non-empty-array"
            | b"non-empty-list"
            | b"positive-int"
            | b"negative-int"
            | b"array-key"
            | b"key-of"
            | b"value-of"
            | b"Closure"
            | b"this"
    )
}

/// Classes, interfaces and attributes of the PHP core: references to them name nothing of a
/// repository.
fn is_stop_type(b: &[u8]) -> bool {
    matches!(
        b,
        b"stdClass"
            | b"Closure"
            | b"Exception"
            | b"Throwable"
            | b"Error"
            | b"TypeError"
            | b"ValueError"
            | b"ArgumentCountError"
            | b"ErrorException"
            | b"RuntimeException"
            | b"LogicException"
            | b"InvalidArgumentException"
            | b"DomainException"
            | b"LengthException"
            | b"OutOfRangeException"
            | b"OutOfBoundsException"
            | b"RangeException"
            | b"OverflowException"
            | b"UnderflowException"
            | b"UnexpectedValueException"
            | b"BadMethodCallException"
            | b"BadFunctionCallException"
            | b"JsonException"
            | b"Traversable"
            | b"Iterator"
            | b"IteratorAggregate"
            | b"ArrayAccess"
            | b"ArrayIterator"
            | b"ArrayObject"
            | b"Countable"
            | b"JsonSerializable"
            | b"Stringable"
            | b"Serializable"
            | b"Generator"
            | b"UnitEnum"
            | b"BackedEnum"
            | b"WeakMap"
            | b"WeakReference"
            | b"DateTime"
            | b"DateTimeImmutable"
            | b"DateTimeInterface"
            | b"DateTimeZone"
            | b"DateInterval"
            | b"DatePeriod"
            | b"SplObjectStorage"
            | b"SplFileInfo"
            | b"SplStack"
            | b"SplQueue"
            | b"PDO"
            | b"PDOStatement"
            | b"PDOException"
            | b"ReflectionClass"
            | b"ReflectionMethod"
            | b"ReflectionProperty"
            | b"ReflectionFunction"
            | b"ReflectionNamedType"
            | b"ReflectionException"
            | b"Attribute"
            | b"Override"
            | b"ReturnTypeWillChange"
            | b"AllowDynamicProperties"
            | b"SensitiveParameter"
    )
}

fn is_stop_attribute(b: &[u8]) -> bool {
    matches!(b, b"Attribute" | b"Override" | b"ReturnTypeWillChange" | b"AllowDynamicProperties" | b"SensitiveParameter" | b"Deprecated" | b"NoDiscard")
}

/// Constants of the PHP core and the magic constants.
fn is_stop_const(b: &[u8]) -> bool {
    if b.starts_with(b"__") && b.ends_with(b"__") {
        return true;
    }
    const PREFIXES: [&[u8]; 22] = [
        b"PHP_",
        b"E_",
        b"JSON_",
        b"SORT_",
        b"ENT_",
        b"PREG_",
        b"FILTER_",
        b"CURLOPT_",
        b"CURLINFO_",
        b"CURLE_",
        b"LIBXML_",
        b"PATHINFO_",
        b"STR_PAD_",
        b"LC_",
        b"LOCK_",
        b"FILE_",
        b"M_",
        b"INI_",
        b"ARRAY_FILTER_",
        b"COUNT_",
        b"MB_CASE_",
        b"PASSWORD_",
    ];
    if PREFIXES.iter().any(|p| b.starts_with(p)) {
        return true;
    }
    matches!(
        b,
        b"null"
            | b"NULL"
            | b"true"
            | b"TRUE"
            | b"false"
            | b"FALSE"
            | b"INF"
            | b"NAN"
            | b"DIRECTORY_SEPARATOR"
            | b"PATH_SEPARATOR"
            | b"STDIN"
            | b"STDOUT"
            | b"STDERR"
            | b"SEEK_SET"
            | b"SEEK_CUR"
            | b"SEEK_END"
            | b"GLOB_BRACE"
            | b"GLOB_ONLYDIR"
            | b"DATE_ATOM"
            | b"DATE_RFC3339"
            | b"ENT_QUOTES"
    )
}

/// The most frequent functions of the PHP core and the language constructs written like calls.
fn is_stop_fn(b: &[u8]) -> bool {
    matches!(
        b,
        b"isset"
            | b"empty"
            | b"unset"
            | b"array"
            | b"list"
            | b"exit"
            | b"die"
            | b"eval"
            | b"count"
            | b"strlen"
            | b"sprintf"
            | b"printf"
            | b"implode"
            | b"explode"
            | b"join"
            | b"in_array"
            | b"array_key_exists"
            | b"array_keys"
            | b"array_values"
            | b"array_merge"
            | b"array_map"
            | b"array_filter"
            | b"array_reduce"
            | b"array_slice"
            | b"array_splice"
            | b"array_search"
            | b"array_shift"
            | b"array_unshift"
            | b"array_pop"
            | b"array_push"
            | b"array_combine"
            | b"array_flip"
            | b"array_unique"
            | b"array_reverse"
            | b"array_walk"
            | b"array_fill"
            | b"array_diff"
            | b"array_intersect"
            | b"array_column"
            | b"array_key_first"
            | b"array_key_last"
            | b"array_is_list"
            | b"usort"
            | b"uasort"
            | b"uksort"
            | b"sort"
            | b"ksort"
            | b"asort"
            | b"rsort"
            | b"is_array"
            | b"is_string"
            | b"is_int"
            | b"is_integer"
            | b"is_numeric"
            | b"is_bool"
            | b"is_null"
            | b"is_object"
            | b"is_callable"
            | b"is_float"
            | b"is_scalar"
            | b"is_iterable"
            | b"is_a"
            | b"is_subclass_of"
            | b"intval"
            | b"floatval"
            | b"strval"
            | b"boolval"
            | b"settype"
            | b"gettype"
            | b"get_class"
            | b"get_parent_class"
            | b"get_object_vars"
            | b"method_exists"
            | b"property_exists"
            | b"function_exists"
            | b"class_exists"
            | b"interface_exists"
            | b"defined"
            | b"constant"
            | b"call_user_func"
            | b"call_user_func_array"
            | b"func_get_args"
            | b"func_num_args"
            | b"compact"
            | b"extract"
            | b"str_replace"
            | b"str_contains"
            | b"str_starts_with"
            | b"str_ends_with"
            | b"str_repeat"
            | b"str_pad"
            | b"str_split"
            | b"strpos"
            | b"stripos"
            | b"strrpos"
            | b"strstr"
            | b"strtolower"
            | b"strtoupper"
            | b"strtr"
            | b"substr"
            | b"substr_count"
            | b"trim"
            | b"ltrim"
            | b"rtrim"
            | b"ucfirst"
            | b"lcfirst"
            | b"ucwords"
            | b"nl2br"
            | b"htmlspecialchars"
            | b"addslashes"
            | b"stripslashes"
            | b"number_format"
            | b"preg_match"
            | b"preg_match_all"
            | b"preg_replace"
            | b"preg_replace_callback"
            | b"preg_split"
            | b"preg_quote"
            | b"json_encode"
            | b"json_decode"
            | b"serialize"
            | b"unserialize"
            | b"var_dump"
            | b"var_export"
            | b"print_r"
            | b"min"
            | b"max"
            | b"abs"
            | b"floor"
            | b"ceil"
            | b"round"
            | b"rand"
            | b"mt_rand"
            | b"random_int"
            | b"time"
            | b"microtime"
            | b"date"
            | b"strtotime"
            | b"mktime"
            | b"file_exists"
            | b"is_file"
            | b"is_dir"
            | b"file_get_contents"
            | b"file_put_contents"
            | b"fopen"
            | b"fclose"
            | b"fwrite"
            | b"fread"
            | b"dirname"
            | b"basename"
            | b"realpath"
            | b"pathinfo"
            | b"trigger_error"
            | b"error_reporting"
            | b"ini_set"
            | b"ini_get"
            | b"spl_object_hash"
            | b"spl_object_id"
            | b"spl_autoload_register"
            | b"iterator_to_array"
            | b"mb_strlen"
            | b"mb_substr"
            | b"mb_strtolower"
            | b"mb_strtoupper"
            | b"ob_start"
            | b"ob_get_clean"
            | b"header"
            | b"md5"
            | b"sha1"
            | b"hash"
            | b"base64_encode"
            | b"base64_decode"
            | b"urlencode"
            | b"rawurlencode"
            | b"http_build_query"
            | b"parse_url"
            | b"parse_str"
            | b"range"
            | b"reset"
            | b"end"
            | b"key"
            | b"current"
            | b"next"
            | b"array_fill_keys"
            | b"array_pad"
            | b"array_chunk"
            | b"array_sum"
            | b"array_product"
            | b"array_rand"
            | b"shuffle"
            | b"vsprintf"
            | b"version_compare"
            | b"usleep"
            | b"sleep"
            | b"error_log"
    )
}
