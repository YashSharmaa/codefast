//! Rust extractor (tree-sitter-rust).
//!
//! Beyond items and call sites it understands the idioms that make Rust code bases hard to
//! navigate with text search:
//!   * impl blocks: methods hang off an `Impl` definition named after the implemented type, and
//!     `impl Trait for T` records an `Implements` reference,
//!   * receiver types: `self`, annotated parameters and bindings, constructor calls and field
//!     chains give member calls a type path (`Server::cfg`) that is linked at query time,
//!   * macros: `macro_rules!` bodies and macro arguments are opaque token trees to the grammar. A
//!     token-level pass recovers the calls they make and the items they define (`cfg_if!`,
//!     `thread_local!`, `bitflags!`, the integer-method templates of `core::num`, ...),
//!   * attributes: `#[test]` / `#[cfg(test)]` mark test code; attribute macros become
//!     `Annotation` references attached to the item they decorate,
//!   * syntax newer than the grammar (`const impl`, `[const]` bounds, `safe fn`, `macro` items,
//!     trait aliases, ...) derails the parser for whole files. Such files are parsed again from
//!     a copy in which those constructs are blanked or rewritten in place, so that every offset
//!     still refers to the original text.

use crate::extract::*;
use crate::facts::*;
use crate::lang::Lang;
use rustc_hash::FxHashMap;
use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::sync::OnceLock;
use tree_sitter::{Node, Tree, TreeCursor};

// ---------------------------------------------------------------------------------------------
// Node kinds
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    /// Anonymous token (punctuation, keyword): always a leaf.
    Tok,
    /// Named node without special handling: its children are visited.
    Other,
    /// Subtree that never contains references.
    Skip,
    Comment,
    Error,
    Identifier,
    TypeIdentifier,
    FieldIdentifier,
    ShorthandFieldIdentifier,
    PrimitiveType,
    SelfKw,
    Super,
    Crate,
    Metavariable,
    IntegerLiteral,
    StringLiteral,
    MutableSpecifier,
    // items
    FunctionItem,
    FunctionSignatureItem,
    StructItem,
    UnionItem,
    EnumItem,
    EnumVariant,
    FieldDeclaration,
    TraitItem,
    ImplItem,
    TypeItem,
    AssociatedType,
    ConstItem,
    StaticItem,
    ModItem,
    ForeignModItem,
    MacroDefinition,
    MacroRule,
    MacroInvocation,
    UseDeclaration,
    ExternCrateDeclaration,
    AttributeItem,
    InnerAttributeItem,
    DeclarationList,
    FieldDeclarationList,
    VisibilityModifier,
    FunctionModifiers,
    TypeParameters,
    TypeParameter,
    ConstParameter,
    WhereClause,
    TraitBounds,
    Parameters,
    Parameter,
    SelfParameter,
    VariadicParameter,
    ClosureParameters,
    // types
    GenericType,
    GenericTypeWithTurbofish,
    ScopedTypeIdentifier,
    ReferenceType,
    PointerType,
    DynamicType,
    AbstractType,
    BoundedType,
    FunctionType,
    TupleType,
    ArrayType,
    UnitType,
    NeverType,
    TypeBinding,
    TypeArguments,
    BracketedType,
    QualifiedType,
    HigherRankedTraitBound,
    // expressions
    CallExpression,
    Arguments,
    FieldExpression,
    ScopedIdentifier,
    GenericFunction,
    StructExpression,
    FieldInitializerList,
    FieldInitializer,
    ShorthandFieldInitializer,
    AssignmentExpression,
    CompoundAssignmentExpr,
    ClosureExpression,
    Block,
    LetDeclaration,
    LetCondition,
    IfExpression,
    ElseClause,
    WhileExpression,
    ForExpression,
    MatchExpression,
    MatchBlock,
    MatchArm,
    MatchPattern,
    ParenthesizedExpression,
    ReferenceExpression,
    UnaryExpression,
    TryExpression,
    TypeCastExpression,
    IndexExpression,
    // patterns
    TuplePattern,
    TupleStructPattern,
    StructPattern,
    FieldPattern,
    RefPattern,
    MutPattern,
    CapturedPattern,
    ReferencePattern,
    SlicePattern,
    OrPattern,
    // use trees
    UseAsClause,
    UseList,
    ScopedUseList,
    UseWildcard,
    // macros
    TokenTree,
    TokenRepetition,
}

/// Classes of anonymous tokens the token-tree pass cares about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tk {
    /// Any other punctuation.
    Punct,
    /// Any other keyword.
    Kw,
    Dot,
    PathSep,
    Bang,
    Quote,
    Eq,
    AssignOp,
    Colon,
    Semi,
    Comma,
    Pipe,
    Hash,
    Dollar,
    At,
    Lt,
    Gt,
    Shr,
    Arrow,
    FatArrow,
    Amp,
    Star,
    LParen,
    LBracket,
    LBrace,
    Close,
    Fn,
    Struct,
    Enum,
    Union,
    Trait,
    Type,
    Const,
    Static,
    Mod,
    Impl,
    Let,
    For,
    Use,
    Pub,
    Where,
    If,
    While,
    Match,
    As,
    Async,
    Unsafe,
    Default,
    Gen,
}

struct Tables {
    kinds: KindTable<K>,
    toks: Vec<Tk>,
    f_name: u16,
    f_body: u16,
    f_type: u16,
    f_parameters: u16,
    f_return_type: u16,
    f_type_parameters: u16,
    f_type_arguments: u16,
    f_trait: u16,
    f_value: u16,
    f_function: u16,
    f_field: u16,
    f_path: u16,
    f_alias: u16,
    f_bounds: u16,
    f_left: u16,
    f_length: u16,
    f_macro: u16,
    f_arguments: u16,
}

impl Tables {
    #[inline]
    fn tok(&self, id: u16) -> Tk {
        match self.toks.get(id as usize) {
            Some(&t) => t,
            None => Tk::Punct,
        }
    }
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
        let special: &[(&str, K)] = &[
            ("ERROR", K::Error),
            ("identifier", K::Identifier),
            ("type_identifier", K::TypeIdentifier),
            ("field_identifier", K::FieldIdentifier),
            ("shorthand_field_identifier", K::ShorthandFieldIdentifier),
            ("primitive_type", K::PrimitiveType),
            ("self", K::SelfKw),
            ("super", K::Super),
            ("crate", K::Crate),
            ("metavariable", K::Metavariable),
            ("integer_literal", K::IntegerLiteral),
            ("string_literal", K::StringLiteral),
            ("raw_string_literal", K::StringLiteral),
            ("mutable_specifier", K::MutableSpecifier),
            ("line_comment", K::Comment),
            ("block_comment", K::Comment),
            ("function_item", K::FunctionItem),
            ("function_signature_item", K::FunctionSignatureItem),
            ("struct_item", K::StructItem),
            ("union_item", K::UnionItem),
            ("enum_item", K::EnumItem),
            ("enum_variant", K::EnumVariant),
            ("field_declaration", K::FieldDeclaration),
            ("trait_item", K::TraitItem),
            ("impl_item", K::ImplItem),
            ("type_item", K::TypeItem),
            ("associated_type", K::AssociatedType),
            ("const_item", K::ConstItem),
            ("static_item", K::StaticItem),
            ("mod_item", K::ModItem),
            ("foreign_mod_item", K::ForeignModItem),
            ("macro_definition", K::MacroDefinition),
            ("macro_rule", K::MacroRule),
            ("macro_invocation", K::MacroInvocation),
            ("use_declaration", K::UseDeclaration),
            ("extern_crate_declaration", K::ExternCrateDeclaration),
            ("attribute_item", K::AttributeItem),
            ("inner_attribute_item", K::InnerAttributeItem),
            ("declaration_list", K::DeclarationList),
            ("field_declaration_list", K::FieldDeclarationList),
            ("visibility_modifier", K::VisibilityModifier),
            ("function_modifiers", K::FunctionModifiers),
            ("type_parameters", K::TypeParameters),
            ("type_parameter", K::TypeParameter),
            ("const_parameter", K::ConstParameter),
            ("where_clause", K::WhereClause),
            ("trait_bounds", K::TraitBounds),
            ("parameters", K::Parameters),
            ("parameter", K::Parameter),
            ("self_parameter", K::SelfParameter),
            ("variadic_parameter", K::VariadicParameter),
            ("closure_parameters", K::ClosureParameters),
            ("generic_type", K::GenericType),
            ("generic_type_with_turbofish", K::GenericTypeWithTurbofish),
            ("scoped_type_identifier", K::ScopedTypeIdentifier),
            ("reference_type", K::ReferenceType),
            ("pointer_type", K::PointerType),
            ("dynamic_type", K::DynamicType),
            ("abstract_type", K::AbstractType),
            ("bounded_type", K::BoundedType),
            ("function_type", K::FunctionType),
            ("tuple_type", K::TupleType),
            ("array_type", K::ArrayType),
            ("unit_type", K::UnitType),
            ("never_type", K::NeverType),
            ("type_binding", K::TypeBinding),
            ("type_arguments", K::TypeArguments),
            ("bracketed_type", K::BracketedType),
            ("qualified_type", K::QualifiedType),
            ("higher_ranked_trait_bound", K::HigherRankedTraitBound),
            ("call_expression", K::CallExpression),
            ("arguments", K::Arguments),
            ("field_expression", K::FieldExpression),
            ("scoped_identifier", K::ScopedIdentifier),
            ("generic_function", K::GenericFunction),
            ("struct_expression", K::StructExpression),
            ("field_initializer_list", K::FieldInitializerList),
            ("field_initializer", K::FieldInitializer),
            ("shorthand_field_initializer", K::ShorthandFieldInitializer),
            ("assignment_expression", K::AssignmentExpression),
            ("compound_assignment_expr", K::CompoundAssignmentExpr),
            ("closure_expression", K::ClosureExpression),
            ("block", K::Block),
            ("let_declaration", K::LetDeclaration),
            ("let_condition", K::LetCondition),
            ("if_expression", K::IfExpression),
            ("else_clause", K::ElseClause),
            ("while_expression", K::WhileExpression),
            ("for_expression", K::ForExpression),
            ("match_expression", K::MatchExpression),
            ("match_block", K::MatchBlock),
            ("match_arm", K::MatchArm),
            ("match_pattern", K::MatchPattern),
            ("parenthesized_expression", K::ParenthesizedExpression),
            ("reference_expression", K::ReferenceExpression),
            ("unary_expression", K::UnaryExpression),
            ("try_expression", K::TryExpression),
            ("type_cast_expression", K::TypeCastExpression),
            ("index_expression", K::IndexExpression),
            ("tuple_pattern", K::TuplePattern),
            ("tuple_struct_pattern", K::TupleStructPattern),
            ("struct_pattern", K::StructPattern),
            ("field_pattern", K::FieldPattern),
            ("ref_pattern", K::RefPattern),
            ("mut_pattern", K::MutPattern),
            ("captured_pattern", K::CapturedPattern),
            ("reference_pattern", K::ReferencePattern),
            ("slice_pattern", K::SlicePattern),
            ("or_pattern", K::OrPattern),
            ("use_as_clause", K::UseAsClause),
            ("use_list", K::UseList),
            ("scoped_use_list", K::ScopedUseList),
            ("use_wildcard", K::UseWildcard),
            ("token_tree", K::TokenTree),
            ("token_repetition", K::TokenRepetition),
            // leaves and subtrees that never contain references
            ("char_literal", K::Skip),
            ("boolean_literal", K::Skip),
            ("float_literal", K::Skip),
            ("negative_literal", K::Skip),
            ("escape_sequence", K::Skip),
            ("string_content", K::Skip),
            ("shebang", K::Skip),
            ("empty_statement", K::Skip),
            ("unit_expression", K::Skip),
            ("lifetime", K::Skip),
            ("lifetime_parameter", K::Skip),
            ("for_lifetimes", K::Skip),
            ("label", K::Skip),
            ("extern_modifier", K::Skip),
            ("fragment_specifier", K::Skip),
            ("token_tree_pattern", K::Skip),
            ("token_repetition_pattern", K::Skip),
            ("token_binding_pattern", K::Skip),
            ("remaining_field_pattern", K::Skip),
            ("continue_expression", K::Skip),
            ("doc_comment", K::Skip),
            ("inner_doc_comment_marker", K::Skip),
            ("outer_doc_comment_marker", K::Skip),
            ("attribute", K::Skip),
        ];
        // Every other named kind is `Other`; the table default is left to anonymous tokens, which
        // are leaves and can be stepped over without a descent attempt.
        let mut pairs: Vec<(&str, K)> = special.to_vec();
        let n = lang.node_kind_count();
        for id in 0..n {
            let id16 = id as u16;
            if !lang.node_kind_is_named(id16) {
                continue;
            }
            if let Some(name) = lang.node_kind_for_id(id16) {
                if !pairs.iter().any(|(s, _)| *s == name) {
                    pairs.push((name, K::Other));
                }
            }
        }
        let kinds = KindTable::build(&lang, K::Tok, &pairs);
        let mut toks = vec![Tk::Punct; n];
        for (id, slot) in toks.iter_mut().enumerate() {
            let id16 = id as u16;
            if lang.node_kind_is_named(id16) {
                continue;
            }
            let Some(name) = lang.node_kind_for_id(id16) else { continue };
            *slot = match name {
                "." => Tk::Dot,
                "::" => Tk::PathSep,
                "!" => Tk::Bang,
                "'" => Tk::Quote,
                "=" => Tk::Eq,
                "+=" | "-=" | "*=" | "/=" | "%=" | "^=" | "&=" | "|=" | "<<=" | ">>=" => Tk::AssignOp,
                ":" => Tk::Colon,
                ";" => Tk::Semi,
                "," => Tk::Comma,
                "|" => Tk::Pipe,
                "#" => Tk::Hash,
                "$" => Tk::Dollar,
                "@" => Tk::At,
                "<" => Tk::Lt,
                ">" => Tk::Gt,
                ">>" => Tk::Shr,
                "->" => Tk::Arrow,
                "=>" => Tk::FatArrow,
                "&" => Tk::Amp,
                "*" => Tk::Star,
                "(" => Tk::LParen,
                "[" => Tk::LBracket,
                "{" => Tk::LBrace,
                ")" | "]" | "}" => Tk::Close,
                "fn" => Tk::Fn,
                "struct" => Tk::Struct,
                "enum" => Tk::Enum,
                "union" => Tk::Union,
                "trait" => Tk::Trait,
                "type" => Tk::Type,
                "const" => Tk::Const,
                "static" => Tk::Static,
                "mod" => Tk::Mod,
                "impl" => Tk::Impl,
                "let" => Tk::Let,
                "for" => Tk::For,
                "use" => Tk::Use,
                "pub" => Tk::Pub,
                "where" => Tk::Where,
                "if" => Tk::If,
                "while" => Tk::While,
                "match" => Tk::Match,
                "as" => Tk::As,
                "async" => Tk::Async,
                "unsafe" => Tk::Unsafe,
                "default" => Tk::Default,
                "gen" => Tk::Gen,
                s if s.as_bytes().first().is_some_and(|b| b.is_ascii_alphabetic()) => Tk::Kw,
                _ => Tk::Punct,
            };
        }
        Tables {
            kinds,
            toks,
            f_name: field(&lang, "name"),
            f_body: field(&lang, "body"),
            f_type: field(&lang, "type"),
            f_parameters: field(&lang, "parameters"),
            f_return_type: field(&lang, "return_type"),
            f_type_parameters: field(&lang, "type_parameters"),
            f_type_arguments: field(&lang, "type_arguments"),
            f_trait: field(&lang, "trait"),
            f_value: field(&lang, "value"),
            f_function: field(&lang, "function"),
            f_field: field(&lang, "field"),
            f_path: field(&lang, "path"),
            f_alias: field(&lang, "alias"),
            f_bounds: field(&lang, "bounds"),
            f_left: field(&lang, "left"),
            f_length: field(&lang, "length"),
            f_macro: field(&lang, "macro"),
            f_arguments: field(&lang, "arguments"),
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Walker state
// ---------------------------------------------------------------------------------------------

/// What the innermost enclosing container is: decides the kind and flags of the items in it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Scope {
    /// File or module level.
    Mod,
    Impl,
    Trait,
    /// Function body: nested items are invisible outside it.
    Fn,
    /// `extern "C" { ... }`: bodiless declarations.
    Foreign,
    /// Struct, union, enum or enum variant: owner of fields.
    Type,
    /// `macro_rules!` body: owner of the items the macro generates.
    Macro,
}

/// How a type is going to be named.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TyUse {
    /// `type_name` of a field, variable or alias: a named type, auto-deref wrappers stripped.
    Decl,
    /// Return type of a function: references stripped, the outer name kept.
    Ret,
    /// Type of a local: like `Decl`, plus primitives, slices and wrapper forms (`Option<T>`).
    Local,
}

/// Attributes seen since the last item.
#[derive(Clone, Copy, Default)]
struct Attrs {
    test: bool,
    cfg_test: bool,
    macro_export: bool,
}

#[derive(Clone, Copy)]
struct Saved {
    parent: u32,
    scope: Scope,
    test_ctx: bool,
    inherit_pub: bool,
    self_ty: u32,
}

/// The implemented type of an impl block.
enum ImplTy {
    Named(u32),
    /// A generic parameter (blanket impl), with its trait bound when it has exactly one.
    Generic(u32),
    Unknown,
}

/// Buffers reused across files (one set per worker thread).
#[derive(Default)]
struct Bufs {
    /// Generic parameters in scope: (name, single trait bound or NONE).
    generics: Vec<(u32, u32)>,
    /// Bindings collected from a pattern, declared once their scope is open: (name, type).
    binds: Vec<(u32, u32)>,
    /// Attribute macros waiting for the item they decorate: (name, qualifier, line).
    annots: Vec<(u32, u32, u32)>,
    /// Names bound by `use` declarations of this file, with the full path each stands for
    /// (`use crate::query as q` binds `q` to `crate::query`).
    imported: FxHashMap<u32, u32>,
    /// Scratch text for paths while `Scratch::buf` is in use.
    tmp: Vec<u8>,
    /// `macro` items rewritten for the parser.
    decl: Vec<DeclMacro>,
}

thread_local! {
    static BUFS: Cell<Option<Box<Bufs>>> = const { Cell::new(None) };
    /// Parser for second attempts at files whose syntax the grammar does not know.
    static PARSER: RefCell<Option<tree_sitter::Parser>> = const { RefCell::new(None) };
}

/// Pre-interned names.
struct Ids {
    self_upper: u32,
    some: u32,
    none: u32,
    ok: u32,
    err: u32,
    underscore: u32,
    f16: u32,
    f128: u32,
    option: u32,
    result: u32,
}

struct W<'a, 't> {
    cx: Ctx<'a>,
    t: &'static Tables,
    b: Box<Bufs>,
    ids: Ids,
    /// The text that was parsed when it is not the file's own (see `sanitize`); else empty.
    patched: &'a [u8],
    /// Nesting depth of function bodies.
    fn_depth: u32,
    /// Innermost container definition (module, impl, trait, type, function), or `NONE`.
    parent: u32,
    scope: Scope,
    /// Name of the `Self` type: the impl's type or the trait; `NONE` outside of both.
    self_ty: u32,
    /// Inside test-only code.
    test_ctx: bool,
    /// Items of the current container are as public as the container (trait items, variants).
    inherit_pub: bool,
    /// Type of the scrutinee of the innermost `match`.
    match_ty: u32,
    pend: Attrs,
    pend_active: bool,
    /// Set by the token pass when a parameter list contains a `self` receiver.
    saw_self: bool,
    /// The impl block being visited implements `Deref`: its `Target` is where member access
    /// on the type continues.
    deref_impl: bool,
    _t: PhantomData<&'t ()>,
}

pub fn extract(_lang: Lang, src: &[u8], tree: &Tree, path: &str, facts: &mut FileFacts, scratch: &mut Scratch) {
    let mut b = BUFS.with(|c| c.take()).unwrap_or_default();
    b.generics.clear();
    b.binds.clear();
    b.annots.clear();
    b.imported.clear();
    b.decl.clear();
    // A file the grammar could not make sense of gets a second chance.
    let mut patched: Vec<u8> = Vec::new();
    let mut reparsed: Option<Tree> = None;
    // (Not for binary data that merely carries the extension: a second parse of noise is slow
    // and recovers nothing.)
    if tree.root_node().has_error() && memchr::memchr(0, src).is_none() {
        if let Some(p) = sanitize(src, &mut b.decl) {
            reparsed = reparse(&p);
            if reparsed.is_some() {
                patched = p;
            } else {
                b.decl.clear();
            }
        }
    }
    let tree = reparsed.as_ref().unwrap_or(tree);
    let mut cx = Ctx::new(src, path, facts, scratch);
    let ids = Ids {
        self_upper: cx.intern(b"Self"),
        some: cx.intern(b"Some"),
        none: cx.intern(b"None"),
        ok: cx.intern(b"Ok"),
        err: cx.intern(b"Err"),
        underscore: cx.intern(b"_"),
        f16: cx.intern(b"f16"),
        f128: cx.intern(b"f128"),
        option: cx.intern(b"Option"),
        result: cx.intern(b"Result"),
    };
    let mut w = W {
        cx,
        t: tables(),
        b,
        ids,
        patched: &patched,
        fn_depth: 0,
        parent: NONE,
        scope: Scope::Mod,
        self_ty: NONE,
        test_ctx: false,
        inherit_pub: false,
        match_ty: NONE,
        pend: Attrs::default(),
        pend_active: false,
        saw_self: false,
        deref_impl: false,
        _t: PhantomData,
    };
    let mut c = tree.walk();
    w.visit(&mut c);
    w.flush_attrs();
    let b = w.b;
    BUFS.with(|c| c.set(Some(b)));
}

#[inline]
fn upper(t: &[u8]) -> bool {
    t.first().is_some_and(|b| b.is_ascii_uppercase())
}

/// Offset of the first segment of path text `p` that names a type (is capitalised), if any.
fn path_type_at(p: &[u8]) -> Option<usize> {
    let mut i = 0;
    loop {
        if p.get(i).is_some_and(|b| b.is_ascii_uppercase()) {
            return Some(i);
        }
        match memchr::memmem::find(&p[i..], b"::") {
            Some(j) => i += j + 2,
            None => return None,
        }
    }
}

/// Canonical form of a qualifier path, in place: a leading `::` dropped, and the type that
/// follows a module path set off with `|` (`crate::a::b::Type` -> `crate::a::b|Type`).
fn set_off_type(p: &mut Vec<u8>) {
    if p.starts_with(b"::") {
        p.drain(..2);
    }
    if let Some(i) = path_type_at(p) {
        if i >= 2 {
            p[i - 2] = b'|';
            p.remove(i - 1);
        }
    }
}

#[inline]
fn clamp_u16(n: usize) -> u16 {
    n.min(u16::MAX as usize) as u16
}

impl<'a, 't> W<'a, 't> {
    #[inline]
    fn k(&self, n: Node<'t>) -> K {
        self.t.kinds.get(n.kind_id())
    }

    #[inline]
    fn tk(&self, n: Node<'t>) -> Tk {
        self.t.tok(n.kind_id())
    }

    /// Name of an identifier node, without the raw-identifier prefix.
    #[inline]
    fn ident(&mut self, n: Node<'t>) -> u32 {
        let s = self.cx.text(n);
        let s = if s.len() > 2 && s[0] == b'r' && s[1] == b'#' { &s[2..] } else { s };
        self.cx.intern(s)
    }

    #[inline]
    fn is_stop(&self, name: u32) -> bool {
        let i = &self.ids;
        name == i.some || name == i.none || name == i.ok || name == i.err || name == i.self_upper || name == i.underscore
    }

    /// `Some` / `None` / `Ok` / `Err` as the prelude means them: bare, or through `Option` /
    /// `Result`. `Status::Ok` is a name of its own.
    #[inline]
    fn is_prelude_variant(&self, name: u32, qual: u32) -> bool {
        self.is_stop(name) && (qual == NONE || qual == self.ids.option || qual == self.ids.result)
    }

    /// `Some(bound)` when `name` is a generic parameter in scope.
    #[inline]
    fn generic(&self, name: u32) -> Option<u32> {
        self.b.generics.iter().rev().find(|g| g.0 == name).map(|g| g.1)
    }

    #[inline]
    fn imported(&self, name: u32) -> bool {
        self.b.imported.contains_key(&name)
    }

    /// The path a lower-case name stands for when a `use` of this file binds it.
    #[inline]
    fn import_target(&self, name: u32) -> Option<u32> {
        if self.b.imported.is_empty() {
            return None;
        }
        self.b.imported.get(&name).copied()
    }

    fn imported_ident(&mut self, n: Node<'t>) -> bool {
        if self.b.imported.is_empty() {
            return false;
        }
        let name = self.ident(n);
        self.imported(name)
    }

