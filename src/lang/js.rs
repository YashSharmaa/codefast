//! JavaScript / TypeScript / TSX extractor (tree-sitter-javascript, tree-sitter-typescript).
//!
//! One walker serves the three grammars: node kind names are mostly shared, kind ids are not, so
//! every grammar gets its own `Tables`. Beyond plain definitions and call sites it understands the
//! idioms that make JS / TS code bases hard to navigate with text search:
//!   * functions that are values: arrow functions and function expressions bound to variables,
//!     class fields, object-literal properties, `exports.x = ...` and `X.prototype.m = ...`,
//!   * dependency injection through constructor parameter properties
//!     (`constructor(private readonly svc: FooService)` declares field `svc: FooService`), so that
//!     `this.svc.run()` carries the receiver path `Class::svc`,
//!   * ES module and CommonJS imports / exports: namespace aliases turn `ns.f()` into a call
//!     qualified by the import target, aliased named imports are recorded under the exported name,
//!   * callbacks bound into slots (`{ onClick: handler }`, `obj.cb = handler`, `<A onClick={h}/>`).
//!
//! Walking the tree is what costs: a cursor step is far more expensive than looking at a few
//! source bytes. The most frequent shapes (`this.x`, `a.b`, `: Type`) are therefore recognised
//! lexically, child loops are counted so that no step is spent on finding out that a node has no
//! further sibling, and the closing token of bracketed nodes is never stepped onto.

use crate::extract::*;
use crate::facts::*;
use crate::lang::Lang;
use rustc_hash::FxHashMap;
use std::marker::PhantomData;
use std::sync::OnceLock;
use tree_sitter::{Language, Node, Tree, TreeCursor};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    /// Named node without special handling: descend.
    Other,
    /// Anonymous token (keyword, punctuation): never has children.
    Anon,
    /// Subtree that never contains references.
    Skip,
    Error,
    Comment,
    Identifier,
    TypeIdentifier,
    PropertyIdentifier,
    ShorthandProperty,
    ShorthandPattern,
    NestedTypeIdentifier,
    NestedIdentifier,
    This,
    Super,
    MetaProperty,
    String,
    Number,
    Literal,
    TemplateString,
    TemplateSubstitution,
    Array,
    UnaryExpression,
    BinaryExpression,
    ImportStatement,
    ImportClause,
    NamespaceImport,
    NamedImports,
    ImportSpecifier,
    ImportRequireClause,
    ImportAlias,
    Import,
    ExportStatement,
    ExportClause,
    ExportSpecifier,
    NamespaceExport,
    FunctionDeclaration,
    FunctionExpression,
    ArrowFunction,
    FormalParameters,
    RequiredParameter,
    OptionalParameter,
    Modifier,
    ClassDeclaration,
    AbstractClass,
    Class,
    ClassBody,
    ClassHeritage,
    ExtendsClause,
    ImplementsClause,
    ClassStaticBlock,
    MethodDefinition,
    MethodSignature,
    AbstractMethodSignature,
    FieldDefinition,
    InterfaceDeclaration,
    ExtendsTypeClause,
    ObjectType,
    PropertySignature,
    FunctionType,
    SignatureType,
    IndexSignature,
    MappedTypeClause,
    InferType,
    TypeAliasDeclaration,
    EnumDeclaration,
    EnumBody,
    EnumAssignment,
    Namespace,
    AmbientDeclaration,
    VariableDeclaration,
    LexicalDeclaration,
    VariableDeclarator,
    ExpressionStatement,
    StatementBlock,
    ForStatement,
    ForInStatement,
    CatchClause,
    SwitchBody,
    /// Statement that only nests other statements (walked when hoisting `var`).
    Stmt,
    CallExpression,
    NewExpression,
    MemberExpression,
    SubscriptExpression,
    AssignmentExpression,
    AugmentedAssignment,
    UpdateExpression,
    ParenthesizedExpression,
    NonNullExpression,
    AwaitExpression,
    AsExpression,
    SatisfiesExpression,
    TypeAssertion,
    Object,
    Pair,
    SpreadElement,
    Arguments,
    ComputedPropertyName,
    ObjectPattern,
    ArrayPattern,
    PairPattern,
    AssignmentPattern,
    RestPattern,
    TypeAnnotation,
    TypeParameters,
    TypeParameter,
    TypeArguments,
    GenericType,
    UnionType,
    IntersectionType,
    ArrayType,
    WrapType,
    LiteralType,
    ThisType,
    TypePredicate,
    Asserts,
    Decorator,
    JsxOpening,
    JsxClosing,
    JsxAttribute,
    JsxExpression,
}

/// Anonymous tokens the walker looks at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tk {
    Other,
    Async,
    Star,
    Accessor,
    Abstract,
    Declare,
    Readonly,
    Const,
    Var,
    Decl,
    Eq,
    Namespace,
}

struct Tables {
    kinds: KindTable<K>,
    toks: Vec<Tk>,
    f_name: u16,
    f_body: u16,
    f_value: u16,
    f_type: u16,
    f_parameter: u16,
    f_object: u16,
    f_property: u16,
    f_function: u16,
    f_arguments: u16,
    f_constructor: u16,
    f_left: u16,
    f_right: u16,
    f_key: u16,
    f_pattern: u16,
    f_source: u16,
    f_alias: u16,
    f_declaration: u16,
    f_module: u16,
    f_argument: u16,
    f_index: u16,
    f_type_arguments: u16,
}

const KINDS: &[(&str, K)] = &[
    ("ERROR", K::Error),
    ("comment", K::Comment),
    ("html_comment", K::Comment),
    ("identifier", K::Identifier),
    ("type_identifier", K::TypeIdentifier),
    ("property_identifier", K::PropertyIdentifier),
    ("private_property_identifier", K::PropertyIdentifier),
    ("shorthand_property_identifier", K::ShorthandProperty),
    ("shorthand_property_identifier_pattern", K::ShorthandPattern),
    ("nested_type_identifier", K::NestedTypeIdentifier),
    ("nested_identifier", K::NestedIdentifier),
    ("this", K::This),
    ("super", K::Super),
    ("meta_property", K::MetaProperty),
    ("string", K::String),
    ("number", K::Number),
    ("true", K::Literal),
    ("false", K::Literal),
    ("null", K::Literal),
    ("regex", K::Literal),
    ("template_string", K::TemplateString),
    ("template_substitution", K::TemplateSubstitution),
    ("array", K::Array),
    ("unary_expression", K::UnaryExpression),
    ("binary_expression", K::BinaryExpression),
    ("import_statement", K::ImportStatement),
    ("import_clause", K::ImportClause),
    ("namespace_import", K::NamespaceImport),
    ("named_imports", K::NamedImports),
    ("import_specifier", K::ImportSpecifier),
    ("import_require_clause", K::ImportRequireClause),
    ("import_alias", K::ImportAlias),
    ("import", K::Import),
    ("export_statement", K::ExportStatement),
    ("export_clause", K::ExportClause),
    ("export_specifier", K::ExportSpecifier),
    ("namespace_export", K::NamespaceExport),
    ("function_declaration", K::FunctionDeclaration),
    ("generator_function_declaration", K::FunctionDeclaration),
    ("function_signature", K::FunctionDeclaration),
    ("function_expression", K::FunctionExpression),
    ("generator_function", K::FunctionExpression),
    ("arrow_function", K::ArrowFunction),
    ("formal_parameters", K::FormalParameters),
    ("required_parameter", K::RequiredParameter),
    ("optional_parameter", K::OptionalParameter),
    ("accessibility_modifier", K::Modifier),
    ("override_modifier", K::Modifier),
    ("class_declaration", K::ClassDeclaration),
    ("abstract_class_declaration", K::AbstractClass),
    ("class", K::Class),
    ("class_body", K::ClassBody),
    ("class_heritage", K::ClassHeritage),
    ("extends_clause", K::ExtendsClause),
    ("implements_clause", K::ImplementsClause),
    ("class_static_block", K::ClassStaticBlock),
    ("method_definition", K::MethodDefinition),
    ("method_signature", K::MethodSignature),
    ("abstract_method_signature", K::AbstractMethodSignature),
    ("field_definition", K::FieldDefinition),
    ("public_field_definition", K::FieldDefinition),
    ("interface_declaration", K::InterfaceDeclaration),
    ("extends_type_clause", K::ExtendsTypeClause),
    ("object_type", K::ObjectType),
    ("interface_body", K::ObjectType),
    ("property_signature", K::PropertySignature),
    ("function_type", K::FunctionType),
    ("constructor_type", K::SignatureType),
    ("call_signature", K::SignatureType),
    ("construct_signature", K::SignatureType),
    ("index_signature", K::IndexSignature),
    ("mapped_type_clause", K::MappedTypeClause),
    ("infer_type", K::InferType),
    ("type_alias_declaration", K::TypeAliasDeclaration),
    ("enum_declaration", K::EnumDeclaration),
    ("enum_body", K::EnumBody),
    ("enum_assignment", K::EnumAssignment),
    ("internal_module", K::Namespace),
    ("module", K::Namespace),
    ("ambient_declaration", K::AmbientDeclaration),
    ("variable_declaration", K::VariableDeclaration),
    ("lexical_declaration", K::LexicalDeclaration),
    ("using_declaration", K::LexicalDeclaration),
    ("variable_declarator", K::VariableDeclarator),
    ("expression_statement", K::ExpressionStatement),
    ("statement_block", K::StatementBlock),
    ("for_statement", K::ForStatement),
    ("for_in_statement", K::ForInStatement),
    ("catch_clause", K::CatchClause),
    ("switch_body", K::SwitchBody),
    ("if_statement", K::Stmt),
    ("else_clause", K::Stmt),
    ("while_statement", K::Stmt),
    ("do_statement", K::Stmt),
    ("try_statement", K::Stmt),
    ("finally_clause", K::Stmt),
    ("switch_statement", K::Stmt),
    ("switch_case", K::Stmt),
    ("switch_default", K::Stmt),
    ("labeled_statement", K::Stmt),
    ("with_statement", K::Stmt),
    ("call_expression", K::CallExpression),
    ("new_expression", K::NewExpression),
    ("member_expression", K::MemberExpression),
    ("subscript_expression", K::SubscriptExpression),
    ("assignment_expression", K::AssignmentExpression),
    ("augmented_assignment_expression", K::AugmentedAssignment),
    ("update_expression", K::UpdateExpression),
    ("parenthesized_expression", K::ParenthesizedExpression),
    ("non_null_expression", K::NonNullExpression),
    ("await_expression", K::AwaitExpression),
    ("as_expression", K::AsExpression),
    ("satisfies_expression", K::SatisfiesExpression),
    ("type_assertion", K::TypeAssertion),
    ("object", K::Object),
    ("pair", K::Pair),
    ("spread_element", K::SpreadElement),
    ("arguments", K::Arguments),
    ("computed_property_name", K::ComputedPropertyName),
    ("object_pattern", K::ObjectPattern),
    ("array_pattern", K::ArrayPattern),
    ("pair_pattern", K::PairPattern),
    ("assignment_pattern", K::AssignmentPattern),
    ("object_assignment_pattern", K::AssignmentPattern),
    ("rest_pattern", K::RestPattern),
    ("type_annotation", K::TypeAnnotation),
    ("type_parameters", K::TypeParameters),
    ("type_parameter", K::TypeParameter),
    ("type_arguments", K::TypeArguments),
    ("generic_type", K::GenericType),
    ("union_type", K::UnionType),
    ("intersection_type", K::IntersectionType),
    ("array_type", K::ArrayType),
    ("tuple_type", K::ArrayType),
    ("parenthesized_type", K::WrapType),
    ("readonly_type", K::WrapType),
    ("literal_type", K::LiteralType),
    ("this_type", K::ThisType),
    ("type_predicate", K::TypePredicate),
    ("asserts", K::Asserts),
    ("decorator", K::Decorator),
    ("jsx_opening_element", K::JsxOpening),
    ("jsx_self_closing_element", K::JsxOpening),
    ("jsx_closing_element", K::JsxClosing),
    ("jsx_attribute", K::JsxAttribute),
    ("jsx_expression", K::JsxExpression),
    // leaves and subtrees that never contain references
    ("string_fragment", K::Skip),
    ("escape_sequence", K::Skip),
    ("regex_pattern", K::Skip),
    ("regex_flags", K::Skip),
    ("hash_bang_line", K::Skip),
    ("undefined", K::Skip),
    ("empty_statement", K::Skip),
    ("debugger_statement", K::Skip),
    ("break_statement", K::Skip),
    ("continue_statement", K::Skip),
    ("statement_identifier", K::Skip),
    ("predefined_type", K::Skip),
    ("optional_chain", K::Skip),
    ("existential_type", K::Skip),
    ("jsx_text", K::Skip),
    ("html_character_reference", K::Skip),
    ("jsx_namespace_name", K::Skip),
];

fn build_tables(lang: Language) -> Tables {
    // Every named kind is listed (explicitly or as `Other`), so that the table's default is left
    // for anonymous tokens: they are half of all nodes and must not cost a descent each.
    let mut pairs: Vec<(&str, K)> = KINDS.to_vec();
    let n = lang.node_kind_count();
    let mut toks = vec![Tk::Other; n];
    for (id, tok) in toks.iter_mut().enumerate() {
        let Some(name) = lang.node_kind_for_id(id as u16) else { continue };
        if lang.node_kind_is_named(id as u16) {
            if !KINDS.iter().any(|(k, _)| *k == name) {
                pairs.push((name, K::Other));
            }
        } else {
            *tok = match name {
                "async" => Tk::Async,
                "*" => Tk::Star,
                "get" | "set" | "static get" => Tk::Accessor,
                "abstract" => Tk::Abstract,
                "declare" => Tk::Declare,
                "readonly" => Tk::Readonly,
                "const" => Tk::Const,
                "var" => Tk::Var,
                "let" | "using" => Tk::Decl,
                "=" => Tk::Eq,
                "namespace" => Tk::Namespace,
                _ => Tk::Other,
            };
        }
    }
    let kinds = KindTable::build(&lang, K::Anon, &pairs);
    Tables {
        kinds,
        toks,
        f_name: field(&lang, "name"),
        f_body: field(&lang, "body"),
        f_value: field(&lang, "value"),
        f_type: field(&lang, "type"),
        f_parameter: field(&lang, "parameter"),
        f_object: field(&lang, "object"),
        f_property: field(&lang, "property"),
        f_function: field(&lang, "function"),
        f_arguments: field(&lang, "arguments"),
        f_constructor: field(&lang, "constructor"),
        f_left: field(&lang, "left"),
        f_right: field(&lang, "right"),
        f_key: field(&lang, "key"),
        f_pattern: field(&lang, "pattern"),
        f_source: field(&lang, "source"),
        f_alias: field(&lang, "alias"),
        f_declaration: field(&lang, "declaration"),
        f_module: field(&lang, "module"),
        f_argument: field(&lang, "argument"),
        f_index: field(&lang, "index"),
        f_type_arguments: field(&lang, "type_arguments"),
    }
}

fn tables(lang: Lang) -> &'static Tables {
    static JS: OnceLock<Tables> = OnceLock::new();
    static TS: OnceLock<Tables> = OnceLock::new();
    static TSX: OnceLock<Tables> = OnceLock::new();
    match lang {
        Lang::TypeScript => TS.get_or_init(|| build_tables(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())),
        Lang::Tsx => TSX.get_or_init(|| build_tables(tree_sitter_typescript::LANGUAGE_TSX.into())),
        _ => JS.get_or_init(|| build_tables(tree_sitter_javascript::LANGUAGE.into())),
    }
}

/// `this_mode` of a function that keeps the `this` of its surroundings (arrows, methods).
const INHERIT: u32 = u32::MAX - 1;

/// No decorators precede a member: (first line, index of their first reference).
const NO_DECO: (u32, u32) = (0, u32::MAX);

/// What a module-level name of this file is bound to.
#[derive(Clone, Copy)]
enum Bind {
    /// `import * as ns from 'm'`, `import ns = require('m')`, `const ns = require('m')`: target.
    Namespace(u32),
    /// `import d from 'm'`: target. The alias doubles as the conventional name of the export.
    Default(u32),
    /// `import { a as b } from 'm'`: the exported name `a` and the target `m`. What is written
    /// `b` is `a` of that module: references carry the target as their qualifier.
    Alias(u32, u32),
    /// Module-level variable. The payload is a receiver path the extractor knows and the
    /// variable's definition does not carry (`const x = make()` gives `make()`); `NONE` when the
    /// definition says it all, and the variable is the root `$name`.
    Var(u32),
    /// Namespace declared in this file (lower-case names would otherwise read as variables).
    Type,
    /// Imported or declared under this very name: not the builtin of the same name.
    Sym,
}

