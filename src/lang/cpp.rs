//! C++ extractor (tree-sitter-cpp).
//!
//! The grammar is a superset of the C grammar, and so is this extractor: everything the C
//! extractor understands (macros with a lexical scan of their bodies, operation tables bound
//! through designated initialisers, calls dispatched through typed member paths, functions
//! defined through macros) works here too. On top of that it models what makes C++ hard to
//! navigate with text search:
//!   * namespaces, classes and their members, with out-of-line `Class::method` definitions
//!     linked to their class by name,
//!   * templates (the templated entity is the definition, its parameters are local types),
//!   * the implicit `this`: unqualified calls and member accesses inside member functions,
//!   * receiver types of locals (`Foo x`, `auto p = new Foo`, `std::unique_ptr<Foo> p`, casts),
//!   * annotation macros the grammar cannot parse (`class LLVM_ABI Foo : Base {`,
//!     `LLVM_ABI StringRef name() const LLVM_READONLY;`): they are blanked out in a copy of the
//!     source, which is parsed again, so that the declarations behind them are seen intact.

use crate::extract::*;
use crate::facts::*;
use crate::lang::Lang;
use rustc_hash::FxHashSet;
use std::cell::RefCell;
use std::sync::OnceLock;
use tree_sitter::{Node, Parser, Tree, TreeCursor};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    Other,
    Skip,
    Identifier,
    TypeIdentifier,
    FieldIdentifier,
    NamespaceIdentifier,
    FunctionDefinition,
    Declaration,
    FieldDeclaration,
    TypeDefinition,
    AliasDeclaration,
    StructSpecifier,
    UnionSpecifier,
    ClassSpecifier,
    EnumSpecifier,
    Enumerator,
    NamespaceDefinition,
    NamespaceAlias,
    NestedNamespace,
    UsingDeclaration,
    TemplateDeclaration,
    TemplateParameterList,
    TypeParameter,
    OptionalTypeParameter,
    ConceptDefinition,
    FriendDeclaration,
    BaseClassClause,
    FieldInitializer,
    FieldInitializerList,
    PreprocDef,
    PreprocFunctionDef,
    PreprocInclude,
    PreprocIf,
    CallExpression,
    FieldExpression,
    AssignmentExpression,
    UpdateExpression,
    InitializerList,
    InitializerPair,
    ExpressionStatement,
    CompoundStatement,
    Error,
    ParameterDeclaration,
    OptionalParameterDeclaration,
    VariadicParameterDeclaration,
    CompoundLiteral,
    NewExpression,
    LambdaExpression,
    LambdaCaptureInit,
    ForRangeLoop,
    ForStatement,
    CatchClause,
    RequiresExpression,
    OffsetofExpression,
    QualifiedIdentifier,
    TemplateType,
    TemplateFunction,
    TemplateMethod,
    DependentName,
    DependentType,
    DestructorName,
    OperatorName,
    OperatorCast,
    FunctionDeclarator,
    AbstractFunctionDeclarator,
    AbstractPointerDeclarator,
    PointerDeclarator,
    ReferenceDeclarator,
    ParenthesizedDeclarator,
    ArrayDeclarator,
    AttributedDeclarator,
    InitDeclarator,
    StructuredBinding,
    VariadicDeclarator,
    ParenthesizedExpression,
    PointerExpression,
    SubscriptExpression,
    CastExpression,
    FieldDesignator,
    StorageClass,
    TypeQualifier,
    TypeDescriptor,
    Comment,
    This,
    PlaceholderType,
    ArgumentList,
    PureVirtualClause,
    DefaultMethodClause,
    DeleteMethodClause,
    TrailingReturnType,
    NumberLiteral,
}

struct Tables {
    /// Kind per symbol id. Anonymous tokens (`(`, `;`, keywords) are `Skip`: they are leaves,
    /// and they are half of all nodes.
    kinds: Vec<K>,
    f_declarator: u16,
    f_type: u16,
    f_body: u16,
    f_name: u16,
    f_parameters: u16,
    f_function: u16,
    f_arguments: u16,
    f_argument: u16,
    f_field: u16,
    f_left: u16,
    f_right: u16,
    f_value: u16,
    f_designator: u16,
    f_path: u16,
    f_operator: u16,
    f_scope: u16,
    f_default_value: u16,
    f_captures: u16,
    f_member: u16,
    f_condition: u16,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let lang: tree_sitter::Language = tree_sitter_cpp::LANGUAGE.into();
        let kinds = KindTable::build(
            &lang,
            K::Other,
            &[
                ("identifier", K::Identifier),
                ("type_identifier", K::TypeIdentifier),
                ("field_identifier", K::FieldIdentifier),
                ("namespace_identifier", K::NamespaceIdentifier),
                ("function_definition", K::FunctionDefinition),
                ("declaration", K::Declaration),
                ("field_declaration", K::FieldDeclaration),
                ("type_definition", K::TypeDefinition),
                ("alias_declaration", K::AliasDeclaration),
                ("struct_specifier", K::StructSpecifier),
                ("union_specifier", K::UnionSpecifier),
                ("class_specifier", K::ClassSpecifier),
                ("enum_specifier", K::EnumSpecifier),
                ("enumerator", K::Enumerator),
                ("namespace_definition", K::NamespaceDefinition),
                ("namespace_alias_definition", K::NamespaceAlias),
                ("nested_namespace_specifier", K::NestedNamespace),
                ("using_declaration", K::UsingDeclaration),
                ("template_declaration", K::TemplateDeclaration),
                ("template_parameter_list", K::TemplateParameterList),
                ("type_parameter_declaration", K::TypeParameter),
                ("variadic_type_parameter_declaration", K::TypeParameter),
                ("optional_type_parameter_declaration", K::OptionalTypeParameter),
                ("concept_definition", K::ConceptDefinition),
                ("friend_declaration", K::FriendDeclaration),
                ("base_class_clause", K::BaseClassClause),
                ("field_initializer", K::FieldInitializer),
                ("field_initializer_list", K::FieldInitializerList),
                ("preproc_def", K::PreprocDef),
                ("preproc_function_def", K::PreprocFunctionDef),
                ("preproc_include", K::PreprocInclude),
                ("preproc_if", K::PreprocIf),
                ("preproc_ifdef", K::PreprocIf),
                ("preproc_elif", K::PreprocIf),
                ("preproc_elifdef", K::PreprocIf),
                ("call_expression", K::CallExpression),
                ("field_expression", K::FieldExpression),
                ("assignment_expression", K::AssignmentExpression),
                ("update_expression", K::UpdateExpression),
                ("initializer_list", K::InitializerList),
                ("initializer_pair", K::InitializerPair),
                ("expression_statement", K::ExpressionStatement),
                ("compound_statement", K::CompoundStatement),
                ("ERROR", K::Error),
                ("parameter_declaration", K::ParameterDeclaration),
                ("optional_parameter_declaration", K::OptionalParameterDeclaration),
                ("variadic_parameter_declaration", K::VariadicParameterDeclaration),
                ("compound_literal_expression", K::CompoundLiteral),
                ("new_expression", K::NewExpression),
                ("lambda_expression", K::LambdaExpression),
                ("lambda_capture_initializer", K::LambdaCaptureInit),
                ("for_range_loop", K::ForRangeLoop),
                ("for_statement", K::ForStatement),
                ("catch_clause", K::CatchClause),
                ("requires_expression", K::RequiresExpression),
                ("offsetof_expression", K::OffsetofExpression),
                ("qualified_identifier", K::QualifiedIdentifier),
                ("template_type", K::TemplateType),
                ("template_function", K::TemplateFunction),
                ("template_method", K::TemplateMethod),
                ("dependent_name", K::DependentName),
                ("dependent_type", K::DependentType),
                ("destructor_name", K::DestructorName),
                ("operator_name", K::OperatorName),
                ("operator_cast", K::OperatorCast),
                ("function_declarator", K::FunctionDeclarator),
                ("abstract_function_declarator", K::AbstractFunctionDeclarator),
                ("abstract_pointer_declarator", K::AbstractPointerDeclarator),
                ("pointer_declarator", K::PointerDeclarator),
                ("pointer_type_declarator", K::PointerDeclarator),
                ("reference_declarator", K::ReferenceDeclarator),
                ("parenthesized_declarator", K::ParenthesizedDeclarator),
                ("array_declarator", K::ArrayDeclarator),
                ("attributed_declarator", K::AttributedDeclarator),
                ("init_declarator", K::InitDeclarator),
                ("structured_binding_declarator", K::StructuredBinding),
                ("variadic_declarator", K::VariadicDeclarator),
                ("parenthesized_expression", K::ParenthesizedExpression),
                ("pointer_expression", K::PointerExpression),
                ("subscript_expression", K::SubscriptExpression),
                ("cast_expression", K::CastExpression),
                ("field_designator", K::FieldDesignator),
                ("storage_class_specifier", K::StorageClass),
                ("type_qualifier", K::TypeQualifier),
                ("type_descriptor", K::TypeDescriptor),
                ("comment", K::Comment),
                ("this", K::This),
                ("placeholder_type_specifier", K::PlaceholderType),
                ("argument_list", K::ArgumentList),
                ("pure_virtual_clause", K::PureVirtualClause),
                ("default_method_clause", K::DefaultMethodClause),
                ("delete_method_clause", K::DeleteMethodClause),
                ("trailing_return_type", K::TrailingReturnType),
                ("number_literal", K::NumberLiteral),
                // leaves and subtrees that never contain references
                ("string_literal", K::Skip),
                ("raw_string_literal", K::Skip),
                ("char_literal", K::Skip),
                ("user_defined_literal", K::Skip),
                ("system_lib_string", K::Skip),
                ("preproc_arg", K::Skip),
                ("preproc_call", K::Skip),
                ("preproc_params", K::Skip),
                ("preproc_directive", K::Skip),
                ("attribute_specifier", K::Skip),
                ("attribute_declaration", K::Skip),
                ("ms_declspec_modifier", K::Skip),
                ("ms_call_modifier", K::Skip),
                ("ms_pointer_modifier", K::Skip),
                ("gnu_asm_expression", K::Skip),
                ("statement_identifier", K::Skip),
                ("primitive_type", K::Skip),
                ("sized_type_specifier", K::Skip),
                ("access_specifier", K::Skip),
                ("virtual_specifier", K::Skip),
                ("ref_qualifier", K::Skip),
                ("lambda_default_capture", K::Skip),
                ("auto", K::Skip),
                ("true", K::Skip),
                ("false", K::Skip),
                ("null", K::Skip),
            ],
        );
        let kinds: Vec<K> = (0..lang.node_kind_count()).map(|id| if lang.node_kind_is_named(id as u16) { kinds.get(id as u16) } else { K::Skip }).collect();
        Tables {
            kinds,
            f_declarator: field(&lang, "declarator"),
            f_type: field(&lang, "type"),
            f_body: field(&lang, "body"),
            f_name: field(&lang, "name"),
            f_parameters: field(&lang, "parameters"),
            f_function: field(&lang, "function"),
            f_arguments: field(&lang, "arguments"),
            f_argument: field(&lang, "argument"),
            f_field: field(&lang, "field"),
            f_left: field(&lang, "left"),
            f_right: field(&lang, "right"),
            f_value: field(&lang, "value"),
            f_designator: field(&lang, "designator"),
            f_path: field(&lang, "path"),
            f_operator: field(&lang, "operator"),
            f_scope: field(&lang, "scope"),
            f_default_value: field(&lang, "default_value"),
            f_captures: field(&lang, "captures"),
            f_member: field(&lang, "member"),
            f_condition: field(&lang, "condition"),
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Local types
// ---------------------------------------------------------------------------------------------
//
// The scope table maps a local name to a `u32`. For a variable that is the name id of its type
// plus two flag bits; three values above every name id mark locals that are not variables.

/// A template parameter: a local type, never referenced.
const TPARAM: u32 = u32::MAX - 1;
/// A local variable bound to a lambda that was recorded as a nested function definition: calls
/// and uses of it are references to that definition.
const LAMBDA: u32 = u32::MAX - 2;
/// Everything at or above is not a type name (`NONE`, `TPARAM`, `LAMBDA`).
const SENTINEL: u32 = u32::MAX - 15;
/// The type is a class template instance (`vector<T> v`): `v.m()` is a member of the template,
/// but what `v->m()`, `*v` and `v[i]` reach is unknown.
const GENERIC: u32 = 1 << 31;
/// The local is a smart pointer and the recorded type is its pointee: `p->m()` and `*p` reach
/// the pointee, `p.m()` is a member of the pointer class.
const SMART: u32 = 1 << 30;
/// The local is not a variable but a type alias (`typedef Foo<int> T;`, `using T = Foo;` in a
/// function body): the recorded type is what it stands for.
const ALIAS: u32 = 1 << 29;
/// With `SMART`: which wrapper the local is (index into `WRAPPERS`, plus one), so that `p.m()`
/// can name the wrapper class.
const WRAP_SHIFT: u32 = 25;
const WRAP_MASK: u32 = 0xF << WRAP_SHIFT;
const TYPE_MASK: u32 = (1 << WRAP_SHIFT) - 1;

/// The scope entry of a local alias for the type `(id, flags)`.
#[inline]
fn alias_of(id: u32, flags: u32) -> u32 {
    if id == NONE { TPARAM } else { id | flags | ALIAS }
}

#[inline]
fn enc(id: u32, flags: u32) -> u32 {
    if id == NONE { NONE } else { id | flags }
}

/// How a receiver expression is accessed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Acc {
    Dot,
    Arrow,
    Deref,
    Index,
}

/// What kind of scope declarations are being read in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sk {
    /// File or namespace scope.
    Top,
    /// Class body.
    Class,
    /// Function or lambda body.
    Func,
}

/// A `{ ... }` block at file scope that belongs to a definition recognised just before it.
struct Pending<'t> {
    def: u32,
    fdecl: Option<Node<'t>>,
    /// End byte of the item the block must directly follow.
    after: usize,
    macro_fn: bool,
}

/// Collections reused from file to file (kept per thread).
#[derive(Default)]
struct State {
    /// Type path for designated initializers in the current initializer list, or `NONE`.
    init_ctx: Vec<u32>,
    /// Identifier arguments of a function-defining macro: treated as locals in its body.
    macro_params: Vec<u32>,
    /// Parameters of the `#define` being scanned.
    pp_params: Vec<u32>,
    /// Names passed to `EXPORT_SYMBOL*`.
    exports: Vec<u32>,
    /// Names known to be namespaces in this file.
    namespaces: Vec<u32>,
    /// `namespace alias = target;`
    ns_aliases: Vec<(u32, u32)>,
    /// Classes that are not visible outside this file (anonymous namespace, function-local).
    local_classes: Vec<u32>,
    /// (class, base class) pairs seen in this file.
    bases: Vec<(u32, u32)>,
    /// (class, data member) pairs seen in this file.
    fields: FxHashSet<u64>,
    /// Names of types defined in this file.
    known_types: FxHashSet<u32>,
    /// Names defined at file or namespace scope in this file (functions, variables, macros,
    /// enumerators): inside a member function they are not reached through `this`.
    globals: FxHashSet<u32>,
    /// Aliases declared in a class body, keyed like `fields`, with the (encoded) type they
    /// stand for: `using MapType = DenseMap<K, V>;`. Names like `MapType`, `iterator` or `Base`
    /// mean something else in every class, so members declared with them get the target.
    class_aliases: rustc_hash::FxHashMap<u64, u32>,
    /// Names written in a type position so far in this file (`Foo x`, `Foo *f()`): `Foo(1)`
    /// constructs one, `: Foo(1)` in a constructor is a base-class initialiser.
    types_seen: FxHashSet<u32>,
    /// Constructor initialisers `name(args)` of a class declared in another file, which may be
    /// a data member or a base class: (index of the member-write reference, index of the
    /// binding recorded with it or `NONE`, argument count). Decided at the end of the file.
    pending_inits: Vec<(u32, u32, u8)>,
}

impl State {
    fn clear(&mut self) {
        self.init_ctx.clear();
        self.macro_params.clear();
        self.pp_params.clear();
        self.exports.clear();
        self.namespaces.clear();
        self.ns_aliases.clear();
        self.local_classes.clear();
        self.bases.clear();
        self.fields.clear();
        self.known_types.clear();
        self.globals.clear();
        self.types_seen.clear();
        self.pending_inits.clear();
        self.class_aliases.clear();
    }
}

#[derive(Default)]
struct Local {
    parser: Option<Parser>,
    patched: Vec<u8>,
    blanks: Vec<Blank>,
    st: State,
}

thread_local! {
    static LOCAL: RefCell<Local> = RefCell::new(Local::default());
}

struct W<'a, 't> {
    cx: Ctx<'a>,
    t: &'static Tables,
    st: &'a mut State,
    sk: Sk,
    /// Nesting of function and lambda bodies.
    fn_depth: u32,
    /// Definition new definitions hang from (namespace, class, function), or `NONE`.
    parent: u32,
    /// Definition that owns the members being declared (class, struct, enum, typedef), or `NONE`.
    container: u32,
    /// Class whose scope unqualified names are looked up in (the implicit `this`), or `NONE`.
    cur_class: u32,
    /// Inside `struct name` used as a type: the keyword is there because `name` alone may
    /// denote a function (`struct stat` and `stat()`).
    elaborated: bool,
    /// Nesting of anonymous namespaces.
    anon_ns: u32,
    in_friend: bool,
    /// The `template <...>` declaration wrapping the definition about to be recorded.
    span: Option<Node<'t>>,
    pending: Option<Pending<'t>>,
    /// The parameter list found among the arguments of the function-defining macro whose body
    /// is about to be read.
    macro_group: Option<Node<'t>>,
    pool: Vec<TreeCursor<'t>>,
    /// Set when the tree is unusable because an annotation macro derailed the grammar without
    /// leaving an error behind: the caller blanks the macros and starts over.
    reparse: bool,
    allow_reparse: bool,
}

pub fn extract(_lang: Lang, src: &[u8], tree: &Tree, path: &str, facts: &mut FileFacts, scratch: &mut Scratch) {
    let nlines = facts.nlines;
    LOCAL.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else { return };
        let Local { parser, patched, blanks, st } = &mut *guard;
        if !tree.root_node().has_error() {
            if !run(src, src, tree, path, facts, scratch, st, &[], true) {
                return;
            }
            // An annotation macro derailed the parse without leaving an error: start over.
            facts.clear();
            facts.nlines = nlines;
        }
        blanks.clear();
        if blanking_enabled() && !hopeless(tree, nlines) && blank_macros(src, patched, blanks) {
            let p = parser.get_or_insert_with(|| {
                let mut p = Parser::new();
                let _ = p.set_language(&tree_sitter_cpp::LANGUAGE.into());
                p
            });
            // Blanking keeps every offset, so the first tree can seed an incremental parse: only
            // the surroundings of the blanked macros are parsed again.
            let seed = if incremental_enabled() { Some(edited(tree, src, blanks)) } else { None };
            if let Some(clean) = p.parse(&*patched, seed.as_ref()) {
                run(patched, src, &clean, path, facts, scratch, st, blanks, false);
                return;
            }
        }
        run(src, src, tree, path, facts, scratch, st, &[], false);
    });
}

/// Whether the top level of the tree is mostly errors: the file is not C++ (binary noise, another
/// language behind a C++ extension). Parsing such input is slow, and parsing it a second time
/// with a few names blanked cannot help.
fn hopeless(tree: &Tree, nlines: u32) -> bool {
    let mut c = tree.root_node().walk();
    let mut errors = 0u32;
    if c.goto_first_child() {
        loop {
            if c.node().is_error() {
                errors += 1;
            }
            if !c.goto_next_sibling() {
                break;
            }
        }
    }
    errors > 64 && errors.saturating_mul(8) > nlines
}

/// The source with annotation macros blanked, as the extractor parses it when the grammar
/// trips over them; `None` when there is nothing to blank. For debugging and tests.
#[doc(hidden)]
pub fn blanked(src: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut blanks = Vec::new();
    blank_macros(src, &mut out, &mut blanks).then_some(out)
}

/// `CODEFAST_CPP_NO_BLANK=1` turns the macro-blanking pass off (to compare its effect).
fn blanking_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CODEFAST_CPP_NO_BLANK").is_none())
}

fn incremental_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CODEFAST_CPP_FULL_REPARSE").is_none())
}

/// A copy of `tree` in which the blanked ranges are marked as edited (same length, new content).
fn edited(tree: &Tree, src: &[u8], blanks: &[Blank]) -> Tree {
    let mut t = tree.clone();
    // running position -> (row, start of its line)
    let (mut pos, mut row, mut line_start) = (0usize, 0usize, 0usize);
    for b in blanks {
        let (s, e) = (b.off as usize, ((b.off + b.len) as usize).min(src.len()));
        if s < pos || e < s {
            continue;
        }
        for i in memchr::memchr_iter(b'\n', &src[pos..s]) {
            row += 1;
            line_start = pos + i + 1;
        }
        let start = tree_sitter::Point { row, column: s - line_start };
        for i in memchr::memchr_iter(b'\n', &src[s..e]) {
            row += 1;
            line_start = s + i + 1;
        }
        pos = e;
        let end = tree_sitter::Point { row, column: e - line_start };
        t.edit(&tree_sitter::InputEdit {
            start_byte: s,
            old_end_byte: e,
            new_end_byte: e,
            start_position: start,
            old_end_position: end,
            new_end_position: end,
        });
    }
    t
}

/// Walk `tree`, which was parsed from `src`. `orig` is the file as it is on disk (`src` is a
/// copy with macros blanked when `blanks` is not empty). Returns true when the walk was given up
/// and must be repeated on a tree parsed from blanked source.
#[allow(clippy::too_many_arguments)]
fn run<'a, 't>(
    src: &'a [u8],
    orig: &'a [u8],
    tree: &'t Tree,
    path: &'a str,
    facts: &'a mut FileFacts,
    scratch: &'a mut Scratch,
    st: &'a mut State,
    blanks: &[Blank],
    allow_reparse: bool,
) -> bool {
    let mut cx = Ctx::new(src, path, facts, scratch);
    cx.is_header = Lang::is_header(path);
    st.clear();
    let mut w = W {
        cx,
        t: tables(),
        st,
        sk: Sk::Top,
        fn_depth: 0,
        parent: NONE,
        container: NONE,
        cur_class: NONE,
        elaborated: false,
        anon_ns: 0,
        in_friend: false,
        span: None,
        pending: None,
        macro_group: None,
        pool: Vec::new(),
        reparse: false,
        allow_reparse,
    };
    let mut c = tree.walk();
    w.visit(&mut c);
    if w.reparse {
        return true;
    }
    w.finish(orig, blanks);
    false
}

impl<'a, 't> W<'a, 't> {
    #[inline]
    fn k(&self, n: Node<'t>) -> K {
        // ERROR nodes carry an id (65535) outside the grammar's symbol table
        match self.t.kinds.get(n.kind_id() as usize) {
            Some(&k) => k,
            None => K::Error,
        }
    }