    /// Run `f` on every child of the cursor's node.
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
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        if c.goto_first_child() {
            loop {
                self.visit(c);
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.cx.depth -= 1;
    }

    /// Visit the receiver of a member access: the first child that is not a comment.
    fn visit_first(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        if c.goto_first_child() {
            loop {
                let n = c.node();
                let k = self.k(n);
                if k != K::Comment {
                    // A lower-case name with members is a variable. When it is not a known
                    // local, a macro introduced it: functions have no members to access.
                    if k != K::Identifier || upper(self.cx.text(n)) || self.imported_ident(n) {
                        self.visit(c);
                    }
                    break;
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.cx.depth -= 1;
    }

    fn visit(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        let k = self.k(n);
        if self.pend_active && !takes_attrs(k) {
            self.flush_attrs();
        }
        match k {
            K::Tok
            | K::Skip
            | K::Comment
            | K::PrimitiveType
            | K::SelfKw
            | K::Super
            | K::Crate
            | K::Metavariable
            | K::IntegerLiteral
            | K::StringLiteral
            | K::MutableSpecifier
            | K::FieldIdentifier
            | K::ShorthandFieldIdentifier
            | K::VisibilityModifier
            | K::FunctionModifiers
            | K::UnitType
            | K::NeverType
            | K::SelfParameter
            | K::VariadicParameter => {}
            K::Identifier => self.identifier(n),
            K::TypeIdentifier => self.type_identifier(n),
            K::FieldExpression => self.field_expression(n, c, RefKind::MemberRead),
            K::CallExpression => self.call_expression(c),
            K::ScopedIdentifier => self.scoped_identifier(n, c, RefKind::Read, 255),
            K::ScopedTypeIdentifier => {
                self.scoped_type(n, c, RefKind::Type);
            }
            K::Block => self.block(c),
            K::LetDeclaration => self.let_declaration(n, c),
            K::LetCondition => self.let_condition(n, c),
            K::IfExpression => self.if_expression(c),
            K::WhileExpression | K::ClosureExpression => self.scoped_children(c),
            K::ForExpression => self.for_expression(n, c),
            K::MatchExpression => self.match_expression(n, c),
            K::MatchArm => self.match_arm(c),
            K::ClosureParameters => self.closure_parameters(c),
            K::StructExpression => self.struct_expression(c),
            K::AssignmentExpression => self.assignment(c, true),
            K::CompoundAssignmentExpr => self.assignment(c, false),
            K::MacroInvocation => self.macro_invocation(c),
            K::TypeBinding => self.type_binding(c),
            K::TypeArguments => self.type_arguments(c),
            K::Parameters => self.type_parameters_list(c),
            K::TypeParameters => self.type_parameters(c),
            K::WhereClause => {
                self.where_bounds(n);
                self.children(c);
            }
            K::FunctionItem => self.function(n, c, true),
            K::FunctionSignatureItem => self.function(n, c, false),
            K::StructItem => self.struct_item(n, c, DefKind::Struct),
            K::UnionItem => self.struct_item(n, c, DefKind::Union),
            K::EnumItem => self.enum_item(n, c),
            K::EnumVariant => self.enum_variant(n, c),
            K::FieldDeclaration => self.field_declaration(n, c),
            K::TraitItem => self.trait_item(n, c),
            K::ImplItem => self.impl_item(n, c),
            K::TypeItem => self.type_item(n, c, false),
            K::AssociatedType => self.type_item(n, c, true),
            K::ConstItem => self.const_item(n, c, DefKind::Constant),
            K::StaticItem => self.const_item(n, c, DefKind::Variable),
            K::ModItem => self.mod_item(n, c),
            K::ForeignModItem => self.foreign_mod(c),
            K::MacroDefinition => self.macro_definition(n, c),
            K::UseDeclaration => self.use_declaration(n, c),
            K::ExternCrateDeclaration => self.extern_crate(n),
            K::AttributeItem => self.attribute_item(n, false),
            K::InnerAttributeItem => self.attribute_item(n, true),
            K::TokenTree | K::TokenRepetition => {
                // A token tree outside a macro handler (only under parse errors).
                self.cx.push_scope();
                self.tokens(c, Tm { body: Body::Code, code: true, defs: false, pat: false }, Init::Plain);
                self.cx.pop_scope();
            }
            K::Error => self.error(c),
            _ => self.children(c),
        }
    }

    /// Syntax errors: keep walking what could be recognised. Bare identifiers directly under an
    /// error node are usually fragments of syntax the grammar does not know, not references.
    fn error(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.facts.parse_errors += 1;
        self.each(c, |w, c| {
            if w.k(c.node()) != K::Identifier {
                w.visit(c);
            }
        });
    }

    // ----- leaves -----

    fn identifier(&mut self, n: Node<'t>) {
        if n.start_byte() == n.end_byte() {
            // A node the parser made up (`Ty<>`, `(): Bound`): it names nothing.
            return;
        }
        let name = self.ident(n);
        if self.cx.is_local(name) || self.is_stop(name) || self.generic(name).is_some() {
            return;
        }
        self.cx.add_ref(name, RefKind::Read, line_of(n), NONE, 255);
    }

    fn type_identifier(&mut self, n: Node<'t>) {
        if n.start_byte() == n.end_byte() {
            return;
        }
        let name = self.ident(n);
        let i = &self.ids;
        if name == i.self_upper || name == i.underscore || name == i.f16 || name == i.f128 || self.generic(name).is_some() {
            return;
        }
        self.cx.add_ref(name, RefKind::Type, line_of(n), NONE, 255);
    }

    // ----- attributes -----

    fn take_attrs(&mut self) -> Attrs {
        self.pend_active = false;
        std::mem::take(&mut self.pend)
    }

    /// Attach the waiting attribute macros to definition `d`.
    fn attach_annots(&mut self, d: u32) {
        if self.b.annots.is_empty() {
            return;
        }
        for i in 0..self.b.annots.len() {
            let (name, qual, line) = self.b.annots[i];
            self.cx.facts.refs.push(Ref { name, qual, line, encl: d, kind: RefKind::Annotation, nargs: 255 });
        }
        self.b.annots.clear();
    }

    /// Attributes that turned out not to decorate an item: on statements, expressions and
    /// parameters they are never attribute macros.
    fn flush_attrs(&mut self) {
        self.pend_active = false;
        self.pend = Attrs::default();
        self.b.annots.clear();
    }

    fn attribute_item(&mut self, n: Node<'t>, inner: bool) {
        let t = self.t;
        let Some(attr) = n.named_child(0) else { return };
        let Some(path) = attr.child(0) else { return };
        let (name_node, qual) = match self.k(path) {
            K::Identifier => (path, None),
            K::ScopedIdentifier => match child(path, t.f_name) {
                Some(nm) => (nm, child(path, t.f_path)),
                None => return,
            },
            _ => return,
        };
        let txt = self.cx.text(name_node);
        let test_like = is_test_attr(txt);
        if inner {
            // `#![cfg(test)]`: the enclosing module is test code.
            if qual.is_none() && txt == b"cfg" && child(attr, t.f_arguments).is_some_and(|a| self.cfg_has_test(a, 0)) {
                self.test_ctx = true;
                if self.parent != NONE && self.scope == Scope::Mod {
                    self.cx.def_mut(self.parent).flags |= dflag::TEST;
                }
            }
            return;
        }
        self.pend_active = true;
        if test_like {
            self.pend.test = true;
        }
        match qual {
            None => {
                if txt == b"cfg" {
                    if child(attr, t.f_arguments).is_some_and(|a| self.cfg_has_test(a, 0)) {
                        self.pend.cfg_test = true;
                    }
                } else if txt == b"macro_export" {
                    self.pend.macro_export = true;
                } else if !is_builtin_attr(txt) {
                    let name = self.ident(name_node);
                    self.b.annots.push((name, NONE, line_of(name_node)));
                }
            }
            Some(q) => {
                // Tool attributes (`rustfmt::skip`, `clippy::...`) are not macros.
                let head = self.path_head(q);
                if matches!(head, b"rustfmt" | b"clippy" | b"rustc" | b"diagnostic" | b"miri" | b"rust_analyzer") {
                    return;
                }
                let name = self.ident(name_node);
                let qual = self.path_qual(q);
                self.b.annots.push((name, qual, line_of(name_node)));
            }
        }
    }

    /// First segment of a path, as text.
    fn path_head(&self, mut p: Node<'t>) -> &'a [u8] {
        for _ in 0..16 {
            if self.k(p) != K::ScopedIdentifier {
                break;
            }
            match child(p, self.t.f_path) {
                Some(x) => p = x,
                None => break,
            }
        }
        self.cx.text(p)
    }

    /// Does a `cfg(...)` predicate require `test` (outside of any `not(...)`)?
    fn cfg_has_test(&self, tt: Node<'t>, depth: u32) -> bool {
        if depth > 6 {
            return false;
        }
        let mut skip_next = false;
        for i in 0..tt.child_count() {
            let Some(ch) = tt.child(i) else { break };
            match self.k(ch) {
                K::Identifier => {
                    let txt = self.cx.text(ch);
                    if txt == b"test" {
                        return true;
                    }
                    skip_next = txt == b"not";
                }
                K::TokenTree => {
                    if !skip_next && self.cfg_has_test(ch, depth + 1) {
                        return true;
                    }
                    skip_next = false;
                }
                _ => {}
            }
        }
        false
    }

    // ----- definitions: shared helpers -----

    #[inline]
    fn has_vis(&self, n: Node<'t>) -> bool {
        n.child(0).is_some_and(|x| self.k(x) == K::VisibilityModifier)
    }

    /// End of a bodiless item, without its `;`.
    fn trim_semi(&self, n: Node<'t>) -> usize {
        let e = n.end_byte();
        if e > n.start_byte() && self.cx.src.get(e - 1) == Some(&b';') { e - 1 } else { e }
    }

    fn new_def(&mut self, name: u32, kind: DefKind, n: Node<'t>, sig_end: usize, at: Attrs, exported: bool) -> u32 {
        let d = self.cx.def(name, kind, n, sig_end);
        let mut flags = 0u8;
        if exported {
            flags |= dflag::EXPORTED;
        }
        if at.test || at.cfg_test || self.test_ctx {
            flags |= dflag::TEST;
        }
        // Items declared in a function body cannot be named from anywhere else.
        if self.scope == Scope::Fn && !matches!(kind, DefKind::Impl | DefKind::Field | DefKind::EnumMember) {
            flags |= dflag::LOCAL;
        }
        let parent = self.parent;
        let lead = if self.patched.is_empty() { 0 } else { self.blanked_lead(n.start_byte()) };
        if lead > 0 && self.cx.src[n.start_byte() - lead..].starts_with(b"pub") {
            flags |= dflag::EXPORTED;
        }
        let dm = self.cx.def_mut(d);
        if lead > 0 {
            // `const impl ..`, `pub(crate) type X;`: modifiers the parser was not shown.
            dm.sig_off -= lead as u32;
            dm.sig_len = clamp_u16(dm.sig_len as usize + lead);
        }
        dm.flags = flags;
        dm.parent = parent;
        if matches!(kind, DefKind::Field | DefKind::EnumMember) {
            // Attributes on fields and variants are helpers of a derive, not macros.
            self.b.annots.clear();
        } else {
            self.attach_annots(d);
        }
        d
    }

    /// Length of the text right before `start` that was blanked for the parser.
    fn blanked_lead(&self, start: usize) -> usize {
        let (p, o) = (self.patched, self.cx.src);
        if start > p.len() || p.len() != o.len() {
            return 0;
        }
        let mut s = start;
        while s > 0 && p[s - 1] == b' ' && o[s - 1] != b'\n' {
            s -= 1;
        }
        while s < start && o[s].is_ascii_whitespace() {
            s += 1;
        }
        start - s
    }

    /// Length of the text right after `end`, on the same line, that was blanked for the parser.
    fn blanked_tail(&self, end: usize) -> usize {
        let (p, o) = (self.patched, self.cx.src);
        if end > p.len() || p.len() != o.len() {
            return 0;
        }
        let mut e = end;
        while e < p.len() && p[e] == b' ' && o[e] != b'\n' {
            e += 1;
        }
        while e > end && o[e - 1].is_ascii_whitespace() {
            e -= 1;
        }
        e - end
    }

    fn enter(&mut self, d: u32, scope: Scope) -> Saved {
        let s = Saved { parent: self.parent, scope: self.scope, test_ctx: self.test_ctx, inherit_pub: self.inherit_pub, self_ty: self.self_ty };
        if d != NONE {
            self.cx.push_def(d);
            self.parent = d;
        }
        self.scope = scope;
        self.inherit_pub = false;
        s
    }

    fn leave(&mut self, d: u32, s: Saved) {
        if d != NONE {
            self.cx.pop_def();
        }
        self.parent = s.parent;
        self.scope = s.scope;
        self.test_ctx = s.test_ctx;
        self.inherit_pub = s.inherit_pub;
        self.self_ty = s.self_ty;
    }

    // ----- generics -----

    /// Register the generic parameters declared by a `type_parameters` node.
    fn register_generics(&mut self, tp: Node<'t>) {
        let t = self.t;
        for i in 0..tp.named_child_count() as u32 {
            let Some(p) = tp.named_child(i) else { break };
            match self.k(p) {
                K::TypeParameter => {
                    if let Some(nm) = child(p, t.f_name) {
                        let name = self.ident(nm);
                        let bound = match child(p, t.f_bounds) {
                            Some(b) => self.bound_of(b),
                            None => NONE,
                        };
                        self.b.generics.push((name, bound));
                    }
                }
                K::ConstParameter => {
                    if let Some(nm) = child(p, t.f_name) {
                        let name = self.ident(nm);
                        self.b.generics.push((name, NONE));
                    }
                }
                _ => {}
            }
        }
    }

    /// Visit the bounds, defaults and const types of a `type_parameters` node (cursor on it).
    fn generic_bounds(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            if matches!(w.k(c.node()), K::TypeParameter | K::ConstParameter) {
                let mut name_seen = false;
                w.each(c, |w, c| {
                    let k = w.k(c.node());
                    if !name_seen && matches!(k, K::TypeIdentifier | K::Identifier) {
                        name_seen = true;
                    } else {
                        w.visit(c);
                    }
                });
            }
        });
    }

    fn type_parameters(&mut self, c: &mut TreeCursor<'t>) {
        self.register_generics(c.node());
        self.generic_bounds(c);
    }

    /// The trait a generic parameter is bounded by, when there is exactly one (marker traits and
    /// lifetimes aside): methods called on a value of that type are the trait's.
    fn bound_of(&mut self, bounds: Node<'t>) -> u32 {
        let t = self.t;
        let mut found = NONE;
        let mut count = 0;
        for i in 0..bounds.named_child_count() as u32 {
            let Some(mut b) = bounds.named_child(i) else { break };
            let mut head: Option<Node<'t>> = None;
            let mut scoped: Option<Node<'t>> = None;
            for _ in 0..4 {
                match self.k(b) {
                    K::TypeIdentifier => {
                        head = Some(b);
                        break;
                    }
                    K::GenericType | K::HigherRankedTraitBound => match child(b, t.f_type) {
                        Some(x) => b = x,
                        None => break,
                    },
                    K::ScopedTypeIdentifier => match child(b, t.f_name) {
                        Some(x) => {
                            scoped = Some(b);
                            b = x;
                        }
                        None => break,
                    },
                    K::FunctionType => match child(b, t.f_trait) {
                        Some(x) => b = x,
                        None => break,
                    },
                    _ => break,
                }
            }
            if let Some(h) = head {
                if is_marker_trait(self.cx.text(h)) {
                    continue;
                }
                count += 1;
                // `T: q::Runner`: the trait keeps its module.
                found = match scoped.and_then(|s| child(s, t.f_path)) {
                    Some(p) => match self.module_type_id(Some(p), h) {
                        NONE => self.ident(h),
                        id => id,
                    },
                    None => self.ident(h),
                };
            }
        }
        if count == 1 { found } else { NONE }
    }

    /// Bounds given in a where clause refine the generic parameters they name.
    fn where_bounds(&mut self, wc: Node<'t>) {
        let t = self.t;
        if self.b.generics.is_empty() {
            return;
        }
        for i in 0..wc.named_child_count() as u32 {
            let Some(pred) = wc.named_child(i) else { break };
            let (Some(left), Some(bounds)) = (child(pred, t.f_left), child(pred, t.f_bounds)) else { continue };
            if self.k(left) != K::TypeIdentifier {
                continue;
            }
            let name = self.ident(left);
            let Some(pos) = self.b.generics.iter().rposition(|g| g.0 == name) else { continue };
            if self.b.generics[pos].1 == NONE {
                let b = self.bound_of(bounds);
                self.b.generics[pos].1 = b;
            }
        }
    }

    /// The where clause of an item, when it is where the grammar puts it: right before the body.
    fn where_clause_of(&self, n: Node<'t>) -> Option<Node<'t>> {
        let cnt = n.child_count();
        if cnt < 2 {
            return None;
        }
        n.child(cnt - 2).filter(|x| self.k(*x) == K::WhereClause)
    }

    // ----- type names -----

    /// Name of a type for the given use, interned; `NONE` when it has no usable name.
    fn type_name(&mut self, ty: Node<'t>, mode: TyUse) -> u32 {
        // Plain named types need no buffer.
        if self.k(ty) == K::TypeIdentifier {
            let name = self.ident(ty);
            if name == self.ids.self_upper {
                return self.self_ty;
            }
            return match self.generic(name) {
                Some(b) if mode != TyUse::Ret => b,
                Some(_) => NONE,
                None if name == self.ids.underscore => NONE,
                None => name,
            };
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let ok = self.type_repr(ty, &mut buf, mode, 0);
        let id = if ok && !buf.is_empty() { self.cx.intern(&buf) } else { NONE };
        self.cx.s.buf = buf;
        id
    }

    /// First type argument of a `type_arguments` node, and whether it is the only one.
    fn first_type_arg(&self, ta: Node<'t>) -> Option<(Node<'t>, bool)> {
        let mut first = None;
        let mut count = 0;
        for i in 0..ta.named_child_count() as u32 {
            let Some(a) = ta.named_child(i) else { break };
            match self.k(a) {
                K::Skip | K::Comment | K::TypeBinding | K::TraitBounds => {}
                _ => {
                    count += 1;
                    if first.is_none() {
                        first = Some(a);
                    }
                }
            }
        }
        first.map(|f| (f, count == 1))
    }

    fn type_repr(&mut self, mut ty: Node<'t>, buf: &mut Vec<u8>, mode: TyUse, depth: u32) -> bool {
        let t = self.t;
        if depth > 6 {
            return false;
        }
        for _ in 0..8 {
            match self.k(ty) {
                K::ReferenceType | K::PointerType | K::HigherRankedTraitBound => match child(ty, t.f_type) {
                    Some(x) => ty = x,
                    None => return false,
                },
                K::TypeIdentifier => {
                    let name = self.ident(ty);
                    let id = if name == self.ids.self_upper {
                        self.self_ty
                    } else {
                        match self.generic(name) {
                            Some(b) if mode != TyUse::Ret => b,
                            Some(_) => NONE,
                            None if name == self.ids.underscore => NONE,
                            None => name,
                        }
                    };
                    if id == NONE {
                        return false;
                    }
                    buf.extend_from_slice(self.cx.name_bytes(id));
                    return true;
                }
                K::Identifier => {
                    buf.extend_from_slice(self.cx.text(ty));
                    return true;
                }
                K::ScopedTypeIdentifier => {
                    let Some(nm) = child(ty, t.f_name) else { return false };
                    let Some(p) = child(ty, t.f_path) else {
                        // `::Name`
                        buf.extend_from_slice(self.cx.text(nm));
                        return true;
                    };
                    // `T::Item`, `Self::Output`, `<T as Tr>::Out`: projections have no fixed name.
                    match self.k(p) {
                        K::BracketedType => return false,
                        K::Identifier => {
                            let pn = self.ident(p);
                            if pn == self.ids.self_upper || self.generic(pn).is_some() {
                                return false;
                            }
                        }
                        _ => {}
                    }
                    // `store::Index` is `crate::store|Index`.
                    return self.module_type(p, self.cx.text(nm), buf);
                }
                K::GenericType => {
                    let Some(head) = child(ty, t.f_type) else { return false };
                    if mode == TyUse::Ret {
                        ty = head;
                        continue;
                    }
                    let head_name = match self.k(head) {
                        K::ScopedTypeIdentifier => match child(head, t.f_name) {
                            Some(x) => x,
                            None => return false,
                        },
                        _ => head,
                    };
                    let htxt = self.cx.text(head_name);
                    let arg = child(ty, t.f_type_arguments).and_then(|ta| self.first_type_arg(ta));
                    if is_deref_wrapper(htxt) {
                        // `Box<T>`, `Rc<T>`, `Arc<T>`: member access goes straight to `T`.
                        match arg {
                            Some((a, true)) => {
                                ty = a;
                                continue;
                            }
                            _ => {
                                ty = head;
                                continue;
                            }
                        }
                    }
                    if mode == TyUse::Local && is_wrapper(htxt) {
                        // `Option<T>`, `Vec<T>`, ...: keep the element type for unwrapping steps.
                        let mark = buf.len();
                        buf.extend_from_slice(htxt);
                        if let Some((a, _)) = arg {
                            let inner = buf.len();
                            buf.push(b'<');
                            if self.type_repr(a, buf, mode, depth + 1) {
                                buf.push(b'>');
                            } else {
                                buf.truncate(inner);
                            }
                        }
                        return buf.len() > mark;
                    }
                    ty = head;
                }
                K::DynamicType | K::AbstractType => {
                    if mode == TyUse::Ret && self.k(ty) == K::AbstractType {
                        return false;
                    }
                    match child(ty, t.f_trait) {
                        Some(x) => ty = x,
                        None => return false,
                    }
                }
                K::BoundedType => match ty.named_child(0) {
                    Some(x) => ty = x,
                    None => return false,
                },
                K::ArrayType => {
                    if mode != TyUse::Local {
                        return false;
                    }
                    buf.extend_from_slice(if child(ty, t.f_length).is_some() { b"array" } else { b"slice" });
                    if let Some(el) = ty.named_child(0) {
                        let inner = buf.len();
                        buf.push(b'<');
                        if self.type_repr(el, buf, mode, depth + 1) {
                            buf.push(b'>');
                        } else {
                            buf.truncate(inner);
                        }
                    }
                    return true;
                }
                K::PrimitiveType => {
                    if mode != TyUse::Local {
                        return false;
                    }
                    buf.extend_from_slice(self.cx.text(ty));
                    return true;
                }
                _ => return false,
            }
        }
        false
    }

    /// The function type a declared type boils down to, if it holds a callable: `fn(A) -> B`,
    /// `Box<dyn Fn(A) -> B>`, `Option<&dyn FnMut()>`.
    fn fn_type(&self, mut ty: Node<'t>) -> Option<Node<'t>> {
        let t = self.t;
        for _ in 0..6 {
            match self.k(ty) {
                K::FunctionType => return Some(ty),
                K::ReferenceType | K::PointerType | K::HigherRankedTraitBound => ty = child(ty, t.f_type)?,
                K::DynamicType | K::AbstractType => ty = child(ty, t.f_trait)?,
                K::BoundedType => ty = ty.named_child(0)?,
                K::GenericType => {
                    let head = child(ty, t.f_type)?;
                    let htxt = self.cx.text(head);
                    if !(is_deref_wrapper(htxt) || htxt == b"Option") {
                        return None;
                    }
                    ty = self.first_type_arg(child(ty, t.f_type_arguments)?)?.0;
                }
                _ => return None,
            }
        }
        None
    }

    fn fn_type_arity(&self, f: Node<'t>) -> (u8, u8) {
        let Some(params) = child(f, self.t.f_parameters) else { return (0, 255) };
        let mut n = 0u32;
        let mut variadic = false;
        for i in 0..params.named_child_count() as u32 {
            let Some(p) = params.named_child(i) else { break };
            match self.k(p) {
                K::Comment | K::AttributeItem | K::SelfParameter => {}
                K::VariadicParameter => variadic = true,
                _ => n += 1,
            }
        }
        let n = n.min(254) as u8;
        (n, if variadic { 255 } else { n })
    }

    /// Set `type_name`, or the callable-slot flag and arity, of a field / variable definition.
    fn set_decl_type(&mut self, d: u32, ty: Node<'t>) {
        match self.fn_type(ty) {
            Some(f) => {
                let (a, b) = self.fn_type_arity(f);
                let dm = self.cx.def_mut(d);
                dm.flags |= dflag::FNPTR;
                dm.arity_min = a;
                dm.arity_max = b;
            }
            None => {
                let tn = self.type_name(ty, TyUse::Decl);
                self.cx.def_mut(d).type_name = tn;
            }
        }
    }

    /// The type an impl block is for.
    fn impl_type(&mut self, mut ty: Node<'t>) -> ImplTy {
        let t = self.t;
        for _ in 0..8 {
            match self.k(ty) {
                K::TypeIdentifier => {
                    let name = self.ident(ty);
                    if name == self.ids.self_upper && self.self_ty != NONE {
                        return ImplTy::Named(self.self_ty);
                    }
                    return match self.generic(name) {
                        Some(b) => ImplTy::Generic(b),
                        None => ImplTy::Named(name),
                    };
                }
                K::PrimitiveType | K::Identifier => return ImplTy::Named(self.cx.name(ty)),
                K::GenericType | K::ReferenceType | K::HigherRankedTraitBound => match child(ty, t.f_type) {
                    Some(x) => ty = x,
                    None => return ImplTy::Unknown,
                },
                K::ScopedTypeIdentifier => {
                    return match child(ty, t.f_name) {
                        Some(x) => ImplTy::Named(self.ident(x)),
                        None => ImplTy::Unknown,
                    };
                }
                K::DynamicType | K::AbstractType => match child(ty, t.f_trait) {
                    Some(x) => ty = x,
                    None => return ImplTy::Unknown,
                },
                K::BoundedType => match ty.named_child(0) {
                    Some(x) => ty = x,
                    None => return ImplTy::Unknown,
                },
                K::FunctionType => match child(ty, t.f_trait) {
                    Some(x) => ty = x,
                    None => return ImplTy::Named(self.cx.intern(b"fn")),
                },
                // Types without a name of their own get the name rustdoc files them under.
                K::PointerType => return ImplTy::Named(self.cx.intern(b"pointer")),
                K::ArrayType => {
                    let name: &[u8] = if child(ty, t.f_length).is_some() { b"array" } else { b"slice" };
                    return ImplTy::Named(self.cx.intern(name));
                }
                K::TupleType => return ImplTy::Named(self.cx.intern(b"tuple")),
                K::UnitType => return ImplTy::Named(self.cx.intern(b"unit")),
                K::NeverType => return ImplTy::Named(self.cx.intern(b"never")),
                _ => return ImplTy::Unknown,
            }
        }
        ImplTy::Unknown
    }

    /// The identifier that names a (possibly generic or scoped) type.
    fn type_head(&self, mut ty: Node<'t>) -> Option<Node<'t>> {
        let t = self.t;
        for _ in 0..4 {
            match self.k(ty) {
                K::TypeIdentifier => return Some(ty),
                K::GenericType => ty = child(ty, t.f_type)?,
                K::ScopedTypeIdentifier => ty = child(ty, t.f_name)?,
                _ => return None,
            }
        }
        None
    }

    // ----- wrapper forms -----

    /// Split a wrapper form `W<Inner>` into its halves.
    fn split(&self, ty: u32) -> Option<(&[u8], &[u8])> {
        if ty == NONE {
            return None;
        }
        let b = self.cx.name_bytes(ty);
        if b.last() != Some(&b'>') {
            return None;
        }
        let i = memchr::memchr(b'<', b)?;
        Some((&b[..i], &b[i + 1..b.len() - 1]))
    }

    /// The qualifier a member access gets on a value of type `ty`: the type itself, or the
    /// wrapper for wrapper forms.
    fn outer(&mut self, ty: u32) -> u32 {
        let mut tmp = [0u8; 48];
        let len = match self.split(ty) {
            None => return ty,
            Some((o, _)) => {
                // A guard or an iterator: what it wraps is known, what it is is not.
                if o == b"?" || o == b"iter" || o.len() > tmp.len() {
                    return NONE;
                }
                tmp[..o.len()].copy_from_slice(o);
                o.len()
            }
        };
        self.cx.intern(&tmp[..len])
    }

    /// Inner type of a wrapper form whose outer name satisfies `pred`.
    fn inner_if(&mut self, ty: u32, pred: fn(&[u8]) -> bool) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        if let Some((o, i)) = self.split(ty) {
            if pred(o) {
                buf.extend_from_slice(i);
            }
        }
        let id = if buf.is_empty() { NONE } else { self.cx.intern(&buf) };
        self.cx.s.buf = buf;
        id
    }

    /// Re-wrap the inner type of a wrapper form: `Vec<T>` -> `Option<T>`.
    fn rewrap(&mut self, ty: u32, pred: fn(&[u8]) -> bool, outer: &[u8]) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        if let Some((o, i)) = self.split(ty) {
            if pred(o) {
                buf.extend_from_slice(outer);
                buf.push(b'<');
                buf.extend_from_slice(i);
                buf.push(b'>');
            }
        }
        let id = if buf.is_empty() { NONE } else { self.cx.intern(&buf) };
        self.cx.s.buf = buf;
        id
    }

    /// Type path of field `field` of a value of type `base`.
    fn field_path(&mut self, base: u32, field: &[u8]) -> u32 {
        if base == NONE {
            return NONE;
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let b = self.cx.name_bytes(base);
        // Wrapper forms have no fields of their own.
        let id = if b.last() == Some(&b'>') {
            NONE
        } else {
            buf.extend_from_slice(b);
            buf.extend_from_slice(b"::");
            buf.extend_from_slice(field);
            self.cx.intern(&buf)
        };
        self.cx.s.buf = buf;
        id
    }

    /// Slot name `Type.field` (`.field` when the owner type is unknown).
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

    // ----- expression types -----

    /// Type of an expression as far as it can be told without resolving names: a type name, a
    /// type path (`T::field`, `$STATIC::field`, `func()`) or a wrapper form (`Option<T>`).
    fn ty_of(&mut self, e: Node<'t>, depth: u32) -> u32 {
        let t = self.t;
        if depth > 8 {
            return NONE;
        }
        match self.k(e) {
            K::SelfKw => self.self_ty,
            K::Identifier => {
                let name = self.ident(e);
                match self.cx.local_type(name) {
                    Some(ty) => ty,
                    None => {
                        let txt = self.cx.text(e);
                        if (upper(txt) && !self.is_stop(name) && self.generic(name).is_none()) || self.imported(name) {
                            let mut buf = std::mem::take(&mut self.cx.s.buf);
                            buf.clear();
                            buf.push(b'$');
                            buf.extend_from_slice(self.cx.name_bytes(name));
                            let id = self.cx.intern(&buf);
                            self.cx.s.buf = buf;
                            id
                        } else {
                            NONE
                        }
                    }
                }
            }
            K::FieldExpression => {
                let (Some(v), Some(f)) = self.field_parts(e) else { return NONE };
                if self.k(f) != K::FieldIdentifier {
                    return NONE;
                }
                let base = self.ty_of(v, depth + 1);
                self.field_path(base, self.cx.text(f))
            }
            K::ParenthesizedExpression | K::UnaryExpression => match e.named_child(0) {
                Some(x) => self.ty_of(x, depth + 1),
                None => NONE,
            },
            K::ReferenceExpression => match child(e, t.f_value) {
                Some(x) => self.ty_of(x, depth + 1),
                None => NONE,
            },
            K::TryExpression => {
                let Some(x) = e.named_child(0) else { return NONE };
                let ctor = self.ctor_call(x);
                if ctor != NONE {
                    return ctor;
                }
                let ty = self.ty_of(x, depth + 1);
                self.inner_if(ty, is_unwrappable)
            }
            K::IndexExpression => match e.named_child(0) {
                Some(x) => {
                    let ty = self.ty_of(x, depth + 1);
                    self.inner_if(ty, is_sequence)
                }
                None => NONE,
            },
            K::TypeCastExpression => match child(e, t.f_type) {
                Some(x) => self.type_name(x, TyUse::Local),
                None => NONE,
            },
            K::StructExpression => match child(e, t.f_name) {
                Some(x) => self.struct_type(x),
                None => NONE,
            },
            K::CallExpression => {
                let Some(mut f) = child(e, t.f_function) else { return NONE };
                if self.k(f) == K::GenericFunction {
                    match child(f, t.f_function) {
                        Some(x) => f = x,
                        None => return NONE,
                    }
                }
                match self.k(f) {
                    K::Identifier => {
                        // A free function: its return type is looked up at query time.
                        let txt = self.cx.text(f);
                        let name = self.ident(f);
                        if upper(txt) || self.cx.is_local(name) {
                            return NONE;
                        }
                        let mut buf = std::mem::take(&mut self.cx.s.buf);
                        buf.clear();
                        buf.extend_from_slice(self.cx.name_bytes(name));
                        buf.extend_from_slice(b"()");
                        let id = self.cx.intern(&buf);
                        self.cx.s.buf = buf;
                        id
                    }
                    K::ScopedIdentifier => self.path_call_type(f, e, depth, false),
                    K::FieldExpression => self.method_type(f, depth),
                    _ => NONE,
                }
            }
            K::ScopedIdentifier => self.path_value_type(e),
            K::MacroInvocation => match child(e, t.f_macro) {
                Some(m) => match self.cx.text(m) {
                    b"vec" => self.cx.intern(b"Vec"),
                    b"format" => self.cx.intern(b"String"),
                    _ => NONE,
                },
                None => NONE,
            },
            // `"text"` and `r"text"` are `str`, `c"text"` is a `CStr`, `b"text"` a byte array.
            K::StringLiteral => match self.cx.text(e).first() {
                Some(b'"') | Some(b'r') => self.cx.intern(b"str"),
                Some(b'c') => self.cx.intern(b"CStr"),
                _ => NONE,
            },
            _ => NONE,
        }
    }

    /// `(value, field)` of a field expression.
    #[inline]
    fn field_parts(&self, n: Node<'t>) -> (Option<Node<'t>>, Option<Node<'t>>) {
        if n.child_count() == 3 { (n.child(0), n.child(2)) } else { (child(n, self.t.f_value), child(n, self.t.f_field)) }
    }

    /// `(path, name)` of a scoped identifier.
    #[inline]
    fn scoped_parts(&self, n: Node<'t>) -> (Option<Node<'t>>, Option<Node<'t>>) {
        match n.child_count() {
            3 => (n.child(0), n.child(2)),
            2 => (None, n.child(1)),
            _ => (child(n, self.t.f_path), child(n, self.t.f_name)),
        }
    }

    /// The qualifier `path` gives a name written after it, and whether it names a type (then
    /// the name is an associated item) rather than a module. `None` for generic parameters and
    /// `<T as Trait>` forms, whose items no definition can be named for.
    fn qual_head(&mut self, path: Node<'t>) -> Option<(u32, bool, &'a [u8])> {
        let t = self.t;
        let head = match self.k(path) {
            K::Identifier | K::SelfKw | K::Crate | K::Super => path,
            K::ScopedIdentifier => child(path, t.f_name)?,
            K::GenericType => {
                let x = child(path, t.f_type)?;
                if matches!(self.k(x), K::ScopedIdentifier | K::ScopedTypeIdentifier) { child(x, t.f_name)? } else { x }
            }
            _ => return None,
        };
        let htxt = self.cx.text(head);
        let mut is_type = upper(htxt);
        if matches!(self.k(head), K::Identifier | K::TypeIdentifier) {
            let id = self.ident(head);
            if id == self.ids.self_upper {
                if self.self_ty == NONE {
                    return None;
                }
            } else if self.generic(id).is_some() {
                return None;
            } else if !is_type && is_primitive(htxt) && !self.imported(id) {
                // `u32::from_str_radix(..)`, `str::from_utf8(..)`
                is_type = true;
            }
        }
        let q = self.path_qual(path);
        if q == NONE { None } else { Some((q, is_type, htxt)) }
    }

    /// Interned concatenation `a` + `sep` + `b` + `tail`.
    fn join(&mut self, a: u32, sep: &[u8], b: &[u8], tail: &[u8]) -> u32 {
        let mut tmp = std::mem::take(&mut self.b.tmp);
        tmp.clear();
        tmp.extend_from_slice(self.cx.name_bytes(a));
        tmp.extend_from_slice(sep);
        tmp.extend_from_slice(b);
        tmp.extend_from_slice(tail);
        let id = self.cx.intern(&tmp);
        self.b.tmp = tmp;
        id
    }

    /// Type of the value `path::name(..)` returns. A conventional constructor of a type
    /// (`T::new(..)`, `T::default()`, `T::from(..)`, `T::with_capacity(..)`) builds a `T`; any
    /// other associated function gives the type path `T::name()` and a module-level function
    /// `module|name()`, whose declared results are read at query time. With `ctor_only`, only
    /// the first form counts.
    fn path_call_type(&mut self, f: Node<'t>, call: Node<'t>, depth: u32, ctor_only: bool) -> u32 {
        let t = self.t;
        let (Some(path), Some(name)) = self.scoped_parts(f) else { return NONE };
        if self.k(name) != K::Identifier {
            return NONE;
        }
        let ntxt = self.cx.text(name);
        let ctor = is_ctor_name(ntxt);
        if ctor_only && !ctor {
            return NONE;
        }
        let Some((q, is_type, htxt)) = self.qual_head(path) else { return NONE };
        if !is_type {
            if ctor_only {
                return NONE;
            }
            // `module::Tuple(..)` builds a tuple struct; `module::make()` returns what it declares.
            return if upper(ntxt) { self.join(q, b"|", ntxt, b"") } else { self.join(q, b"|", ntxt, b"()") };
        }
        if ntxt == b"new" && (is_deref_wrapper(htxt) || is_wrapper(htxt)) {
            // `Box::new(x)` is an `x`; `RefCell::new(x)` wraps one.
            let arg = child(call, t.f_arguments).and_then(|a| a.named_child(0));
            let inner = match arg {
                Some(a) => self.ty_of(a, depth + 1),
                None => NONE,
            };
            if is_deref_wrapper(htxt) {
                return inner;
            }
            if inner != NONE {
                let mut buf = std::mem::take(&mut self.cx.s.buf);
                buf.clear();
                buf.extend_from_slice(htxt);
                buf.push(b'<');
                buf.extend_from_slice(self.cx.name_bytes(inner));
                buf.push(b'>');
                let w = self.cx.intern(&buf);
                self.cx.s.buf = buf;
                return w;
            }
        }
        // `Enum::Variant(..)` is a value of the enum.
        if ctor || upper(ntxt) { q } else { self.join(q, b"::", ntxt, b"()") }
    }

    /// Constructor call whose result may still be wrapped in a `Result` / `Option`.
    fn ctor_call(&mut self, e: Node<'t>) -> u32 {
        if self.k(e) != K::CallExpression {
            return NONE;
        }
        match child(e, self.t.f_function) {
            Some(f) if self.k(f) == K::ScopedIdentifier => self.path_call_type(f, e, 4, true),
            _ => NONE,
        }
    }

    /// Type of a path used as a value: `Enum::Variant` is a value of the enum, `Type::CONST`
    /// has the declared type of that associated constant, `module::STATIC` is a variable of
    /// another module.
    fn path_value_type(&mut self, e: Node<'t>) -> u32 {
        let (Some(path), Some(name)) = self.scoped_parts(e) else { return NONE };
        let ntxt = self.cx.text(name);
        if self.k(name) != K::Identifier || !upper(ntxt) {
            return NONE;
        }
        let Some((q, is_type, _)) = self.qual_head(path) else { return NONE };
        if !is_type {
            return self.join(q, b"|$", ntxt, b"");
        }
        if ntxt.iter().any(|b| b.is_ascii_lowercase()) { q } else { self.join(q, b"::", ntxt, b"") }
    }

    /// Type of a method call `value.method(..)`: what follows from the receiver's type for the
    /// well-known methods of the standard wrappers, else the type path `T::method()`, the
    /// declared result of that method.
    fn method_type(&mut self, f: Node<'t>, depth: u32) -> u32 {
        let (Some(v), Some(m)) = self.field_parts(f) else { return NONE };
        if self.k(m) != K::FieldIdentifier {
            return NONE;
        }
        let mtxt = self.cx.text(m);
        if matches!(mtxt, b"unwrap" | b"expect" | b"unwrap_or" | b"unwrap_or_else" | b"unwrap_or_default" | b"unwrap_unchecked") {
            let ctor = self.ctor_call(v);
            if ctor != NONE {
                return ctor;
            }
        }
        let ty = self.ty_of(v, depth + 1);
        if ty == NONE {
            return NONE;
        }
        let known = match mtxt {
            b"clone" => ty,
            b"unwrap" | b"expect" | b"unwrap_or" | b"unwrap_or_else" | b"unwrap_or_default" | b"unwrap_unchecked" => self.inner_if(ty, is_unwrappable),
            // `Option` to `Option`.
            b"as_ref" | b"as_mut" | b"as_deref" | b"as_deref_mut" => {
                if self.split(ty).is_some_and(|(o, _)| is_unwrappable(o)) {
                    ty
                } else {
                    NONE
                }
            }
            // Iterators: only what they yield is known (to `for` loops and `next()`).
            b"iter" | b"iter_mut" | b"into_iter" | b"drain" => self.rewrap(ty, is_sequence, b"iter"),
            b"take" | b"cloned" | b"copied" | b"filter" => {
                if self.split(ty).is_some_and(|(o, _)| o == b"iter" || o == b"Option") {
                    ty
                } else {
                    NONE
                }
            }
            b"by_ref" | b"rev" | b"skip" | b"peekable" => {
                if self.split(ty).is_some_and(|(o, _)| o == b"iter") {
                    ty
                } else {
                    NONE
                }
            }
            b"next" | b"peek" | b"next_back" => self.rewrap(ty, |o| o == b"iter", b"Option"),
            b"borrow" | b"borrow_mut" => self.inner_if(ty, |o| o == b"RefCell"),
            // The guard may come wrapped in a `LockResult` (std) or bare (parking_lot): only a
            // following `unwrap()` tells.
            b"lock" | b"read" | b"write" => self.rewrap(ty, |o| matches!(o, b"Mutex" | b"RwLock"), b"?"),
            b"first" | b"last" | b"get" | b"pop" | b"first_mut" | b"last_mut" | b"get_mut" => self.rewrap(ty, is_sequence, b"Option"),
            _ => NONE,
        };
        if known != NONE {
            return known;
        }
        // A method of a standard wrapper, or one that only the standard types have: its result
        // is not something a definition of this repository declares.
        if self.split(ty).is_some() || is_std_method(mtxt) {
            return NONE;
        }
        self.join(ty, b"::", mtxt, b"()")
    }

    /// Qualifier for a member access on `value`.
    fn recv_qual(&mut self, value: Node<'t>) -> u32 {
        match self.k(value) {
            K::SelfKw => self.self_ty,
            _ => {
                let ty = self.ty_of(value, 0);
                self.outer(ty)
            }
        }
    }

    /// Type named by the head of a struct expression or struct pattern, for field qualifiers:
    /// the struct, or the variant of `Enum::Variant { .. }`.
    fn struct_head(&mut self, n: Node<'t>) -> u32 {
        let t = self.t;
        match self.k(n) {
            K::TypeIdentifier | K::Identifier => {
                let name = self.ident(n);
                if name == self.ids.self_upper {
                    self.self_ty
                } else if self.generic(name).is_some() {
                    NONE
                } else {
                    name
                }
            }
            K::ScopedTypeIdentifier | K::ScopedIdentifier => {
                let (path, name) = self.scoped_parts(n);
                let Some(nm) = name else { return NONE };
                // `module::Type { .. }` keeps its module; a variant (`Enum::Variant { .. }`) is
                // known by its own name.
                match self.module_type_id(path, nm) {
                    NONE => self.variant_path(path, nm),
                    id => id,
                }
            }
            K::GenericTypeWithTurbofish | K::GenericType => match child(n, t.f_type) {
                Some(x) => self.struct_head(x),
                None => NONE,
            },
            _ => NONE,
        }
    }

    /// `Enum::Variant` (with the enum's module, `m|Enum::Variant`) for the head of a struct-like
    /// variant: a variant is not a type of its own, and its bare name may be the name of an
    /// unrelated type the file sees. Falls back to the bare name when the enum is not known.
    fn variant_path(&mut self, path: Option<Node<'t>>, nm: Node<'t>) -> u32 {
        let t = self.t;
        let bare = self.ident(nm);
        let Some(p) = path else { return bare };
        let head = match self.k(p) {
            K::Identifier => Some(p),
            K::ScopedIdentifier => child(p, t.f_name),
            _ => None,
        };
        let Some(h) = head else { return bare };
        if !upper(self.cx.text(h)) || self.generic_ident(h) {
            return bare;
        }
        let q = self.path_qual(p);
        if q == NONE || q == self.ids.self_upper {
            return bare;
        }
        let mut tmp = std::mem::take(&mut self.b.tmp);
        tmp.clear();
        tmp.extend_from_slice(self.cx.name_bytes(q));
        tmp.extend_from_slice(b"::");
        tmp.extend_from_slice(self.cx.text(nm));
        let id = self.cx.intern(&tmp);
        self.b.tmp = tmp;
        id
    }

    /// Type of the value a struct expression builds: for `Enum::Variant { .. }` that is the enum.
    fn struct_type(&mut self, n: Node<'t>) -> u32 {
        let t = self.t;
        if self.k(n) == K::ScopedTypeIdentifier {
            if let Some(p) = child(n, t.f_path) {
                let head = match self.k(p) {
                    K::Identifier => Some(p),
                    K::ScopedIdentifier => child(p, t.f_name),
                    _ => None,
                };
                if let Some(h) = head {
                    if upper(self.cx.text(h)) {
                        // The path names the enum, possibly through a module.
                        return if self.generic_ident(h) { NONE } else { self.path_qual(p) };
                    }
                }
            }
        }
        self.struct_head(n)
    }

    fn generic_ident(&mut self, n: Node<'t>) -> bool {
        if self.b.generics.is_empty() {
            return false;
        }
        let id = self.ident(n);
        self.generic(id).is_some()
    }

    // ----- paths -----

    /// Qualifier of a scoped name: the path before it, with `Self` replaced by the impl's type,
    /// a first segment bound by a `use` of this file expanded to what it stands for, and a type
    /// that follows a module path set off with `|` (`crate::a::b|Type`).
    fn path_qual(&mut self, p: Node<'t>) -> u32 {
        match self.k(p) {
            K::Identifier => {
                let name = self.ident(p);
                if name == self.ids.self_upper {
                    if self.self_ty != NONE { self.self_ty } else { name }
                } else {
                    match self.generic(name) {
                        Some(b) => b,
                        None if upper(self.cx.text(p)) => name,
                        None => self.import_target(name).unwrap_or(name),
                    }
                }
            }
            K::SelfKw | K::Crate | K::Super => self.cx.name(p),
            _ => {
                let mut tmp = std::mem::take(&mut self.b.tmp);
                tmp.clear();
                let ok = self.path_text(p, &mut tmp, 0);
                let id = if ok {
                    set_off_type(&mut tmp);
                    if tmp.is_empty() { NONE } else { self.cx.intern(&tmp) }
                } else {
                    NONE
                };
                self.b.tmp = tmp;
                id
            }
        }
    }

    /// `module|Name` for a type written `module::Name` (`path` is what precedes the name);
    /// `NONE` when the path goes through a type (`T::Item`, `Enum::Variant`): no fixed name.
    fn module_type(&mut self, path: Node<'t>, name: &[u8], buf: &mut Vec<u8>) -> bool {
        let mark = buf.len();
        if !self.path_text(path, buf, 0) {
            buf.truncate(mark);
            return false;
        }
        if let Some(rest) = buf[mark..].strip_prefix(b"::") {
            let n = rest.len();
            buf.copy_within(mark + 2.., mark);
            buf.truncate(mark + n);
        }
        if buf.len() == mark || path_type_at(&buf[mark..]).is_some() {
            buf.truncate(mark);
            return false;
        }
        buf.push(b'|');
        buf.extend_from_slice(name);
        true
    }

    /// Interned `module|Name` (see `module_type`), or the bare name without a path.
    fn module_type_id(&mut self, path: Option<Node<'t>>, name: Node<'t>) -> u32 {
        let Some(path) = path else { return self.ident(name) };
        let mut tmp = std::mem::take(&mut self.b.tmp);
        tmp.clear();
        let txt = self.cx.text(name);
        let id = if self.module_type(path, txt, &mut tmp) { self.cx.intern(&tmp) } else { NONE };
        self.b.tmp = tmp;
        id
    }

    fn path_text(&mut self, p: Node<'t>, buf: &mut Vec<u8>, depth: u32) -> bool {
        let t = self.t;
        if depth > 16 {
            return false;
        }
        match self.k(p) {
            // Always the first segment of the path: the names in a path's `name` position are
            // appended as text below.
            K::Identifier | K::TypeIdentifier => {
                let name = self.ident(p);
                if name == self.ids.self_upper && self.self_ty != NONE {
                    buf.extend_from_slice(self.cx.name_bytes(self.self_ty));
                } else if upper(self.cx.text(p)) {
                    buf.extend_from_slice(self.cx.name_bytes(name));
                } else {
                    // `use crate::query as q` then `q::run()`: the module `q` stands for.
                    let id = self.import_target(name).unwrap_or(name);
                    buf.extend_from_slice(self.cx.name_bytes(id));
                }
                true
            }
            K::SelfKw | K::Crate | K::Super | K::PrimitiveType => {
                buf.extend_from_slice(self.cx.text(p));
                true
            }
            K::Metavariable => {
                let txt = self.cx.text(p);
                buf.extend_from_slice(if txt == b"$crate" { b"crate" } else { txt });
                true
            }
            K::ScopedIdentifier | K::ScopedTypeIdentifier => {
                let (path, name) = self.scoped_parts(p);
                let Some(name) = name else { return false };
                if let Some(path) = path {
                    if !self.path_text(path, buf, depth + 1) {
                        return false;
                    }
                }
                buf.extend_from_slice(b"::");
                buf.extend_from_slice(self.cx.text(name));
                true
            }
            K::GenericType | K::GenericTypeWithTurbofish => match child(p, t.f_type) {
                Some(x) => self.path_text(x, buf, depth + 1),
                None => false,
            },
            K::BracketedType => {
                let Some(inner) = p.named_child(0) else { return false };
                if self.k(inner) == K::QualifiedType {
                    // `<T as Trait>::name`: the trait declares the name.
                    return match child(inner, t.f_alias) {
                        Some(tr) => self.path_text(tr, buf, depth + 1),
                        None => false,
                    };
                }
                // `<[T]>::name`, `<Foo<T>>::name`: the type owns the name.
                let id = match self.impl_type(inner) {
                    ImplTy::Named(id) => id,
                    ImplTy::Generic(bound) => bound,
                    ImplTy::Unknown => NONE,
                };
                if id == NONE {
                    return false;
                }
                buf.extend_from_slice(self.cx.name_bytes(id));
                true
            }
            _ => false,
        }
    }

    /// A path segment that qualifies a name: a type when capitalised.
    fn qualifier_ref(&mut self, n: Node<'t>, qual: u32) {
        if !upper(self.cx.text(n)) {
            return;
        }
        let name = self.ident(n);
        if self.is_stop(name) || self.generic(name).is_some() {
            return;
        }
        self.cx.add_ref(name, RefKind::Type, line_of(n), qual, 255);
    }

    /// The cursor is on the path in front of a scoped name. Emits a type reference when the
    /// innermost qualifier names a type, and visits generic arguments written inside the path.
    fn path_prefix(&mut self, c: &mut TreeCursor<'t>, innermost: bool) {
        let n = c.node();
        match self.k(n) {
            K::Identifier => {
                if innermost {
                    self.qualifier_ref(n, NONE);
                }
            }
            K::ScopedIdentifier => {
                let (path, name) = self.scoped_parts(n);
                if innermost {
                    if let Some(nm) = name {
                        if self.k(nm) == K::Identifier && upper(self.cx.text(nm)) {
                            let q = match path {
                                Some(p) => self.path_qual(p),
                                None => NONE,
                            };
                            self.qualifier_ref(nm, q);
                        }
                    }
                }
                if path.is_some_and(|p| matches!(self.k(p), K::ScopedIdentifier | K::GenericType | K::BracketedType)) {
                    self.path_first(c, false);
                }
            }
            K::GenericType | K::BracketedType => self.visit(c),
            _ => {}
        }
    }

    /// Descend to the path child of a scoped name and process it.
    fn path_first(&mut self, c: &mut TreeCursor<'t>, innermost: bool) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        if c.goto_first_child() {
            loop {
                let k = self.k(c.node());
                if k != K::Comment {
                    if k != K::Tok {
                        self.path_prefix(c, innermost);
                    }
                    break;
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.cx.depth -= 1;
    }

    /// `a::b::name` in expression or pattern position.
    fn scoped_identifier(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: RefKind, nargs: u8) {
        let (path, name_node) = self.scoped_parts(n);
        if let Some(nm) = name_node {
            if self.k(nm) == K::Identifier {
                let name = self.ident(nm);
                let qual = match path {
                    Some(p) => self.path_qual(p),
                    None => NONE,
                };
                if !self.is_prelude_variant(name, qual) {
                    self.cx.add_ref(name, kind, line_of(nm), qual, nargs);
                }
            }
        }
        if path.is_some() {
            self.path_first(c, true);
        }
    }

    /// `a::b::Name` in type position (also the head of struct expressions and patterns).
    /// Returns the name.
    fn scoped_type(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: RefKind) -> u32 {
        let (path, name_node) = self.scoped_parts(n);
        let mut name = NONE;
        if let Some(nm) = name_node {
            name = self.ident(nm);
            let qual = match path {
                Some(p) => self.path_qual(p),
                None => NONE,
            };
            self.cx.add_ref(name, kind, line_of(nm), qual, 255);
        }
        if path.is_some() {
            self.path_first(c, true);
        }
        name
    }

    /// A type used as a named head (implemented trait, supertrait): `kind` instead of `Type`
    /// for the head, generic arguments visited as usual.
    fn head_ref(&mut self, c: &mut TreeCursor<'t>, kind: RefKind) {
        let n = c.node();
        match self.k(n) {
            K::TypeIdentifier => {
                let name = self.ident(n);
                if name != self.ids.self_upper && self.generic(name).is_none() {
                    self.cx.add_ref(name, kind, line_of(n), NONE, 255);
                }
            }
            K::ScopedTypeIdentifier => {
                self.scoped_type(n, c, kind);
            }
            K::GenericType => self.each(c, |w, c| match w.k(c.node()) {
                K::TypeIdentifier | K::ScopedTypeIdentifier => w.head_ref(c, kind),
                _ => w.visit(c),
            }),
            _ => self.visit(c),
        }
    }

    /// `Item = T` inside generic arguments: the associated type's name is not a reference.
    fn type_binding(&mut self, c: &mut TreeCursor<'t>) {
        let mut name_seen = false;
        self.each(c, |w, c| {
            if !name_seen && w.k(c.node()) == K::TypeIdentifier {
                name_seen = true;
            } else {
                w.visit(c);
            }
        });
    }

    /// Generic arguments. `Item: Bound` (an associated type bound) names an associated type of
    /// the trait, not a type of its own: only the bounds are references.
    fn type_arguments(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            let n = c.node();
            if w.k(n) == K::TypeIdentifier {
                let rest = &w.cx.src[n.end_byte()..];
                let i = rest.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(rest.len());
                if rest.get(i) == Some(&b':') && rest.get(i + 1) != Some(&b':') {
                    return;
                }
            }
            w.visit(c);
        });
    }

    /// Parameter list of a function type: only the types matter.
    fn type_parameters_list(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            if w.k(c.node()) == K::Parameter {
                let mut colon = false;
                w.each(c, |w, c| {
                    let ch = c.node();
                    if colon {
                        w.visit(c);
                    } else if w.k(ch) == K::Tok && w.tk(ch) == Tk::Colon {
                        colon = true;
                    }
                });
            } else {
                w.visit(c);
            }
        });
    }

    // ----- definitions -----

    fn function(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, has_body: bool) {
        let t = self.t;
        let at = self.take_attrs();
        let name_node = match child(n, t.f_name) {
            Some(x) if self.k(x) == K::Identifier => x,
            _ => {
                self.flush_attrs();
                return self.children(c);
            }
        };
        let body = if has_body { child(n, t.f_body) } else { None };
        let sig_end = match body {
            Some(b) => b.start_byte(),
            None => self.trim_semi(n),
        };
        let name = self.ident(name_node);
        let outer = self.scope;
        let kind = if matches!(outer, Scope::Impl | Scope::Trait) { DefKind::Method } else { DefKind::Function };
        let exported = self.has_vis(n) || (outer == Scope::Trait && self.inherit_pub);
        let d = self.new_def(name, kind, n, sig_end, at, exported);
        if body.is_none() {
            self.cx.def_mut(d).flags |= if outer == Scope::Trait { dflag::DECL | dflag::ABSTRACT } else { dflag::DECL };
        }
        let saved = self.enter(d, Scope::Fn);
        if at.test || at.cfg_test {
            self.test_ctx = true;
        }
        self.cx.push_scope();
        self.fn_depth += 1;
        let gmark = self.b.generics.len();
        let wc = self.where_clause_of(n);
        let mut nparams = 0u32;
        let mut variadic = false;
        let mut is_async = false;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Tok | K::Identifier | K::Metavariable | K::VisibilityModifier | K::Comment => {}
                K::FunctionModifiers => is_async = w.has_token(ch, Tk::Async),
                K::TypeParameters => {
                    w.type_parameters(c);
                    if let Some(wc) = wc {
                        w.where_bounds(wc);
                    }
                }
                K::Parameters => (nparams, variadic) = w.fn_parameters(c),
                _ => w.visit(c),
            }
        });
        let ty = match child(n, t.f_return_type) {
            Some(r) => self.type_name(r, TyUse::Ret),
            None => NONE,
        };
        {
            let dm = self.cx.def_mut(d);
            dm.arity_min = nparams.min(254) as u8;
            dm.arity_max = if variadic { 255 } else { dm.arity_min };
            dm.type_name = ty;
            if is_async {
                dm.flags |= dflag::ASYNC;
            }
        }
        self.b.generics.truncate(gmark);
        self.fn_depth -= 1;
        self.cx.pop_scope();
        self.leave(d, saved);
    }

    fn has_token(&self, n: Node<'t>, tk: Tk) -> bool {
        (0..n.child_count()).any(|i| n.child(i).is_some_and(|x| self.k(x) == K::Tok && self.tk(x) == tk))
    }

    /// Parameters of a function item: declares them as locals. Returns their number (without
    /// the `self` receiver) and whether the list is variadic.
    fn fn_parameters(&mut self, c: &mut TreeCursor<'t>) -> (u32, bool) {
        let mut n = 0u32;
        let mut variadic = false;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Parameter => {
                    if w.parameter(ch, c) {
                        n += 1;
                    }
                }
                K::VariadicParameter => {
                    variadic = true;
                    // `mut args: ...`
                    if let Some(p) = ch.named_child(0).filter(|p| w.k(*p) == K::Identifier) {
                        let name = w.ident(p);
                        w.cx.declare(name, NONE);
                    }
                }
                K::Tok | K::SelfParameter | K::AttributeItem | K::Comment => {}
                _ => {
                    // Anonymous parameter: just a type.
                    n += 1;
                    w.visit(c);
                }
            }
        });
        (n, variadic)
    }

    /// `pattern: Type`. Returns false for a typed `self` receiver.
    fn parameter(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) -> bool {
        let ty = match child(n, self.t.f_type) {
            Some(x) => self.type_name(x, TyUse::Local),
            None => NONE,
        };
        let mut is_self = false;
        let mut colon = false;
        let mark = self.b.binds.len();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Tok => {
                    if w.tk(ch) == Tk::Colon {
                        colon = true;
                    }
                }
                K::MutableSpecifier | K::Comment | K::AttributeItem => {}
                K::SelfKw if !colon => is_self = true,
                _ if !colon => w.pattern(c, ty),
                _ => w.visit(c),
            }
        });
        self.declare_binds(mark);
        !is_self
    }

    fn closure_parameters(&mut self, c: &mut TreeCursor<'t>) {
        let mark = self.b.binds.len();
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Tok | K::Comment => {}
                K::Parameter => {
                    w.parameter(ch, c);
                }
                _ => w.pattern(c, NONE),
            }
        });
        self.declare_binds(mark);
    }

    fn struct_item(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: DefKind) {
        let t = self.t;
        let at = self.take_attrs();
        let Some(nm) = child(n, t.f_name) else {
            self.flush_attrs();
            return self.children(c);
        };
        let sig_end = match child(n, t.f_body) {
            Some(b) if self.k(b) == K::FieldDeclarationList => b.start_byte(),
            _ => self.trim_semi(n),
        };
        let name = self.ident(nm);
        let d = self.new_def(name, kind, n, sig_end, at, self.has_vis(n));
        let saved = self.enter(d, Scope::Type);
        self.self_ty = name;
        let gmark = self.b.generics.len();
        let mut name_seen = false;
        self.each(c, |w, c| match w.k(c.node()) {
            K::TypeIdentifier if !name_seen => name_seen = true,
            K::TypeParameters => w.type_parameters(c),
            _ => w.visit(c),
        });
        self.b.generics.truncate(gmark);
        self.leave(d, saved);
    }

    fn field_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let at = self.take_attrs();
        let Some(nm) = child(n, t.f_name) else {
            self.flush_attrs();
            return self.children(c);
        };
        let name = self.ident(nm);
        let d = self.new_def(name, DefKind::Field, n, n.end_byte(), at, self.has_vis(n) || self.inherit_pub);
        let mut ty = child(n, t.f_type);
        let mut default: Option<Node<'t>> = None;
        if n.has_error() {
            // `name: Type = value` (default field values) is newer than the grammar: the type
            // lands in an error node and the value is taken for the type.
            for i in 0..n.child_count() {
                let Some(e) = n.child(i) else { break };
                if self.k(e) == K::Error && self.cx.text(e).ends_with(b"=") {
                    if let Some(real) = e.named_child(0) {
                        default = ty;
                        ty = Some(real);
                    }
                    break;
                }
            }
        }
        if let Some(ty) = ty {
            self.set_decl_type(d, ty);
        }
        if !self.patched.is_empty() {
            // `name: Type = default`: the default was blanked for the parser.
            let tail = self.blanked_tail(n.end_byte());
            if tail > 0 {
                let dm = self.cx.def_mut(d);
                dm.sig_len = clamp_u16(dm.sig_len as usize + tail);
            }
        }
        let mut colon = false;
        self.each(c, |w, c| {
            let ch = c.node();
            if !colon {
                colon = w.k(ch) == K::Tok && w.tk(ch) == Tk::Colon;
            } else if Some(ch) == default {
                w.value_as_type(ch, c);
            } else {
                w.visit(c);
            }
        });
    }

    /// An expression the parser took for a type (see `field_declaration`): record the call or
    /// path it most likely is.
    fn value_as_type(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let (head, kind, nargs) = match self.k(n) {
            K::FunctionType => {
                let nargs = match child(n, t.f_parameters) {
                    Some(p) => p.named_child_count().min(254) as u8,
                    None => 255,
                };
                (child(n, t.f_trait), RefKind::Call, nargs)
            }
            K::ScopedTypeIdentifier | K::TypeIdentifier => (Some(n), RefKind::Read, 255),
            _ => (None, RefKind::Read, 255),
        };
        let Some(h) = head else { return };
        match self.k(h) {
            K::TypeIdentifier => {
                let name = self.ident(h);
                if !self.is_stop(name) && self.generic(name).is_none() {
                    self.cx.add_ref(name, kind, line_of(h), NONE, nargs);
                }
            }
            K::ScopedTypeIdentifier => {
                let (path, nm) = self.scoped_parts(h);
                if let Some(nm) = nm {
                    let name = self.ident(nm);
                    if !self.is_stop(name) {
                        let qual = match path {
                            Some(p) => self.path_qual(p),
                            None => NONE,
                        };
                        self.cx.add_ref(name, kind, line_of(nm), qual, nargs);
                    }
                }
                if h == n {
                    self.path_first(c, true);
                }
            }
            _ => {}
        }
    }

    fn enum_item(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let at = self.take_attrs();
        let Some(nm) = child(n, t.f_name) else {
            self.flush_attrs();
            return self.children(c);
        };
        let sig_end = match child(n, t.f_body) {
            Some(b) => b.start_byte(),
            None => n.end_byte(),
        };
        let name = self.ident(nm);
        let exported = self.has_vis(n);
        let d = self.new_def(name, DefKind::Enum, n, sig_end, at, exported);
        let saved = self.enter(d, Scope::Type);
        self.self_ty = name;
        self.inherit_pub = exported;
        let gmark = self.b.generics.len();
        let mut name_seen = false;
        self.each(c, |w, c| match w.k(c.node()) {
            K::TypeIdentifier if !name_seen => name_seen = true,
            K::TypeParameters => w.type_parameters(c),
            _ => w.visit(c),
        });
        self.b.generics.truncate(gmark);
        self.leave(d, saved);
    }

    fn enum_variant(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let at = self.take_attrs();
        let Some(nm) = child(n, t.f_name) else {
            self.flush_attrs();
            return self.children(c);
        };
        let sig_end = match child(n, t.f_body) {
            Some(b) if self.k(b) == K::FieldDeclarationList => b.start_byte(),
            _ => n.end_byte(),
        };
        let name = self.ident(nm);
        let exported = self.inherit_pub;
        let d = self.new_def(name, DefKind::EnumMember, n, sig_end, at, exported);
        // Fields of a struct-like variant belong to the variant, and are as public as it is.
        let saved = self.enter(d, Scope::Type);
        self.inherit_pub = exported;
        let mut name_seen = false;
        self.each(c, |w, c| match w.k(c.node()) {
            K::Identifier if !name_seen => name_seen = true,
            _ => w.visit(c),
        });
        self.leave(d, saved);
    }

    fn trait_item(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let at = self.take_attrs();
        let Some(nm) = child(n, t.f_name) else {
            self.flush_attrs();
            return self.children(c);
        };
        let sig_end = match child(n, t.f_body) {
            Some(b) => b.start_byte(),
            None => n.end_byte(),
        };
        let name = self.ident(nm);
        let exported = self.has_vis(n);
        let d = self.new_def(name, DefKind::Trait, n, sig_end, at, exported);
        let saved = self.enter(d, Scope::Trait);
        self.self_ty = name;
        self.inherit_pub = exported;
        if at.cfg_test {
            self.test_ctx = true;
        }
        let gmark = self.b.generics.len();
        let mut name_seen = false;
        self.each(c, |w, c| match w.k(c.node()) {
            K::TypeIdentifier if !name_seen => name_seen = true,
            K::TypeParameters => w.type_parameters(c),
            // Supertraits.
            K::TraitBounds => w.each(c, |w, c| w.head_ref(c, RefKind::Extends)),
            _ => w.visit(c),
        });
        self.b.generics.truncate(gmark);
        self.leave(d, saved);
    }

    fn impl_item(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let at = self.take_attrs();
        let gmark = self.b.generics.len();
        // Generic parameters first: the implemented type may be one of them (blanket impl).
        if let Some(tp) = child(n, t.f_type_parameters) {
            self.register_generics(tp);
            if let Some(wc) = self.where_clause_of(n) {
                self.where_bounds(wc);
            }
        }
        let tr = child(n, t.f_trait);
        let body = child(n, t.f_body);
        let implemented = match child(n, t.f_type) {
            Some(x) => self.impl_type(x),
            None => ImplTy::Unknown,
        };
        let tr_name = match tr.and_then(|x| self.type_head(x)) {
            Some(h) => self.ident(h),
            None => NONE,
        };
        // A blanket impl has no type to be named after: it is filed under the trait.
        let (name, self_ty) = match implemented {
            ImplTy::Named(id) => (id, id),
            ImplTy::Generic(bound) => (tr_name, bound),
            ImplTy::Unknown => (tr_name, NONE),
        };
        let d = if name != NONE {
            let sig_end = match body {
                Some(b) => b.start_byte(),
                None => self.trim_semi(n),
            };
            self.new_def(name, DefKind::Impl, n, sig_end, at, false)
        } else {
            self.flush_attrs();
            NONE
        };
        let saved = self.enter(d, Scope::Impl);
        self.self_ty = self_ty;
        if at.cfg_test {
            self.test_ctx = true;
        }
        let outer_deref = self.deref_impl;
        self.deref_impl = d != NONE && tr_name != NONE && self.cx.name_bytes(tr_name) == b"Deref";
        let mut negative = false;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::TypeParameters => w.generic_bounds(c),
                K::Tok => {
                    if w.tk(ch) == Tk::Bang {
                        negative = true;
                    }
                }
                _ if Some(ch) == tr => w.head_ref(c, if negative || d == NONE { RefKind::Type } else { RefKind::Implements }),
                _ => w.visit(c),
            }
        });
        self.deref_impl = outer_deref;
        self.b.generics.truncate(gmark);
        self.leave(d, saved);
    }

    /// `type Name = T;`, and `type Name: Bounds;` in traits and extern blocks.
    fn type_item(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, associated: bool) {
        let t = self.t;
        let at = self.take_attrs();
        let Some(nm) = child(n, t.f_name) else {
            self.flush_attrs();
            return self.children(c);
        };
        let name = self.ident(nm);
        let outer = self.scope;
        let exported = self.has_vis(n) || (outer == Scope::Trait && self.inherit_pub);
        let d = self.new_def(name, DefKind::TypeAlias, n, self.trim_semi(n), at, exported);
        if associated {
            self.cx.def_mut(d).flags |= if outer == Scope::Trait { dflag::DECL | dflag::ABSTRACT } else { dflag::DECL };
        }
        self.cx.push_def(d);
        let gmark = self.b.generics.len();
        let mut name_seen = false;
        self.each(c, |w, c| match w.k(c.node()) {
            K::TypeIdentifier if !name_seen => name_seen = true,
            K::TypeParameters => w.type_parameters(c),
            _ => w.visit(c),
        });
        if !associated {
            if let Some(ty) = child(n, t.f_type) {
                let tn = self.type_name(ty, TyUse::Decl);
                self.cx.def_mut(d).type_name = tn;
                if self.deref_impl && outer == Scope::Impl && tn != NONE && self.cx.name_bytes(name) == b"Target" {
                    // `impl Deref for X { type Target = Y; }`: members of `Y` are reachable
                    // through an `X`, like those of an embedded struct.
                    let encl = self.parent;
                    // `type Target = ffi::CStr`: the module goes into the qualifier.
                    let (tn, tq) = match memchr::memchr(b'|', self.cx.name_bytes(tn)) {
                        Some(i) => {
                            let full = self.cx.name_bytes(tn).to_vec();
                            (self.cx.intern(&full[i + 1..]), self.cx.intern(&full[..i]))
                        }
                        None => (tn, NONE),
                    };
                    self.cx.facts.refs.push(Ref { name: tn, qual: tq, line: line_of(ty), encl, kind: RefKind::Extends, nargs: 255 });
                }
            }
        }
        self.b.generics.truncate(gmark);
        self.cx.pop_def();
    }

    fn const_item(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: DefKind) {
        let t = self.t;
        let at = self.take_attrs();
        let Some(nm) = child(n, t.f_name) else {
            self.flush_attrs();
            return self.children(c);
        };
        let value = child(n, t.f_value);
        let sig_end = match value {
            Some(v) => v.start_byte(),
            None => self.trim_semi(n),
        };
        let name = self.ident(nm);
        let outer = self.scope;
        let exported = self.has_vis(n) || (outer == Scope::Trait && self.inherit_pub);
        let d = self.new_def(name, kind, n, sig_end, at, exported);
        if value.is_none() {
            self.cx.def_mut(d).flags |= if outer == Scope::Trait { dflag::DECL | dflag::ABSTRACT } else { dflag::DECL };
        }
        if let Some(ty) = child(n, t.f_type) {
            self.set_decl_type(d, ty);
        }
        self.cx.push_def(d);
        let mut name_seen = false;
        self.each(c, |w, c| match w.k(c.node()) {
            K::Identifier if !name_seen => name_seen = true,
            _ => w.visit(c),
        });
        self.cx.pop_def();
    }

    fn mod_item(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let at = self.take_attrs();
        let Some(nm) = child(n, t.f_name) else {
            self.flush_attrs();
            return self.children(c);
        };
        let body = child(n, t.f_body);
        let sig_end = match body {
            Some(b) => b.start_byte(),
            None => self.trim_semi(n),
        };
        let name = self.ident(nm);
        let d = self.new_def(name, DefKind::Module, n, sig_end, at, self.has_vis(n));
        if body.is_none() {
            self.cx.def_mut(d).flags |= dflag::DECL;
            return;
        }
        let saved = self.enter(d, Scope::Mod);
        self.self_ty = NONE;
        if at.cfg_test {
            self.test_ctx = true;
        }
        self.each(c, |w, c| {
            if w.k(c.node()) == K::DeclarationList {
                w.children(c);
            }
        });
        self.leave(d, saved);
    }

    /// `extern "C" { ... }`: the declarations inside have no bodies.
    fn foreign_mod(&mut self, c: &mut TreeCursor<'t>) {
        let at = self.take_attrs();
        self.flush_attrs();
        let saved = self.enter(NONE, Scope::Foreign);
        if at.cfg_test {
            self.test_ctx = true;
        }
        self.children(c);
        self.leave(NONE, saved);
    }

    fn macro_definition(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let at = self.take_attrs();
        let Some(nm) = child(n, self.t.f_name) else {
            self.flush_attrs();
            return;
        };
        let name = self.ident(nm);
        let d = self.new_def(name, DefKind::Macro, n, nm.end_byte(), at, at.macro_export);
        // Unknown arity, and callable (an arity of 0..0 would mean "object-like").
        self.cx.def_mut(d).arity_max = 255;
        let saved = self.enter(d, Scope::Macro);
        self.each(c, |w, c| {
            if w.k(c.node()) == K::MacroRule {
                w.each(c, |w, c| {
                    if w.k(c.node()) == K::TokenTree {
                        w.cx.push_scope();
                        w.tokens(c, Tm { body: Body::Code, code: true, defs: true, pat: false }, Init::Plain);
                        w.cx.pop_scope();
                    }
                });
            }
        });
        self.leave(d, saved);
    }

    // ----- imports -----

    fn use_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        self.flush_attrs();
        let flags = if self.has_vis(n) { iflag::REEXPORT } else { 0 };
        let line = line_of(n);
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        self.each(c, |w, c| {
            if !matches!(w.k(c.node()), K::Tok | K::VisibilityModifier | K::Comment) {
                w.use_tree(c, &mut buf, flags, line);
            }
        });
        self.cx.s.buf = buf;
    }

    /// One clause of a use tree; `prefix` holds the path in front of it.
    fn use_tree(&mut self, c: &mut TreeCursor<'t>, prefix: &mut Vec<u8>, flags: u8, line: u32) {
        let t = self.t;
        let n = c.node();
        match self.k(n) {
            K::Identifier | K::SelfKw | K::Super | K::Crate | K::Metavariable => {
                let name = self.cx.text(n);
                self.import_leaf(prefix, name, None, false, flags, line);
            }
            K::ScopedIdentifier => {
                let base = prefix.len();
                let (path, name) = self.scoped_parts(n);
                if let (Some(nm), true) = (name, self.use_prefix(path, prefix)) {
                    let name = self.cx.text(nm);
                    self.import_leaf(prefix, name, None, false, flags, line);
                }
                prefix.truncate(base);
            }
            K::UseAsClause => {
                let (Some(path), alias) = (child(n, t.f_path), child(n, t.f_alias)) else { return };
                let alias = alias.map(|a| self.cx.text(a));
                let base = prefix.len();
                if self.k(path) == K::ScopedIdentifier {
                    let (p, name) = self.scoped_parts(path);
                    if let (Some(nm), true) = (name, self.use_prefix(p, prefix)) {
                        let name = self.cx.text(nm);
                        self.import_leaf(prefix, name, alias, false, flags, line);
                    }
                } else {
                    let name = self.cx.text(path);
                    self.import_leaf(prefix, name, alias, false, flags, line);
                }
                prefix.truncate(base);
            }
            K::UseWildcard => {
                let base = prefix.len();
                if self.use_prefix(n.named_child(0), prefix) {
                    self.import_leaf(prefix, b"", None, true, flags, line);
                }
                prefix.truncate(base);
            }
            K::ScopedUseList => {
                let base = prefix.len();
                if self.use_prefix(child(n, t.f_path), prefix) {
                    self.each(c, |w, c| {
                        if w.k(c.node()) == K::UseList {
                            w.each(c, |w, c| {
                                if !matches!(w.k(c.node()), K::Tok | K::Comment) {
                                    w.use_tree(c, prefix, flags, line);
                                }
                            });
                        }
                    });
                }
                prefix.truncate(base);
            }
            K::UseList => self.each(c, |w, c| {
                if !matches!(w.k(c.node()), K::Tok | K::Comment) {
                    w.use_tree(c, prefix, flags, line);
                }
            }),
            _ => {}
        }
    }

    /// Append a path to the prefix of a use tree.
    fn use_prefix(&mut self, path: Option<Node<'t>>, prefix: &mut Vec<u8>) -> bool {
        let Some(p) = path else { return true };
        if !prefix.is_empty() {
            prefix.extend_from_slice(b"::");
        }
        self.use_path(p, prefix, 0)
    }

    fn use_path(&mut self, p: Node<'t>, buf: &mut Vec<u8>, depth: u32) -> bool {
        if depth > 32 {
            return false;
        }
        match self.k(p) {
            K::Identifier | K::SelfKw | K::Super | K::Crate => {
                buf.extend_from_slice(self.cx.text(p));
                true
            }
            K::Metavariable => {
                let txt = self.cx.text(p);
                buf.extend_from_slice(if txt == b"$crate" { b"crate" } else { txt });
                true
            }
            K::ScopedIdentifier => {
                let (path, name) = self.scoped_parts(p);
                let Some(name) = name else { return false };
                if let Some(path) = path {
                    if !self.use_path(path, buf, depth + 1) {
                        return false;
                    }
                }
                buf.extend_from_slice(b"::");
                buf.extend_from_slice(self.cx.text(name));
                true
            }
            _ => false,
        }
    }

    /// Record one imported name; `prefix` is the module path in front of it.
    fn import_leaf(&mut self, prefix: &[u8], name: &[u8], alias: Option<&[u8]>, wildcard: bool, mut flags: u8, line: u32) {
        let head = prefix.strip_prefix(b"::").unwrap_or(prefix);
        let head = if head.is_empty() { name } else { head };
        let first = head.split(|&b| b == b':').next().unwrap_or(head);
        if matches!(first, b"std" | b"core" | b"alloc") {
            flags |= iflag::SYSTEM;
        }
        // `use a::b::*`, and `use a::b::{self}` which imports `b` itself.
        let (target, name): (&[u8], &[u8]) = if wildcard {
            (prefix, b"")
        } else if name == b"self" && !prefix.is_empty() {
            match memchr::memmem::rfind(prefix, b"::") {
                Some(i) => (&prefix[..i], &prefix[i + 2..]),
                None => (prefix, b""),
            }
        } else if prefix.is_empty() {
            (name, b"")
        } else {
            (prefix, name)
        };
        if target.is_empty() || memchr::memchr(b'$', target).is_some() || name.first() == Some(&b'$') {
            // `use a::$name;` in a macro body: known only at expansion.
            return;
        }
        let target_id = self.cx.intern(target);
        let name_id = if name.is_empty() { NONE } else { self.cx.intern(name) };
        let alias_id = match alias {
            Some(a) => self.cx.intern(a),
            None => NONE,
        };
        if wildcard {
            flags |= iflag::WILDCARD;
        }
        self.cx.add_import(target_id, name_id, alias_id, line, flags);
        // The reference names the imported symbol; for whole-module and glob imports that is
        // the last segment of the path.
        let (ref_name, ref_qual) = if name_id != NONE {
            (name_id, target_id)
        } else {
            match memchr::memmem::rfind(target, b"::") {
                Some(i) => (self.cx.intern(&target[i + 2..]), if i == 0 { NONE } else { self.cx.intern(&target[..i]) }),
                None => (target_id, NONE),
            }
        };
        // `use super::*` names no symbol.
        if !matches!(self.cx.name_bytes(ref_name), b"super" | b"self" | b"crate") {
            self.cx.add_ref(ref_name, RefKind::Import, line, ref_qual, 255);
        }
        if !wildcard {
            let bound = if alias_id != NONE { alias_id } else { ref_name };
            if bound != self.ids.underscore {
                // What the bound name stands for: the whole path, without a leading `::`.
                let full = if name_id != NONE {
                    let mut tmp = std::mem::take(&mut self.b.tmp);
                    tmp.clear();
                    tmp.extend_from_slice(target.strip_prefix(b"::").unwrap_or(target));
                    tmp.extend_from_slice(b"::");
                    tmp.extend_from_slice(name);
                    let id = self.cx.intern(&tmp);
                    self.b.tmp = tmp;
                    id
                } else if let Some(t) = target.strip_prefix(b"::") {
                    self.cx.intern(t)
                } else {
                    target_id
                };
                self.b.imported.insert(bound, full);
            }
        }
    }

    fn extern_crate(&mut self, n: Node<'t>) {
        self.flush_attrs();
        let t = self.t;
        let Some(nm) = child(n, t.f_name) else { return };
        let name = self.ident(nm);
        let alias = match child(n, t.f_alias) {
            Some(a) => self.ident(a),
            None => NONE,
        };
        let line = line_of(n);
        let flags = if self.has_vis(n) { iflag::SYSTEM | iflag::REEXPORT } else { iflag::SYSTEM };
        self.cx.add_import(name, NONE, alias, line, flags);
        self.cx.add_ref(name, RefKind::Import, line, NONE, 255);
        let bound = if alias != NONE { alias } else { name };
        self.b.imported.insert(bound, name);
    }

    // ----- patterns and bindings -----

    fn declare_binds(&mut self, mark: usize) {
        for i in mark..self.b.binds.len() {
            let (name, ty) = self.b.binds[i];
            self.cx.declare(name, ty);
        }
        self.b.binds.truncate(mark);
    }

    /// Walk a pattern (cursor on it): collect its bindings into `binds`, to be declared by the
    /// caller once their scope is open, and emit references for the paths, constants and types
    /// it mentions. `ty` is the type of the matched value when known.
    fn pattern(&mut self, c: &mut TreeCursor<'t>, ty: u32) {
        let n = c.node();
        match self.k(n) {
            K::Identifier => {
                let name = self.ident(n);
                if !upper(self.cx.text(n)) {
                    self.b.binds.push((name, ty));
                } else if !self.is_stop(name) && self.generic(name).is_none() {
                    // A constant, unit struct or imported variant.
                    self.cx.add_ref(name, RefKind::Read, line_of(n), NONE, 255);
                }
            }
            K::Tok | K::Skip | K::Comment | K::IntegerLiteral | K::StringLiteral | K::MutableSpecifier => {}
            K::MutPattern | K::RefPattern | K::ReferencePattern | K::CapturedPattern | K::OrPattern => self.each(c, |w, c| w.pattern(c, ty)),
            K::TuplePattern => self.each(c, |w, c| w.pattern(c, NONE)),
            K::SlicePattern => {
                let el = self.inner_if(ty, is_sequence);
                self.each(c, |w, c| w.pattern(c, el));
            }
            K::TupleStructPattern => {
                let mut head = true;
                let mut sub = NONE;
                self.each(c, |w, c| {
                    let ch = c.node();
                    let k = w.k(ch);
                    if k == K::Comment {
                        return;
                    }
                    if !head {
                        return w.pattern(c, sub);
                    }
                    head = false;
                    match k {
                        K::Identifier => {
                            let name = w.ident(ch);
                            if name == w.ids.some || name == w.ids.ok {
                                // `Some(x)` / `Ok(x)` unwrap the matched value.
                                sub = w.inner_if(ty, is_unwrappable);
                            } else if !w.is_stop(name) && w.generic(name).is_none() {
                                w.cx.add_ref(name, RefKind::Read, line_of(ch), NONE, 255);
                            }
                        }
                        K::ScopedIdentifier => w.scoped_identifier(ch, c, RefKind::Read, 255),
                        _ => w.visit(c),
                    }
                });
            }
            K::StructPattern => {
                let mut sty = NONE;
                self.each(c, |w, c| {
                    let ch = c.node();
                    match w.k(ch) {
                        K::TypeIdentifier => {
                            sty = w.struct_head(ch);
                            w.type_identifier(ch);
                        }
                        K::ScopedTypeIdentifier => {
                            sty = w.struct_head(ch);
                            w.scoped_type(ch, c, RefKind::Type);
                        }
                        K::FieldPattern => w.field_pattern(c, sty),
                        _ => {}
                    }
                });
            }
            K::ScopedIdentifier => self.scoped_identifier(n, c, RefKind::Read, 255),
            // Ranges, literals, const blocks, macros.
            _ => self.visit(c),
        }
    }

    /// `field`, `field: pattern`, `ref mut field` inside a struct pattern of type `sty`.
    fn field_pattern(&mut self, c: &mut TreeCursor<'t>, sty: u32) {
        let mut fty = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::ShorthandFieldIdentifier => {
                    let name = w.ident(ch);
                    w.cx.add_ref(name, RefKind::MemberRead, line_of(ch), sty, 255);
                    let ty = w.field_path(sty, w.cx.text(ch));
                    w.b.binds.push((name, ty));
                }
                K::FieldIdentifier => {
                    let name = w.ident(ch);
                    w.cx.add_ref(name, RefKind::MemberRead, line_of(ch), sty, 255);
                    fty = w.field_path(sty, w.cx.text(ch));
                }
                K::Tok | K::MutableSpecifier | K::Comment => {}
                _ => w.pattern(c, fty),
            }
        });
    }

    // ----- statements and expressions -----

    fn block(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        self.children(c);
        self.cx.pop_scope();
    }

    /// `while` loops and closures: bindings made in the head are visible in the body.
    fn scoped_children(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        self.children(c);
        self.cx.pop_scope();
    }

    /// `if let` bindings are visible in the consequence but not in the `else` branch.
    fn if_expression(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        let mut open = true;
        self.each(c, |w, c| {
            if open && w.k(c.node()) == K::ElseClause {
                w.cx.pop_scope();
                open = false;
            }
            w.visit(c);
        });
        if open {
            self.cx.pop_scope();
        }
    }

    fn let_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        self.flush_attrs();
        let t = self.t;
        let ty = match child(n, t.f_type) {
            Some(x) => self.type_name(x, TyUse::Local),
            None => match child(n, t.f_value) {
                Some(v) => self.ty_of(v, 0),
                None => NONE,
            },
        };
        // The initialiser is evaluated before the pattern's names exist.
        let mark = self.b.binds.len();
        let mut pat_seen = false;
        self.each(c, |w, c| match w.k(c.node()) {
            K::Tok | K::MutableSpecifier | K::Comment => {}
            _ if !pat_seen => {
                pat_seen = true;
                w.pattern(c, ty);
            }
            _ => w.visit(c),
        });
        self.declare_binds(mark);
    }

    /// `let PATTERN = value` in a condition: the bindings go to the scope opened by the
    /// enclosing `if` / `while` / match arm.
    fn let_condition(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let ty = match child(n, self.t.f_value) {
            Some(v) => self.ty_of(v, 0),
            None => NONE,
        };
        let mark = self.b.binds.len();
        let mut pat_seen = false;
        self.each(c, |w, c| match w.k(c.node()) {
            K::Tok | K::Comment => {}
            _ if !pat_seen => {
                pat_seen = true;
                w.pattern(c, ty);
            }
            _ => w.visit(c),
        });
        self.declare_binds(mark);
    }

    fn for_expression(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let ety = match child(n, self.t.f_value) {
            Some(v) => {
                let ty = self.ty_of(v, 0);
                self.inner_if(ty, is_sequence)
            }
            None => NONE,
        };
        self.cx.push_scope();
        let mark = self.b.binds.len();
        let mut pat_seen = false;
        self.each(c, |w, c| match w.k(c.node()) {
            K::Tok | K::Skip | K::Comment => {}
            _ if !pat_seen => {
                pat_seen = true;
                w.pattern(c, ety);
            }
            K::Block => {
                w.declare_binds(mark);
                w.visit(c);
            }
            _ => w.visit(c),
        });
        self.b.binds.truncate(mark);
        self.cx.pop_scope();
    }

    fn match_expression(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let ty = match child(n, self.t.f_value) {
            Some(v) => self.ty_of(v, 0),
            None => NONE,
        };
        let saved = self.match_ty;
        self.each(c, |w, c| {
            if w.k(c.node()) == K::MatchBlock {
                w.match_ty = ty;
            }
            w.visit(c);
        });
        self.match_ty = saved;
    }

    fn match_arm(&mut self, c: &mut TreeCursor<'t>) {
        self.flush_attrs();
        let ty = self.match_ty;
        self.cx.push_scope();
        self.each(c, |w, c| {
            if w.k(c.node()) == K::MatchPattern {
                let mark = w.b.binds.len();
                let mut pat_seen = false;
                w.each(c, |w, c| match w.k(c.node()) {
                    K::Tok | K::Comment => {}
                    _ if !pat_seen => {
                        pat_seen = true;
                        w.pattern(c, ty);
                        // Declared before the guard is visited.
                        w.declare_binds(mark);
                    }
                    _ => w.visit(c),
                });
            } else {
                w.visit(c);
            }
        });
        self.cx.pop_scope();
    }

    fn call_expression(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        if c.goto_first_child() {
            let mut callee_seen = false;
            // The call's own reference is the first one its callee records, if it records any
            // (calling a local closure does not).
            let call_ref = self.cx.facts.refs.len();
            let mut has_ref = false;
            loop {
                let ch = c.node();
                let k = self.k(ch);
                if !callee_seen && k != K::Comment {
                    callee_seen = true;
                    has_ref = self.callee(ch, c);
                } else if k == K::Arguments {
                    let mut nargs = 0u32;
                    self.each(c, |w, c| {
                        if !matches!(w.k(c.node()), K::Tok | K::Comment | K::AttributeItem) {
                            nargs += 1;
                        }
                        w.visit(c);
                    });
                    if let Some(r) = self.cx.facts.refs.get_mut(call_ref).filter(|_| has_ref) {
                        if matches!(r.kind, RefKind::Call | RefKind::MemberCall) {
                            r.nargs = nargs.min(254) as u8;
                        }
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

    /// The function position of a call (cursor on it). Returns true when the first reference
    /// it recorded is the call itself.
    fn callee(&mut self, f: Node<'t>, c: &mut TreeCursor<'t>) -> bool {
        match self.k(f) {
            K::Identifier => {
                let mut name = self.ident(f);
                // Closures and function-pointer parameters are locals.
                if self.cx.is_local(name) {
                    return false;
                }
                if name == self.ids.self_upper {
                    // `Self(..)`: the tuple struct's constructor.
                    if self.self_ty == NONE {
                        return false;
                    }
                    name = self.self_ty;
                } else if self.is_stop(name) {
                    return false;
                }
                self.cx.add_ref(name, RefKind::Call, line_of(f), NONE, 255);
                true
            }
            K::ScopedIdentifier => {
                let before = self.cx.facts.refs.len();
                self.scoped_identifier(f, c, RefKind::Call, 255);
                self.cx.facts.refs.get(before).is_some_and(|r| r.kind == RefKind::Call)
            }
            K::FieldExpression => self.method_call(f, c),
            K::GenericFunction => {
                // `f::<T>(..)`, `x.f::<T>(..)`
                let mut head = true;
                let mut own = false;
                self.each(c, |w, c| {
                    let ch = c.node();
                    if head && w.k(ch) != K::Comment {
                        head = false;
                        own = w.callee(ch, c);
                    } else {
                        w.visit(c);
                    }
                });
                own
            }
            K::ParenthesizedExpression => {
                // `(self.handler)(x)`: a call through a field that holds a callable.
                if f.named_child(0).is_some_and(|x| self.k(x) == K::FieldExpression) {
                    let mut own = false;
                    self.each(c, |w, c| {
                        let ch = c.node();
                        if w.k(ch) == K::FieldExpression {
                            own = w.method_call(ch, c);
                        }
                    });
                    own
                } else {
                    self.visit(c);
                    false
                }
            }
            _ => {
                self.visit(c);
                false
            }
        }
    }

    /// `value.name(..)` (cursor on the field expression). Returns true when it recorded the
    /// member call.
    fn method_call(&mut self, f: Node<'t>, c: &mut TreeCursor<'t>) -> bool {
        let (value, field) = self.field_parts(f);
        let mut own = false;
        if let (Some(v), Some(fl)) = (value, field) {
            if self.k(fl) == K::FieldIdentifier {
                // `T::new(..).unwrap()`: the constructor returned a `Result` / `Option`.
                let q = if self.ctor_call(v) != NONE && is_unwrap_family(self.cx.text(fl)) { NONE } else { self.recv_qual(v) };
                let name = self.ident(fl);
                self.cx.add_ref(name, RefKind::MemberCall, line_of(fl), q, 255);
                own = true;
            }
        }
        self.visit_first(c);
        own
    }

    fn field_expression(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: RefKind) {
        let (value, field) = self.field_parts(n);
        if let (Some(v), Some(fl)) = (value, field) {
            if self.k(fl) == K::FieldIdentifier {
                let q = self.recv_qual(v);
                let name = self.ident(fl);
                self.cx.add_ref(name, kind, line_of(fl), q, 255);
            }
        }
        self.visit_first(c);
    }

    /// `left = right`, `left += right`.
    fn assignment(&mut self, c: &mut TreeCursor<'t>, plain: bool) {
        let mut left = true;
        let mut target: Option<(u32, Node<'t>)> = None;
        self.each(c, |w, c| {
            let ch = c.node();
            let k = w.k(ch);
            if k == K::Tok || k == K::Comment {
                return;
            }
            if left {
                left = false;
                target = w.lvalue(c);
                return;
            }
            // `obj.callback = handler` binds a function into a slot.
            match (target, plain) {
                (Some((q, fl)), true) => match w.bindable(ch) {
                    Some(v) => {
                        let slot = w.slot(q, w.cx.text(fl));
                        let name = w.ident(v);
                        w.cx.add_ref(name, RefKind::Bind, line_of(v), slot, 255);
                        if k == K::ScopedIdentifier {
                            w.path_first(c, true);
                        }
                    }
                    None => w.visit(c),
                },
                _ => w.visit(c),
            }
        });
    }

    /// The written place of an assignment (cursor on it). Returns the receiver qualifier and
    /// field node when it is a field.
    fn lvalue(&mut self, c: &mut TreeCursor<'t>) -> Option<(u32, Node<'t>)> {
        let n = c.node();
        match self.k(n) {
            K::Identifier => {
                let name = self.ident(n);
                if !self.cx.is_local(name) && !self.is_stop(name) && self.generic(name).is_none() {
                    self.cx.add_ref(name, RefKind::Write, line_of(n), NONE, 255);
                }
                None
            }
            K::FieldExpression => {
                let (value, field) = self.field_parts(n);
                let mut out = None;
                if let (Some(v), Some(fl)) = (value, field) {
                    if self.k(fl) == K::FieldIdentifier {
                        let q = self.recv_qual(v);
                        let name = self.ident(fl);
                        self.cx.add_ref(name, RefKind::MemberWrite, line_of(fl), q, 255);
                        out = Some((q, fl));
                    }
                }
                self.visit_first(c);
                out
            }
            // `a[i] = v`, `*p = v`, `(x) = v`: the indexed / dereferenced place is written.
            K::IndexExpression | K::ParenthesizedExpression | K::UnaryExpression => {
                let mut first = true;
                self.each(c, |w, c| {
                    let k = w.k(c.node());
                    if k == K::Tok || k == K::Comment {
                        return;
                    }
                    if first {
                        first = false;
                        w.lvalue(c);
                    } else {
                        w.visit(c);
                    }
                });
                None
            }
            _ => {
                self.visit(c);
                None
            }
        }
    }

    /// A value that can be bound into a slot: a plain path to a function (by convention a
    /// lower-case name that is not a local).
    fn bindable(&mut self, v: Node<'t>) -> Option<Node<'t>> {
        match self.k(v) {
            K::Identifier => {
                let txt = self.cx.text(v);
                if upper(txt) {
                    return None;
                }
                let name = self.ident(v);
                if self.cx.is_local(name) || self.generic(name).is_some() || name == self.ids.underscore { None } else { Some(v) }
            }
            K::ScopedIdentifier => {
                let nm = self.scoped_parts(v).1?;
                if self.k(nm) == K::Identifier && !upper(self.cx.text(nm)) { Some(nm) } else { None }
            }
            _ => None,
        }
    }

    /// `T { field: value, .. }`
    fn struct_expression(&mut self, c: &mut TreeCursor<'t>) {
        let mut tname = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::TypeIdentifier => {
                    tname = w.struct_head(ch);
                    if tname != NONE {
                        w.cx.add_ref(tname, RefKind::New, line_of(ch), NONE, 255);
                    }
                }
                K::ScopedTypeIdentifier => {
                    tname = w.struct_head(ch);
                    w.scoped_type(ch, c, RefKind::New);
                }
                K::GenericTypeWithTurbofish => {
                    // `Foo::<T> { .. }`
                    tname = w.struct_head(ch);
                    let mut head = true;
                    w.each(c, |w, c| {
                        let ch = c.node();
                        match w.k(ch) {
                            K::TypeIdentifier if head => {
                                head = false;
                                if tname != NONE {
                                    w.cx.add_ref(tname, RefKind::New, line_of(ch), NONE, 255);
                                }
                            }
                            K::ScopedIdentifier if head => {
                                head = false;
                                w.scoped_identifier(ch, c, RefKind::New, 255);
                            }
                            _ => w.visit(c),
                        }
                    });
                }
                K::FieldInitializerList => w.each(c, |w, c| {
                    let ch = c.node();
                    match w.k(ch) {
                        K::FieldInitializer => w.field_initializer(c, tname),
                        K::ShorthandFieldInitializer => w.each(c, |w, c| {
                            let ch = c.node();
                            if w.k(ch) == K::Identifier {
                                let name = w.ident(ch);
                                w.cx.add_ref(name, RefKind::MemberWrite, line_of(ch), tname, 255);
                                w.identifier(ch);
                            }
                        }),
                        _ => w.visit(c),
                    }
                }),
                _ => w.visit(c),
            }
        });
    }

    /// `field: value` in a struct expression of type `tname`.
    fn field_initializer(&mut self, c: &mut TreeCursor<'t>, tname: u32) {
        let mut field: Option<Node<'t>> = None;
        let mut colon = false;
        self.each(c, |w, c| {
            let ch = c.node();
            let k = w.k(ch);
            if !colon {
                match k {
                    K::FieldIdentifier => {
                        let name = w.ident(ch);
                        w.cx.add_ref(name, RefKind::MemberWrite, line_of(ch), tname, 255);
                        field = Some(ch);
                    }
                    K::Tok => colon = w.tk(ch) == Tk::Colon,
                    _ => {}
                }
                return;
            }
            if k == K::Comment {
                return;
            }
            match (field, w.bindable(ch)) {
                (Some(fl), Some(v)) => {
                    let slot = w.slot(tname, w.cx.text(fl));
                    let name = w.ident(v);
                    w.cx.add_ref(name, RefKind::Bind, line_of(v), slot, 255);
                    if k == K::ScopedIdentifier {
                        w.path_first(c, true);
                    }
                }
                _ => w.visit(c),
            }
        });
    }

    // ----- macros -----

    fn macro_invocation(&mut self, c: &mut TreeCursor<'t>) {
        if !self.b.decl.is_empty() {
            let n = c.node();
            let name_pos = child(n, self.t.f_macro).map(|m| m.start_byte() as u32);
            if let Some(site) = self.b.decl.iter().find(|d| Some(d.name) == name_pos).copied() {
                return self.decl_macro(n, c, site);
            }
        }
        self.flush_attrs();
        let mut mname: &[u8] = b"";
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Identifier => {
                    let name = w.ident(ch);
                    mname = w.cx.text(ch);
                    w.cx.add_ref(name, RefKind::MacroCall, line_of(ch), NONE, 255);
                }
                K::ScopedIdentifier => {
                    let (path, nm) = w.scoped_parts(ch);
                    if let Some(nm) = nm {
                        let name = w.ident(nm);
                        mname = w.cx.text(nm);
                        let qual = match path {
                            Some(p) => w.path_qual(p),
                            None => NONE,
                        };
                        w.cx.add_ref(name, RefKind::MacroCall, line_of(nm), qual, 255);
                    }
                }
                K::TokenTree => w.macro_tokens(c, mname, w.fn_depth > 0, true),
                _ => {}
            }
        });
    }

    /// `macro name(..) { .. }` / `macro name { (..) => { .. } }`: shown to the parser as an
    /// invocation `name! { .. }` whose tree is the macro's body (see `sanitize`).
    fn decl_macro(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, site: DeclMacro) {
        let at = self.take_attrs();
        let Some(nm) = child(n, self.t.f_macro) else { return };
        let name = self.ident(nm);
        let mut flags = 0u8;
        if site.exported || at.macro_export {
            flags |= dflag::EXPORTED;
        }
        if at.cfg_test || self.test_ctx {
            flags |= dflag::TEST;
        }
        if self.scope == Scope::Fn {
            flags |= dflag::LOCAL;
        }
        let d = self.cx.add_def(Def {
            name,
            parent: self.parent,
            qual: NONE,
            line: line_of(n),
            end_line: end_line_of(n),
            sig_off: site.start,
            sig_len: clamp_u16(nm.end_byte().saturating_sub(site.start as usize)),
            kind: DefKind::Macro,
            flags,
            type_name: NONE,
            arity_min: 0,
            arity_max: 255,
        });
        self.attach_annots(d);
        let saved = self.enter(d, Scope::Macro);
        self.each(c, |w, c| {
            if w.k(c.node()) == K::TokenTree {
                w.cx.push_scope();
                w.tokens(c, Tm { body: Body::Code, code: true, defs: true, pat: false }, if site.rules { Init::Rules } else { Init::Plain });
                w.cx.pop_scope();
            }
        });
        self.leave(d, saved);
    }

    /// Scan the argument tree of a macro invocation (cursor on the tree).
    fn macro_tokens(&mut self, c: &mut TreeCursor<'t>, mname: &[u8], code: bool, defs: bool) {
        if !scans_tokens(mname) {
            return;
        }
        let m = Tm { body: Body::Code, code, defs: defs && !is_quote_macro(mname), pat: false };
        let init = if matches!(mname, b"matches" | b"assert_matches" | b"debug_assert_matches") { Init::PatAfterComma } else { Init::Plain };
        self.cx.push_scope();
        self.tokens(c, m, init);
        self.cx.pop_scope();
    }
}