/// What the object of a member expression denotes.
#[derive(Clone, Copy)]
enum Recv {
    /// Nothing usable: member references carry no qualifier.
    Unknown,
    /// Builtin global or literal: its members are never symbols of the repository.
    Stop,
    /// Module alias: `ns.f()` is a call qualified by the import target (interned path).
    Module(u32),
    /// A type or namespace by name (`T`, or `TARGET|T` when written through a module alias),
    /// or a capitalised name that may be one: `T.f()` is a static call qualified by `T`. As the
    /// receiver of anything else the name is used as a value, so it roots a path as `$T`: the
    /// resolver looks members up on the class or namespace itself, or on the type of a
    /// variable should the name turn out to be one.
    Type(u32),
    /// A namespace this file declares: a container used as a value, so that even `ns.f()` is a
    /// member call on the root `$ns`.
    Ns(u32),
    /// A value whose type path is known (interned).
    Path(u32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ObjMode {
    Plain,
    /// `module.exports = { ... }`: members are exported functions.
    Exports,
}

/// Context handed to the next object literal that is visited.
struct ObjCtx {
    id: usize,
    /// Declared type of the literal: every key is a write to a member of it.
    ty: u32,
    /// The module-level variable the literal is bound to, as the path root `$name`: what
    /// `this` denotes in its methods and the owner of its slots.
    owner: u32,
    mode: ObjMode,
}

/// A reference waiting for its argument count: (name, kind, qualifier, line).
type Target = (u32, RefKind, u32, u32);

/// Run `$body` for every child of `$n`, the cursor's node, with the cursor on the child. The
/// loop is counted: stepping past the last child would cost as much as a real step. `$body`
/// must not `continue`.
macro_rules! each_child {
    ($self:ident, $n:expr, $c:ident, |$ch:ident, $k:ident| $body:block) => {{
        let mut left = $n.child_count();
        if left > 0 && $c.goto_first_child() {
            loop {
                let $ch = $c.node();
                let $k = $self.k($ch);
                $body
                left -= 1;
                if left == 0 || !$c.goto_next_sibling() {
                    break;
                }
            }
            $c.goto_parent();
        }
    }};
}

/// Like `each_child`, for a node enclosed in brackets (`( .. )`, `{ .. }`, `[ .. ]`, `< .. >`):
/// neither bracket is handed to `$body` and the closing one is not even stepped onto.
macro_rules! each_inner {
    ($self:ident, $n:expr, $c:ident, |$ch:ident, $k:ident| $body:block) => {{
        let total = $n.child_count();
        if total > 2 && $c.goto_first_child() {
            let mut left = total - 2;
            while $c.goto_next_sibling() {
                let $ch = $c.node();
                let $k = $self.k($ch);
                $body
                left -= 1;
                if left == 0 {
                    break;
                }
            }
            $c.goto_parent();
        }
    }};
}

struct W<'a, 't> {
    cx: Ctx<'a>,
    t: &'static Tables,
    /// TypeScript or TSX (JavaScript otherwise).
    ts: bool,
    /// Enclosing function bodies and plain blocks: declarations are local when > 0.
    local_depth: u32,
    fn_depth: u32,
    ns_depth: u32,
    ambient: u32,
    /// Name of what `this` denotes (class, object-literal owner), or `NONE`.
    this_ty: u32,
    /// Qualifier of `super`: `^Class`, computed on first use for the current `this_ty`; the base
    /// class itself inside an anonymous class.
    super_q: u32,
    /// Set for the parameter list of a constructor: parent of its parameter properties.
    ctor: Option<u32>,
    /// JavaScript declares fields by assigning to `this` in the constructor:
    /// (owning definition, function depth of the constructor body).
    ctor_fields: Option<(u32, u32)>,
    /// Field names already defined for the owner of `ctor_fields`.
    cls_fields: Vec<u32>,
    obj_ctx: Option<ObjCtx>,
    binds: FxHashMap<u32, Bind>,
    /// `binds` has entries that rename or silence plain identifiers.
    has_alias: bool,
    /// Qualifier of the name `free_word` returned last: the import target behind an aliased
    /// named import, else `NONE`. Only maintained while `has_alias`.
    free_qual: u32,
    /// Names exported by name: `export { a }`, `export default a`, `module.exports = { a }`.
    exports: Vec<u32>,
    /// Module-level definitions, for `LOCAL` / `EXPORTED` in `finish`.
    toplevel: Vec<u32>,
    /// Variables and fields initialised with a named value: (definition, value, owner type of
    /// the value or `NONE`). Those that turn out to hold a function of this file get `FNPTR`.
    slot_values: Vec<(u32, u32, u32)>,
    is_module: bool,
    has_var: bool,
    marks: bool,
    uses_arguments: bool,
    array_id: u32,
    _t: PhantomData<&'t ()>,
}

pub fn extract(lang: Lang, src: &[u8], tree: &Tree, path: &str, facts: &mut FileFacts, scratch: &mut Scratch) {
    let mut cx = Ctx::new(src, path, facts, scratch);
    cx.is_header = Lang::is_header(path);
    let mut w = W {
        cx,
        t: tables(lang),
        ts: lang != Lang::JavaScript,
        local_depth: 0,
        fn_depth: 0,
        ns_depth: 0,
        ambient: 0,
        this_ty: NONE,
        super_q: NONE,
        ctor: None,
        ctor_fields: None,
        cls_fields: Vec::new(),
        obj_ctx: None,
        binds: FxHashMap::default(),
        has_alias: false,
        free_qual: NONE,
        exports: Vec::new(),
        toplevel: Vec::new(),
        slot_values: Vec::new(),
        is_module: false,
        has_var: memchr::memmem::find(src, b"var ").is_some(),
        marks: false,
        uses_arguments: false,
        array_id: NONE,
        _t: PhantomData,
    };
    let mut c = tree.walk();
    w.visit(&mut c);
    w.finish();
}

impl<'a, 't> W<'a, 't> {
    #[inline]
    fn k(&self, n: Node<'t>) -> K {
        self.t.kinds.get(n.kind_id())
    }

    #[inline]
    fn tk(&self, n: Node<'t>) -> Tk {
        self.t.toks.get(n.kind_id() as usize).copied().unwrap_or(Tk::Other)
    }

    #[inline]
    fn visit(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        let k = self.k(n);
        self.node(n, k, c);
    }

    fn children(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        each_child!(self, n, c, |ch, k| {
            self.node(ch, k, c);
        });
    }

    /// Children of a bracketed node, without its brackets.
    fn inner(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        each_inner!(self, n, c, |ch, k| {
            self.node(ch, k, c);
        });
    }

    /// Visit `n`, the cursor's node, of kind `k`. All recursion passes through here, which is
    /// where the depth guard sits.
    fn node(&mut self, n: Node<'t>, k: K, c: &mut TreeCursor<'t>) {
        match k {
            K::Anon
            | K::Skip
            | K::Comment
            | K::PropertyIdentifier
            | K::ShorthandPattern
            | K::NestedIdentifier
            | K::This
            | K::Super
            | K::MetaProperty
            | K::String
            | K::Number
            | K::Literal
            | K::Modifier
            | K::LiteralType
            | K::ThisType
            | K::Import
            | K::JsxClosing => return,
            K::Identifier => return self.identifier(n, RefKind::Read),
            K::TypeIdentifier => return self.type_word(self.cx.text(n), line_of(n), RefKind::Type),
            K::NestedTypeIdentifier => return self.nested_type(n, RefKind::Type),
            K::ShorthandProperty => return self.shorthand(n, NONE, NONE, ObjMode::Plain),
            _ => {}
        }
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        match k {
            K::MemberExpression => {
                if let Some((name, kind, qual, line)) = self.member_ref(n, c, false, false) {
                    self.cx.add_ref(name, kind, line, qual, 255);
                }
            }
            K::CallExpression => self.call(n, c),
            K::ExpressionStatement => {
                // `expr;`: only the expression matters.
                if c.goto_first_child() {
                    self.visit(c);
                    c.goto_parent();
                }
            }
            K::TypeAnnotation => self.type_annotation(n, c),
            K::StatementBlock | K::SwitchBody | K::ForStatement => self.block(n, k, c),
            K::ParenthesizedExpression | K::TypeArguments | K::TemplateSubstitution | K::FormalParameters | K::ObjectType | K::Arguments => self.inner(n, c),
            K::Array => {
                if !is_data(self.cx.text(n)) {
                    self.inner(n, c);
                }
            }
            K::TemplateString => {
                // Only substitutions hold code.
                if memchr::memmem::find(self.cx.text(n), b"${").is_some() {
                    self.children(n, c);
                }
            }
            K::NewExpression => self.new_expr(n, c),
            K::AssignmentExpression => self.assignment(n, c),
            K::AugmentedAssignment | K::UpdateExpression => self.write_target(n, c),
            K::ArrowFunction => {
                self.function(n, c, NONE, INHERIT, true);
            }
            K::FunctionExpression => {
                self.function(n, c, NONE, NONE, true);
            }
            K::Object => self.object(n, c),
            K::AsExpression | K::SatisfiesExpression => self.cast(n, c),
            K::LexicalDeclaration | K::VariableDeclaration => self.var_decl(n, k, c, n, 0),
            K::FunctionDeclaration => self.function_decl(n, c, n, 0),
            K::ClassDeclaration | K::AbstractClass | K::Class => self.class(n, k, c, n, 0, NONE),
            K::InterfaceDeclaration => self.interface(n, c, n, 0),
            K::TypeAliasDeclaration => self.type_alias(n, c, n, 0),
            K::EnumDeclaration => self.enum_decl(n, c, n, 0),
            K::Namespace => self.namespace(n, c, n, 0),
            K::AmbientDeclaration => self.ambient_decl(n, c, n, 0),
            K::ImportStatement => self.import_statement(n, c),
            K::ImportAlias => self.import_alias(n, n, 0),
            K::ExportStatement => self.export_statement(n, c),
            K::ForInStatement => self.for_in(n, c),
            K::CatchClause => self.catch_clause(n, c),
            K::Decorator => self.decorator(n, c),
            K::JsxOpening => self.jsx_open(n, c),
            K::JsxAttribute => self.jsx_attribute(n, c),
            K::RequiredParameter | K::OptionalParameter => self.loose_parameter(n, c),
            K::TypeParameters => self.type_parameters(n, c),
            K::FunctionType | K::SignatureType => {
                // Their own type parameters live in their own scope.
                self.cx.push_scope();
                self.children(n, c);
                self.cx.pop_scope();
            }
            K::IndexSignature => self.index_signature(n, c),
            K::InferType => self.infer_type(n, c),
            K::TypePredicate => self.type_predicate(n, c),
            K::Asserts => {
                // `asserts x`: `x` names a parameter.
                each_child!(self, n, c, |ch, ck| {
                    if ck != K::Identifier {
                        self.node(ch, ck, c);
                    }
                });
            }
            K::MethodDefinition | K::AbstractMethodSignature => {
                self.method(n, c, 0, NO_DECO);
            }
            K::MethodSignature => {
                // A method of an anonymous object type (`x: { run(): void }`): there is no named
                // container it could be a member of. Interfaces, aliases and classes define
                // theirs in `type_members` and `class_body`.
                self.cx.push_scope();
                self.children(n, c);
                self.cx.pop_scope();
            }
            K::ClassStaticBlock => {
                self.cx.push_scope();
                self.local_depth += 1;
                self.fn_depth += 1;
                self.children(n, c);
                self.fn_depth -= 1;
                self.local_depth -= 1;
                self.cx.pop_scope();
            }
            K::Error => {
                // Tree-sitter could not parse this stretch. Loose identifiers under an ERROR
                // node are noise, whatever still has structure is walked.
                self.cx.facts.parse_errors += 1;
                each_child!(self, n, c, |ch, ck| {
                    if !matches!(ck, K::Identifier | K::TypeIdentifier) {
                        self.node(ch, ck, c);
                    }
                });
            }
            _ => self.children(n, c),
        }
        self.cx.depth -= 1;
    }

    // ----- names -----

    /// The symbol a non-local identifier denotes: aliased imports map back to the exported
    /// name, namespace aliases denote no symbol at all.
    #[inline]
    fn canon(&self, name: u32) -> Option<u32> {
        if !self.has_alias {
            return Some(name);
        }
        match self.binds.get(&name) {
            Some(Bind::Alias(orig, _)) => Some(*orig),
            Some(Bind::Namespace(_)) => None,
            _ => Some(name),
        }
    }

    /// The import target behind the name written `name`, when it is the alias of a named
    /// import (`NONE` otherwise): the qualifier of what the name denotes.
    #[inline]
    fn alias_target(&self, name: u32) -> u32 {
        if !self.has_alias {
            return NONE;
        }
        match self.binds.get(&name) {
            Some(Bind::Alias(_, target)) => *target,
            _ => NONE,
        }
    }

    /// `TARGET|orig` for a type written through the alias of a named import, else the name.
    fn canon_type(&mut self, name: u32) -> u32 {
        if !self.has_alias {
            return name;
        }
        match self.binds.get(&name).copied() {
            Some(Bind::Alias(orig, target)) => {
                let mut buf = std::mem::take(&mut self.cx.s.buf);
                buf.clear();
                buf.extend_from_slice(self.cx.facts.names.get(target));
                buf.push(b'|');
                buf.extend_from_slice(self.cx.facts.names.get(orig));
                let id = self.cx.intern(&buf);
                self.cx.s.buf = buf;
                id
            }
            Some(Bind::Namespace(_)) => NONE,
            _ => name,
        }
    }

    /// A builtin's name that this file imports or declares itself is no builtin here.
    fn shadowed(&mut self, word: &[u8]) -> bool {
        if self.binds.is_empty() {
            return false;
        }
        let id = self.cx.intern(word);
        self.binds.contains_key(&id)
    }

    /// A word used as a value: the symbol it denotes, unless it is a local or a builtin. `pos`
    /// is its byte offset.
    fn free_word(&mut self, text: &[u8], pos: usize) -> Option<u32> {
        if text.is_empty() {
            return None;
        }
        if is_stop(text) && !self.shadowed(text) {
            if text == b"arguments" {
                self.uses_arguments = true;
            }
            return None;
        }
        let name = self.cx.intern(text);
        if self.cx.is_local(name) || (self.marks && self.cx.s.handled.contains(&(pos as u32))) {
            return None;
        }
        if self.has_alias {
            self.free_qual = self.alias_target(name);
        }
        self.canon(name)
    }

    #[inline]
    fn free_name(&mut self, n: Node<'t>) -> Option<u32> {
        self.free_word(self.cx.text(n), n.start_byte())
    }

    fn identifier(&mut self, n: Node<'t>, kind: RefKind) {
        if let Some(name) = self.free_name(n) {
            self.cx.add_ref(name, kind, line_of(n), self.free_qual, 255);
        }
    }

    /// A type named by a bare word.
    fn type_word(&mut self, word: &[u8], line: u32, kind: RefKind) {
        if word.is_empty() || (is_stop(word) && !self.shadowed(word)) {
            return;
        }
        let name = self.cx.intern(word);
        if self.cx.is_local(name) {
            return;
        }
        let qual = self.alias_target(name);
        if let Some(name) = self.canon(name) {
            self.cx.add_ref(name, kind, line, qual, 255);
        }
    }

    /// `: Type`. Most annotations are a single word: no need to walk into them.
    fn type_annotation(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        match simple_annotation(self.cx.text(n)) {
            Some(word) => self.type_word(word, line_of(n), RefKind::Type),
            None => self.children(n, c),
        }
    }

    /// Qualifier of a dotted name (`ns.Inner` of `ns.Inner.Type`), as written except that a
    /// module alias is replaced by its import target.
    fn qualifier(&mut self, m: Node<'t>, depth: u32) -> u32 {
        let t = self.t;
        match self.k(m) {
            K::Identifier => {
                let q = self.cx.name(m);
                match self.binds.get(&q) {
                    Some(Bind::Namespace(target)) | Some(Bind::Default(target)) => *target,
                    // A namespace or class imported under an alias: an attribute of its module.
                    Some(Bind::Alias(orig, target)) => {
                        let (orig, target) = (*orig, *target);
                        let mut buf = std::mem::take(&mut self.cx.s.buf);
                        buf.clear();
                        buf.extend_from_slice(self.cx.facts.names.get(target));
                        buf.push(b'.');
                        buf.extend_from_slice(self.cx.facts.names.get(orig));
                        let id = self.cx.intern(&buf);
                        self.cx.s.buf = buf;
                        id
                    }
                    _ => q,
                }
            }
            K::NestedIdentifier | K::MemberExpression if depth < 8 => match (child(m, t.f_object), child(m, t.f_property)) {
                (Some(o), Some(p)) => {
                    let root = self.qualifier(o, depth + 1);
                    self.join(root, b".", self.cx.text(p))
                }
                _ => self.cx.name(m),
            },
            _ => self.cx.name(m),
        }
    }

    /// `ns.Type`: the simple name, qualified by the import target or the qualifier as written.
    fn nested_type(&mut self, n: Node<'t>, kind: RefKind) {
        let t = self.t;
        let (Some(nm), Some(m)) = (child(n, t.f_name), child(n, t.f_module)) else { return };
        if nm.start_byte() == nm.end_byte() {
            return;
        }
        let qual = self.qualifier(m, 0);
        let name = self.cx.name(nm);
        self.cx.add_ref(name, kind, line_of(nm), qual, 255);
        // `Kind.Member`, `Outer.Inner`: unless the root is a module alias (which names no symbol),
        // it is a use of that enum, namespace or class.
        let mut root = m;
        for _ in 0..8 {
            match child(root, t.f_object) {
                Some(o) if matches!(self.k(root), K::NestedIdentifier | K::MemberExpression) => root = o,
                _ => break,
            }
        }
        if self.k(root) == K::Identifier && root.start_byte() < root.end_byte() {
            let id = self.cx.name(root);
            if !matches!(self.binds.get(&id), Some(Bind::Namespace(_)) | Some(Bind::Default(_))) {
                self.type_word(self.cx.text(root), line_of(root), RefKind::Type);
            }
        }
    }

    /// Name of a class member, object key or enum member; `NONE` when it is computed.
    fn member_name(&mut self, n: Node<'t>) -> u32 {
        match self.k(n) {
            K::PropertyIdentifier | K::Number | K::Identifier | K::ShorthandProperty | K::ShorthandPattern => {
                if n.start_byte() == n.end_byte() {
                    NONE
                } else {
                    self.cx.name(n)
                }
            }
            K::String => {
                let s = self.string_content(n);
                if is_member_string(s) { self.cx.intern(s) } else { NONE }
            }
            K::ComputedPropertyName => match self.first_named(n) {
                Some(x) if self.k(x) == K::String => self.member_name(x),
                _ => NONE,
            },
            _ => NONE,
        }
    }

    fn string_content(&self, s: Node<'t>) -> &'a [u8] {
        let t = self.cx.text(s);
        if t.len() >= 2 { &t[1..t.len() - 1] } else { b"" }
    }

    /// First named child that is not a comment.
    fn first_named(&self, n: Node<'t>) -> Option<Node<'t>> {
        let mut i = 0;
        while let Some(x) = n.named_child(i) {
            if self.k(x) != K::Comment {
                return Some(x);
            }
            i += 1;
        }
        None
    }

    fn join(&mut self, a: u32, sep: &[u8], b: &[u8]) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        if a != NONE {
            buf.extend_from_slice(self.cx.name_bytes(a));
        }
        buf.extend_from_slice(sep);
        buf.extend_from_slice(b);
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    fn prefixed(&mut self, prefix: &[u8], id: u32) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.extend_from_slice(prefix);
        buf.extend_from_slice(self.cx.name_bytes(id));
        let out = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        out
    }

    /// Slot `Q.field` of a bound value (`.field` when the owner is unknown).
    fn slot(&mut self, qual: u32, field: u32) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        if qual != NONE {
            buf.extend_from_slice(self.cx.name_bytes(qual));
        }
        buf.push(b'.');
        buf.extend_from_slice(self.cx.name_bytes(field));
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    fn super_qual(&mut self) -> u32 {
        // An object literal (`this` is `$name`) has no ancestors to speak of.
        if self.super_q == NONE && self.this_type() != NONE {
            self.super_q = self.prefixed(b"^", self.this_ty);
        }
        self.super_q
    }

    /// The type `this` has, when it is one: inside the methods of a module-level object literal
    /// `this` is the variable (`$name`), which is a path root and no type name.
    fn this_type(&self) -> u32 {
        if self.this_ty != NONE && self.cx.name_bytes(self.this_ty).first() == Some(&b'$') { NONE } else { self.this_ty }
    }

    /// `TARGET|name`: a name written with a module qualifier.
    fn module_qualified(&mut self, target: u32, name: &[u8]) -> u32 {
        self.join(target, b"|", name)
    }

    /// The dotted qualifier of a non-member reference (`New`, `Extends`, `Annotation`) for a
    /// receiver path that starts at a module or at a name used as a value: `TARGET|$a::b` is
    /// written `TARGET.a.b` and `$A::b` is written `A.b`, like the qualifier of a type reference.
    fn dotted(&mut self, qual: u32) -> u32 {
        if qual == NONE {
            return NONE;
        }
        let q = self.cx.facts.names.get(qual);
        let module = memchr::memchr(b'|', q);
        if module.is_none() && q.first() != Some(&b'$') {
            return qual;
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let mut rest = q;
        if let Some(at) = module {
            buf.extend_from_slice(&q[..at]);
            buf.push(b'.');
            rest = &q[at + 1..];
        }
        rest = rest.strip_prefix(b"$").unwrap_or(rest);
        while let Some(i) = memchr::memmem::find(rest, b"::") {
            buf.extend_from_slice(&rest[..i]);
            buf.push(b'.');
            rest = &rest[i + 2..];
        }
        buf.extend_from_slice(rest);
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    // ----- definitions -----

    fn is_ambient(&self) -> bool {
        self.ambient > 0 || self.cx.is_header
    }

    /// Scope-dependent flags of a declaration. Definitions nested in function bodies are only
    /// visible in their file, like non-exported namespace members; module-level ones are settled
    /// in `finish`, once every `export { .. }` has been seen.
    fn decl_flags(&self, flags: u8) -> u8 {
        if flags & dflag::EXPORTED != 0 {
            return flags;
        }
        if self.local_depth > 0 || (self.ns_depth > 0 && !self.is_ambient()) {
            return flags | dflag::LOCAL;
        }
        flags
    }

    /// Book-keeping for a declaration that binds a name of its scope.
    fn register(&mut self, d: u32) {
        if self.local_depth > 0 {
            return;
        }
        if self.ns_depth == 0 {
            self.toplevel.push(d);
        }
        let name = self.cx.facts.defs[d as usize].name;
        if is_stop(self.cx.name_bytes(name)) {
            self.binds.entry(name).or_insert(Bind::Sym);
        }
    }

    /// Start of a declaration's signature: decorators belong to its span, not to its signature.
    fn sig_start(&self, span: Node<'t>) -> usize {
        match span.child(0) {
            Some(first) if self.k(first) == K::Decorator => {
                for i in 1..span.child_count() {
                    if let Some(ch) = span.child(i) {
                        if !matches!(self.k(ch), K::Decorator | K::Comment) {
                            return ch.start_byte();
                        }
                    }
                }
                span.start_byte()
            }
            _ => span.start_byte(),
        }
    }

    fn def(&mut self, name: u32, kind: DefKind, span: Node<'t>, sig_end: usize, flags: u8) -> u32 {
        let sig_off = self.sig_start(span);
        let parent = self.cx.encl();
        self.def_at(name, kind, line_of(span), end_line_of(span), sig_off, sig_end, flags, parent)
    }

    #[allow(clippy::too_many_arguments)]
    fn def_at(&mut self, name: u32, kind: DefKind, line: u32, end_line: u32, sig_off: usize, sig_end: usize, flags: u8, parent: u32) -> u32 {
        let sig_len = sig_end.saturating_sub(sig_off).min(u16::MAX as usize) as u16;
        self.cx.add_def(Def {
            name,
            parent,
            qual: NONE,
            line,
            end_line,
            sig_off: sig_off as u32,
            sig_len,
            kind,
            flags,
            type_name: NONE,
            arity_min: 0,
            arity_max: 0,
        })
    }

    /// References made by decorators that precede definition `d`: they were recorded against
    /// `outer`, the definition enclosing `d`, and belong to `d`.
    fn adopt(&mut self, d: u32, outer: u32, from: u32) {
        if from == u32::MAX {
            return;
        }
        for r in self.cx.facts.refs.iter_mut().skip(from as usize) {
            if r.encl == outer {
                r.encl = d;
            }
        }
    }

    /// Set the type of variable or field `d`, and its arity when it holds a function.
    fn typed(&mut self, d: u32, annotation: Option<Node<'t>>, value: Option<Node<'t>>) -> u32 {
        let ty = self.declared_type(annotation, value);
        let arity = annotation.and_then(|x| self.fn_type(x, 0)).map(|f| self.fn_type_arity(f));
        let dm = self.cx.def_mut(d);
        dm.type_name = ty;
        if let Some((a, b)) = arity {
            dm.flags |= dflag::FNPTR;
            dm.arity_min = a;
            dm.arity_max = b;
        }
        ty
    }

    fn function_decl(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, span: Node<'t>, flags: u8) {
        let t = self.t;
        let Some(name_node) = child(n, t.f_name).filter(|x| x.start_byte() < x.end_byte()) else {
            self.function(n, c, NONE, NONE, false);
            return;
        };
        let name = self.cx.name(name_node);
        let body = child(n, t.f_body);
        let mut fl = self.decl_flags(flags);
        if body.is_none() {
            fl |= dflag::DECL;
        }
        let sig_end = body.map(|b| b.start_byte()).unwrap_or(n.end_byte());
        let d = self.def(name, DefKind::Function, span, sig_end, fl);
        self.register(d);
        // `function Foo() { this.x = ... }`: a constructor function, `this` is a `Foo` and, in
        // JavaScript, its assignments to `this` declare the fields.
        let constructor = self.cx.text(name_node)[0].is_ascii_uppercase();
        let saved = (self.ctor_fields.take(), std::mem::take(&mut self.cls_fields));
        if constructor && !self.ts {
            self.ctor_fields = Some((d, self.fn_depth + 1));
        }
        self.function(n, c, d, if constructor { name } else { NONE }, false);
        (self.ctor_fields, self.cls_fields) = saved;
    }

    /// Walk a function-like node: scope, parameters, body. `def` is its definition (or `NONE`),
    /// `this_mode` what `this` denotes inside (`INHERIT` to keep the current meaning) and
    /// `named_expr` tells that a name child is the function expression's own, visible inside only.
    /// Returns whether the node is a `get` / `set` accessor.
    fn function(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, def: u32, this_mode: u32, named_expr: bool) -> bool {
        let t = self.t;
        if def != NONE {
            self.cx.push_def(def);
        }
        self.cx.push_scope();
        self.local_depth += 1;
        self.fn_depth += 1;
        let arrow = self.k(n) == K::ArrowFunction;
        let saved_this = (self.this_ty, self.super_q);
        if this_mode != INHERIT {
            self.this_ty = this_mode;
            self.super_q = NONE;
        }
        let saved_args = if arrow { None } else { Some(std::mem::replace(&mut self.uses_arguments, false)) };
        let ctor = self.ctor.take();
        let mut mods = 0u8;
        let mut accessor = false;
        let mut arity = (0u8, 0u8);
        each_child!(self, n, c, |ch, k| {
            match k {
                K::Anon => match self.tk(ch) {
                    Tk::Async | Tk::Star => mods |= dflag::ASYNC,
                    Tk::Accessor => accessor = true,
                    _ => {}
                },
                K::Identifier => {
                    if is_field(c, t.f_name) {
                        if named_expr {
                            self.declare_pattern(ch, NONE, 0);
                        }
                    } else if is_field(c, t.f_parameter) {
                        self.declare_pattern(ch, NONE, 0);
                        arity = (1, 1);
                    } else {
                        self.identifier(ch, RefKind::Read);
                    }
                }
                K::TypeParameters => self.type_parameters(ch, c),
                K::FormalParameters => arity = self.params(ch, c, ctor),
                K::TypeAnnotation => {
                    // The return type: type parameters are in scope by now.
                    if def != NONE {
                        let ty = self.type_name(ch, 0);
                        self.cx.def_mut(def).type_name = ty;
                    }
                    self.type_annotation(ch, c);
                }
                K::StatementBlock => {
                    if self.has_var {
                        self.hoist_vars(ch);
                    }
                    self.hoist_lexical(ch, c);
                    self.inner(ch, c);
                }
                _ => self.node(ch, k, c),
            }
        });
        if def != NONE {
            let variadic = !arrow && self.uses_arguments;
            let d = self.cx.def_mut(def);
            d.arity_min = arity.0;
            d.arity_max = if variadic { 255 } else { arity.1 };
            d.flags |= mods;
        }
        if let Some(s) = saved_args {
            self.uses_arguments = s;
        }
        (self.this_ty, self.super_q) = saved_this;
        self.fn_depth -= 1;
        self.local_depth -= 1;
        self.cx.pop_scope();
        if def != NONE {
            self.cx.pop_def();
        }
        accessor
    }

    /// Declare the parameters of `n`, a `formal_parameters` node, and visit their types,
    /// decorators and default values. Returns the arity.
    fn params(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, ctor: Option<u32>) -> (u8, u8) {
        let (mut count, mut min, mut variadic) = (0u32, 0u32, false);
        each_inner!(self, n, c, |ch, k| {
            match k {
                K::RequiredParameter | K::OptionalParameter => {
                    let (counted, required, rest) = self.param(ch, k, c, ctor);
                    if counted {
                        count += 1;
                        if rest {
                            variadic = true;
                        } else if required {
                            min = count;
                        }
                    }
                }
                // JavaScript: bare patterns
                K::Identifier => {
                    count += 1;
                    min = count;
                    self.declare_pattern(ch, NONE, 0);
                }
                K::AssignmentPattern => {
                    count += 1;
                    self.declare_pattern(ch, NONE, 0);
                    self.node(ch, k, c);
                }
                K::RestPattern => {
                    variadic = true;
                    self.declare_pattern(ch, NONE, 0);
                    self.node(ch, k, c);
                }
                K::ObjectPattern | K::ArrayPattern => {
                    count += 1;
                    min = count;
                    self.declare_pattern(ch, NONE, 0);
                    self.node(ch, k, c);
                }
                _ => self.node(ch, k, c),
            }
        });
        (min.min(254) as u8, if variadic { 255 } else { count.min(254) as u8 })
    }

    /// One TypeScript parameter: `@deco private readonly name?: Type = value`. Returns whether
    /// it counts as a parameter (`this: T` does not), is required, and is a rest parameter.
    fn param(&mut self, p: Node<'t>, pk: K, c: &mut TreeCursor<'t>, ctor: Option<u32>) -> (bool, bool, bool) {
        let mut pattern: Option<Node<'t>> = None;
        let mut annotation = None;
        let mut ty = NONE;
        let (mut property, mut has_default, mut receiver) = (false, false, false);
        each_child!(self, p, c, |ch, k| {
            match k {
                K::Anon => match self.tk(ch) {
                    Tk::Readonly => property = true,
                    Tk::Eq => has_default = true,
                    _ => {}
                },
                K::Modifier => property = true,
                K::Identifier if pattern.is_none() && !has_default => pattern = Some(ch),
                K::This if pattern.is_none() && !has_default => receiver = true,
                K::RestPattern | K::ObjectPattern | K::ArrayPattern if pattern.is_none() && !has_default => {
                    pattern = Some(ch);
                    self.declare_pattern(ch, NONE, 0);
                    self.node(ch, k, c);
                }
                K::TypeAnnotation => {
                    annotation = Some(ch);
                    ty = self.type_name(ch, 0);
                    self.type_annotation(ch, c);
                }
                _ => self.node(ch, k, c),
            }
        });
        if receiver {
            return (false, false, false);
        }
        let mut rest = false;
        if let Some(pat) = pattern {
            match self.k(pat) {
                K::Identifier => {
                    // `constructor(private readonly svc: Svc)`: the modifier makes it a field.
                    if let Some(owner) = ctor.filter(|_| property) {
                        self.param_property(p, pat, owner, ty, annotation);
                    }
                    self.declare_pattern(pat, ty, 0);
                }
                K::ObjectPattern => self.pattern_reads(pat, if ty == NONE { Recv::Unknown } else { Recv::Path(ty) }, false),
                K::RestPattern => {
                    rest = true;
                    if let Some(id) = self.first_named(pat).filter(|x| self.k(*x) == K::Identifier) {
                        self.declare_pattern(id, ty, 0);
                    }
                }
                _ => {}
            }
        }
        (true, pk == K::RequiredParameter && !has_default, rest)
    }

    /// Arity of a parameter list without declaring anything (function types).
    fn arity_of(&self, params: Node<'t>) -> (u8, u8) {
        let t = self.t;
        let (mut n, mut min, mut variadic) = (0u32, 0u32, false);
        let mut cc = params.walk();
        if cc.goto_first_child() {
            loop {
                let ch = cc.node();
                let k = self.k(ch);
                if matches!(k, K::RequiredParameter | K::OptionalParameter) {
                    let pk = child(ch, t.f_pattern).or_else(|| child(ch, t.f_name)).map(|p| self.k(p));
                    if pk != Some(K::This) {
                        n += 1;
                        if pk == Some(K::RestPattern) {
                            variadic = true;
                        } else if k == K::RequiredParameter && child(ch, t.f_value).is_none() {
                            min = n;
                        }
                    }
                }
                if !cc.goto_next_sibling() {
                    break;
                }
            }
        }
        (min.min(254) as u8, if variadic { 255 } else { n.min(254) as u8 })
    }

    fn param_property(&mut self, p: Node<'t>, name_node: Node<'t>, owner: u32, ty: u32, annotation: Option<Node<'t>>) {
        if name_node.start_byte() == name_node.end_byte() {
            return;
        }
        let name = self.cx.name(name_node);
        let sig_off = self.sig_start(p);
        let d = self.def_at(name, DefKind::Field, line_of(p), end_line_of(p), sig_off, p.end_byte(), 0, owner);
        let arity = annotation.and_then(|x| self.fn_type(x, 0)).map(|f| self.fn_type_arity(f));
        let dm = self.cx.def_mut(d);
        dm.type_name = ty;
        if let Some((a, b)) = arity {
            dm.flags |= dflag::FNPTR;
            dm.arity_min = a;
            dm.arity_max = b;
        }
    }

    /// Declare the name a single identifier binds.
    fn declare_name(&mut self, nm: Node<'t>) {
        if nm.start_byte() < nm.end_byte() {
            let id = self.cx.name(nm);
            self.cx.declare(id, NONE);
        }
    }

    /// Declare every name a binding pattern introduces. Only a plain identifier carries `ty`.
    fn declare_pattern(&mut self, p: Node<'t>, ty: u32, depth: u32) {
        let t = self.t;
        if depth > 24 {
            return;
        }
        match self.k(p) {
            K::Identifier | K::ShorthandPattern => {
                if p.start_byte() < p.end_byte() {
                    let nm = self.cx.name(p);
                    self.cx.declare(nm, ty);
                }
            }
            K::AssignmentPattern => {
                if let Some(l) = child(p, t.f_left) {
                    self.declare_pattern(l, NONE, depth + 1);
                }
            }
            K::PairPattern => {
                if let Some(v) = child(p, t.f_value) {
                    self.declare_pattern(v, NONE, depth + 1);
                }
            }
            K::RestPattern | K::ObjectPattern | K::ArrayPattern => {
                let mut cc = p.walk();
                if cc.goto_first_child() {
                    loop {
                        let ch = cc.node();
                        if !matches!(self.k(ch), K::Anon | K::Comment) {
                            self.declare_pattern(ch, NONE, depth + 1);
                        }
                        if !cc.goto_next_sibling() {
                            break;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// `const { a, b: c } = x` reads members `a` and `b` of `x`, when it is known what `x` is.
    /// The names it binds then have the types of those members: `c` is an `X::b`.
    fn pattern_reads(&mut self, pattern: Node<'t>, source: Recv, top: bool) {
        let t = self.t;
        let (kind, qual, base) = match source {
            Recv::Path(q) => (RefKind::MemberRead, q, q),
            Recv::Type(q) | Recv::Ns(q) => {
                let root = self.value_root(q);
                (RefKind::MemberRead, root, root)
            }
            Recv::Module(q) => (RefKind::Read, q, NONE),
            Recv::Unknown | Recv::Stop => return,
        };
        let mut cc = pattern.walk();
        if !cc.goto_first_child() {
            return;
        }
        loop {
            let ch = cc.node();
            // (member, name bound to it)
            let entry = match self.k(ch) {
                K::ShorthandPattern => Some((ch, Some(ch))),
                K::PairPattern => child(ch, t.f_key)
                    .filter(|x| self.k(*x) == K::PropertyIdentifier)
                    .map(|key| (key, child(ch, t.f_value).filter(|v| self.k(*v) == K::Identifier))),
                K::AssignmentPattern => child(ch, t.f_left).filter(|x| self.k(*x) == K::ShorthandPattern).map(|key| (key, Some(key))),
                _ => None,
            };
            if let Some((key, bound)) = entry.filter(|(key, _)| key.start_byte() < key.end_byte()) {
                let name = self.cx.name(key);
                self.cx.add_ref(name, kind, line_of(key), qual, 255);
                if let Some(local) = bound.filter(|x| kind == RefKind::MemberRead && x.start_byte() < x.end_byte()) {
                    let ty = self.segment(base, self.cx.text(key));
                    let local = self.cx.name(local);
                    if top {
                        self.binds.insert(local, Bind::Var(ty));
                    } else {
                        self.cx.declare(local, ty);
                    }
                }
            }
            if !cc.goto_next_sibling() {
                break;
            }
        }
    }

    /// `var` is function-scoped: declare the ones nested in the statements of a function body
    /// before walking it, so that uses outside their block are still locals.
    fn hoist_vars(&mut self, body: Node<'t>) {
        let mut cc = body.walk();
        if !cc.goto_first_child() {
            return;
        }
        let mut depth = 1u32;
        loop {
            let ch = cc.node();
            let descend = match self.k(ch) {
                K::VariableDeclaration => {
                    self.hoist_decl(ch);
                    false
                }
                K::ForInStatement => {
                    self.hoist_for_in(ch);
                    true
                }
                K::StatementBlock | K::Stmt | K::ForStatement | K::CatchClause | K::SwitchBody => true,
                _ => false,
            };
            if descend && depth < 64 && cc.goto_first_child() {
                depth += 1;
                continue;
            }
            loop {
                if cc.goto_next_sibling() {
                    break;
                }
                if depth <= 1 {
                    return;
                }
                cc.goto_parent();
                depth -= 1;
            }
        }
    }

    /// `let` and `const` are visible in their whole block: a closure may use one that is
    /// declared further down (`on(() => use(item)); const item = ..`). The names declared by
    /// the statements of `block`, the cursor's node, become locals before the block is walked;
    /// their types follow when the declarations are reached.
    fn hoist_lexical(&mut self, block: Node<'t>, c: &mut TreeCursor<'t>) {
        let mut left = block.child_count();
        // A single statement cannot use what it declares itself before declaring it, and without
        // a function nested in the block nothing can run before the declarations it follows.
        if left < 4 {
            return;
        }
        let text = self.cx.text(block);
        if memchr::memmem::find(text, b"=>").is_none() && memchr::memmem::find(text, b"function").is_none() {
            return;
        }
        if !c.goto_first_child() {
            return;
        }
        loop {
            let ch = c.node();
            if self.k(ch) == K::LexicalDeclaration {
                // `const a = .., b = ..`: the children are the keyword, declarators and commas.
                let mut i = 1;
                while let Some(d) = ch.child(i) {
                    i += 1;
                    if self.k(d) != K::VariableDeclarator {
                        continue;
                    }
                    // A declarator is `name [: type] [= value]`.
                    let Some(name) = d.child(0) else { continue };
                    // Functions and classes are definitions, not locals.
                    let last = d.child(d.child_count().saturating_sub(1) as u32).filter(|v| v.id() != name.id());
                    if last.is_some_and(|v| self.k(v) == K::Class || self.func_value(v).is_some()) {
                        continue;
                    }
                    self.declare_pattern(name, NONE, 0);
                }
            }
            left -= 1;
            if left == 0 || !c.goto_next_sibling() {
                break;
            }
        }
        c.goto_parent();
    }

    fn hoist_decl(&mut self, decl: Node<'t>) {
        let t = self.t;
        let mut i = 0;
        while let Some(d) = decl.named_child(i) {
            i += 1;
            if self.k(d) != K::VariableDeclarator {
                continue;
            }
            let Some(name) = child(d, t.f_name) else { continue };
            let value = child(d, t.f_value);
            // Functions and classes are definitions, not locals.
            if value.is_some_and(|v| matches!(self.k(v), K::ArrowFunction | K::FunctionExpression | K::Class)) {
                continue;
            }
            // Strict types only: the locals an initialiser mentions are not declared yet.
            let ty = self.declared_type(child(d, t.f_type), value);
            self.declare_pattern(name, ty, 0);
        }
    }

    fn hoist_for_in(&mut self, n: Node<'t>) {
        let mut is_var = false;
        for i in 0..n.child_count().min(6) {
            let Some(ch) = n.child(i) else { break };
            if self.k(ch) == K::Anon {
                is_var |= self.tk(ch) == Tk::Var;
            } else if is_var {
                self.declare_pattern(ch, NONE, 0);
                break;
            }
        }
    }

    /// `<T extends A = B, U>`: the names are type variables of the current scope.
    fn type_parameters(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        // A constraint may mention a parameter declared after it (`<E extends Box<T>, T>`): all
        // names are declared before any constraint or default is walked.
        let mut i = 0;
        while let Some(p) = n.named_child(i) {
            i += 1;
            if self.k(p) == K::TypeParameter {
                if let Some(nm) = child(p, t.f_name) {
                    self.declare_name(nm);
                }
            }
        }
        each_inner!(self, n, c, |ch, k| {
            if k == K::TypeParameter {
                // The name is a local by now: the walk only sees constraint and default.
                self.children(ch, c);
            }
        });
    }

    // ----- classes -----

    fn class(&mut self, n: Node<'t>, k: K, c: &mut TreeCursor<'t>, span: Node<'t>, flags: u8, name_override: u32) {
        let t = self.t;
        let name = if name_override != NONE {
            name_override
        } else {
            match child(n, t.f_name).filter(|x| x.start_byte() < x.end_byte()) {
                Some(x) => self.cx.name(x),
                None => NONE,
            }
        };
        let mut def = NONE;
        // Inside an anonymous class `super` is its base class; a named class has ancestors.
        let mut base = NONE;
        if name != NONE {
            let mut fl = self.decl_flags(flags);
            if k == K::AbstractClass {
                fl |= dflag::ABSTRACT;
            }
            if self.is_ambient() {
                fl |= dflag::DECL;
            }
            let sig_end = child(n, t.f_body).map(|b| b.start_byte()).unwrap_or(n.end_byte());
            def = self.def(name, DefKind::Class, span, sig_end, fl);
            self.register(def);
            self.cx.push_def(def);
        } else {
            base = self.base_name(n);
        }
        self.cx.push_scope();
        let saved_this = (self.this_ty, self.super_q);
        self.this_ty = name;
        self.super_q = base;
        let saved_fields = (self.ctor_fields.take(), std::mem::take(&mut self.cls_fields));
        each_child!(self, n, c, |ch, ck| {
            match ck {
                K::TypeParameters => self.type_parameters(ch, c),
                K::ClassHeritage => self.heritage(ch, c),
                K::ClassBody => self.class_body(ch, c),
                // the class name
                K::TypeIdentifier | K::Identifier => {}
                _ => self.node(ch, ck, c),
            }
        });
        (self.ctor_fields, self.cls_fields) = saved_fields;
        (self.this_ty, self.super_q) = saved_this;
        self.cx.pop_scope();
        if def != NONE {
            self.cx.pop_def();
        }
    }

    /// `class extends Base { .. }`: the base class when it is a plain name.
    fn base_name(&mut self, n: Node<'t>) -> u32 {
        let t = self.t;
        for i in 0..n.child_count().min(8) {
            let Some(h) = n.child(i) else { break };
            if self.k(h) != K::ClassHeritage {
                continue;
            }
            let base = match self.first_named(h) {
                Some(x) if self.k(x) == K::ExtendsClause => child(x, t.f_value),
                other => other,
            };
            return match base {
                Some(e) if self.k(e) == K::Identifier => self.free_name(e).unwrap_or(NONE),
                _ => NONE,
            };
        }
        NONE
    }

    /// `extends B implements I, J`.
    fn heritage(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        each_child!(self, n, c, |ch, k| {
            match k {
                K::ExtendsClause => {
                    each_child!(self, ch, c, |e, ek| {
                        if is_field(c, t.f_value) {
                            self.extends_expr(e, ek, c);
                        } else {
                            self.node(e, ek, c);
                        }
                    });
                }
                K::ImplementsClause => self.type_list(ch, c, RefKind::Implements),
                K::Anon | K::Comment => {}
                // JavaScript: the base class expression itself
                _ => self.extends_expr(ch, k, c),
            }
        });
    }

    fn extends_expr(&mut self, e: Node<'t>, k: K, c: &mut TreeCursor<'t>) {
        match k {
            K::Identifier => self.identifier(e, RefKind::Extends),
            K::MemberExpression => {
                if let Some((name, _, qual, line)) = self.member_ref(e, c, true, false) {
                    let qual = self.dotted(qual);
                    self.cx.add_ref(name, RefKind::Extends, line, qual, 255);
                }
            }
            _ => self.node(e, k, c),
        }
    }

    /// A list of named types that are inherited from (`implements A, B<T>`, interface `extends`).
    fn type_list(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: RefKind) {
        let t = self.t;
        each_child!(self, n, c, |ch, k| {
            match k {
                K::TypeIdentifier => self.type_word(self.cx.text(ch), line_of(ch), kind),
                K::NestedTypeIdentifier => self.nested_type(ch, kind),
                K::GenericType => {
                    each_child!(self, ch, c, |g, gk| {
                        match gk {
                            K::TypeIdentifier if is_field(c, t.f_name) => self.type_word(self.cx.text(g), line_of(g), kind),
                            K::NestedTypeIdentifier if is_field(c, t.f_name) => self.nested_type(g, kind),
                            _ => self.node(g, gk, c),
                        }
                    });
                }
                _ => self.node(ch, k, c),
            }
        });
    }

    fn class_body(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        // TypeScript attaches method decorators as siblings: they open the member's span and
        // their references belong to the member.
        let mut deco = NO_DECO;
        each_inner!(self, n, c, |ch, k| {
            match k {
                K::Decorator => {
                    if deco.0 == 0 {
                        deco = (line_of(ch), self.cx.facts.refs.len() as u32);
                    }
                    self.decorator(ch, c);
                }
                K::MethodDefinition | K::MethodSignature => {
                    self.method(ch, c, 0, deco);
                    deco = NO_DECO;
                }
                K::AbstractMethodSignature => {
                    self.method(ch, c, dflag::ABSTRACT, deco);
                    deco = NO_DECO;
                }
                K::FieldDefinition => {
                    self.field(ch, c, deco);
                    deco = NO_DECO;
                }
                K::Anon | K::Comment => {}
                _ => {
                    deco = NO_DECO;
                    self.node(ch, k, c);
                }
            }
        });
    }

    /// A method, accessor or method signature of a class, interface or object literal. Returns
    /// its definition.
    fn method(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, flags: u8, deco: (u32, u32)) -> u32 {
        let t = self.t;
        let name = match child(n, t.f_name) {
            Some(x) => self.member_name(x),
            None => NONE,
        };
        if name == NONE {
            self.function(n, c, NONE, INHERIT, false);
            return NONE;
        }
        let body = child(n, t.f_body);
        let mut fl = flags;
        if body.is_none() {
            fl |= dflag::DECL;
        }
        let sig_end = body.map(|b| b.start_byte()).unwrap_or(n.end_byte());
        let line = if deco.0 != 0 { deco.0 } else { line_of(n) };
        let parent = self.cx.encl();
        let sig_off = self.sig_start(n);
        let d = self.def_at(name, DefKind::Method, line, end_line_of(n), sig_off, sig_end, fl, parent);
        self.adopt(d, parent, deco.1);
        let saved = self.ctor_fields.take();
        if self.cx.name_bytes(name) == b"constructor" {
            self.ctor = Some(parent);
            if !self.ts && parent != NONE {
                self.ctor_fields = Some((parent, self.fn_depth + 1));
            }
        }
        let accessor = self.function(n, c, d, INHERIT, false);
        self.ctor_fields = saved;
        self.ctor = None;
        if accessor {
            self.cx.def_mut(d).kind = DefKind::Property;
        }
        d
    }

    /// A class field. Function-valued fields are methods named after the field.
    fn field(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, deco: (u32, u32)) {
        let t = self.t;
        let name = match child(n, t.f_name).or_else(|| child(n, t.f_property)) {
            Some(x) => self.member_name(x),
            None => NONE,
        };
        if name == NONE {
            return self.children(n, c);
        }
        let value = child(n, t.f_value);
        let annotation = child(n, t.f_type);
        let line = if deco.0 != 0 { deco.0 } else { line_of(n) };
        let parent = self.cx.encl();
        let sig_off = self.sig_start(n);
        let func = value.and_then(|v| self.func_value(v));
        let (kind, sig_end) = match func {
            Some(f) => (DefKind::Method, child(f, t.f_body).map(|b| b.start_byte()).unwrap_or(n.end_byte())),
            None => (DefKind::Field, value.map(|v| v.start_byte()).unwrap_or(n.end_byte())),
        };
        let mut flags = if self.is_ambient() { dflag::DECL } else { 0 };
        let d = self.def_at(name, kind, line, end_line_of(n), sig_off, sig_end, flags, parent);
        self.adopt(d, parent, deco.1);
        self.cls_fields.push(name);
        let mut bound = false;
        if func.is_none() {
            let ty = self.typed(d, annotation, value);
            self.cx.push_def(d);
            if let Some(v) = value {
                // `handler = onEvent;` binds a function into the field's slot.
                if let Some((vn, again)) = self.bindable(v) {
                    let slot = self.slot(self.this_ty, name);
                    self.cx.add_ref(vn, RefKind::Bind, line_of(v), slot, 255);
                    let owner = self.bound_owner(v);
                    self.slot_values.push((d, vn, owner));
                    bound = !again;
                } else if let Some(vn) = self.bound_function(v) {
                    // `handler = this.onEvent.bind(this)`: certainly a function.
                    let slot = self.slot(self.this_ty, name);
                    self.cx.add_ref(vn, RefKind::Bind, line_of(v), slot, 255);
                    self.cx.def_mut(d).flags |= dflag::FNPTR;
                }
                if ty != NONE && self.k(v) == K::Object {
                    self.obj_ctx = Some(ObjCtx { id: v.id(), ty, owner: NONE, mode: ObjMode::Plain });
                }
            }
        }
        each_child!(self, n, c, |ch, k| {
            match k {
                K::Anon => match self.tk(ch) {
                    Tk::Abstract => flags |= dflag::ABSTRACT | dflag::DECL,
                    Tk::Declare => flags |= dflag::DECL,
                    _ => {}
                },
                _ if value.is_some_and(|v| v.id() == ch.id()) => {
                    if let Some(f) = func {
                        self.wrapped_function(ch, f, c, d, self.this_ty);
                    } else if !bound {
                        self.node(ch, k, c);
                    }
                }
                _ => self.node(ch, k, c),
            }
        });
        if func.is_none() {
            self.cx.pop_def();
            self.obj_ctx = None;
        }
        self.cx.def_mut(d).flags |= flags;
    }

    // ----- TypeScript declarations -----

    fn interface(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, span: Node<'t>, flags: u8) {
        let t = self.t;
        let Some(name_node) = child(n, t.f_name).filter(|x| x.start_byte() < x.end_byte()) else { return self.children(n, c) };
        let name = self.cx.name(name_node);
        let sig_end = child(n, t.f_body).map(|b| b.start_byte()).unwrap_or(n.end_byte());
        let fl = self.decl_flags(flags);
        let d = self.def(name, DefKind::Interface, span, sig_end, fl);
        self.register(d);
        self.cx.push_def(d);
        self.cx.push_scope();
        let saved_this = (self.this_ty, self.super_q);
        self.this_ty = name;
        self.super_q = NONE;
        each_child!(self, n, c, |ch, k| {
            match k {
                // the interface name
                K::TypeIdentifier => {}
                K::TypeParameters => self.type_parameters(ch, c),
                K::ExtendsTypeClause => self.type_list(ch, c, RefKind::Extends),
                K::ObjectType => self.type_members(ch, c),
                _ => self.node(ch, k, c),
            }
        });
        (self.this_ty, self.super_q) = saved_this;
        self.cx.pop_scope();
        self.cx.pop_def();
    }

    /// Members of an interface body or of the object type a type alias names: declarations.
    fn type_members(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        each_inner!(self, n, c, |ch, k| {
            match k {
                K::PropertySignature => {
                    let name = match child(ch, t.f_name) {
                        Some(x) => self.member_name(x),
                        None => NONE,
                    };
                    if name == NONE {
                        self.children(ch, c);
                    } else {
                        let d = self.def(name, DefKind::Field, ch, ch.end_byte(), dflag::DECL);
                        self.typed(d, child(ch, t.f_type), None);
                        self.cx.push_def(d);
                        self.children(ch, c);
                        self.cx.pop_def();
                    }
                }
                K::MethodSignature => {
                    self.method(ch, c, dflag::DECL, NO_DECO);
                }
                _ => self.node(ch, k, c),
            }
        });
    }

    fn type_alias(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, span: Node<'t>, flags: u8) {
        let t = self.t;
        let Some(name_node) = child(n, t.f_name).filter(|x| x.start_byte() < x.end_byte()) else { return self.children(n, c) };
        let name = self.cx.name(name_node);
        let value = child(n, t.f_value);
        let sig_end = value.map(|v| v.start_byte()).unwrap_or(n.end_byte());
        let fl = self.decl_flags(flags);
        let d = self.def(name, DefKind::TypeAlias, span, sig_end, fl);
        self.register(d);
        self.cx.push_def(d);
        self.cx.push_scope();
        each_child!(self, n, c, |ch, k| {
            if ch.id() == name_node.id() {
                // the alias itself
            } else if value.is_some_and(|v| v.id() == ch.id()) {
                // Type parameters are in scope by now.
                let ty = self.type_name(ch, 0);
                self.cx.def_mut(d).type_name = ty;
                self.alias_members(ch, k, c, 0);
            } else if k == K::TypeParameters {
                self.type_parameters(ch, c);
            } else {
                self.node(ch, k, c);
            }
        });
        self.cx.pop_scope();
        self.cx.pop_def();
    }

    /// The type an alias names: the members of an object type, also of each object type of a
    /// union or intersection (`A | B | C` nests to the left), are members of the alias.
    fn alias_members(&mut self, n: Node<'t>, k: K, c: &mut TreeCursor<'t>, depth: u32) {
        match k {
            K::ObjectType => self.type_members(n, c),
            K::IntersectionType | K::UnionType | K::WrapType if depth < 64 => {
                each_child!(self, n, c, |m, mk| {
                    self.alias_members(m, mk, c, depth + 1);
                });
            }
            _ => self.node(n, k, c),
        }
    }

    fn enum_decl(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, span: Node<'t>, flags: u8) {
        let t = self.t;
        let Some(name_node) = child(n, t.f_name).filter(|x| x.start_byte() < x.end_byte()) else { return self.children(n, c) };
        let name = self.cx.name(name_node);
        let sig_end = child(n, t.f_body).map(|b| b.start_byte()).unwrap_or(n.end_byte());
        let mut fl = self.decl_flags(flags);
        if self.is_ambient() {
            fl |= dflag::DECL;
        }
        let d = self.def(name, DefKind::Enum, span, sig_end, fl);
        self.register(d);
        self.cx.push_def(d);
        each_child!(self, n, c, |body, bk| {
            if bk == K::EnumBody {
                each_inner!(self, body, c, |m, mk| {
                    match mk {
                        K::EnumAssignment => {
                            let mn = match child(m, t.f_name) {
                                Some(x) => self.member_name(x),
                                None => NONE,
                            };
                            if mn != NONE {
                                self.def(mn, DefKind::EnumMember, m, m.end_byte(), 0);
                            }
                            self.children(m, c);
                        }
                        K::PropertyIdentifier | K::String => {
                            let mn = self.member_name(m);
                            if mn != NONE {
                                self.def(mn, DefKind::EnumMember, m, m.end_byte(), 0);
                            }
                        }
                        _ => self.node(m, mk, c),
                    }
                });
            }
        });
        self.cx.pop_def();
    }

    /// `namespace A.B { .. }`, `module M { .. }`, `declare module 'm' { .. }`.
    fn namespace(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, span: Node<'t>, flags: u8) {
        let t = self.t;
        let Some(name_node) = child(n, t.f_name) else { return self.children(n, c) };
        let external = self.k(name_node) == K::String;
        let (name, qual) = match self.k(name_node) {
            K::String => {
                let s = self.string_content(name_node);
                (if s.is_empty() { NONE } else { self.cx.intern(s) }, NONE)
            }
            K::NestedIdentifier => match (child(name_node, t.f_property), child(name_node, t.f_object)) {
                (Some(p), Some(o)) => (self.cx.name(p), self.cx.name(o)),
                _ => (NONE, NONE),
            },
            _ if name_node.start_byte() < name_node.end_byte() => (self.cx.name(name_node), NONE),
            _ => (NONE, NONE),
        };
        if name == NONE {
            return self.children(n, c);
        }
        let sig_end = child(n, t.f_body).map(|b| b.start_byte()).unwrap_or(n.end_byte());
        let mut fl = self.decl_flags(flags);
        if self.is_ambient() {
            fl |= dflag::DECL;
        }
        let d = self.def(name, DefKind::Namespace, span, sig_end, fl);
        // `declare module 'name'` declares the module of that name, no binding of this file.
        if !external {
            self.register(d);
            if self.local_depth == 0 {
                self.binds.insert(name, Bind::Type);
            }
        }
        if qual != NONE && self.cx.def_mut(d).parent == NONE {
            self.cx.def_mut(d).qual = qual;
        }
        self.cx.push_def(d);
        self.ns_depth += 1;
        each_child!(self, n, c, |ch, k| {
            // The body's statements are declarations of the namespace, not block locals.
            if k == K::StatementBlock {
                self.inner(ch, c);
            }
        });
        self.ns_depth -= 1;
        self.cx.pop_def();
    }

    /// `declare ...`.
    fn ambient_decl(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, span: Node<'t>, flags: u8) {
        self.ambient += 1;
        each_child!(self, n, c, |ch, k| {
            match k {
                // `declare global { .. }`: declarations of the global scope, not of this module
                K::StatementBlock => {
                    self.ns_depth += 1;
                    self.inner(ch, c);
                    self.ns_depth -= 1;
                }
                _ => self.declaration(ch, k, c, span, flags),
            }
        });
        self.ambient -= 1;
    }

    /// Dispatch a declaration that sits inside `span` (`export ...`, `declare ...`).
    fn declaration(&mut self, n: Node<'t>, k: K, c: &mut TreeCursor<'t>, span: Node<'t>, flags: u8) {
        match k {
            K::FunctionDeclaration => self.function_decl(n, c, span, flags),
            K::ClassDeclaration | K::AbstractClass | K::Class => self.class(n, k, c, span, flags, NONE),
            K::InterfaceDeclaration => self.interface(n, c, span, flags),
            K::TypeAliasDeclaration => self.type_alias(n, c, span, flags),
            K::EnumDeclaration => self.enum_decl(n, c, span, flags),
            K::Namespace => self.namespace(n, c, span, flags),
            K::LexicalDeclaration | K::VariableDeclaration => self.var_decl(n, k, c, span, flags),
            K::AmbientDeclaration => self.ambient_decl(n, c, span, flags),
            K::ImportAlias => self.import_alias(n, span, flags),
            _ => self.node(n, k, c),
        }
    }

    // ----- variables -----

    fn var_decl(&mut self, n: Node<'t>, k: K, c: &mut TreeCursor<'t>, span: Node<'t>, flags: u8) {
        // `var` is hoisted out of blocks: at file level it is a module-level binding.
        let top = self.local_depth == 0 || (k == K::VariableDeclaration && self.fn_depth == 0);
        let mut is_const = false;
        let mut declarators = 0;
        let mut i = 0;
        while let Some(x) = n.named_child(i) {
            i += 1;
            if self.k(x) == K::VariableDeclarator {
                declarators += 1;
                if declarators > 1 {
                    break;
                }
            }
        }
        each_child!(self, n, c, |ch, ck| {
            match ck {
                K::Anon => is_const |= self.tk(ch) == Tk::Const,
                K::VariableDeclarator => {
                    let dspan = if declarators == 1 { span } else { ch };
                    self.declarator(ch, c, dspan, flags, is_const, top);
                }
                _ => self.node(ch, ck, c),
            }
        });
    }

    /// Type of a variable or field: its annotation, else what its initialiser shows for certain.
    fn declared_type(&mut self, annotation: Option<Node<'t>>, value: Option<Node<'t>>) -> u32 {
        if let Some(a) = annotation {
            let ty = self.type_name(a, 0);
            if ty != NONE {
                return ty;
            }
        }
        match value {
            Some(v) => self.value_type(v, 0),
            None => NONE,
        }
    }

    /// Type an expression has by construction: `new T()`, `x as T`, `<T>x`, `this`.
    fn value_type(&mut self, v: Node<'t>, depth: u32) -> u32 {
        let t = self.t;
        if depth > 4 {
            return NONE;
        }
        match self.k(v) {
            K::NewExpression => match child(v, t.f_constructor) {
                Some(f) if self.k(f) == K::Identifier && f.start_byte() < f.end_byte() => {
                    let name = self.cx.name(f);
                    if self.cx.is_local(name) { NONE } else { self.canon_type(name) }
                }
                // `new ns.T()`: the type keeps its module.
                Some(f) if self.k(f) == K::MemberExpression => match (child(f, t.f_object), child(f, t.f_property)) {
                    (Some(o), Some(p)) if p.start_byte() < p.end_byte() && self.names_module(o) => {
                        let q = self.qualifier(o, 0);
                        self.module_qualified(q, self.cx.text(p))
                    }
                    _ => NONE,
                },
                _ => NONE,
            },
            K::AsExpression | K::SatisfiesExpression => match v.named_child(1) {
                Some(ty) => self.type_name(ty, 0),
                None => NONE,
            },
            K::TypeAssertion => match self.first_named(v) {
                Some(args) if self.k(args) == K::TypeArguments => match self.first_named(args) {
                    Some(ty) => self.type_name(ty, 0),
                    None => NONE,
                },
                _ => NONE,
            },
            K::ParenthesizedExpression | K::NonNullExpression => match self.first_named(v) {
                Some(x) => self.value_type(x, depth + 1),
                None => NONE,
            },
            K::This => self.this_type(),
            _ => NONE,
        }
    }

    /// Is `o` (the qualifier of `o.T`) a dotted name that starts at a module alias, a namespace
    /// or a type, and not at a value?
    fn names_module(&mut self, o: Node<'t>) -> bool {
        let t = self.t;
        let mut root = o;
        for _ in 0..8 {
            match self.k(root) {
                K::MemberExpression | K::NestedIdentifier => match child(root, t.f_object) {
                    Some(x) => root = x,
                    None => return false,
                },
                _ => break,
            }
        }
        if self.k(root) != K::Identifier || root.start_byte() == root.end_byte() {
            return false;
        }
        let text = self.cx.text(root);
        let name = self.cx.intern(text);
        if self.cx.is_local(name) {
            return false;
        }
        match self.binds.get(&name) {
            Some(Bind::Namespace(_)) | Some(Bind::Default(_)) | Some(Bind::Type) => true,
            Some(Bind::Var(_)) => false,
            Some(Bind::Alias(orig, _)) => is_type_like(self.cx.name_bytes(*orig)),
            Some(Bind::Sym) | None => is_type_like(text) && !(is_stop(text) && !self.binds.contains_key(&name)),
        }
    }

    /// Receiver path of a local initialised with `v`: its type by construction, else what the
    /// expression denotes (`make()` gives `make()`, `this.svc` gives `Class::svc`, `await x`
    /// what `x` resolves to).
    fn value_path(&mut self, v: Node<'t>) -> u32 {
        let ty = self.value_type(v, 0);
        if ty != NONE {
            return ty;
        }
        match self.k(v) {
            K::CallExpression | K::MemberExpression | K::AwaitExpression | K::Identifier | K::ParenthesizedExpression | K::NonNullExpression => {
                match self.recv(v, 0) {
                    Recv::Path(q) => q,
                    _ => NONE,
                }
            }
            _ => NONE,
        }
    }

    /// Type of a local variable: its annotation, else the path of its initialiser.
    fn local_type(&mut self, annotation: Option<Node<'t>>, value: Option<Node<'t>>) -> u32 {
        if let Some(a) = annotation {
            let ty = self.type_name(a, 0);
            if ty != NONE {
                return ty;
            }
        }
        match value {
            Some(v) => self.value_path(v),
            None => NONE,
        }
    }

    /// A type named by a bare word, as the type of a value.
    fn type_name_word(&mut self, word: &[u8]) -> u32 {
        if word == b"this" {
            return self.this_type();
        }
        if word.is_empty() || is_primitive(word) || is_type_operator(word) {
            return NONE;
        }
        let name = self.cx.intern(word);
        if self.cx.is_local(name) { NONE } else { self.canon_type(name) }
    }

    /// Simple name of a type: `Foo<T>` -> `Foo`, `ns.Foo` -> `Foo`, `Foo | undefined` -> `Foo`,
    /// `Foo[]` -> `Array`. `NONE` for primitives, type parameters and anything structural.
    fn type_name(&mut self, ty: Node<'t>, depth: u32) -> u32 {
        let t = self.t;
        if depth > 8 {
            return NONE;
        }
        match self.k(ty) {
            K::TypeAnnotation => match simple_annotation(self.cx.text(ty)) {
                Some(word) => self.type_name_word(word),
                None => match self.first_named(ty) {
                    Some(x) => self.type_name(x, depth + 1),
                    None => NONE,
                },
            },
            K::WrapType => match self.first_named(ty) {
                Some(x) => self.type_name(x, depth + 1),
                None => NONE,
            },
            K::TypeIdentifier => self.type_name_word(self.cx.text(ty)),
            // `ns.T`: the module the type was written through stays with the name.
            K::NestedTypeIdentifier => match (child(ty, t.f_name), child(ty, t.f_module)) {
                (Some(x), Some(m)) if x.start_byte() < x.end_byte() => {
                    let q = self.qualifier(m, 0);
                    self.module_qualified(q, self.cx.text(x))
                }
                _ => NONE,
            },
            K::GenericType => {
                let Some(nm) = child(ty, t.f_name) else { return NONE };
                // Wrappers that keep the members of their argument. A promise is one as far as
                // `await` goes, and nothing but `then` is ever called on the promise itself.
                if self.k(nm) == K::TypeIdentifier
                    && matches!(self.cx.text(nm), b"Readonly" | b"Partial" | b"Required" | b"NonNullable" | b"Pick" | b"Omit" | b"Promise" | b"PromiseLike")
                    && !self.shadowed(self.cx.text(nm))
                {
                    let arg = ty.named_child(1).and_then(|a| self.first_named(a));
                    return match arg {
                        Some(a) => self.type_name(a, depth + 1),
                        None => NONE,
                    };
                }
                self.type_name(nm, depth + 1)
            }
            K::UnionType => {
                // `T | undefined`, `T | null`: the one member that is a real type.
                let mut found = NONE;
                let mut i = 0;
                while let Some(m) = ty.named_child(i) {
                    i += 1;
                    match self.k(m) {
                        K::Comment => continue,
                        K::LiteralType if matches!(self.cx.text(m), b"undefined" | b"null") => continue,
                        _ => {}
                    }
                    let x = self.type_name(m, depth + 1);
                    if x == NONE || (found != NONE && found != x) {
                        return NONE;
                    }
                    found = x;
                }
                found
            }
            K::ArrayType => {
                if self.array_id == NONE {
                    self.array_id = self.cx.intern(b"Array");
                }
                self.array_id
            }
            K::ThisType => self.this_type(),
            _ => NONE,
        }
    }

    /// The function type an annotation denotes, if it is one (`(x: A) => B`, also when optional).
    fn fn_type(&self, ty: Node<'t>, depth: u32) -> Option<Node<'t>> {
        if depth > 4 {
            return None;
        }
        match self.k(ty) {
            K::FunctionType => Some(ty),
            K::TypeAnnotation => {
                // A single word is no function type.
                if simple_annotation(self.cx.text(ty)).is_some() {
                    return None;
                }
                self.fn_type(self.first_named(ty)?, depth + 1)
            }
            K::WrapType => self.fn_type(self.first_named(ty)?, depth + 1),
            K::UnionType => {
                let mut found = None;
                let mut i = 0;
                while let Some(m) = ty.named_child(i) {
                    i += 1;
                    match self.k(m) {
                        K::Comment => continue,
                        K::LiteralType if matches!(self.cx.text(m), b"undefined" | b"null") => continue,
                        _ => {}
                    }
                    if found.is_some() {
                        return None;
                    }
                    found = Some(self.fn_type(m, depth + 1)?);
                }
                found
            }
            _ => None,
        }
    }

    fn fn_type_arity(&self, f: Node<'t>) -> (u8, u8) {
        let mut i = 0;
        while let Some(x) = f.named_child(i) {
            i += 1;
            if self.k(x) == K::FormalParameters {
                return self.arity_of(x);
            }
        }
        (0, 255)
    }

    /// The function a value is, behind parentheses and casts: `((a) => ..) as Handler`.
    fn func_value(&self, v: Node<'t>) -> Option<Node<'t>> {
        let f = match self.k(v) {
            K::ArrowFunction | K::FunctionExpression => return Some(v),
            K::ParenthesizedExpression | K::AsExpression | K::SatisfiesExpression | K::TypeAssertion | K::NonNullExpression => self.peel(v),
            _ => return None,
        };
        if matches!(self.k(f), K::ArrowFunction | K::FunctionExpression) { Some(f) } else { None }
    }

    /// Visit `n`, the cursor's node, which is function `f` or wraps it in parentheses and
    /// casts: `f` is walked as definition `def`, everything around it as usual.
    fn wrapped_function(&mut self, n: Node<'t>, f: Node<'t>, c: &mut TreeCursor<'t>, def: u32, owner: u32) {
        if n.id() == f.id() {
            let mode = if self.k(n) == K::ArrowFunction { INHERIT } else { owner };
            self.function(n, c, def, mode, true);
            return;
        }
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        each_child!(self, n, c, |ch, k| {
            if k != K::Anon && ch.start_byte() <= f.start_byte() && f.end_byte() <= ch.end_byte() {
                self.wrapped_function(ch, f, c, def, owner);
            } else {
                self.node(ch, k, c);
            }
        });
        self.cx.depth -= 1;
    }

    /// Expression behind parentheses, casts and `!`.
    fn peel(&self, mut e: Node<'t>) -> Node<'t> {
        for _ in 0..4 {
            let inner = match self.k(e) {
                K::ParenthesizedExpression | K::NonNullExpression | K::AsExpression | K::SatisfiesExpression => self.first_named(e),
                K::TypeAssertion => e.named_child(1),
                _ => None,
            };
            match inner {
                Some(x) => e = x,
                None => break,
            }
        }
        e
    }

    fn is_literal(&self, v: Node<'t>) -> bool {
        let v = self.peel(v);
        match self.k(v) {
            K::Number | K::String | K::Literal => true,
            K::UnaryExpression => child(v, self.t.f_argument).is_some_and(|a| self.k(a) == K::Number),
            K::TemplateString => {
                let mut i = 0;
                while let Some(x) = v.named_child(i) {
                    i += 1;
                    if self.k(x) != K::Skip {
                        return false;
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// `require('m')`, `import('m')` or `await import('m')` with a literal argument: the module
    /// path.
    fn require_target(&self, call: Node<'t>) -> Option<&'a [u8]> {
        let t = self.t;
        let (call, awaited) = match self.k(call) {
            K::AwaitExpression => (self.first_named(call)?, true),
            _ => (call, false),
        };
        if self.k(call) != K::CallExpression {
            return None;
        }
        let f = child(call, t.f_function)?;
        match self.k(f) {
            K::Identifier if self.cx.text(f) == b"require" => {}
            // Without `await` the value is a promise of the module, not the module.
            K::Import if awaited => {}
            _ => return None,
        }
        let a = child(call, t.f_arguments)?;
        if self.k(a) != K::Arguments {
            return None;
        }
        let s = self.first_named(a)?;
        if self.k(s) != K::String {
            return None;
        }
        Some(self.string_content(s))
    }

    fn add_module_import(&mut self, target: &[u8], name: u32, alias: u32, line: u32, flags: u8) -> u32 {
        let tid = self.cx.intern(target);
        self.cx.add_import(tid, name, alias, line, flags | import_flags(target));
        tid
    }

    #[allow(clippy::too_many_arguments)]
    fn declarator(&mut self, d: Node<'t>, c: &mut TreeCursor<'t>, span: Node<'t>, flags: u8, is_const: bool, top: bool) {
        let t = self.t;
        let Some(name_node) = child(d, t.f_name) else { return self.children(d, c) };
        let value = child(d, t.f_value);
        let annotation = child(d, t.f_type);
        if self.k(name_node) != K::Identifier {
            return self.destructure(d, c, name_node, value, span, flags, top);
        }
        if name_node.start_byte() == name_node.end_byte() {
            return self.children(d, c);
        }
        let name = self.cx.name(name_node);
        let vk = value.map(|v| self.k(v));
        // A function or class bound to a name is that function or class.
        let func = value.and_then(|v| self.func_value(v));
        if let Some(f) = func.or(value.filter(|_| vk == Some(K::Class))) {
            let mut def = NONE;
            if func.is_some() {
                let fl = self.decl_flags(flags);
                let sig_end = child(f, t.f_body).map(|b| b.start_byte()).unwrap_or(d.end_byte());
                def = self.def(name, DefKind::Function, span, sig_end, fl);
                self.register(def);
            }
            each_child!(self, d, c, |ch, k| {
                if value.is_some_and(|v| v.id() == ch.id()) {
                    match k {
                        K::Class => self.class(ch, k, c, span, flags, name),
                        _ => self.wrapped_function(ch, f, c, def, NONE),
                    }
                } else if ch.id() != name_node.id() {
                    // The annotation: `const f: Handler = (e) => ..`
                    if def != NONE {
                        self.cx.push_def(def);
                    }
                    self.node(ch, k, c);
                    if def != NONE {
                        self.cx.pop_def();
                    }
                }
            });
            return;
        }
        // `const m = require('m')`, `const f = require('m').f`
        if let Some(v) = value {
            if let Some(target) = self.require_target(v) {
                let tid = self.add_module_import(target, NONE, name, line_of(d), 0);
                if top {
                    self.binds.insert(name, Bind::Namespace(tid));
                    self.has_alias = true;
                    self.is_module = true;
                } else {
                    // A local bound to a module: its "type" is the marker `TARGET|`.
                    let marker = self.join(tid, b"|", b"");
                    self.cx.declare(name, marker);
                }
                return;
            }
            if vk == Some(K::MemberExpression) && top {
                if let (Some(o), Some(p)) = (child(v, t.f_object), child(v, t.f_property)) {
                    if let Some(target) = self.require_target(o) {
                        let sym = self.cx.name(p);
                        let tid = self.add_module_import(target, sym, if sym == name { NONE } else { name }, line_of(d), 0);
                        self.cx.add_ref(sym, RefKind::Import, line_of(d), tid, 255);
                        if sym == name {
                            self.binds.insert(name, Bind::Sym);
                        } else {
                            self.binds.insert(name, Bind::Alias(sym, tid));
                            self.has_alias = true;
                        }
                        self.is_module = true;
                        return;
                    }
                }
            }
        }
        let object = value.map(|v| self.peel(v)).filter(|v| self.k(*v) == K::Object);
        if !top {
            let ty = self.local_type(annotation, value);
            self.cx.declare(name, ty);
            if let (Some(obj), true) = (object, ty != NONE) {
                self.obj_ctx = Some(ObjCtx { id: obj.id(), ty, owner: NONE, mode: ObjMode::Plain });
            }
            each_child!(self, d, c, |ch, k| {
                if ch.id() != name_node.id() {
                    self.node(ch, k, c);
                }
            });
            self.obj_ctx = None;
            return;
        }
        let literal = value.is_some_and(|v| self.is_literal(v));
        let kind = if is_const && (literal || is_all_caps(self.cx.text(name_node))) { DefKind::Constant } else { DefKind::Variable };
        let mut fl = self.decl_flags(flags);
        if value.is_none() && self.is_ambient() {
            fl |= dflag::DECL;
        }
        let sig_end = value.map(|v| v.start_byte()).unwrap_or(d.end_byte());
        let def = self.def(name, kind, span, sig_end, fl);
        self.register(def);
        let ty = self.typed(def, annotation, value);
        if self.local_depth == 0 {
            // As a receiver the variable is `$name`: the resolver reads its declared type, or
            // takes an object literal for the container of its members. Only what the
            // definition cannot say is kept here: the path of a call result or of a member.
            let path = match value {
                Some(v) if ty == NONE && object.is_none() => self.value_path(v),
                _ => NONE,
            };
            self.binds.insert(name, Bind::Var(path));
        }
        if let Some(obj) = object {
            let owner = if self.local_depth == 0 { self.prefixed(b"$", name) } else { NONE };
            if self.local_depth == 0 {
                self.spread_namespaces(obj, name, line_of(d));
            }
            self.obj_ctx = Some(ObjCtx { id: obj.id(), ty, owner, mode: ObjMode::Plain });
        }
        self.cx.push_def(def);
        // `const f = handler`, `const f = Class.method`: the variable is a slot holding that
        // function (or value).
        let mut bound = false;
        if let Some(v) = value {
            if let Some((vn, again)) = self.bindable(v) {
                let slot = self.slot(NONE, name);
                self.cx.add_ref(vn, RefKind::Bind, line_of(v), slot, 255);
                let owner = self.bound_owner(v);
                self.slot_values.push((def, vn, owner));
                bound = !again;
            } else if let Some(vn) = self.bound_function(v) {
                // `const join = path.join.bind(path)`: certainly a function.
                let slot = self.slot(NONE, name);
                self.cx.add_ref(vn, RefKind::Bind, line_of(v), slot, 255);
                self.cx.def_mut(def).flags |= dflag::FNPTR;
            }
        }
        each_child!(self, d, c, |ch, k| {
            if ch.id() != name_node.id() && !(bound && value.is_some_and(|v| v.id() == ch.id())) {
                self.node(ch, k, c);
            }
        });
        self.cx.pop_def();
        self.obj_ctx = None;
    }

    /// `const { a, b: c } = x`, `const [d, e] = y`.
    #[allow(clippy::too_many_arguments)]
    fn destructure(&mut self, d: Node<'t>, c: &mut TreeCursor<'t>, pattern: Node<'t>, value: Option<Node<'t>>, span: Node<'t>, flags: u8, top: bool) {
        let t = self.t;
        let object = self.k(pattern) == K::ObjectPattern;
        // `const { a, b: c } = require('m')`: named imports.
        if let Some(target) = value.filter(|_| object).and_then(|v| self.require_target(v)) {
            let line = line_of(d);
            let tid = self.cx.intern(target);
            let iflags = import_flags(target);
            let mut cc = pattern.walk();
            if cc.goto_first_child() {
                loop {
                    let ch = cc.node();
                    let pair = match self.k(ch) {
                        K::ShorthandPattern => Some((ch, ch)),
                        K::PairPattern => match (child(ch, t.f_key), child(ch, t.f_value)) {
                            (Some(k), Some(v)) if self.k(k) == K::PropertyIdentifier && self.k(v) == K::Identifier => Some((k, v)),
                            _ => None,
                        },
                        _ => None,
                    };
                    if let Some((k, v)) = pair {
                        let sym = self.cx.name(k);
                        let local = self.cx.name(v);
                        self.cx.add_import(tid, sym, if sym == local { NONE } else { local }, line, iflags);
                        self.cx.add_ref(sym, RefKind::Import, line, tid, 255);
                        if !top {
                            // A local bound to a symbol of a module: the value `TARGET|$sym`.
                            let ty = self.join(tid, b"|$", self.cx.text(k));
                            self.cx.declare(local, ty);
                        } else if sym == local {
                            self.binds.insert(local, Bind::Sym);
                        } else {
                            self.binds.insert(local, Bind::Alias(sym, tid));
                            self.has_alias = true;
                        }
                    }
                    if !cc.goto_next_sibling() {
                        break;
                    }
                }
            }
            if top {
                self.is_module = true;
            }
            return;
        }
        let source = match value.filter(|_| object) {
            Some(v) => self.recv(v, 0),
            None => Recv::Unknown,
        };
        if !top {
            self.declare_pattern(pattern, NONE, 0);
            self.pattern_reads(pattern, source, false);
            return self.children(d, c);
        }
        let sig_end = value.map(|v| v.start_byte()).unwrap_or(d.end_byte());
        let fl = self.decl_flags(flags);
        let first = self.cx.facts.defs.len() as u32;
        self.pattern_defs(pattern, span, sig_end, fl, 0);
        let pushed = (self.cx.facts.defs.len() as u32) > first;
        if pushed {
            self.cx.push_def(first);
        }
        self.pattern_reads(pattern, source, self.local_depth == 0);
        self.children(d, c);
        if pushed {
            self.cx.pop_def();
        }
    }

    /// Module-level destructuring: every bound name is a variable.
    fn pattern_defs(&mut self, p: Node<'t>, span: Node<'t>, sig_end: usize, flags: u8, depth: u32) {
        let t = self.t;
        if depth > 24 {
            return;
        }
        match self.k(p) {
            k @ (K::Identifier | K::ShorthandPattern) => {
                if p.start_byte() < p.end_byte() {
                    let name = self.cx.name(p);
                    let d = self.def(name, DefKind::Variable, span, sig_end, flags);
                    self.register(d);
                    if k == K::Identifier {
                        // The walk over the pattern must not take the name for a reference.
                        self.cx.mark(p);
                        self.marks = true;
                    }
                    if self.local_depth == 0 {
                        self.binds.insert(name, Bind::Var(NONE));
                    }
                }
            }
            K::AssignmentPattern => {
                if let Some(l) = child(p, t.f_left) {
                    self.pattern_defs(l, span, sig_end, flags, depth + 1);
                }
            }
            K::PairPattern => {
                if let Some(v) = child(p, t.f_value) {
                    self.pattern_defs(v, span, sig_end, flags, depth + 1);
                }
            }
            K::RestPattern | K::ObjectPattern | K::ArrayPattern => {
                let mut cc = p.walk();
                if cc.goto_first_child() {
                    loop {
                        let ch = cc.node();
                        if !matches!(self.k(ch), K::Anon | K::Comment) {
                            self.pattern_defs(ch, span, sig_end, flags, depth + 1);
                        }
                        if !cc.goto_next_sibling() {
                            break;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // ----- statements -----

    /// A block (or a statement with its own bindings): a scope of locals.
    fn block(&mut self, n: Node<'t>, k: K, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        self.local_depth += 1;
        if k == K::ForStatement {
            self.children(n, c);
        } else {
            if k == K::StatementBlock {
                self.hoist_lexical(n, c);
            }
            self.inner(n, c);
        }
        self.local_depth -= 1;
        self.cx.pop_scope();
    }

    fn for_in(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        self.cx.push_scope();
        self.local_depth += 1;
        let left = child(n, t.f_left);
        let mut declares = false;
        each_child!(self, n, c, |ch, k| {
            match k {
                K::Anon => declares |= matches!(self.tk(ch), Tk::Const | Tk::Var | Tk::Decl),
                _ if left.is_some_and(|l| l.id() == ch.id()) => {
                    if declares {
                        self.declare_pattern(ch, NONE, 0);
                        self.node(ch, k, c);
                    } else if k == K::Identifier {
                        self.identifier(ch, RefKind::Write);
                    } else {
                        self.node(ch, k, c);
                    }
                }
                _ => self.node(ch, k, c),
            }
        });
        self.local_depth -= 1;
        self.cx.pop_scope();
    }

    fn catch_clause(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        self.cx.push_scope();
        self.local_depth += 1;
        if let Some(p) = child(n, t.f_parameter) {
            self.declare_pattern(p, NONE, 0);
        }
        each_child!(self, n, c, |ch, k| {
            // The handler's block shares the scope of its parameter.
            if k == K::StatementBlock {
                self.inner(ch, c);
            } else {
                self.node(ch, k, c);
            }
        });
        self.local_depth -= 1;
        self.cx.pop_scope();
    }

    // ----- type-level nodes -----

    /// A parameter outside a function with a body (function types, call signatures, tuples):
    /// its name is not a reference.
    fn loose_parameter(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let name = child(n, t.f_pattern).or_else(|| child(n, t.f_name));
        each_child!(self, n, c, |ch, k| {
            if name.is_none_or(|x| x.id() != ch.id()) {
                self.node(ch, k, c);
            }
        });
    }

    /// `[key: string]: T`, `[K in keyof T]: T[K]`.
    fn index_signature(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        self.cx.push_scope();
        let key = child(n, t.f_name);
        each_child!(self, n, c, |ch, k| {
            if key.is_some_and(|x| x.id() == ch.id()) {
                // the key's name
            } else if k == K::MappedTypeClause {
                if let Some(nm) = child(ch, t.f_name) {
                    self.declare_name(nm);
                }
                self.children(ch, c);
            } else {
                self.node(ch, k, c);
            }
        });
        self.cx.pop_scope();
    }

    /// `infer U`: `U` is a type variable of the enclosing declaration.
    fn infer_type(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let var = self.first_named(n).filter(|x| self.k(*x) == K::TypeIdentifier);
        if let Some(nm) = var {
            self.declare_name(nm);
        }
        let scoped = self.cx.in_scope();
        each_child!(self, n, c, |ch, k| {
            if scoped || var.is_none_or(|x| x.id() != ch.id()) {
                self.node(ch, k, c);
            }
        });
    }

    /// `x is T`: `x` names a parameter.
    fn type_predicate(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let name = child(n, self.t.f_name);
        each_child!(self, n, c, |ch, k| {
            if name.is_none_or(|x| x.id() != ch.id()) {
                self.node(ch, k, c);
            }
        });
    }

    // ----- expressions -----

    /// What a bare word used as the object of a member expression denotes. With a `line`, the
    /// reference the word itself makes (to a variable, type or namespace) is recorded as well.
    fn recv_word(&mut self, word: &[u8], line: u32) -> Recv {
        match word {
            b"this" => return if self.this_ty == NONE { Recv::Unknown } else { Recv::Path(self.this_ty) },
            b"super" => {
                return match self.super_qual() {
                    NONE => Recv::Unknown,
                    q => Recv::Path(q),
                };
            }
            _ => {}
        }
        let name = self.cx.intern(word);
        if let Some(ty) = self.cx.local_type(name) {
            if ty == NONE {
                return Recv::Unknown;
            }
            // `const m = require('m')` inside a function: a module alias with a local scope.
            let tb = self.cx.name_bytes(ty);
            if let Some(target) = tb.strip_suffix(b"|") {
                let target = target.to_vec();
                return Recv::Module(self.cx.intern(&target));
            }
            return Recv::Path(ty);
        }
        let mut type_like = is_type_like(word);
        let mut known = NONE;
        let mut namespace = false;
        match self.binds.get(&name).copied() {
            Some(Bind::Namespace(target)) | Some(Bind::Default(target)) => return Recv::Module(target),
            Some(Bind::Var(path)) => {
                known = path;
                type_like = false;
            }
            Some(Bind::Type) => namespace = true,
            Some(Bind::Alias(orig, target)) => {
                // `import { a as b }`: `b.f()` is on `a` of that module.
                if line != 0 {
                    self.cx.add_ref(orig, RefKind::Read, line, target, 255);
                }
                let ob = self.cx.name_bytes(orig);
                return if is_type_like(ob) {
                    Recv::Type(self.join(target, b"|", ob.to_vec().as_slice()))
                } else {
                    Recv::Path(self.join(target, b"|$", ob.to_vec().as_slice()))
                };
            }
            Some(Bind::Sym) => {}
            None => {
                if is_stop(word) {
                    if word == b"arguments" {
                        self.uses_arguments = true;
                    }
                    return Recv::Stop;
                }
            }
        }
        if line != 0 {
            self.cx.add_ref(name, RefKind::Read, line, NONE, 255);
        }
        if known != NONE {
            Recv::Path(known)
        } else if namespace {
            Recv::Ns(name)
        } else if type_like {
            Recv::Type(name)
        } else {
            Recv::Path(self.prefixed(b"$", name))
        }
    }

    /// What an expression used as the object of a member expression denotes.
    fn recv(&mut self, e: Node<'t>, depth: u32) -> Recv {
        let t = self.t;
        if depth > 8 {
            return Recv::Unknown;
        }
        match self.k(e) {
            K::This | K::Super | K::Identifier => self.recv_word(self.cx.text(e), 0),
            K::MemberExpression => {
                if let Some((word, prop)) = simple_member(self.cx.text(e)) {
                    let r = self.recv_word(word, 0);
                    return self.extend(r, prop);
                }
                let (Some(o), Some(p)) = (child(e, t.f_object), child(e, t.f_property)) else { return Recv::Unknown };
                let r = self.recv(o, depth + 1);
                self.extend(r, self.cx.text(p))
            }
            K::ParenthesizedExpression | K::NonNullExpression => match self.first_named(e) {
                Some(x) => self.recv(x, depth + 1),
                None => Recv::Unknown,
            },
            K::AsExpression | K::SatisfiesExpression | K::TypeAssertion | K::NewExpression => match self.value_type(e, 0) {
                NONE => Recv::Unknown,
                ty => Recv::Path(ty),
            },
            K::CallExpression => {
                if let Some(target) = self.require_target(e) {
                    return Recv::Module(self.cx.intern(target));
                }
                // `f()`: resolved through the return type of `f`.
                let mut function = child(e, t.f_function);
                // `await f<T>(x)` arrives as a call of `await f` (see `callee_slot`).
                if let Some(a) = function.filter(|f| self.k(*f) == K::AwaitExpression && self.misplaced_type_arguments(e)) {
                    function = a.named_child(a.named_child_count().saturating_sub(1) as u32);
                }
                match function {
                    Some(f) if self.k(f) == K::Identifier => {
                        let text = self.cx.text(f);
                        if text.is_empty() || (is_stop(text) && !self.shadowed(text)) {
                            return Recv::Unknown;
                        }
                        let name = self.cx.intern(text);
                        // A variable known to hold a function: the result of that function.
                        let held = match self.cx.local_type(name) {
                            Some(ty) => Some(ty),
                            None => match self.binds.get(&name) {
                                Some(Bind::Var(path)) if *path != NONE => Some(*path),
                                _ => None,
                            },
                        };
                        if let Some(path) = held {
                            return match self.call_of(path) {
                                NONE => Recv::Unknown,
                                q => Recv::Path(q),
                            };
                        }
                        match self.canon_type(name) {
                            NONE => Recv::Unknown,
                            name => Recv::Path(self.join(name, b"()", b"")),
                        }
                    }
                    // `x.m()`: the declared result of method `m` of what `x` is; `ns.f()`: the
                    // result of function `f` of that module.
                    Some(f) if self.k(f) == K::MemberExpression => {
                        let (r, prop) = match simple_member(self.cx.text(f)) {
                            Some((word, prop)) => (self.recv_word(word, 0), prop),
                            None => {
                                let (Some(o), Some(p)) = (child(f, t.f_object), child(f, t.f_property)) else { return Recv::Unknown };
                                (self.recv(o, depth + 1), self.cx.text(p))
                            }
                        };
                        if prop.is_empty() || matches!(prop, b"call" | b"apply" | b"bind") {
                            return Recv::Unknown;
                        }
                        match r {
                            Recv::Module(m) => {
                                let q = self.module_qualified(m, prop);
                                Recv::Path(self.join(q, b"()", b""))
                            }
                            Recv::Path(q) => {
                                let q = self.segment(q, prop);
                                Recv::Path(self.join(q, b"()", b""))
                            }
                            Recv::Type(q) | Recv::Ns(q) => {
                                let root = self.value_root(q);
                                let q = self.segment(root, prop);
                                Recv::Path(self.join(q, b"()", b""))
                            }
                            Recv::Unknown | Recv::Stop => Recv::Unknown,
                        }
                    }
                    _ => Recv::Unknown,
                }
            }
            // What a promise resolves to: declared results are recorded without the wrapper.
            K::AwaitExpression => match self.first_named(e) {
                Some(_) if self.require_target(e).is_some() => match self.require_target(e) {
                    Some(target) => Recv::Module(self.cx.intern(target)),
                    None => Recv::Unknown,
                },
                Some(x) => match self.recv(x, depth + 1) {
                    Recv::Path(q) => Recv::Path(q),
                    Recv::Module(m) => Recv::Module(m),
                    _ => Recv::Unknown,
                },
                None => Recv::Unknown,
            },
            K::String | K::Number | K::Literal | K::TemplateString | K::Array | K::MetaProperty => Recv::Stop,
            _ => Recv::Unknown,
        }
    }

    /// `q::prop`. The `^` of a `super` receiver does not survive a further segment: the member
    /// is looked up on the class, which finds inherited members as well.
    fn segment(&mut self, q: u32, prop: &[u8]) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let qb = self.cx.name_bytes(q);
        buf.extend_from_slice(qb.strip_prefix(b"^").unwrap_or(qb));
        buf.extend_from_slice(b"::");
        buf.extend_from_slice(prop);
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// `r.prop` as a receiver.
    fn extend(&mut self, r: Recv, prop: &[u8]) -> Recv {
        match r {
            Recv::Unknown => Recv::Unknown,
            Recv::Stop => Recv::Stop,
            // `ns.Type`: a type written with its module; `ns.obj`: a module-level object of
            // that module.
            Recv::Module(m) => {
                if is_type_like(prop) {
                    Recv::Type(self.module_qualified(m, prop))
                } else {
                    Recv::Path(self.join(m, b"|$", prop))
                }
            }
            Recv::Path(q) => Recv::Path(self.segment(q, prop)),
            Recv::Type(q) | Recv::Ns(q) => {
                let root = self.value_root(q);
                Recv::Path(self.segment(root, prop))
            }
        }
    }

    /// The path root of type or namespace `q` used as a value: `$T`, or `TARGET|$T`.
    fn value_root(&mut self, q: u32) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let qb = self.cx.facts.names.get(q);
        match memchr::memchr(b'|', qb) {
            Some(at) => {
                buf.extend_from_slice(&qb[..at + 1]);
                buf.push(b'$');
                buf.extend_from_slice(&qb[at + 1..]);
            }
            None => {
                buf.push(b'$');
                buf.extend_from_slice(qb);
            }
        }
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// The reference `r.prop` makes to `prop`.
    fn target(&mut self, r: Recv, prop: &[u8], line: u32, call: bool, write: bool) -> Option<Target> {
        let member = if call {
            RefKind::MemberCall
        } else if write {
            RefKind::MemberWrite
        } else {
            RefKind::MemberRead
        };
        let (kind, qual) = match r {
            Recv::Stop => return None,
            Recv::Unknown => (member, NONE),
            Recv::Path(q) => (member, q),
            // `T.f()` is a call qualified by the type or namespace; `T.x` reads a member of it
            // used as a value.
            Recv::Type(q) => {
                if call {
                    (RefKind::Call, q)
                } else {
                    (member, self.value_root(q))
                }
            }
            Recv::Ns(q) => (member, self.value_root(q)),
            Recv::Module(q) => {
                let kind = if call {
                    RefKind::Call
                } else if write {
                    RefKind::Write
                } else {
                    RefKind::Read
                };
                (kind, q)
            }
        };
        if prop.is_empty() {
            return None;
        }
        Some((self.cx.intern(prop), kind, qual, line))
    }

    /// Walk member expression `m`, the cursor's node: record what its object refers to and
    /// return what the object denotes, the property and the property's line. Chains are walked
    /// once, from the root outwards.
    fn member_parts(&mut self, m: Node<'t>, c: &mut TreeCursor<'t>) -> Option<(Recv, &'a [u8], u32)> {
        let text = self.cx.text(m);
        if let Some((word, prop)) = simple_member(text) {
            // `this.x`, `a.b`: nothing below needs a visit.
            let r = self.recv_word(word, line_of(m));
            return Some((r, prop, end_line_of(m)));
        }
        let t = self.t;
        let (Some(o), Some(p)) = (child(m, t.f_object), child(m, t.f_property)) else {
            self.children(m, c);
            return None;
        };
        let ok = self.k(o);
        let mut r = Recv::Unknown;
        // The object is the first child; the property after it holds nothing.
        if c.goto_first_child() {
            if ok != K::MemberExpression {
                r = self.recv(o, 0);
                self.node(o, ok, c);
            } else if self.cx.depth < MAX_DEPTH {
                self.cx.depth += 1;
                if let Some((ro, po, lo)) = self.member_parts(o, c) {
                    if let Some((name, kind, qual, line)) = self.target(ro, po, lo, false, false) {
                        self.cx.add_ref(name, kind, line, qual, 255);
                    }
                    r = self.extend(ro, po);
                }
                self.cx.depth -= 1;
            }
            c.goto_parent();
        }
        if p.start_byte() == p.end_byte() {
            return None;
        }
        Some((r, self.cx.text(p), line_of(p)))
    }

    /// The reference member expression `m` makes to its property, after walking its object.
    fn member_ref(&mut self, m: Node<'t>, c: &mut TreeCursor<'t>, call: bool, write: bool) -> Option<Target> {
        let (r, prop, line) = self.member_parts(m, c)?;
        self.target(r, prop, line, call, write)
    }

    /// Like `member_ref`, without a cursor and without walking the object.
    fn member_target(&mut self, m: Node<'t>, call: bool) -> Option<Target> {
        let t = self.t;
        if let Some((word, prop)) = simple_member(self.cx.text(m)) {
            let r = self.recv_word(word, 0);
            return self.target(r, prop, end_line_of(m), call, false);
        }
        let (o, p) = (child(m, t.f_object)?, child(m, t.f_property)?);
        let r = self.recv(o, 0);
        self.target(r, self.cx.text(p), line_of(p), call, false)
    }

    /// Count the arguments of `n`, an `arguments` node, while visiting them.
    fn arguments(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) -> u8 {
        let mut count = 0u32;
        let mut spread = false;
        each_inner!(self, n, c, |ch, k| {
            match k {
                K::Anon | K::Comment => {}
                K::SpreadElement => {
                    spread = true;
                    self.node(ch, k, c);
                }
                _ => {
                    count += 1;
                    self.node(ch, k, c);
                }
            }
        });
        if spread { 255 } else { count.min(254) as u8 }
    }

    fn call(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let mut target: Option<Target> = None;
        let mut nargs = 255u8;
        // -1: `f.call(this, ..)` passes one argument less, 1: `f.apply(this, args)` an unknown number
        let mut shift = 0i8;
        let mut left = n.child_count();
        if left > 0 && c.goto_first_child() {
            // The callee comes first.
            let f = c.node();
            let fk = self.k(f);
            (target, shift) = self.callee_slot(n, f, fk, c, 0);
            left -= 1;
            while left > 0 && c.goto_next_sibling() {
                let ch = c.node();
                let k = self.k(ch);
                if k == K::Arguments {
                    // Nothing follows the arguments.
                    nargs = self.arguments(ch, c);
                    break;
                }
                self.node(ch, k, c);
                left -= 1;
            }
            c.goto_parent();
        }
        if let Some((name, kind, qual, line)) = target {
            let nargs = match shift {
                -1 if nargs != 255 => nargs.saturating_sub(1),
                1 => 255,
                _ => nargs,
            };
            self.cx.add_ref(name, kind, line, qual, nargs);
        }
    }

    /// What sits in the callee position `f` of `call` (the cursor is on it): the reference the
    /// call makes, still without its argument count, and how that count shifts.
    fn callee_slot(&mut self, call: Node<'t>, f: Node<'t>, fk: K, c: &mut TreeCursor<'t>, depth: u32) -> (Option<Target>, i8) {
        let t = self.t;
        let mut target: Option<Target> = None;
        let mut shift = 0i8;
        match fk {
            K::Identifier => {
                let text = self.cx.text(f);
                if text == b"require" {
                    if let Some(m) = self.require_target(call) {
                        self.add_module_import(m, NONE, NONE, line_of(call), 0);
                    }
                } else if text == b"await" {
                    // `await (expr)(..)` is read by the grammar as a call of `await`: the keyword
                    // is no symbol, and what is awaited is visited as the argument it became.
                } else if let Some(name) = self.free_name(f) {
                    target = Some((name, RefKind::Call, self.free_qual, line_of(f)));
                } else if let Some((name, module)) = self.module_callee(f) {
                    target = Some((name, RefKind::Call, module, line_of(f)));
                } else {
                    target = self.destructured_call(f);
                }
            }
            K::MemberExpression => return self.callee(call, f, c),
            K::SubscriptExpression => {
                // `x["name"](..)`: a member call written with a string.
                target = self.subscript_target(f);
                self.node(f, fk, c);
            }
            K::Super => {
                // `super(...)` runs the constructor of an ancestor.
                let q = self.super_qual();
                if q != NONE {
                    let name = self.cx.intern(b"constructor");
                    target = Some((name, RefKind::MemberCall, q, line_of(f)));
                }
            }
            K::Import => {
                // `import('./lazy')`
                let arg = child(call, t.f_arguments).and_then(|a| self.first_named(a));
                if let Some(s) = arg.filter(|s| self.k(*s) == K::String) {
                    let m = self.string_content(s);
                    self.add_module_import(m, NONE, NONE, line_of(call), 0);
                }
            }
            // The TypeScript grammar binds explicit type arguments more loosely than a prefix or
            // binary operator: `await f<T>(x)`, `!f<T>(x)` and `a + f<T>(x)` arrive as a call
            // whose callee is `await f`, `!f`, `a + f`. What is called is the last operand.
            K::UnaryExpression | K::AwaitExpression | K::BinaryExpression if depth < 8 && self.misplaced_type_arguments(call) => {
                let mut left = f.child_count();
                if left > 0 && c.goto_first_child() {
                    loop {
                        let ch = c.node();
                        let k = self.k(ch);
                        left -= 1;
                        if left == 0 {
                            (target, shift) = self.callee_slot(call, ch, k, c, depth + 1);
                            break;
                        }
                        self.node(ch, k, c);
                        if !c.goto_next_sibling() {
                            break;
                        }
                    }
                    c.goto_parent();
                }
            }
            // `(f)(x)`, `(a.b)(x)`: the parentheses change nothing.
            K::ParenthesizedExpression
                if depth < 8 && f.named_child_count() == 1 && f.named_child(0).is_some_and(|x| matches!(self.k(x), K::Identifier | K::MemberExpression)) =>
            {
                if c.goto_first_child() {
                    while !c.node().is_named() && c.goto_next_sibling() {}
                    let x = c.node();
                    let xk = self.k(x);
                    if matches!(xk, K::Identifier | K::MemberExpression) {
                        (target, shift) = self.callee_slot(call, x, xk, c, depth + 1);
                    }
                    c.goto_parent();
                }
            }
            // `await (f)(x)`, read as `(await(f))(x)`: `f` is what is called.
            K::CallExpression if depth < 8 => match self.awaited_callee(f) {
                Some((args, arg)) => {
                    if c.goto_first_child() {
                        while c.node().id() != args.id() && c.goto_next_sibling() {}
                        if c.node().id() == args.id() && c.goto_first_child() {
                            while c.node().id() != arg.id() && c.goto_next_sibling() {}
                            if c.node().id() == arg.id() {
                                (target, shift) = self.callee_slot(call, arg, self.k(arg), c, depth + 1);
                            }
                            c.goto_parent();
                        }
                        c.goto_parent();
                    }
                }
                None => self.node(f, fk, c),
            },
            _ => self.node(f, fk, c),
        }
        (target, shift)
    }

    /// `m(..)` or `new m(..)` where `m` is bound to a whole module (`const m = require('m')`,
    /// `import m = require('m')`): what runs is the export of that module (`module.exports =
    /// function ..`, `export = ..`). The reference is made under the binding's name, by
    /// convention the name of that export, qualified by the import target.
    fn module_callee(&mut self, f: Node<'t>) -> Option<(u32, u32)> {
        let text = self.cx.text(f);
        if text.is_empty() {
            return None;
        }
        let name = self.cx.intern(text);
        if let Some(ty) = self.cx.local_type(name) {
            if ty == NONE {
                return None;
            }
            let target = self.cx.name_bytes(ty).strip_suffix(b"|")?.to_vec();
            return Some((name, self.cx.intern(&target)));
        }
        match self.binds.get(&name) {
            Some(Bind::Namespace(target)) => Some((name, *target)),
            _ => None,
        }
    }

    /// The call carries type arguments although its callee is an operator expression: see
    /// `callee_slot`.
    #[inline]
    fn misplaced_type_arguments(&self, call: Node<'t>) -> bool {
        self.ts && child(call, self.t.f_type_arguments).is_some()
    }

    /// `await(f)` as the grammar reads `await (f)` in front of a second argument list: the
    /// argument list and the single name or member expression in it.
    fn awaited_callee(&self, inner: Node<'t>) -> Option<(Node<'t>, Node<'t>)> {
        let t = self.t;
        let f = child(inner, t.f_function).filter(|f| self.k(*f) == K::Identifier && self.cx.text(*f) == b"await")?;
        let _ = f;
        let args = child(inner, t.f_arguments).filter(|a| a.named_child_count() == 1)?;
        let arg = args.named_child(0).filter(|x| matches!(self.k(*x), K::Identifier | K::MemberExpression))?;
        Some((args, arg))
    }

    /// `x["name"]` as a callee: the member call it makes, when the index is a string that could
    /// name a member.
    fn subscript_target(&mut self, f: Node<'t>) -> Option<Target> {
        let t = self.t;
        let (o, i) = (child(f, t.f_object)?, child(f, t.f_index)?);
        if self.k(i) != K::String {
            return None;
        }
        let name = self.string_content(i);
        if name.is_empty() || name.len() > 64 || name.iter().any(|&b| b == b' ' || b == b'\\' || b == b'\n') {
            return None;
        }
        let r = self.recv(o, 0);
        self.target(r, name, line_of(i), true, false)
    }

    /// `const { onChange } = props; onChange(x)` calls member `onChange` of what `props` is: a
    /// local bound by destructuring carries the path of the member it came from.
    /// Likewise `const run = this.run; run()` and `const f = ns.func; f()`.
    fn destructured_call(&mut self, f: Node<'t>) -> Option<Target> {
        let name = self.cx.intern(self.cx.text(f));
        let ty = self.cx.local_type(name).filter(|ty| *ty != NONE)?;
        let path = self.cx.facts.names.get(ty);
        if path.ends_with(b")") {
            return None;
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.extend_from_slice(path);
        let target = if let Some(at) = memchr::memmem::rfind(&buf, b"::") {
            let owner = self.cx.intern(&buf[..at]);
            Some((self.cx.intern(&buf[at + 2..]), RefKind::MemberCall, owner, line_of(f)))
        } else if let Some(at) = memchr::memmem::find(&buf, b"|$") {
            let module = self.cx.intern(&buf[..at]);
            Some((self.cx.intern(&buf[at + 2..]), RefKind::Call, module, line_of(f)))
        } else {
            None
        };
        self.cx.s.buf = buf;
        target
    }

    /// The path of the result of calling a variable whose value has path `path`: a variable
    /// holding `Class::method` yields `Class::method()`, one holding `TARGET|$func` yields
    /// `TARGET|func()`, one holding `$func` yields `func()`.
    fn call_of(&mut self, path: u32) -> u32 {
        if path == NONE {
            return NONE;
        }
        let p = self.cx.facts.names.get(path);
        if p.ends_with(b")") {
            return NONE;
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        if memchr::memmem::find(p, b"::").is_some() {
            buf.extend_from_slice(p);
        } else if let Some(at) = memchr::memmem::find(p, b"|$") {
            buf.extend_from_slice(&p[..at + 1]);
            buf.extend_from_slice(&p[at + 2..]);
        } else if let Some(var) = p.strip_prefix(b"$") {
            buf.extend_from_slice(var);
        }
        let id = if buf.is_empty() {
            NONE
        } else {
            buf.extend_from_slice(b"()");
            self.cx.intern(&buf)
        };
        self.cx.s.buf = buf;
        id
    }

    /// The callee `f` of `call` is a member expression (the cursor is on it). `f.call(this, ..)`
    /// and `f.apply(this, ..)` call `f` itself, `f.bind(this)` uses it as a value; the second
    /// result tells how the argument count shifts.
    fn callee(&mut self, call: Node<'t>, f: Node<'t>, c: &mut TreeCursor<'t>) -> (Option<Target>, i8) {
        let t = self.t;
        let text = self.cx.text(f);
        let simple = simple_member(text);
        let shift = match simple.map(|(_, prop)| prop).unwrap_or_else(|| trailing_word(text)) {
            b"call" => -1,
            b"apply" => 1,
            b"bind" => 2,
            _ => 0,
        };
        // `ns.call(..)`, `ns.bind(..)` on a module alias are functions of that module.
        let on_module = simple.is_some_and(|(word, _)| {
            let id = self.cx.intern(word);
            !self.cx.is_local(id) && matches!(self.binds.get(&id), Some(Bind::Namespace(_)) | Some(Bind::Default(_)))
        });
        if shift != 0 && !on_module && self.explicit_this(call) {
            match simple {
                Some((word, _)) if !matches!(word, b"this" | b"super") => {
                    let Some(name) = self.free_word(word, f.start_byte()) else { return (None, 0) };
                    if shift == 2 {
                        self.cx.add_ref(name, RefKind::Read, line_of(f), self.free_qual, 255);
                        return (None, 0);
                    }
                    return (Some((name, RefKind::Call, self.free_qual, line_of(f))), shift);
                }
                None => {
                    if let Some(o) = child(f, t.f_object).filter(|o| self.k(*o) == K::MemberExpression) {
                        let mut target = None;
                        if c.goto_first_child() {
                            target = self.member_ref(o, c, shift != 2, false);
                            c.goto_parent();
                        }
                        if shift == 2 {
                            if let Some((name, kind, qual, line)) = target {
                                self.cx.add_ref(name, kind, line, qual, 255);
                            }
                            return (None, 0);
                        }
                        return (target, shift);
                    }
                }
                _ => {}
            }
        }
        (self.member_ref(f, c, true, false), 0)
    }

    /// `f.call(this, ..)`: the first argument is the receiver, not an ordinary argument.
    fn explicit_this(&self, call: Node<'t>) -> bool {
        let Some(a) = child(call, self.t.f_arguments).and_then(|a| self.first_named(a)) else { return false };
        self.k(a) == K::This || matches!(self.cx.text(a), b"null" | b"undefined")
    }

    fn new_expr(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let mut target: Option<(u32, u32, u32)> = None;
        let mut nargs = 0u8;
        let mut seen = false;
        each_child!(self, n, c, |ch, k| {
            match k {
                K::Anon | K::Comment => {}
                K::Arguments => nargs = self.arguments(ch, c),
                // The constructor comes first.
                K::Identifier if !seen => {
                    seen = true;
                    if let Some(name) = self.free_name(ch) {
                        target = Some((name, self.free_qual, line_of(ch)));
                    } else if let Some((name, module)) = self.module_callee(ch) {
                        target = Some((name, module, line_of(ch)));
                    }
                }
                K::MemberExpression if !seen => {
                    seen = true;
                    if let Some((name, _, qual, line)) = self.member_ref(ch, c, true, false) {
                        target = Some((name, self.dotted(qual), line));
                    }
                }
                _ => {
                    seen = true;
                    self.node(ch, k, c);
                }
            }
        });
        if let Some((name, qual, line)) = target {
            self.cx.add_ref(name, RefKind::New, line, qual, nargs);
        }
    }

    /// A value that can be bound into a slot: a non-local function or variable, or a member of
    /// `this`. Returns its name and whether the value still has to be walked.
    fn bindable(&mut self, v: Node<'t>) -> Option<(u32, bool)> {
        match self.k(v) {
            K::Identifier => self.free_name(v).map(|name| (name, false)),
            K::MemberExpression => match simple_member(self.cx.text(v)) {
                Some((b"this", prop)) => Some((self.cx.intern(prop), true)),
                Some((b"super", _)) => None,
                // `Class.method`, `ns.func`: a static method or a function of a module. The
                // member expression keeps its own reference, which carries the owner.
                // Capitalised members are enum members and constants: values, not functions.
                Some((word, prop)) if prop[0].is_ascii_lowercase() || prop[0] == b'_' => match self.recv_word(word, 0) {
                    Recv::Type(_) | Recv::Ns(_) | Recv::Module(_) => Some((self.cx.intern(prop), true)),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        }
    }

    /// `f.bind(x)`, `a.m.bind(a)`: the name of the function such a call binds.
    fn bound_function(&mut self, v: Node<'t>) -> Option<u32> {
        let t = self.t;
        if self.k(v) != K::CallExpression {
            return None;
        }
        let f = child(v, t.f_function)?;
        if self.k(f) != K::MemberExpression {
            return None;
        }
        let (o, p) = (child(f, t.f_object)?, child(f, t.f_property)?);
        if self.cx.text(p) != b"bind" {
            return None;
        }
        match self.k(o) {
            K::Identifier => self.free_name(o),
            K::MemberExpression => {
                let m = child(o, t.f_property)?;
                if m.start_byte() == m.end_byte() { None } else { Some(self.cx.name(m)) }
            }
            _ => None,
        }
    }

    /// The type a bound value `Class.method` is a member of (`NONE` for a plain name).
    fn bound_owner(&mut self, v: Node<'t>) -> u32 {
        if self.k(v) != K::MemberExpression {
            return NONE;
        }
        match simple_member(self.cx.text(v)) {
            Some((word, _)) => match self.recv_word(word, 0) {
                Recv::Type(q) | Recv::Ns(q) => q,
                // A function of another module: nothing this file could confirm.
                _ => u32::MAX - 1,
            },
            None => u32::MAX - 1,
        }
    }

    fn assignment(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let (Some(left), Some(right)) = (child(n, t.f_left), child(n, t.f_right)) else { return self.children(n, c) };
        match self.k(left) {
            K::Identifier => {
                if self.cx.text(n).starts_with(b"using ") {
                    // `using x = ..`: a declaration, as far as this grammar goes an assignment.
                    self.declare_pattern(left, NONE, 0);
                } else {
                    self.identifier(left, RefKind::Write);
                }
                each_child!(self, n, c, |ch, k| {
                    if ch.id() != left.id() {
                        self.node(ch, k, c);
                    }
                });
            }
            K::MemberExpression => {
                if self.commonjs_export(n, left, right, c) || self.member_function(n, left, right, c) {
                    return;
                }
                let mut bound = false;
                each_child!(self, n, c, |ch, k| {
                    if ch.id() == left.id() {
                        if let Some((name, kind, qual, line)) = self.member_ref(left, c, false, true) {
                            self.cx.add_ref(name, kind, line, qual, 255);
                            if kind == RefKind::MemberWrite {
                                // `obj.cb = handler` binds a function (or value) into the slot.
                                if let Some((v, again)) = self.bindable(right) {
                                    let slot = self.slot(qual, name);
                                    self.cx.add_ref(v, RefKind::Bind, line_of(right), slot, 255);
                                    bound = !again;
                                }
                                self.ctor_field(left, right, name);
                            }
                        }
                    } else if !(bound && ch.id() == right.id()) {
                        self.node(ch, k, c);
                    }
                });
            }
            _ => self.children(n, c),
        }
    }

    /// `x += 1`, `x++`: the target is written.
    fn write_target(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(target) = child(n, t.f_left).or_else(|| child(n, t.f_argument)) else { return self.children(n, c) };
        let tk = self.k(target);
        if !matches!(tk, K::Identifier | K::MemberExpression) {
            return self.children(n, c);
        }
        each_child!(self, n, c, |ch, k| {
            if ch.id() != target.id() {
                self.node(ch, k, c);
            } else if tk == K::Identifier {
                self.identifier(ch, RefKind::Write);
            } else if let Some((name, kind, qual, line)) = self.member_ref(ch, c, false, true) {
                self.cx.add_ref(name, kind, line, qual, 255);
            }
        });
    }

    /// JavaScript declares fields by assignment: `this.x = ...` in a constructor defines `x`.
    fn ctor_field(&mut self, left: Node<'t>, right: Node<'t>, name: u32) {
        let Some((owner, depth)) = self.ctor_fields else { return };
        if depth != self.fn_depth || !matches!(simple_member(self.cx.text(left)), Some((b"this", _))) {
            return;
        }
        if self.cls_fields.contains(&name) {
            return;
        }
        self.cls_fields.push(name);
        let d = self.def_at(name, DefKind::Field, line_of(left), end_line_of(right), left.start_byte(), right.start_byte(), 0, owner);
        let ty = self.value_type(right, 0);
        self.cx.def_mut(d).type_name = ty;
    }

    /// `module.exports = ..`, `exports.name = ..`, `module.exports.name = ..`.
    fn commonjs_export(&mut self, n: Node<'t>, left: Node<'t>, right: Node<'t>, c: &mut TreeCursor<'t>) -> bool {
        let t = self.t;
        let text = self.cx.text(left);
        let (whole, name) = if text == b"module.exports" {
            (true, &b""[..])
        } else if let Some(name) = text.strip_prefix(b"module.exports.").or_else(|| text.strip_prefix(b"exports.")) {
            (false, name)
        } else {
            return false;
        };
        if !whole && (name.is_empty() || !name.iter().all(|&b| is_ident_byte(b))) {
            return false;
        }
        if self.fn_depth == 0 {
            self.is_module = true;
        }
        let rk = self.k(right);
        if whole {
            match rk {
                K::Identifier => {
                    if let Some(v) = self.free_name(right) {
                        self.exports.push(v);
                        if self.fn_depth == 0 {
                            let default = self.cx.intern(b"default");
                            self.export_alias(v, default, line_of(n));
                        }
                    }
                }
                K::Object => self.obj_ctx = Some(ObjCtx { id: right.id(), ty: NONE, owner: NONE, mode: ObjMode::Exports }),
                K::FunctionExpression | K::ArrowFunction | K::Class if self.fn_depth == 0 => {
                    // `module.exports = function name() {}` / `class Name {}`. Without a name
                    // the importer's binding is all there is: the definition is `default`.
                    let default = self.cx.intern(b"default");
                    let name = match child(right, t.f_name).filter(|x| x.start_byte() < x.end_byte()) {
                        Some(nm) if rk != K::ArrowFunction => self.cx.name(nm),
                        _ => default,
                    };
                    self.export_alias(name, default, line_of(n));
                    self.export_value(n, right, name, c);
                    return true;
                }
                K::FunctionExpression | K::Class => {
                    if let Some(nm) = child(right, t.f_name).filter(|x| x.start_byte() < x.end_byte()) {
                        let name = self.cx.name(nm);
                        self.export_value(n, right, name, c);
                        return true;
                    }
                }
                _ => {}
            }
            self.assigned_value(n, right, c);
            self.obj_ctx = None;
            return true;
        }
        let name = self.cx.intern(name);
        match rk {
            K::FunctionExpression | K::ArrowFunction | K::Class => self.export_value(n, right, name, c),
            K::Identifier => {
                // `exports.foo = foo`
                if let Some(v) = self.free_name(right) {
                    self.exports.push(v);
                }
                self.assigned_value(n, right, c);
            }
            _ => {
                if self.require_target(right).is_some() || self.fn_depth > 0 {
                    self.assigned_value(n, right, c);
                    return true;
                }
                let kind = if self.is_literal(right) { DefKind::Constant } else { DefKind::Variable };
                let ty = self.value_type(right, 0);
                let d = self.def_at(name, kind, line_of(n), end_line_of(n), n.start_byte(), right.start_byte(), dflag::EXPORTED, self.cx.encl());
                self.cx.def_mut(d).type_name = ty;
                self.cx.push_def(d);
                self.assigned_value(n, right, c);
                self.cx.pop_def();
            }
        }
        true
    }

    /// Visit only the value of assignment `n`.
    fn assigned_value(&mut self, n: Node<'t>, right: Node<'t>, c: &mut TreeCursor<'t>) {
        each_child!(self, n, c, |ch, k| {
            if ch.id() == right.id() {
                self.node(ch, k, c);
            }
        });
    }

    /// `exports.name = function () {}` / `class {}`: an exported definition named `name`.
    fn export_value(&mut self, n: Node<'t>, right: Node<'t>, name: u32, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let mut def = NONE;
        if self.k(right) != K::Class {
            let sig_end = child(right, t.f_body).map(|b| b.start_byte()).unwrap_or(n.end_byte());
            def = self.def_at(name, DefKind::Function, line_of(n), end_line_of(n), n.start_byte(), sig_end, dflag::EXPORTED, self.cx.encl());
        }
        each_child!(self, n, c, |ch, k| {
            if ch.id() == right.id() {
                match k {
                    K::Class => self.class(ch, k, c, n, dflag::EXPORTED, name),
                    K::ArrowFunction => {
                        self.function(ch, c, def, INHERIT, true);
                    }
                    _ => {
                        self.function(ch, c, def, NONE, true);
                    }
                }
            }
        });
    }

    /// `X.prototype.m = function () {}`, `X.m = function () {}`, `this.m = () => {}`: a method
    /// of `X` (of the enclosing class), defined by assignment.
    fn member_function(&mut self, n: Node<'t>, left: Node<'t>, right: Node<'t>, c: &mut TreeCursor<'t>) -> bool {
        let t = self.t;
        if !matches!(self.k(right), K::FunctionExpression | K::ArrowFunction) {
            return false;
        }
        let (Some(o), Some(p)) = (child(left, t.f_object), child(left, t.f_property)) else { return false };
        if p.start_byte() == p.end_byte() {
            return false;
        }
        let owner = match self.k(o) {
            // TypeScript declares its members; there this is an ordinary write.
            K::This if !self.ts => self.this_ty,
            K::Identifier => self.owner_name(o),
            K::MemberExpression => match (child(o, t.f_object), child(o, t.f_property)) {
                (Some(oo), Some(op)) if self.k(oo) == K::Identifier && self.cx.text(op) == b"prototype" => self.owner_name(oo),
                _ => NONE,
            },
            _ => NONE,
        };
        if owner == NONE {
            return false;
        }
        let name = self.cx.name(p);
        let sig_end = child(right, t.f_body).map(|b| b.start_byte()).unwrap_or(n.end_byte());
        let d = self.def_at(name, DefKind::Method, line_of(n), end_line_of(n), n.start_byte(), sig_end, 0, NONE);
        self.cx.def_mut(d).qual = owner;
        self.cls_fields.push(name);
        each_child!(self, n, c, |ch, k| {
            if ch.id() == right.id() {
                let mode = if k == K::ArrowFunction { INHERIT } else { owner };
                self.function(ch, c, d, mode, true);
            }
        });
        true
    }

    /// A non-local name that can own members defined by assignment.
    fn owner_name(&mut self, o: Node<'t>) -> u32 {
        let text = self.cx.text(o);
        if text.is_empty() {
            return NONE;
        }
        let name = self.cx.intern(text);
        if self.cx.is_local(name) {
            return NONE;
        }
        match self.binds.get(&name) {
            Some(Bind::Namespace(_)) | Some(Bind::Default(_)) => NONE,
            Some(Bind::Alias(orig, _)) => *orig,
            Some(_) => name,
            None if is_stop(text) => NONE,
            None => name,
        }
    }

    /// `x as T`, `x satisfies T`: an object literal behind the cast has type `T`.
    fn cast(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        if self.obj_ctx.is_none() {
            if let Some(e) = self.first_named(n).filter(|e| self.k(*e) == K::Object) {
                let ty = match n.named_child(1) {
                    Some(x) => self.type_name(x, 0),
                    None => NONE,
                };
                if ty != NONE {
                    self.obj_ctx = Some(ObjCtx { id: e.id(), ty, owner: NONE, mode: ObjMode::Plain });
                }
            }
        }
        self.children(n, c);
    }

    // ----- object literals -----

    fn object(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let (ty, owner, mode) = match self.obj_ctx.take() {
            Some(ctx) if ctx.id == n.id() => (ctx.ty, ctx.owner, ctx.mode),
            _ => (NONE, NONE, ObjMode::Plain),
        };
        // Keys of a typed literal are member writes; anything else that is plain data is inert.
        let plain = ty == NONE && mode == ObjMode::Plain;
        if plain && is_data(self.cx.text(n)) {
            return;
        }
        each_inner!(self, n, c, |ch, k| {
            match k {
                K::Pair => {
                    if !(plain && is_data(self.cx.text(ch))) {
                        self.pair(ch, c, ty, owner, mode);
                    }
                }
                K::MethodDefinition => {
                    let saved = (self.this_ty, self.super_q);
                    self.this_ty = owner;
                    self.super_q = NONE;
                    let d = self.method(ch, c, 0, NO_DECO);
                    (self.this_ty, self.super_q) = saved;
                    if d != NONE && mode == ObjMode::Exports {
                        self.export_member(d);
                    }
                }
                K::ShorthandProperty => self.shorthand(ch, ty, owner, mode),
                _ => self.node(ch, k, c),
            }
        });
    }

    /// A function in `module.exports = { .. }` is an exported function of the module.
    fn export_member(&mut self, d: u32) {
        let dm = self.cx.def_mut(d);
        if dm.kind == DefKind::Method {
            dm.kind = DefKind::Function;
        }
        dm.flags |= dflag::EXPORTED;
    }

    /// `key: value` of an object literal whose declared type is `ty` and whose name is `owner`.
    fn pair(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, ty: u32, owner: u32, mode: ObjMode) {
        let t = self.t;
        let (Some(key), Some(value)) = (child(n, t.f_key), child(n, t.f_value)) else { return self.children(n, c) };
        let vk = self.k(value);
        if let Some(f) = self.func_value(value) {
            let name = self.member_name(key);
            if name == NONE {
                return self.children(n, c);
            }
            let sig_end = child(f, t.f_body).map(|b| b.start_byte()).unwrap_or(n.end_byte());
            let d = self.def(name, DefKind::Method, n, sig_end, 0);
            if mode == ObjMode::Exports {
                self.export_member(d);
            }
            each_child!(self, n, c, |ch, k| {
                if ch.id() == value.id() {
                    self.wrapped_function(ch, f, c, d, owner);
                } else {
                    self.node(ch, k, c);
                }
            });
            return;
        }
        match vk {
            K::Identifier | K::MemberExpression => {
                let name = self.member_name(key);
                let slot_owner = if ty != NONE { ty } else { owner };
                let mut visit = true;
                match self.bindable(value).filter(|_| name != NONE) {
                    Some((v, _)) if mode == ObjMode::Exports && vk == K::Identifier => {
                        // `module.exports = { name: local }`
                        self.exports.push(v);
                    }
                    Some((v, again)) => {
                        self.cx.add_ref(name, RefKind::MemberWrite, line_of(key), slot_owner, 255);
                        let slot = self.slot(slot_owner, name);
                        self.cx.add_ref(v, RefKind::Bind, line_of(value), slot, 255);
                        visit = again;
                    }
                    None => {
                        if ty != NONE && name != NONE {
                            self.cx.add_ref(name, RefKind::MemberWrite, line_of(key), ty, 255);
                        }
                    }
                }
                if visit {
                    self.children(n, c);
                }
            }
            _ => {
                if ty != NONE {
                    let name = self.member_name(key);
                    if name != NONE {
                        self.cx.add_ref(name, RefKind::MemberWrite, line_of(key), ty, 255);
                        if vk == K::Object {
                            // Nested literal: its type is the type of this field.
                            let nested = self.join(ty, b"::", self.cx.name_bytes(name).to_vec().as_slice());
                            self.obj_ctx = Some(ObjCtx { id: value.id(), ty: nested, owner: NONE, mode: ObjMode::Plain });
                        }
                    }
                }
                self.children(n, c);
                self.obj_ctx = None;
            }
        }
    }

    /// `{ handler }`: the value of that name bound to the member of that name.
    fn shorthand(&mut self, n: Node<'t>, ty: u32, owner: u32, mode: ObjMode) {
        let Some(name) = self.free_name(n) else {
            if ty != NONE && n.start_byte() < n.end_byte() {
                let key = self.cx.name(n);
                self.cx.add_ref(key, RefKind::MemberWrite, line_of(n), ty, 255);
            }
            return;
        };
        if mode == ObjMode::Exports {
            self.exports.push(name);
            self.cx.add_ref(name, RefKind::Read, line_of(n), NONE, 255);
            return;
        }
        let key = self.cx.name(n);
        let slot_owner = if ty != NONE { ty } else { owner };
        self.cx.add_ref(key, RefKind::MemberWrite, line_of(n), slot_owner, 255);
        let slot = self.slot(slot_owner, key);
        self.cx.add_ref(name, RefKind::Bind, line_of(n), slot, 255);
    }

    // ----- decorators, JSX -----

    /// `@name`, `@name(args)`, `@ns.name(args)`.
    fn decorator(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        each_child!(self, n, c, |ch, k| {
            self.decorator_expr(ch, k, c, 0);
        });
    }

    fn decorator_expr(&mut self, e: Node<'t>, k: K, c: &mut TreeCursor<'t>, depth: u32) {
        match k {
            K::Identifier => self.identifier(e, RefKind::Annotation),
            K::MemberExpression => {
                if let Some((name, _, qual, line)) = self.member_target(e, true) {
                    let qual = self.dotted(qual);
                    self.cx.add_ref(name, RefKind::Annotation, line, qual, 255);
                }
            }
            K::CallExpression | K::ParenthesizedExpression if depth < 3 => {
                let mut first = true;
                each_child!(self, e, c, |ch, ck| {
                    if ck == K::Arguments {
                        self.arguments(ch, c);
                    } else if k == K::ParenthesizedExpression || first {
                        self.decorator_expr(ch, ck, c, depth + 1);
                    } else {
                        self.node(ch, ck, c);
                    }
                    first = false;
                });
            }
            _ => self.node(e, k, c),
        }
    }

    /// `<Name attr={..}>`: an element with a capitalised name instantiates a component.
    fn jsx_open(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let name = child(n, t.f_name);
        if let Some(nm) = name {
            match self.k(nm) {
                K::Identifier => {
                    if self.cx.text(nm).first().is_some_and(|b| b.is_ascii_uppercase()) {
                        self.identifier(nm, RefKind::New);
                    }
                }
                K::MemberExpression => {
                    if let Some((name, _, qual, line)) = self.member_target(nm, true) {
                        let qual = self.dotted(qual);
                        self.cx.add_ref(name, RefKind::New, line, qual, 255);
                    }
                }
                _ => {}
            }
        }
        each_child!(self, n, c, |ch, k| {
            if name.is_none_or(|x| x.id() != ch.id()) {
                self.node(ch, k, c);
            }
        });
    }

    /// `onClick={handler}` binds `handler` into the slot `onClick`.
    fn jsx_attribute(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let mut key = None;
        let mut value = None;
        let mut i = 0;
        while let Some(x) = n.named_child(i) {
            i += 1;
            match self.k(x) {
                K::PropertyIdentifier => key = Some(x),
                K::JsxExpression => value = self.first_named(x),
                _ => {}
            }
        }
        if let (Some(k), Some(v)) = (key, value) {
            if let Some((vn, again)) = self.bindable(v) {
                let kn = self.cx.name(k);
                self.cx.add_ref(kn, RefKind::MemberWrite, line_of(k), NONE, 255);
                let slot = self.slot(NONE, kn);
                self.cx.add_ref(vn, RefKind::Bind, line_of(v), slot, 255);
                if !again {
                    return;
                }
            }
        }
        self.children(n, c);
    }

    // ----- imports and exports -----

    fn import_statement(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        if self.ns_depth == 0 {
            self.is_module = true;
        }
        let line = line_of(n);
        // `import x = require('m')` keeps the path inside its clause.
        let source = child(n, t.f_source).or_else(|| {
            let mut i = 0;
            while let Some(x) = n.named_child(i) {
                i += 1;
                if self.k(x) == K::ImportRequireClause {
                    return child(x, t.f_source);
                }
            }
            None
        });
        let Some(source) = source else { return };
        let target_text = self.string_content(source);
        let target = self.cx.intern(target_text);
        let flags = import_flags(target_text);
        let mut bindings = false;
        each_child!(self, n, c, |clause, ck| {
            if matches!(ck, K::ImportClause | K::ImportRequireClause) {
                bindings = true;
                each_child!(self, clause, c, |ch, k| {
                    match k {
                        K::Identifier if ch.start_byte() < ch.end_byte() => {
                            let local = self.cx.name(ch);
                            if ck == K::ImportRequireClause {
                                // `import fs = require('fs')`
                                self.cx.add_import(target, NONE, local, line, flags);
                                self.binds.insert(local, Bind::Namespace(target));
                                self.has_alias = true;
                            } else {
                                self.default_import(target, local, line_of(ch), flags);
                            }
                        }
                        K::NamespaceImport => {
                            if let Some(id) = self.first_named(ch).filter(|x| x.start_byte() < x.end_byte()) {
                                let local = self.cx.name(id);
                                self.cx.add_import(target, NONE, local, line, flags);
                                self.binds.insert(local, Bind::Namespace(target));
                                self.has_alias = true;
                            }
                        }
                        K::NamedImports => {
                            each_inner!(self, ch, c, |spec, sk| {
                                if sk == K::ImportSpecifier {
                                    self.import_specifier(spec, target, flags);
                                }
                            });
                        }
                        _ => {}
                    }
                });
            }
        });
        if !bindings {
            // `import './side-effect'`
            self.cx.add_import(target, NONE, NONE, line, flags);
        }
    }

    /// `import local from 'm'`. The exported name is `default`; references go by the local
    /// name, which by convention is the name of what the module exports.
    fn default_import(&mut self, target: u32, local: u32, line: u32, flags: u8) {
        let default = self.cx.intern(b"default");
        self.cx.add_import(target, default, local, line, flags);
        self.cx.add_ref(local, RefKind::Import, line, target, 255);
        self.binds.insert(local, Bind::Default(target));
        self.has_alias = true;
    }

    fn import_specifier(&mut self, spec: Node<'t>, target: u32, flags: u8) {
        let t = self.t;
        let Some(nm) = child(spec, t.f_name) else { return };
        let sym = self.member_name(nm);
        if sym == NONE {
            return;
        }
        let local = match child(spec, t.f_alias) {
            Some(a) if a.start_byte() < a.end_byte() => self.cx.name(a),
            _ => sym,
        };
        let line = line_of(spec);
        if self.cx.name_bytes(sym) == b"default" {
            return self.default_import(target, local, line, flags);
        }
        self.cx.add_import(target, sym, if local == sym { NONE } else { local }, line, flags);
        self.cx.add_ref(sym, RefKind::Import, line, target, 255);
        if local == sym {
            self.binds.insert(local, Bind::Sym);
        } else {
            self.binds.insert(local, Bind::Alias(sym, target));
            self.has_alias = true;
        }
    }

    /// `import A = N.M.T;`: a local alias of another entity.
    fn import_alias(&mut self, n: Node<'t>, span: Node<'t>, flags: u8) {
        let t = self.t;
        let mut name = None;
        let mut target = None;
        let mut i = 0;
        while let Some(x) = n.named_child(i) {
            i += 1;
            match self.k(x) {
                K::Identifier if name.is_none() => name = Some(x),
                K::Identifier | K::NestedIdentifier => target = Some(x),
                _ => {}
            }
        }
        let (Some(nm), Some(tg)) = (name, target) else { return };
        if nm.start_byte() == nm.end_byte() {
            return;
        }
        let name = self.cx.name(nm);
        let fl = self.decl_flags(flags);
        let d = self.def(name, DefKind::TypeAlias, span, n.end_byte(), fl);
        self.register(d);
        let (sym, qual) = match self.k(tg) {
            K::NestedIdentifier => match (child(tg, t.f_property), child(tg, t.f_object)) {
                (Some(p), Some(o)) => (self.cx.name(p), self.qualifier(o, 0)),
                _ => (NONE, NONE),
            },
            _ => (self.cx.name(tg), NONE),
        };
        if sym != NONE {
            let ty = if qual == NONE { sym } else { self.module_qualified(qual, self.cx.text(tg).rsplit(|&b| b == b'.').next().unwrap_or(b"")) };
            self.cx.def_mut(d).type_name = ty;
            self.cx.push_def(d);
            self.cx.add_ref(sym, RefKind::Type, line_of(tg), qual, 255);
            self.cx.pop_def();
        }
    }

    fn export_statement(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        if self.ns_depth == 0 && self.local_depth == 0 {
            self.is_module = true;
        }
        let decl = child(n, t.f_declaration);
        let source = child(n, t.f_source);
        let line = line_of(n);
        let reexport = source.map(|s| {
            let text = self.string_content(s);
            (self.cx.intern(text), import_flags(text) | iflag::REEXPORT)
        });
        let mut wildcard = reexport.is_some();
        let mut assigned = false;
        let mut is_default = false;
        // Decorators in front of `export` belong to the declaration that follows.
        let mut deco = u32::MAX;
        each_child!(self, n, c, |ch, k| {
            if decl.is_some_and(|d| d.id() == ch.id()) {
                let first = self.cx.facts.defs.len() as u32;
                let deco_end = self.cx.facts.refs.len();
                self.declaration(ch, k, c, n, dflag::EXPORTED);
                if is_default && (self.cx.facts.defs.len() as u32) > first {
                    // `export default function name() {}`: importers know it as `default`.
                    let name = self.cx.facts.defs[first as usize].name;
                    let default = self.cx.intern(b"default");
                    self.export_alias(name, default, line);
                }
                if deco != u32::MAX && (self.cx.facts.defs.len() as u32) > first {
                    let outer = self.cx.encl();
                    for r in &mut self.cx.facts.refs[deco as usize..deco_end] {
                        if r.encl == outer {
                            r.encl = first;
                        }
                    }
                }
            } else if k == K::Decorator {
                if deco == u32::MAX {
                    deco = self.cx.facts.refs.len() as u32;
                }
                self.decorator(ch, c);
            } else if k == K::ExportClause {
                wildcard = false;
                each_inner!(self, ch, c, |spec, sk| {
                    if sk == K::ExportSpecifier {
                        self.export_specifier(spec, reexport);
                    }
                });
            } else if k == K::NamespaceExport {
                // `export * as ns from './m'`
                wildcard = false;
                if let (Some((target, flags)), Some(id)) = (reexport, self.first_named(ch)) {
                    let alias = self.member_name(id);
                    self.cx.add_import(target, NONE, alias, line, flags);
                }
            } else if k == K::Anon {
                match self.tk(ch) {
                    Tk::Eq => assigned = true,
                    // `export as namespace Lib;` names a global, it exports no symbol.
                    Tk::Namespace => assigned = false,
                    _ => {
                        if self.cx.text(ch) == b"default" {
                            is_default = true;
                        }
                    }
                }
            } else if is_field(c, t.f_value) {
                self.export_default(ch, k, c, n);
            } else if k == K::Identifier {
                // `export = name;`
                if assigned {
                    if let Some(name) = self.free_name(ch) {
                        self.exports.push(name);
                        self.cx.add_ref(name, RefKind::Read, line_of(ch), NONE, 255);
                        let default = self.cx.intern(b"default");
                        self.export_alias(name, default, line_of(ch));
                    }
                }
            } else if source.is_none_or(|s| s.id() != ch.id()) {
                self.node(ch, k, c);
            }
        });
        if let (true, Some((target, flags))) = (wildcard, reexport) {
            // `export * from './m'`
            self.cx.add_import(target, NONE, NONE, line, flags | iflag::WILDCARD);
        }
    }

    /// This module exports its own `name` under another name (`export { a as b }`,
    /// `export default a`, `module.exports = a`). Recorded as a re-export with an empty
    /// target: the name is looked up in this very file.
    fn export_alias(&mut self, name: u32, alias: u32, line: u32) {
        if name == NONE || alias == NONE || name == alias {
            return;
        }
        let this = self.cx.intern(b"");
        self.cx.add_import(this, name, alias, line, iflag::REEXPORT);
    }

    /// `const z = { ...nsA, ...nsB }` at module level: `z.f` is `nsA.f` or `nsB.f`. Each
    /// spread namespace is recorded as a namespace import under the object's own name.
    fn spread_namespaces(&mut self, obj: Node<'t>, name: u32, line: u32) {
        let mut cur = obj.walk();
        for ch in obj.named_children(&mut cur) {
            if self.k(ch) != K::SpreadElement {
                continue;
            }
            let Some(arg) = ch.named_child(0).filter(|a| self.k(*a) == K::Identifier) else { continue };
            let id = self.cx.name(arg);
            if let Some(Bind::Namespace(target)) = self.binds.get(&id).copied() {
                let flags = import_flags(self.cx.name_bytes(target));
                self.cx.add_import(target, NONE, name, line, flags);
            }
        }
    }

    /// `a` or `b as c` in `export { a, b as c }` (names of this file) or in
    /// `export { a, b as c } from './m'`.
    fn export_specifier(&mut self, spec: Node<'t>, reexport: Option<(u32, u8)>) {
        let t = self.t;
        let Some(nm) = child(spec, t.f_name) else { return };
        let sym = self.member_name(nm);
        if sym == NONE {
            return;
        }
        match reexport {
            Some((target, flags)) => {
                let alias = match child(spec, t.f_alias) {
                    Some(a) => self.member_name(a),
                    None => NONE,
                };
                self.cx.add_import(target, sym, if alias == sym { NONE } else { alias }, line_of(spec), flags);
                self.cx.add_ref(sym, RefKind::Import, line_of(spec), target, 255);
            }
            None => {
                let name = self.canon(sym).unwrap_or(sym);
                self.exports.push(name);
                if let Some(a) = child(spec, t.f_alias) {
                    let alias = self.member_name(a);
                    self.export_alias(name, alias, line_of(spec));
                }
            }
        }
    }

    /// `export default <expression>`.
    fn export_default(&mut self, v: Node<'t>, k: K, c: &mut TreeCursor<'t>, span: Node<'t>) {
        let t = self.t;
        match k {
            K::Identifier => {
                if let Some(name) = self.free_name(v) {
                    self.exports.push(name);
                    self.cx.add_ref(name, RefKind::Read, line_of(v), NONE, 255);
                    let default = self.cx.intern(b"default");
                    self.export_alias(name, default, line_of(v));
                }
            }
            K::FunctionExpression | K::ArrowFunction => {
                // Anonymous default export: importers know it as `default`.
                let name = self.cx.intern(b"default");
                let sig_end = child(v, t.f_body).map(|b| b.start_byte()).unwrap_or(v.end_byte());
                let d = self.def(name, DefKind::Function, span, sig_end, dflag::EXPORTED);
                self.register(d);
                self.function(v, c, d, if k == K::ArrowFunction { INHERIT } else { NONE }, true);
            }
            K::Class => {
                let name = match child(v, t.f_name).filter(|x| x.start_byte() < x.end_byte()) {
                    Some(x) => self.cx.name(x),
                    None => self.cx.intern(b"default"),
                };
                let default = self.cx.intern(b"default");
                self.export_alias(name, default, line_of(v));
                self.class(v, k, c, span, dflag::EXPORTED, name);
            }
            _ => self.node(v, k, c),
        }
    }

    fn finish(&mut self) {
        self.mark_function_slots();
        if self.toplevel.is_empty() {
            return;
        }
        self.exports.sort_unstable();
        // A script shares its top level with every other script; a module does not.
        let module = self.is_module && !self.cx.is_header;
        for &i in &self.toplevel {
            let d = &mut self.cx.facts.defs[i as usize];
            if self.exports.binary_search(&d.name).is_ok() {
                d.flags |= dflag::EXPORTED;
            }
            if module && d.flags & dflag::EXPORTED == 0 {
                d.flags |= dflag::LOCAL;
            }
        }
    }
}

impl<'a, 't> W<'a, 't> {
    /// `const f = handler`, `static create = makeThing`, `const f = Class.method`: when the
    /// value is a function or method this file defines, the variable or field holds a callable.
    fn mark_function_slots(&mut self) {
        if self.slot_values.is_empty() {
            return;
        }
        let slots = std::mem::take(&mut self.slot_values);
        let defs = &mut self.cx.facts.defs;
        // (name, owner type or `NONE`) of every module-level function and every method.
        let mut callables: Vec<(u32, u32)> = Vec::new();
        for d in defs.iter() {
            match d.kind {
                DefKind::Function if d.parent == NONE => callables.push((d.name, NONE)),
                DefKind::Method | DefKind::Function if d.parent != NONE => callables.push((d.name, defs[d.parent as usize].name)),
                _ => {}
            }
        }
        callables.sort_unstable();
        for (slot, value, owner) in slots {
            if callables.binary_search(&(value, owner)).is_ok() {
                defs[slot as usize].flags |= dflag::FNPTR;
            }
        }
    }
}

/// Imports of packages, as far as the specifier shows it: `node:fs`, `react`, `@scope/pkg`.
/// Relative paths and specifiers with directories (`src/util`, `vs/base/common/event`) may
/// equally be path aliases of the repository.
fn import_flags(target: &[u8]) -> u8 {
    if target.starts_with(b"node:") {
        return iflag::SYSTEM;
    }
    match target.first() {
        None | Some(b'.') | Some(b'/') | Some(b'~') | Some(b'#') => 0,
        Some(b'@') => {
            if target.get(1) == Some(&b'/') {
                0
            } else {
                iflag::SYSTEM
            }
        }
        _ => {
            if memchr::memchr(b'/', target).is_none() {
                iflag::SYSTEM
            } else {
                0
            }
        }
    }
}

fn is_all_caps(name: &[u8]) -> bool {
    let mut upper = false;
    for &b in name {
        match b {
            b'A'..=b'Z' => upper = true,
            b'0'..=b'9' | b'_' | b'$' => {}
            _ => return false,
        }
    }
    upper
}

/// Capitalised names denote types and namespaces, except `SCREAMING_CASE` constants.
fn is_type_like(name: &[u8]) -> bool {
    name.first().is_some_and(|b| b.is_ascii_uppercase()) && !(name.contains(&b'_') && is_all_caps(name))
}

#[inline]
/// A string key that can name a member (`'content-type'`, `'~validate'`, `'editor.fontSize'`):
/// not a sentence (`'a failed load': () => ..` in a table of test cases) nor a selector
/// (`'CallExpression[callee.name="x"]'`), which nothing can refer to by name.
fn is_member_string(s: &[u8]) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s
            .iter()
            .any(|&b| b <= b' ' || matches!(b, b'[' | b']' | b'(' | b')' | b'{' | b'}' | b'"' | b'\'' | b'`' | b'=' | b',' | b';' | b'<' | b'>' | b'|' | b'\\'))
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// `word.prop`, `word?.prop`: the two parts of a member expression whose object is a bare word
/// (`this`, `super` or an identifier), recognised from its text alone.
fn simple_member(text: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut i = 0;
    while i < text.len() && is_ident_byte(text[i]) {
        i += 1;
    }
    if i == 0 || text[0].is_ascii_digit() {
        return None;
    }
    let rest = &text[i..];
    let prop = match rest.first() {
        Some(b'.') => &rest[1..],
        Some(b'?') if rest.get(1) == Some(&b'.') => &rest[2..],
        _ => return None,
    };
    let bare = prop.strip_prefix(b"#").unwrap_or(prop);
    if bare.is_empty() || !bare.iter().all(|&b| is_ident_byte(b)) {
        return None;
    }
    Some((&text[..i], prop))
}

/// Text of an object literal, an array literal or a `key: value` pair that holds nothing but
/// literals, the way JSON does: there is nothing in it to walk for. Strings, numbers, `true`,
/// `false`, `null`, `undefined`, signs and brackets pass; a word passes as a key only, that is
/// when a colon follows; comments, template strings and everything else do not pass.
fn is_data(text: &[u8]) -> bool {
    let n = text.len();
    let mut i = 0;
    while i < n {
        let b = text[i];
        match b {
            b' ' | b'\t' | b'\n' | b'\r' | b'{' | b'}' | b'[' | b']' | b'(' | b')' | b',' | b':' | b'-' | b'+' | b'.' => i += 1,
            b'\'' | b'"' => {
                i += 1;
                loop {
                    match text.get(i) {
                        None | Some(b'\n') => return false,
                        Some(b'\\') => i += 2,
                        Some(&x) if x == b => break,
                        Some(_) => i += 1,
                    }
                }
                i += 1;
            }
            b'0'..=b'9' => {
                i += 1;
                while i < n && (text[i].is_ascii_alphanumeric() || text[i] == b'_' || text[i] == b'.') {
                    i += 1;
                }
            }
            _ if is_ident_byte(b) => {
                let start = i;
                while i < n && is_ident_byte(text[i]) {
                    i += 1;
                }
                if !matches!(&text[start..i], b"true" | b"false" | b"null" | b"undefined") {
                    let mut j = i;
                    while j < n && (text[j] == b' ' || text[j] == b'\t') {
                        j += 1;
                    }
                    if text.get(j) != Some(&b':') {
                        return false;
                    }
                }
            }
            _ => return false,
        }
    }
    true
}

/// The identifier a text ends with.
fn trailing_word(text: &[u8]) -> &[u8] {
    let mut i = text.len();
    while i > 0 && is_ident_byte(text[i - 1]) {
        i -= 1;
    }
    &text[i..]
}

/// `: Word`: the single word a type annotation consists of, when it is that simple.
fn simple_annotation(text: &[u8]) -> Option<&[u8]> {
    let rest = text.strip_prefix(b":")?;
    let mut i = 0;
    while i < rest.len() && (rest[i] == b' ' || rest[i] == b'\t') {
        i += 1;
    }
    let word = &rest[i..];
    if word.is_empty() || word[0].is_ascii_digit() || !word.iter().all(|&b| is_ident_byte(b)) {
        return None;
    }
    Some(word)
}

/// Types without members of their own.
fn is_primitive(w: &[u8]) -> bool {
    matches!(
        w,
        b"string"
            | b"number"
            | b"boolean"
            | b"any"
            | b"unknown"
            | b"never"
            | b"void"
            | b"object"
            | b"symbol"
            | b"bigint"
            | b"undefined"
            | b"null"
            | b"true"
            | b"false"
    )
}

/// Utility types that compute a type instead of naming one.
fn is_type_operator(w: &[u8]) -> bool {
    matches!(
        w,
        b"Record"
            | b"ReturnType"
            | b"Parameters"
            | b"Awaited"
            | b"Exclude"
            | b"Extract"
            | b"InstanceType"
            | b"ConstructorParameters"
            | b"ThisParameterType"
            | b"OmitThisParameter"
            | b"ThisType"
            | b"Uppercase"
            | b"Lowercase"
            | b"Capitalize"
            | b"Uncapitalize"
            | b"NoInfer"
    )
}

/// Globals of the language and of its standard type library, plus the host globals every
/// program uses: never symbols of the repository, unless a file imports or declares the name.
fn is_stop(w: &[u8]) -> bool {
    match w.len() {
        3 => matches!(w, b"Map" | b"Set" | b"NaN" | b"any"),
        4 => matches!(w, b"JSON" | b"Math" | b"Date" | b"void" | b"Pick" | b"Omit" | b"Intl" | b"eval" | b"null" | b"true" | b"this"),
        5 => matches!(w, b"Array" | b"Error" | b"never" | b"Proxy" | b"false" | b"isNaN" | b"super"),
        6 => matches!(
            w,
            b"Object"
                | b"String"
                | b"Number"
                | b"Symbol"
                | b"RegExp"
                | b"window"
                | b"module"
                | b"string"
                | b"number"
                | b"object"
                | b"Record"
                | b"BigInt"
                | b"bigint"
                | b"symbol"
        ),
        7 => matches!(
            w,
            b"console"
                | b"Promise"
                | b"Boolean"
                | b"WeakMap"
                | b"WeakSet"
                | b"process"
                | b"require"
                | b"exports"
                | b"boolean"
                | b"unknown"
                | b"Partial"
                | b"Exclude"
                | b"Extract"
                | b"Awaited"
                | b"Reflect"
                | b"Atomics"
                | b"WeakRef"
                | b"NoInfer"
        ),
        8 => matches!(w, b"Infinity" | b"document" | b"parseInt" | b"Readonly" | b"Required" | b"Function" | b"DataView" | b"isFinite" | b"ThisType"),
        9 => matches!(
            w,
            b"undefined"
                | b"arguments"
                | b"TypeError"
                | b"EvalError"
                | b"URIError"
                | b"encodeURI"
                | b"decodeURI"
                | b"Uppercase"
                | b"Lowercase"
                | b"ArrayLike"
                | b"Int8Array"
        ),
        10 => matches!(
            w,
            b"globalThis"
                | b"parseFloat"
                | b"setTimeout"
                | b"ReturnType"
                | b"Parameters"
                | b"RangeError"
                | b"Uint8Array"
                | b"Int16Array"
                | b"Int32Array"
                | b"Capitalize"
        ),
        11 => matches!(
            w,
            b"setInterval"
                | b"SyntaxError"
                | b"ArrayBuffer"
                | b"Uint16Array"
                | b"Uint32Array"
                | b"NonNullable"
                | b"PromiseLike"
                | b"PropertyKey"
                | b"ReadonlyMap"
                | b"ReadonlySet"
        ),
        12 => matches!(w, b"clearTimeout" | b"Float32Array" | b"Float64Array" | b"InstanceType" | b"Uncapitalize" | b"setImmediate"),
        13 => matches!(w, b"clearInterval" | b"BigInt64Array" | b"ReadonlyArray"),
        14 => matches!(w, b"ReferenceError" | b"AggregateError" | b"BigUint64Array" | b"queueMicrotask" | b"clearImmediate"),
        15 => matches!(w, b"structuredClone"),
        17 => matches!(w, b"Uint8ClampedArray" | b"SharedArrayBuffer" | b"ThisParameterType" | b"OmitThisParameter"),
        18 => matches!(w, b"encodeURIComponent" | b"decodeURIComponent"),
        20 => matches!(w, b"FinalizationRegistry" | b"TemplateStringsArray"),
        21 => matches!(w, b"ConstructorParameters"),
        _ => false,
    }
}