    fn children(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH || self.reparse {
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

    /// Visit every child except the ones attached through field `skip`.
    fn children_except(&mut self, c: &mut TreeCursor<'t>, skip: u16) {
        if self.cx.depth >= MAX_DEPTH || self.reparse {
            return;
        }
        self.cx.depth += 1;
        if c.goto_first_child() {
            loop {
                if !is_field(c, skip) {
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

    fn visit(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        match self.k(n) {
            K::Skip | K::FieldIdentifier | K::StorageClass | K::Comment | K::DestructorName | K::OperatorName | K::This | K::NumberLiteral => {}
            K::Identifier => self.identifier(n),
            K::TypeIdentifier => self.type_identifier(n),
            K::NamespaceIdentifier => self.scope_identifier(n),
            K::QualifiedIdentifier => self.qualified(n, c),
            K::FunctionDefinition => self.function_definition(n, c),
            K::Declaration => self.declaration(n, c, false),
            K::FieldDeclaration => self.declaration(n, c, true),
            K::TypeDefinition => self.type_definition(n, c),
            K::AliasDeclaration => self.alias_declaration(n, c),
            K::StructSpecifier => self.record_specifier(n, c, DefKind::Struct),
            K::UnionSpecifier => self.record_specifier(n, c, DefKind::Union),
            K::ClassSpecifier => self.record_specifier(n, c, DefKind::Class),
            K::EnumSpecifier => self.record_specifier(n, c, DefKind::Enum),
            K::Enumerator => self.enumerator(n, c),
            K::NamespaceDefinition => self.namespace_definition(n, c),
            K::NamespaceAlias => self.namespace_alias(n),
            K::UsingDeclaration => self.using_declaration(n),
            K::TemplateDeclaration => self.template_declaration(n, c),
            K::TemplateParameterList => self.template_parameter_list(c),
            K::TypeParameter => self.type_parameter(n),
            K::OptionalTypeParameter => self.optional_type_parameter(n, c),
            K::ConceptDefinition => self.concept_definition(n, c),
            K::FriendDeclaration => self.friend_declaration(c),
            K::BaseClassClause => self.base_clause(c),
            K::FieldInitializer => self.field_initializer(n, c),
            K::PreprocDef => self.preproc_def(n, false),
            K::PreprocFunctionDef => self.preproc_def(n, true),
            K::PreprocInclude => self.include(n),
            K::PreprocIf => self.preproc_if(c),
            K::CallExpression => self.call_expression(n, c),
            K::FieldExpression => self.field_expression(n, c),
            K::AssignmentExpression => self.assignment(n, c),
            K::UpdateExpression => self.update(n, c),
            K::InitializerList => self.initializer_list(c),
            K::ExpressionStatement => self.expression_statement(n, c),
            K::CompoundStatement => self.compound(n, c),
            K::Error => self.error(n, c),
            K::ParameterDeclaration | K::OptionalParameterDeclaration | K::VariadicParameterDeclaration => self.parameter_declaration(n, c),
            K::CompoundLiteral => self.compound_literal(n, c),
            K::NewExpression => self.new_expression(n, c),
            K::LambdaExpression => self.lambda(n, c),
            K::ForRangeLoop => self.for_range(n, c),
            K::ForStatement => self.scoped(c),
            K::CatchClause | K::RequiresExpression => self.scoped_params(n, c),
            K::OffsetofExpression => self.offsetof(n, c),
            _ => self.children(c),
        }
    }

    // ----- small helpers -----

    fn cursor_at(&mut self, n: Node<'t>) -> TreeCursor<'t> {
        match self.pool.pop() {
            Some(mut c) => {
                c.reset(n);
                c
            }
            None => n.walk(),
        }
    }

    fn release(&mut self, c: TreeCursor<'t>) {
        if self.pool.len() < 8 {
            self.pool.push(c);
        }
    }

    #[inline]
    fn is_empty(n: Node<'t>) -> bool {
        n.end_byte() <= n.start_byte()
    }

    #[inline]
    fn field_key(class: u32, name: u32) -> u64 {
        ((class as u64) << 32) | name as u64
    }

    #[inline]
    fn is_member(&self, class: u32, name: u32) -> bool {
        class != NONE && !self.st.fields.is_empty() && self.st.fields.contains(&Self::field_key(class, name))
    }

    fn is_base(&self, class: u32, name: u32) -> bool {
        self.st.bases.iter().any(|&(c, b)| c == class && b == name)
    }

    fn is_namespace(&self, name: u32) -> bool {
        self.st.namespaces.contains(&name) || self.cx.name_bytes(name) == b"std"
    }

    fn add_namespace(&mut self, name: u32) {
        if !self.st.namespaces.contains(&name) {
            self.st.namespaces.push(name);
        }
    }

    fn alias_target(&self, name: u32) -> Option<u32> {
        self.st.ns_aliases.iter().find(|&&(a, _)| a == name).map(|&(_, t)| t)
    }

    /// The class an unqualified, non-local name may belong to (the implicit `this`): the
    /// enclosing class, unless the name is known to live at file or namespace scope.
    #[inline]
    fn implicit_class(&self, name: u32) -> u32 {
        // a name of file scope (a global, an enumerator) is not reached through `this`, unless
        // the class declares a member of that name: the member hides it
        if self.cur_class != NONE && !self.st.globals.is_empty() && self.st.globals.contains(&name) && !self.is_member(self.cur_class, name) {
            NONE
        } else {
            self.cur_class
        }
    }

    /// Whether definitions made here are invisible outside this file.
    #[inline]
    fn ctx_local(&self) -> bool {
        self.fn_depth > 0 || (self.anon_ns > 0 && !self.cx.is_header)
    }

    #[inline]
    fn local_of(&self, name: u32) -> Option<u32> {
        if self.cx.in_scope() { self.cx.local_type(name) } else { None }
    }

    /// Whether `name` is a type that only exists locally: a template parameter or an alias
    /// declared in a function body. Such names are never references.
    #[inline]
    fn is_local_type(&self, name: u32) -> bool {
        matches!(self.local_of(name), Some(r) if r == TPARAM || (r < SENTINEL && r & ALIAS != 0))
    }

    /// Number of arguments in an argument or initializer list. Comments are named nodes too
    /// (`f(x, /*Flag=*/true)`): they do not count.
    fn count_args(&mut self, list: Node<'t>) -> usize {
        let n = list.named_child_count();
        if n == 0 || memchr::memchr(b'/', self.cx.text(list)).is_none() {
            return n;
        }
        let mut count = 0usize;
        let mut c = self.cursor_at(list);
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if ch.is_named() && self.k(ch) != K::Comment {
                    count += 1;
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(c);
        count
    }

    /// Intern `prefix` + `rest`.
    fn joined(&mut self, prefix: &[u8], rest: &[u8]) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.extend_from_slice(prefix);
        buf.extend_from_slice(rest);
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    // ----- leaves -----

    fn identifier(&mut self, n: Node<'t>) {
        let pos = n.start_byte() as u32;
        if n.end_byte() as u32 <= pos || (!self.cx.s.handled.is_empty() && self.cx.s.handled.contains(&pos)) {
            return;
        }
        let text = self.cx.text(n);
        if reserved(text) {
            return;
        }
        let name = self.cx.intern(text);
        match self.local_of(name) {
            Some(LAMBDA) => return self.cx.add_ref(name, RefKind::Read, line_of(n), NONE, 255),
            Some(_) => return,
            None => {}
        }
        let write = !self.cx.s.writes.is_empty() && self.cx.s.writes.contains(&pos);
        self.value_ref(name, line_of(n), write);
    }

    /// A non-local name used as a value. Inside a class it may be a member reached through the
    /// implicit `this`: a known data member becomes a member access, anything else carries the
    /// class as qualifier so that the linker can look there first.
    fn value_ref(&mut self, name: u32, line: u32, write: bool) {
        let mut cls = self.cur_class;
        let kind = if self.is_member(cls, name) {
            if write { RefKind::MemberWrite } else { RefKind::MemberRead }
        } else {
            cls = self.implicit_class(name);
            if write { RefKind::Write } else { RefKind::Read }
        };
        self.cx.add_ref(name, kind, line, cls, 255);
    }

    fn type_identifier(&mut self, n: Node<'t>) {
        if Self::is_empty(n) || self.cx.is_marked(n) {
            return;
        }
        let text = self.cx.text(n);
        if reserved(text) {
            return;
        }
        let name = self.cx.intern(text);
        if self.is_local_type(name) {
            return;
        }
        self.cx.add_ref(name, RefKind::Type, line_of(n), NONE, 255);
        if !self.elaborated {
            self.st.types_seen.insert(name);
        }
    }

    /// Whether `name` is known to denote a type here: the enclosing class, a type defined in
    /// this file, or a name already written in a type position.
    #[inline]
    fn is_type_name(&self, name: u32) -> bool {
        name == self.cur_class || self.st.known_types.contains(&name) || (!self.st.types_seen.is_empty() && self.st.types_seen.contains(&name))
    }

    /// A namespace or class used as a scope: `ns::f()`, `Type::member`.
    fn scope_identifier(&mut self, n: Node<'t>) {
        if Self::is_empty(n) || self.cx.is_marked(n) {
            return;
        }
        let text = self.cx.text(n);
        if text == b"std" {
            return;
        }
        let name = self.cx.intern(text);
        if self.is_local_type(name) || self.alias_target(name).is_some() {
            return;
        }
        self.cx.add_ref(name, RefKind::Type, line_of(n), NONE, 255);
    }

    // ----- declarator helpers -----

    /// Find the function declarator inside a declarator chain. Returns the declarator, the
    /// terminal that names the declared entity and whether the name sits behind a pointer inside
    /// the function declarator (`int (*fp)(int)` is a function pointer, not a function).
    fn function_declarator(&self, mut d: Node<'t>) -> Option<(Node<'t>, Node<'t>, bool)> {
        let t = self.t;
        let mut fdecl: Option<Node<'t>> = None;
        let mut ptr_inside = false;
        for _ in 0..16 {
            match self.k(d) {
                K::FunctionDeclarator => {
                    fdecl = Some(d);
                    ptr_inside = false;
                    d = child(d, t.f_declarator)?;
                }
                K::PointerDeclarator => {
                    if fdecl.is_some() {
                        ptr_inside = true;
                    }
                    d = child(d, t.f_declarator)?;
                }
                K::ReferenceDeclarator => {
                    if fdecl.is_some() {
                        ptr_inside = true;
                    }
                    d = d.named_child(0)?;
                }
                K::ArrayDeclarator | K::InitDeclarator => d = child(d, t.f_declarator).or_else(|| d.named_child(0))?,
                K::AttributedDeclarator | K::VariadicDeclarator => d = d.named_child(0)?,
                K::ParenthesizedDeclarator => d = last_named(d)?,
                K::QualifiedIdentifier => {
                    let leaf = self.qualified_last(d)?;
                    return match self.k(leaf) {
                        // `void (Foo::*pm)(int)`: a pointer to member function
                        K::PointerDeclarator => fdecl.map(|f| (f, leaf, true)),
                        K::OperatorCast => Some((leaf, d, false)),
                        _ => fdecl.map(|f| (f, d, ptr_inside)),
                    };
                }
                K::OperatorCast => return Some((d, d, false)),
                K::Identifier | K::FieldIdentifier | K::TypeIdentifier | K::OperatorName | K::DestructorName | K::TemplateFunction | K::TemplateMethod => {
                    return fdecl.map(|f| (f, d, ptr_inside));
                }
                _ => return None,
            }
        }
        None
    }

    /// The terminal of a declarator chain: the node that names the declared entity.
    fn declared_ident(&self, mut d: Node<'t>) -> Option<Node<'t>> {
        let t = self.t;
        for _ in 0..16 {
            match self.k(d) {
                K::Identifier | K::FieldIdentifier | K::TypeIdentifier | K::QualifiedIdentifier | K::StructuredBinding => return Some(d),
                K::FunctionDeclarator | K::PointerDeclarator | K::ArrayDeclarator | K::InitDeclarator => {
                    d = child(d, t.f_declarator).or_else(|| d.named_child(0))?;
                }
                K::ReferenceDeclarator | K::AttributedDeclarator | K::VariadicDeclarator => d = d.named_child(0)?,
                K::ParenthesizedDeclarator => d = last_named(d)?,
                _ => return None,
            }
        }
        None
    }

    /// Whether a declarator declares a pointer (`*p`, `*const p`, `**p`).
    fn is_pointer(&self, mut d: Node<'t>) -> bool {
        let t = self.t;
        for _ in 0..8 {
            match self.k(d) {
                K::PointerDeclarator => return true,
                K::InitDeclarator | K::ArrayDeclarator => match child(d, t.f_declarator) {
                    Some(x) => d = x,
                    None => return false,
                },
                K::ReferenceDeclarator | K::AttributedDeclarator => match d.named_child(0) {
                    Some(x) => d = x,
                    None => return false,
                },
                K::ParenthesizedDeclarator => match last_named(d) {
                    Some(x) => d = x,
                    None => return false,
                },
                _ => return false,
            }
        }
        false
    }

    /// The last component of a (possibly nested) qualified identifier.
    fn qualified_last(&self, mut q: Node<'t>) -> Option<Node<'t>> {
        for _ in 0..16 {
            let nm = child(q, self.t.f_name)?;
            if self.k(nm) != K::QualifiedIdentifier {
                return Some(nm);
            }
            q = nm;
        }
        None
    }

    /// The identifier inside a name that may carry template arguments.
    fn leaf_name(&self, mut n: Node<'t>) -> Option<Node<'t>> {
        for _ in 0..4 {
            match self.k(n) {
                K::TemplateFunction | K::TemplateMethod | K::TemplateType => n = child(n, self.t.f_name)?,
                K::DependentName => n = n.named_child(0)?,
                _ => return Some(n),
            }
        }
        None
    }

    /// Simple name of the scope component of one qualified identifier level.
    fn scope_name(&self, s: Node<'t>) -> Option<Node<'t>> {
        match self.k(s) {
            K::NamespaceIdentifier => Some(s),
            K::TemplateType => child(s, self.t.f_name),
            K::DependentName => s.named_child(0).and_then(|x| child(x, self.t.f_name)),
            _ => None,
        }
    }

    /// Name of a declared entity from the terminal of its declarator: the interned name, the
    /// scope component directly before it (`NONE` when unqualified) and the node to mark.
    fn decl_name(&mut self, term: Node<'t>) -> Option<(u32, u32, Node<'t>)> {
        let t = self.t;
        let mut term = term;
        let mut qual = NONE;
        for _ in 0..16 {
            match self.k(term) {
                K::Identifier | K::FieldIdentifier | K::TypeIdentifier => {
                    // a keyword here is what is left of code the grammar could not read
                    if Self::is_empty(term) || reserved(self.cx.text(term)) {
                        return None;
                    }
                    return Some((self.cx.name(term), qual, term));
                }
                K::OperatorName => return Some((self.op_name(term), qual, term)),
                K::DestructorName => return Some((self.squeezed(term), qual, term)),
                K::OperatorCast => return Some((self.cast_name(term), qual, term)),
                K::TemplateFunction | K::TemplateMethod | K::TemplateType => term = child(term, t.f_name)?,
                K::DependentName => term = term.named_child(0)?,
                K::PointerDeclarator => term = child(term, t.f_declarator)?,
                K::QualifiedIdentifier => {
                    if let Some(s) = child(term, t.f_scope) {
                        let nm = child(term, t.f_name)?;
                        // `MACRO std::string f()` leaves a qualified name without its `::`
                        let glued = memchr::memchr(b':', &self.cx.src[s.end_byte().min(nm.start_byte())..nm.start_byte()]).is_some();
                        qual = match self.scope_name(s) {
                            Some(x) if glued && !Self::is_empty(x) => self.cx.name(x),
                            _ => NONE,
                        };
                        term = nm;
                    } else {
                        term = child(term, t.f_name)?;
                    }
                }
                _ => return None,
            }
        }
        None
    }

    /// `operator ==` -> `operator==`, `operator new []` -> `operator new[]`.
    fn op_name(&mut self, n: Node<'t>) -> u32 {
        let txt = self.cx.text(n);
        if !txt.iter().any(|b| b.is_ascii_whitespace()) {
            return self.cx.intern(txt);
        }
        let rest = txt.strip_prefix(b"operator").unwrap_or(txt);
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.extend_from_slice(b"operator");
        for &b in rest {
            if b.is_ascii_whitespace() {
                continue;
            }
            // `operator new`, `operator co_await`: a word, kept apart
            if buf.len() == 8 && (b.is_ascii_alphabetic() || b == b'_') {
                buf.push(b' ');
            }
            buf.push(b);
        }
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// Text of a node with all whitespace removed (`~ Foo` -> `~Foo`).
    fn squeezed(&mut self, n: Node<'t>) -> u32 {
        let txt = self.cx.text(n);
        if !txt.iter().any(|b| b.is_ascii_whitespace()) {
            return self.cx.intern(txt);
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.extend(txt.iter().copied().filter(|b| !b.is_ascii_whitespace()));
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// `operator const char *() const` -> `operator const char *`.
    fn cast_name(&mut self, n: Node<'t>) -> u32 {
        let t = self.t;
        let mut end = n.end_byte();
        let mut d = child(n, t.f_declarator);
        for _ in 0..8 {
            let Some(x) = d else { break };
            if self.k(x) == K::AbstractFunctionDeclarator {
                if let Some(p) = child(x, t.f_parameters) {
                    end = p.start_byte();
                }
                break;
            }
            d = child(x, t.f_declarator).or_else(|| x.named_child(0));
        }
        let start = n.start_byte();
        let txt = &self.cx.src[start..end.max(start)];
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let mut space = false;
        for &b in txt {
            if b.is_ascii_whitespace() {
                space = true;
                continue;
            }
            if space && !buf.is_empty() {
                buf.push(b' ');
            }
            space = false;
            buf.push(b);
        }
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    // ----- types -----

    /// Simple name of the type a type node denotes and how members are reached through it:
    /// `(name id or NONE, GENERIC | SMART | 0)`. Pointers, references and qualifiers live in
    /// declarators and are not part of the type node. `std::unique_ptr<T>` / `shared_ptr<T>` /
    /// `weak_ptr<T>` denote their pointee when `unwrap` is set.
    fn type_info(&mut self, ty: Node<'t>, unwrap: bool) -> (u32, u32) {
        let t = self.t;
        let mut ty = ty;
        for _ in 0..8 {
            match self.k(ty) {
                K::TypeIdentifier => {
                    if Self::is_empty(ty) {
                        return (NONE, 0);
                    }
                    let id = self.cx.name(ty);
                    return match self.local_of(id) {
                        Some(TPARAM) => (NONE, 0),
                        // a local alias stands for its target
                        Some(raw) if raw < SENTINEL && raw & ALIAS != 0 => {
                            if raw & SMART != 0 && !unwrap {
                                (NONE, 0)
                            } else {
                                (raw & TYPE_MASK, raw & (GENERIC | SMART))
                            }
                        }
                        _ => match self.class_alias(id) {
                            // an alias of the enclosing class stands for its target
                            Some(raw) if raw & SMART == 0 || unwrap => (raw & TYPE_MASK, raw & (GENERIC | SMART)),
                            Some(_) => (NONE, 0),
                            None => (id, 0),
                        },
                    };
                }
                K::StructSpecifier | K::UnionSpecifier | K::ClassSpecifier | K::EnumSpecifier | K::QualifiedIdentifier => match child(ty, t.f_name) {
                    Some(x) => ty = x,
                    None => return (NONE, 0),
                },
                K::TypeDescriptor => match child(ty, t.f_type) {
                    Some(x) => ty = x,
                    None => return (NONE, 0),
                },
                K::DependentName | K::DependentType => match ty.named_child(0) {
                    Some(x) => ty = x,
                    None => return (NONE, 0),
                },
                K::TemplateType => {
                    let Some(nm) = child(ty, t.f_name) else { return (NONE, 0) };
                    if Self::is_empty(nm) {
                        return (NONE, 0);
                    }
                    if unwrap && is_smart_pointer(self.cx.text(nm)) {
                        let arg = child(ty, t.f_arguments).and_then(|a| a.named_child(0));
                        return match arg {
                            Some(a) => (self.type_info(a, false).0, SMART | wrapper_bits(self.cx.text(nm))),
                            None => (NONE, 0),
                        };
                    }
                    let id = self.cx.name(nm);
                    if self.local_of(id) == Some(TPARAM) {
                        return (NONE, 0);
                    }
                    return (id, GENERIC);
                }
                _ => return (NONE, 0),
            }
        }
        (NONE, 0)
    }

    /// What the alias `name` declared in the body of the enclosing class stands for (encoded).
    #[inline]
    fn class_alias(&self, name: u32) -> Option<u32> {
        if self.cur_class == NONE || self.st.class_aliases.is_empty() {
            return None;
        }
        self.st.class_aliases.get(&Self::field_key(self.cur_class, name)).copied()
    }

    /// Whether a type is written `typename X::name`: a member of something that depends on a
    /// template parameter. Its last name (`type`, `iterator`) says nothing on its own.
    fn is_dependent(&self, ty: Node<'t>) -> bool {
        let inner = if self.k(ty) == K::TypeDescriptor { child(ty, self.t.f_type) } else { Some(ty) };
        inner.is_some_and(|x| self.k(x) == K::DependentType)
    }

    /// Remember an alias declared in a class body.
    fn note_class_alias(&mut self, name: u32, target: u32, flags: u32) {
        if self.sk == Sk::Class && self.cur_class != NONE && target != NONE && target != name {
            self.st.class_aliases.insert(Self::field_key(self.cur_class, name), target | flags);
        }
    }

    /// The identifier that carries a type's simple name, with the scope it is qualified by.
    fn type_head(&mut self, ty: Node<'t>) -> Option<(Node<'t>, u32)> {
        let t = self.t;
        match self.k(ty) {
            K::TypeIdentifier => Some((ty, NONE)),
            K::TemplateType => child(ty, t.f_name).map(|x| (x, NONE)),
            K::TypeDescriptor => child(ty, t.f_type).and_then(|x| self.type_head(x)),
            K::QualifiedIdentifier => {
                let (last, qual) = self.split_qualified(ty);
                let leaf = self.leaf_name(last?)?;
                if self.k(leaf) == K::TypeIdentifier { Some((leaf, qual)) } else { None }
            }
            _ => None,
        }
    }

    /// Return type of a function: the declared type, or the trailing one behind `auto`.
    fn return_type(&mut self, ty: Option<Node<'t>>, fdecl: Node<'t>) -> u32 {
        let Some(ty) = ty else { return NONE };
        if self.k(ty) != K::PlaceholderType {
            return self.type_info(ty, true).0;
        }
        let mut c = self.cursor_at(fdecl);
        let mut out = NONE;
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if self.k(ch) == K::TrailingReturnType {
                    if let Some(td) = ch.named_child(0) {
                        out = self.type_info(td, true).0;
                    }
                    break;
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(c);
        out
    }

    fn arity(&mut self, fdecl: Node<'t>) -> (u8, u8) {
        if !matches!(self.k(fdecl), K::FunctionDeclarator | K::AbstractFunctionDeclarator) {
            return (0, 0);
        }
        let Some(params) = child(fdecl, self.t.f_parameters) else { return (0, 255) };
        let (mut req, mut opt, mut variadic) = (0u32, 0u32, false);
        let mut c = self.cursor_at(params);
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                match self.k(ch) {
                    K::ParameterDeclaration => {
                        let is_void = child(ch, self.t.f_declarator).is_none() && self.cx.text(ch) == b"void";
                        if !is_void {
                            req += 1;
                        }
                    }
                    K::OptionalParameterDeclaration => opt += 1,
                    K::VariadicParameterDeclaration => variadic = true,
                    _ => {
                        if !ch.is_named() && self.cx.text(ch) == b"..." {
                            variadic = true;
                        }
                    }
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(c);
        let min = req.min(254) as u8;
        let max = (req + opt).min(254) as u8;
        (min, if variadic { 255 } else { max })
    }

    /// One pass over the specifiers of a declaration.
    fn specs(&mut self, n: Node<'t>) -> Specs {
        let mut s = Specs::default();
        let mut c = self.cursor_at(n);
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                match self.k(ch) {
                    K::StorageClass => match self.cx.text(ch) {
                        b"static" => s.is_static = true,
                        b"extern" => s.is_extern = true,
                        _ => {}
                    },
                    K::TypeQualifier => match self.cx.text(ch) {
                        b"const" => s.is_const = true,
                        b"constexpr" | b"constinit" => s.is_constexpr = true,
                        _ => {}
                    },
                    _ => {}
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(c);
        s
    }

    /// Declare the names a declarator introduces as locals of type `ty` (encoded).
    fn declare_declarator(&mut self, d: Node<'t>, ty: u32) {
        let t = self.t;
        let mut d = d;
        let mut ty = ty;
        for _ in 0..16 {
            match self.k(d) {
                K::Identifier => {
                    if !Self::is_empty(d) {
                        let nm = self.cx.name(d);
                        self.cx.declare(nm, ty);
                        self.cx.mark(d);
                    }
                    return;
                }
                K::StructuredBinding => {
                    let mut c = self.cursor_at(d);
                    if c.goto_first_child() {
                        loop {
                            let ch = c.node();
                            if self.k(ch) == K::Identifier && !Self::is_empty(ch) {
                                let nm = self.cx.name(ch);
                                self.cx.declare(nm, NONE);
                                self.cx.mark(ch);
                            }
                            if !c.goto_next_sibling() {
                                break;
                            }
                        }
                    }
                    self.release(c);
                    return;
                }
                K::PointerDeclarator => {
                    // a pointer to a template instance is an ordinary pointer; a pointer to a
                    // smart pointer is not worth modelling
                    if ty < SENTINEL {
                        ty = if ty & SMART != 0 { NONE } else { ty & !GENERIC };
                    }
                    match child(d, t.f_declarator) {
                        Some(x) => d = x,
                        None => return,
                    }
                }
                K::FunctionDeclarator => {
                    ty = NONE;
                    match child(d, t.f_declarator) {
                        Some(x) => d = x,
                        None => return,
                    }
                }
                K::ArrayDeclarator | K::InitDeclarator => match child(d, t.f_declarator).or_else(|| d.named_child(0)) {
                    Some(x) => d = x,
                    None => return,
                },
                K::ReferenceDeclarator | K::AttributedDeclarator | K::VariadicDeclarator => match d.named_child(0) {
                    Some(x) => d = x,
                    None => return,
                },
                K::ParenthesizedDeclarator => match last_named(d) {
                    Some(x) => d = x,
                    None => return,
                },
                _ => return,
            }
        }
    }

    fn declare_params(&mut self, fdecl: Node<'t>) {
        if let Some(params) = child(fdecl, self.t.f_parameters) {
            self.declare_param_list(params);
        }
    }

    fn declare_param_list(&mut self, params: Node<'t>) {
        let t = self.t;
        let mut c = self.cursor_at(params);
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if matches!(self.k(ch), K::ParameterDeclaration | K::OptionalParameterDeclaration | K::VariadicParameterDeclaration) {
                    if let Some(d) = child(ch, t.f_declarator) {
                        let ty = match child(ch, t.f_type) {
                            Some(x) => {
                                let (id, fl) = self.type_info(x, true);
                                enc(id, fl)
                            }
                            None => NONE,
                        };
                        self.declare_declarator(d, ty);
                    }
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(c);
    }

    // ----- functions -----

    fn function_definition(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let span = self.span.take().unwrap_or(n);
        let found = child(n, t.f_declarator).and_then(|d| self.function_declarator(d));
        let Some((fdecl, term, _)) = found else {
            // `struct MACRO Name { ... }` parses as a function `Name` returning `struct MACRO`.
            if self.allow_reparse && self.macro_class_head(n) {
                self.reparse = true;
                return;
            }
            return self.children(c);
        };
        if self.sk == Sk::Top && self.fn_depth == 0 && self.cx.s.handled.len() > 2048 {
            self.cx.s.handled.clear();
            self.cx.s.writes.clear();
        }
        let ty = child(n, t.f_type);
        let body = child(n, t.f_body);
        let in_class = self.sk == Sk::Class;
        if ty.is_none() && self.k(term) == K::Identifier && self.k(fdecl) == K::FunctionDeclarator {
            if self.sk == Sk::Func {
                // inside a body, `FOR_EACH(x, list) { ... }` is a statement macro
                return self.statement_macro(c, fdecl, term);
            }
            // No return type and no qualifier: a constructor inside its class, otherwise a
            // function defined through a macro (`TEST(Suite, Name) { }`).
            let is_ctor = in_class && self.cx.text(term) == self.cx.name_bytes_or_empty(self.cur_class);
            if !is_ctor {
                return self.macro_function(c, span, fdecl, term, body);
            }
        }
        // `inline A::operator bool() const { }` is read as a function `bool` returning
        // `A::operator`
        let conv = ty.filter(|_| self.k(term) == K::Identifier).and_then(|x| self.conversion_scope(x));
        let (name, qual, leaf) = match conv {
            Some(scope) => (self.joined(b"operator ", self.cx.text(term)), scope, term),
            None => match self.decl_name(term) {
                Some(x) => x,
                None => return self.children(c),
            },
        };

        // signature end and the clauses that stand in for a body
        let mut sig_end = body.map(|b| b.start_byte()).unwrap_or(n.end_byte());
        let (mut is_static, mut deleted, mut pure) = (false, false, false);
        let mut cc = self.cursor_at(n);
        if cc.goto_first_child() {
            loop {
                let ch = cc.node();
                match self.k(ch) {
                    K::StorageClass => is_static |= self.cx.text(ch) == b"static",
                    K::FieldInitializerList | K::DefaultMethodClause => sig_end = sig_end.min(ch.start_byte()),
                    K::DeleteMethodClause => {
                        deleted = true;
                        sig_end = sig_end.min(ch.start_byte());
                    }
                    K::PureVirtualClause => {
                        pure = true;
                        sig_end = sig_end.min(ch.start_byte());
                    }
                    _ => {}
                }
                if !cc.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(cc);

        let (kind, parent, dqual, class_ctx) = self.function_home(qual, in_class);
        let is_ctor = conv.is_none() && ((qual != NONE && name == qual) || (in_class && qual == NONE && name == self.cur_class));
        let tname = if conv.is_some() {
            NONE
        } else if is_ctor {
            if let Some(x) = ty {
                self.annotation_type(x);
            }
            name
        } else if self.k(leaf) == K::OperatorCast {
            match child(leaf, t.f_type) {
                Some(x) => self.type_info(x, true).0,
                None => NONE,
            }
        } else {
            self.return_type(ty, fdecl)
        };
        let (amin, amax) = self.arity(fdecl);
        let mut flags = 0u8;
        if deleted || pure {
            flags |= dflag::DECL;
        }
        if pure {
            flags |= dflag::ABSTRACT;
        }
        let file_static = is_static && !in_class && qual == NONE && !self.cx.is_header;
        if file_static || self.ctx_local() || (qual != NONE && self.st.local_classes.contains(&qual)) {
            flags |= dflag::LOCAL;
        }
        let d = self.cx.def(name, kind, span, sig_end);
        {
            let dm = self.cx.def_mut(d);
            dm.parent = parent;
            dm.qual = dqual;
            dm.flags = flags;
            dm.type_name = tname;
            dm.arity_min = amin;
            dm.arity_max = amax;
        }
        self.cx.mark(leaf);
        if kind == DefKind::Function && !in_class {
            self.st.globals.insert(name);
        }
        self.function_body(c, d, Some(fdecl), class_ctx, false);
    }

    /// When the "return type" of a function is `operator` or ends in `::operator`, the scope
    /// written before it (`NONE` when there is none): the function is a conversion operator.
    fn conversion_scope(&mut self, ty: Node<'t>) -> Option<u32> {
        match self.k(ty) {
            K::TypeIdentifier => (self.cx.text(ty) == b"operator").then_some(NONE),
            K::QualifiedIdentifier => {
                let last = self.qualified_last(ty)?;
                if self.k(last) != K::TypeIdentifier || self.cx.text(last) != b"operator" {
                    return None;
                }
                let (_, _, tail) = self.split_scopes(ty);
                Some(tail)
            }
            _ => None,
        }
    }

    /// Kind, parent, by-name qualifier and class context of a function named with qualifier
    /// `qual` (`NONE` when unqualified).
    fn function_home(&self, qual: u32, in_class: bool) -> (DefKind, u32, u32, u32) {
        if qual != NONE {
            if self.is_namespace(qual) { (DefKind::Function, NONE, qual, self.cur_class) } else { (DefKind::Method, NONE, qual, qual) }
        } else if in_class && !self.in_friend {
            (DefKind::Method, self.container, NONE, self.cur_class)
        } else if in_class {
            (DefKind::Function, self.parent, NONE, self.cur_class)
        } else {
            (DefKind::Function, self.parent, NONE, self.cur_class)
        }
    }

    /// A constructor has no return type: a type in front of one is an annotation macro
    /// (`LLVM_ABI Foo(int);`). Record it as a use of the macro, not of a type.
    fn annotation_type(&mut self, ty: Node<'t>) {
        if self.k(ty) == K::TypeIdentifier && !Self::is_empty(ty) && !self.cx.is_marked(ty) {
            let id = self.cx.name(ty);
            self.cx.add_ref(id, RefKind::Read, line_of(ty), NONE, 255);
            self.cx.mark(ty);
        }
    }

    /// `struct MACRO Name { ... }` / `class MACRO Name { ... }` parsed as a function definition.
    fn macro_class_head(&self, n: Node<'t>) -> bool {
        let t = self.t;
        let (Some(ty), Some(d), Some(_)) = (child(n, t.f_type), child(n, t.f_declarator), child(n, t.f_body)) else { return false };
        matches!(self.k(ty), K::StructSpecifier | K::ClassSpecifier | K::UnionSpecifier) && child(ty, t.f_body).is_none() && self.k(d) == K::Identifier
    }

    /// Visit a function with its definition, scope and parameters in place. The cursor is on the
    /// function definition (or on the block that is its body).
    fn function_body(&mut self, c: &mut TreeCursor<'t>, d: u32, fdecl: Option<Node<'t>>, class_ctx: u32, macro_locals: bool) {
        let saved = (self.sk, self.parent, self.container, self.cur_class, self.in_friend);
        self.cx.push_def(d);
        self.cx.push_scope();
        self.fn_depth += 1;
        self.sk = Sk::Func;
        self.parent = d;
        self.container = NONE;
        self.cur_class = class_ctx;
        self.in_friend = false;
        if let Some(f) = fdecl {
            self.declare_params(f);
        }
        if macro_locals {
            let params = std::mem::take(&mut self.st.macro_params);
            for &id in &params {
                self.cx.declare(id, NONE);
            }
            self.st.macro_params = params;
            if let Some(list) = self.macro_group.take() {
                self.declare_param_list(list);
            }
        }
        self.children(c);
        self.fn_depth -= 1;
        self.cx.pop_scope();
        self.cx.pop_def();
        (self.sk, self.parent, self.container, self.cur_class, self.in_friend) = saved;
    }

    /// `FOR_EACH(item, list) { ... }` inside a function body: a call of the macro whose
    /// "parameters" are arguments, followed by a block. An argument with a type (`auto x`)
    /// declares a local for the block.
    fn statement_macro(&mut self, c: &mut TreeCursor<'t>, fdecl: Node<'t>, macro_node: Node<'t>) {
        let nargs = self.vexing_args(fdecl).min(254) as u8;
        self.call_ident(macro_node, nargs);
        self.cx.mark(macro_node);
        self.cx.push_scope();
        self.declare_params(fdecl);
        self.children(c);
        self.cx.pop_scope();
    }

    /// `MACRO(name, ...) { body }`: a function defined through a macro. The arguments parse as
    /// parameters; the ones that are a lone identifier name the function (gtest: the suite and
    /// the test) or are parameters of the generated function.
    fn macro_function(&mut self, c: &mut TreeCursor<'t>, span: Node<'t>, fdecl: Node<'t>, macro_node: Node<'t>, body: Option<Node<'t>>) {
        let t = self.t;
        let mname = self.cx.text(macro_node);
        let mut idents: [Option<Node<'t>>; 2] = [None, None];
        let mut nparams = 0u32;
        // `MACRO(ret, name, (int a, char *b)) { }`: a parenthesised group of declarations
        // among the arguments is the parameter list of the generated function
        let mut group: Option<Node<'t>> = None;
        let mut groups = 0u32;
        // the lone identifier argument directly in front of that group: `(ret, name, (params))`
        // names the function, whatever the return type is called
        let mut last_ident: Option<Node<'t>> = None;
        let mut before_group: Option<Node<'t>> = None;
        self.st.macro_params.clear();
        if let Some(params) = child(fdecl, t.f_parameters) {
            let mut pc = self.cursor_at(params);
            if pc.goto_first_child() {
                loop {
                    let ch = pc.node();
                    if ch.is_named() && self.k(ch) != K::Comment {
                        nparams += 1;
                    }
                    if self.k(ch) == K::ParameterDeclaration && child(ch, t.f_type).is_none_or(Self::is_empty) {
                        let inner = child(ch, t.f_declarator).filter(|d| self.k(*d) == K::AbstractFunctionDeclarator).and_then(|d| child(d, t.f_parameters));
                        if let Some(list) = inner {
                            group = Some(list);
                            groups += 1;
                            before_group = last_ident;
                        }
                    }
                    if ch.is_named() && self.k(ch) != K::Comment {
                        last_ident = None;
                    }
                    if self.k(ch) == K::ParameterDeclaration && child(ch, t.f_declarator).is_none() {
                        if let Some(ty) = child(ch, t.f_type).filter(|x| self.k(*x) == K::TypeIdentifier && !Self::is_empty(*x)) {
                            last_ident = Some(ty);
                            self.cx.mark(ty);
                            if idents[0].is_none() {
                                idents[0] = Some(ty);
                            } else {
                                if idents[1].is_none() {
                                    idents[1] = Some(ty);
                                }
                                let id = self.cx.name(ty);
                                self.st.macro_params.push(id);
                            }
                        }
                    }
                    if !pc.goto_next_sibling() {
                        break;
                    }
                }
            }
            self.release(pc);
        }
        let sig_end = body.map(|b| b.start_byte()).unwrap_or(fdecl.end_byte());
        let gtest = match (idents[0], idents[1]) {
            (Some(suite), Some(test)) if nparams == 2 && is_gtest_macro(mname) => Some((suite, test)),
            _ => None,
        };
        let in_class = self.sk == Sk::Class;
        let mut flags = dflag::MACRO_DEFINED;
        if contains(mname, b"TEST") {
            flags |= dflag::TEST;
        }
        let mut class_ctx = self.cur_class;
        // the return type written as the first argument of `MACRO(ret, name, (params))`
        let mut ret_type: Option<(u32, u32)> = None;
        let d;
        match gtest {
            Some((suite, test)) => {
                let name = self.cx.name(test);
                let sid = self.cx.name(suite);
                d = self.cx.def(name, DefKind::Function, span, sig_end);
                let dm = self.cx.def_mut(d);
                dm.qual = sid;
                dm.flags = flags | dflag::TEST;
                self.st.macro_params.clear();
                self.cx.push_def(d);
                if is_fixture_macro(mname) {
                    // the test body is a member of a class derived from the fixture
                    class_ctx = sid;
                    self.cx.add_ref(sid, RefKind::Type, line_of(suite), NONE, 255);
                }
                self.cx.pop_def();
            }
            None => {
                let named = before_group.filter(|_| groups == 1 && nparams >= 3);
                if let Some(x) = named {
                    // the other identifier arguments are types, not parameters of the function
                    let id = self.cx.name(x);
                    self.st.macro_params.clear();
                    if let Some(first) = idents[0].filter(|f| f.id() != x.id()) {
                        let fid = self.cx.name(first);
                        if fid != id {
                            ret_type = Some((fid, line_of(first)));
                        }
                    }
                }
                let name_node = named.or(idents[0]).unwrap_or(macro_node);
                let base = self.cx.text(name_node);
                let name = if mname.starts_with(b"SYSCALL_DEFINE") {
                    self.joined(b"sys_", base)
                } else if mname.starts_with(b"COMPAT_SYSCALL_DEFINE") {
                    self.joined(b"compat_sys_", base)
                } else {
                    self.cx.intern(base)
                };
                let kind = if in_class && !self.in_friend { DefKind::Method } else { DefKind::Function };
                d = self.cx.def(name, kind, span, sig_end);
                let parent = if in_class { self.container } else { self.parent };
                let dm = self.cx.def_mut(d);
                dm.parent = parent;
                dm.flags = flags;
                dm.arity_max = 255;
            }
        }
        let m = self.cx.name(macro_node);
        self.cx.mark(macro_node);
        self.cx.push_def(d);
        self.cx.add_ref(m, RefKind::Call, line_of(macro_node), NONE, 255);
        if let Some((ty, line)) = ret_type {
            self.cx.add_ref(ty, RefKind::Type, line, NONE, 255);
        }
        self.cx.pop_def();
        self.macro_group = group.filter(|_| groups == 1);
        self.function_body(c, d, None, class_ctx, true);
    }

    // ----- declarations -----

    /// A `declaration`, or (`member`) a `field_declaration` of a class body.
    fn declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, member: bool) {
        let t = self.t;
        let ty = child(n, t.f_type);
        if self.sk == Sk::Func && !member {
            return self.local_declaration(n, c, ty);
        }
        let span = self.span.take().unwrap_or(n);
        let (tname, tflags) = match ty {
            Some(x) => self.type_info(x, true),
            None => (NONE, 0),
        };
        let sp = self.specs(n);
        let mut first = NONE;
        let mut push = false;
        let mut last = NONE;
        let mut cc = self.cursor_at(n);
        if cc.goto_first_child() {
            loop {
                if is_field(&cc, t.f_declarator) {
                    let d = cc.node();
                    let (di, is_fn) = self.declare_one(n, span, d, ty, tname, tflags, sp);
                    last = di;
                    if first == NONE && di != NONE {
                        first = di;
                        push = is_fn || !member;
                    }
                } else if member && last != NONE && is_field(&cc, t.f_default_value) {
                    // `int x_ = 0;` / `virtual void f() = 0;`: the member declared just before
                    let v = cc.node();
                    let pure = self.k(v) == K::NumberLiteral && self.cx.text(v) == b"0";
                    let lambda = self.k(v) == K::LambdaExpression;
                    let dm = self.cx.def_mut(last);
                    let end = (v.start_byte() as u32).saturating_sub(dm.sig_off);
                    dm.sig_len = dm.sig_len.min(end.min(u16::MAX as u32) as u16);
                    if dm.kind == DefKind::Method && pure {
                        dm.flags |= dflag::ABSTRACT;
                    } else if dm.kind == DefKind::Field && lambda {
                        dm.flags |= dflag::FNPTR;
                    }
                    push |= last == first;
                }
                if !cc.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(cc);
        let push = push && first != NONE;
        if push {
            self.cx.push_def(first);
        }
        self.st.init_ctx.push(tname);
        self.children(c);
        self.st.init_ctx.pop();
        if push {
            self.cx.pop_def();
        }
    }

    /// Record what one declarator of a file-, namespace- or class-scope declaration declares.
    /// Returns the definition and whether it is a function.
    #[allow(clippy::too_many_arguments)]
    fn declare_one(&mut self, n: Node<'t>, span: Node<'t>, d: Node<'t>, ty: Option<Node<'t>>, tname: u32, tflags: u32, sp: Specs) -> (u32, bool) {
        let t = self.t;
        let in_class = self.sk == Sk::Class;
        let has_init = self.k(d) == K::InitDeclarator;
        let init = if has_init { child(d, t.f_value) } else { None };
        let fd = self.function_declarator(d);
        if let Some((fdecl, term, false)) = fd {
            // function or member function declaration
            let Some((name, qual, leaf)) = self.decl_name(term) else { return (NONE, false) };
            // `Q_OBJECT` and `DECLARE_X(A)` on consecutive lines, neither with a semicolon, read
            // as a declaration of `DECLARE_X` returning `Q_OBJECT`: two macro uses, no definition
            if qual == NONE && self.k(leaf) != K::OperatorName && macro_shaped(self.cx.text(leaf)) {
                if ty.is_some_and(|x| self.k(x) == K::TypeIdentifier && macro_shaped(self.cx.text(x))) {
                    self.cx.add_ref(name, RefKind::Call, line_of(leaf), NONE, 255);
                    self.cx.mark(leaf);
                    return (NONE, false);
                }
            }
            let (kind, parent, dqual, _) = self.function_home(qual, in_class);
            let is_ctor = (qual != NONE && name == qual) || (in_class && qual == NONE && name == self.cur_class);
            if ty.is_none() && qual == NONE && !is_ctor && matches!(self.k(leaf), K::Identifier | K::FieldIdentifier) && self.k(fdecl) == K::FunctionDeclarator
            {
                // Only a constructor is declared without a type. Anything else is a deduction
                // guide (`Foo(int) -> Foo<int>;`) or a macro invoked where a declaration can
                // stand (`DECLARE_GETTER(Name, kind)` in a class body): a use, not a definition.
                let defines_body = self.sk == Sk::Top && block_follows(self.cx.src, n.end_byte());
                if self.is_type_name(name) {
                    self.cx.mark(leaf);
                    return (NONE, false);
                }
                if !defines_body {
                    self.cx.mark(leaf);
                    let nargs = match child(fdecl, t.f_parameters) {
                        Some(params) => self.vexing_list(params, name, 0).min(254) as u8,
                        None => 255,
                    };
                    self.cx.add_ref(name, RefKind::Call, line_of(leaf), NONE, nargs);
                    return (NONE, false);
                }
            }
            let tn = if is_ctor {
                if let Some(x) = ty {
                    self.annotation_type(x);
                }
                name
            } else if self.k(leaf) == K::OperatorCast {
                match child(leaf, t.f_type) {
                    Some(x) => self.type_info(x, true).0,
                    None => NONE,
                }
            } else {
                self.return_type(ty, fdecl)
            };
            let (amin, amax) = self.arity(fdecl);
            let mut flags = dflag::DECL;
            // `virtual void f() = 0;` reaches here when the class body parsed as a block
            if init.is_some_and(|v| self.k(v) == K::NumberLiteral && self.cx.text(v) == b"0") {
                flags |= dflag::ABSTRACT;
            }
            let file_static = sp.is_static && !in_class && qual == NONE && !self.cx.is_header;
            if file_static || self.ctx_local() || (qual != NONE && self.st.local_classes.contains(&qual)) {
                flags |= dflag::LOCAL;
            }
            let di = self.cx.def(name, kind, span, n.end_byte());
            {
                let dm = self.cx.def_mut(di);
                dm.parent = parent;
                dm.qual = dqual;
                dm.flags = flags;
                dm.type_name = tn;
                dm.arity_min = amin;
                dm.arity_max = amax;
            }
            self.cx.mark(leaf);
            if kind == DefKind::Function && !in_class {
                self.st.globals.insert(name);
            }
            if self.sk == Sk::Top && self.k(fdecl) == K::FunctionDeclarator {
                // `int f(void) ATTR { ... }` parses as a declaration followed by a block
                self.pending = Some(Pending { def: di, fdecl: Some(fdecl), after: n.end_byte(), macro_fn: false });
            }
            return (di, true);
        }

        // variable, data member or structured binding
        let term = match fd {
            Some((_, id, _)) => Some(id),
            None => self.declared_ident(d),
        };
        let Some(term) = term else { return (NONE, false) };
        if self.k(term) == K::StructuredBinding {
            return (self.binding_defs(n, span, term, sp), false);
        }
        let Some((name, qual, leaf)) = self.decl_name(term) else { return (NONE, false) };
        // `MACRO(Name)` where a declaration can stand reads as a variable `(Name)` of type
        // `MACRO`: a use of the macro. Only `DEFINE_X(name)` is taken to define `name`.
        let mut macro_made: Option<(u32, u32)> = None;
        if self.k(d) == K::ParenthesizedDeclarator && qual == NONE {
            if let Some(x) = ty.filter(|x| self.k(*x) == K::TypeIdentifier && macro_shaped(self.cx.text(*x)) && !self.cx.is_marked(*x)) {
                let m = self.cx.name(x);
                self.cx.mark(x);
                if !is_define_macro(self.cx.text(x)) || type_like(self.cx.text(leaf)) {
                    self.cx.add_ref(m, RefKind::Call, line_of(x), NONE, 1);
                    self.cx.add_ref(name, RefKind::Arg, line_of(leaf), m, 255);
                    self.cx.mark(leaf);
                    return (NONE, false);
                }
                macro_made = Some((m, line_of(x)));
            }
        }
        // `ALWAYS_INLINE USED` cut off from the declaration it annotates by a preprocessor
        // directive reads as a variable `USED` of type `ALWAYS_INLINE`: two macro uses
        if !has_init && qual == NONE && allcaps(self.cx.text(leaf)) && last_child_missing(n) {
            if let Some(x) = ty.filter(|x| self.k(*x) == K::TypeIdentifier && macro_shaped(self.cx.text(*x))) {
                self.annotation_type(x);
                self.cx.add_ref(name, RefKind::Read, line_of(leaf), NONE, 255);
                self.cx.mark(leaf);
                return (NONE, false);
            }
        }
        let local = (sp.is_static && !in_class && !self.cx.is_header) || self.ctx_local() || (qual != NONE && self.st.local_classes.contains(&qual));
        let di;
        if let Some(lam) = init.filter(|v| self.k(*v) == K::LambdaExpression && !in_class) {
            // a lambda bound to a name is a function
            let sig_end = child(lam, t.f_body).map(|b| b.start_byte()).unwrap_or(lam.end_byte());
            let (amin, amax) = match child(lam, t.f_declarator) {
                Some(f) => self.arity(f),
                None => (0, 0),
            };
            di = self.cx.def(name, DefKind::Function, span, sig_end);
            let parent = self.parent;
            let dm = self.cx.def_mut(di);
            dm.parent = parent;
            dm.arity_min = amin;
            dm.arity_max = amax;
            if local {
                dm.flags |= dflag::LOCAL;
            }
            self.st.globals.insert(name);
        } else {
            let member = in_class || (qual != NONE && !self.is_namespace(qual));
            let kind = if member {
                DefKind::Field
            } else if sp.is_constexpr || (sp.is_const && !self.is_pointer(d)) {
                DefKind::Constant
            } else {
                DefKind::Variable
            };
            let sig_end = init.map(|v| v.start_byte()).unwrap_or(n.end_byte());
            let mut flags = 0u8;
            if local {
                flags |= dflag::LOCAL;
            }
            if macro_made.is_some() {
                flags |= dflag::MACRO_DEFINED;
            }
            if sp.is_extern && !has_init && !member {
                flags |= dflag::DECL;
            }
            let callable = tflags & GENERIC != 0 && tname != NONE && is_function_wrapper(self.cx.name_bytes(tname));
            if fd.is_some() || callable {
                flags |= dflag::FNPTR;
            }
            let ar = match fd {
                Some((f, _, _)) => self.arity(f),
                None => (0, 0),
            };
            di = self.cx.def(name, kind, span, sig_end);
            let (parent, dqual) = if qual != NONE {
                (NONE, qual)
            } else if in_class {
                (self.container, NONE)
            } else {
                (self.parent, NONE)
            };
            let dm = self.cx.def_mut(di);
            dm.parent = parent;
            dm.qual = dqual;
            dm.flags = flags;
            dm.type_name = if macro_made.is_some() { NONE } else { tname };
            dm.arity_min = ar.0;
            dm.arity_max = ar.1;
            if let Some((m, line)) = macro_made {
                // the use of the defining macro belongs to what it defines
                self.cx.push_def(di);
                self.cx.add_ref(m, RefKind::Call, line, NONE, 1);
                self.cx.pop_def();
            }
            if in_class && self.cur_class != NONE {
                self.st.fields.insert(Self::field_key(self.cur_class, name));
            } else if !member {
                self.st.globals.insert(name);
            }
        }
        self.cx.mark(leaf);
        // `T x(args);` / `T x{args};` construct a T
        if let Some(v) = init.filter(|v| matches!(self.k(*v), K::ArgumentList | K::InitializerList)) {
            self.cx.push_def(di);
            {
                let nargs = self.count_args(v);
                self.new_ref(ty, nargs);
            }
            self.cx.pop_def();
        }
        (di, false)
    }

    /// `auto [a, b] = f();` at file or namespace scope: one variable per name.
    fn binding_defs(&mut self, n: Node<'t>, span: Node<'t>, binding: Node<'t>, sp: Specs) -> u32 {
        let mut first = NONE;
        let local = (sp.is_static && !self.cx.is_header) || self.ctx_local();
        let mut c = self.cursor_at(binding);
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if self.k(ch) == K::Identifier && !Self::is_empty(ch) {
                    let name = self.cx.name(ch);
                    let di = self.cx.def(name, DefKind::Variable, span, n.end_byte());
                    let parent = self.parent;
                    let dm = self.cx.def_mut(di);
                    dm.parent = parent;
                    if local {
                        dm.flags |= dflag::LOCAL;
                    }
                    self.cx.mark(ch);
                    if first == NONE {
                        first = di;
                    }
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(c);
        first
    }

    /// A declaration inside a function body: locals, with their types.
    fn local_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, ty: Option<Node<'t>>) {
        let t = self.t;
        let (tname, tflags) = match ty {
            Some(x) => self.type_info(x, true),
            None => (NONE, 0),
        };
        let auto = ty.is_some_and(|x| self.k(x) == K::PlaceholderType);
        // `if (auto *p = f())`: the value hangs off the declaration itself
        let cond_value = child(n, t.f_value);
        let mut lam_def = NONE;
        let mut cc = self.cursor_at(n);
        if cc.goto_first_child() {
            loop {
                if is_field(&cc, t.f_declarator) {
                    let d = cc.node();
                    let init = if self.k(d) == K::InitDeclarator { child(d, t.f_value) } else { cond_value };
                    match self.function_declarator(d) {
                        Some((fdecl, term, false)) if self.k(term) == K::Identifier && self.k(fdecl) == K::FunctionDeclarator => {
                            // `Foo x(a, b);` parses as a function declaration: in a body it is a
                            // variable constructed from arguments, unless the "arguments" can
                            // only be parameters (`extern int f(void);`, `int g(int n);`).
                            if self.args_not_params(fdecl) {
                                let nargs = self.vexing_args(fdecl);
                                if !Self::is_empty(term) {
                                    let nm = self.cx.name(term);
                                    self.cx.declare(nm, enc(tname, tflags));
                                    self.cx.mark(term);
                                }
                                self.new_ref(ty, nargs);
                            } else {
                                // a function declared locally is not a local: calls of it are
                                // references
                                self.cx.mark(term);
                            }
                        }
                        _ => {
                            let lam = init.filter(|v| self.k(*v) == K::LambdaExpression);
                            let id = self.declared_ident(d).filter(|x| self.k(*x) == K::Identifier && !Self::is_empty(*x));
                            if let (Some(lam), Some(id)) = (lam, id) {
                                // a named local lambda is a nested function
                                let name = self.cx.name(id);
                                let sig_end = child(lam, t.f_body).map(|b| b.start_byte()).unwrap_or(lam.end_byte());
                                let (amin, amax) = match child(lam, t.f_declarator) {
                                    Some(f) => self.arity(f),
                                    None => (0, 0),
                                };
                                let di = self.cx.def(name, DefKind::Function, n, sig_end);
                                let parent = self.parent;
                                let dm = self.cx.def_mut(di);
                                dm.parent = parent;
                                dm.flags = dflag::LOCAL;
                                dm.arity_min = amin;
                                dm.arity_max = amax;
                                self.cx.declare(name, LAMBDA);
                                self.cx.mark(id);
                                if lam_def == NONE {
                                    lam_def = di;
                                }
                            } else {
                                let ty_enc = if auto {
                                    match init {
                                        Some(v) => self.infer(v),
                                        None => NONE,
                                    }
                                } else {
                                    enc(tname, tflags)
                                };
                                self.declare_declarator(d, ty_enc);
                                if let Some(v) = init.filter(|v| !auto && matches!(self.k(*v), K::ArgumentList | K::InitializerList)) {
                                    {
                                        let nargs = self.count_args(v);
                                        self.new_ref(ty, nargs);
                                    }
                                }
                            }
                        }
                    }
                }
                if !cc.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(cc);
        if lam_def != NONE {
            self.cx.push_def(lam_def);
        }
        self.st.init_ctx.push(tname);
        self.children(c);
        self.st.init_ctx.pop();
        if lam_def != NONE {
            self.cx.pop_def();
        }
    }

    /// Whether every "parameter" of a declarator could be an expression (a bare or qualified
    /// name, or a call of one with such arguments), and there is at least one.
    fn args_not_params(&mut self, fdecl: Node<'t>) -> bool {
        match child(fdecl, self.t.f_parameters) {
            Some(params) => self.exprs_not_params(params, 0) == Some(true),
            None => false,
        }
    }

    /// `Some(any)` when every parameter in the list could be an expression, `None` otherwise.
    fn exprs_not_params(&mut self, params: Node<'t>, depth: u32) -> Option<bool> {
        let t = self.t;
        let mut any = false;
        let mut ok = true;
        let mut c = self.cursor_at(params);
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if ch.is_named() && self.k(ch) != K::Comment {
                    any = true;
                    let ty = if self.k(ch) == K::ParameterDeclaration { child(ch, t.f_type) } else { None };
                    let named = ty.is_some_and(|x| matches!(self.k(x), K::TypeIdentifier | K::QualifiedIdentifier | K::TemplateType));
                    ok = named
                        && match child(ch, t.f_declarator) {
                            // a lone `Foo<int>` can only be a type
                            None => ch.child_count() == 1 && ty.is_some_and(|x| self.k(x) != K::TemplateType),
                            // `f()` / `f(a, g())` read as a parameter of function type: in a
                            // body it is a call
                            Some(d) => {
                                depth < 4
                                    && ch.child_count() == 2
                                    && self.k(d) == K::AbstractFunctionDeclarator
                                    && d.child_count() == 1
                                    && child(d, t.f_parameters).is_some_and(|p| self.exprs_not_params(p, depth + 1).is_some())
                            }
                        };
                    if !ok {
                        break;
                    }
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(c);
        ok.then_some(any)
    }

    /// The "parameters" of `Foo x(a, b);` inside a body are constructor arguments. A lone
    /// identifier parsed as a type name is a value: emit it as one. Returns the argument count.
    fn vexing_args(&mut self, fdecl: Node<'t>) -> usize {
        match child(fdecl, self.t.f_parameters) {
            Some(params) => self.vexing_list(params, NONE, 0),
            None => 0,
        }
    }

    /// Emit the "parameters" of a list as the expressions they are: names as values (as
    /// arguments of `arg_of` when that is a macro), `f(..)` as calls. Returns their number.
    fn vexing_list(&mut self, params: Node<'t>, arg_of: u32, depth: u32) -> usize {
        let t = self.t;
        let mut count = 0usize;
        let mut c = self.cursor_at(params);
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if ch.is_named() && self.k(ch) != K::Comment {
                    count += 1;
                }
                if self.k(ch) == K::ParameterDeclaration {
                    let ty = child(ch, t.f_type);
                    let inner =
                        child(ch, t.f_declarator).filter(|d| depth < 4 && self.k(*d) == K::AbstractFunctionDeclarator).and_then(|d| child(d, t.f_parameters));
                    let lone = child(ch, t.f_declarator).is_none();
                    if let Some(args) = inner {
                        // `f(a, b)`: a call
                        let nargs = self.vexing_list(args, NONE, depth + 1).min(254) as u8;
                        match ty {
                            Some(x) if self.k(x) == K::TypeIdentifier && !Self::is_empty(x) && !self.cx.is_marked(x) => {
                                self.call_ident(x, nargs);
                                self.cx.mark(x);
                            }
                            // `cast<T>(x)`: a call of a function template
                            Some(x) if self.k(x) == K::TemplateType => {
                                let nm = child(x, t.f_name).filter(|nm| self.k(*nm) == K::TypeIdentifier && !Self::is_empty(*nm) && !self.cx.is_marked(*nm));
                                if let Some(nm) = nm {
                                    let text = self.cx.text(nm);
                                    if is_cast_name(text) || text == b"isa" {
                                        let saved = std::mem::replace(&mut self.cur_class, NONE);
                                        self.call_ident(nm, nargs);
                                        self.cur_class = saved;
                                    } else {
                                        self.call_ident(nm, nargs);
                                    }
                                    self.cx.mark(nm);
                                }
                            }
                            Some(q) if self.k(q) == K::QualifiedIdentifier => {
                                let (last, qual) = self.split_qualified(q);
                                if let Some(leaf) = last.filter(|x| self.k(*x) == K::TypeIdentifier && !Self::is_empty(*x) && !self.cx.is_marked(*x)) {
                                    let name = self.cx.name(leaf);
                                    self.cx.add_ref(name, RefKind::Call, line_of(leaf), qual, nargs);
                                    self.cx.mark(leaf);
                                }
                            }
                            _ => {}
                        }
                    } else if let Some(ty) = ty.filter(|x| lone && self.k(*x) == K::TypeIdentifier && !Self::is_empty(*x)) {
                        if !self.cx.is_marked(ty) {
                            let name = self.cx.name(ty);
                            self.cx.mark(ty);
                            match self.local_of(name) {
                                Some(LAMBDA) => self.cx.add_ref(name, RefKind::Read, line_of(ty), NONE, 255),
                                Some(_) => {}
                                None if arg_of != NONE => self.cx.add_ref(name, RefKind::Arg, line_of(ty), arg_of, 255),
                                None => self.value_ref(name, line_of(ty), false),
                            }
                        }
                    } else if let Some(q) = ty.filter(|x| lone && self.k(*x) == K::QualifiedIdentifier) {
                        // `Foo x(Kind::Value);`: a qualified value, not a type
                        let (last, qual) = self.split_qualified(q);
                        if let Some(leaf) = last.filter(|x| self.k(*x) == K::TypeIdentifier && !Self::is_empty(*x) && !self.cx.is_marked(*x)) {
                            let name = self.cx.name(leaf);
                            self.cx.add_ref(name, RefKind::Read, line_of(leaf), qual, 255);
                            self.cx.mark(leaf);
                        }
                    }
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(c);
        count
    }

    /// Instantiation of the type `ty` with `nargs` arguments.
    fn new_ref(&mut self, ty: Option<Node<'t>>, nargs: usize) {
        let Some(ty) = ty else { return };
        let Some((leaf, qual)) = self.type_head(ty) else { return };
        if Self::is_empty(leaf) || self.cx.is_marked(leaf) {
            return;
        }
        let name = self.cx.name(leaf);
        if self.is_local_type(name) {
            return;
        }
        self.cx.add_ref(name, RefKind::New, line_of(leaf), qual, nargs.min(254) as u8);
        self.cx.mark(leaf);
    }

    /// Type of an `auto` local from its initialiser (encoded), `NONE` when it does not say.
    fn infer(&mut self, v: Node<'t>) -> u32 {
        let t = self.t;
        let mut v = v;
        for _ in 0..4 {
            match v.named_child(0) {
                Some(x) if self.k(v) == K::ParenthesizedExpression => v = x,
                _ => break,
            }
        }
        match self.k(v) {
            K::NewExpression => match child(v, t.f_type) {
                Some(x) => self.type_info(x, false).0,
                None => NONE,
            },
            K::CompoundLiteral => match child(v, t.f_type) {
                Some(x) => {
                    let (id, fl) = self.type_info(x, true);
                    enc(id, fl)
                }
                None => NONE,
            },
            K::CastExpression => match child(v, t.f_type) {
                Some(x) => self.type_info(x, false).0,
                None => NONE,
            },
            K::This => self.cur_class,
            K::PointerExpression => match child(v, t.f_argument) {
                Some(a) if self.k(a) == K::This => self.cur_class,
                _ => NONE,
            },
            K::CallExpression => {
                let (id, fl) = self.call_result(v);
                if id != NONE { enc(id, fl) } else { self.infer_path(v) }
            }
            K::FieldExpression | K::Identifier | K::QualifiedIdentifier | K::SubscriptExpression => self.infer_path(v),
            _ => NONE,
        }
    }

    /// `auto x = a.b();` / `auto x = a.b;`: the local stands for the expression it is initialised
    /// with, so its "type" is that expression's type path (`A::b()`, `A::b`).
    fn infer_path(&mut self, v: Node<'t>) -> u32 {
        let q = self.qual_of(v, Acc::Dot);
        if q == NONE {
            return NONE;
        }
        // chains of such locals must not grow without bound
        let path = self.cx.name_bytes(q);
        if path.len() > 96 || memchr::memmem::find_iter(path, b"::").count() > 3 { NONE } else { q }
    }

    /// Result type of a call when the call itself names it: `static_cast<T>(x)`, `dyn_cast<T>(x)`,
    /// `std::make_unique<T>(..)`, `T(..)` for a type defined in this file.
    fn call_result(&mut self, call: Node<'t>) -> (u32, u32) {
        let t = self.t;
        let Some(f) = child(call, t.f_function) else { return (NONE, 0) };
        let f = match self.k(f) {
            K::QualifiedIdentifier => match self.qualified_last(f) {
                Some(x) => x,
                None => return (NONE, 0),
            },
            _ => f,
        };
        match self.k(f) {
            K::TemplateFunction => {
                let Some(nm) = child(f, t.f_name) else { return (NONE, 0) };
                let name = self.cx.text(nm);
                let smart = is_make_smart(name);
                if !smart && !is_cast_name(name) {
                    return (NONE, 0);
                }
                let Some(arg) = child(f, t.f_arguments).and_then(|a| a.named_child(0)).filter(|a| self.k(*a) == K::TypeDescriptor) else {
                    return (NONE, 0);
                };
                let id = self.type_info(arg, false).0;
                (id, if smart || contains(name, b"pointer_cast") { SMART } else { 0 })
            }
            K::Identifier => {
                if Self::is_empty(f) {
                    return (NONE, 0);
                }
                let id = self.cx.name(f);
                if self.is_type_name(id) && self.local_of(id).is_none() { (id, 0) } else { (NONE, 0) }
            }
            _ => (NONE, 0),
        }
    }

    fn type_definition(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let ty = child(n, t.f_type);
        let span = self.span.take().unwrap_or(n);
        if self.sk == Sk::Func {
            // a function-local alias: a local type, standing for what it names
            let (target, tflags) = match ty {
                Some(x) => self.type_info(x, true),
                None => (NONE, 0),
            };
            let mut cc = self.cursor_at(n);
            if cc.goto_first_child() {
                loop {
                    if is_field(&cc, t.f_declarator) {
                        let d = cc.node();
                        if let Some(id) = self.declared_ident(d).filter(|x| self.k(*x) == K::TypeIdentifier && !Self::is_empty(*x)) {
                            let nm = self.cx.name(id);
                            let entry = if id.id() == d.id() {
                                alias_of(target, tflags)
                            } else if self.is_pointer(d) && tflags & SMART == 0 {
                                alias_of(target, 0)
                            } else {
                                TPARAM
                            };
                            self.cx.declare(nm, entry);
                            self.cx.mark(id);
                        }
                    }
                    if !cc.goto_next_sibling() {
                        break;
                    }
                }
            }
            self.release(cc);
            return self.children(c);
        }
        let (tname, tflags) = match ty {
            Some(x) => self.type_info(x, true),
            None => (NONE, 0),
        };
        let body = ty.and_then(|x| child(x, t.f_body));
        let anon_body = body.is_some() && ty.is_some_and(|x| child(x, t.f_name).is_none_or(Self::is_empty));
        let sig_end = body.map(|b| b.start_byte()).unwrap_or(n.end_byte());
        let local = self.ctx_local();
        let parent = if self.sk == Sk::Class { self.container } else { self.parent };
        let mut first = NONE;
        let mut first_name = NONE;
        let mut cc = self.cursor_at(n);
        if cc.goto_first_child() {
            loop {
                if is_field(&cc, t.f_declarator) {
                    let d = cc.node();
                    if let Some(id) = self.declared_ident(d).filter(|x| self.k(*x) == K::TypeIdentifier && !Self::is_empty(*x)) {
                        let nm = self.cx.name(id);
                        if id.id() == d.id() && body.is_none() && ty.is_some_and(|x| !self.is_dependent(x)) {
                            self.note_class_alias(nm, tname, tflags);
                        }
                        let di = self.cx.def(nm, DefKind::TypeAlias, span, sig_end);
                        let dm = self.cx.def_mut(di);
                        dm.parent = parent;
                        dm.type_name = tname;
                        if local {
                            dm.flags |= dflag::LOCAL;
                        }
                        self.cx.mark(id);
                        self.st.known_types.insert(nm);
                        if first == NONE {
                            first = di;
                            first_name = nm;
                        }
                    }
                }
                if !cc.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(cc);
        if first == NONE {
            return self.children(c);
        }
        self.cx.push_def(first);
        let saved = (self.container, self.cur_class);
        if anon_body {
            // `typedef struct { ... } T;`: the members belong to T
            self.container = first;
            self.cur_class = first_name;
        }
        self.children(c);
        (self.container, self.cur_class) = saved;
        self.cx.pop_def();
    }

    fn alias_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let span = self.span.take().unwrap_or(n);
        let Some(nm) = child(n, t.f_name).filter(|x| !Self::is_empty(*x)) else { return self.children(c) };
        let name = self.cx.name(nm);
        if self.sk == Sk::Func {
            // `using T = Foo<int>;` in a body: a local type, standing for what it names
            let entry = match child(n, t.f_type) {
                Some(x) => {
                    let (target, tflags) = self.type_info(x, true);
                    // `using P = Foo *;`: the alias is a pointer to its target
                    match child(x, t.f_declarator) {
                        None => alias_of(target, tflags),
                        Some(d) if self.k(d) == K::AbstractPointerDeclarator && tflags & SMART == 0 => alias_of(target, 0),
                        Some(_) => TPARAM,
                    }
                }
                None => TPARAM,
            };
            self.cx.declare(name, entry);
            return self.children_except(c, t.f_name);
        }
        let (tname, tflags) = match child(n, t.f_type) {
            Some(x) => {
                let plain = child(x, t.f_declarator).is_none();
                let (id, fl) = self.type_info(x, true);
                // `using P = Foo *;` is a pointer: no smart-pointer or template semantics
                if plain {
                    (id, fl)
                } else if fl & SMART != 0 {
                    (NONE, 0)
                } else {
                    (id, 0)
                }
            }
            None => (NONE, 0),
        };
        if child(n, t.f_type).is_some_and(|x| !self.is_dependent(x)) {
            self.note_class_alias(name, tname, tflags);
        }
        let d = self.cx.def(name, DefKind::TypeAlias, span, n.end_byte());
        let parent = if self.sk == Sk::Class { self.container } else { self.parent };
        let local = self.ctx_local();
        let dm = self.cx.def_mut(d);
        dm.parent = parent;
        dm.type_name = tname;
        if local {
            dm.flags |= dflag::LOCAL;
        }
        self.st.known_types.insert(name);
        self.cx.push_def(d);
        self.children_except(c, t.f_name);
        self.cx.pop_def();
    }

    fn record_specifier(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: DefKind) {
        let t = self.t;
        let name_node = child(n, t.f_name).filter(|x| !Self::is_empty(*x));
        let body = child(n, t.f_body);
        let Some(b) = body else {
            // a use of the type (`struct foo *p`, `class Foo;`): its name is visited as a type
            let saved = std::mem::replace(&mut self.elaborated, true);
            self.children(c);
            self.elaborated = saved;
            return;
        };
        let named = name_node.and_then(|nm| self.decl_name(nm));
        let Some((name, qual, leaf)) = named else {
            // Anonymous record: its members belong to whatever encloses it.
            self.span = None;
            if kind == DefKind::Enum {
                return self.children(c);
            }
            let saved = self.sk;
            self.sk = Sk::Class;
            self.children(c);
            self.sk = saved;
            return;
        };
        let span = self.span.take().unwrap_or(n);
        let d = self.cx.def(name, kind, span, b.start_byte());
        let local = self.ctx_local() || (qual != NONE && self.st.local_classes.contains(&qual));
        {
            let parent = self.parent;
            let dm = self.cx.def_mut(d);
            if qual != NONE {
                dm.qual = qual;
            } else {
                dm.parent = parent;
            }
            if local {
                dm.flags |= dflag::LOCAL;
            }
        }
        self.cx.mark(leaf);
        self.st.known_types.insert(name);
        if local && !self.st.local_classes.contains(&name) {
            self.st.local_classes.push(name);
        }
        let saved = (self.sk, self.parent, self.container, self.cur_class, self.in_friend);
        self.parent = d;
        self.container = d;
        self.in_friend = false;
        if kind != DefKind::Enum {
            self.sk = Sk::Class;
            self.cur_class = name;
            self.prescan_fields(b, name, 0);
        }
        self.cx.push_def(d);
        self.children(c);
        self.cx.pop_def();
        (self.sk, self.parent, self.container, self.cur_class, self.in_friend) = saved;
    }

    /// Data members of a class body, collected before its inline member functions are read:
    /// a member may be used above its declaration.
    fn prescan_fields(&mut self, body: Node<'t>, class: u32, depth: u32) {
        let t = self.t;
        let mut c = self.cursor_at(body);
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if self.k(ch) == K::FieldDeclaration {
                    let mut any = false;
                    let mut cc = self.cursor_at(ch);
                    if cc.goto_first_child() {
                        loop {
                            if is_field(&cc, t.f_declarator) {
                                any = true;
                                let d = cc.node();
                                if self.function_declarator(d).is_none_or(|(_, _, ptr)| ptr) {
                                    if let Some(id) = self.declared_ident(d).filter(|x| self.k(*x) == K::FieldIdentifier && !Self::is_empty(*x)) {
                                        let nm = self.cx.name(id);
                                        self.st.fields.insert(Self::field_key(class, nm));
                                    }
                                }
                            }
                            if !cc.goto_next_sibling() {
                                break;
                            }
                        }
                    }
                    self.release(cc);
                    if !any && depth < 3 {
                        // anonymous struct / union: its members are members of this class
                        let inner = child(ch, t.f_type)
                            .filter(|x| matches!(self.k(*x), K::StructSpecifier | K::UnionSpecifier) && child(*x, t.f_name).is_none())
                            .and_then(|x| child(x, t.f_body));
                        if let Some(inner) = inner {
                            self.prescan_fields(inner, class, depth + 1);
                        }
                    }
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(c);
    }

    /// `: public Base, private ns::Other<int>`: the enclosing definition is the class.
    fn base_clause(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let n = c.node();
        let class = self.cur_class;
        let mut cc = self.cursor_at(n);
        if cc.goto_first_child() {
            loop {
                let ch = cc.node();
                let base = match self.k(ch) {
                    K::TypeIdentifier => Some((ch, NONE)),
                    K::TemplateType => child(ch, t.f_name).map(|x| (x, NONE)),
                    K::QualifiedIdentifier => {
                        let (last, qual) = self.split_qualified(ch);
                        last.and_then(|l| self.leaf_name(l)).filter(|x| self.k(*x) == K::TypeIdentifier).map(|x| (x, qual))
                    }
                    _ => None,
                };
                if let Some((leaf, qual)) = base.filter(|(x, _)| !Self::is_empty(*x)) {
                    let name = self.cx.name(leaf);
                    if !self.is_local_type(name) {
                        self.cx.add_ref(name, RefKind::Extends, line_of(leaf), qual, 255);
                        if class != NONE {
                            self.st.bases.push((class, name));
                        }
                    }
                    self.cx.mark(leaf);
                }
                if !cc.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(cc);
        self.children(c);
    }

    fn enumerator(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        if let Some(nm) = child(n, t.f_name).filter(|x| !Self::is_empty(*x)) {
            let name = self.cx.name(nm);
            let d = self.cx.def(name, DefKind::EnumMember, n, n.end_byte());
            let local = self.ctx_local();
            // an unnamed enumeration outside a class: its enumerators belong to what encloses it
            let container = if self.container == NONE { self.parent } else { self.container };
            let dm = self.cx.def_mut(d);
            dm.parent = container;
            if local {
                dm.flags |= dflag::LOCAL;
            }
            if self.sk == Sk::Top {
                self.st.globals.insert(name);
            }
        }
        self.children_except(c, t.f_name);
    }

    fn namespace_definition(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let sig_end = child(n, t.f_body).map(|b| b.start_byte()).unwrap_or(n.end_byte());
        let saved = (self.parent, self.anon_ns);
        self.span = None;
        let mut pushed = 0u32;
        match child(n, t.f_name) {
            Some(nm) => self.namespace_names(nm, n, sig_end, &mut pushed, 0),
            None => self.anon_ns += 1,
        }
        self.children_except(c, t.f_name);
        for _ in 0..pushed {
            self.cx.pop_def();
        }
        (self.parent, self.anon_ns) = saved;
    }

    /// `namespace a::b { }` defines `a` and, inside it, `b`.
    fn namespace_names(&mut self, nm: Node<'t>, n: Node<'t>, sig_end: usize, pushed: &mut u32, depth: u32) {
        match self.k(nm) {
            K::NamespaceIdentifier => {
                if Self::is_empty(nm) {
                    return;
                }
                let name = self.cx.name(nm);
                let d = self.cx.def(name, DefKind::Namespace, n, sig_end);
                self.cx.def_mut(d).parent = self.parent;
                self.parent = d;
                self.cx.push_def(d);
                *pushed += 1;
                self.add_namespace(name);
            }
            K::NestedNamespace if depth < 16 => {
                for i in 0..nm.named_child_count().min(64) as u32 {
                    if let Some(x) = nm.named_child(i) {
                        self.namespace_names(x, n, sig_end, pushed, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }

    /// `namespace fs = std::filesystem;`: an import of the namespace under a local name.
    fn namespace_alias(&mut self, n: Node<'t>) {
        let t = self.t;
        let Some(nm) = child(n, t.f_name).filter(|x| !Self::is_empty(*x)) else { return };
        let Some(target) = last_named(n).filter(|x| x.id() != nm.id()) else { return };
        let alias = self.cx.name(nm);
        // the target as written, without whitespace
        let tid = self.squeezed(target);
        let sys = self.cx.name_bytes(tid).starts_with(b"std");
        self.cx.add_import(tid, NONE, alias, line_of(n), if sys { iflag::SYSTEM } else { 0 });
        self.st.ns_aliases.push((alias, tid));
        self.add_namespace(alias);
    }

    /// `using namespace a::b;`, `using a::b::c;`, `using enum E;`
    fn using_declaration(&mut self, n: Node<'t>) {
        let Some(target) = n.named_child(0) else { return };
        let (mut is_ns, mut is_enum) = (false, false);
        if let Some(kw) = n.child(1).filter(|x| !x.is_named()) {
            match self.cx.text(kw) {
                b"namespace" => is_ns = true,
                b"enum" => is_enum = true,
                _ => {}
            }
        }
        let line = line_of(n);
        match self.k(target) {
            K::Identifier => {
                if Self::is_empty(target) || !(is_ns || is_enum) {
                    return;
                }
                let id = self.cx.name(target);
                let sys = self.cx.text(target) == b"std";
                self.cx.add_import(id, NONE, NONE, line, iflag::WILDCARD | if sys { iflag::SYSTEM } else { 0 });
                if is_ns {
                    self.add_namespace(id);
                } else {
                    self.cx.add_ref(id, RefKind::Type, line, NONE, 255);
                }
            }
            K::QualifiedIdentifier => {
                // an import: the scope is its target, kept as written
                let (last, scope, _) = self.split_scopes(target);
                let Some(leaf) = last.and_then(|l| self.leaf_name(l)).filter(|x| !Self::is_empty(*x)) else { return };
                let name = match self.k(leaf) {
                    K::Identifier | K::TypeIdentifier | K::FieldIdentifier => self.cx.name(leaf),
                    K::OperatorName => self.op_name(leaf),
                    _ => return,
                };
                let sys = scope != NONE && is_std_scope(self.cx.name_bytes(scope));
                let sysf = if sys { iflag::SYSTEM } else { 0 };
                if is_ns || is_enum {
                    // the whole path names the namespace / enumeration
                    let full = if scope == NONE {
                        name
                    } else {
                        let mut buf = std::mem::take(&mut self.cx.s.buf);
                        buf.clear();
                        buf.extend_from_slice(self.cx.name_bytes(scope));
                        buf.extend_from_slice(b"::");
                        buf.extend_from_slice(self.cx.name_bytes(name));
                        let id = self.cx.intern(&buf);
                        self.cx.s.buf = buf;
                        id
                    };
                    self.cx.add_import(full, NONE, NONE, line, iflag::WILDCARD | sysf);
                    if is_ns {
                        self.add_namespace(name);
                    } else {
                        self.cx.add_ref(name, RefKind::Type, line, scope, 255);
                    }
                } else {
                    let target_id = if scope == NONE { self.cx.intern(b"") } else { scope };
                    self.cx.add_import(target_id, name, NONE, line, sysf);
                    self.cx.add_ref(name, RefKind::Import, line, scope, 255);
                    if self.sk != Sk::Class {
                        self.st.globals.insert(name);
                    }
                }
            }
            _ => {}
        }
    }

    fn template_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let owned = self.span.is_none();
        if owned {
            self.span = Some(n);
        }
        self.cx.push_scope();
        self.children(c);
        self.cx.pop_scope();
        if owned {
            self.span = None;
        }
    }

    fn template_parameter_list(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        let t = self.t;
        self.cx.depth += 1;
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if matches!(self.k(ch), K::ParameterDeclaration | K::OptionalParameterDeclaration | K::VariadicParameterDeclaration) {
                    // a non-type parameter: local as a value and, in template arguments, as a type
                    let id = child(ch, t.f_declarator).and_then(|d| self.declared_ident(d)).filter(|x| self.k(*x) == K::Identifier && !Self::is_empty(*x));
                    if let Some(id) = id {
                        let nm = self.cx.name(id);
                        self.cx.declare(nm, TPARAM);
                        self.cx.mark(id);
                    }
                }
                self.visit(c);
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.cx.depth -= 1;
    }

    /// `typename T`, `class... Ts`
    fn type_parameter(&mut self, n: Node<'t>) {
        if let Some(id) = n.named_child(0).filter(|x| self.k(*x) == K::TypeIdentifier && !Self::is_empty(*x)) {
            let nm = self.cx.name(id);
            self.cx.declare(nm, TPARAM);
        }
    }

    /// `typename T = Default`
    fn optional_type_parameter(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        if let Some(id) = child(n, t.f_name).filter(|x| !Self::is_empty(*x)) {
            let nm = self.cx.name(id);
            self.cx.declare(nm, TPARAM);
        }
        self.children_except(c, t.f_name);
    }

    fn concept_definition(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let span = self.span.take().unwrap_or(n);
        let Some(nm) = child(n, t.f_name).filter(|x| !Self::is_empty(*x)) else { return self.children(c) };
        let name = self.cx.name(nm);
        let d = self.cx.def(name, DefKind::Trait, span, n.end_byte());
        let parent = self.parent;
        let local = self.ctx_local();
        let dm = self.cx.def_mut(d);
        dm.parent = parent;
        if local {
            dm.flags |= dflag::LOCAL;
        }
        self.st.known_types.insert(name);
        self.cx.push_def(d);
        self.children_except(c, t.f_name);
        self.cx.pop_def();
    }

    fn friend_declaration(&mut self, c: &mut TreeCursor<'t>) {
        let saved = self.in_friend;
        self.in_friend = true;
        self.children(c);
        self.in_friend = saved;
    }

    fn parameter_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        if let Some(id) = child(n, self.t.f_declarator).and_then(|d| self.declared_ident(d)).filter(|x| self.k(*x) == K::Identifier) {
            self.cx.mark(id);
        }
        self.children(c);
    }

    /// `x_(x)`, `Base(1)`, `y{2}` in a constructor's initialiser list.
    fn field_initializer(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let Some(first) = n.named_child(0) else { return self.children(c) };
        let args = n.named_child(1).filter(|a| matches!(self.k(*a), K::ArgumentList | K::InitializerList));
        let nargs = args.map(|a| self.count_args(a).min(254) as u8).unwrap_or(255);
        let cls = self.cur_class;
        match self.k(first) {
            K::FieldIdentifier if !Self::is_empty(first) => {
                let name = self.cx.name(first);
                let member = self.is_member(cls, name);
                if !member && (self.is_base(cls, name) || self.is_type_name(name)) {
                    // base-class or delegating constructor
                    self.cx.add_ref(name, RefKind::Call, line_of(first), NONE, nargs);
                } else {
                    let at = self.cx.facts.refs.len() as u32;
                    self.cx.add_ref(name, RefKind::MemberWrite, line_of(first), cls, 255);
                    // a single non-local value initialises the member: a binding
                    let single = args.filter(|a| a.named_child_count() == 1).and_then(|a| a.named_child(0));
                    let mut bind = NONE;
                    if let Some(v) = single.and_then(|v| self.bindable(v)) {
                        let slot = self.slot(cls, self.cx.text(first));
                        let vn = self.cx.name(v);
                        bind = self.cx.facts.refs.len() as u32;
                        self.cx.add_ref(vn, RefKind::Bind, line_of(v), slot, 255);
                        self.cx.mark(v);
                    }
                    if !member && cls != NONE && !self.st.known_types.contains(&cls) {
                        // the class is declared elsewhere: this may still be a base class
                        self.st.pending_inits.push((at, bind, nargs));
                    }
                }
            }
            K::TemplateMethod => {
                // `Base<T>(x)`
                if let Some(nm) = child(first, self.t.f_name).filter(|x| self.k(*x) == K::FieldIdentifier && !Self::is_empty(*x)) {
                    let name = self.cx.name(nm);
                    self.cx.add_ref(name, RefKind::Call, line_of(nm), NONE, nargs);
                }
            }
            K::QualifiedIdentifier => {
                let (last, qual) = self.split_qualified(first);
                if let Some(leaf) = last.and_then(|l| self.leaf_name(l)).filter(|x| !Self::is_empty(*x)) {
                    if matches!(self.k(leaf), K::FieldIdentifier | K::Identifier | K::TypeIdentifier) {
                        let name = self.cx.name(leaf);
                        self.cx.add_ref(name, RefKind::Call, line_of(leaf), qual, nargs);
                        self.cx.mark(leaf);
                    }
                }
            }
            _ => {}
        }
        self.children(c);
    }

    // ----- preprocessor -----

    /// The import of an `#include` directive given as text.
    fn include_text(&mut self, dir: &[u8], line: u32) {
        let rest = dir[1..].trim_ascii_start();
        let Some(rest) = rest.strip_prefix(b"include") else { return };
        let rest = rest.trim_ascii_start();
        let (close, sys) = match rest.first() {
            Some(b'"') => (b'"', false),
            Some(b'<') => (b'>', true),
            _ => return,
        };
        let Some(n) = memchr::memchr(close, &rest[1..]) else { return };
        if n == 0 {
            return;
        }
        let target = self.cx.intern(&rest[1..1 + n]);
        self.cx.add_import(target, NONE, NONE, line, if sys { iflag::SYSTEM } else { 0 });
    }

    fn include(&mut self, n: Node<'t>) {
        let Some(p) = child(n, self.t.f_path) else { return };
        let txt = self.cx.text(p);
        if txt.len() < 3 {
            return;
        }
        let sys = match txt[0] {
            b'"' => false,
            b'<' => true,
            _ => return,
        };
        let target = self.cx.intern(&txt[1..txt.len() - 1]);
        self.cx.add_import(target, NONE, NONE, line_of(n), if sys { iflag::SYSTEM } else { 0 });
    }

    /// `#if COND` / `#ifdef NAME`: the condition is not in the scope of an enclosing class.
    fn preproc_if(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH || self.reparse {
            return;
        }
        let t = self.t;
        self.cx.depth += 1;
        if c.goto_first_child() {
            let mut head = true;
            // `#ifndef GUARD_H` at the top of a header is no reference
            let ifndef = self.sk == Sk::Top && self.cx.text(c.node()).ends_with(b"ifndef");
            loop {
                let ch = c.node();
                if head && ch.is_named() {
                    // the condition
                    head = false;
                    let guard = ifndef && self.k(ch) == K::Identifier && is_header_guard(self.cx.text(ch));
                    if !guard && (is_field(c, t.f_condition) || is_field(c, t.f_name)) {
                        let saved = std::mem::replace(&mut self.cur_class, NONE);
                        self.visit(c);
                        self.cur_class = saved;
                    } else if !guard {
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
        self.cx.depth -= 1;
    }

    fn preproc_def(&mut self, n: Node<'t>, function_like: bool) {
        let t = self.t;
        let Some(nm_node) = child(n, t.f_name).filter(|x| !Self::is_empty(*x)) else { return };
        let value = child(n, t.f_value);
        if value.is_none() && !function_like && is_header_guard(self.cx.text(nm_node)) {
            return;
        }
        let name = self.cx.name(nm_node);
        let sig_end = n.end_byte().min(n.start_byte() + 200);
        let d = self.cx.def(name, DefKind::Macro, n, sig_end);
        self.st.globals.insert(name);
        self.st.pp_params.clear();
        if function_like {
            let mut variadic = false;
            if let Some(ps) = child(n, t.f_parameters) {
                let mut pc = self.cursor_at(ps);
                if pc.goto_first_child() {
                    loop {
                        let ch = pc.node();
                        if self.k(ch) == K::Identifier {
                            let id = self.cx.name(ch);
                            self.st.pp_params.push(id);
                        } else if self.cx.text(ch) == b"..." {
                            variadic = true;
                        }
                        if !pc.goto_next_sibling() {
                            break;
                        }
                    }
                }
                self.release(pc);
            }
            let a = self.st.pp_params.len().min(254) as u8;
            let dm = self.cx.def_mut(d);
            dm.arity_min = a;
            dm.arity_max = if variadic { 255 } else { a };
        } else {
            // Object-like macros are not callable: arity 0/0 would wrongly match `FOO()`.
            self.cx.def_mut(d).arity_max = 0;
        }
        if let Some(v) = value {
            self.cx.push_def(d);
            self.scan_text(self.cx.text(v), line_of(v));
            self.cx.pop_def();
        }
    }

    /// Macro bodies (and the arguments of blanked annotation macros) are opaque text to the
    /// grammar. A lexical pass recovers the names they use, so that
    /// `#define LLVM_DEBUG(...) DEBUG_WITH_TYPE(DEBUG_TYPE, __VA_ARGS__)` links to both names.
    fn scan_text(&mut self, text: &[u8], first_line: u32) {
        let n = text.len();
        let mut line = first_line;
        let mut i = 0usize;
        let mut prev: u8 = 0;
        let mut prev2: u8 = 0;
        let mut prev_word: &[u8] = b"";
        // scope written before the current word (`ns::` -> `ns`)
        let mut scope: &[u8] = b"";
        while i < n {
            let b = text[i];
            match b {
                b'\n' => {
                    line += 1;
                    i += 1;
                }
                b' ' | b'\t' | b'\r' | b'\\' => i += 1,
                b'"' | b'\'' => {
                    i += 1;
                    while i < n && text[i] != b {
                        if text[i] == b'\\' {
                            i += 1;
                        }
                        if i < n && text[i] == b'\n' {
                            line += 1;
                        }
                        i += 1;
                    }
                    i += 1;
                    prev2 = prev;
                    prev = b;
                    prev_word = b"";
                    scope = b"";
                }
                b'/' if i + 1 < n && text[i + 1] == b'*' => {
                    i += 2;
                    while i + 1 < n && !(text[i] == b'*' && text[i + 1] == b'/') {
                        if text[i] == b'\n' {
                            line += 1;
                        }
                        i += 1;
                    }
                    i += 2;
                }
                b'/' if i + 1 < n && text[i + 1] == b'/' => {
                    while i < n && text[i] != b'\n' {
                        i += 1;
                    }
                }
                _ if b == b'_' || b.is_ascii_alphabetic() || b >= 0x80 => {
                    let s = i;
                    while i < n && (text[i] == b'_' || text[i].is_ascii_alphanumeric() || text[i] >= 0x80) {
                        i += 1;
                    }
                    let word = &text[s..i];
                    let mut j = i;
                    while j < n && (text[j] == b' ' || text[j] == b'\t') {
                        j += 1;
                    }
                    let next = if j < n { text[j] } else { 0 };
                    let next2 = if j + 1 < n { text[j + 1] } else { 0 };
                    let pasted = (prev == b'#' && prev2 == b'#') || (next == b'#' && next2 == b'#');
                    let stringified = prev == b'#' && prev2 != b'#';
                    let member = prev == b'.' || (prev == b'>' && prev2 == b'-');
                    let scoped = prev == b':' && prev2 == b':';
                    let is_scope = next == b':' && next2 == b':';
                    if !pasted && !stringified && !is_cpp_keyword(word) {
                        let id = self.cx.intern(word);
                        let qual = if scoped && !scope.is_empty() && scope != b"std" { self.cx.intern(scope) } else { NONE };
                        if is_scope || matches!(prev_word, b"struct" | b"union" | b"enum" | b"class" | b"typename") {
                            if !self.st.pp_params.contains(&id) {
                                self.cx.add_ref(id, RefKind::Type, line, qual, 255);
                            }
                        } else if member {
                            let k = if next == b'(' { RefKind::MemberCall } else { RefKind::MemberRead };
                            self.cx.add_ref(id, k, line, NONE, 255);
                        } else if !self.st.pp_params.contains(&id) {
                            let k = if prev_word == b"new" {
                                RefKind::New
                            } else if next == b'(' {
                                RefKind::Call
                            } else {
                                RefKind::Read
                            };
                            self.cx.add_ref(id, k, line, qual, 255);
                        }
                    }
                    scope = if is_scope { word } else { b"" };
                    prev_word = word;
                    prev2 = prev;
                    prev = b'a';
                }
                _ if b.is_ascii_digit() => {
                    while i < n && (text[i].is_ascii_alphanumeric() || text[i] == b'.' || text[i] == b'\'') {
                        i += 1;
                    }
                    prev_word = b"";
                    scope = b"";
                    prev2 = prev;
                    prev = b'0';
                }
                _ => {
                    prev2 = prev;
                    prev = b;
                    prev_word = b"";
                    if b != b':' {
                        scope = b"";
                    }
                    i += 1;
                }
            }
        }
    }

    // ----- expressions -----

    fn call_expression(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let nargs = match child(n, t.f_arguments) {
            Some(a) => self.count_args(a).min(254) as u8,
            None => 255,
        };
        if let Some(mut f) = child(n, t.f_function) {
            // `(*ops->fn)(x)`, `(fn)(x)`
            for _ in 0..4 {
                match self.k(f) {
                    K::ParenthesizedExpression => match f.named_child(0) {
                        Some(x) => f = x,
                        None => break,
                    },
                    K::PointerExpression => match child(f, t.f_argument) {
                        Some(x) => f = x,
                        None => break,
                    },
                    _ => break,
                }
            }
            match self.k(f) {
                K::Identifier => {
                    self.call_ident(f, nargs);
                    self.cx.mark(f);
                }
                K::TemplateFunction => {
                    if let Some(nm) = child(f, t.f_name).filter(|x| self.k(*x) == K::Identifier) {
                        // `dyn_cast<T>(x)`, `isa<T>(x)`: free function templates, never members
                        let text = self.cx.text(nm);
                        if is_cast_name(text) || text == b"isa" {
                            let saved = std::mem::replace(&mut self.cur_class, NONE);
                            self.call_ident(nm, nargs);
                            self.cur_class = saved;
                        } else {
                            self.call_ident(nm, nargs);
                        }
                        self.cx.mark(nm);
                        self.make_smart_new(f, nm, nargs);
                    }
                }
                K::QualifiedIdentifier => {
                    let (last, qual) = self.split_qualified(f);
                    if let Some(last) = last {
                        if let Some(leaf) = self.leaf_name(last).filter(|x| !Self::is_empty(*x)) {
                            match self.k(leaf) {
                                K::Identifier | K::FieldIdentifier | K::TypeIdentifier => {
                                    let name = self.cx.name(leaf);
                                    self.cx.add_ref(name, RefKind::Call, line_of(leaf), qual, nargs);
                                    self.cx.mark(leaf);
                                    if self.k(last) == K::TemplateFunction {
                                        self.make_smart_new(last, leaf, nargs);
                                    }
                                }
                                K::OperatorName => {
                                    let name = self.op_name(leaf);
                                    self.cx.add_ref(name, RefKind::Call, line_of(leaf), qual, nargs);
                                }
                                _ => {}
                            }
                        }
                    }
                }
                K::FieldExpression => self.member_ref(f, Some(nargs)),
                _ => {}
            }
            self.arg_refs(f, child(n, t.f_arguments));
        }
        self.children(c);
    }

    /// Non-local names handed to a call as arguments are recorded with the callee's name: for
    /// functions this is how callbacks are registered (`std::sort(a, b, by_area)`,
    /// `pool.submit(worker)`). Names that may be members of the enclosing class stay reads
    /// qualified by that class.
    fn arg_refs(&mut self, callee: Node<'t>, args: Option<Node<'t>>) {
        let t = self.t;
        let Some(a) = args.filter(|a| a.named_child_count() > 0) else { return };
        let leaf = match self.k(callee) {
            K::Identifier => Some(callee),
            K::TemplateFunction => self.leaf_name(callee),
            K::QualifiedIdentifier => self.qualified_last(callee).and_then(|l| self.leaf_name(l)),
            K::FieldExpression => child(callee, t.f_field).and_then(|f| self.leaf_name(f)),
            _ => None,
        };
        let Some(leaf) = leaf.filter(|x| matches!(self.k(*x), K::Identifier | K::FieldIdentifier | K::TypeIdentifier) && !Self::is_empty(*x)) else { return };
        let mut callee_id = NONE;
        let mut ac = self.cursor_at(a);
        if ac.goto_first_child() {
            loop {
                let mut e = ac.node();
                if self.k(e) == K::PointerExpression && child(e, t.f_operator).is_some_and(|o| self.cx.text(o) == b"&") {
                    if let Some(x) = child(e, t.f_argument) {
                        e = x;
                    }
                }
                if self.k(e) == K::Identifier && !Self::is_empty(e) && !self.cx.is_marked(e) {
                    let text = self.cx.text(e);
                    if !reserved(text) {
                        let name = self.cx.intern(text);
                        let free = match self.local_of(name) {
                            Some(LAMBDA) => true,
                            Some(_) => false,
                            None => !self.is_member(self.cur_class, name) && self.implicit_class(name) == NONE,
                        };
                        if free {
                            if callee_id == NONE {
                                callee_id = self.cx.name(leaf);
                            }
                            self.cx.add_ref(name, RefKind::Arg, line_of(e), callee_id, 255);
                            self.cx.mark(e);
                        }
                    }
                }
                if !ac.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(ac);
    }

    /// A call through a bare name.
    fn call_ident(&mut self, f: Node<'t>, nargs: u8) {
        if Self::is_empty(f) {
            return;
        }
        let text = self.cx.text(f);
        if reserved(text) {
            return;
        }
        let name = self.cx.intern(text);
        match self.local_of(name) {
            Some(LAMBDA) => return self.cx.add_ref(name, RefKind::Call, line_of(f), NONE, nargs),
            // a call through a local: a function pointer, a functor, a template parameter
            Some(_) => return,
            None => {}
        }
        if self.is_type_name(name) {
            // `Foo(1)` for the enclosing class, a type defined in this file or a name used as
            // a type before constructs a Foo
            self.cx.add_ref(name, RefKind::New, line_of(f), NONE, nargs);
        } else {
            // an ALL_CAPS callee is a macro, not a member function
            let cls = if macro_shaped(text) { NONE } else { self.implicit_class(name) };
            self.cx.add_ref(name, RefKind::Call, line_of(f), cls, nargs);
        }
    }

    /// `std::make_unique<T>(args)` / `make_shared<T>(args)` construct a T.
    fn make_smart_new(&mut self, tf: Node<'t>, name: Node<'t>, nargs: u8) {
        if !is_make_smart(self.cx.text(name)) {
            return;
        }
        let arg = child(tf, self.t.f_arguments).and_then(|a| a.named_child(0)).filter(|a| self.k(*a) == K::TypeDescriptor);
        if let Some(ty) = arg.and_then(|a| child(a, self.t.f_type)) {
            self.new_ref(Some(ty), nargs as usize);
        }
    }

    fn field_expression(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        self.member_ref(n, None);
        self.children(c);
    }

    /// The member reference of a field expression: a call when `call` carries the argument
    /// count, otherwise a read or a write.
    fn member_ref(&mut self, fe: Node<'t>, call: Option<u8>) {
        let t = self.t;
        let Some(f) = child(fe, t.f_field) else { return };
        // `obj.*pm`: the right side is a value, not a member name
        let op = child(fe, t.f_operator).map(|o| self.cx.text(o)).unwrap_or(b".");
        if op.len() > 1 && op.ends_with(b"*") {
            return;
        }
        let acc = if op == b"->" { Acc::Arrow } else { Acc::Dot };
        let (leaf, scope) = match self.k(f) {
            K::FieldIdentifier => (Some(f), None),
            K::TemplateMethod | K::DependentName => (self.leaf_name(f), None),
            K::QualifiedIdentifier => {
                // `obj.Base::m()`
                let (last, q) = self.split_qualified(f);
                (last.and_then(|l| self.leaf_name(l)), Some(q))
            }
            K::DestructorName if call.is_some() => (Some(f), None),
            _ => (None, None),
        };
        let Some(leaf) = leaf.filter(|x| !Self::is_empty(*x)) else { return };
        if !self.cx.s.handled.is_empty() && self.cx.is_marked(leaf) {
            return;
        }
        let name = match self.k(leaf) {
            K::FieldIdentifier | K::Identifier => self.cx.name(leaf),
            K::OperatorName => self.op_name(leaf),
            K::DestructorName => self.squeezed(leaf),
            _ => return,
        };
        let qual = match scope {
            Some(q) => q,
            None => match child(fe, t.f_argument) {
                Some(a) => self.qual_of(a, acc),
                None => NONE,
            },
        };
        let pos = leaf.start_byte() as u32;
        let kind = match call {
            Some(_) => RefKind::MemberCall,
            None if !self.cx.s.writes.is_empty() && self.cx.s.writes.contains(&pos) => RefKind::MemberWrite,
            None => RefKind::MemberRead,
        };
        self.cx.add_ref(name, kind, line_of(leaf), qual, call.unwrap_or(255));
        if call.is_some() {
            self.cx.mark(leaf);
        }
    }

    /// Type path of an expression used as a receiver, interned; `NONE` when unknown.
    fn qual_of(&mut self, e: Node<'t>, acc: Acc) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let ok = self.qual_path(e, acc, &mut buf, 0);
        let q = if ok && !buf.is_empty() { self.cx.intern(&buf) } else { NONE };
        self.cx.s.buf = buf;
        q
    }

    /// Type a local of encoded type `raw` gives access to when reached through `acc`.
    fn receiver_type(raw: u32, acc: Acc) -> Option<u32> {
        if raw >= SENTINEL || raw & ALIAS != 0 {
            return None;
        }
        let id = raw & TYPE_MASK;
        if raw & SMART != 0 {
            matches!(acc, Acc::Arrow | Acc::Deref).then_some(id)
        } else if raw & GENERIC != 0 {
            (acc == Acc::Dot).then_some(id)
        } else {
            Some(id)
        }
    }

    fn qual_path(&mut self, e: Node<'t>, acc: Acc, buf: &mut Vec<u8>, depth: u32) -> bool {
        if depth > 4 {
            return false;
        }
        let t = self.t;
        match self.k(e) {
            K::Identifier => {
                if Self::is_empty(e) {
                    return false;
                }
                let name = self.cx.name(e);
                if let Some(raw) = self.local_of(name) {
                    if acc == Acc::Dot && raw < SENTINEL && raw & ALIAS == 0 && raw & SMART != 0 {
                        // `Expected<T> e; e.takeError()`: a member of the wrapper class itself
                        return match (raw & WRAP_MASK) >> WRAP_SHIFT {
                            0 => false,
                            w => {
                                buf.extend_from_slice(WRAPPERS[w as usize - 1]);
                                true
                            }
                        };
                    }
                    return match Self::receiver_type(raw, acc) {
                        Some(ty) => {
                            buf.extend_from_slice(self.cx.name_bytes(ty));
                            true
                        }
                        None => false,
                    };
                }
                let cls = self.implicit_class(name);
                if cls != NONE {
                    // not a local inside a member function: taken to be a data member
                    buf.extend_from_slice(self.cx.name_bytes(cls));
                    buf.extend_from_slice(b"::");
                } else {
                    buf.push(b'$');
                }
                buf.extend_from_slice(self.cx.text(e));
                true
            }
            K::This => {
                if self.cur_class == NONE {
                    return false;
                }
                buf.extend_from_slice(self.cx.name_bytes(self.cur_class));
                true
            }
            K::FieldExpression => {
                let (Some(arg), Some(fl)) = (child(e, t.f_argument), child(e, t.f_field)) else { return false };
                if self.k(fl) != K::FieldIdentifier {
                    return false;
                }
                let op = child(e, t.f_operator).map(|o| self.cx.text(o)).unwrap_or(b".");
                if op.len() > 1 && op.ends_with(b"*") {
                    // `a.*pm`: not a member of a
                    return false;
                }
                let inner = if op == b"->" { Acc::Arrow } else { Acc::Dot };
                if !self.qual_path(arg, inner, buf, depth + 1) {
                    return false;
                }
                buf.extend_from_slice(b"::");
                buf.extend_from_slice(self.cx.text(fl));
                true
            }
            K::ParenthesizedExpression => match e.named_child(0) {
                Some(x) => self.qual_path(x, acc, buf, depth + 1),
                None => false,
            },
            K::PointerExpression => {
                let Some(x) = child(e, t.f_argument) else { return false };
                let deref = child(e, t.f_operator).is_none_or(|o| self.cx.text(o) == b"*");
                self.qual_path(x, if deref { Acc::Deref } else { Acc::Dot }, buf, depth + 1)
            }
            K::SubscriptExpression => match child(e, t.f_argument) {
                Some(x) => self.qual_path(x, Acc::Index, buf, depth + 1),
                None => false,
            },
            K::CastExpression | K::NewExpression | K::CompoundLiteral => {
                let ty = match child(e, t.f_type) {
                    Some(x) => self.type_info(x, false).0,
                    None => NONE,
                };
                if ty == NONE {
                    return false;
                }
                buf.extend_from_slice(self.cx.name_bytes(ty));
                true
            }
            K::CallExpression => {
                let (id, fl) = self.call_result(e);
                if id != NONE {
                    return match Self::receiver_type(enc(id, fl), acc) {
                        Some(ty) => {
                            buf.extend_from_slice(self.cx.name_bytes(ty));
                            true
                        }
                        None => false,
                    };
                }
                let Some(f) = child(e, t.f_function) else { return false };
                match self.k(f) {
                    K::Identifier | K::TemplateFunction => {
                        let Some(leaf) = self.leaf_name(f).filter(|x| self.k(*x) == K::Identifier && !Self::is_empty(*x)) else { return false };
                        let text = self.cx.text(leaf);
                        if reserved(text) {
                            return false;
                        }
                        let name = self.cx.intern(text);
                        if self.local_of(name).is_some() {
                            return false;
                        }
                        // inside a class an unqualified call goes through the implicit `this`:
                        // `getContext().getDiagnostics()` is `Class::getContext()`
                        let cls = if macro_shaped(text) { NONE } else { self.implicit_class(name) };
                        if cls != NONE {
                            buf.extend_from_slice(self.cx.name_bytes(cls));
                            buf.extend_from_slice(b"::");
                        }
                        buf.extend_from_slice(text);
                        buf.extend_from_slice(b"()");
                        true
                    }
                    K::QualifiedIdentifier => {
                        let Some(leaf) = self.qualified_last(f).and_then(|l| self.leaf_name(l)).filter(|x| self.k(*x) == K::Identifier && !Self::is_empty(*x))
                        else {
                            return false;
                        };
                        // `Type::get()` is a member of Type; `ns::f()` of a namespace of this
                        // file is the free function `f()`
                        if let Some(scope) = self.result_scope(f, leaf) {
                            buf.extend_from_slice(self.cx.name_bytes(scope));
                            buf.extend_from_slice(b"::");
                        }
                        buf.extend_from_slice(self.cx.text(leaf));
                        buf.extend_from_slice(b"()");
                        true
                    }
                    K::FieldExpression => {
                        // result of a member call: `recv::method()`
                        let (Some(arg), Some(fl)) = (child(f, t.f_argument), child(f, t.f_field)) else { return false };
                        // `recv.get<T>()`: the template name without its arguments
                        let fl = if self.k(fl) == K::TemplateMethod { child(fl, t.f_name).unwrap_or(fl) } else { fl };
                        if self.k(fl) != K::FieldIdentifier || Self::is_empty(fl) {
                            return false;
                        }
                        let inner = if child(f, t.f_operator).is_some_and(|o| self.cx.text(o) == b"->") { Acc::Arrow } else { Acc::Dot };
                        if !self.qual_path(arg, inner, buf, depth + 1) {
                            return false;
                        }
                        buf.extend_from_slice(b"::");
                        buf.extend_from_slice(self.cx.text(fl));
                        buf.extend_from_slice(b"()");
                        true
                    }
                    _ => false,
                }
            }
            K::QualifiedIdentifier => {
                // `ns::global.field`: a non-local variable by its simple name
                match self.qualified_last(e).filter(|x| self.k(*x) == K::Identifier && !Self::is_empty(*x)) {
                    Some(leaf) => {
                        buf.push(b'$');
                        buf.extend_from_slice(self.cx.text(leaf));
                        true
                    }
                    None => false,
                }
            }
            _ => false,
        }
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

    /// A value that can be bound into a slot: a non-local name, possibly behind `&`, a cast or a
    /// scope (`&Class::method`). Returns the identifier that names it.
    fn bindable(&mut self, mut r: Node<'t>) -> Option<Node<'t>> {
        for _ in 0..3 {
            match self.k(r) {
                K::PointerExpression | K::ParenthesizedExpression => {
                    r = child(r, self.t.f_argument).or_else(|| r.named_child(0))?;
                }
                K::CastExpression => r = child(r, self.t.f_value)?,
                _ => break,
            }
        }
        if self.k(r) == K::QualifiedIdentifier {
            r = self.qualified_last(r)?;
        }
        if self.k(r) != K::Identifier || Self::is_empty(r) {
            return None;
        }
        let text = self.cx.text(r);
        if reserved(text) {
            return None;
        }
        let name = self.cx.intern(text);
        match self.local_of(name) {
            Some(LAMBDA) | None => Some(r),
            Some(_) => None,
        }
    }

    fn lvalue_target(&self, mut l: Node<'t>) -> Node<'t> {
        for _ in 0..6 {
            match self.k(l) {
                K::ParenthesizedExpression => match l.named_child(0) {
                    Some(x) => l = x,
                    None => break,
                },
                K::SubscriptExpression => match child(l, self.t.f_argument) {
                    Some(x) => l = x,
                    None => break,
                },
                _ => break,
            }
        }
        l
    }

    fn mark_write(&mut self, target: Node<'t>) {
        match self.k(target) {
            K::Identifier => {
                self.cx.s.writes.insert(target.start_byte() as u32);
            }
            K::FieldExpression => {
                if let Some(fl) = child(target, self.t.f_field).and_then(|f| self.leaf_name(f)) {
                    self.cx.s.writes.insert(fl.start_byte() as u32);
                }
            }
            K::QualifiedIdentifier => {
                if let Some(leaf) = self.qualified_last(target).and_then(|l| self.leaf_name(l)) {
                    self.cx.s.writes.insert(leaf.start_byte() as u32);
                }
            }
            _ => {}
        }
    }

    fn assignment(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        if let Some(l) = child(n, t.f_left) {
            let target = self.lvalue_target(l);
            self.mark_write(target);
            let plain = child(n, t.f_operator).is_none_or(|op| self.cx.text(op) == b"=");
            if plain {
                match self.k(target) {
                    K::FieldExpression => {
                        if let Some(fl) = child(target, t.f_field).filter(|f| self.k(*f) == K::FieldIdentifier) {
                            if let Some(v) = child(n, t.f_right).and_then(|r| self.bindable(r)) {
                                let acc = if child(target, t.f_operator).is_some_and(|o| self.cx.text(o) == b"->") { Acc::Arrow } else { Acc::Dot };
                                let q = match child(target, t.f_argument) {
                                    Some(a) => self.qual_of(a, acc),
                                    None => NONE,
                                };
                                let slot = self.slot(q, self.cx.text(fl));
                                let vn = self.cx.name(v);
                                self.cx.add_ref(vn, RefKind::Bind, line_of(v), slot, 255);
                                self.cx.mark(v);
                            }
                        }
                    }
                    K::Identifier if self.cur_class != NONE && !Self::is_empty(target) => {
                        // `cb_ = handler;` in a member function, `cb_` a known data member
                        let name = self.cx.name(target);
                        if self.local_of(name).is_none() && self.is_member(self.cur_class, name) {
                            if let Some(v) = child(n, t.f_right).and_then(|r| self.bindable(r)) {
                                let slot = self.slot(self.cur_class, self.cx.text(target));
                                let vn = self.cx.name(v);
                                self.cx.add_ref(vn, RefKind::Bind, line_of(v), slot, 255);
                                self.cx.mark(v);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        self.children(c);
    }

    fn update(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        if let Some(a) = child(n, self.t.f_argument) {
            let target = self.lvalue_target(a);
            self.mark_write(target);
        }
        self.children(c);
    }

    /// `T{..}` / `(T){..}`: an instantiation of T.
    fn compound_literal(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let ty = child(n, t.f_type);
        let tname = match ty {
            Some(x) => self.type_info(x, true).0,
            None => NONE,
        };
        let nargs = child(n, t.f_value).map(|v| self.count_args(v)).unwrap_or(0);
        self.new_ref(ty, nargs);
        self.st.init_ctx.push(tname);
        self.children(c);
        self.st.init_ctx.pop();
    }

    fn new_expression(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let nargs = child(n, t.f_arguments).map(|a| self.count_args(a)).unwrap_or(0);
        self.new_ref(child(n, t.f_type), nargs);
        self.children(c);
    }

    fn lambda(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let saved = (self.sk, self.parent, self.container);
        self.cx.push_scope();
        self.fn_depth += 1;
        self.sk = Sk::Func;
        self.container = NONE;
        let encl = self.cx.encl();
        if encl != NONE {
            self.parent = encl;
        }
        // `[n = expr]` introduces a name; plain captures name outer locals
        if let Some(caps) = child(n, t.f_captures) {
            let mut cc = self.cursor_at(caps);
            if cc.goto_first_child() {
                loop {
                    let ch = cc.node();
                    if self.k(ch) == K::LambdaCaptureInit {
                        if let Some(l) = child(ch, t.f_left).filter(|x| !Self::is_empty(*x)) {
                            let nm = self.cx.name(l);
                            self.cx.declare(nm, NONE);
                            self.cx.mark(l);
                        }
                    }
                    if !cc.goto_next_sibling() {
                        break;
                    }
                }
            }
            self.release(cc);
        }
        if let Some(d) = child(n, t.f_declarator) {
            self.declare_params(d);
        }
        self.children(c);
        self.fn_depth -= 1;
        self.cx.pop_scope();
        (self.sk, self.parent, self.container) = saved;
    }

    /// `for (T x : range)`: the loop variable lives for the loop.
    fn for_range(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        self.cx.push_scope();
        let ty = match child(n, t.f_type) {
            Some(x) => {
                let (id, fl) = self.type_info(x, true);
                enc(id, fl)
            }
            None => NONE,
        };
        if let Some(d) = child(n, t.f_declarator) {
            self.declare_declarator(d, ty);
        }
        self.children(c);
        self.cx.pop_scope();
    }

    fn scoped(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        self.children(c);
        self.cx.pop_scope();
    }

    /// `catch (const E &e) { }`, `requires (T a) { }`: parameters local to the construct.
    fn scoped_params(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        if let Some(params) = child(n, self.t.f_parameters) {
            self.declare_param_list(params);
        }
        self.children(c);
        self.cx.pop_scope();
    }

    /// `offsetof(T, member)` reads `T::member`.
    fn offsetof(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        if let Some(m) = child(n, t.f_member).filter(|x| !Self::is_empty(*x)) {
            let ty = match child(n, t.f_type) {
                Some(x) => self.type_info(x, false).0,
                None => NONE,
            };
            let name = self.cx.name(m);
            self.cx.add_ref(name, RefKind::MemberRead, line_of(m), ty, 255);
        }
        self.children(c);
    }

    /// The last component of a qualified identifier, its scope as written with template
    /// arguments left out (`a::b::c<int>::f` -> `f`, `a::b::c`) and the name of the scope
    /// component directly before the last one, as written (`c`). The scope is `NONE` for the
    /// global scope and for scopes that depend on a template parameter; a namespace alias is
    /// replaced by its target.
    fn split_scopes(&mut self, q: Node<'t>) -> (Option<Node<'t>>, u32, u32) {
        let t = self.t;
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let mut cur = q;
        let mut last = None;
        let mut dependent = false;
        let mut tail = NONE;
        for _ in 0..16 {
            if let Some(s) = child(cur, t.f_scope) {
                match self.scope_name(s).filter(|x| !Self::is_empty(*x)) {
                    Some(sn) => {
                        let id = self.cx.name(sn);
                        tail = id;
                        if buf.is_empty() {
                            if self.is_local_type(id) {
                                dependent = true;
                            }
                            match self.alias_target(id) {
                                Some(target) => buf.extend_from_slice(self.cx.name_bytes(target)),
                                None => buf.extend_from_slice(self.cx.text(sn)),
                            }
                        } else {
                            buf.extend_from_slice(b"::");
                            buf.extend_from_slice(self.cx.text(sn));
                        }
                    }
                    None => dependent = true,
                }
            }
            match child(cur, t.f_name) {
                Some(nm) if self.k(nm) == K::QualifiedIdentifier => cur = nm,
                Some(nm) => {
                    last = Some(nm);
                    break;
                }
                None => break,
            }
        }
        let (full, tail) = if dependent || buf.is_empty() { (NONE, NONE) } else { (self.cx.intern(&buf), tail) };
        self.cx.s.buf = buf;
        (last, full, tail)
    }

    /// The last component of a qualified identifier and the qualifier a reference to it
    /// carries. Types are plain names, linked by name and include reachability, so a scope
    /// that ends in a type is that type's name with its namespaces dropped
    /// (`llvm::Triple::normalize` -> `Triple`); a scope that is a namespace of this file is
    /// kept as written (`geo::shapes::length` -> `geo::shapes`, an alias as its target).
    fn split_qualified(&mut self, q: Node<'t>) -> (Option<Node<'t>>, u32) {
        let (last, full, tail) = self.split_scopes(q);
        if full == NONE || self.is_namespace(tail) {
            return (last, full);
        }
        let path = self.cx.name_bytes(full);
        let qual = match memchr::memmem::rfind(path, b"::") {
            Some(i) => tail_of(self, i + 2, full),
            None => full,
        };
        (last, qual)
    }

    /// How the result of a call through a qualified name is rooted in a type path: `Some(T)`
    /// for `T::f()` (a static member function, or a function of a namespace known only by
    /// name: both are members of `T`), `None` for the bare `f()` (a function of a namespace of
    /// this file, or what may be a constructor: `llvm::Twine(x)`).
    fn result_scope(&mut self, f: Node<'t>, leaf: Node<'t>) -> Option<u32> {
        let (_, full, tail) = self.split_scopes(f);
        if full == NONE || self.is_namespace(tail) {
            return None;
        }
        let lower = |w: &[u8]| w.first().is_some_and(|b| b.is_ascii_lowercase());
        if lower(self.cx.name_bytes(tail)) && !lower(self.cx.text(leaf)) && !self.st.known_types.contains(&tail) {
            return None;
        }
        Some(tail)
    }

    /// A qualified name that is not a callee: `ns::value`, `Type::CONSTANT`, `&Class::method`,
    /// `ns::Type` in a type position.
    fn qualified(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let leaf = self.qualified_last(n).and_then(|l| self.leaf_name(l)).filter(|x| !Self::is_empty(*x));
        if let Some(leaf) = leaf.filter(|x| !self.cx.is_marked(*x)) {
            let kind = self.k(leaf);
            if matches!(kind, K::Identifier | K::TypeIdentifier) {
                let (_, qual) = self.split_qualified(n);
                let text = self.cx.text(leaf);
                if !reserved(text) {
                    let name = self.cx.intern(text);
                    let pos = leaf.start_byte() as u32;
                    let rk = if kind == K::TypeIdentifier {
                        RefKind::Type
                    } else if !self.cx.s.writes.is_empty() && self.cx.s.writes.contains(&pos) {
                        RefKind::Write
                    } else {
                        RefKind::Read
                    };
                    self.cx.add_ref(name, rk, line_of(leaf), qual, 255);
                }
                self.cx.mark(leaf);
            }
        }
        self.qualified_parts(c);
    }

    /// Visit the scopes and template arguments of a qualified identifier without treating its
    /// nested levels as qualified names of their own.
    fn qualified_parts(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        if c.goto_first_child() {
            loop {
                if self.k(c.node()) == K::QualifiedIdentifier {
                    self.qualified_parts(c);
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

    /// `{ .field = value, ... }`: record member writes with the owning type, and bind function
    /// names to their slots. The cursor is on the initializer_list.
    fn initializer_list(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        let ctx = *self.st.init_ctx.last().unwrap_or(&NONE);
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                match self.k(ch) {
                    K::InitializerPair => self.initializer_pair(c, ctx),
                    // Array element or positional nested aggregate: same element type.
                    K::InitializerList => self.initializer_list(c),
                    _ => self.visit(c),
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
            c.goto_parent();
        }
        self.cx.depth -= 1;
    }

    fn initializer_pair(&mut self, c: &mut TreeCursor<'t>, ctx: u32) {
        let t = self.t;
        let mut field_name: Option<&'a [u8]> = None;
        let mut designators = 0;
        if !c.goto_first_child() {
            return;
        }
        loop {
            let ch = c.node();
            if is_field(c, t.f_designator) {
                designators += 1;
                let fl = match self.k(ch) {
                    K::FieldDesignator => ch.named_child(0),
                    // `field: value` (GNU)
                    K::FieldIdentifier => Some(ch),
                    _ => None,
                };
                match fl {
                    Some(fl) => {
                        if designators == 1 && !Self::is_empty(fl) {
                            let name = self.cx.name(fl);
                            self.cx.add_ref(name, RefKind::MemberWrite, line_of(fl), ctx, 255);
                            field_name = Some(self.cx.text(fl));
                        }
                    }
                    // `[IDX] = ...`: the index expression may reference constants.
                    None => self.visit(c),
                }
            } else if is_field(c, t.f_value) {
                match (self.k(ch), field_name) {
                    (K::InitializerList, Some(f)) if designators == 1 => {
                        // Nested aggregate: its type is the type of field `f` in `ctx`.
                        let nested = if ctx != NONE {
                            let mut buf = std::mem::take(&mut self.cx.s.buf);
                            buf.clear();
                            buf.extend_from_slice(self.cx.name_bytes(ctx));
                            buf.extend_from_slice(b"::");
                            buf.extend_from_slice(f);
                            let id = self.cx.intern(&buf);
                            self.cx.s.buf = buf;
                            id
                        } else {
                            NONE
                        };
                        self.st.init_ctx.push(nested);
                        self.initializer_list(c);
                        self.st.init_ctx.pop();
                    }
                    (K::InitializerList, _) => self.initializer_list(c),
                    (_, Some(f)) if designators == 1 => match self.bindable(ch) {
                        Some(v) => {
                            let slot = self.slot(ctx, f);
                            let vn = self.cx.name(v);
                            self.cx.add_ref(vn, RefKind::Bind, line_of(v), slot, 255);
                            self.cx.mark(v);
                            // scopes of `&Class::method` are still references
                            if self.k(ch) != K::Identifier {
                                self.visit(c);
                            }
                        }
                        None => self.visit(c),
                    },
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

    // ----- statements at file scope: macro-defined functions and variables -----

    fn expression_statement(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        if self.sk != Sk::Top {
            return self.children(c);
        }
        let t = self.t;
        let Some(call) = n.named_child(0).filter(|x| self.k(*x) == K::CallExpression) else {
            return self.children(c);
        };
        let Some(func) = child(call, t.f_function).filter(|f| self.k(*f) == K::Identifier && !Self::is_empty(*f)) else {
            return self.children(c);
        };
        let mname = self.cx.text(func);
        let args = child(call, t.f_arguments);

        // `MACRO("name", ...) { body }`: the arguments do not parse as parameters, so the
        // grammar sees a call followed by a block. The block is the body of a function.
        if block_follows(self.cx.src, n.end_byte()) {
            let mut first_id: Option<Node<'t>> = None;
            // `MACRO(ret, name, (int a, char *b)) { }`: the parenthesised group is the parameter
            // list of the generated function and the identifier in front of it its name
            let mut group: Option<Node<'t>> = None;
            let (mut groups, mut nargs) = (0u32, 0u32);
            let mut last_ident: Option<Node<'t>> = None;
            let mut before_group: Option<Node<'t>> = None;
            self.st.macro_params.clear();
            if let Some(a) = args {
                let mut ac = self.cursor_at(a);
                if ac.goto_first_child() {
                    loop {
                        let ch = ac.node();
                        let is_ident = self.k(ch) == K::Identifier && !Self::is_empty(ch) && !reserved(self.cx.text(ch));
                        if ch.is_named() && self.k(ch) != K::Comment {
                            nargs += 1;
                            if self.k(ch) == K::ParenthesizedExpression {
                                group = Some(ch);
                                groups += 1;
                                before_group = last_ident;
                            }
                            last_ident = if is_ident { Some(ch) } else { None };
                        }
                        if is_ident {
                            if first_id.is_none() {
                                first_id = Some(ch);
                            } else {
                                let id = self.cx.name(ch);
                                self.st.macro_params.push(id);
                            }
                        }
                        if !ac.goto_next_sibling() {
                            break;
                        }
                    }
                }
                self.release(ac);
            }
            let named = before_group.filter(|_| groups == 1 && nargs >= 3 && last_ident.is_none());
            let mut ret_type: Option<Node<'t>> = None;
            if let (Some(x), Some(g)) = (named, group) {
                self.st.macro_params.clear();
                self.group_params(self.cx.text(g));
                ret_type = first_id.filter(|f| f.id() != x.id());
            }
            let name_node = named.or(first_id).unwrap_or(func);
            let name = self.cx.name(name_node);
            let d = self.cx.def(name, DefKind::Function, n, n.end_byte());
            let parent = self.parent;
            let dm = self.cx.def_mut(d);
            dm.parent = parent;
            dm.flags = dflag::MACRO_DEFINED | if contains(mname, b"TEST") { dflag::TEST } else { 0 };
            dm.arity_max = 255;
            let m = self.cx.name(func);
            self.cx.push_def(d);
            self.cx.add_ref(m, RefKind::Call, line_of(func), NONE, 255);
            if let Some(ty) = ret_type {
                let id = self.cx.name(ty);
                self.cx.add_ref(id, RefKind::Type, line_of(ty), NONE, 255);
            }
            self.cx.pop_def();
            self.pending = Some(Pending { def: d, fdecl: None, after: n.end_byte(), macro_fn: true });
            return;
        }

        if mname.starts_with(b"EXPORT_SYMBOL") {
            if let Some(id) = args.and_then(|a| a.named_child(0)).filter(|x| self.k(*x) == K::Identifier) {
                let name = self.cx.name(id);
                self.st.exports.push(name);
            }
            return;
        }

        // `DEFINE_MUTEX(name);`, `LIST_HEAD(name);`, `DEFINE_PER_CPU(type, name);`,
        // `STATISTIC(NumFoo, "...");`
        let var_macro = is_var_macro(mname);
        if var_macro || is_define_macro(mname) {
            if let Some(a) = args {
                let want_last = contains(mname, b"PER_CPU");
                let mut pick: Option<Node<'t>> = None;
                let mut ac = self.cursor_at(a);
                if ac.goto_first_child() {
                    loop {
                        let ch = ac.node();
                        if self.k(ch) == K::Identifier && !Self::is_empty(ch) && (pick.is_none() || want_last) {
                            pick = Some(ch);
                        }
                        if !ac.goto_next_sibling() {
                            break;
                        }
                    }
                }
                self.release(ac);
                // `DEFINE_CONVERSION_FUNCTIONS(Type, TypeRef)` names a type, not a variable
                if let Some(id) = pick.filter(|id| var_macro || !type_like(self.cx.text(*id))) {
                    let name = self.cx.name(id);
                    let d = self.cx.def(name, DefKind::Variable, n, n.end_byte());
                    let parent = self.parent;
                    let local = var_macro && !self.cx.is_header;
                    let dm = self.cx.def_mut(d);
                    dm.parent = parent;
                    dm.flags |= dflag::MACRO_DEFINED;
                    if local {
                        dm.flags |= dflag::LOCAL;
                    }
                    self.st.globals.insert(name);
                    self.cx.mark(id);
                    self.cx.push_def(d);
                    self.children(c);
                    self.cx.pop_def();
                    return;
                }
            }
        }
        self.children(c);
    }

    /// The parameter names of a parameter list given as text, `(int a, const char *b)`: the
    /// last word of every comma-separated piece that has more than one word. They become the
    /// locals of the macro-defined function whose body follows.
    fn group_params(&mut self, text: &[u8]) {
        let inner = text.strip_prefix(b"(").and_then(|x| x.strip_suffix(b")")).unwrap_or(text);
        let mut depth = 0i32;
        let mut words = 0u32;
        let mut last: &[u8] = b"";
        let mut i = 0usize;
        let n = inner.len();
        loop {
            let b = if i < n { inner[i] } else { b',' };
            if b == b',' && depth == 0 {
                if words >= 2 && !last.is_empty() && !reserved(last) && self.st.macro_params.len() < 64 {
                    let id = self.cx.intern(last);
                    self.st.macro_params.push(id);
                }
                words = 0;
                last = b"";
                if i >= n {
                    break;
                }
                i += 1;
            } else if b == b'_' || b.is_ascii_alphabetic() {
                let s0 = i;
                while i < n && (inner[i] == b'_' || inner[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                if depth == 0 {
                    words += 1;
                    last = &inner[s0..i];
                }
            } else {
                match b {
                    b'(' | b'[' | b'<' => depth += 1,
                    b')' | b']' | b'>' => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
        }
    }

    fn compound(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        match self.sk {
            Sk::Func => self.scoped(c),
            Sk::Top => {
                // a block at file scope is the body of the function recognised just before it
                let Some(p) = self.pending.take().filter(|p| skip_blank(self.cx.src, p.after) == n.start_byte()) else {
                    return self.children(c);
                };
                {
                    let dm = self.cx.def_mut(p.def);
                    dm.end_line = end_line_of(n);
                    dm.flags &= !dflag::DECL;
                }
                self.function_body(c, p.def, p.fdecl, self.cur_class, p.macro_fn);
            }
            Sk::Class => self.children(c),
        }
    }

    /// Annotation macros the grammar cannot place (`__init`, `__user`) surface as identifiers
    /// directly under ERROR nodes: they are noise, not references.
    fn error(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        self.cx.facts.parse_errors += 1;
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        let d = if self.sk == Sk::Top { self.error_define(n) } else { NONE };
        if d != NONE {
            self.cx.push_def(d);
        }
        self.cx.depth += 1;
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
        self.cx.depth -= 1;
        if d != NONE {
            self.cx.pop_def();
        }
    }

    /// `DEFINE_PER_CPU(struct foo, name);` parses as a function declarator inside an error.
    fn error_define(&mut self, n: Node<'t>) -> u32 {
        let t = self.t;
        let Some(fd) = n.named_child(0).filter(|x| self.k(*x) == K::FunctionDeclarator) else { return NONE };
        let Some(m) = child(fd, t.f_declarator).filter(|x| self.k(*x) == K::Identifier && is_define_macro(self.cx.text(*x))) else { return NONE };
        let Some(params) = child(fd, t.f_parameters) else { return NONE };
        let mut pick: Option<Node<'t>> = None;
        let mut pc = self.cursor_at(params);
        if pc.goto_first_child() {
            loop {
                let ch = pc.node();
                if self.k(ch) == K::ParameterDeclaration && child(ch, t.f_declarator).is_none() {
                    if let Some(ty) = child(ch, t.f_type).filter(|x| self.k(*x) == K::TypeIdentifier && !Self::is_empty(*x)) {
                        pick = Some(ty);
                    }
                }
                if !pc.goto_next_sibling() {
                    break;
                }
            }
        }
        self.release(pc);
        let Some(id) = pick else { return NONE };
        let name = self.cx.name(id);
        let d = self.cx.def(name, DefKind::Variable, n, n.end_byte());
        let parent = self.parent;
        let dm = self.cx.def_mut(d);
        dm.parent = parent;
        dm.flags |= dflag::MACRO_DEFINED;
        self.cx.mark(id);
        let mid = self.cx.name(m);
        self.cx.mark(m);
        self.cx.push_def(d);
        self.cx.add_ref(mid, RefKind::Call, line_of(m), NONE, 255);
        self.cx.pop_def();
        d
    }

    /// Constructor initialisers that could not be told apart when they were read: a name
    /// that is written as a type somewhere in the file is a base class, and its initialiser a
    /// call of that class's constructor (a value handed to it is an argument, not a binding).
    fn settle_inits(&mut self) {
        let pending = std::mem::take(&mut self.st.pending_inits);
        // names used as a type or as a scope (`Base::f()`) anywhere in the file
        let mut names: Vec<(u32, bool)> = Vec::new();
        for &(at, _, _) in &pending {
            let name = self.cx.facts.refs[at as usize].name;
            if !names.iter().any(|&(n, _)| n == name) {
                names.push((name, self.st.types_seen.contains(&name)));
            }
        }
        if names.iter().any(|&(_, seen)| !seen) {
            for r in &self.cx.facts.refs {
                if matches!(r.kind, RefKind::Type | RefKind::New | RefKind::Extends) {
                    if let Some(e) = names.iter_mut().find(|e| e.0 == r.name) {
                        e.1 = true;
                    }
                }
            }
        }
        for &(at, bind, nargs) in &pending {
            let name = self.cx.facts.refs[at as usize].name;
            if !names.iter().any(|&(n, seen)| n == name && seen) {
                continue;
            }
            let r = &mut self.cx.facts.refs[at as usize];
            r.kind = RefKind::Call;
            r.qual = NONE;
            r.nargs = nargs;
            if bind != NONE {
                let b = &mut self.cx.facts.refs[bind as usize];
                b.kind = RefKind::Arg;
                b.qual = name;
            }
        }
        self.st.pending_inits = pending;
    }

    /// `Ret ns::f() { }` in a file that never opens `ns`: when the definition was read, `ns`
    /// could have been a class, and `f` was taken for one of its member functions. A scope
    /// written in lower case that the file never uses as a type, never derives from, never
    /// constructs and gives no constructor or destructor is a namespace: `f` is a function, the
    /// same one its prototype in the header declares.
    fn settle_scopes(&mut self) {
        let mut cands: Vec<(u32, bool)> = Vec::new();
        for d in &self.cx.facts.defs {
            if d.kind == DefKind::Method && d.parent == NONE && d.qual != NONE && !cands.iter().any(|c| c.0 == d.qual) {
                let q = self.cx.facts.names.get(d.qual);
                if q.first().is_some_and(|b| b.is_ascii_lowercase()) && !q.ends_with(b"iterator") && !q.ends_with(b"_traits") && !q.ends_with(b"_t") {
                    let class = self.st.types_seen.contains(&d.qual) || self.st.known_types.contains(&d.qual);
                    cands.push((d.qual, class));
                }
            }
        }
        if cands.iter().all(|c| c.1) {
            return;
        }
        for d in &self.cx.facts.defs {
            let nb = self.cx.facts.names.get(d.name);
            let dtor = nb.strip_prefix(b"~");
            for c in cands.iter_mut().filter(|c| !c.1) {
                let q = self.cx.facts.names.get(c.0);
                if nb == q || dtor == Some(q) || nb.starts_with(b"operator") && d.qual == c.0 {
                    c.1 = true;
                }
            }
        }
        for r in &self.cx.facts.refs {
            if matches!(r.kind, RefKind::New | RefKind::Extends) {
                if let Some(c) = cands.iter_mut().find(|c| c.0 == r.name) {
                    c.1 = true;
                }
            }
        }
        cands.retain(|c| !c.1);
        if cands.is_empty() {
            return;
        }
        let mut spans: Vec<(u32, u32, u32)> = Vec::new();
        for d in self.cx.facts.defs.iter_mut() {
            if d.kind == DefKind::Method && d.parent == NONE && cands.iter().any(|c| c.0 == d.qual) {
                d.kind = DefKind::Function;
                spans.push((d.line, d.end_line, d.qual));
            }
        }
        // unqualified names in those bodies were tied to the scope as if it were a class
        for r in self.cx.facts.refs.iter_mut() {
            if r.qual != NONE
                && matches!(r.kind, RefKind::Call | RefKind::Read | RefKind::Write)
                && spans.iter().any(|s| s.2 == r.qual && s.0 <= r.line && r.line <= s.1)
            {
                r.qual = NONE;
            }
        }
    }

    fn finish(&mut self, orig: &[u8], blanks: &[Blank]) {
        if !self.st.pending_inits.is_empty() {
            self.settle_inits();
        }
        self.settle_scopes();
        if !self.st.exports.is_empty() {
            let exports = &self.st.exports;
            for d in self.cx.facts.defs.iter_mut() {
                if matches!(d.kind, DefKind::Function | DefKind::Variable) && exports.contains(&d.name) {
                    d.flags |= dflag::EXPORTED;
                }
            }
        }
        if !blanks.is_empty() {
            self.blank_refs(orig, blanks);
        }
    }

    /// The macros that were blanked out before parsing are still uses of those macros: record
    /// them, attributed to the innermost definition spanning their line.
    fn blank_refs(&mut self, orig: &[u8], blanks: &[Blank]) {
        self.st.pp_params.clear();
        self.cx.s.def_stack.clear();
        let ndefs = self.cx.facts.defs.len();
        let mut next_def = 0usize;
        let mut line = 1u32;
        let mut pos = 0usize;
        for b in blanks {
            let off = b.off as usize;
            let end = off + b.len as usize;
            let name_end = off + b.name_len as usize;
            if off < pos || end > orig.len() || name_end > end {
                continue;
            }
            if b.name_len == 0 {
                // an attribute that was blanked has no name: nothing to record; an `#include`
                // blanked inside an initialiser list is still an import
                if orig[off] == b'#' {
                    let l = line + memchr::memchr_iter(b'\n', &orig[pos..off]).count() as u32;
                    self.include_text(&orig[off..end], l);
                }
                continue;
            }
            line += memchr::memchr_iter(b'\n', &orig[pos..off]).count() as u32;
            pos = off;
            // open the definitions starting at or before this line, close the ones that ended
            while next_def < ndefs && self.cx.facts.defs[next_def].line <= line {
                self.cx.s.def_stack.push(next_def as u32);
                next_def += 1;
            }
            while let Some(&top) = self.cx.s.def_stack.last() {
                if self.cx.facts.defs[top as usize].end_line < line {
                    self.cx.s.def_stack.pop();
                } else {
                    break;
                }
            }
            let name = self.cx.intern(&orig[off..name_end]);
            if name_end < end {
                self.cx.add_ref(name, RefKind::Call, line, NONE, 255);
                // the arguments, without the parentheses
                let args = &orig[name_end..end];
                let inner = match (memchr::memchr(b'(', args), memchr::memrchr(b')', args)) {
                    (Some(a), Some(z)) if a < z => &args[a + 1..z],
                    _ => args,
                };
                let first = line + memchr::memchr_iter(b'\n', &orig[off..name_end]).count() as u32;
                self.scan_text(inner, first);
            } else {
                self.cx.add_ref(name, RefKind::Read, line, NONE, 255);
            }
        }
        self.cx.s.def_stack.clear();
    }
}

/// Specifiers of a declaration that decide what it declares.
#[derive(Default, Clone, Copy)]
struct Specs {
    is_static: bool,
    is_extern: bool,
    is_const: bool,
    is_constexpr: bool,
}

trait NameBytes {
    fn name_bytes_or_empty(&self, id: u32) -> &[u8];
}

impl NameBytes for Ctx<'_> {
    #[inline]
    fn name_bytes_or_empty(&self, id: u32) -> &[u8] {
        if id == NONE { b"" } else { self.name_bytes(id) }
    }
}

/// Intern the part of name `id` that starts at byte `from`.
fn tail_of(w: &mut W, from: usize, id: u32) -> u32 {
    let mut buf = std::mem::take(&mut w.cx.s.buf);
    buf.clear();
    buf.extend_from_slice(&w.cx.name_bytes(id)[from..]);
    let out = w.cx.intern(&buf);
    w.cx.s.buf = buf;
    out
}

/// Whether the last token of a node was made up by the parser (a missing `;`).
fn last_child_missing(n: Node) -> bool {
    n.child_count().checked_sub(1).and_then(|i| n.child(i as u32)).is_some_and(|c| c.is_missing())
}

#[inline]
fn last_named<'t>(n: Node<'t>) -> Option<Node<'t>> {
    n.named_child(n.named_child_count().checked_sub(1)? as u32)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(hay, needle).is_some()
}

/// Offset of the first byte at or after `i` that is neither whitespace nor inside a comment.
fn skip_blank(src: &[u8], mut i: usize) -> usize {
    let n = src.len();
    loop {
        while i < n && src[i].is_ascii_whitespace() {
            i += 1;
        }
        if i + 1 < n && src[i] == b'/' && src[i + 1] == b'/' {
            i = memchr::memchr(b'\n', &src[i..]).map(|j| i + j + 1).unwrap_or(n);
        } else if i + 1 < n && src[i] == b'/' && src[i + 1] == b'*' {
            i = memchr::memmem::find(&src[i + 2..], b"*/").map(|j| i + 2 + j + 2).unwrap_or(n);
        } else {
            return i;
        }
    }
}

fn block_follows(src: &[u8], end: usize) -> bool {
    let i = skip_blank(src, end);
    i < src.len() && src[i] == b'{'
}

/// `UpperCamelCase`: by convention a type name.
fn type_like(w: &[u8]) -> bool {
    w.first().is_some_and(|b| b.is_ascii_uppercase()) && w.iter().any(|b| b.is_ascii_lowercase())
}

/// Macros that define a file-static variable named by their first argument (LLVM).
fn is_var_macro(m: &[u8]) -> bool {
    matches!(m, b"STATISTIC" | b"ALWAYS_ENABLED_STATISTIC" | b"DEBUG_COUNTER")
}

fn is_define_macro(m: &[u8]) -> bool {
    m.starts_with(b"DEFINE_") || m.starts_with(b"DECLARE_") || m.ends_with(b"_HEAD") && m.iter().all(|b| !b.is_ascii_lowercase())
}

fn is_header_guard(name: &[u8]) -> bool {
    name.ends_with(b"_H")
        || name.ends_with(b"_H_")
        || name.ends_with(b"_H__")
        || name.ends_with(b"_HPP")
        || name.ends_with(b"_HPP_")
        || name.ends_with(b"_INCLUDED")
}

/// gtest: `TEST(Suite, Name)`, `TEST_F`, `TEST_P`, `TYPED_TEST`, `TYPED_TEST_P` and their
/// project-specific wrappers (`IN_PROC_BROWSER_TEST_F`).
fn is_gtest_macro(m: &[u8]) -> bool {
    matches!(m, b"TEST" | b"TEST_F" | b"TEST_P" | b"TYPED_TEST" | b"TYPED_TEST_P")
        || m.ends_with(b"_TEST")
        || m.ends_with(b"_TEST_F")
        || m.ends_with(b"_TEST_P")
}

/// gtest macros whose first argument is a fixture class.
fn is_fixture_macro(m: &[u8]) -> bool {
    !(m == b"TEST" || m.ends_with(b"_TEST") && !m.starts_with(b"TYPED_"))
}

/// Class templates whose `->` and `*` reach their first template argument: smart pointers and
/// option-like wrappers.
fn is_smart_pointer(w: &[u8]) -> bool {
    WRAPPERS.contains(&w)
}

const WRAPPERS: [&[u8]; 10] =
    [b"unique_ptr", b"shared_ptr", b"weak_ptr", b"optional", b"IntrusiveRefCntPtr", b"Expected", b"ErrorOr", b"scoped_refptr", b"intrusive_ptr", b"RefPtr"];

/// The `WRAP_MASK` bits of a smart-pointer class.
fn wrapper_bits(w: &[u8]) -> u32 {
    match WRAPPERS.iter().position(|x| *x == w) {
        Some(i) => (i as u32 + 1) << WRAP_SHIFT,
        None => 0,
    }
}

fn is_make_smart(w: &[u8]) -> bool {
    matches!(w, b"make_unique" | b"make_shared" | b"allocate_shared" | b"make_unique_for_overwrite" | b"make_shared_for_overwrite")
}

/// `static_cast`, `dyn_cast`, `cast_or_null`, `any_cast`, `static_pointer_cast`, ...: function
/// templates whose first template argument names the result type.
fn is_cast_name(w: &[u8]) -> bool {
    w == b"cast" || w.ends_with(b"_cast") || w.starts_with(b"cast_") || contains(w, b"_cast_")
}

/// Class templates that hold a callable.
fn is_function_wrapper(w: &[u8]) -> bool {
    matches!(w, b"function" | b"function_ref" | b"unique_function" | b"move_only_function")
}

fn is_std_scope(scope: &[u8]) -> bool {
    scope == b"std" || scope.starts_with(b"std::")
}

/// C++ keywords, fundamental types and compiler built-ins that reach the walker as identifiers
/// (keywords do in code the grammar could not fully parse), plus `std`: never references.
fn reserved(w: &[u8]) -> bool {
    let Some(&b) = w.first() else { return true };
    if !(b.is_ascii_lowercase() || b == b'_') {
        return false;
    }
    match w.len() {
        2 => matches!(w, b"if" | b"do" | b"or"),
        3 => matches!(w, b"std" | b"for" | b"int" | b"new" | b"try" | b"and" | b"not" | b"xor" | b"asm"),
        4 => matches!(w, b"this" | b"true" | b"else" | b"void" | b"bool" | b"char" | b"long" | b"auto" | b"enum" | b"case" | b"goto"),
        5 => matches!(
            w,
            b"false"
                | b"final"
                | b"class"
                | b"const"
                | b"short"
                | b"float"
                | b"union"
                | b"using"
                | b"while"
                | b"break"
                | b"catch"
                | b"throw"
                | b"compl"
                | b"bitor"
                | b"or_eq"
                | b"_Bool"
        ),
        6 => matches!(
            w,
            b"typeid"
                | b"sizeof"
                | b"struct"
                | b"static"
                | b"extern"
                | b"inline"
                | b"double"
                | b"signed"
                | b"return"
                | b"switch"
                | b"delete"
                | b"friend"
                | b"public"
                | b"export"
                | b"and_eq"
                | b"not_eq"
                | b"xor_eq"
                | b"bitand"
        ),
        7 => matches!(
            w,
            b"nullptr"
                | b"alignof"
                | b"alignas"
                | b"virtual"
                | b"private"
                | b"typedef"
                | b"mutable"
                | b"default"
                | b"concept"
                | b"wchar_t"
                | b"char8_t"
                | b"__int64"
        ),
        8 => matches!(
            w,
            b"override"
                | b"noexcept"
                | b"decltype"
                | b"__func__"
                | b"__FILE__"
                | b"__LINE__"
                | b"unsigned"
                | b"template"
                | b"typename"
                | b"operator"
                | b"explicit"
                | b"volatile"
                | b"continue"
                | b"register"
                | b"requires"
                | b"co_await"
                | b"co_yield"
                | b"char16_t"
                | b"char32_t"
                | b"__int128"
                | b"_Float16"
        ),
        9 => matches!(w, b"namespace" | b"protected" | b"constexpr" | b"consteval" | b"constinit" | b"co_return"),
        10 => matches!(w, b"const_cast" | b"__float128" | b"__int128_t"),
        11 => matches!(w, b"static_cast" | b"__uint128_t"),
        12 => matches!(w, b"dynamic_cast" | b"__FUNCTION__" | b"thread_local"),
        13 => matches!(w, b"static_assert" | b"__attribute__" | b"__extension__"),
        16 => w == b"reinterpret_cast",
        19 => w == b"__PRETTY_FUNCTION__",
        _ => false,
    }
}

fn is_cpp_keyword(w: &[u8]) -> bool {
    matches!(
        w,
        b"if"
            | b"else"
            | b"do"
            | b"while"
            | b"for"
            | b"return"
            | b"sizeof"
            | b"typeof"
            | b"__typeof__"
            | b"__typeof"
            | b"struct"
            | b"union"
            | b"enum"
            | b"class"
            | b"const"
            | b"volatile"
            | b"static"
            | b"extern"
            | b"inline"
            | b"__inline__"
            | b"__inline"
            | b"register"
            | b"void"
            | b"int"
            | b"char"
            | b"long"
            | b"short"
            | b"unsigned"
            | b"signed"
            | b"float"
            | b"double"
            | b"_Bool"
            | b"bool"
            | b"wchar_t"
            | b"char8_t"
            | b"char16_t"
            | b"char32_t"
            | b"goto"
            | b"break"
            | b"continue"
            | b"switch"
            | b"case"
            | b"default"
            | b"_Generic"
            | b"__attribute__"
            | b"__extension__"
            | b"asm"
            | b"__asm__"
            | b"defined"
            | b"__VA_ARGS__"
            | b"__VA_OPT__"
            | b"NULL"
            | b"true"
            | b"false"
            | b"auto"
            | b"restrict"
            | b"__restrict"
            | b"_Static_assert"
            | b"alignof"
            | b"_Alignof"
            | b"__alignof__"
            | b"alignas"
            | b"and"
            | b"and_eq"
            | b"bitand"
            | b"bitor"
            | b"catch"
            | b"co_await"
            | b"co_return"
            | b"co_yield"
            | b"compl"
            | b"concept"
            | b"const_cast"
            | b"consteval"
            | b"constexpr"
            | b"constinit"
            | b"decltype"
            | b"delete"
            | b"dynamic_cast"
            | b"explicit"
            | b"export"
            | b"final"
            | b"friend"
            | b"mutable"
            | b"namespace"
            | b"new"
            | b"noexcept"
            | b"not"
            | b"not_eq"
            | b"nullptr"
            | b"operator"
            | b"or"
            | b"or_eq"
            | b"override"
            | b"private"
            | b"protected"
            | b"public"
            | b"reinterpret_cast"
            | b"requires"
            | b"static_assert"
            | b"static_cast"
            | b"std"
            | b"template"
            | b"this"
            | b"thread_local"
            | b"throw"
            | b"try"
            | b"typedef"
            | b"typeid"
            | b"typename"
            | b"using"
            | b"virtual"
            | b"xor"
            | b"xor_eq"
            | b"__func__"
            | b"__FILE__"
            | b"__LINE__"
    )
}

// ---------------------------------------------------------------------------------------------
// Annotation macros
// ---------------------------------------------------------------------------------------------
//
// The grammar knows nothing about macros. An attribute-like macro in the wrong place does not
// just cost one node: `class LLVM_ABI Value : public Base<Value> {` is read as a declaration of
// variables, the class body spills into the enclosing namespace and its closing brace closes
// the namespace. Such macros are found with a small lexer in places where a name cannot stand
// in valid C++, and replaced by spaces in a copy of the source, which is then parsed again.
// Offsets and line numbers stay the same, so facts extracted from the second tree describe the
// file on disk. The blanked macros are recorded and emitted as references afterwards.

/// One blanked macro use: `len` bytes at `off`, the first `name_len` of which are its name (the
/// rest are its arguments).
#[derive(Clone, Copy)]
struct Blank {
    off: u32,
    name_len: u32,
    len: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum T {
    Eof,
    Ident,
    Num,
    Str,
    /// A preprocessor directive (also: start of file).
    Dir,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBrack,
    RBrack,
    Semi,
    Comma,
    Colon,
    Scope,
    Star,
    Amp,
    AmpAmp,
    Lt,
    Gt,
    Shr,
    Assign,
    Tilde,
    Dot,
    Arrow,
    Op,
}

#[derive(Clone, Copy)]
struct Tok {
    t: T,
    s: u32,
    e: u32,
}

#[derive(Clone)]
struct Lx<'s> {
    src: &'s [u8],
    i: usize,
    /// Only whitespace and comments since the last newline.
    bol: bool,
}

#[inline]
fn ident_byte(b: u8) -> bool {
    b == b'_' || b == b'$' || b.is_ascii_alphanumeric() || b >= 0x80
}

impl<'s> Lx<'s> {
    fn new(src: &'s [u8]) -> Self {
        Lx { src, i: 0, bol: true }
    }

    #[inline]
    fn word(&self, t: Tok) -> &'s [u8] {
        &self.src[t.s as usize..t.e as usize]
    }

    #[inline]
    fn at(&self, i: usize) -> u8 {
        if i < self.src.len() { self.src[i] } else { 0 }
    }

    fn next(&mut self) -> Tok {
        let s = self.src;
        let n = s.len();
        // whitespace and comments
        loop {
            while self.i < n {
                match s[self.i] {
                    b'\n' => {
                        self.bol = true;
                        self.i += 1;
                    }
                    b' ' | b'\t' | b'\r' | 0x0b | 0x0c => self.i += 1,
                    b'\\' if self.at(self.i + 1) == b'\n' => self.i += 2,
                    b'\\' if self.at(self.i + 1) == b'\r' && self.at(self.i + 2) == b'\n' => self.i += 3,
                    _ => break,
                }
            }
            if self.i + 1 < n && s[self.i] == b'/' {
                if s[self.i + 1] == b'/' {
                    self.line_comment();
                    continue;
                }
                if s[self.i + 1] == b'*' {
                    self.i = memchr::memmem::find(&s[self.i + 2..], b"*/").map(|j| self.i + 2 + j + 2).unwrap_or(n);
                    continue;
                }
            }
            break;
        }
        if self.i >= n {
            return Tok { t: T::Eof, s: n as u32, e: n as u32 };
        }
        let start = self.i;
        let b = s[start];
        let bol = self.bol;
        self.bol = false;
        let t = if b == b'#' && bol {
            self.directive();
            T::Dir
        } else if b.is_ascii_digit() || (b == b'.' && self.at(start + 1).is_ascii_digit()) {
            self.number();
            T::Num
        } else if ident_byte(b) {
            while self.i < n && ident_byte(s[self.i]) {
                self.i += 1;
            }
            let w = &s[start..self.i];
            let q = self.at(self.i);
            if q == b'"' && matches!(w, b"R" | b"LR" | b"uR" | b"UR" | b"u8R") {
                self.raw_string();
                T::Str
            } else if (q == b'"' || q == b'\'') && matches!(w, b"L" | b"u" | b"U" | b"u8") {
                self.quoted(q);
                T::Str
            } else {
                T::Ident
            }
        } else {
            self.i += 1;
            let nx = self.at(self.i);
            match b {
                b'"' | b'\'' => {
                    self.i = start;
                    self.quoted(b);
                    T::Str
                }
                b'(' => T::LParen,
                b')' => T::RParen,
                b'{' => T::LBrace,
                b'}' => T::RBrace,
                b'[' => T::LBrack,
                b']' => T::RBrack,
                b';' => T::Semi,
                b',' => T::Comma,
                b'~' => T::Tilde,
                b':' => {
                    if nx == b':' {
                        self.i += 1;
                        T::Scope
                    } else {
                        T::Colon
                    }
                }
                b'*' => {
                    if nx == b'=' {
                        self.i += 1;
                        T::Op
                    } else {
                        T::Star
                    }
                }
                b'&' => {
                    if nx == b'&' {
                        self.i += 1;
                        T::AmpAmp
                    } else if nx == b'=' {
                        self.i += 1;
                        T::Op
                    } else {
                        T::Amp
                    }
                }
                b'<' => {
                    if nx == b'<' || nx == b'=' {
                        self.i += 1;
                        if self.at(self.i) == b'=' || self.at(self.i) == b'>' {
                            self.i += 1;
                        }
                        T::Op
                    } else {
                        T::Lt
                    }
                }
                b'>' => {
                    if nx == b'>' {
                        self.i += 1;
                        if self.at(self.i) == b'=' {
                            self.i += 1;
                            T::Op
                        } else {
                            T::Shr
                        }
                    } else if nx == b'=' {
                        self.i += 1;
                        T::Op
                    } else {
                        T::Gt
                    }
                }
                b'=' => {
                    if nx == b'=' {
                        self.i += 1;
                        T::Op
                    } else {
                        T::Assign
                    }
                }
                b'-' => {
                    if nx == b'>' {
                        self.i += 1;
                        if self.at(self.i) == b'*' {
                            self.i += 1;
                            T::Op
                        } else {
                            T::Arrow
                        }
                    } else {
                        if nx == b'-' || nx == b'=' {
                            self.i += 1;
                        }
                        T::Op
                    }
                }
                b'.' => {
                    if nx == b'.' && self.at(self.i + 1) == b'.' {
                        self.i += 2;
                        T::Op
                    } else if nx == b'*' {
                        self.i += 1;
                        T::Op
                    } else {
                        T::Dot
                    }
                }
                _ => {
                    // `+=`, `++`, `||`, `!=`, ...
                    if nx == b'=' || (nx == b && matches!(b, b'+' | b'|')) {
                        self.i += 1;
                    }
                    T::Op
                }
            }
        };
        Tok { t, s: start as u32, e: self.i as u32 }
    }

    fn line_comment(&mut self) {
        let s = self.src;
        let n = s.len();
        loop {
            match memchr::memchr(b'\n', &s[self.i..]) {
                Some(j) => {
                    let nl = self.i + j;
                    self.i = nl;
                    // a backslash before the newline continues the comment
                    let before = if nl > 0 && s[nl - 1] == b'\r' { nl.saturating_sub(2) } else { nl.saturating_sub(1) };
                    if nl > 0 && s[before] == b'\\' {
                        self.i = nl + 1;
                        continue;
                    }
                    return;
                }
                None => {
                    self.i = n;
                    return;
                }
            }
        }
    }

    /// Skip a preprocessor directive: to the end of the line, following continuations and block
    /// comments that run over it.
    fn directive(&mut self) {
        let s = self.src;
        let n = s.len();
        while self.i < n {
            match s[self.i] {
                b'\n' => return,
                b'\\' if self.at(self.i + 1) == b'\n' => self.i += 2,
                b'\\' if self.at(self.i + 1) == b'\r' && self.at(self.i + 2) == b'\n' => self.i += 3,
                b'/' if self.at(self.i + 1) == b'*' => {
                    self.i = memchr::memmem::find(&s[self.i + 2..], b"*/").map(|j| self.i + 2 + j + 2).unwrap_or(n);
                }
                b'/' if self.at(self.i + 1) == b'/' => {
                    self.line_comment();
                    return;
                }
                b'"' => self.quoted(b'"'),
                _ => self.i += 1,
            }
        }
    }

    fn number(&mut self) {
        let s = self.src;
        let n = s.len();
        while self.i < n {
            let b = s[self.i];
            if b.is_ascii_alphanumeric() || b == b'.' || b == b'_' {
                self.i += 1;
            } else if b == b'\'' && self.at(self.i + 1).is_ascii_alphanumeric() {
                // digit separator
                self.i += 2;
            } else if (b == b'+' || b == b'-') && matches!(s[self.i - 1], b'e' | b'E' | b'p' | b'P') {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    /// A string or character literal starting at the current position (at an optional prefix:
    /// the opening quote is searched from here). Stops at the end of the line when unterminated.
    fn quoted(&mut self, q: u8) {
        let s = self.src;
        let n = s.len();
        while self.i < n && s[self.i] != q {
            self.i += 1;
        }
        self.i += 1;
        while self.i < n {
            let b = s[self.i];
            if b == b'\\' {
                self.i += 2;
            } else if b == q {
                self.i += 1;
                return;
            } else if b == b'\n' {
                return;
            } else {
                self.i += 1;
            }
        }
        self.i = self.i.min(n);
    }

    /// `R"delim( ... )delim"`, positioned at the opening quote.
    fn raw_string(&mut self) {
        let s = self.src;
        let n = s.len();
        let open = self.i + 1;
        let Some(p) = memchr::memchr(b'(', &s[open.min(n)..(open + 18).min(n)]) else {
            self.quoted(b'"');
            return;
        };
        let delim = &s[open..open + p];
        let mut i = open + p + 1;
        while let Some(j) = memchr::memchr(b')', &s[i.min(n)..]) {
            let close = i + j;
            let after = close + 1;
            if s[after.min(n)..].starts_with(delim) && self.at(after + delim.len()) == b'"' {
                self.i = after + delim.len() + 1;
                return;
            }
            i = close + 1;
        }
        self.i = n;
    }
}

/// Uppercase letters, digits and underscores only, with at least one letter.
fn allcaps(w: &[u8]) -> bool {
    w.len() >= 2
        && !w[0].is_ascii_digit()
        && w.iter().all(|&b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && w.iter().any(|b| b.is_ascii_uppercase())
}

/// Looks like a macro rather than a type or a variable: all caps and long, or all caps with an
/// underscore (`LLVM_ABI`, `EXPORT`, `WINAPI`; not `DWORD`, `SCEV`, `TLI`).
fn macro_shaped(w: &[u8]) -> bool {
    allcaps(w) && (w.len() >= 6 || (w.len() >= 4 && w.contains(&b'_')))
}

/// Lower-case attribute macros by their suffix: `simdjson_inline`, `nssv_noexcept`,
/// `foo_warn_unused`.
fn attr_like(w: &[u8]) -> bool {
    w.len() >= 8
        && w.contains(&b'_')
        && (w.ends_with(b"inline")
            || w.ends_with(b"unused")
            || w.ends_with(b"noexcept")
            || w.ends_with(b"nodiscard")
            || w.ends_with(b"noreturn")
            || w.ends_with(b"deprecated")
            || w.ends_with(b"_export")
            || w.ends_with(b"_api")
            || contains(w, b"constexpr"))
}

/// A name that looks like an annotation macro.
fn annotation_shaped(w: &[u8]) -> bool {
    macro_shaped(w) || attr_like(w)
}

fn is_export_macro(w: &[u8]) -> bool {
    w.ends_with(b"ABI")
        || w.ends_with(b"API")
        || w.ends_with(b"EXPORT")
        || w.ends_with(b"EXPORTS")
        || w.ends_with(b"EXPORTED")
        || w.ends_with(b"VISIBILITY")
        || w.ends_with(b"PUBLIC")
}

/// Calling-convention macros stand between the return type and the name.
fn is_callconv(w: &[u8]) -> bool {
    w.ends_with(b"CALL")
        || w.ends_with(b"CALLTYPE")
        || w.ends_with(b"CALLBACK")
        || w.ends_with(b"API")
        || w.ends_with(b"APIV")
        || w.ends_with(b"ENTRY")
        || w == b"PASCAL"
}

/// Keywords that can only start (or continue the specifiers of) a declaration.
fn kw_decl(w: &[u8]) -> bool {
    matches!(
        w,
        b"void"
            | b"bool"
            | b"char"
            | b"short"
            | b"int"
            | b"long"
            | b"float"
            | b"double"
            | b"signed"
            | b"unsigned"
            | b"wchar_t"
            | b"char8_t"
            | b"char16_t"
            | b"char32_t"
            | b"auto"
            | b"static"
            | b"extern"
            | b"inline"
            | b"virtual"
            | b"explicit"
            | b"friend"
            | b"constexpr"
            | b"consteval"
            | b"constinit"
            | b"typename"
            | b"struct"
            | b"class"
            | b"union"
            | b"enum"
            | b"typedef"
            | b"template"
            | b"mutable"
            | b"thread_local"
            | b"public"
            | b"private"
            | b"protected"
            | b"namespace"
            | b"using"
    )
}

/// Keywords no name can directly precede in valid code: built-in types, class keys, access
/// specifiers, `template`, `namespace`, `using`.
fn kw_after_annotation(w: &[u8]) -> bool {
    matches!(
        w,
        b"void"
            | b"bool"
            | b"char"
            | b"short"
            | b"int"
            | b"long"
            | b"float"
            | b"double"
            | b"signed"
            | b"unsigned"
            | b"wchar_t"
            | b"char8_t"
            | b"char16_t"
            | b"char32_t"
            | b"struct"
            | b"class"
            | b"union"
            | b"enum"
            | b"template"
            | b"namespace"
            | b"using"
            | b"public"
            | b"private"
            | b"protected"
    )
}

fn kw_builtin_type(w: &[u8]) -> bool {
    matches!(
        w,
        b"void"
            | b"bool"
            | b"char"
            | b"short"
            | b"int"
            | b"long"
            | b"float"
            | b"double"
            | b"signed"
            | b"unsigned"
            | b"wchar_t"
            | b"char8_t"
            | b"char16_t"
            | b"char32_t"
            | b"auto"
    )
}

fn lex_keyword(w: &[u8]) -> bool {
    kw_decl(w)
        || matches!(
            w,
            b"if"
                | b"else"
                | b"do"
                | b"while"
                | b"for"
                | b"return"
                | b"sizeof"
                | b"const"
                | b"volatile"
                | b"register"
                | b"goto"
                | b"break"
                | b"continue"
                | b"switch"
                | b"case"
                | b"default"
                | b"alignof"
                | b"alignas"
                | b"catch"
                | b"co_await"
                | b"co_return"
                | b"co_yield"
                | b"concept"
                | b"const_cast"
                | b"decltype"
                | b"delete"
                | b"dynamic_cast"
                | b"export"
                | b"final"
                | b"new"
                | b"noexcept"
                | b"nullptr"
                | b"operator"
                | b"override"
                | b"reinterpret_cast"
                | b"requires"
                | b"static_assert"
                | b"static_cast"
                | b"this"
                | b"throw"
                | b"try"
                | b"typeid"
                | b"true"
                | b"false"
                | b"and"
                | b"or"
                | b"not"
                | b"xor"
                | b"__attribute__"
                | b"__attribute"
                | b"__declspec"
                | b"__extension__"
                | b"__typeof__"
                | b"typeof"
                | b"__cdecl"
                | b"__stdcall"
                | b"__fastcall"
                | b"__thiscall"
                | b"__vectorcall"
                | b"__clrcall"
                | b"__inline"
                | b"__inline__"
                | b"__forceinline"
                | b"__restrict"
                | b"__restrict__"
                | b"__unaligned"
                | b"__thread"
                | b"__asm__"
                | b"__asm"
                | b"asm"
                | b"restrict"
                | b"_Atomic"
                | b"_Noreturn"
                | b"_Nonnull"
                | b"_Alignas"
                | b"__based"
        )
}

/// Skip to the `)` matching an already consumed `(`. Returns the offset behind it.
fn skip_parens(l: &mut Lx) -> Option<u32> {
    let mut depth = 1u32;
    for _ in 0..4096 {
        let t = l.next();
        match t.t {
            T::LParen => depth += 1,
            T::RParen => {
                depth -= 1;
                if depth == 0 {
                    return Some(t.e);
                }
            }
            T::Eof | T::Dir => return None,
            _ => {}
        }
    }
    None
}

/// Skip to the `>` matching an already consumed `<`.
fn skip_angles(l: &mut Lx) -> bool {
    let mut depth = 1i32;
    for _ in 0..512 {
        let t = l.next();
        match t.t {
            T::Lt => depth += 1,
            T::Gt => depth -= 1,
            T::Shr => depth -= 2,
            T::LParen => {
                if skip_parens(l).is_none() {
                    return false;
                }
            }
            T::Semi | T::LBrace | T::RBrace | T::Eof | T::Dir => return false,
            _ => {}
        }
        if depth <= 0 {
            return true;
        }
    }
    false
}

/// Skip to the `]` matching an already consumed `[`.
fn skip_brackets(l: &mut Lx) -> bool {
    let mut depth = 1u32;
    for _ in 0..256 {
        let t = l.next();
        match t.t {
            T::LBrack => depth += 1,
            T::RBrack => {
                depth -= 1;
                if depth == 0 {
                    return true;
                }
            }
            T::Semi | T::LBrace | T::RBrace | T::Eof | T::Dir => return false,
            _ => {}
        }
    }
    false
}

/// After `MACRO B` followed by `::` or `<` (`first`): does a declared name follow the type that
/// starts at `B`? `MACRO std::string f()`, `MACRO Expected<T> g()` and `MACRO Foo::Foo()` /
/// `MACRO Foo::~Foo()` (a constructor or destructor has no return type) say yes;
/// `TYPE Foo::bar()` says no: there the first name is the return type.
fn name_follows_type(l: &mut Lx, first: T, first_word: &[u8]) -> bool {
    let mut t = first;
    let mut last: &[u8] = first_word;
    let mut prev: &[u8] = b"";
    let mut dtor = false;
    for _ in 0..16 {
        match t {
            T::Lt => {
                if !skip_angles(l) {
                    return false;
                }
            }
            T::Scope => {
                let mut x = l.next();
                if x.t == T::Ident && l.word(x) == b"template" {
                    x = l.next();
                }
                if x.t == T::Tilde {
                    dtor = true;
                    x = l.next();
                }
                if x.t != T::Ident {
                    return false;
                }
                let w = l.word(x);
                if w == b"operator" {
                    return false;
                }
                prev = last;
                last = w;
            }
            _ => return false,
        }
        let nx = l.next();
        match nx.t {
            T::Scope | T::Lt => t = nx.t,
            T::Ident => {
                let w = l.word(nx);
                return !lex_keyword(w) || w == b"operator";
            }
            T::Star | T::Amp | T::AmpAmp => {
                let y = l.next();
                return matches!(y.t, T::Ident | T::Star | T::Amp | T::AmpAmp);
            }
            T::LParen => return dtor || (!prev.is_empty() && prev == last),
            _ => return false,
        }
    }
    false
}

/// Find annotation macros in `src` and write a copy with them blanked to `out`. Returns false
/// (and leaves `out` alone) when there are none.
fn blank_macros(src: &[u8], out: &mut Vec<u8>, blanks: &mut Vec<Blank>) -> bool {
    let mut lx = Lx::new(src);
    let bof = Tok { t: T::Dir, s: 0, e: 0 };
    // the last three tokens that were kept, most recent first
    let (mut prev, mut prev2, mut prev3) = (bof, bof, bof);
    let mut unordered = false;
    // for every open parenthesis: does it open the parameter list of a function declarator?
    let mut pstack = [false; 64];
    let mut pdepth = 0usize;
    // just behind the parameter list of a function declarator and its qualifiers
    let mut trail = false;
    let mut after_operator = false;
    let mut blank_until = 0u32;
    // the `[[...]]` attribute that ended at the previous kept token: (start, end)
    let mut attr_open = 0u32;
    let mut attr = (0u32, 0u32);
    // inside a constructor's member initialiser list, and the comma in front of the directives
    // blanked there
    let mut ctor_init = false;
    let mut init_comma: Option<Tok> = None;
    // brace depth, and the depth of the enumerator list being read
    let mut bdepth = 0i32;
    let mut enum_pending = false;
    let mut enum_body: Option<i32> = None;
    loop {
        let tok = lx.next();
        if tok.t == T::Eof {
            break;
        }
        if tok.s < blank_until {
            continue;
        }
        // `: a(x), b(y),\n#define GET_INITS\n#include "Gen.inc"\n#undef GET_INITS\n{ }`: the
        // grammar has no directives inside an initialiser list and loses the constructor body
        // and the definitions after it. The directives go, and with them the comma that the
        // included text would have continued.
        if ctor_init {
            match tok.t {
                T::Dir if prev.t == T::Comma && tok.e > tok.s => {
                    blanks.push(Blank { off: tok.s, name_len: 0, len: tok.e - tok.s });
                    init_comma = Some(prev);
                    continue;
                }
                T::LBrace if matches!(prev.t, T::RParen | T::RBrace | T::Comma) => {
                    if let Some(cm) = init_comma.take() {
                        if prev.s == cm.s {
                            blanks.push(Blank { off: cm.s, name_len: 0, len: cm.e - cm.s });
                            unordered = true;
                        }
                    }
                    ctor_init = false;
                }
                T::Semi => {
                    ctor_init = false;
                    init_comma = None;
                }
                _ => {}
            }
        } else if tok.t == T::Colon && trail && pdepth == 0 {
            ctor_init = true;
            init_comma = None;
        }
        match tok.t {
            T::LBrace => {
                bdepth += 1;
                if std::mem::take(&mut enum_pending) {
                    enum_body = Some(bdepth);
                }
            }
            T::RBrace => {
                if enum_body == Some(bdepth) {
                    enum_body = None;
                }
                bdepth -= 1;
            }
            T::Semi => enum_pending = false,
            // `enum E { A, B, LLVM_MARK_AS_BITMASK_ENUM(B) };`: a macro call standing where an
            // enumerator is expected. The grammar cannot read it and may lose everything up to
            // the next place it can resynchronise.
            T::Ident if enum_body == Some(bdepth) && pdepth == 0 && matches!(prev.t, T::LBrace | T::Comma | T::Dir) && allcaps(lx.word(tok)) => {
                let mut p = lx.clone();
                if p.next().t == T::LParen {
                    if let Some(end) = skip_parens(&mut p) {
                        if matches!(p.next().t, T::RBrace | T::Comma) {
                            blanks.push(Blank { off: tok.s, name_len: tok.e - tok.s, len: end - tok.s });
                            blank_until = end;
                            continue;
                        }
                    }
                }
            }
            _ => {}
        }
        let mut keep_trail = false;
        match tok.t {
            T::LBrack if prev.t == T::LBrack && prev.e == tok.s => attr_open = prev.s,
            T::RBrack if prev.t == T::RBrack && prev.e == tok.s => attr = (attr_open, tok.e),
            T::LParen => {
                let ok = match prev.t {
                    T::Ident => {
                        let w = lx.word(prev);
                        if lex_keyword(w) { (trail && matches!(w, b"noexcept" | b"throw")) || after_operator } else { !macro_shaped(w) }
                    }
                    T::Gt | T::RParen | T::RBrack => true,
                    _ => after_operator,
                };
                after_operator = false;
                if pdepth < pstack.len() {
                    pstack[pdepth] = ok;
                }
                pdepth += 1;
            }
            T::RParen => {
                pdepth = pdepth.saturating_sub(1);
                trail = pdepth < pstack.len() && pstack[pdepth];
                keep_trail = true;
            }
            T::Amp | T::AmpAmp => keep_trail = true,
            T::Ident => {
                let w = lx.word(tok);
                // `annotation void f()`, `Q_OBJECT public:`: a name directly in front of a type
                // keyword or a class key is not part of the declaration, whatever its case.
                // (Not so in front of a specifier: `Foo static x;` is valid, and so is
                // `Concept auto x`.)
                if kw_after_annotation(w) {
                    while prev.t == T::Ident && !lex_keyword(lx.word(prev)) && !matches!(prev2.t, T::Dot | T::Arrow | T::Scope) && prev.s >= blank_until {
                        blanks.push(Blank { off: prev.s, name_len: prev.e - prev.s, len: prev.e - prev.s });
                        unordered = true;
                        prev = prev2;
                        prev2 = prev3;
                        prev3 = bof;
                    }
                }
                // `[[nodiscard]] friend T f();`: the grammar has no attributes in front of
                // `friend` and reads `friend` as a type name
                if w == b"friend" && prev.t == T::RBrack && prev.e == attr.1 && attr.0 < attr.1 {
                    blanks.push(Blank { off: attr.0, name_len: 0, len: attr.1 - attr.0 });
                    unordered = true;
                }
                if trail && matches!(w, b"const" | b"volatile" | b"noexcept" | b"override" | b"final" | b"mutable" | b"throw") {
                    keep_trail = true;
                } else if matches!(w, b"class" | b"struct" | b"union" | b"enum") {
                    enum_pending |= w == b"enum";
                    if let Some(end) = class_head(&lx, w, blanks) {
                        blank_until = end;
                    }
                } else if w == b"operator" {
                    after_operator = true;
                } else if annotation_shaped(w) && !matches!(prev.t, T::Dot | T::Arrow | T::Scope) {
                    if let Some(end) = annotation_end(&lx, tok, prev, prev2, trail) {
                        blanks.push(Blank { off: tok.s, name_len: tok.e - tok.s, len: end - tok.s });
                        blank_until = end;
                        // the macro is gone: what was before it is still what is before the next token
                        continue;
                    }
                }
            }
            _ => {}
        }
        if tok.t != T::Ident && tok.t != T::LParen {
            after_operator &= matches!(
                tok.t,
                T::Op | T::Star | T::Amp | T::AmpAmp | T::Lt | T::Gt | T::Shr | T::Assign | T::Tilde | T::Comma | T::Arrow | T::LBrack | T::RBrack
            );
        }
        if !keep_trail {
            trail = false;
        }
        prev3 = prev2;
        prev2 = prev;
        prev = tok;
    }
    if blanks.is_empty() {
        return false;
    }
    if unordered {
        blanks.sort_unstable_by_key(|b| b.off);
        blanks.dedup_by_key(|b| b.off);
    }
    out.clear();
    out.extend_from_slice(src);
    for b in blanks.iter() {
        let (s, e) = (b.off as usize, (b.off + b.len) as usize);
        if e > out.len() {
            continue;
        }
        for x in &mut out[s..e] {
            if *x != b'\n' && *x != b'\r' {
                *x = b' ';
            }
        }
    }
    true
}

/// At a class key: `class MACRO Name {`, `struct MACRO(args) Name : Base {`,
/// `class MACRO Name final {`, `class MACRO Name<T> {`. Two names in a row before the body or
/// the base clause cannot both be the class name: all but the last are macros. Returns the end
/// of the blanked range.
fn class_head(lx: &Lx, key: &[u8], blanks: &mut Vec<Blank>) -> Option<u32> {
    let mut l = lx.clone();
    let mut ids = [(0u32, 0u32, 0u32); 6];
    let mut n = 0usize;
    let mut forward = false;
    let mut t = l.next();
    if key == b"enum" && t.t == T::Ident && matches!(l.word(t), b"class" | b"struct") {
        t = l.next();
    }
    loop {
        match t.t {
            T::Ident => {
                let w = l.word(t);
                if w == b"final" {
                    let mut p = l.clone();
                    if matches!(p.next().t, T::LBrace | T::Colon) {
                        break;
                    }
                }
                if lex_keyword(w) || n == ids.len() {
                    return None;
                }
                let mut end = t.e;
                let mut p = l.clone();
                let t2 = p.next();
                if t2.t == T::LParen && allcaps(w) {
                    end = skip_parens(&mut p)?;
                    l = p;
                    ids[n] = (t.s, t.e, end);
                    n += 1;
                    t = l.next();
                } else {
                    ids[n] = (t.s, t.e, end);
                    n += 1;
                    l = p;
                    t = t2;
                }
            }
            T::LBrack => {
                // `class [[nodiscard]] Foo`
                if !skip_brackets(&mut l) {
                    return None;
                }
                t = l.next();
            }
            T::LBrace | T::Colon | T::Lt => break,
            T::Semi => {
                forward = true;
                break;
            }
            _ => return None,
        }
    }
    if n < 2 || !ids[..n - 1].iter().all(|id| allcaps(&lx.src[id.0 as usize..id.1 as usize])) {
        return None;
    }
    if forward {
        // `class LLVM_ABI Foo;` is a forward declaration, `struct stat st;` a variable: only an
        // export-style macro makes it the former
        let w = &lx.src[ids[0].0 as usize..ids[0].1 as usize];
        if n != 2 || !macro_shaped(w) || !(key == b"class" || is_export_macro(w)) {
            return None;
        }
    }
    for id in &ids[..n - 1] {
        blanks.push(Blank { off: id.0, name_len: id.1 - id.0, len: id.2 - id.0 });
    }
    Some(ids[n - 2].2)
}

/// Decide whether the macro-shaped identifier `tok` is an annotation: a name standing where
/// valid C++ has none. Returns the end of the text to blank (the name and its arguments).
fn annotation_end(lx: &Lx, tok: Tok, prev: Tok, prev2: Tok, trail: bool) -> Option<u32> {
    let w = lx.word(tok);
    let mut l = lx.clone();
    let mut n1 = l.next();
    let mut end = tok.e;
    let mut parens = false;
    if n1.t == T::LParen {
        end = skip_parens(&mut l)?;
        parens = true;
        n1 = l.next();
    }

    // behind a function declarator: `bool f() const LLVM_READONLY;`, `void g() REQUIRES(mu) {`
    if trail {
        let ok = match n1.t {
            T::Semi | T::LBrace | T::Assign | T::Colon | T::Arrow => true,
            T::Ident => {
                let w1 = l.word(n1);
                matches!(w1, b"override" | b"final" | b"const" | b"noexcept" | b"volatile" | b"try" | b"requires") || annotation_shaped(w1)
            }
            _ => false,
        };
        if ok {
            return Some(end);
        }
    }

    let prev_word = if prev.t == T::Ident { lx.word(prev) } else { b"" };
    let prev_is_name = prev.t == T::Ident && !lex_keyword(prev_word);

    // behind an array declarator: `int buf[N] GUARDED_BY(mu) = {};`
    if prev.t == T::RBrack && parens && matches!(n1.t, T::Semi | T::Assign | T::LBrace) {
        return Some(end);
    }

    // behind a declared name: `int x_ GUARDED_BY(mu_);`, `(const char *s LIFETIME_BOUND)`
    if prev_is_name && matches!(n1.t, T::Semi | T::Comma | T::RParen | T::Assign | T::LBrace | T::LBrack) {
        let after_declarator = match prev2.t {
            T::Star | T::Amp | T::AmpAmp | T::Gt => true,
            T::Ident => {
                let w2 = lx.word(prev2);
                // `using alias MACRO = type;`
                kw_builtin_type(w2) || !lex_keyword(w2) || (w2 == b"using" && n1.t == T::Assign)
            }
            _ => false,
        };
        return if after_declarator { Some(end) } else { None };
    }

    // between a type and a name: `void WINAPI f()`, `HRESULT STDMETHODCALLTYPE QueryInterface(`
    if prev_is_name || (prev.t == T::Ident && kw_builtin_type(prev_word)) {
        if parens {
            return None;
        }
        return match n1.t {
            T::Ident if !lex_keyword(l.word(n1)) => Some(end),
            T::Star | T::Amp => {
                let x = l.next();
                if x.t == T::Ident && !lex_keyword(l.word(x)) { Some(end) } else { None }
            }
            _ => None,
        };
    }

    // in front of a declaration
    let lead_ok = match prev.t {
        T::Ident => {
            !matches!(prev_word, b"struct" | b"class" | b"union" | b"enum" | b"goto" | b"namespace" | b"new" | b"operator" | b"using" | b"typename" | b"case")
        }
        // `[[nodiscard]] MACRO int f();`
        T::RBrack => prev2.t == T::RBrack,
        T::Num | T::RParen | T::Dot | T::Arrow | T::Scope => false,
        _ => true,
    };
    if !lead_ok {
        return None;
    }
    let mut a = n1;
    let mut cv = false;
    while a.t == T::Ident && matches!(l.word(a), b"const" | b"volatile") {
        a = l.next();
        cv = true;
    }
    match a.t {
        T::Ident => {
            let w1 = l.word(a);
            if kw_decl(w1) {
                // `MACRO void f();`, `MACRO(x) unsigned bits : 1;` — but `DEFINE_X(n)` defines n
                return if parens && is_define_macro(w) { None } else { Some(end) };
            }
            if lex_keyword(w1) {
                return None;
            }
            let b = l.next();
            match b.t {
                T::Ident => {
                    // three names in a row: one of the first two is not part of the declaration
                    let w2 = l.word(b);
                    if kw_decl(w2) {
                        // `MACRO1 MACRO2 void f();`
                        return Some(end);
                    }
                    if lex_keyword(w2) && w2 != b"operator" {
                        return None;
                    }
                    if !parens && !cv && macro_shaped(w1) && is_callconv(w1) { None } else { Some(end) }
                }
                // `MACRO Foo(int x) : x_(x) {}` / `MACRO Foo() = default;`: only a constructor has
                // an initialiser list or is defaulted, and a constructor has no return type
                T::LParen if !parens && !cv => {
                    skip_parens(&mut l)?;
                    let mut x = l.next();
                    for _ in 0..6 {
                        if x.t != T::Ident {
                            break;
                        }
                        let wx = l.word(x);
                        if !(matches!(wx, b"noexcept" | b"const") || annotation_shaped(wx)) {
                            break;
                        }
                        x = l.next();
                        if x.t == T::LParen {
                            skip_parens(&mut l)?;
                            x = l.next();
                        }
                    }
                    match x.t {
                        T::Colon => Some(end),
                        T::Assign => {
                            let y = l.next();
                            if y.t == T::Ident && l.word(y) == b"default" { Some(end) } else { None }
                        }
                        _ => None,
                    }
                }
                T::Star | T::Amp | T::AmpAmp => {
                    let x = l.next();
                    if matches!(x.t, T::Ident | T::Star | T::Amp | T::AmpAmp) { Some(end) } else { None }
                }
                T::Scope | T::Lt => {
                    if name_follows_type(&mut l, b.t, w1) {
                        Some(end)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
        // `MACRO ~Foo();`
        T::Tilde if !parens && !cv && matches!(prev.t, T::Semi | T::LBrace | T::RBrace | T::Colon | T::Dir) => {
            let b = l.next();
            let x = l.next();
            if b.t == T::Ident && x.t == T::LParen { Some(end) } else { None }
        }
        _ => None,
    }
}