// ---------------------------------------------------------------------------------------------
// Token trees
// ---------------------------------------------------------------------------------------------

/// What a token tree holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Body {
    /// Statements, expressions, items.
    Code,
    /// Body of a struct or union: `name: Type` elements.
    Fields,
    /// Body of an enum: variants.
    Variants,
    /// Parameter list of a function.
    Params,
    /// Types (tuple struct fields, bounds).
    Types,
}

/// Mode of a token-tree scan, inherited by nested trees.
#[derive(Clone, Copy)]
struct Tm {
    body: Body,
    /// Expression context: plain lower-case identifiers are reads of values.
    code: bool,
    /// Item-like token sequences define symbols.
    defs: bool,
    /// The tree is part of a pattern.
    pat: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Init {
    Plain,
    /// `matches!(value, PATTERN)`: a pattern follows the first comma.
    PatAfterComma,
    /// Rules of a macro definition: `(pattern) => { body }` whatever the delimiter.
    Rules,
}

/// Previous significant token.
#[derive(Clone, Copy, PartialEq, Eq)]
enum P {
    Start,
    Dot,
    PathSep,
    Quote,
    Ident,
    Kw,
    Punct,
    Value,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PCtx {
    Plain,
    Dot,
    Path,
    /// Last segment in front of a turbofish, or the segment after one: the qualifier is given.
    Turbo,
}

/// An identifier whose role depends on the token after it.
#[derive(Clone, Copy)]
struct Pend<'t> {
    node: Node<'t>,
    ctx: PCtx,
    /// Type path of the receiver (after a `.`).
    recv: u32,
    /// `self`, `crate`, `super`, a primitive type: a path segment, never a reference.
    silent: bool,
    /// Follows another identifier: a name in some macro-specific syntax, not an expression.
    dsl: bool,
    /// First token of a comma-separated element.
    first: bool,
    /// Qualifier, for `PCtx::Turbo`.
    qual: u32,
    /// Item modifiers in front (`pub`).
    mods: Option<(u32, u32)>,
    mods_pub: bool,
}

/// What follows a pending identifier.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Next {
    Paren,
    Brace,
    Bracket,
    PathSep,
    Dot,
    Eq,
    AssignOp,
    Colon,
    Lt,
    Ident,
    Other,
}

/// Outcome of resolving a pending identifier in front of a nested tree.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Res {
    None,
    /// A call was recorded: reference index, to patch the argument count.
    Call(usize),
    /// A struct literal.
    Lit,
    /// An enum variant definition.
    Variant(u32),
}

/// A declared type still to be attached to something.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    None,
    Param(u32),
    Def(u32),
}

#[derive(Clone, Copy)]
struct MacPend<'t> {
    name: u32,
    qual: u32,
    node: Node<'t>,
    rules: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Next,
    End,
}

/// Scan state of one token tree level.
struct Ts<'t> {
    m: Tm,
    delim: Tk,
    prev: P,
    last_tk: Tk,
    pend: Option<Pend<'t>>,
    recv: u32,
    chain_on: bool,
    chain_start: u32,
    chain_end: u32,
    chain_len: u32,
    /// End and text length of the path without its last segment (0 when it has one segment).
    chain_pend: u32,
    chain_plen: u32,
    chain_last: Option<Node<'t>>,
    /// Depth inside a turbofish written in the middle of a path (`Vec::<T>::new()`).
    turbo: i32,
    turbo_last: Option<Node<'t>>,
    /// Qualifier of the segment in front of the turbofish, and of what follows it.
    turbo_qual: u32,
    turbo_full: u32,
    /// Qualifier for the identifier right after `Type::<T>::`.
    qual_next: u32,
    /// In a pattern: lower-case identifiers are bindings.
    pat: bool,
    let_pat: bool,
    for_pat: bool,
    closure: bool,
    /// End of the last brace-delimited tree at this level (0: none).
    blk_end: u32,
    arms: bool,
    /// Between `if` / `while` / `match` / `in` and the block: no struct literals.
    cond: bool,
    after_colon: bool,
    pat_after_comma: bool,
    /// Item keyword waiting for its name.
    kw: Tk,
    kw_pos: (u32, u32),
    kw_pub: bool,
    kw_async: bool,
    /// Start of the current run of item modifiers (`pub`, `const`, `unsafe`, ...).
    mods: Option<(u32, u32)>,
    mods_pub: bool,
    mods_async: bool,
    hash: bool,
    /// A test attribute (`#[test]`, `#[cfg(test)]`) precedes the item being read.
    attr_test: bool,
    kw_test: bool,
    /// The previous token was a `$`.
    dollar: bool,
    mac: Option<MacPend<'t>>,
    sticky: bool,
    keep_mods: bool,
    keep_elem: bool,
    set_elem: bool,
    /// `const` / `static` / `type` definition waiting for its `;`.
    open_def: u32,
    open_gmark: usize,
    /// Field definition waiting for its `,`.
    open_field: u32,
    want: Want,
    elem_start: bool,
    elems: u32,
    nonempty: bool,
    countable: bool,
}

impl<'t> Ts<'t> {
    fn new(m: Tm) -> Self {
        Ts {
            m,
            delim: Tk::Punct,
            prev: P::Start,
            last_tk: Tk::Punct,
            pend: None,
            recv: NONE,
            chain_on: false,
            chain_start: 0,
            chain_end: 0,
            chain_len: 0,
            chain_pend: 0,
            chain_plen: 0,
            chain_last: None,
            turbo: 0,
            turbo_last: None,
            turbo_qual: NONE,
            turbo_full: NONE,
            qual_next: NONE,
            pat: m.pat,
            let_pat: false,
            for_pat: false,
            closure: false,
            blk_end: 0,
            arms: false,
            cond: false,
            after_colon: false,
            pat_after_comma: false,
            kw: Tk::Punct,
            kw_pos: (0, 0),
            kw_pub: false,
            kw_async: false,
            mods: None,
            mods_pub: false,
            mods_async: false,
            hash: false,
            attr_test: false,
            kw_test: false,
            dollar: false,
            mac: None,
            sticky: false,
            keep_mods: false,
            keep_elem: false,
            set_elem: false,
            open_def: NONE,
            open_gmark: 0,
            open_field: NONE,
            want: Want::None,
            elem_start: true,
            elems: 0,
            nonempty: false,
            countable: true,
        }
    }
}

struct HOpts {
    /// Stop at the parameter list.
    stop_paren: bool,
    /// Stop at `=` (and take the type after `:` as the declared type).
    stop_eq: bool,
    /// A generic parameter list may come first.
    generics: bool,
    /// Hold back the last type name outside of generics: it names the item (impl headers).
    hold: bool,
}

enum HEnd<'t> {
    Params(Node<'t>),
    Body(Node<'t>),
    Semi(Node<'t>),
    Eq(Node<'t>),
    End,
}

struct Hdr<'t> {
    end: HEnd<'t>,
    /// Held type name (see `HOpts::hold`).
    name: Option<Node<'t>>,
    /// Trait of `impl Trait for Type`.
    tr: Option<Node<'t>>,
    /// The name an implemented type without one is filed under (`tuple`, `array`, `slice`).
    anon: Option<&'static [u8]>,
    /// Declared / return type name.
    ty: u32,
}

#[inline]
fn pos_of(n: Node) -> (u32, u32) {
    (n.start_byte() as u32, line_of(n))
}

impl<'a, 't> W<'a, 't> {
    /// Opening delimiter of a token tree.
    #[inline]
    fn delim(&self, n: Node<'t>) -> Tk {
        match n.child(0) {
            Some(x) => self.tk(x),
            None => Tk::Punct,
        }
    }

    /// Token-level pass over one token tree (cursor on the tree). Returns the number of
    /// comma-separated elements at its top level, 255 when it cannot be told.
    fn tokens(&mut self, c: &mut TreeCursor<'t>, m: Tm, init: Init) -> u8 {
        if self.cx.depth >= MAX_DEPTH {
            return 255;
        }
        if !c.goto_first_child() {
            return 0;
        }
        self.cx.depth += 1;
        let mut st = Ts::new(m);
        st.delim = self.tk(c.node());
        match init {
            Init::Rules => {
                st.arms = true;
                st.pat = true;
            }
            Init::PatAfterComma => st.pat_after_comma = true,
            Init::Plain => {
                if st.delim == Tk::LBrace && m.body == Body::Code && !m.pat {
                    // A brace tree with `=>` at its top level holds match arms.
                    loop {
                        let n = c.node();
                        if self.k(n) == K::Tok && self.tk(n) == Tk::FatArrow {
                            st.arms = true;
                            st.pat = true;
                            break;
                        }
                        if !c.goto_next_sibling() {
                            break;
                        }
                    }
                    c.goto_parent();
                    c.goto_first_child();
                }
            }
        }
        loop {
            if self.token(c, &mut st) == Step::End {
                break;
            }
            if !c.goto_next_sibling() {
                break;
            }
        }
        self.resolve(&mut st, Next::Other);
        let last = c.node();
        self.close_field(&mut st, last);
        self.close_def(&mut st, last);
        self.want_done(&mut st, NONE);
        c.goto_parent();
        self.cx.depth -= 1;
        if !st.countable {
            return 255;
        }
        (st.elems + st.nonempty as u32).min(254) as u8
    }

    fn token(&mut self, c: &mut TreeCursor<'t>, st: &mut Ts<'t>) -> Step {
        let n = c.node();
        let id = n.kind_id();
        let k = self.t.kinds.get(id);
        if k == K::Comment {
            return Step::Next;
        }
        st.sticky = false;
        st.keep_mods = false;
        st.keep_elem = false;
        st.set_elem = false;
        let qual_next = std::mem::replace(&mut st.qual_next, NONE);
        let dollar = std::mem::replace(&mut st.dollar, false);
        let step = match k {
            K::Identifier if dollar => self.tok_metavar(n, c, st),
            K::Identifier => self.tok_ident(n, c, st, qual_next),
            K::Tok => self.tok_punct(n, self.t.tok(id), c, st, qual_next),
            K::TokenTree => self.tok_tree(n, c, st),
            K::TokenRepetition => {
                // `$( ... )*`: the same kind of content as its surroundings.
                self.resolve(st, Next::Other);
                let m = Tm { pat: st.pat, ..st.m };
                self.tokens(c, m, Init::Plain);
                st.prev = P::Value;
                st.recv = NONE;
                st.nonempty = true;
                Step::Next
            }
            K::SelfKw | K::Crate | K::Super | K::PrimitiveType => {
                self.tok_segment(n, k, st);
                Step::Next
            }
            K::Metavariable => self.tok_metavar(n, c, st),
            K::MutableSpecifier => {
                self.resolve(st, Next::Other);
                // `static mut NAME`
                st.sticky = st.kw == Tk::Static;
                st.prev = P::Kw;
                st.nonempty = true;
                Step::Next
            }
            _ => {
                // Literals.
                self.resolve(st, Next::Other);
                self.want_done(st, NONE);
                st.recv = NONE;
                st.prev = P::Value;
                st.nonempty = true;
                Step::Next
            }
        };
        if !st.sticky {
            st.kw = Tk::Punct;
            st.hash = false;
            st.mac = None;
        }
        if !st.keep_mods {
            st.mods = None;
            st.mods_pub = false;
            st.mods_async = false;
            st.attr_test = false;
        }
        if st.set_elem {
            st.elem_start = true;
        } else if !st.keep_elem {
            st.elem_start = false;
        }
        step
    }

    fn tok_ident(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, st: &mut Ts<'t>, qual_next: u32) -> Step {
        let txt = self.cx.text(n);
        st.nonempty = true;
        // Keywords the grammar leaves as identifiers inside token trees.
        match txt {
            b"in" => {
                self.resolve(st, Next::Other);
                if st.for_pat {
                    st.for_pat = false;
                    st.pat = false;
                }
                st.cond = true;
                st.prev = P::Kw;
                return Step::Next;
            }
            b"extern" => {
                self.resolve(st, Next::Other);
                if st.mods.is_none() {
                    st.mods = Some(pos_of(n));
                }
                st.keep_mods = true;
                st.keep_elem = true;
                st.prev = P::Kw;
                return Step::Next;
            }
            b"ref" => {
                self.resolve(st, Next::Other);
                // `static ref NAME` (lazy_static)
                st.sticky = st.kw == Tk::Static;
                st.prev = P::Kw;
                return Step::Next;
            }
            b"else" | b"move" | b"dyn" | b"try" | b"yield" | b"box" | b"macro" => {
                self.resolve(st, Next::Other);
                st.prev = P::Kw;
                return Step::Next;
            }
            b"raw" if st.prev == P::Punct && st.last_tk == Tk::Amp => {
                // `&raw const place`: what follows is a pointer kind, like in `*const T`.
                st.last_tk = Tk::Star;
                return Step::Next;
            }
            _ => {}
        }
        if st.prev == P::Quote {
            // Lifetime or label.
            st.prev = P::Punct;
            return Step::Next;
        }
        if st.kw != Tk::Punct {
            let kw = st.kw;
            return self.syn_named(kw, Some(n), c, st);
        }
        if let Some(mp) = st.mac {
            if mp.rules {
                return self.syn_macro_rules(n, mp.node, c, st);
            }
        }
        // Two identifiers in a row (the previous one is still pending), or `@name`: a word of
        // some macro-specific syntax.
        let dsl = st.prev == P::Ident || (st.prev == P::Punct && st.last_tk == Tk::At && !upper(txt));
        self.resolve(st, Next::Ident);
        let ctx = match st.prev {
            P::Dot => PCtx::Dot,
            P::PathSep if st.chain_on => PCtx::Path,
            P::PathSep if qual_next != NONE => PCtx::Turbo,
            _ => PCtx::Plain,
        };
        let qual = if ctx == PCtx::Turbo { qual_next } else { NONE };
        let recv = if ctx == PCtx::Dot { st.recv } else { NONE };
        st.recv = NONE;
        if ctx != PCtx::Path {
            st.chain_on = false;
        }
        st.pend = Some(Pend { node: n, ctx, recv, silent: false, dsl, first: st.elem_start, qual, mods: st.mods, mods_pub: st.mods_pub });
        st.prev = P::Ident;
        Step::Next
    }

    /// `self`, `crate`, `super` or a primitive type: path segments that are never references.
    fn tok_segment(&mut self, n: Node<'t>, k: K, st: &mut Ts<'t>) {
        self.resolve(st, Next::Other);
        st.nonempty = true;
        if k == K::PrimitiveType {
            if let Want::Param(_) = st.want {
                let id = self.cx.name(n);
                self.want_done(st, id);
            } else {
                self.want_done(st, NONE);
            }
        }
        if k == K::SelfKw && st.m.body == Body::Params {
            self.saw_self = true;
        }
        let ctx = if st.prev == P::PathSep && st.chain_on { PCtx::Path } else { PCtx::Plain };
        if ctx == PCtx::Plain {
            st.chain_on = false;
        }
        st.recv = NONE;
        st.pend = Some(Pend { node: n, ctx, recv: NONE, silent: true, dsl: false, first: false, qual: NONE, mods: None, mods_pub: false });
        st.prev = P::Ident;
    }

    fn tok_metavar(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, st: &mut Ts<'t>) -> Step {
        st.nonempty = true;
        if st.kw != Tk::Punct {
            // `fn $name(..) { .. }`: an item whose name is only known at expansion.
            let kw = st.kw;
            return self.syn_named(kw, None, c, st);
        }
        self.resolve(st, Next::Other);
        self.want_done(st, NONE);
        st.recv = NONE;
        if self.cx.text(n) == b"$crate" {
            st.chain_on = false;
            st.pend = Some(Pend { node: n, ctx: PCtx::Plain, recv: NONE, silent: true, dsl: false, first: false, qual: NONE, mods: None, mods_pub: false });
            st.prev = P::Ident;
        } else {
            st.chain_on = false;
            st.prev = P::Value;
        }
        Step::Next
    }

    fn tok_punct(&mut self, n: Node<'t>, tk: Tk, c: &mut TreeCursor<'t>, st: &mut Ts<'t>, qual_next: u32) -> Step {
        let prev_tk = st.last_tk;
        let prev = st.prev;
        st.last_tk = tk;
        match tk {
            Tk::LParen | Tk::LBracket | Tk::LBrace => {
                // Opening delimiter of this level (or of a repetition group).
                st.keep_elem = true;
                st.prev = P::Start;
                return Step::Next;
            }
            Tk::Close => {
                self.resolve(st, Next::Other);
                st.prev = P::Punct;
                return Step::Next;
            }
            Tk::Comma => {
                self.resolve(st, Next::Other);
                self.want_done(st, NONE);
                self.close_field(st, n);
                if !st.closure {
                    if st.nonempty {
                        st.elems += 1;
                    }
                    st.nonempty = false;
                    st.after_colon = false;
                }
                if st.arms || st.closure {
                    st.pat = true;
                } else if st.pat_after_comma {
                    st.pat_after_comma = false;
                    st.pat = true;
                }
                st.set_elem = true;
                st.recv = NONE;
                st.prev = P::Punct;
                return Step::Next;
            }
            _ => {}
        }
        st.nonempty = true;
        match tk {
            Tk::Dot => {
                self.resolve(st, Next::Dot);
                st.prev = P::Dot;
                return Step::Next;
            }
            Tk::PathSep => {
                let turbo = st.pend.as_ref().is_some_and(|p| p.ctx == PCtx::Turbo);
                self.resolve(st, Next::PathSep);
                if turbo {
                    st.qual_next = qual_next;
                }
                st.recv = NONE;
                st.prev = P::PathSep;
                return Step::Next;
            }
            Tk::Bang => {
                if st.hash {
                    // `#![...]`
                    st.sticky = true;
                    st.keep_elem = true;
                    return Step::Next;
                }
                if let Some(p) = st.pend.take() {
                    if !p.silent && p.ctx != PCtx::Dot && self.k(p.node) == K::Identifier {
                        // `name!` or `path::name!`: a macro invocation if a tree follows.
                        let qual = if p.ctx == PCtx::Path && st.chain_on { self.chain_text(st.chain_start, st.chain_end, st.chain_len) } else { NONE };
                        let name = self.ident(p.node);
                        st.mac = Some(MacPend { name, qual, node: p.node, rules: self.cx.text(p.node) == b"macro_rules" });
                        st.sticky = true;
                    }
                }
                st.chain_on = false;
                st.recv = NONE;
                st.prev = P::Punct;
                return Step::Next;
            }
            _ => {}
        }
        st.recv = NONE;
        match tk {
            Tk::Quote => {
                self.resolve(st, Next::Other);
                st.prev = P::Quote;
                // `&'a T`: still in front of the type.
                return Step::Next;
            }
            Tk::Eq => {
                self.resolve(st, Next::Eq);
                self.want_done(st, NONE);
                if st.pat && !st.closure {
                    st.pat = false;
                    st.let_pat = false;
                }
            }
            Tk::AssignOp => {
                self.resolve(st, Next::AssignOp);
            }
            Tk::Colon => {
                self.resolve(st, Next::Colon);
                st.after_colon = true;
                if st.pat && !st.m.pat {
                    // `let x: T`, `|x: T|`: the type follows.
                    st.pat = false;
                }
            }
            Tk::Semi => {
                self.resolve(st, Next::Other);
                self.want_done(st, NONE);
                self.close_def(st, n);
                st.pat = st.arms;
                st.let_pat = false;
                st.for_pat = false;
                st.cond = false;
                st.after_colon = false;
                st.set_elem = true;
                st.countable = false;
            }
            Tk::Pipe => {
                self.resolve(st, Next::Other);
                if st.closure {
                    st.closure = false;
                    st.pat = false;
                } else if !st.pat && (!matches!(prev, P::Ident | P::Value) || (prev == P::Value && self.after_block(st, n))) {
                    // `|a, b| expr`: closure parameters. Right after a block (`{ .. } |x| ..`,
                    // callback-style macros) a `|` cannot be an operator either.
                    st.closure = true;
                    st.pat = true;
                }
            }
            Tk::FatArrow => {
                self.resolve(st, Next::Other);
                if st.arms {
                    st.pat = false;
                }
                st.cond = false;
            }
            Tk::Hash => {
                self.resolve(st, Next::Other);
                st.hash = true;
                st.sticky = true;
                st.keep_elem = true;
                st.keep_mods = true;
            }
            Tk::Dollar => {
                // `$name` in the body of a `macro` item: a metavariable.
                self.resolve(st, Next::Other);
                st.dollar = true;
                st.sticky = st.kw != Tk::Punct;
                st.prev = P::Punct;
                return Step::Next;
            }
            Tk::Lt => {
                self.resolve(st, Next::Lt);
                if st.for_pat && prev_tk == Tk::For && prev == P::Kw {
                    // `for<'a>`: a higher-ranked bound, not a loop.
                    st.for_pat = false;
                    st.pat = false;
                }
                if prev == P::PathSep {
                    // Turbofish: commas inside are not argument separators.
                    st.countable = false;
                    if st.chain_on && st.turbo == 0 {
                        // In the middle of a path: `Vec::<T>::new()`, `mem::size_of::<T>()`.
                        st.turbo = 1;
                        st.turbo_last = st.chain_last.take();
                        st.turbo_qual = if st.chain_plen > 0 { self.chain_text(st.chain_start, st.chain_pend, st.chain_plen) } else { NONE };
                        st.turbo_full = self.chain_text(st.chain_start, st.chain_end, st.chain_len);
                        st.chain_on = false;
                    }
                } else if st.turbo > 0 {
                    st.turbo += 1;
                }
            }
            Tk::Gt | Tk::Shr if st.turbo > 0 => {
                self.resolve(st, Next::Other);
                st.turbo -= if tk == Tk::Shr { 2 } else { 1 };
                if st.turbo <= 0 {
                    // The segment in front of the generic arguments is decided by what follows.
                    st.turbo = 0;
                    if let Some(last) = st.turbo_last.take() {
                        st.qual_next = std::mem::replace(&mut st.turbo_full, NONE);
                        st.pend = Some(Pend {
                            node: last,
                            ctx: PCtx::Turbo,
                            recv: NONE,
                            silent: false,
                            dsl: false,
                            first: false,
                            qual: st.turbo_qual,
                            mods: None,
                            mods_pub: false,
                        });
                        st.prev = P::Ident;
                        return Step::Next;
                    }
                }
            }
            Tk::Amp | Tk::Star => {
                self.resolve(st, Next::Other);
            }
            Tk::Fn => {
                self.resolve(st, Next::Other);
                st.kw = Tk::Fn;
                st.kw_pos = st.mods.unwrap_or_else(|| pos_of(n));
                st.kw_pub = st.mods_pub;
                st.kw_async = st.mods_async;
                st.kw_test = st.attr_test;
                st.sticky = true;
                st.prev = P::Kw;
                return Step::Next;
            }
            Tk::Struct | Tk::Enum | Tk::Trait | Tk::Mod | Tk::Type => {
                self.resolve(st, Next::Other);
                st.kw = tk;
                st.kw_pos = st.mods.unwrap_or_else(|| pos_of(n));
                st.kw_pub = st.mods_pub;
                st.kw_test = st.attr_test;
                st.sticky = true;
                st.prev = P::Kw;
                return Step::Next;
            }
            Tk::Union | Tk::Default | Tk::Gen => {
                if st.kw == Tk::Fn {
                    // `fn default() -> Self`
                    return self.syn_named(Tk::Fn, Some(n), c, st);
                }
                if matches!(prev, P::Dot | P::PathSep) {
                    // `T::default()`, `rng.gen()`, `a.union(b)`: ordinary names here.
                    let ctx = match prev {
                        P::Dot => PCtx::Dot,
                        _ if st.chain_on => PCtx::Path,
                        _ => PCtx::Plain,
                    };
                    let recv = if ctx == PCtx::Dot { st.recv } else { NONE };
                    self.resolve(st, Next::Other);
                    st.recv = NONE;
                    st.pend = Some(Pend { node: n, ctx, recv, silent: false, dsl: false, first: false, qual: NONE, mods: None, mods_pub: false });
                    st.prev = P::Ident;
                    return Step::Next;
                }
                self.resolve(st, Next::Other);
                if tk == Tk::Union {
                    st.kw = Tk::Union;
                    st.kw_pos = st.mods.unwrap_or_else(|| pos_of(n));
                    st.kw_pub = st.mods_pub;
                    st.sticky = true;
                } else if tk == Tk::Default {
                    // `default fn`
                    if st.mods.is_none() {
                        st.mods = Some(pos_of(n));
                    }
                    st.keep_mods = true;
                }
                st.prev = P::Kw;
                return Step::Next;
            }
            Tk::Const => {
                self.resolve(st, Next::Other);
                // Not `*const T`, and not a const generic parameter.
                if !(prev == P::Punct && matches!(prev_tk, Tk::Star | Tk::Lt | Tk::Comma)) {
                    if st.mods.is_none() {
                        st.mods = Some(pos_of(n));
                    }
                    st.keep_mods = true;
                    st.kw = Tk::Const;
                    st.kw_pos = st.mods.unwrap_or_else(|| pos_of(n));
                    st.kw_pub = st.mods_pub;
                    st.kw_test = st.attr_test;
                    st.sticky = true;
                }
                st.prev = P::Kw;
                return Step::Next;
            }
            Tk::Static => {
                self.resolve(st, Next::Other);
                if prev == P::Quote {
                    // `'static`
                    st.prev = P::Punct;
                    return Step::Next;
                }
                st.kw = Tk::Static;
                st.kw_pos = st.mods.unwrap_or_else(|| pos_of(n));
                st.kw_pub = st.mods_pub;
                st.kw_test = st.attr_test;
                st.sticky = true;
                st.prev = P::Kw;
                return Step::Next;
            }
            Tk::Impl => {
                self.resolve(st, Next::Other);
                // An impl block starts a statement; `impl Trait` in type position does not.
                // (`prev_tk` is still the `#` after an attribute: `#[stable(..)] impl ..`.)
                let item = st.m.body == Body::Code
                    && !st.pat
                    && (matches!(prev, P::Start | P::Value | P::Kw) || (prev == P::Punct && matches!(prev_tk, Tk::Semi | Tk::Hash)));
                if item && st.want == Want::None {
                    return self.syn_impl(n, c, st);
                }
                st.prev = P::Kw;
                return Step::Next;
            }
            Tk::Let => {
                self.resolve(st, Next::Other);
                st.pat = true;
                st.let_pat = true;
                st.prev = P::Kw;
                return Step::Next;
            }
            Tk::For => {
                self.resolve(st, Next::Other);
                st.pat = true;
                st.for_pat = true;
                st.prev = P::Kw;
                return Step::Next;
            }
            Tk::If => {
                self.resolve(st, Next::Other);
                // A guard ends the pattern of a match arm.
                st.pat = false;
                st.cond = true;
                st.prev = P::Kw;
                return Step::Next;
            }
            Tk::While | Tk::Match => {
                self.resolve(st, Next::Other);
                st.cond = true;
                st.prev = P::Kw;
                return Step::Next;
            }
            Tk::Use => {
                self.resolve(st, Next::Other);
                let reexport = st.mods_pub;
                st.prev = P::Punct;
                st.last_tk = Tk::Semi;
                st.set_elem = true;
                st.countable = false;
                return self.tok_use(c, reexport);
            }
            Tk::Pub => {
                self.resolve(st, Next::Other);
                if st.mods.is_none() {
                    st.mods = Some(pos_of(n));
                }
                st.mods_pub = true;
                st.keep_mods = true;
                st.keep_elem = true;
                st.prev = P::Kw;
                return Step::Next;
            }
            Tk::Async | Tk::Unsafe => {
                self.resolve(st, Next::Other);
                if st.mods.is_none() {
                    st.mods = Some(pos_of(n));
                }
                if tk == Tk::Async {
                    st.mods_async = true;
                }
                st.keep_mods = true;
                st.prev = P::Kw;
                return Step::Next;
            }
            Tk::As | Tk::Where | Tk::Kw => {
                self.resolve(st, Next::Other);
                st.prev = P::Kw;
                return Step::Next;
            }
            _ => {
                self.resolve(st, Next::Other);
                if !matches!(tk, Tk::Gt | Tk::Shr | Tk::Arrow) {
                    self.want_done(st, NONE);
                }
            }
        }
        st.prev = P::Punct;
        Step::Next
    }

    /// Is `n` the first token after a brace-delimited tree of this level?
    fn after_block(&self, st: &Ts<'t>, n: Node<'t>) -> bool {
        let (a, b) = (st.blk_end as usize, n.start_byte());
        a != 0 && a <= b && self.cx.src[a..b].iter().all(|x| x.is_ascii_whitespace())
    }

    fn tok_tree(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, st: &mut Ts<'t>) -> Step {
        let delim = self.delim(n);
        if st.hash && delim == Tk::LBracket {
            // Attribute: `#[...]`.
            self.resolve(st, Next::Other);
            st.keep_elem = true;
            st.keep_mods = true;
            if self.attr_tree_is_test(n) {
                st.attr_test = true;
            }
            return Step::Next;
        }
        if st.prev == P::Kw && st.last_tk == Tk::Pub && delim == Tk::LParen {
            // `pub(crate)`
            st.keep_mods = true;
            st.keep_elem = true;
            return Step::Next;
        }
        st.nonempty = true;
        if let Some(mp) = st.mac.take() {
            // `name!(...)` inside a token tree.
            self.cx.add_ref(mp.name, RefKind::MacroCall, line_of(mp.node), mp.qual, 255);
            let mname = self.cx.text(mp.node);
            self.macro_tokens(c, mname, st.m.code, st.m.defs);
            st.recv = NONE;
            st.prev = P::Value;
            return Step::Next;
        }
        let was_arm_body = st.arms && !st.pat && st.prev == P::Punct && st.last_tk == Tk::FatArrow;
        let next = match delim {
            Tk::LParen => Next::Paren,
            Tk::LBrace => Next::Brace,
            Tk::LBracket => Next::Bracket,
            _ => Next::Other,
        };
        let res = self.resolve(st, next);
        let mut m = Tm { body: Body::Code, code: st.m.code, defs: false, pat: false };
        let mut push = false;
        match res {
            Res::Call(_) => m.code = true,
            Res::Lit => {}
            Res::Variant(_) => {
                m.body = if delim == Tk::LBrace { Body::Fields } else { Body::Types };
                m.defs = st.m.defs;
            }
            Res::None => {
                if st.pat {
                    m.pat = true;
                } else {
                    match st.m.body {
                        Body::Params => {
                            if st.after_colon {
                                m.body = Body::Types;
                            } else {
                                m.pat = true;
                            }
                        }
                        Body::Fields | Body::Types | Body::Variants => m.body = Body::Types,
                        Body::Code => {
                            if delim == Tk::LBrace {
                                m.defs = st.m.defs;
                                push = true;
                            }
                        }
                    }
                }
            }
        }
        self.want_done(st, NONE);
        if let Res::Variant(d) = res {
            // Payload of an enum variant: fields belong to the variant.
            if d != NONE {
                let dm = self.cx.def_mut(d);
                dm.end_line = end_line_of(n);
                if delim == Tk::LParen {
                    dm.sig_len = clamp_u16(n.end_byte().saturating_sub(dm.sig_off as usize));
                }
            }
            let saved = self.enter(d, Scope::Type);
            self.tokens(c, m, Init::Plain);
            self.leave(d, saved);
        } else {
            if push {
                self.cx.push_scope();
            }
            let cnt = self.tokens(c, m, Init::Plain);
            if push {
                self.cx.pop_scope();
            }
            if let Res::Call(i) = res {
                if let Some(r) = self.cx.facts.refs.get_mut(i) {
                    r.nargs = cnt;
                }
            }
        }
        if delim == Tk::LBrace {
            st.cond = false;
            st.blk_end = n.end_byte() as u32;
            if was_arm_body {
                // The block of a match arm: the next arm starts with a pattern.
                st.pat = true;
            }
        }
        st.recv = NONE;
        st.prev = P::Value;
        Step::Next
    }

    /// Is the attribute `#[..]` whose bracket tree is `tt` a test marker?
    fn attr_tree_is_test(&self, tt: Node<'t>) -> bool {
        let mut last: &[u8] = b"";
        for i in 0..tt.child_count().min(12) {
            let Some(ch) = tt.child(i) else { break };
            match self.k(ch) {
                // `test`, `tokio::test`: the last segment of the attribute's path counts.
                K::Identifier => last = self.cx.text(ch),
                K::TokenTree => return last == b"cfg" && self.cfg_has_test(ch, 0),
                K::Tok if matches!(self.tk(ch), Tk::PathSep | Tk::LBracket | Tk::Close) => {}
                _ => break,
            }
        }
        is_test_attr(last)
    }

    /// Text of a path written in a token tree (`len` is what its segments add up to), with a
    /// leading `Self` replaced by the impl's type.
    fn chain_text(&mut self, start: u32, end: u32, len: u32) -> u32 {
        let (s, e) = (start as usize, end as usize);
        // Whitespace or comments inside the path: no usable text.
        if e <= s || e > self.cx.src.len() || (e - s) as u32 != len {
            return NONE;
        }
        let bytes = &self.cx.src[s..e];
        // `$crate::a::b`
        let bytes = bytes.strip_prefix(b"$").unwrap_or(bytes);
        let first_len = memchr::memmem::find(bytes, b"::").unwrap_or(bytes.len());
        let (first, rest) = bytes.split_at(first_len);
        let mut tmp = std::mem::take(&mut self.b.tmp);
        tmp.clear();
        if first == b"Self" && self.self_ty != NONE {
            tmp.extend_from_slice(self.cx.name_bytes(self.self_ty));
        } else if !first.is_empty() && !upper(first) && !self.b.imported.is_empty() {
            // A module the file imported under this name.
            let id = self.cx.intern(first);
            let id = self.import_target(id).unwrap_or(id);
            tmp.extend_from_slice(self.cx.name_bytes(id));
        } else {
            tmp.extend_from_slice(first);
        }
        tmp.extend_from_slice(rest);
        set_off_type(&mut tmp);
        let id = if tmp.is_empty() { NONE } else { self.cx.intern(&tmp) };
        self.b.tmp = tmp;
        id
    }

    /// Attach a declared type to whatever is waiting for one.
    fn want_done(&mut self, st: &mut Ts<'t>, ty: u32) {
        match std::mem::replace(&mut st.want, Want::None) {
            Want::None => {}
            Want::Param(name) => self.cx.declare(name, ty),
            Want::Def(d) => {
                if ty != NONE && d != NONE {
                    self.cx.def_mut(d).type_name = ty;
                }
            }
        }
    }

    fn close_field(&mut self, st: &mut Ts<'t>, at: Node<'t>) {
        if st.open_field == NONE {
            return;
        }
        let d = std::mem::replace(&mut st.open_field, NONE);
        let dm = self.cx.def_mut(d);
        dm.sig_len = clamp_u16(at.start_byte().saturating_sub(dm.sig_off as usize));
    }

    fn close_def(&mut self, st: &mut Ts<'t>, at: Node<'t>) {
        if st.open_def == NONE {
            return;
        }
        let d = std::mem::replace(&mut st.open_def, NONE);
        self.cx.def_mut(d).end_line = line_of(at);
        self.cx.pop_def();
        self.b.generics.truncate(st.open_gmark);
    }

    /// Decide what the pending identifier is, now that the token after it is known.
    fn resolve(&mut self, st: &mut Ts<'t>, next: Next) -> Res {
        let Some(p) = st.pend.take() else { return Res::None };
        let node = p.node;
        if p.ctx == PCtx::Dot {
            // Member access.
            let name = self.ident(node);
            let line = line_of(node);
            return match next {
                Next::Paren => {
                    self.cx.add_ref(name, RefKind::MemberCall, line, p.recv, 255);
                    Res::Call(self.cx.facts.refs.len() - 1)
                }
                Next::PathSep => {
                    // `.collect::<T>()`
                    self.cx.add_ref(name, RefKind::MemberCall, line, p.recv, 255);
                    Res::None
                }
                Next::Eq | Next::AssignOp => {
                    self.cx.add_ref(name, RefKind::MemberWrite, line, p.recv, 255);
                    Res::None
                }
                _ => {
                    self.cx.add_ref(name, RefKind::MemberRead, line, p.recv, 255);
                    if next == Next::Dot {
                        st.recv = self.field_path(p.recv, self.cx.text(node));
                    }
                    Res::None
                }
            };
        }
        let txt = self.cx.text(node);
        if next == Next::PathSep {
            // Not the last segment of its path.
            if p.ctx == PCtx::Turbo {
                // `Type::<T>::Assoc::name`: the path goes on after generic arguments.
                self.qualifier_ref(node, p.qual);
                st.chain_on = false;
                return Res::None;
            }
            let len = (node.end_byte() - node.start_byte()) as u32;
            if !st.chain_on || p.ctx == PCtx::Plain {
                st.chain_on = true;
                st.chain_start = node.start_byte() as u32;
                st.chain_len = len;
                st.chain_pend = st.chain_start;
                st.chain_plen = 0;
            } else {
                st.chain_pend = st.chain_end;
                st.chain_plen = st.chain_len;
                st.chain_len += len + 2;
            }
            st.chain_end = node.end_byte() as u32;
            st.chain_last = if p.silent { None } else { Some(node) };
            return Res::None;
        }
        let (in_path, qual) = match p.ctx {
            PCtx::Turbo => (p.qual != NONE, p.qual),
            PCtx::Path if st.chain_on => {
                let q = self.chain_text(st.chain_start, st.chain_end, st.chain_len);
                // The segment in front names a type when capitalised.
                if let Some(h) = st.chain_last.take() {
                    if upper(self.cx.text(h)) {
                        let hq = if st.chain_plen > 0 { self.chain_text(st.chain_start, st.chain_pend, st.chain_plen) } else { NONE };
                        self.qualifier_ref(h, hq);
                    }
                }
                (true, q)
            }
            _ => (false, NONE),
        };
        st.chain_on = false;
        if p.silent {
            if next == Next::Dot && !in_path && self.k(node) == K::SelfKw {
                st.recv = self.self_ty;
            }
            return Res::None;
        }
        let name = self.ident(node);
        let line = line_of(node);
        let up = upper(txt);
        let is_self = name == self.ids.self_upper;
        // Enum variant definition.
        if st.m.body == Body::Variants && p.first && !in_path {
            if !st.m.defs {
                return Res::Variant(NONE);
            }
            let d = self.syn_def(name, DefKind::EnumMember, pos_of(node), node.end_byte(), line, self.inherit_pub);
            return Res::Variant(d);
        }
        if st.pat {
            // Pattern position: lower-case names are bindings.
            match next {
                Next::Paren | Next::Brace => {
                    if up && !is_self && !self.is_stop(name) && self.generic(name).is_none() {
                        self.cx.add_ref(name, if next == Next::Brace { RefKind::Type } else { RefKind::Read }, line, qual, 255);
                    }
                }
                Next::Colon if st.m.pat && st.delim == Tk::LBrace => {
                    // Field name of a struct pattern.
                }
                _ => {
                    if in_path || up {
                        if !self.is_stop(name) && self.generic(name).is_none() {
                            self.cx.add_ref(name, RefKind::Read, line, qual, 255);
                        }
                    } else if name != self.ids.underscore {
                        self.cx.declare(name, NONE);
                    }
                }
            }
            return Res::None;
        }
        match next {
            Next::Paren => {
                if p.dsl {
                    return Res::None;
                }
                let name = if is_self {
                    if self.self_ty == NONE {
                        return Res::None;
                    }
                    self.self_ty
                } else {
                    name
                };
                if self.is_prelude_variant(name, qual) || (!in_path && self.cx.is_local(name)) {
                    return Res::None;
                }
                self.cx.add_ref(name, RefKind::Call, line, qual, 255);
                Res::Call(self.cx.facts.refs.len() - 1)
            }
            Next::Brace if up && !st.cond && !p.dsl && st.m.body == Body::Code => {
                // Struct literal.
                let name = if is_self { self.self_ty } else { name };
                if name != NONE && !self.is_stop(name) && self.generic(name).is_none() {
                    self.cx.add_ref(name, RefKind::New, line, qual, 255);
                }
                Res::Lit
            }
            Next::Eq => Res::None,
            Next::Colon if !in_path => {
                match st.m.body {
                    Body::Params if !up => st.want = Want::Param(name),
                    Body::Fields if p.first && st.m.defs => {
                        let start = p.mods.unwrap_or_else(|| pos_of(node));
                        let d = self.syn_def(name, DefKind::Field, start, node.end_byte(), line, p.mods_pub);
                        st.open_field = d;
                        st.want = Want::Def(d);
                    }
                    _ => {}
                }
                Res::None
            }
            _ => {
                let types = st.m.body != Body::Code && st.open_def == NONE;
                if st.want != Want::None && (up || in_path) {
                    // The head of a declared type; `Box<T>` and friends stand for `T`.
                    if !(next == Next::Lt && is_deref_wrapper(txt)) {
                        let ty = if is_self {
                            self.self_ty
                        } else {
                            match self.generic(name) {
                                Some(b) => b,
                                None => name,
                            }
                        };
                        self.want_done(st, ty);
                    }
                }
                if in_path {
                    if !self.is_prelude_variant(name, qual) {
                        self.cx.add_ref(name, if types { RefKind::Type } else { RefKind::Read }, line, qual, 255);
                    }
                } else if is_self || self.is_stop(name) || self.generic(name).is_some() {
                } else if up {
                    self.cx.add_ref(name, if types { RefKind::Type } else { RefKind::Read }, line, NONE, 255);
                    if next == Next::Dot {
                        let mut buf = std::mem::take(&mut self.cx.s.buf);
                        buf.clear();
                        buf.push(b'$');
                        buf.extend_from_slice(self.cx.name_bytes(name));
                        st.recv = self.cx.intern(&buf);
                        self.cx.s.buf = buf;
                    }
                } else if let Some(ty) = self.cx.local_type(name) {
                    if next == Next::Dot {
                        st.recv = self.outer(ty);
                    }
                } else if (st.m.code && !p.dsl && !types && next != Next::Dot) || self.imported(name) {
                    self.cx.add_ref(name, RefKind::Read, line, NONE, 255);
                }
                Res::None
            }
        }
    }

    // ----- items written inside token trees -----

    /// A definition synthesised from tokens; its extent is fixed up as the item is scanned.
    fn syn_def(&mut self, name: u32, kind: DefKind, pos: (u32, u32), sig_end: usize, end_line: u32, exported: bool) -> u32 {
        let mut flags = dflag::MACRO_DEFINED;
        if exported {
            flags |= dflag::EXPORTED;
        }
        if self.test_ctx {
            flags |= dflag::TEST;
        }
        let callable = matches!(kind, DefKind::Function | DefKind::Method | DefKind::Macro);
        self.cx.add_def(Def {
            name,
            parent: self.parent,
            qual: NONE,
            line: pos.1,
            end_line: end_line.max(pos.1),
            sig_off: pos.0,
            sig_len: clamp_u16(sig_end.saturating_sub(pos.0 as usize)),
            kind,
            flags,
            type_name: NONE,
            arity_min: 0,
            arity_max: if callable { 255 } else { 0 },
        })
    }

    /// References recorded while scanning an item's header, before its definition existed,
    /// belong to that definition.
    fn adopt_refs(&mut self, from: usize, d: u32) {
        let outer = self.cx.encl();
        for r in self.cx.facts.refs.iter_mut().skip(from) {
            if r.encl == outer {
                r.encl = d;
            }
        }
    }

    fn syn_span(&mut self, d: u32, sig_end: usize, end_line: u32) {
        if d == NONE {
            return;
        }
        let dm = self.cx.def_mut(d);
        dm.sig_len = clamp_u16(sig_end.saturating_sub(dm.sig_off as usize));
        dm.end_line = end_line.max(dm.line);
    }

    fn syn_type_ref(&mut self, n: Node<'t>) {
        if self.k(n) != K::Identifier {
            return;
        }
        let name = self.ident(n);
        if self.is_stop(name) || self.generic(name).is_some() {
            return;
        }
        self.cx.add_ref(name, RefKind::Type, line_of(n), NONE, 255);
    }

    /// Consume the tokens of an item header up to its body, a `;` or an `=`: registers generic
    /// parameters and records the types it mentions. The cursor ends on the terminator.
    fn syn_header(&mut self, c: &mut TreeCursor<'t>, o: HOpts) -> Hdr<'t> {
        let mut h = Hdr { end: HEnd::End, name: None, tr: None, anon: None, ty: NONE };
        let mut angle = 0i32;
        let mut generics = o.generics;
        let mut prev = Tk::Punct;
        let mut prev_tok = false;
        let mut lifetime = false;
        let mut frozen = false;
        let mut for_pending = false;
        let mut want_ty = false;
        let mut dollar = false;
        // Start of the path the current token continues (`a::b::Name`).
        let mut path_at: Option<usize> = None;
        while c.goto_next_sibling() {
            let n = c.node();
            let id = n.kind_id();
            let mut k = self.t.kinds.get(id);
            if k == K::Comment {
                continue;
            }
            let tk = if k == K::Tok { self.t.tok(id) } else { Tk::Punct };
            if dollar && k == K::Identifier {
                // `$name` in the body of a `macro` item.
                k = K::Metavariable;
            }
            dollar = k == K::Tok && tk == Tk::Dollar;
            if for_pending {
                for_pending = false;
                if !(k == K::Tok && tk == Tk::Lt) {
                    // `impl Trait for Type` (not `for<'a>`).
                    h.tr = h.name.take();
                }
            }
            let mut keep_want = false;
            match k {
                K::Tok => match tk {
                    Tk::Lt => {
                        angle += 1;
                        if want_ty && o.stop_eq && h.ty != NONE && is_deref_wrapper(self.cx.name_bytes(h.ty)) {
                            // `Box<T>`: the declared type is `T`.
                            h.ty = NONE;
                            keep_want = true;
                        }
                    }
                    Tk::Gt | Tk::Shr => {
                        angle -= if tk == Tk::Shr { 2 } else { 1 };
                        if angle <= 0 {
                            generics = false;
                        }
                    }
                    Tk::Semi if angle <= 0 => {
                        h.end = HEnd::Semi(n);
                        return h;
                    }
                    Tk::Eq if o.stop_eq && angle <= 0 => {
                        h.end = HEnd::Eq(n);
                        return h;
                    }
                    Tk::Quote => {
                        lifetime = true;
                        keep_want = true;
                    }
                    Tk::Arrow if angle <= 0 => {
                        want_ty = true;
                        keep_want = true;
                        h.ty = NONE;
                    }
                    Tk::Colon if o.stop_eq && angle <= 0 => {
                        want_ty = true;
                        keep_want = true;
                    }
                    Tk::For if angle <= 0 => for_pending = true,
                    Tk::Where => frozen = true,
                    Tk::PathSep | Tk::Amp | Tk::Star => keep_want = true,
                    _ => {}
                },
                K::Identifier => {
                    let txt = self.cx.text(n);
                    keep_want = true;
                    if lifetime {
                        lifetime = false;
                    } else if matches!(txt, b"dyn" | b"extern" | b"in" | b"ref") {
                    } else if generics && angle == 1 && prev_tok && matches!(prev, Tk::Lt | Tk::Comma | Tk::Const) {
                        let name = self.ident(n);
                        self.b.generics.push((name, NONE));
                    } else if upper(txt) {
                        if want_ty && angle <= 0 && (h.ty == NONE || (prev_tok && prev == Tk::PathSep)) {
                            let name = self.ident(n);
                            h.ty = if prev_tok && prev == Tk::PathSep {
                                // `fmt::Result` keeps its module; `Self::Output` has no fixed name.
                                match path_at {
                                    Some(s) if s + 2 <= n.start_byte() => self.syn_module_type(s, n.start_byte() - 2, name),
                                    _ => name,
                                }
                            } else if name == self.ids.self_upper {
                                self.self_ty
                            } else {
                                match self.generic(name) {
                                    Some(b) => b,
                                    None => name,
                                }
                            };
                        }
                        if o.hold && angle <= 0 && !frozen {
                            h.anon = None;
                            if let Some(x) = h.name.replace(n) {
                                self.syn_type_ref(x);
                            }
                        } else {
                            self.syn_type_ref(n);
                        }
                    }
                }
                K::PrimitiveType => {
                    if o.hold && angle <= 0 && !frozen {
                        h.anon = None;
                        if let Some(x) = h.name.replace(n) {
                            self.syn_type_ref(x);
                        }
                    }
                }
                K::Metavariable => {
                    // A type only known at expansion.
                    if o.hold && angle <= 0 && !frozen {
                        h.anon = None;
                        if let Some(x) = h.name.take() {
                            self.syn_type_ref(x);
                        }
                    }
                }
                K::MutableSpecifier => keep_want = true,
                K::TokenTree | K::TokenRepetition => {
                    let delim = if k == K::TokenTree { self.delim(n) } else { Tk::Punct };
                    match delim {
                        Tk::LParen if o.stop_paren && angle <= 0 => {
                            h.end = HEnd::Params(n);
                            return h;
                        }
                        Tk::LBrace if angle <= 0 => {
                            h.end = HEnd::Body(n);
                            return h;
                        }
                        Tk::LBrace => {
                            self.cx.push_scope();
                            self.tokens(c, Tm { body: Body::Code, code: true, defs: false, pat: false }, Init::Plain);
                            self.cx.pop_scope();
                        }
                        _ => {
                            self.tokens(c, Tm { body: Body::Types, code: false, defs: false, pat: false }, Init::Plain);
                            if o.hold && angle <= 0 && !frozen && k == K::TokenTree {
                                // `impl Trait for [T]`, `for (A, B)`: types without a name of
                                // their own get the name rustdoc files them under.
                                if let Some(x) = h.name.take() {
                                    self.syn_type_ref(x);
                                }
                                let semi = (0..n.child_count()).any(|i| n.child(i).is_some_and(|x| self.k(x) == K::Tok && self.tk(x) == Tk::Semi));
                                h.anon = match delim {
                                    Tk::LParen if n.child_count() <= 2 => Some(b"unit"),
                                    Tk::LParen => Some(b"tuple"),
                                    Tk::LBracket if semi => Some(b"array"),
                                    Tk::LBracket => Some(b"slice"),
                                    _ => None,
                                };
                            }
                        }
                    }
                }
                _ => {}
            }
            if !keep_want {
                want_ty = false;
            }
            match k {
                K::Identifier | K::Crate | K::SelfKw | K::Super | K::Metavariable => {
                    if path_at.is_none() || !(prev_tok && prev == Tk::PathSep) {
                        path_at = Some(n.start_byte());
                    }
                }
                K::Tok if tk == Tk::PathSep => {}
                _ => path_at = None,
            }
            prev = tk;
            prev_tok = k == K::Tok;
        }
        h
    }

    /// `module|Name` for a type written `module::Name` in a token tree, where the module path
    /// spans the bytes `start..end`; `NONE` when that path goes through a type.
    fn syn_module_type(&mut self, start: usize, end: usize, name: u32) -> u32 {
        if end <= start || end > self.cx.src.len() || self.cx.src[start..end].iter().any(|b| b.is_ascii_whitespace() || *b == b'<') {
            return name;
        }
        let q = self.chain_text(start as u32, end as u32, (end - start) as u32);
        if q == NONE {
            return name;
        }
        let qb = self.cx.name_bytes(q);
        if memchr::memchr(b'|', qb).is_some() || path_type_at(qb).is_some() {
            return NONE;
        }
        let mut tmp = std::mem::take(&mut self.b.tmp);
        tmp.clear();
        tmp.extend_from_slice(self.cx.name_bytes(q));
        tmp.push(b'|');
        tmp.extend_from_slice(self.cx.name_bytes(name));
        let id = self.cx.intern(&tmp);
        self.b.tmp = tmp;
        id
    }

    fn syn_named(&mut self, kw: Tk, name: Option<Node<'t>>, c: &mut TreeCursor<'t>, st: &mut Ts<'t>) -> Step {
        self.resolve(st, Next::Other);
        st.chain_on = false;
        st.recv = NONE;
        st.kw = Tk::Punct;
        let defs = st.m.defs;
        let (pos, exported, is_async) = (st.kw_pos, st.kw_pub, st.kw_async);
        // `#[test] fn ..` / `#[cfg(test)] mod ..` written inside the macro.
        let outer_test = self.test_ctx;
        if st.kw_test {
            self.test_ctx = true;
        }
        st.kw_test = false;
        let step = match kw {
            Tk::Fn => self.syn_fn(name, c, defs, pos, exported, is_async),
            Tk::Struct | Tk::Union | Tk::Enum | Tk::Trait => self.syn_type(kw, name, c, defs, pos, exported),
            Tk::Mod => self.syn_mod(name, c, defs, pos, exported),
            Tk::Const | Tk::Static | Tk::Type => self.syn_stmt(kw, name, c, st, defs, pos, exported),
            _ => Step::Next,
        };
        self.test_ctx = outer_test;
        st.prev = P::Value;
        st.set_elem = true;
        st.countable = false;
        step
    }

    /// `fn name<G>(params) -> Ret where .. { body }` written in a token tree (cursor on the name).
    fn syn_fn(&mut self, name: Option<Node<'t>>, c: &mut TreeCursor<'t>, defs: bool, pos: (u32, u32), exported: bool, is_async: bool) -> Step {
        let gmark = self.b.generics.len();
        let rmark = self.cx.facts.refs.len();
        let h1 = self.syn_header(c, HOpts { stop_paren: true, stop_eq: false, generics: true, hold: false });
        let params = match h1.end {
            HEnd::Params(p) => p,
            HEnd::End => {
                self.b.generics.truncate(gmark);
                return Step::End;
            }
            _ => {
                self.b.generics.truncate(gmark);
                return Step::Next;
            }
        };
        let d = match name {
            Some(nm) if defs => {
                let id = self.ident(nm);
                let exported = exported || (self.scope == Scope::Trait && self.inherit_pub);
                let d = self.syn_def(id, DefKind::Function, pos, params.end_byte(), end_line_of(params), exported);
                if is_async {
                    self.cx.def_mut(d).flags |= dflag::ASYNC;
                }
                self.adopt_refs(rmark, d);
                d
            }
            _ => NONE,
        };
        let outer = self.scope;
        let saved = self.enter(d, Scope::Fn);
        self.cx.push_scope();
        self.fn_depth += 1;
        self.saw_self = false;
        self.tokens(c, Tm { body: Body::Params, code: false, defs: false, pat: false }, Init::Plain);
        let has_self = self.saw_self;
        let h2 = self.syn_header(c, HOpts { stop_paren: false, stop_eq: false, generics: false, hold: false });
        let (sig_end, end_line, has_body) = match h2.end {
            HEnd::Body(b) => (b.start_byte(), end_line_of(b), true),
            HEnd::Semi(s) => (s.start_byte(), line_of(s), false),
            _ => (c.node().start_byte(), line_of(c.node()), false),
        };
        if d != NONE {
            self.syn_span(d, sig_end, end_line);
            let arity = self.syn_param_count(params);
            let dm = self.cx.def_mut(d);
            dm.type_name = h2.ty;
            if has_self || matches!(outer, Scope::Impl | Scope::Trait) {
                dm.kind = DefKind::Method;
            }
            if let Some(n) = arity {
                let n = n.saturating_sub(has_self as u32).min(254) as u8;
                dm.arity_min = n;
                dm.arity_max = n;
            }
            if !has_body {
                dm.flags |= if outer == Scope::Trait { dflag::DECL | dflag::ABSTRACT } else { dflag::DECL };
            }
        }
        if has_body {
            self.tokens(c, Tm { body: Body::Code, code: true, defs, pat: false }, Init::Plain);
        }
        self.fn_depth -= 1;
        self.cx.pop_scope();
        self.leave(d, saved);
        self.b.generics.truncate(gmark);
        if matches!(h2.end, HEnd::End) { Step::End } else { Step::Next }
    }

    /// Number of parameters written in the token tree of a parameter list (a `self` receiver
    /// included); `None` when a repetition or `...` leaves it open.
    fn syn_param_count(&self, params: Node<'t>) -> Option<u32> {
        let mut angle = 0i32;
        let mut n = 0u32;
        let mut nonempty = false;
        for i in 0..params.child_count() {
            let x = params.child(i)?;
            match self.k(x) {
                K::Comment => continue,
                K::TokenRepetition => return None,
                K::Tok => match self.tk(x) {
                    Tk::LParen | Tk::Close => continue,
                    Tk::Lt => angle += 1,
                    Tk::Gt => angle -= 1,
                    Tk::Shr => angle -= 2,
                    Tk::Comma if angle <= 0 => {
                        n += nonempty as u32;
                        nonempty = false;
                        continue;
                    }
                    _ => {
                        if self.cx.text(x) == b"..." {
                            return None;
                        }
                    }
                },
                _ => {}
            }
            nonempty = true;
        }
        Some(n + nonempty as u32)
    }

    /// `struct` / `union` / `enum` / `trait` written in a token tree (cursor on the name).
    fn syn_type(&mut self, kw: Tk, name: Option<Node<'t>>, c: &mut TreeCursor<'t>, defs: bool, pos: (u32, u32), exported: bool) -> Step {
        let kind = match kw {
            Tk::Struct => DefKind::Struct,
            Tk::Union => DefKind::Union,
            Tk::Enum => DefKind::Enum,
            _ => DefKind::Trait,
        };
        let gmark = self.b.generics.len();
        let name_id = match name {
            Some(nm) => self.ident(nm),
            None => NONE,
        };
        let d = match name {
            Some(nm) if defs => self.syn_def(name_id, kind, pos, nm.end_byte(), line_of(nm), exported),
            _ => NONE,
        };
        let saved = self.enter(d, if kw == Tk::Trait { Scope::Trait } else { Scope::Type });
        self.self_ty = name_id;
        self.inherit_pub = exported;
        let h = self.syn_header(c, HOpts { stop_paren: false, stop_eq: false, generics: true, hold: false });
        let step = match h.end {
            HEnd::Body(b) => {
                self.syn_span(d, b.start_byte(), end_line_of(b));
                let body = match kw {
                    Tk::Enum => Body::Variants,
                    Tk::Trait => Body::Code,
                    _ => Body::Fields,
                };
                self.tokens(c, Tm { body, code: false, defs, pat: false }, Init::Plain);
                Step::Next
            }
            HEnd::Semi(s) => {
                self.syn_span(d, s.start_byte(), line_of(s));
                Step::Next
            }
            HEnd::End => {
                let l = c.node();
                self.syn_span(d, l.start_byte(), line_of(l));
                Step::End
            }
            _ => Step::Next,
        };
        self.leave(d, saved);
        self.b.generics.truncate(gmark);
        step
    }

    /// `impl<G> Trait for Type { .. }` written in a token tree (cursor on `impl`).
    fn syn_impl(&mut self, kw: Node<'t>, c: &mut TreeCursor<'t>, st: &mut Ts<'t>) -> Step {
        let defs = st.m.defs;
        let pos = st.mods.unwrap_or_else(|| pos_of(kw));
        st.chain_on = false;
        st.prev = P::Value;
        st.set_elem = true;
        st.countable = false;
        let gmark = self.b.generics.len();
        let rmark = self.cx.facts.refs.len();
        let h = self.syn_header(c, HOpts { stop_paren: false, stop_eq: false, generics: true, hold: true });
        let tr_id = match h.tr {
            Some(x) if self.k(x) == K::Identifier => self.ident(x),
            _ => NONE,
        };
        // (definition name, `Self` type)
        let (mut name, mut self_ty) = (NONE, NONE);
        if let Some(x) = h.name {
            if self.k(x) == K::PrimitiveType {
                name = self.cx.name(x);
                self_ty = name;
            } else {
                let id = self.ident(x);
                match self.generic(id) {
                    // Blanket impl: filed under the trait.
                    Some(bound) => {
                        name = tr_id;
                        self_ty = bound;
                    }
                    None => {
                        name = id;
                        self_ty = id;
                        self.syn_type_ref(x);
                    }
                }
            }
        } else if let Some(a) = h.anon {
            name = self.cx.intern(a);
            self_ty = name;
        }
        let step = match h.end {
            HEnd::Body(b) => {
                let d = if defs && name != NONE { self.syn_def(name, DefKind::Impl, pos, b.start_byte(), end_line_of(b), false) } else { NONE };
                if d != NONE {
                    self.adopt_refs(rmark, d);
                }
                let saved = self.enter(d, Scope::Impl);
                self.self_ty = self_ty;
                if let Some(x) = h.tr {
                    if tr_id != NONE && self.generic(tr_id).is_none() {
                        self.cx.add_ref(tr_id, if d != NONE { RefKind::Implements } else { RefKind::Type }, line_of(x), NONE, 255);
                    }
                }
                self.tokens(c, Tm { body: Body::Code, code: false, defs, pat: false }, Init::Plain);
                self.leave(d, saved);
                Step::Next
            }
            HEnd::End => Step::End,
            _ => Step::Next,
        };
        self.b.generics.truncate(gmark);
        step
    }

    /// `mod name;` / `mod name { .. }` written in a token tree (cursor on the name).
    fn syn_mod(&mut self, name: Option<Node<'t>>, c: &mut TreeCursor<'t>, defs: bool, pos: (u32, u32), exported: bool) -> Step {
        let Some(nm) = name else { return Step::Next };
        let id = self.ident(nm);
        if !c.goto_next_sibling() {
            return Step::End;
        }
        let n = c.node();
        match self.k(n) {
            K::TokenTree if self.delim(n) == Tk::LBrace => {
                let d = if defs { self.syn_def(id, DefKind::Module, pos, n.start_byte(), end_line_of(n), exported) } else { NONE };
                let saved = self.enter(d, Scope::Mod);
                self.self_ty = NONE;
                self.tokens(c, Tm { body: Body::Code, code: false, defs, pat: false }, Init::Plain);
                self.leave(d, saved);
            }
            K::Tok if self.tk(n) == Tk::Semi => {
                if defs {
                    let d = self.syn_def(id, DefKind::Module, pos, n.start_byte(), line_of(n), exported);
                    self.cx.def_mut(d).flags |= dflag::DECL;
                }
            }
            _ => {}
        }
        Step::Next
    }

    /// `const NAME: T = ..;`, `static NAME: T = ..;`, `type Name = ..;` written in a token tree
    /// (cursor on the name). The value is scanned by the caller, up to the `;`.
    fn syn_stmt(&mut self, kw: Tk, name: Option<Node<'t>>, c: &mut TreeCursor<'t>, st: &mut Ts<'t>, defs: bool, pos: (u32, u32), exported: bool) -> Step {
        let Some(nm) = name else { return Step::Next };
        if !defs || self.cx.text(nm) == b"_" {
            return Step::Next;
        }
        let kind = match kw {
            Tk::Const => DefKind::Constant,
            Tk::Static => DefKind::Variable,
            _ => DefKind::TypeAlias,
        };
        let id = self.ident(nm);
        let outer = self.scope;
        let d = self.syn_def(id, kind, pos, nm.end_byte(), line_of(nm), exported || (outer == Scope::Trait && self.inherit_pub));
        let gmark = self.b.generics.len();
        self.cx.push_def(d);
        let h = self.syn_header(c, HOpts { stop_paren: false, stop_eq: true, generics: kw == Tk::Type, hold: false });
        if kw != Tk::Type {
            self.cx.def_mut(d).type_name = h.ty;
        }
        match h.end {
            HEnd::Eq(e) => {
                self.syn_span(d, e.start_byte(), line_of(e));
                st.open_def = d;
                st.open_gmark = gmark;
                if kw == Tk::Type {
                    // The aliased type follows.
                    st.want = Want::Def(d);
                }
                Step::Next
            }
            HEnd::Semi(s) => {
                self.syn_span(d, s.start_byte(), line_of(s));
                self.cx.def_mut(d).flags |= if outer == Scope::Trait { dflag::DECL | dflag::ABSTRACT } else { dflag::DECL };
                self.cx.pop_def();
                self.b.generics.truncate(gmark);
                Step::Next
            }
            HEnd::End => {
                // `static NAME: T` as the last thing in the tree (`scoped_thread_local!`).
                let l = c.node();
                self.syn_span(d, l.start_byte(), line_of(l));
                self.cx.pop_def();
                self.b.generics.truncate(gmark);
                Step::End
            }
            _ => {
                self.cx.pop_def();
                self.b.generics.truncate(gmark);
                Step::Next
            }
        }
    }

    /// `macro_rules! name { .. }` written in a token tree (cursor on the name).
    fn syn_macro_rules(&mut self, name: Node<'t>, kw: Node<'t>, c: &mut TreeCursor<'t>, st: &mut Ts<'t>) -> Step {
        st.mac = None;
        st.chain_on = false;
        st.prev = P::Value;
        st.set_elem = true;
        let id = self.ident(name);
        if !c.goto_next_sibling() {
            return Step::End;
        }
        let tree = c.node();
        if self.k(tree) != K::TokenTree {
            return Step::Next;
        }
        let d = if st.m.defs { self.syn_def(id, DefKind::Macro, pos_of(kw), name.end_byte(), end_line_of(tree), false) } else { NONE };
        let saved = self.enter(d, Scope::Macro);
        self.cx.push_scope();
        self.tokens(c, Tm { body: Body::Code, code: true, defs: st.m.defs, pat: false }, Init::Rules);
        self.cx.pop_scope();
        self.leave(d, saved);
        Step::Next
    }

    /// `use a::b::{c, d as e};` written in a token tree (cursor on `use`): consumed through
    /// the `;`.
    fn tok_use(&mut self, c: &mut TreeCursor<'t>, reexport: bool) -> Step {
        let flags = if reexport { iflag::REEXPORT } else { 0 };
        let line = line_of(c.node());
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let ended = self.tok_use_seq(c, &mut buf, flags, line, true);
        self.cx.s.buf = buf;
        if ended { Step::End } else { Step::Next }
    }

    /// Clauses of a token-level use tree. At the top level the cursor walks the siblings after
    /// `use` up to the `;`; in a `{ .. }` group it walks the group's children. Returns true when
    /// the siblings ran out.
    fn tok_use_seq(&mut self, c: &mut TreeCursor<'t>, prefix: &mut Vec<u8>, flags: u8, line: u32, top: bool) -> bool {
        if self.cx.depth >= MAX_DEPTH {
            return false;
        }
        let base = prefix.len();
        // Start of the last segment within `prefix`, and the alias after `as`.
        let mut seg: Option<usize> = None;
        let mut alias: Option<Node<'t>> = None;
        let mut want_alias = false;
        let mut glob = false;
        let mut ended = true;
        if !top && !c.goto_first_child() {
            return false;
        }
        loop {
            if top && !c.goto_next_sibling() {
                break;
            }
            let n = c.node();
            let k = self.k(n);
            let mut flush = false;
            match k {
                K::Identifier | K::SelfKw | K::Super | K::Crate | K::Metavariable => {
                    if want_alias {
                        alias = Some(n);
                        want_alias = false;
                    } else if self.cx.text(n) == b"as" {
                        want_alias = true;
                    } else {
                        if !prefix.is_empty() {
                            prefix.extend_from_slice(b"::");
                        }
                        seg = Some(prefix.len());
                        let txt = self.cx.text(n);
                        prefix.extend_from_slice(if txt == b"$crate" { b"crate" } else { txt });
                    }
                }
                K::Tok => match self.tk(n) {
                    Tk::As => want_alias = true,
                    Tk::Star => glob = true,
                    Tk::Comma => flush = true,
                    Tk::Semi if top => {
                        ended = false;
                        flush = true;
                    }
                    _ => {}
                },
                K::TokenTree if self.delim(n) == Tk::LBrace => {
                    self.cx.depth += 1;
                    self.tok_use_seq(c, prefix, flags, line, false);
                    self.cx.depth -= 1;
                    seg = None;
                    prefix.truncate(base);
                }
                _ => {}
            }
            if flush {
                self.tok_use_leaf(prefix, base, seg.take(), alias.take(), glob, flags, line);
                glob = false;
                want_alias = false;
                prefix.truncate(base);
                if top && !ended {
                    // The `;` that closes the declaration.
                    break;
                }
            }
            if !top && !c.goto_next_sibling() {
                break;
            }
        }
        if !top {
            self.tok_use_leaf(prefix, base, seg.take(), alias.take(), glob, flags, line);
            prefix.truncate(base);
            c.goto_parent();
            return false;
        }
        ended
    }

    fn tok_use_leaf(&mut self, prefix: &[u8], base: usize, seg: Option<usize>, alias: Option<Node<'t>>, glob: bool, flags: u8, line: u32) {
        let alias = alias.map(|a| self.cx.text(a));
        if glob {
            if !prefix.is_empty() {
                self.import_leaf(prefix, b"", None, true, flags, line);
            }
            return;
        }
        let Some(s) = seg else { return };
        if s < base || s > prefix.len() {
            return;
        }
        // The last segment is the imported name, what precedes its `::` is the module.
        let head: &[u8] = if s >= 2 { &prefix[..s - 2] } else { b"" };
        self.import_leaf(head, &prefix[s..], alias, false, flags, line);
    }
}

// ---------------------------------------------------------------------------------------------
// Syntax the grammar does not know
// ---------------------------------------------------------------------------------------------

/// A `macro name(..) { .. }` item, rewritten into the invocation `name! { .. }` for the parser.
#[derive(Clone, Copy)]
struct DeclMacro {
    /// Offset of the name.
    name: u32,
    /// Offset of the first modifier (`pub`), or of `macro`.
    start: u32,
    exported: bool,
    /// `macro name { (pattern) => { .. } }` rather than `macro name(pattern) { .. }`.
    rules: bool,
}

fn reparse(text: &[u8]) -> Option<Tree> {
    PARSER.with(|cell| {
        let mut slot = cell.try_borrow_mut().ok()?;
        if slot.is_none() {
            let mut p = tree_sitter::Parser::new();
            p.set_language(&tree_sitter_rust::LANGUAGE.into()).ok()?;
            *slot = Some(p);
        }
        slot.as_mut()?.parse(text, None)
    })
}

#[inline]
fn is_ident_byte(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric() || b >= 0x80
}

fn skip_ws(src: &[u8], mut i: usize) -> usize {
    while i < src.len() && src[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn skip_blank(src: &[u8], mut i: usize) -> usize {
    while i < src.len() && (src[i] == b' ' || src[i] == b'\t') {
        i += 1;
    }
    i
}

fn word_end(src: &[u8], mut i: usize) -> usize {
    while i < src.len() && is_ident_byte(src[i]) {
        i += 1;
    }
    i
}

/// Is the word starting at `i` exactly `w`?
fn word_is(src: &[u8], i: usize, w: &[u8]) -> bool {
    src.get(i..i + w.len()) == Some(w) && !src.get(i + w.len()).is_some_and(|&b| is_ident_byte(b))
}

/// Do the `len` bytes at `i` form a whole word?
fn whole_word(src: &[u8], i: usize, len: usize) -> bool {
    (i == 0 || !is_ident_byte(src[i - 1])) && !src.get(i + len).is_some_and(|&b| is_ident_byte(b))
}

/// Offset of the delimiter that closes the one at `open`, looking at most `limit` bytes ahead.
fn matching(src: &[u8], open: usize, limit: usize) -> Option<usize> {
    let end = open.saturating_add(limit).min(src.len());
    let mut depth = 0i32;
    let mut i = open;
    while i < end {
        match src[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth <= 0 {
                    return Some(i);
                }
            }
            b'"' => {
                i += 1;
                while i < end && src[i] != b'"' {
                    if src[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Start of the visibility (`pub`, `pub(crate)`) that ends right before `i`, if there is one.
fn vis_before(src: &[u8], i: usize) -> Option<usize> {
    let mut p = i;
    while p > 0 && (src[p - 1] == b' ' || src[p - 1] == b'\t') {
        p -= 1;
    }
    if p == i || p == 0 {
        return None;
    }
    if src[p - 1] == b')' {
        // `pub(crate)`, `pub(in path)`
        let lo = p.saturating_sub(48);
        let open = src[lo..p - 1].iter().rposition(|&b| b == b'(' || b == b'\n')? + lo;
        if src[open] != b'(' {
            return None;
        }
        p = open;
    }
    if p >= 3 && &src[p - 3..p] == b"pub" && whole_word(src, p - 3, 3) { Some(p - 3) } else { None }
}

/// The copy of a file's text that is being rewritten: allocated on the first edit.
struct Patch<'s> {
    src: &'s [u8],
    out: Vec<u8>,
}

impl Patch<'_> {
    fn buf(&mut self) -> &mut [u8] {
        if self.out.is_empty() {
            self.out.extend_from_slice(self.src);
        }
        &mut self.out
    }

    fn blank(&mut self, from: usize, to: usize) {
        if from < to && to <= self.src.len() {
            for b in &mut self.buf()[from..to] {
                // Line structure stays: positions are reported as lines.
                if *b != b'\n' && *b != b'\r' {
                    *b = b' ';
                }
            }
        }
    }

    fn put(&mut self, at: usize, text: &[u8]) {
        if at + text.len() <= self.src.len() {
            self.buf()[at..at + text.len()].copy_from_slice(text);
        }
    }
}

/// Rewrite syntax the grammar does not know into something it parses, without moving a single
/// byte: unknown modifiers and bounds are blanked, a few constructs are swapped for known ones
/// of the same length. Returns the rewritten text, or `None` when nothing applied. The
/// extractor keeps reading names and signatures from the original text.
fn sanitize(src: &[u8], sites: &mut Vec<DeclMacro>) -> Option<Vec<u8>> {
    use memchr::memmem::find_iter;
    let mut p = Patch { src, out: Vec::new() };

    // `[const] Trait` and `~const Trait` bounds.
    for i in find_iter(src, b"[const]") {
        p.blank(i, i + 7);
    }
    for i in memchr::memchr_iter(b'~', src) {
        p.blank(i, i + 1);
    }
    for i in find_iter(src, b"const") {
        if !whole_word(src, i, 5) {
            continue;
        }
        let j = skip_ws(src, i + 5);
        if j == i + 5 {
            continue;
        }
        let hit = if word_is(src, j, b"impl") || word_is(src, j, b"trait") {
            // `const impl Trait for T`, `const trait Tr`
            true
        } else if word_is(src, j, b"unsafe") {
            let k = skip_ws(src, j + 6);
            word_is(src, k, b"impl") || word_is(src, k, b"trait")
        } else if src[j..].first() == Some(&b'|') {
            // `const |x| ..`: const closure
            true
        } else if word_is(src, j, b"move") {
            src.get(skip_ws(src, j + 4)) == Some(&b'|')
        } else {
            // `T: const Trait`, `+ const Trait`
            let mut q = i;
            while q > 0 && src[q - 1].is_ascii_whitespace() {
                q -= 1;
            }
            let after_bound = q > 0 && (src[q - 1] == b'+' || (src[q - 1] == b':' && (q < 2 || src[q - 2] != b':')));
            let named = src.get(j).is_some_and(|&b| b.is_ascii_alphabetic() || b == b'_' || b == b'?');
            after_bound && named && !word_is(src, j, b"fn") && !word_is(src, j, b"unsafe") && !word_is(src, j, b"extern") && !word_is(src, j, b"async")
        };
        if hit {
            p.blank(i, i + 5);
        }
    }
    // `safe fn` / `safe static` in `unsafe extern` blocks.
    for i in find_iter(src, b"safe") {
        if whole_word(src, i, 4) {
            let j = skip_ws(src, i + 4);
            if j > i + 4 && (word_is(src, j, b"fn") || word_is(src, j, b"static")) {
                p.blank(i, i + 4);
            }
        }
    }
    // `pub impl(crate) trait Tr`: restriction on who may implement.
    for i in find_iter(src, b"impl(") {
        if i > 0 && is_ident_byte(src[i - 1]) {
            continue;
        }
        let end = (i + 64).min(src.len());
        let Some(close) = src[i..end].iter().position(|&b| b == b')' || b == b'\n').map(|x| i + x) else { continue };
        if src[close] != b')' {
            continue;
        }
        let mut k = skip_ws(src, close + 1);
        while word_is(src, k, b"unsafe") || word_is(src, k, b"auto") || word_is(src, k, b"const") {
            k = skip_ws(src, word_end(src, k));
        }
        if word_is(src, k, b"trait") {
            p.blank(i, close + 1);
        }
    }
    for (word, next) in [(&b"auto"[..], &b"trait"[..]), (b"final", b"fn"), (b"super", b"let")] {
        for i in find_iter(src, word) {
            if whole_word(src, i, word.len()) {
                let j = skip_ws(src, i + word.len());
                if j > i + word.len() && word_is(src, j, next) {
                    p.blank(i, i + word.len());
                }
            }
        }
    }
    // Metavariable expressions `${count($x)}`, and `$$`.
    for i in memchr::memchr_iter(b'$', src) {
        match src.get(i + 1) {
            Some(b'$') => p.blank(i, i + 1),
            Some(b'{') => {
                let e = word_end(src, i + 2);
                let close = if e > i + 2 && src.get(e) == Some(&b'(') { matching(src, i + 1, 256) } else { None };
                match close {
                    Some(c) => p.blank(i, c + 1),
                    None => p.blank(i, i + 1),
                }
            }
            _ => {}
        }
    }
    // `1.0f16`, `2f128`: float types the grammar has no suffix for.
    for (suffix, repl) in [(&b"f16"[..], &b"f32"[..]), (b"f128", b"f64 ")] {
        for i in find_iter(src, suffix) {
            if i == 0 || src.get(i + suffix.len()).is_some_and(|&b| is_ident_byte(b)) {
                continue;
            }
            let mut s = i;
            while s > 0 && (src[s - 1].is_ascii_alphanumeric() || src[s - 1] == b'_' || src[s - 1] == b'.') {
                s -= 1;
            }
            if s < i && src[s].is_ascii_digit() {
                p.put(i, repl);
            }
        }
    }
    // `trait Alias = Bound + Other;` reads like a type alias.
    for i in find_iter(src, b"trait") {
        if !whole_word(src, i, 5) {
            continue;
        }
        let j = skip_ws(src, i + 5);
        let e = word_end(src, j);
        if j == i + 5 || e == j {
            continue;
        }
        let mut k = e;
        if src.get(k) == Some(&b'<') {
            let end = (k + 200).min(src.len());
            let mut depth = 0i32;
            let mut closed = false;
            while k < end {
                match src[k] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth -= 1;
                        if depth == 0 {
                            closed = true;
                            k += 1;
                            break;
                        }
                    }
                    b'{' | b';' => break,
                    _ => {}
                }
                k += 1;
            }
            if !closed {
                continue;
            }
        }
        let k = skip_ws(src, k);
        if src.get(k) == Some(&b'=') && !matches!(src.get(k + 1), Some(b'=') | Some(b'>')) {
            p.put(i, b"type ");
        }
    }
    // `pub type Opaque;` in extern blocks: the grammar takes no visibility there.
    for i in find_iter(src, b"type") {
        if !whole_word(src, i, 4) {
            continue;
        }
        let Some(vis) = vis_before(src, i) else { continue };
        let j = skip_ws(src, i + 4);
        let e = word_end(src, j);
        if e > j && src.get(skip_ws(src, e)) == Some(&b';') {
            p.blank(vis, i);
        }
    }
    // `macro name(params) { body }` and `macro name { (params) => { body } }`: shown as the
    // invocation `name! { .. }`, whose token tree is the body (or the rules).
    for i in find_iter(src, b"macro") {
        if !whole_word(src, i, 5) {
            continue;
        }
        let n0 = skip_blank(src, i + 5);
        let n1 = word_end(src, n0);
        if n0 == i + 5 || n1 == n0 || src[n0].is_ascii_digit() {
            continue;
        }
        let vis = vis_before(src, i);
        let start = vis.unwrap_or(i);
        // Items start a line; prose mentioning "macro foo(..)" does not.
        let mut ls = start;
        while ls > 0 && (src[ls - 1] == b' ' || src[ls - 1] == b'\t') {
            ls -= 1;
        }
        if ls > 0 && src[ls - 1] != b'\n' {
            continue;
        }
        if src.get(n1) == Some(&b'(') {
            let Some(close) = matching(src, n1, 1 << 14) else { continue };
            if src.get(skip_ws(src, close + 1)) != Some(&b'{') {
                continue;
            }
            p.blank(start, n0);
            p.blank(n1, close + 1);
            p.put(n1, b"!");
            sites.push(DeclMacro { name: n0 as u32, start: start as u32, exported: vis.is_some(), rules: false });
        } else {
            let m = skip_blank(src, n1);
            if m > n1 && src.get(m) == Some(&b'{') {
                p.blank(start, n0);
                p.put(n1, b"!");
                sites.push(DeclMacro { name: n0 as u32, start: start as u32, exported: vis.is_some(), rules: true });
            }
        }
    }
    // `macro_rules! Name { derive() (..) => {..} }`: rules for use as a derive.
    for i in find_iter(src, b"derive()") {
        if whole_word(src, i, 6) && src.get(skip_ws(src, i + 8)) == Some(&b'(') {
            let mut q = i;
            while q > 0 && src[q - 1].is_ascii_whitespace() {
                q -= 1;
            }
            if q > 0 && matches!(src[q - 1], b'{' | b';') {
                p.blank(i, i + 8);
            }
        }
    }
    // `name: Type = default,` in a struct body (default field values).
    let mut ls = 0usize;
    while ls < src.len() {
        let le = memchr::memchr(b'\n', &src[ls..]).map_or(src.len(), |i| ls + i);
        if let Some((eq, end)) = default_field_value(&src[ls..le]) {
            p.blank(ls + eq, ls + end);
        }
        ls = le + 1;
    }
    if p.out.is_empty() { None } else { Some(p.out) }
}

/// If `line` reads `[pub] name: Type = value,`, the span of ` = value` (up to the comma).
fn default_field_value(line: &[u8]) -> Option<(usize, usize)> {
    let line_end = line.trim_ascii_end().len();
    if line_end < 6 || line[line_end - 1] != b',' {
        return None;
    }
    let mut i = skip_blank(line, 0);
    if word_is(line, i, b"pub") {
        i = skip_blank(line, i + 3);
        if line.get(i) == Some(&b'(') {
            i = skip_blank(line, i + line[i..line_end].iter().position(|&b| b == b')')? + 1);
        }
    }
    // Fields are lower-case; a capitalised name here is a generic parameter with a default.
    if !line.get(i).is_some_and(|&b| b.is_ascii_lowercase() || b == b'_') {
        return None;
    }
    let e = word_end(line, i);
    let colon = skip_blank(line, e);
    if line.get(colon) != Some(&b':') || line.get(colon + 1) == Some(&b':') {
        return None;
    }
    let mut depth = 0i32;
    let mut k = colon + 1;
    while k + 1 < line_end {
        match line[k] {
            b'<' | b'(' | b'[' | b'{' => depth += 1,
            b'>' if line[k - 1] == b'-' || line[k - 1] == b'=' => {}
            b'>' | b')' | b']' | b'}' => depth -= 1,
            b'"' | b'\'' if depth <= 0 && line[k] == b'"' => return None,
            b'=' if depth <= 0 => {
                // `==`, `=>`, `<=`, `>=`, `!=` are operators of a value, not a default.
                if matches!(line[k + 1], b'=' | b'>') || matches!(line[k - 1], b'<' | b'>' | b'!' | b'=') {
                    return None;
                }
                return Some((k, line_end - 1));
            }
            _ => {}
        }
        k += 1;
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Name tables
// ---------------------------------------------------------------------------------------------

/// Node kinds that consume the attributes in front of them (or leave them pending).
#[inline]
fn takes_attrs(k: K) -> bool {
    matches!(
        k,
        K::AttributeItem
            | K::Comment
            | K::FunctionItem
            | K::FunctionSignatureItem
            | K::StructItem
            | K::UnionItem
            | K::EnumItem
            | K::EnumVariant
            | K::FieldDeclaration
            | K::TraitItem
            | K::ImplItem
            | K::TypeItem
            | K::AssociatedType
            | K::ConstItem
            | K::StaticItem
            | K::ModItem
            | K::ForeignModItem
            | K::MacroDefinition
            | K::MacroInvocation
    )
}

/// `#[test]`, `#[tokio::test]`, `#[bench]`, `#[rstest]`, ...
fn is_test_attr(name: &[u8]) -> bool {
    matches!(name, b"bench" | b"quickcheck" | b"should_panic") || name.ends_with(b"test") || name.starts_with(b"test_")
}

/// Attributes built into the language or the compiler: not macros defined anywhere.
fn is_builtin_attr(name: &[u8]) -> bool {
    if name.starts_with(b"rustc_") {
        return true;
    }
    matches!(
        name,
        b"derive"
            | b"cfg"
            | b"cfg_attr"
            | b"test"
            | b"inline"
            | b"allow"
            | b"warn"
            | b"deny"
            | b"forbid"
            | b"expect"
            | b"doc"
            | b"must_use"
            | b"repr"
            | b"non_exhaustive"
            | b"deprecated"
            | b"automatically_derived"
            | b"rustfmt"
            | b"clippy"
            | b"ignore"
            | b"should_panic"
            | b"bench"
            | b"macro_export"
            | b"macro_use"
            | b"proc_macro"
            | b"proc_macro_derive"
            | b"proc_macro_attribute"
            | b"derive_const"
            | b"link"
            | b"link_name"
            | b"link_ordinal"
            | b"link_section"
            | b"no_link"
            | b"no_mangle"
            | b"export_name"
            | b"used"
            | b"crate_name"
            | b"crate_type"
            | b"no_main"
            | b"no_std"
            | b"no_core"
            | b"no_implicit_prelude"
            | b"no_builtins"
            | b"cold"
            | b"track_caller"
            | b"target_feature"
            | b"instruction_set"
            | b"naked"
            | b"optimize"
            | b"coverage"
            | b"no_sanitize"
            | b"sanitize"
            | b"path"
            | b"recursion_limit"
            | b"type_length_limit"
            | b"panic_handler"
            | b"global_allocator"
            | b"alloc_error_handler"
            | b"windows_subsystem"
            | b"feature"
            | b"debugger_visualizer"
            | b"collapse_debuginfo"
            | b"stable"
            | b"unstable"
            | b"lang"
            | b"may_dangle"
            | b"fundamental"
            | b"marker"
            | b"thread_local"
            | b"linkage"
            | b"start"
            | b"main"
            | b"prelude_import"
            | b"allow_internal_unstable"
            | b"allow_internal_unsafe"
            | b"const_trait"
            | b"ffi_const"
            | b"ffi_pure"
            | b"compiler_builtins"
            | b"needs_allocator"
            | b"needs_panic_runtime"
            | b"panic_runtime"
            | b"profiler_runtime"
            | b"register_tool"
            | b"cfi_encoding"
            | b"pointee"
            | b"unsafe"
            | b"cfg_eval"
            | b"cfg_accessible"
            | b"type_const"
            | b"loop_match"
            | b"const_continue"
            | b"define_opaque"
            | b"default_lib_allocator"
            | b"pattern_complexity_limit"
            | b"custom_mir"
            | b"pin_v2"
    )
}

/// Traits that add no methods: they do not tell where a method call on a bounded type goes.
fn is_marker_trait(name: &[u8]) -> bool {
    matches!(name, b"Send" | b"Sync" | b"Sized" | b"Unpin" | b"Copy" | b"UnwindSafe" | b"RefUnwindSafe" | b"Freeze" | b"DynSend" | b"DynSync")
}

/// Smart pointers that dereference to their argument: a member access on `Box<T>` is one on `T`.
fn is_deref_wrapper(name: &[u8]) -> bool {
    matches!(name, b"Box" | b"Rc" | b"Arc" | b"Pin" | b"Cow" | b"ManuallyDrop" | b"Ref" | b"RefMut" | b"MutexGuard" | b"RwLockReadGuard" | b"RwLockWriteGuard")
}

/// Wrappers that have methods of their own but hand out their argument through well-known
/// steps (`unwrap`, `borrow`, indexing, iteration).
fn is_wrapper(name: &[u8]) -> bool {
    is_unwrappable(name) || is_sequence(name) || matches!(name, b"RefCell" | b"Mutex" | b"RwLock")
}

/// `Option`, `Result` and its aliases (`io::Result`, `PResult`), and a lock guard of unknown
/// wrapping (`?`).
fn is_unwrappable(name: &[u8]) -> bool {
    name == b"Option" || name == b"?" || name.ends_with(b"Result")
}

/// Collections of one element type, and iterators over them (`iter`).
fn is_sequence(name: &[u8]) -> bool {
    matches!(name, b"Vec" | b"VecDeque" | b"ThinVec" | b"slice" | b"array" | b"iter")
}

/// Conventional constructors: `new`, `default`, `with_*`, `from*`.
fn is_ctor_name(name: &[u8]) -> bool {
    matches!(name, b"new" | b"default") || name.starts_with(b"with_") || name.starts_with(b"from")
}

fn is_primitive(name: &[u8]) -> bool {
    matches!(
        name,
        b"bool"
            | b"char"
            | b"str"
            | b"u8"
            | b"u16"
            | b"u32"
            | b"u64"
            | b"u128"
            | b"usize"
            | b"i8"
            | b"i16"
            | b"i32"
            | b"i64"
            | b"i128"
            | b"isize"
            | b"f16"
            | b"f32"
            | b"f64"
            | b"f128"
    )
}

/// Methods that, on a receiver that is not known to be a standard wrapper, still belong to
/// one: a type path through them leads nowhere.
fn is_std_method(name: &[u8]) -> bool {
    is_unwrap_family(name)
        || matches!(
            name,
            b"as_ref"
                | b"as_mut"
                | b"as_deref"
                | b"as_deref_mut"
                | b"borrow"
                | b"borrow_mut"
                | b"iter"
                | b"iter_mut"
                | b"into_iter"
                | b"cloned"
                | b"copied"
                | b"by_ref"
                | b"peekable"
                | b"collect"
                | b"into"
                | b"try_into"
                | b"to_string"
                | b"to_owned"
                | b"to_vec"
        )
}

/// Methods of `Option` / `Result` that a constructor's result goes through when it is fallible.
fn is_unwrap_family(name: &[u8]) -> bool {
    matches!(
        name,
        b"unwrap"
            | b"expect"
            | b"unwrap_or"
            | b"unwrap_or_else"
            | b"unwrap_or_default"
            | b"unwrap_unchecked"
            | b"unwrap_err"
            | b"expect_err"
            | b"is_some"
            | b"is_none"
            | b"is_ok"
            | b"is_err"
            | b"ok"
            | b"err"
            | b"ok_or"
            | b"ok_or_else"
            | b"map_err"
    )
}

/// Macros whose arguments are not Rust code worth scanning.
fn scans_tokens(name: &[u8]) -> bool {
    !matches!(
        name,
        b"stringify"
            | b"concat"
            | b"concat_idents"
            | b"cfg"
            | b"env"
            | b"option_env"
            | b"include"
            | b"include_str"
            | b"include_bytes"
            | b"compile_error"
            | b"asm"
            | b"global_asm"
            | b"naked_asm"
            | b"llvm_asm"
            | b"line"
            | b"column"
            | b"file"
            | b"module_path"
    )
}

/// Macros that build code for another crate: items inside are not definitions of this one.
fn is_quote_macro(name: &[u8]) -> bool {
    name.starts_with(b"quote") || name.starts_with(b"parse_quote")
}
