//! Swift extractor (tree-sitter-swift).
//!
//! A Swift module (a SwiftPM target, an Xcode target) has no imports between its files: every
//! declaration that is not `private` is visible in all files of the module, and a type is often
//! spread over several files through `extension T`. The resolver therefore treats the module as
//! the package of a file (see `Resolver::swift_module`), and this extractor records:
//!   * `extension T` as an `Impl` definition named `T` (the form of a Rust `impl T` block): its
//!     members have the `Impl` as parent and the resolver attaches the block to the type `T`
//!     the file can see, also across files and modules. An extension of a type this repository
//!     does not define (`extension String`) still owns its members.
//!   * the members each type and extension of this file declares (collected before the walk), so
//!     that a bare `count` or `reload()` inside a type is attributed to the type that declares
//!     it. A bare name no type of this file declares, inside a type, is qualified by the
//!     innermost enclosing type: it may be inherited, come from an extension in another file or
//!     be a global, and the resolver tries the type first and the module scope second.
//!   * the type of every local that syntax can tell (parameters, `let x: T`, `let x = T()`,
//!     `let x = T.make()`, `if let`, `guard let`, `for`, `catch let e as E`), so that member
//!     accesses carry a receiver type path. Optional chaining and forced unwraps keep the type.
//!   * the contextual type of an implicit member expression (`.idle`, `.init(..)`,
//!     `.make()`) where syntax shows it: an annotated declaration, a `return` in a function
//!     with a declared result, an assignment, a comparison and the subject of a `switch`.
//!
//! Conventions the resolver relies on:
//!   * Kinds. `class` and `actor` are `Class`, `struct` `Struct`, `enum` `Enum` with its cases as
//!     `EnumMember` (arity: the associated values), `protocol` `Interface` whose requirements
//!     are `DECL | ABSTRACT`, `associatedtype` a declared `TypeAlias`. Stored properties are
//!     `Field` (`FNPTR` when function typed), computed ones `Property`; globals are `Constant`
//!     (`let`) or `Variable` (`var`). Initialisers are methods named `init`, deinitialisers
//!     `deinit`, subscripts `subscript`, operator functions are named by their operator.
//!     `private` and `fileprivate` are `LOCAL`, `public` and `open` `EXPORTED`.
//!   * Calls. `f()` inside a type is a `Call` qualified by the type that declares `f` in this
//!     file, else by the innermost enclosing type. `self.f()` is a `MemberCall C`,
//!     `super.f()` a `MemberCall ^C`, `T.f()` a static `Call T`. `T(..)`, `T.init(..)` and
//!     `.init(..)` with a known contextual type are `New T`. A trailing closure is an argument.
//!   * Inheritance. In a class the first entry of the inheritance clause is the superclass
//!     (`Extends`) unless this file declares it as a protocol or its name reads like one; every
//!     other entry, and every entry of a struct, enum, actor or extension, is a conformance
//!     (`Implements`). A protocol `Extends` the protocols it refines.
//!   * Callbacks. A function stored into a member (`button.onTap = handle`) is a `Bind` of the
//!     slot. A non-local name or a member of `self` or a type handed to a call (`map(transform)`,
//!     `#selector(T.method)`) is an `Arg` of that call, next to its `Read` or `MemberRead`.

use crate::extract::*;
use crate::facts::*;
use crate::lang::Lang;
use rustc_hash::FxHashSet;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::OnceLock;
use tree_sitter::{Node, Parser, Tree, TreeCursor};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    /// Named nodes without special handling: recurse into the children.
    Other,
    /// Subtrees that never contain references.
    Skip,
    Comment,
    Error,
    SimpleIdentifier,
    TypeIdentifier,
    UserType,
    ImportDeclaration,
    ClassDeclaration,
    ProtocolDeclaration,
    ClassBody,
    FunctionDeclaration,
    ProtocolFunctionDeclaration,
    InitDeclaration,
    DeinitDeclaration,
    SubscriptDeclaration,
    PropertyDeclaration,
    ProtocolPropertyDeclaration,
    EnumEntry,
    TypealiasDeclaration,
    AssociatedtypeDeclaration,
    MacroDeclaration,
    Modifiers,
    Attribute,
    InheritanceSpecifier,
    TypeParameters,
    TypeParameter,
    Parameter,
    FunctionBody,
    ComputedProperty,
    Accessor,
    Observers,
    Observer,
    TypeAnnotation,
    OptionalType,
    ArrayType,
    DictionaryType,
    FunctionType,
    WrapperType,
    TupleType,
    TupleTypeItem,
    CallExpression,
    CallSuffix,
    ValueArguments,
    ValueArgument,
    ConstructorExpression,
    ConstructorSuffix,
    NavigationExpression,
    NavigationSuffix,
    PrefixExpression,
    PostfixExpression,
    SelfExpression,
    SuperExpression,
    Assignment,
    DirectlyAssignable,
    LambdaLiteral,
    CaptureList,
    CaptureListItem,
    LambdaFunctionType,
    LambdaParameter,
    KeyPathExpression,
    SelectorExpression,
    MacroInvocation,
    IfStatement,
    GuardStatement,
    WhileStatement,
    ForStatement,
    SwitchStatement,
    SwitchEntry,
    SwitchPattern,
    Pattern,
    CatchBlock,
    DoStatement,
    ControlTransfer,
    Transparent,
    TernaryExpression,
    EqualityExpression,
    AsExpression,
    ValueBindingPattern,
    TupleExpression,
}

struct Tables {
    kinds: KindTable<K>,
    f_name: u16,
    f_body: u16,
    f_kind: u16,
    f_return_type: u16,
    f_default_value: u16,
    f_external_name: u16,
    f_type: u16,
    f_value: u16,
    f_target: u16,
    f_suffix: u16,
    f_result: u16,
    f_bound_identifier: u16,
    f_item: u16,
    f_collection: u16,
    f_expr: u16,
    f_error: u16,
    f_inherits_from: u16,
    f_constructed_type: u16,
    f_wrapped: u16,
    f_element: u16,
    f_data_contents: u16,
    f_raw_value: u16,
    f_if_true: u16,
    f_if_false: u16,
    f_lhs: u16,
    f_rhs: u16,
    f_reference_specifier: u16,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let lang: tree_sitter::Language = tree_sitter_swift::LANGUAGE.into();
        let kinds = KindTable::build(
            &lang,
            K::Other,
            &[
                ("ERROR", K::Error),
                ("comment", K::Comment),
                ("multiline_comment", K::Comment),
                ("simple_identifier", K::SimpleIdentifier),
                ("type_identifier", K::TypeIdentifier),
                ("user_type", K::UserType),
                ("import_declaration", K::ImportDeclaration),
                ("class_declaration", K::ClassDeclaration),
                ("protocol_declaration", K::ProtocolDeclaration),
                ("class_body", K::ClassBody),
                ("enum_class_body", K::ClassBody),
                ("protocol_body", K::ClassBody),
                ("function_declaration", K::FunctionDeclaration),
                ("protocol_function_declaration", K::ProtocolFunctionDeclaration),
                ("init_declaration", K::InitDeclaration),
                ("deinit_declaration", K::DeinitDeclaration),
                ("subscript_declaration", K::SubscriptDeclaration),
                ("property_declaration", K::PropertyDeclaration),
                ("protocol_property_declaration", K::ProtocolPropertyDeclaration),
                ("enum_entry", K::EnumEntry),
                ("typealias_declaration", K::TypealiasDeclaration),
                ("associatedtype_declaration", K::AssociatedtypeDeclaration),
                ("macro_declaration", K::MacroDeclaration),
                ("modifiers", K::Modifiers),
                ("attribute", K::Attribute),
                ("inheritance_specifier", K::InheritanceSpecifier),
                ("type_parameters", K::TypeParameters),
                ("type_parameter", K::TypeParameter),
                ("parameter", K::Parameter),
                ("function_body", K::FunctionBody),
                ("computed_property", K::ComputedProperty),
                ("computed_getter", K::Accessor),
                ("computed_setter", K::Accessor),
                ("computed_modify", K::Accessor),
                ("willset_didset_block", K::Observers),
                ("willset_clause", K::Observer),
                ("didset_clause", K::Observer),
                ("type_annotation", K::TypeAnnotation),
                ("optional_type", K::OptionalType),
                ("array_type", K::ArrayType),
                ("dictionary_type", K::DictionaryType),
                ("function_type", K::FunctionType),
                ("opaque_type", K::WrapperType),
                ("existential_type", K::WrapperType),
                ("type_modifiers", K::Skip),
                ("tuple_type", K::TupleType),
                ("tuple_type_item", K::TupleTypeItem),
                ("call_expression", K::CallExpression),
                ("call_suffix", K::CallSuffix),
                ("value_arguments", K::ValueArguments),
                ("value_argument", K::ValueArgument),
                ("constructor_expression", K::ConstructorExpression),
                ("constructor_suffix", K::ConstructorSuffix),
                ("navigation_expression", K::NavigationExpression),
                ("navigation_suffix", K::NavigationSuffix),
                ("prefix_expression", K::PrefixExpression),
                ("postfix_expression", K::PostfixExpression),
                ("self_expression", K::SelfExpression),
                ("super_expression", K::SuperExpression),
                ("assignment", K::Assignment),
                ("directly_assignable_expression", K::DirectlyAssignable),
                ("lambda_literal", K::LambdaLiteral),
                ("capture_list", K::CaptureList),
                ("capture_list_item", K::CaptureListItem),
                ("lambda_function_type", K::LambdaFunctionType),
                ("lambda_parameter", K::LambdaParameter),
                ("key_path_expression", K::KeyPathExpression),
                ("selector_expression", K::SelectorExpression),
                ("macro_invocation", K::MacroInvocation),
                ("if_statement", K::IfStatement),
                ("guard_statement", K::GuardStatement),
                ("while_statement", K::WhileStatement),
                ("repeat_while_statement", K::WhileStatement),
                ("for_statement", K::ForStatement),
                ("switch_statement", K::SwitchStatement),
                ("switch_entry", K::SwitchEntry),
                ("switch_pattern", K::SwitchPattern),
                ("pattern", K::Pattern),
                ("catch_block", K::CatchBlock),
                ("do_statement", K::DoStatement),
                ("control_transfer_statement", K::ControlTransfer),
                ("try_expression", K::Transparent),
                ("await_expression", K::Transparent),
                ("unsafe_expression", K::Transparent),
                ("consume_expression", K::Transparent),
                ("ternary_expression", K::TernaryExpression),
                ("equality_expression", K::EqualityExpression),
                ("as_expression", K::AsExpression),
                ("value_binding_pattern", K::ValueBindingPattern),
                ("tuple_expression", K::TupleExpression),
                // leaves and subtrees that never contain references
                ("identifier", K::Skip),
                ("integer_literal", K::Skip),
                ("real_literal", K::Skip),
                ("hex_literal", K::Skip),
                ("oct_literal", K::Skip),
                ("bin_literal", K::Skip),
                ("boolean_literal", K::Skip),
                ("nil_literal", K::Skip),
                ("line_str_text", K::Skip),
                ("multi_line_str_text", K::Skip),
                ("str_escaped_char", K::Skip),
                ("raw_str_part", K::Skip),
                ("raw_str_end_part", K::Skip),
                ("raw_str_continuing_indicator", K::Skip),
                ("regex_literal", K::Skip),
                ("special_literal", K::Skip),
                ("statement_label", K::Skip),
                ("wildcard_pattern", K::Skip),
                ("custom_operator", K::Skip),
                ("directive", K::Skip),
                ("diagnostic", K::Skip),
                ("availability_condition", K::Skip),
                ("operator_declaration", K::Skip),
                ("precedence_group_declaration", K::Skip),
                ("throws", K::Skip),
                ("throws_clause", K::Other),
                ("visibility_modifier", K::Skip),
                ("member_modifier", K::Skip),
                ("property_modifier", K::Skip),
                ("function_modifier", K::Skip),
                ("mutation_modifier", K::Skip),
                ("inheritance_modifier", K::Skip),
                ("ownership_modifier", K::Skip),
                ("parameter_modifier", K::Skip),
                ("parameter_modifiers", K::Skip),
                ("property_behavior_modifier", K::Skip),
                ("value_argument_label", K::Skip),
                ("shebang_line", K::Skip),
                ("where_keyword", K::Skip),
                ("else", K::Skip),
                ("catch_keyword", K::Skip),
                ("default_keyword", K::Skip),
                ("try_operator", K::Skip),
                ("as_operator", K::Skip),
                ("bang", K::Skip),
                ("getter_specifier", K::Skip),
                ("setter_specifier", K::Skip),
                ("modify_specifier", K::Skip),
            ],
        );
        Tables {
            kinds,
            f_name: field(&lang, "name"),
            f_body: field(&lang, "body"),
            f_kind: field(&lang, "declaration_kind"),
            f_return_type: field(&lang, "return_type"),
            f_default_value: field(&lang, "default_value"),
            f_external_name: field(&lang, "external_name"),
            f_type: field(&lang, "type"),
            f_value: field(&lang, "value"),
            f_target: field(&lang, "target"),
            f_suffix: field(&lang, "suffix"),
            f_result: field(&lang, "result"),
            f_bound_identifier: field(&lang, "bound_identifier"),
            f_item: field(&lang, "item"),
            f_collection: field(&lang, "collection"),
            f_expr: field(&lang, "expr"),
            f_error: field(&lang, "error"),
            f_inherits_from: field(&lang, "inherits_from"),
            f_constructed_type: field(&lang, "constructed_type"),
            f_wrapped: field(&lang, "wrapped"),
            f_element: field(&lang, "element"),
            f_data_contents: field(&lang, "data_contents"),
            f_raw_value: field(&lang, "raw_value"),
            f_if_true: field(&lang, "if_true"),
            f_if_false: field(&lang, "if_false"),
            f_lhs: field(&lang, "lhs"),
            f_rhs: field(&lang, "rhs"),
            f_reference_specifier: field(&lang, "reference_specifier"),
        }
    })
}

// What the top-level declarations of this file say about a name (collected before the walk).
/// A type (class, struct, enum, actor, protocol, alias).
const F_TYPE: u8 = 1;
/// A function.
const F_FUNC: u8 = 2;
/// A global constant or variable.
const F_VALUE: u8 = 4;
/// A protocol.
const F_PROTO: u8 = 8;
/// A class or actor.
const F_CLASS: u8 = 16;
/// A test class: an `XCTestCase` subclass, directly or through a test class of this file.
const F_TEST: u8 = 32;
/// Declared as a stored or computed property or a global variable somewhere in this file.
const F_PROP: u8 = 64;

/// Most bindings one declaration is looked at for (`var a, b, c: Int`, `let (x, y) = ..`).
const MAX_BINDINGS: usize = 8;
/// Longest receiver type path (in bytes) that is followed.
const MAX_PATH_LEN: usize = 512;
/// Most segments a receiver type path gets.
const MAX_SEGMENTS: usize = 6;

/// A type body the walker is inside of.
#[derive(Clone, Copy)]
struct Frame {
    /// Simple name of the type (the extended type for an extension): the qualifier of implicit
    /// `self` members.
    name: u32,
    /// The definition of the type or extension.
    def: u32,
    kind: DefKind,
    test: bool,
    exported: bool,
}

/// What an expression denotes, as far as it can be a receiver.
#[derive(Clone, Copy)]
enum R {
    /// A value whose type path is the id (`NONE` when unknown).
    Val(u32),
    /// A type used as a value: its path (`T`, `Outer::Inner`, `Module|T`), its simple name, and
    /// what a `Type` or `New` reference of the simple name is qualified by (the module or the
    /// outer type, `NONE` when written alone).
    Ty(u32, u32, u32),
    /// An imported module.
    Module(u32),
    /// `super`.
    Super,
}

const UNKNOWN: R = R::Val(NONE);

/// One binding of a property declaration: (pattern, type annotation, value, accessors).
type Binding<'t> = (Option<Node<'t>>, Option<Node<'t>>, Option<Node<'t>>, Option<Node<'t>>);

/// Flags read off the modifiers of a declaration.
#[derive(Clone, Copy, Default)]
struct Mods {
    flags: u8,
    test: bool,
    /// Where the signature starts once the leading attributes are skipped.
    sig_start: usize,
}

struct W<'a, 't> {
    cx: Ctx<'a>,
    t: &'static Tables,
    /// Enclosing type bodies, innermost last.
    frames: Vec<Frame>,
    /// Nesting of function, accessor and closure bodies within the innermost type body: inside
    /// one, declarations are locals.
    fn_depth: u32,
    /// `F_*` flags by name id.
    top: Vec<u8>,
    /// (type name, member name) of the members the types and extensions of this file declare.
    members: FxHashSet<u64>,
    /// Imported modules by name id.
    modules: FxHashSet<u32>,
    /// Declared result types of the enclosing bodies, for `return .member`.
    rets: Vec<u32>,
    /// Contextual type of the expression about to be visited (`NONE`: none known).
    want: u32,
    /// Name of the function whose arguments are being visited, for `Arg` references.
    callee: u32,
    /// Functions declared inside the enclosing function bodies: a call of one is a plain call
    /// even inside a type.
    local_fns: Vec<u32>,
    /// Cursors for subtrees visited out of order (see `cur`).
    pool: Vec<TreeCursor<'t>>,
    /// The bare name `self`, `error`, `newValue`, `oldValue` interned once.
    id_error: u32,
    id_new_value: u32,
    id_old_value: u32,
    id_init: u32,
    has_error: bool,
    _t: PhantomData<&'t ()>,
}

pub fn extract(_lang: Lang, src: &[u8], tree: &Tree, path: &str, facts: &mut FileFacts, scratch: &mut Scratch) {
    let reparsed = if tree.root_node().has_error() { reparse_without_directives(src, tree) } else { None };
    let tree = reparsed.as_ref().unwrap_or(tree);
    let cx = Ctx::new(src, path, facts, scratch);
    let root = tree.root_node();
    let mut w = W {
        cx,
        t: tables(),
        frames: Vec::new(),
        fn_depth: 0,
        top: Vec::new(),
        members: FxHashSet::default(),
        modules: FxHashSet::default(),
        rets: Vec::new(),
        want: NONE,
        callee: NONE,
        local_fns: Vec::new(),
        pool: Vec::new(),
        id_error: NONE,
        id_new_value: NONE,
        id_old_value: NONE,
        id_init: NONE,
        has_error: root.has_error(),
        _t: PhantomData,
    };
    w.id_error = w.cx.intern(b"error");
    w.id_new_value = w.cx.intern(b"newValue");
    w.id_old_value = w.cx.intern(b"oldValue");
    w.id_init = w.cx.intern(b"init");
    w.prescan(root);
    let mut c = tree.walk();
    // The file is one scope for the bindings of top-level code (`main.swift`, scripts): they
    // are globals, but a function may not use one before it is declared, which syntax cannot
    // tell, so they are definitions and not locals.
    w.children(&mut c);
}

// ---------------------------------------------------------------------------------------------
// Conditional compilation
// ---------------------------------------------------------------------------------------------
//
// The grammar parses `#if` only where a statement or a declaration may stand. Between the cases
// of a `switch`, around an attribute or in the middle of a declaration it fails, and the error
// often takes the enclosing type with it: its methods would surface as free functions. Such a
// file is parsed again with the directive lines blanked (both branches kept), and if that still
// fails with only the first branch of every `#if` kept. Blanking keeps every byte offset and
// line; node text is always read from the original source. The tree with the fewest errors wins.

thread_local! {
    static REPARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

fn reparse_without_directives(src: &[u8], old: &Tree) -> Option<Tree> {
    memchr::memmem::find(src, b"#if")?;
    let mut best: Option<Tree> = None;
    let mut best_errors = error_count(old.root_node());
    for first_only in [false, true] {
        let patched = blank_directives(src, first_only);
        let tree = REPARSER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_none() {
                let mut p = Parser::new();
                p.set_language(&tree_sitter_swift::LANGUAGE.into()).ok()?;
                *slot = Some(p);
            }
            slot.as_mut()?.parse(&patched, None)
        })?;
        let errors = error_count(tree.root_node());
        if errors < best_errors {
            best_errors = errors;
            best = Some(tree);
        }
        if best_errors == 0 {
            break;
        }
    }
    best
}

/// The source with the lines of `#if`, `#elseif`, `#else` and `#endif` replaced by spaces, and
/// with `first_only` also every branch after the first.
fn blank_directives(src: &[u8], first_only: bool) -> Vec<u8> {
    let mut out = src.to_vec();
    // Per open `#if`: whether the branch being read is skipped.
    let mut skip: Vec<bool> = Vec::new();
    let mut start = 0;
    while start < src.len() {
        let end = memchr::memchr(b'\n', &src[start..]).map_or(src.len(), |i| start + i);
        let line = &src[start..end];
        let t = line.trim_ascii_start();
        let word = |w: &[u8]| t.starts_with(w) && !t.get(w.len()).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
        let directive = if word(b"#if") {
            skip.push(false);
            true
        } else if word(b"#elseif") || word(b"#else") {
            if let Some(s) = skip.last_mut() {
                *s = first_only;
            }
            true
        } else if word(b"#endif") {
            skip.pop();
            true
        } else {
            false
        };
        if directive || skip.iter().any(|&s| s) {
            for b in &mut out[start..end] {
                if *b != b'\r' {
                    *b = b' ';
                }
            }
        }
        start = end + 1;
    }
    out
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

#[inline]
fn key(a: u32, b: u32) -> u64 {
    ((a as u64) << 32) | b as u64
}

#[inline]
fn is_upper(s: &[u8]) -> bool {
    s.first().is_some_and(|b| b.is_ascii_uppercase()) || (s.first() == Some(&b'_') && s.get(1).is_some_and(|b| b.is_ascii_uppercase()))
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

    /// A cursor on `n`, from the pool of cursors the walk has finished with: creating one
    /// allocates, resetting one does not.
    #[inline]
    fn cur(&mut self, n: Node<'t>) -> TreeCursor<'t> {
        match self.pool.pop() {
            Some(mut c) => {
                c.reset(n);
                c
            }
            None => n.walk(),
        }
    }

    #[inline]
    fn put(&mut self, c: TreeCursor<'t>) {
        self.pool.push(c);
    }

    fn ex_node(&mut self, n: Node<'t>) -> R {
        let mut c = self.cur(n);
        let r = self.ex(&mut c);
        self.put(c);
        r
    }

    fn recv_node(&mut self, n: Node<'t>) -> R {
        let mut c = self.cur(n);
        let r = self.recv(&mut c);
        self.put(c);
        r
    }

    fn visit_node(&mut self, n: Node<'t>) {
        let mut c = self.cur(n);
        self.visit(&mut c);
        self.put(c);
    }

    fn call_suffix_node(&mut self, n: Node<'t>) {
        let mut c = self.cur(n);
        self.call_suffix(&mut c);
        self.put(c);
    }

    fn children(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            if c.node().is_named() {
                w.visit(c)
            }
        });
    }

    fn scoped(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.push_scope();
        self.children(c);
        self.cx.pop_scope();
    }

    /// The text of a node, without the backticks of an escaped identifier (`` `default` ``).
    #[inline]
    fn txt(&self, n: Node<'t>) -> &'a [u8] {
        let t = self.cx.text(n);
        if t.len() > 2 && t[0] == b'`' && t[t.len() - 1] == b'`' { &t[1..t.len() - 1] } else { t }
    }

    /// The name a node spells, interned.
    #[inline]
    fn nm(&mut self, n: Node<'t>) -> u32 {
        let t = self.txt(n);
        self.cx.intern(t)
    }

    #[inline]
    fn top_flags(&self, id: u32) -> u8 {
        self.top.get(id as usize).copied().unwrap_or(0)
    }

    fn flag_top(&mut self, id: u32, f: u8) {
        let i = id as usize;
        if i >= self.top.len() {
            self.top.resize(i + 1, 0);
        }
        self.top[i] |= f;
    }

    /// Simple name of the innermost enclosing type, `NONE` outside types.
    #[inline]
    fn self_ty(&self) -> u32 {
        self.frames.last().map_or(NONE, |f| f.name)
    }

    /// The enclosing type that declares member `name` in this file, innermost first.
    fn member_owner(&self, name: u32) -> Option<u32> {
        self.frames.iter().rev().find(|f| self.members.contains(&key(f.name, name))).map(|f| f.name)
    }

    /// A builtin type name that no top-level declaration of this file redefines.
    fn builtin_type(&self, text: &[u8], id: u32) -> bool {
        is_builtin_type(text) && self.top_flags(id) & F_TYPE == 0
    }

    // ----- interning of composed names -----

    /// `a SEP b SUFFIX` interned: `T::field`, `T::method()`, `Module|T`. `NONE` when a part is
    /// unknown or the path grows too long.
    fn path3(&mut self, a: u32, sep: &[u8], b: u32, suffix: &[u8]) -> u32 {
        if a == NONE || b == NONE {
            return NONE;
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.extend_from_slice(self.cx.name_bytes(a));
        let segs = memchr::memmem::find_iter(&buf, b"::").count();
        let id = if buf.len() > MAX_PATH_LEN || segs >= MAX_SEGMENTS {
            NONE
        } else {
            buf.extend_from_slice(sep);
            buf.extend_from_slice(self.cx.name_bytes(b));
            buf.extend_from_slice(suffix);
            self.cx.intern(&buf)
        };
        self.cx.s.buf = buf;
        id
    }

    /// `base::name` (a member's type), or `base::name()` (a method's result).
    #[inline]
    fn seg(&mut self, base: u32, name: u32, call: bool) -> u32 {
        self.path3(base, b"::", name, if call { b"()" } else { b"" })
    }

    /// `prefix name suffix` interned: `^C`, `$x`, `f()`.
    fn wrap(&mut self, prefix: &[u8], name: u32, suffix: &[u8]) -> u32 {
        if name == NONE {
            return NONE;
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.extend_from_slice(prefix);
        buf.extend_from_slice(self.cx.name_bytes(name));
        buf.extend_from_slice(suffix);
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// Slot `owner.field`, or `.field` when the owner is unknown.
    fn slot(&mut self, owner: u32, field: u32) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        if owner != NONE {
            buf.extend_from_slice(self.cx.name_bytes(owner));
        }
        buf.push(b'.');
        buf.extend_from_slice(self.cx.name_bytes(field));
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    // ----- pre-scan: imports, top-level names and the members of the types of this file -----

    fn prescan(&mut self, root: Node<'t>) {
        let mut c = root.walk();
        let mut supers: Vec<(u32, u32)> = Vec::new();
        self.prescan_decls(&mut c, NONE, 0, &mut supers);
        // A class is a test class when it derives from `XCTestCase`, from a class named like a
        // test base (`BaseTestCase`), or from a test class of this file.
        for _ in 0..3 {
            for &(cls, sup) in &supers {
                let st = self.cx.name_bytes(sup);
                if st == b"XCTestCase" || st.ends_with(b"TestCase") || self.top_flags(sup) & F_TEST != 0 {
                    self.flag_top(cls, F_TEST);
                }
            }
        }
    }

    /// Declarations directly in a file (`owner == NONE`) or a type body (`owner` = its name).
    fn prescan_decls(&mut self, c: &mut TreeCursor<'t>, owner: u32, depth: u32, supers: &mut Vec<(u32, u32)>) {
        if depth > 16 {
            return;
        }
        let t = self.t;
        each_child(c, |c| {
            let n = c.node();
            let k = self.k(n);
            match k {
                K::ImportDeclaration if owner == NONE => self.import(n),
                K::ClassDeclaration | K::ProtocolDeclaration => {
                    let Some(nm) = child(n, t.f_name) else { return };
                    let name = self.last_type_ident(nm);
                    if name == NONE {
                        return;
                    }
                    let kind_text = child(n, t.f_kind).map(|x| self.txt(x)).unwrap_or(b"protocol");
                    if owner != NONE {
                        self.members.insert(key(owner, name));
                    } else if kind_text != b"extension" {
                        self.flag_top(name, F_TYPE);
                    }
                    match kind_text {
                        b"class" | b"actor" => {
                            self.flag_top(name, F_CLASS);
                            if let Some(sup) = self.first_super(n) {
                                supers.push((name, sup));
                            }
                        }
                        b"protocol" => self.flag_top(name, F_PROTO),
                        _ => {}
                    }
                    if let Some(body) = child(n, t.f_body) {
                        let mut bc = self.cur(body);
                        self.prescan_decls(&mut bc, name, depth + 1, supers);
                        self.put(bc);
                    }
                }
                K::FunctionDeclaration | K::ProtocolFunctionDeclaration => {
                    let Some(nm) = child(n, t.f_name) else { return };
                    let id = self.nm(nm);
                    if owner != NONE {
                        self.members.insert(key(owner, id));
                    } else {
                        self.flag_top(id, F_FUNC);
                    }
                }
                K::InitDeclaration if owner != NONE => {
                    self.members.insert(key(owner, self.id_init));
                }
                K::PropertyDeclaration | K::ProtocolPropertyDeclaration => {
                    let mut names = [None; MAX_BINDINGS];
                    let cnt = self.binding_names(n, &mut names);
                    for nm in names.iter().take(cnt).flatten() {
                        let id = self.nm(*nm);
                        self.flag_top(id, F_PROP);
                        if owner != NONE {
                            self.members.insert(key(owner, id));
                        } else {
                            self.flag_top(id, F_VALUE);
                        }
                    }
                }
                K::EnumEntry if owner != NONE => {
                    let mut ec = self.cur(n);
                    each_child(&mut ec, |ec| {
                        if is_field(ec, t.f_name) {
                            let id = self.nm(ec.node());
                            self.members.insert(key(owner, id));
                        }
                    });
                    self.put(ec);
                }
                K::TypealiasDeclaration | K::AssociatedtypeDeclaration => {
                    if let Some(nm) = child(n, t.f_name) {
                        let id = self.nm(nm);
                        if owner != NONE {
                            self.members.insert(key(owner, id));
                        } else {
                            self.flag_top(id, F_TYPE);
                        }
                    }
                }
                _ => {}
            }
        });
    }

    /// Name of the first entry of the inheritance clause of a type declaration.
    fn first_super(&mut self, n: Node<'t>) -> Option<u32> {
        let t = self.t;
        let mut c = self.cur(n);
        let mut out = None;
        each_child(&mut c, |c| {
            let ch = c.node();
            if out.is_none()
                && self.k(ch) == K::InheritanceSpecifier
                && let Some(ut) = child(ch, t.f_inherits_from).filter(|u| self.k(*u) == K::UserType)
            {
                let id = self.last_type_ident(ut);
                if id != NONE {
                    out = Some(id);
                }
            }
        });
        self.put(c);
        out
    }

    /// Simple name of a type written as `type_identifier` or `user_type` (`A.B` -> `B`).
    fn last_type_ident(&mut self, n: Node<'t>) -> u32 {
        match self.k(n) {
            K::TypeIdentifier | K::SimpleIdentifier => {
                if n.end_byte() > n.start_byte() {
                    self.nm(n)
                } else {
                    NONE
                }
            }
            K::UserType => {
                let mut last = None;
                let mut c = self.cur(n);
                each_child(&mut c, |c| {
                    if self.k(c.node()) == K::TypeIdentifier {
                        last = Some(c.node());
                    }
                });
                self.put(c);
                last.map_or(NONE, |x| self.nm(x))
            }
            _ => NONE,
        }
    }

    /// The names a property declaration binds, in order.
    fn binding_names(&mut self, n: Node<'t>, out: &mut [Option<Node<'t>>; MAX_BINDINGS]) -> usize {
        let t = self.t;
        let mut cnt = 0;
        let mut c = self.cur(n);
        each_child(&mut c, |c| {
            if is_field(c, t.f_name) && self.k(c.node()) == K::Pattern {
                self.pattern_names(c.node(), out, &mut cnt, 0);
            }
        });
        self.put(c);
        cnt
    }

    fn pattern_names(&mut self, p: Node<'t>, out: &mut [Option<Node<'t>>; MAX_BINDINGS], cnt: &mut usize, depth: u32) {
        if depth > 6 {
            return;
        }
        let mut c = self.cur(p);
        each_child(&mut c, |c| {
            let ch = c.node();
            match self.k(ch) {
                K::SimpleIdentifier if *cnt < MAX_BINDINGS && ch.end_byte() > ch.start_byte() => {
                    out[*cnt] = Some(ch);
                    *cnt += 1;
                }
                K::Pattern => self.pattern_names(ch, out, cnt, depth + 1),
                _ => {}
            }
        });
        self.put(c);
    }

    // ----- imports -----

    /// `import M`, `import struct M.T`, `@testable import M`, `@_exported import M`.
    fn import(&mut self, n: Node<'t>) {
        let mut ident = None;
        let mut named = false;
        let mut reexport = false;
        let mut c = self.cur(n);
        each_child(&mut c, |c| {
            let ch = c.node();
            match self.k(ch) {
                K::Skip if ch.is_named() => ident = Some(ch),
                K::Modifiers => reexport |= memchr::memmem::find(self.txt(ch), b"_exported").is_some(),
                _ if !ch.is_named() => {
                    named |= matches!(self.txt(ch), b"struct" | b"class" | b"enum" | b"protocol" | b"typealias" | b"func" | b"var" | b"let" | b"actor")
                }
                _ => {}
            }
        });
        self.put(c);
        let Some(id) = ident else { return };
        let text = self.txt(id);
        if text.is_empty() {
            return;
        }
        let first = &text[..memchr::memchr(b'.', text).unwrap_or(text.len())];
        let fid = self.cx.intern(first);
        self.modules.insert(fid);
        let mut flags = if is_system_module(first) { iflag::SYSTEM } else { 0 };
        if reexport {
            flags |= iflag::REEXPORT;
        }
        let line = line_of(n);
        match memchr::memrchr(b'.', text).filter(|_| named) {
            Some(i) => {
                let target = self.cx.intern(&text[..i]);
                let name = self.cx.intern(&text[i + 1..]);
                self.cx.add_import(target, name, NONE, line, flags);
                self.cx.add_ref(name, RefKind::Import, line, target, 255);
            }
            None => {
                let target = self.cx.intern(text);
                // A module import makes every public name of the module visible.
                self.cx.add_import(target, NONE, NONE, line, flags | iflag::WILDCARD);
            }
        }
    }

    // ----- traversal -----

    fn visit(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        match self.k(n) {
            K::Skip | K::Comment | K::ImportDeclaration | K::ValueBindingPattern => {}
            K::Error => {
                self.cx.facts.parse_errors += 1;
                // Identifiers the parser could not place: whether they declare or use a name is
                // unknown, so only the structure below them is looked at.
                self.each(c, |w, c| {
                    if c.node().is_named() && w.k(c.node()) != K::SimpleIdentifier {
                        w.visit(c)
                    }
                });
            }
            K::TypeIdentifier => {
                let id = self.nm(n);
                self.type_name_ref(n, id, RefKind::Type, NONE);
            }
            K::UserType => self.type_ref(c, RefKind::Type),
            K::ClassDeclaration => self.type_decl(n, c, false),
            K::ProtocolDeclaration => self.type_decl(n, c, true),
            K::FunctionDeclaration | K::ProtocolFunctionDeclaration | K::InitDeclaration | K::DeinitDeclaration | K::SubscriptDeclaration => {
                self.function(n, c)
            }
            K::PropertyDeclaration | K::ProtocolPropertyDeclaration => self.property(n, c),
            K::EnumEntry => self.enum_entry(n, c),
            K::TypealiasDeclaration | K::AssociatedtypeDeclaration | K::MacroDeclaration => self.alias_decl(n, c),
            K::Modifiers => self.children(c),
            K::Attribute => self.attribute(c),
            K::TypeParameters => self.type_params(c),
            K::FunctionBody => self.children(c),
            K::IfStatement | K::GuardStatement | K::WhileStatement => self.conditional(n, c),
            K::ForStatement => self.for_statement(c),
            K::SwitchStatement => self.switch_statement(c),
            K::CatchBlock => self.catch_block(c),
            K::DoStatement | K::SwitchEntry => self.scoped(c),
            K::LambdaLiteral => self.lambda(c),
            K::ComputedProperty | K::Observers => self.children(c),
            K::Accessor | K::Observer => self.accessor(c, NONE),
            K::Pattern => self.pattern(c, false, NONE),
            K::Other => {
                if self.has_error && n.is_missing() {
                    self.cx.facts.parse_errors += 1;
                }
                self.children(c)
            }
            _ => {
                self.ex(c);
            }
        }
    }

    // ----- types -----

    /// A reference of `kind` to type name `id` written at `n`, unless it is a builtin, a type
    /// parameter or `Self`.
    fn type_name_ref(&mut self, n: Node<'t>, id: u32, kind: RefKind, qual: u32) {
        if id == NONE {
            return;
        }
        let text = self.txt(n);
        if text.is_empty() || text == b"Self" || text == b"_" {
            return;
        }
        if qual == NONE && (self.cx.is_local(id) || self.builtin_type(text, id)) {
            return;
        }
        self.cx.add_ref(id, kind, line_of(n), qual, 255);
    }

    /// `T`, `Outer.Inner`, `Module.T`, `T<A, B>`: the last component is a reference of `kind`,
    /// the components before it `Type` references, each qualified by the one before it (the
    /// first one after a module by the module). Type arguments are visited.
    fn type_ref(&mut self, c: &mut TreeCursor<'t>, kind: RefKind) {
        let mut comps: [Option<Node<'t>>; 8] = [None; 8];
        let mut cnt = 0;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::TypeIdentifier => {
                    if cnt < comps.len() {
                        comps[cnt] = Some(ch);
                        cnt += 1;
                    }
                }
                _ if ch.is_named() => w.visit(c),
                _ => {}
            }
        });
        let ids: [u32; 8] = std::array::from_fn(|i| comps[i].map_or(NONE, |x| self.nm(x)));
        let start = if cnt > 1 && self.modules.contains(&ids[0]) && self.top_flags(ids[0]) & F_TYPE == 0 { 1 } else { 0 };
        for i in start..cnt {
            let Some(node) = comps[i] else { continue };
            let qual = if i == 0 { NONE } else { ids[i - 1] };
            self.type_name_ref(node, ids[i], if i + 1 == cnt { kind } else { RefKind::Type }, qual);
        }
    }

    /// The name a declared type gives a value: generic arguments, optionals and implicitly
    /// unwrapped optionals, `some` / `any` stripped, the element of an array, the value of a
    /// dictionary. `A.B` is the path `A::B`, `M.T` with an imported module `M` is `M|T`, `Self`
    /// the enclosing type. `NONE` for function types, tuples, metatypes and type parameters.
    fn type_name_of(&mut self, mut ty: Node<'t>) -> u32 {
        let t = self.t;
        for _ in 0..8 {
            let next = match self.k(ty) {
                K::UserType => return self.user_type_name(ty),
                K::TypeIdentifier => {
                    let id = self.nm(ty);
                    return if self.txt(ty) == b"Self" {
                        self.self_ty()
                    } else if self.cx.is_local(id) {
                        NONE
                    } else {
                        id
                    };
                }
                K::OptionalType => child(ty, t.f_wrapped),
                K::ArrayType | K::DictionaryType => {
                    let dict = self.k(ty) == K::DictionaryType;
                    let e = if dict { child(ty, t.f_value) } else { child(ty, t.f_element).or_else(|| child(ty, t.f_name)) };
                    let et = e.map_or(NONE, |e| self.type_name_of(e));
                    return self.collection(dict, et);
                }
                K::WrapperType | K::TypeAnnotation => ty.named_child(0).filter(|x| self.k(*x) != K::Skip).or_else(|| child(ty, t.f_type)),
                K::TupleType => {
                    // `(T)` is `T`; a real tuple has no members.
                    if ty.named_child_count() == 1 { ty.named_child(0) } else { None }
                }
                K::TupleTypeItem => child(ty, t.f_type).or_else(|| child(ty, t.f_name)),
                _ => None,
            };
            match next {
                Some(x) => ty = x,
                None => return NONE,
            }
        }
        NONE
    }

    fn user_type_name(&mut self, ut: Node<'t>) -> u32 {
        let mut comps: [u32; 8] = [NONE; 8];
        let mut cnt = 0;
        let mut c = self.cur(ut);
        each_child(&mut c, |c| {
            let ch = c.node();
            if self.k(ch) == K::TypeIdentifier && cnt < comps.len() && ch.end_byte() > ch.start_byte() {
                comps[cnt] = self.nm(ch);
                cnt += 1;
            }
        });
        self.put(c);
        if cnt == 0 {
            return NONE;
        }
        if cnt == 1 {
            let id = comps[0];
            if self.cx.name_bytes(id) == b"Self" {
                return self.self_ty();
            }
            if self.cx.is_local(id) || matches!(self.cx.name_bytes(id), b"Any" | b"AnyObject" | b"Void" | b"Never") {
                return NONE;
            }
            return id;
        }
        if self.cx.is_local(comps[0]) {
            // `T.Element` of a type parameter.
            return NONE;
        }
        let module = self.modules.contains(&comps[0]) && self.top_flags(comps[0]) & F_TYPE == 0;
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        for (i, &id) in comps[..cnt].iter().enumerate() {
            if i > 0 {
                buf.extend_from_slice(if i == 1 && module { b"|" } else { b"::" });
            }
            buf.extend_from_slice(self.cx.name_bytes(id));
        }
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// Declare the generic parameters of a declaration, then visit their constraints.
    fn type_params(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            if w.k(c.node()) != K::TypeParameter {
                if c.node().is_named() {
                    w.visit(c);
                }
                return;
            }
            w.each(c, |w, c| {
                let ch = c.node();
                match w.k(ch) {
                    K::TypeIdentifier => {
                        let id = w.nm(ch);
                        w.cx.declare(id, NONE);
                    }
                    _ if ch.is_named() => w.visit(c),
                    _ => {}
                }
            });
        });
    }

    // ----- modifiers and attributes -----

    /// Visibility, test attribute and signature start of a declaration, without emitting.
    fn mods(&mut self, n: Node<'t>) -> Mods {
        let mut c = self.cur(n);
        let mut mc = self.cur(n);
        let m = self.mods_with(n, &mut c, &mut mc);
        self.put(mc);
        self.put(c);
        m
    }

    fn mods_with(&self, n: Node<'t>, c: &mut TreeCursor<'t>, mc: &mut TreeCursor<'t>) -> Mods {
        let mut m = Mods { flags: 0, test: false, sig_start: n.start_byte() };
        let mut found_start = false;
        if !c.goto_first_child() {
            return m;
        }
        loop {
            let ch = c.node();
            match self.k(ch) {
                K::Modifiers => {
                    mc.reset(ch);
                    if mc.goto_first_child() {
                        loop {
                            let x = mc.node();
                            match self.k(x) {
                                K::Attribute => {
                                    let text = self.txt(x);
                                    if text == b"@Test" || text.starts_with(b"@Test(") {
                                        m.test = true;
                                    }
                                }
                                _ => {
                                    if !found_start && x.is_named() {
                                        m.sig_start = x.start_byte();
                                        found_start = true;
                                    }
                                    let text = self.txt(x);
                                    if text.starts_with(b"private") || text.starts_with(b"fileprivate") {
                                        if !text.contains(&b'(') {
                                            m.flags |= dflag::LOCAL;
                                        }
                                    } else if text == b"public" || text == b"open" {
                                        m.flags |= dflag::EXPORTED;
                                    }
                                }
                            }
                            if !mc.goto_next_sibling() {
                                break;
                            }
                        }
                    }
                }
                K::Attribute | K::Comment => {}
                _ => {
                    if !found_start {
                        m.sig_start = ch.start_byte();
                    }
                    return m;
                }
            }
            if !c.goto_next_sibling() {
                return m;
            }
        }
    }

    /// `@Wrapper`, `@MainActor`, `@objc`: an annotation for every attribute that is not part of
    /// the language (those are written in lower case).
    fn attribute(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::UserType => {
                    let id = w.last_type_ident(ch);
                    if id != NONE && is_upper(w.cx.name_bytes(id)) {
                        let text = w.cx.name_bytes(id);
                        if !is_builtin_attr(text) {
                            w.type_ref(c, RefKind::Annotation);
                        }
                    }
                }
                // Arguments of an attribute: `@available(iOS 13, *)` names no symbols, but a
                // property wrapper may be initialised with expressions.
                K::SimpleIdentifier | K::Skip => {}
                _ if ch.is_named() => w.visit(c),
                _ => {}
            }
        });
    }

    // ----- definitions: types -----

    fn type_decl(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, protocol: bool) {
        let t = self.t;
        let Some(nm) = child(n, t.f_name) else { return self.children(c) };
        let name = self.last_type_ident(nm);
        if name == NONE {
            return self.children(c);
        }
        let kind_text: &[u8] = if protocol { b"protocol" } else { child(n, t.f_kind).map(|x| self.txt(x)).unwrap_or(b"class") };
        let kind = match kind_text {
            b"struct" => DefKind::Struct,
            b"enum" => DefKind::Enum,
            b"extension" => DefKind::Impl,
            b"protocol" => DefKind::Interface,
            _ => DefKind::Class,
        };
        let body = child(n, t.f_body);
        let sig_end = body.map_or(n.end_byte(), |b| b.start_byte());
        let m = self.mods(n);
        let d = self.cx.def(name, kind, n, sig_end);
        let local = if self.fn_depth > 0 { dflag::LOCAL } else { 0 };
        let parent = self.cx.encl();
        self.set_sig(d, m.sig_start, sig_end);
        {
            let dm = self.cx.def_mut(d);
            dm.parent = parent;
            dm.flags |= m.flags | local;
        }
        let test = self.top_flags(name) & F_TEST != 0;
        let frame = Frame { name, def: d, kind, test, exported: m.flags & dflag::EXPORTED != 0 };
        self.cx.push_def(d);
        self.cx.push_scope();
        let mut heritage = 0u32;
        self.each(c, |w, c| {
            let ch = c.node();
            if ch == nm {
                if kind == DefKind::Impl && w.k(ch) == K::UserType {
                    // The extended type is used here.
                    w.type_ref(c, RefKind::Type);
                }
                return;
            }
            match w.k(ch) {
                K::InheritanceSpecifier => {
                    w.heritage(ch, c, kind, heritage);
                    heritage += 1;
                }
                K::ClassBody => {
                    let saved = std::mem::replace(&mut w.fn_depth, 0);
                    w.frames.push(frame);
                    w.children(c);
                    w.frames.pop();
                    w.fn_depth = saved;
                }
                _ if ch.is_named() => w.visit(c),
                _ => {}
            }
        });
        self.cx.pop_scope();
        self.cx.pop_def();
    }

    /// One entry of an inheritance clause: `Extends` or `Implements` (see the module comment).
    fn heritage(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: DefKind, index: u32) {
        let t = self.t;
        let Some(ut) = child(n, t.f_inherits_from) else { return self.children(c) };
        let rk = match kind {
            DefKind::Interface => RefKind::Extends,
            DefKind::Class if index == 0 => {
                let id = self.last_type_ident(ut);
                let f = self.top_flags(id);
                if f & F_PROTO != 0 || (f & F_CLASS == 0 && id != NONE && looks_like_protocol(self.cx.name_bytes(id))) {
                    RefKind::Implements
                } else {
                    RefKind::Extends
                }
            }
            _ => RefKind::Implements,
        };
        self.each(c, |w, c| {
            let ch = c.node();
            if ch == ut && w.k(ch) == K::UserType {
                w.type_ref(c, rk);
            } else if ch.is_named() {
                w.visit(c);
            }
        });
    }

    fn set_sig(&mut self, d: u32, start: usize, end: usize) {
        let dm = self.cx.def_mut(d);
        if start >= dm.sig_off as usize && start < end {
            dm.sig_off = start as u32;
        }
        let s = dm.sig_off as usize;
        dm.sig_len = end.saturating_sub(s).min(u16::MAX as usize) as u16;
    }

    /// `case a, b(Int, name: String) = raw`: one `EnumMember` per name.
    fn enum_entry(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let Some(fr) = self.frames.last().copied() else { return self.children(c) };
        let m = self.mods(n);
        let mut last = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            if is_field(c, t.f_name) {
                if ch.end_byte() <= ch.start_byte() {
                    return;
                }
                let id = w.nm(ch);
                let d = w.cx.def(id, DefKind::EnumMember, n, n.end_byte());
                w.set_sig(d, m.sig_start, n.end_byte());
                let dm = w.cx.def_mut(d);
                dm.parent = fr.def;
                dm.flags |= m.flags | if fr.exported { dflag::EXPORTED } else { 0 };
                last = d;
            } else if is_field(c, t.f_data_contents) {
                let mut arity = 0u8;
                w.cx.push_def(last);
                w.each(c, |w, c| {
                    if is_field(c, w.t.f_name) {
                        arity = arity.saturating_add(1);
                    }
                    if c.node().is_named() && w.k(c.node()) != K::SimpleIdentifier {
                        w.visit(c);
                    }
                });
                w.cx.pop_def();
                if last != NONE {
                    let dm = w.cx.def_mut(last);
                    dm.arity_min = arity;
                    dm.arity_max = arity;
                }
            } else if is_field(c, t.f_raw_value) {
                w.cx.push_def(last);
                w.visit(c);
                w.cx.pop_def();
            } else if ch.is_named() {
                w.visit(c);
            }
        });
    }

    /// `typealias A = B`, `associatedtype E: P`, `macro m(..) = ..`.
    fn alias_decl(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        // (An alias of a collection type names the collection: `Array`, `Dictionary`.)
        let t = self.t;
        let Some(nm) = child(n, t.f_name).filter(|x| x.end_byte() > x.start_byte()) else { return self.children(c) };
        let name = self.nm(nm);
        let kind = if self.k(n) == K::MacroDeclaration { DefKind::Macro } else { DefKind::TypeAlias };
        let m = self.mods(n);
        let value = child(n, t.f_value);
        let d = self.cx.def(name, kind, n, n.end_byte());
        self.set_sig(d, m.sig_start, n.end_byte());
        let local = if self.fn_depth > 0 { dflag::LOCAL } else { 0 };
        let parent = self.cx.encl();
        let decl = if self.k(n) == K::AssociatedtypeDeclaration { dflag::DECL } else { 0 };
        {
            let dm = self.cx.def_mut(d);
            dm.parent = parent;
            dm.flags |= m.flags | local | decl;
        }
        self.cx.push_def(d);
        self.cx.push_scope();
        self.each(c, |w, c| {
            let ch = c.node();
            if ch == nm {
                return;
            }
            if Some(ch) == value && kind == DefKind::TypeAlias {
                let tn = w.type_name_of(ch);
                let tn = w.recv_q(tn);
                w.cx.def_mut(d).type_name = tn;
            }
            if ch.is_named() {
                w.visit(c);
            }
        });
        self.cx.pop_scope();
        self.cx.pop_def();
    }

    // ----- definitions: functions -----

    fn function(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let nk = self.k(n);
        let name = match nk {
            K::InitDeclaration => self.id_init,
            K::DeinitDeclaration => self.cx.intern(b"deinit"),
            K::SubscriptDeclaration => self.cx.intern(b"subscript"),
            _ => match child(n, t.f_name).filter(|x| x.end_byte() > x.start_byte()) {
                Some(nm) => self.nm(nm),
                None => return self.children(c),
            },
        };
        let fr = if self.fn_depth == 0 { self.frames.last().copied() } else { None };
        let kind = if fr.is_some() { DefKind::Method } else { DefKind::Function };
        let body = child(n, t.f_body);
        let requirement = fr.is_some_and(|f| f.kind == DefKind::Interface);
        let accessors = if nk == K::SubscriptDeclaration { self.first_of(n, K::ComputedProperty) } else { None };
        let sig_end = body.or(accessors).map_or(n.end_byte(), |b| b.start_byte());
        let m = self.mods(n);
        let d = self.cx.def(name, kind, n, sig_end);
        self.set_sig(d, m.sig_start, sig_end);
        let mut flags = m.flags;
        if requirement {
            flags |= dflag::DECL | dflag::ABSTRACT;
        } else if body.is_none() && nk != K::SubscriptDeclaration {
            flags |= dflag::DECL;
        }
        if requirement && fr.is_some_and(|f| f.exported) {
            flags |= dflag::EXPORTED;
        }
        let text = self.cx.name_bytes(name);
        if m.test || (fr.is_some_and(|f| f.test) && text.starts_with(b"test") && !self.has_params(n)) {
            flags |= dflag::TEST;
        }
        let parent = self.cx.encl();
        {
            let dm = self.cx.def_mut(d);
            dm.parent = parent;
            dm.flags |= flags;
        }
        if fr.is_none() && self.fn_depth > 0 {
            self.local_fns.push(name);
        }
        let fns0 = self.local_fns.len();
        self.cx.push_def(d);
        self.cx.push_scope();
        self.fn_depth += 1;
        // The result type is read where it is written, once the generic parameters it may
        // name are declared.
        self.rets.push(NONE);
        let name_node = if nk == K::SubscriptDeclaration { None } else { child(n, t.f_name) };
        let ret_node = child(n, t.f_return_type);
        let (mut count, mut defaults, mut variadic, mut is_async) = (0u32, 0u32, false, false);
        let mut last_param_ty = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            if !ch.is_named() {
                if w.txt(ch) == b"async" {
                    is_async = true;
                }
                return;
            }
            if Some(ch) == name_node {
                return;
            }
            if Some(ch) == ret_node {
                let ret = w.type_name_of(ch);
                let ret = w.recv_q(ret);
                w.cx.def_mut(d).type_name = ret;
                if let Some(r) = w.rets.last_mut() {
                    *r = ret;
                }
                w.visit(c);
                return;
            }
            if is_field(c, t.f_default_value) {
                defaults += 1;
                w.want = w.recv_q(last_param_ty);
                w.ex(c);
                return;
            }
            match w.k(ch) {
                K::Parameter => {
                    count += 1;
                    let (ty, var) = w.parameter(ch, c);
                    last_param_ty = ty;
                    variadic |= var;
                }
                K::TypeParameters => w.type_params(c),
                K::FunctionBody | K::ComputedProperty => w.children(c),
                _ => w.visit(c),
            }
        });
        self.rets.pop();
        self.local_fns.truncate(fns0);
        self.fn_depth -= 1;
        self.cx.pop_scope();
        self.cx.pop_def();
        let n_params = count.min(254) as u8;
        let dm = self.cx.def_mut(d);
        dm.arity_min = n_params.saturating_sub(defaults.min(254) as u8);
        dm.arity_max = if variadic { 255 } else { n_params };
        if is_async {
            dm.flags |= dflag::ASYNC;
        }
    }

    fn first_of(&mut self, n: Node<'t>, k: K) -> Option<Node<'t>> {
        let mut c = self.cur(n);
        let mut out = None;
        each_child(&mut c, |c| {
            if out.is_none() && self.k(c.node()) == k {
                out = Some(c.node());
            }
        });
        self.put(c);
        out
    }

    /// A function declaration with at least one parameter.
    fn has_params(&mut self, n: Node<'t>) -> bool {
        let mut c = self.cur(n);
        let mut any = false;
        each_child(&mut c, |c| any |= self.k(c.node()) == K::Parameter);
        self.put(c);
        any
    }

    /// A parameter: its type is visited, its internal name becomes a local of that type (the
    /// external label is not a name of anything). Returns (type, variadic).
    fn parameter(&mut self, p: Node<'t>, c: &mut TreeCursor<'t>) -> (u32, bool) {
        let t = self.t;
        let nm = child(p, t.f_name).filter(|x| self.k(*x) == K::SimpleIdentifier);
        let ty_node = child(p, t.f_type);
        let ty = ty_node.map_or(NONE, |x| self.type_name_of(x));
        let mut variadic = false;
        self.each(c, |w, c| {
            let ch = c.node();
            if !ch.is_named() {
                variadic |= w.txt(ch) == b"...";
                return;
            }
            if Some(ch) == nm || is_field(c, t.f_external_name) || w.k(ch) == K::SimpleIdentifier {
                return;
            }
            w.visit(c);
        });
        if let Some(nm) = nm {
            let id = self.nm(nm);
            self.cx.declare(id, ty);
        }
        (ty, variadic)
    }

    /// `get { }`, `set(v) { }`, `willSet { }`, `didSet(old) { }`: a body with the implicit
    /// `newValue` / `oldValue` or the named parameter as locals.
    fn accessor(&mut self, c: &mut TreeCursor<'t>, ty: u32) {
        self.cx.push_scope();
        self.fn_depth += 1;
        self.cx.declare(self.id_new_value, ty);
        self.cx.declare(self.id_old_value, ty);
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::SimpleIdentifier => {
                    let id = w.nm(ch);
                    w.cx.declare(id, ty);
                }
                _ if ch.is_named() => w.visit(c),
                _ => {}
            }
        });
        self.fn_depth -= 1;
        self.cx.pop_scope();
    }

    // ----- definitions and locals: properties -----

    fn property(&mut self, n: Node<'t>, _c: &mut TreeCursor<'t>) {
        let t = self.t;
        let is_def = self.fn_depth == 0;
        let fr = self.frames.last().copied();
        let requirement = self.k(n) == K::ProtocolPropertyDeclaration;
        // The bindings of the declaration.
        let mut groups: [Binding<'t>; MAX_BINDINGS] = [(None, None, None, None); MAX_BINDINGS];
        let mut ng = 0usize;
        let mut is_let = false;
        {
            let mut pc = self.cur(n);
            each_child(&mut pc, |pc| {
                let ch = pc.node();
                match self.k(ch) {
                    K::ValueBindingPattern => is_let = self.txt(ch) == b"let",
                    K::Pattern if is_field(pc, t.f_name) => {
                        if ng < MAX_BINDINGS {
                            groups[ng].0 = Some(ch);
                            ng += 1;
                        }
                    }
                    K::TypeAnnotation if ng > 0 => groups[ng - 1].1 = Some(ch),
                    K::ComputedProperty | K::Observers if ng > 0 => groups[ng - 1].3 = Some(ch),
                    K::Other if ng == 0 && self.txt(ch) == b"let" => is_let = true,
                    _ if ng > 0 && is_field(pc, t.f_value) => groups[ng - 1].2 = Some(ch),
                    _ => {}
                }
            });
            self.put(pc);
        }
        if requirement && ng > 0 && groups[ng - 1].3.is_none() {
            groups[ng - 1].3 = self.first_of(n, K::Other);
        }
        // `var x, y: Double`: a binding without its own annotation and value has the type of
        // the next one that is annotated.
        for i in (0..ng.saturating_sub(1)).rev() {
            if groups[i].1.is_none() && groups[i].2.is_none() {
                groups[i].1 = groups[i + 1].1;
            }
        }
        let m = self.mods(n);
        // Modifiers and attributes belong to the declaration's first binding.
        let mut visited_mods = false;
        for g in groups.iter().take(ng) {
            let (Some(pat), ann, value, acc) = *g else { continue };
            let ann_ty = ann.map_or(NONE, |a| self.type_name_of(a));
            let ann_q = self.recv_q(ann_ty);
            let mut names = [None; MAX_BINDINGS];
            let mut cnt = 0;
            if let Some(b) = child(pat, t.f_bound_identifier) {
                names[0] = Some(b);
                cnt = 1;
            } else {
                self.pattern_names(pat, &mut names, &mut cnt, 0);
            }
            let mut defs = [NONE; MAX_BINDINGS];
            if is_def {
                let sig_end = value.map(|v| self.before_eq(v.start_byte())).or(acc.map(|a| a.start_byte())).unwrap_or(n.end_byte());
                for (i, nm) in names.iter().take(cnt).enumerate() {
                    let Some(nm) = nm else { continue };
                    let id = self.nm(*nm);
                    let kind = match fr {
                        Some(_) if acc.is_some_and(|a| self.k(a) == K::ComputedProperty) || requirement => DefKind::Property,
                        Some(_) => DefKind::Field,
                        None if is_let => DefKind::Constant,
                        None => DefKind::Variable,
                    };
                    let d = self.cx.def(id, kind, n, sig_end);
                    self.set_sig(d, m.sig_start, sig_end);
                    let mut flags = m.flags;
                    if requirement {
                        flags |= dflag::DECL | dflag::ABSTRACT;
                        if fr.is_some_and(|f| f.exported) {
                            flags |= dflag::EXPORTED;
                        }
                    }
                    if let Some((amin, amax)) = ann.and_then(|a| self.fn_arity(a)) {
                        flags |= dflag::FNPTR;
                        let dm = self.cx.def_mut(d);
                        dm.arity_min = amin;
                        dm.arity_max = amax;
                    }
                    let parent = self.cx.encl();
                    let dm = self.cx.def_mut(d);
                    dm.parent = parent;
                    dm.flags |= flags;
                    dm.type_name = ann_q;
                    defs[i] = d;
                }
            }
            let encl = defs[0];
            if encl != NONE {
                self.cx.push_def(encl);
            }
            if !visited_mods {
                visited_mods = true;
                let mut mc = self.cur(n);
                each_child(&mut mc, |mc| {
                    if matches!(self.k(mc.node()), K::Modifiers | K::Attribute) {
                        self.visit(mc);
                    }
                });
                self.put(mc);
            }
            if let Some(a) = ann {
                self.visit_node(a);
            }
            let mut inferred = NONE;
            if let Some(v) = value {
                self.want = ann_q;
                let r = self.ex_node(v);
                inferred = self.value_type(r);
                if is_def && encl != NONE {
                    self.bind_value(v, encl, fr.map_or(NONE, |f| f.name));
                }
            }
            let ty = if ann_ty != NONE { ann_ty } else { inferred };
            if let Some(a) = acc {
                let mut ac = self.cur(a);
                self.rets.push(ty);
                if self.k(a) == K::ComputedProperty {
                    // `{ expr }` is a getter written without `get`.
                    self.fn_depth += 1;
                    self.cx.push_scope();
                    self.each(&mut ac, |w, c| {
                        let ch = c.node();
                        match w.k(ch) {
                            K::Accessor => w.accessor(c, ty),
                            _ if ch.is_named() => w.visit(c),
                            _ => {}
                        }
                    });
                    self.cx.pop_scope();
                    self.fn_depth -= 1;
                } else {
                    self.each(&mut ac, |w, c| {
                        let ch = c.node();
                        match w.k(ch) {
                            K::Observer | K::Accessor => w.accessor(c, ty),
                            _ if ch.is_named() => w.visit(c),
                            _ => {}
                        }
                    });
                }
                self.put(ac);
                self.rets.pop();
            }
            if encl != NONE {
                self.cx.pop_def();
            }
            if is_def {
                for &d in defs.iter().take(cnt) {
                    if d != NONE && ann_ty == NONE && cnt == 1 {
                        let q = self.recv_q(inferred);
                        self.cx.def_mut(d).type_name = q;
                    }
                }
            } else {
                for nm in names.iter().take(cnt).flatten() {
                    let id = self.nm(*nm);
                    self.cx.declare(id, if cnt == 1 { ty } else { NONE });
                }
            }
        }
    }

    /// Where a signature that ends at an initialiser ends: before the `=` and the space around.
    fn before_eq(&self, mut end: usize) -> usize {
        let src = self.cx.src;
        while end > 0 && src[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        if end > 0 && src[end - 1] == b'=' {
            end -= 1;
        }
        while end > 0 && src[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        end
    }

    /// Arity of a function-typed annotation (`(Int, String) -> Void`, optional or not).
    fn fn_arity(&self, mut ty: Node<'t>) -> Option<(u8, u8)> {
        let t = self.t;
        for _ in 0..6 {
            match self.k(ty) {
                K::FunctionType => {
                    let params = ty.named_child(0)?;
                    let n = if self.k(params) == K::TupleType { params.named_child_count().min(254) as u8 } else { 1 };
                    return Some((n, n));
                }
                K::TypeAnnotation => ty = child(ty, t.f_type).or_else(|| ty.named_child(0))?,
                K::OptionalType => ty = child(ty, t.f_wrapped)?,
                K::TupleType if ty.named_child_count() == 1 => ty = ty.named_child(0)?,
                K::TupleTypeItem => ty = child(ty, t.f_type).or_else(|| child(ty, t.f_name))?,
                _ => return None,
            }
        }
        None
    }

    /// `[T]` for an array of `T`, `[:V]` for a dictionary with values `V`: how a local remembers
    /// the elements of a collection (for `for` loops and subscripts). As a receiver it is the
    /// collection type itself (`recv_q`).
    fn collection(&mut self, dict: bool, elem: u32) -> u32 {
        if elem == NONE || self.cx.name_bytes(elem).first() == Some(&b'[') {
            return self.cx.intern(if dict { b"Dictionary" } else { b"Array" });
        }
        self.wrap(if dict { b"[:" } else { b"[" }, elem, b"]")
    }

    /// The element type of a collection type made by `collection`.
    fn elem_of(&mut self, p: u32) -> u32 {
        if p == NONE {
            return NONE;
        }
        let b = self.cx.name_bytes(p);
        if b.len() < 3 || b[0] != b'[' || b[b.len() - 1] != b']' {
            return NONE;
        }
        let inner = if b[1] == b':' { 2..b.len() - 1 } else { 1..b.len() - 1 };
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.extend_from_slice(&self.cx.name_bytes(p)[inner]);
        let id = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        id
    }

    /// A type as a receiver qualifier: a collection of `collection` is `Array` / `Dictionary`.
    fn recv_q(&mut self, p: u32) -> u32 {
        if p == NONE {
            return NONE;
        }
        match self.cx.name_bytes(p) {
            [b'[', b':', ..] => self.cx.intern(b"Dictionary"),
            [b'[', ..] => self.cx.intern(b"Array"),
            _ => p,
        }
    }

    fn recv_r(&mut self, r: R) -> R {
        match r {
            R::Val(p) => R::Val(self.recv_q(p)),
            _ => r,
        }
    }

    /// The type a declaration takes from its initialiser.
    fn value_type(&self, r: R) -> u32 {
        match r {
            R::Val(p) => p,
            _ => NONE,
        }
    }

    /// A property or global initialised with a function: `Bind` of the function into the slot.
    fn bind_value(&mut self, v: Node<'t>, d: u32, owner: u32) {
        let Some(f) = self.function_value(v) else { return };
        let name = self.cx.def_mut(d).name;
        let slot = self.slot(owner, name);
        self.cx.add_ref(f, RefKind::Bind, line_of(v), slot, 255);
    }

    /// The function an expression names without calling it: a bare non-local name that is not a
    /// variable of this file, `self.m`, `T.m`.
    fn function_value(&mut self, v: Node<'t>) -> Option<u32> {
        let t = self.t;
        match self.k(v) {
            K::SimpleIdentifier => {
                let text = self.txt(v);
                if text.is_empty() || is_upper(text) || text[0] == b'$' || is_builtin_fn(text) {
                    return None;
                }
                let id = self.cx.intern(text);
                if self.cx.is_local(id) || self.top_flags(id) & F_PROP != 0 {
                    return None;
                }
                Some(id)
            }
            K::NavigationExpression => {
                let tg = child(v, t.f_target)?;
                let ok = match self.k(tg) {
                    K::SelfExpression => true,
                    K::SimpleIdentifier => is_upper(self.txt(tg)),
                    _ => false,
                };
                let s = child(v, t.f_suffix).and_then(|s| child(s, t.f_suffix))?;
                let text = self.txt(s);
                if !ok || text.is_empty() || is_upper(text) || text == b"self" || self.k(s) != K::SimpleIdentifier {
                    return None;
                }
                let id = self.cx.intern(text);
                if self.top_flags(id) & F_PROP != 0 {
                    return None;
                }
                Some(id)
            }
            _ => None,
        }
    }

    // ----- statements -----

    /// `if`, `guard` and `while`: their bindings (`if let x = e`, `if let x`, `if case let
    /// .some(v) = e`) are locals of the statement, of the enclosing block for `guard`.
    fn conditional(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let guard = self.k(n) == K::GuardStatement;
        if !guard {
            self.cx.push_scope();
        }
        let mut pending: [u32; MAX_BINDINGS] = [NONE; MAX_BINDINGS];
        let mut np = 0usize;
        let mut in_case = false;
        let mut binding = false;
        let mut after_eq = false;
        let mut dot = false;
        let mut case_ty = NONE;
        self.each(c, |w, c| {
            let ch = c.node();
            if !ch.is_named() {
                match w.txt(ch) {
                    b"case" => {
                        in_case = true;
                        binding = false;
                        case_ty = w.case_subject(ch);
                    }
                    b"=" => {
                        after_eq = true;
                        in_case = false;
                    }
                    b"," => {
                        for &p in &pending[..np] {
                            if p != NONE && !w.cx.is_local(p) {
                                w.cx.declare(p, NONE);
                            } else if p != NONE {
                                // `if let x` (shorthand) keeps the type of the `x` it unwraps.
                                let ty = w.cx.local_type(p).unwrap_or(NONE);
                                w.cx.declare(p, ty);
                            }
                        }
                        np = 0;
                        after_eq = false;
                        binding = false;
                    }
                    b"." => dot = true,
                    _ => {}
                }
                return;
            }
            let was_dot = std::mem::replace(&mut dot, false);
            if is_field(c, t.f_bound_identifier) {
                if np < MAX_BINDINGS {
                    pending[np] = w.nm(ch);
                    np += 1;
                }
                return;
            }
            match w.k(ch) {
                K::ValueBindingPattern => binding = true,
                K::SimpleIdentifier if in_case && was_dot => {
                    if !is_builtin_value(w.txt(ch)) {
                        let id = w.nm(ch);
                        w.cx.add_ref(id, RefKind::MemberRead, line_of(ch), case_ty, 255);
                    }
                }
                K::Pattern if in_case => w.pattern(c, binding, case_ty),
                K::SimpleIdentifier if in_case && binding => {
                    let id = w.nm(ch);
                    w.cx.declare(id, NONE);
                }
                _ if after_eq && np > 0 => {
                    let r = w.ex(c);
                    let ty = w.value_type(r);
                    for &p in &pending[..np] {
                        if p != NONE {
                            w.cx.declare(p, if np == 1 { ty } else { NONE });
                        }
                    }
                    np = 0;
                    after_eq = false;
                }
                _ if after_eq => {
                    after_eq = false;
                    w.ex(c);
                }
                _ => {
                    // A shorthand binding (`if let x {`) takes effect before the body.
                    if np > 0 && !in_case {
                        for &p in &pending[..np] {
                            if p != NONE {
                                let ty = w.cx.local_type(p).unwrap_or(NONE);
                                w.cx.declare(p, ty);
                            }
                        }
                        np = 0;
                    }
                    w.visit(c);
                }
            }
        });
        if !guard {
            self.cx.pop_scope();
        }
    }

    /// The type of the value an `if case` / `guard case` pattern is matched against, when it is
    /// `self` or a local: the expression after the `=` that follows the `case` token.
    fn case_subject(&mut self, case_tok: Node<'t>) -> u32 {
        let mut n = case_tok.next_sibling();
        for _ in 0..32 {
            let Some(x) = n else { return NONE };
            if !x.is_named() && self.txt(x) == b"=" {
                let v = x.next_named_sibling();
                let ty = match v.map(|v| (self.k(v), v)) {
                    Some((K::SelfExpression, _)) => self.self_ty(),
                    Some((K::SimpleIdentifier, v)) => {
                        let id = self.nm(v);
                        self.cx.local_type(id).unwrap_or(NONE)
                    }
                    _ => NONE,
                };
                return self.recv_q(ty);
            }
            n = x.next_sibling();
        }
        NONE
    }

    /// A pattern of a `case`, a `catch`, a `for` or an `if case`: names after `let` / `var`
    /// are locals, a name after `.` is an enum case of type `case_ty`, the rest are expressions.
    fn pattern(&mut self, c: &mut TreeCursor<'t>, binding: bool, case_ty: u32) {
        let t = self.t;
        let mut binding = binding;
        let mut dot = false;
        self.each(c, |w, c| {
            let ch = c.node();
            if !ch.is_named() {
                let text = w.txt(ch);
                if text == b"." {
                    dot = true;
                } else if text == b"let" || text == b"var" {
                    binding = true;
                }
                return;
            }
            let was_dot = std::mem::replace(&mut dot, false);
            match w.k(ch) {
                K::ValueBindingPattern => binding = true,
                K::SimpleIdentifier if was_dot => {
                    if !is_builtin_value(w.txt(ch)) {
                        let id = w.nm(ch);
                        w.cx.add_ref(id, RefKind::MemberRead, line_of(ch), case_ty, 255);
                    }
                }
                K::SimpleIdentifier if binding || is_field(c, t.f_bound_identifier) => {
                    let id = w.nm(ch);
                    w.cx.declare(id, NONE);
                }
                K::Pattern => w.pattern(c, binding, NONE),
                _ => w.visit(c),
            }
        });
    }

    fn for_statement(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        self.cx.push_scope();
        let n = c.node();
        // The sequence is evaluated before the loop variable exists.
        let mut elem = NONE;
        if let Some(coll) = child(n, t.f_collection) {
            let r = self.ex_node(coll);
            let seq = self.value_type(r);
            elem = self.elem_of(seq);
        }
        let item = child(n, t.f_item);
        self.each(c, |w, c| {
            let ch = c.node();
            if !ch.is_named() || is_field(c, t.f_collection) {
                return;
            }
            if Some(ch) == item {
                let mut names = [None; MAX_BINDINGS];
                let mut cnt = 0;
                if let Some(b) = child(ch, t.f_bound_identifier) {
                    names[0] = Some(b);
                    cnt = 1;
                } else {
                    w.pattern_names(ch, &mut names, &mut cnt, 0);
                }
                for nm in names.iter().take(cnt).flatten() {
                    let id = w.nm(*nm);
                    w.cx.declare(id, if cnt == 1 { elem } else { NONE });
                }
                return;
            }
            w.visit(c);
        });
        self.cx.pop_scope();
    }

    fn switch_statement(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let n = c.node();
        let mut subject = NONE;
        if let Some(e) = child(n, t.f_expr) {
            let r = self.ex_node(e);
            let v = self.value_type(r);
            subject = self.recv_q(v);
        }
        self.each(c, |w, c| {
            let ch = c.node();
            if !ch.is_named() || is_field(c, t.f_expr) {
                return;
            }
            if w.k(ch) != K::SwitchEntry {
                return w.visit(c);
            }
            w.cx.push_scope();
            w.each(c, |w, c| {
                let x = c.node();
                match w.k(x) {
                    K::SwitchPattern => w.each(c, |w, c| {
                        if w.k(c.node()) == K::Pattern {
                            w.pattern(c, false, subject);
                        } else if c.node().is_named() {
                            w.visit(c);
                        }
                    }),
                    _ if x.is_named() => w.visit(c),
                    _ => {}
                }
            });
            w.cx.pop_scope();
        });
    }

    /// `catch let e as E { }` binds `e` of type `E`, a bare `catch { }` binds `error`.
    fn catch_block(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let n = c.node();
        self.cx.push_scope();
        match child(n, t.f_error) {
            None => self.cx.declare(self.id_error, NONE),
            Some(p) => {
                // `let e as E`: the type is the `as` part of the outer pattern.
                let mut ty = NONE;
                let mut pc = self.cur(p);
                each_child(&mut pc, |pc| {
                    let x = pc.node();
                    if matches!(self.k(x), K::UserType | K::TypeIdentifier) {
                        ty = self.type_name_of(x);
                    }
                });
                self.put(pc);
                let mut names = [None; MAX_BINDINGS];
                let mut cnt = 0;
                self.pattern_names(p, &mut names, &mut cnt, 0);
                if let Some(b) = child(p, t.f_bound_identifier) {
                    names[0] = Some(b);
                    cnt = cnt.max(1);
                }
                for nm in names.iter().take(cnt).flatten() {
                    let id = self.nm(*nm);
                    self.cx.declare(id, if cnt == 1 { ty } else { NONE });
                }
            }
        }
        self.each(c, |w, c| {
            let ch = c.node();
            if !ch.is_named() {
                return;
            }
            if is_field(c, t.f_error) {
                // Names are declared; the types and enum cases of the pattern are references.
                w.each(c, |w, c| {
                    let x = c.node();
                    if matches!(w.k(x), K::UserType | K::TypeIdentifier) {
                        w.visit(c);
                    }
                });
                return;
            }
            w.visit(c);
        });
        self.cx.pop_scope();
    }

    /// A closure: its captures and parameters are locals; `$0` and friends are always local.
    fn lambda(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        self.cx.push_scope();
        self.fn_depth += 1;
        self.rets.push(NONE);
        let saved_callee = std::mem::replace(&mut self.callee, NONE);
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::CaptureList => w.each(c, |w, c| {
                    if w.k(c.node()) != K::CaptureListItem {
                        return;
                    }
                    let item = c.node();
                    if let Some(v) = child(item, t.f_value) {
                        w.ex_node(v);
                    }
                    if let Some(nm) = child(item, t.f_name).filter(|x| w.k(*x) == K::SimpleIdentifier) {
                        let id = w.nm(nm);
                        w.cx.declare(id, NONE);
                    }
                }),
                K::LambdaFunctionType => w.lambda_type(c),
                _ if ch.is_named() => w.visit(c),
                _ => {}
            }
        });
        self.callee = saved_callee;
        self.rets.pop();
        self.fn_depth -= 1;
        self.cx.pop_scope();
    }

    fn lambda_type(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::Other if ch.is_named() => w.each(c, |w, c| {
                    let p = c.node();
                    if w.k(p) != K::LambdaParameter {
                        if p.is_named() {
                            w.visit(c);
                        }
                        return;
                    }
                    let nm = child(p, t.f_name).filter(|x| w.k(*x) == K::SimpleIdentifier);
                    let ty = child(p, t.f_type).map_or(NONE, |x| w.type_name_of(x));
                    w.each(c, |w, c| {
                        let x = c.node();
                        if x.is_named() && Some(x) != nm && w.k(x) != K::SimpleIdentifier {
                            w.visit(c);
                        }
                    });
                    if let Some(nm) = nm {
                        let id = w.nm(nm);
                        w.cx.declare(id, ty);
                    }
                }),
                _ if ch.is_named() => w.visit(c),
                _ => {}
            }
        });
    }

    // ----- expressions -----

    /// Visit an expression and say what it denotes as a receiver.
    fn ex(&mut self, c: &mut TreeCursor<'t>) -> R {
        let want = std::mem::replace(&mut self.want, NONE);
        if self.cx.depth >= MAX_DEPTH {
            return UNKNOWN;
        }
        self.cx.depth += 1;
        let n = c.node();
        let t = self.t;
        let r = match self.k(n) {
            K::SimpleIdentifier => self.ident(n, false, false),
            K::NavigationExpression => self.navigation(n, c, false).0,
            K::CallExpression => self.call(n, c, want),
            K::ConstructorExpression => self.constructor(n, c),
            K::PrefixExpression => self.prefix(n, c, want),
            K::PostfixExpression => match child(n, t.f_target) {
                Some(x) => self.ex_node(x),
                None => UNKNOWN,
            },
            K::SelfExpression => R::Val(self.self_ty()),
            K::SuperExpression => R::Super,
            K::KeyPathExpression => self.key_path(c),
            K::SelectorExpression => {
                self.selector(c);
                UNKNOWN
            }
            K::MacroInvocation => {
                self.macro_invocation(n, c);
                UNKNOWN
            }
            K::Transparent => {
                let mut r = UNKNOWN;
                self.each(c, |w, c| {
                    if is_field(c, w.t.f_expr) {
                        w.want = want;
                        r = w.ex(c);
                    } else if c.node().is_named() {
                        w.visit(c);
                    }
                });
                r
            }
            K::TupleExpression => {
                // `(e)` is `e`.
                let single = n.named_child_count() == 1;
                let mut r = UNKNOWN;
                self.each(c, |w, c| {
                    if c.node().is_named() {
                        if single {
                            w.want = want;
                            r = w.ex(c);
                        } else {
                            w.visit(c);
                        }
                    }
                });
                r
            }
            K::TernaryExpression => {
                self.each(c, |w, c| {
                    if is_field(c, w.t.f_if_true) || is_field(c, w.t.f_if_false) {
                        w.want = want;
                        w.ex(c);
                    } else if c.node().is_named() {
                        w.visit(c);
                    }
                });
                UNKNOWN
            }
            K::EqualityExpression => {
                self.equality(n, c);
                UNKNOWN
            }
            K::AsExpression => {
                let mut ty = NONE;
                self.each(c, |w, c| {
                    let ch = c.node();
                    if !ch.is_named() {
                        return;
                    }
                    if !is_field(c, w.t.f_expr) && matches!(w.k(ch), K::UserType | K::OptionalType | K::ArrayType | K::DictionaryType) {
                        ty = w.type_name_of(ch);
                    }
                    w.visit(c);
                });
                R::Val(ty)
            }
            K::Assignment => {
                self.assignment(n);
                UNKNOWN
            }
            K::ControlTransfer => {
                let ret = *self.rets.last().unwrap_or(&NONE);
                let is_return = self.txt(n).starts_with(b"return");
                self.each(c, |w, c| {
                    if is_field(c, w.t.f_result) && is_return {
                        w.want = ret;
                        w.ex(c);
                    } else if c.node().is_named() {
                        w.visit(c);
                    }
                });
                UNKNOWN
            }
            K::LambdaLiteral => {
                self.lambda(c);
                UNKNOWN
            }
            K::Error | K::Skip | K::Comment => {
                self.visit(c);
                UNKNOWN
            }
            K::Other | K::UserType | K::TypeIdentifier | K::Pattern => {
                self.visit(c);
                UNKNOWN
            }
            _ => {
                self.visit_generic(c);
                UNKNOWN
            }
        };
        self.cx.depth -= 1;
        r
    }

    /// Children of a node that has no handler of its own.
    fn visit_generic(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        if self.has_error && n.is_missing() {
            self.cx.facts.parse_errors += 1;
        }
        self.children(c);
    }

    /// A bare name in an expression. `recv`: it is the receiver of a member access, where a
    /// type name is a `Type` reference rather than a value read.
    fn ident(&mut self, n: Node<'t>, recv: bool, write: bool) -> R {
        let text = self.txt(n);
        if text.is_empty() || text[0] == b'$' || text == b"_" {
            return UNKNOWN;
        }
        let id = self.cx.intern(text);
        if let Some(ty) = self.cx.local_type(id) {
            return R::Val(ty);
        }
        let line = line_of(n);
        if is_upper(text) {
            if text == b"Self" {
                let s = self.self_ty();
                return R::Ty(s, s, NONE);
            }
            if self.modules.contains(&id) && self.top_flags(id) & F_TYPE == 0 {
                return R::Module(id);
            }
            if !self.builtin_type(text, id) {
                self.cx.add_ref(id, if recv { RefKind::Type } else { RefKind::Read }, line, NONE, 255);
            }
            return R::Ty(id, id, NONE);
        }
        if is_builtin_value(text) {
            return UNKNOWN;
        }
        let (rk, mk) = if write { (RefKind::Write, RefKind::MemberWrite) } else { (RefKind::Read, RefKind::MemberRead) };
        if let Some(d) = self.member_owner(id) {
            self.cx.add_ref(id, mk, line, d, 255);
            return R::Val(self.seg(d, id, false));
        }
        let tf = self.top_flags(id);
        if tf & (F_VALUE | F_FUNC) != 0 {
            self.cx.add_ref(id, rk, line, NONE, 255);
            return R::Val(if tf & F_VALUE != 0 { self.wrap(b"$", id, b"") } else { NONE });
        }
        let s = self.self_ty();
        self.cx.add_ref(id, rk, line, s, 255);
        if s != NONE { R::Val(self.seg(s, id, false)) } else { R::Val(self.wrap(b"$", id, b"")) }
    }

    /// `target.name` read (or written): the member access and the path of its value. Returns
    /// (path, the receiver's path) for slots.
    fn navigation(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, write: bool) -> (R, u32) {
        let t = self.t;
        let (Some(tg), Some(sx)) = (child(n, t.f_target), child(n, t.f_suffix)) else {
            self.children(c);
            return (UNKNOWN, NONE);
        };
        let Some(s) = child(sx, t.f_suffix).filter(|s| self.k(*s) == K::SimpleIdentifier && s.end_byte() > s.start_byte()) else {
            self.recv_node(tg);
            return (UNKNOWN, NONE);
        };
        let raw = self.recv_node(tg);
        // `T.self`, `T.Type`: the metatype, no member.
        if matches!(self.txt(s), b"self" | b"Type" | b"Protocol") {
            return (raw, NONE);
        }
        let r = self.recv_r(raw);
        let name = self.nm(s);
        let line = line_of(s);
        if let R::Val(p) = raw {
            // `xs.first`, `xs.last` of an array whose elements are known.
            if matches!(self.txt(s), b"first" | b"last") {
                let e = self.elem_of(p);
                if e != NONE && !write {
                    let q = self.recv_q(p);
                    self.cx.add_ref(name, RefKind::MemberRead, line, q, 255);
                    return (R::Val(e), NONE);
                }
            }
        }
        let mk = if write { RefKind::MemberWrite } else { RefKind::MemberRead };
        let stext = self.txt(s);
        match r {
            R::Module(m) => {
                if is_upper(stext) {
                    self.cx.add_ref(name, RefKind::Type, line, m, 255);
                    let p = self.path3(m, b"|", name, b"");
                    (R::Ty(p, name, m), NONE)
                } else {
                    self.cx.add_ref(name, if write { RefKind::Write } else { RefKind::Read }, line, m, 255);
                    (R::Val(self.path3(m, b"|$", name, b"")), NONE)
                }
            }
            R::Ty(p, simple, _) => {
                if is_upper(stext) {
                    self.cx.add_ref(name, RefKind::Type, line, simple, 255);
                    (R::Ty(self.seg(p, name, false), name, simple), NONE)
                } else {
                    // A static member or an enum case.
                    self.cx.add_ref(name, mk, line, p, 255);
                    (R::Val(self.seg(p, name, false)), p)
                }
            }
            R::Val(p) => {
                self.cx.add_ref(name, mk, line, p, 255);
                (R::Val(self.seg(p, name, false)), p)
            }
            R::Super => {
                let s = self.self_ty();
                let q = self.wrap(b"^", s, b"");
                self.cx.add_ref(name, mk, line, q, 255);
                (UNKNOWN, q)
            }
        }
    }

    /// The receiver of a member access or call.
    fn recv(&mut self, c: &mut TreeCursor<'t>) -> R {
        let n = c.node();
        if self.k(n) == K::SimpleIdentifier { self.ident(n, true, false) } else { self.ex(c) }
    }

    fn call(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, want: u32) -> R {
        let Some(mut callee) = n.named_child(0) else { return UNKNOWN };
        // `!f(x)` and `-f(x)` are parsed as a call of `!f`: the operator applies to the result.
        for _ in 0..4 {
            if self.k(callee) != K::PrefixExpression || self.implicit_member(callee).is_some() {
                break;
            }
            match child(callee, self.t.f_target) {
                Some(x) => callee = x,
                None => break,
            }
        }
        let Some(suffix) = self.first_of(n, K::CallSuffix) else {
            self.children(c);
            return UNKNOWN;
        };
        // `xs[i]`: a subscript, whose element has the type the receiver's path names (arrays
        // and dictionaries are typed by their elements).
        if self.is_subscript(suffix) {
            let mut r = UNKNOWN;
            self.each(c, |w, c| {
                let ch = c.node();
                if ch == callee {
                    r = w.recv(c);
                } else if ch.is_named() {
                    w.visit(c);
                }
            });
            return match r {
                R::Val(p) => R::Val(self.elem_of(p)),
                _ => UNKNOWN,
            };
        }
        let nargs = self.count_args(suffix);
        let line = line_of(callee);
        // Resolve the callee first: its references, the call reference and the result.
        let (result, callee_name) = match self.k(callee) {
            K::SimpleIdentifier => {
                let text = self.txt(callee);
                if text == b"defer" || text.is_empty() || text[0] == b'$' {
                    (UNKNOWN, NONE)
                } else {
                    let id = self.cx.intern(text);
                    self.call_ident(id, line, nargs)
                }
            }
            K::NavigationExpression => self.call_member(callee, line, nargs),
            K::PrefixExpression => self.call_implicit(callee, line, nargs, want),
            _ => {
                self.ex_node(callee);
                (UNKNOWN, NONE)
            }
        };
        let saved = std::mem::replace(&mut self.callee, callee_name);
        self.call_suffix_node(suffix);
        self.callee = saved;
        result
    }

    fn is_subscript(&self, suffix: Node<'t>) -> bool {
        suffix.named_child(0).filter(|a| self.k(*a) == K::ValueArguments).and_then(|a| a.child(0)).is_some_and(|b| self.txt(b) == b"[")
    }

    /// Arguments of a call: the value arguments and the trailing closures.
    fn count_args(&mut self, suffix: Node<'t>) -> u8 {
        let t = self.t;
        let mut n = 0u32;
        let mut c = self.cur(suffix);
        each_child(&mut c, |c| {
            let ch = c.node();
            match self.k(ch) {
                K::ValueArguments => {
                    // The argument list is the cursor's own subtree: walk it in place.
                    if c.goto_first_child() {
                        loop {
                            let a = c.node();
                            if self.k(a) == K::ValueArgument && child(a, t.f_reference_specifier).is_none() {
                                n += 1;
                            }
                            if !c.goto_next_sibling() {
                                break;
                            }
                        }
                        c.goto_parent();
                    }
                }
                K::LambdaLiteral => n += 1,
                _ => {}
            }
        });
        self.put(c);
        n.min(254) as u8
    }

    fn call_suffix(&mut self, c: &mut TreeCursor<'t>) {
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::ValueArguments => w.each(c, |w, c| {
                    if w.k(c.node()) == K::ValueArgument {
                        w.value_argument(c);
                    } else if c.node().is_named() {
                        w.visit(c);
                    }
                }),
                K::LambdaLiteral => w.lambda(c),
                // The label of a further trailing closure.
                K::SimpleIdentifier => {}
                _ if ch.is_named() => w.visit(c),
                _ => {}
            }
        });
    }

    /// One argument: its value, and an `Arg` of the call when it names a function.
    fn value_argument(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let callee = self.callee;
        self.each(c, |w, c| {
            let ch = c.node();
            if !is_field(c, t.f_value) {
                if ch.is_named() && w.k(ch) != K::Skip && !is_field(c, t.f_name) && !is_field(c, t.f_reference_specifier) {
                    w.visit(c);
                }
                return;
            }
            // The arguments of `print(..)` and friends are never callbacks.
            let f = if callee != NONE && !is_builtin_fn(w.cx.name_bytes(callee)) { w.function_value(ch) } else { None };
            w.ex(c);
            if let Some(f) = f {
                w.cx.add_ref(f, RefKind::Arg, line_of(ch), callee, 255);
            }
        });
    }

    /// `f(..)` with a bare callee. Returns (result, callee name).
    fn call_ident(&mut self, id: u32, line: u32, nargs: u8) -> (R, u32) {
        if self.cx.is_local(id) {
            // A closure held by a local.
            return (UNKNOWN, NONE);
        }
        let text = self.cx.name_bytes(id);
        let tf = self.top_flags(id);
        // `XCTAssertEqual(..)`: the assertion functions of XCTest are written capitalised.
        if text.starts_with(b"XCT") && tf & F_FUNC == 0 {
            return (UNKNOWN, id);
        }
        if is_upper(text) && tf & F_FUNC == 0 {
            if text == b"Self" {
                let s = self.self_ty();
                if s != NONE {
                    self.cx.add_ref(s, RefKind::New, line, NONE, nargs);
                }
                return (R::Val(s), s);
            }
            if !self.builtin_type(text, id) {
                self.cx.add_ref(id, RefKind::New, line, NONE, nargs);
            }
            return (R::Val(id), id);
        }
        if is_builtin_fn(text) && tf & F_FUNC == 0 && self.member_owner(id).is_none() {
            return (UNKNOWN, id);
        }
        if self.local_fns.contains(&id) {
            self.cx.add_ref(id, RefKind::Call, line, NONE, nargs);
            return (R::Val(self.wrap(b"", id, b"()")), id);
        }
        if let Some(d) = self.member_owner(id) {
            self.cx.add_ref(id, RefKind::Call, line, d, nargs);
            return (R::Val(self.seg(d, id, true)), id);
        }
        if tf & (F_FUNC | F_VALUE) != 0 {
            self.cx.add_ref(id, RefKind::Call, line, NONE, nargs);
            return (R::Val(self.wrap(b"", id, b"()")), id);
        }
        let s = self.self_ty();
        self.cx.add_ref(id, RefKind::Call, line, s, nargs);
        if s != NONE { (R::Val(self.seg(s, id, true)), id) } else { (R::Val(self.wrap(b"", id, b"()")), id) }
    }

    /// `recv.name(..)`.
    fn call_member(&mut self, callee: Node<'t>, line: u32, nargs: u8) -> (R, u32) {
        let t = self.t;
        let (Some(tg), Some(s)) = (child(callee, t.f_target), child(callee, t.f_suffix).and_then(|s| child(s, t.f_suffix))) else {
            self.ex_node(callee);
            return (UNKNOWN, NONE);
        };
        if self.k(s) != K::SimpleIdentifier || s.end_byte() <= s.start_byte() {
            self.ex_node(callee);
            return (UNKNOWN, NONE);
        }
        let r = self.recv_node(tg);
        let r = self.recv_r(r);
        let name = self.nm(s);
        let stext = self.txt(s);
        let upper = is_upper(stext);
        let is_init = name == self.id_init;
        match r {
            R::Module(m) => {
                if upper {
                    self.cx.add_ref(name, RefKind::New, line, m, nargs);
                    (R::Val(self.path3(m, b"|", name, b"")), name)
                } else {
                    self.cx.add_ref(name, RefKind::Call, line, m, nargs);
                    (R::Val(self.path3(m, b"|", name, b"()")), name)
                }
            }
            R::Ty(p, simple, q) => {
                if is_init {
                    // `T.init(..)` is `T(..)`.
                    if simple != NONE {
                        self.cx.add_ref(simple, RefKind::New, line, q, nargs);
                    }
                    (R::Val(p), simple)
                } else if upper {
                    self.cx.add_ref(name, RefKind::New, line, simple, nargs);
                    (R::Val(self.seg(p, name, false)), name)
                } else {
                    self.cx.add_ref(name, RefKind::Call, line, p, nargs);
                    (R::Val(self.seg(p, name, true)), name)
                }
            }
            // `x.init(..)` on a receiver of unknown type (a type parameter, a metatype value)
            // says nothing about which initialiser runs.
            R::Val(NONE) if is_init => (UNKNOWN, name),
            R::Val(p) => {
                self.cx.add_ref(name, RefKind::MemberCall, line, p, nargs);
                (R::Val(if is_init { p } else { self.seg(p, name, true) }), name)
            }
            R::Super => {
                let s = self.self_ty();
                let q = self.wrap(b"^", s, b"");
                self.cx.add_ref(name, RefKind::MemberCall, line, q, nargs);
                (R::Val(if is_init { s } else { self.seg(s, name, true) }), name)
            }
        }
    }

    /// `.name(..)`: an implicit member of the contextual type `want`.
    fn call_implicit(&mut self, callee: Node<'t>, line: u32, nargs: u8, want: u32) -> (R, u32) {
        let Some(s) = self.implicit_member(callee) else {
            self.ex_node(callee);
            return (UNKNOWN, NONE);
        };
        let name = self.nm(s);
        if name == self.id_init {
            if want == NONE {
                return (UNKNOWN, NONE);
            }
            let wt = self.cx.name_bytes(want);
            if !wt.contains(&b':') && !wt.contains(&b'|') {
                self.cx.add_ref(want, RefKind::New, line, NONE, nargs);
            } else {
                self.cx.add_ref(name, RefKind::MemberCall, line, want, nargs);
            }
            return (R::Val(want), want);
        }
        if want != NONE {
            self.cx.add_ref(name, RefKind::Call, line, want, nargs);
            (R::Val(self.seg(want, name, true)), name)
        } else {
            self.cx.add_ref(name, RefKind::MemberCall, line, NONE, nargs);
            (UNKNOWN, name)
        }
    }

    /// The member named by `.name` (a prefix expression with the `.` operator).
    fn implicit_member(&self, n: Node<'t>) -> Option<Node<'t>> {
        let t = self.t;
        let first = n.child(0)?;
        if first.is_named() || self.txt(first) != b"." {
            return None;
        }
        child(n, t.f_target).filter(|x| self.k(*x) == K::SimpleIdentifier && x.end_byte() > x.start_byte())
    }

    fn prefix(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, want: u32) -> R {
        if let Some(s) = self.implicit_member(n) {
            let name = self.nm(s);
            self.cx.add_ref(name, RefKind::MemberRead, line_of(s), want, 255);
            return R::Val(self.seg(want, name, false));
        }
        // `&x`, `-x`, `!x`: `&x` keeps the type.
        let mut r = UNKNOWN;
        self.each(c, |w, c| {
            if is_field(c, w.t.f_target) {
                r = w.ex(c);
            } else if c.node().is_named() {
                w.visit(c);
            }
        });
        r
    }

    /// `T<A>(..)`, `[T](..)`, `[K: V](..)`: an instantiation of the named type.
    fn constructor(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) -> R {
        let t = self.t;
        let ty = child(n, t.f_constructed_type);
        let mut result = UNKNOWN;
        let mut callee = NONE;
        if let Some(ty) = ty.filter(|x| self.k(*x) == K::UserType) {
            let id = self.last_type_ident(ty);
            result = R::Val(self.type_name_of(ty));
            callee = id;
            let suffix = self.first_of(n, K::ConstructorSuffix);
            let nargs = suffix.map_or(0, |s| self.count_args(s));
            let text = if id == NONE { &b""[..] } else { self.cx.name_bytes(id) };
            if id != NONE && !self.builtin_type(text, id) && !self.cx.is_local(id) {
                self.cx.add_ref(id, RefKind::New, line_of(ty), NONE, nargs);
            }
        }
        let saved = std::mem::replace(&mut self.callee, callee);
        self.each(c, |w, c| {
            let ch = c.node();
            if Some(ch) == ty && w.k(ch) == K::UserType {
                // The type itself is the `New` reference; its arguments are types.
                w.each(c, |w, c| {
                    if c.node().is_named() && w.k(c.node()) != K::TypeIdentifier {
                        w.visit(c);
                    }
                });
            } else if w.k(ch) == K::ConstructorSuffix {
                w.call_suffix(c);
            } else if ch.is_named() {
                w.visit(c);
            }
        });
        self.callee = saved;
        result
    }

    /// `\T.a.b` and `\.a`: the root of a key path. The members are read through the
    /// navigation the grammar wraps around it.
    fn key_path(&mut self, c: &mut TreeCursor<'t>) -> R {
        let mut r = UNKNOWN;
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::TypeIdentifier => {
                    let id = w.nm(ch);
                    w.type_name_ref(ch, id, RefKind::Type, NONE);
                    let ty = w.type_name_of(ch);
                    r = R::Ty(ty, id, NONE);
                }
                K::SimpleIdentifier => {
                    let id = w.nm(ch);
                    let base = match r {
                        R::Ty(p, _, _) | R::Val(p) => p,
                        _ => NONE,
                    };
                    w.cx.add_ref(id, RefKind::MemberRead, line_of(ch), base, 255);
                    r = R::Val(w.seg(base, id, false));
                }
                _ if ch.is_named() => w.visit(c),
                _ => {}
            }
        });
        r
    }

    /// `#selector(T.method(_:))`, `#selector(method)`: the method is read as a value and handed
    /// to the call around it.
    fn selector(&mut self, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let callee = if self.callee != NONE { self.callee } else { self.cx.intern(b"selector") };
        self.each(c, |w, c| {
            let ch = c.node();
            if !ch.is_named() {
                return;
            }
            // `T.m(a:b:)` is parsed as a call: what is referenced is its callee.
            let target = if w.k(ch) == K::CallExpression { ch.named_child(0).unwrap_or(ch) } else { ch };
            let name = match w.k(target) {
                K::NavigationExpression => child(target, t.f_suffix).and_then(|s| child(s, t.f_suffix)),
                K::SimpleIdentifier => Some(target),
                _ => None,
            };
            w.ex_node(target);
            if let Some(nm) = name.filter(|x| w.k(*x) == K::SimpleIdentifier && x.end_byte() > x.start_byte()) {
                let id = w.nm(nm);
                w.cx.add_ref(id, RefKind::Arg, line_of(nm), callee, 255);
            }
        });
    }

    /// `#name(..)`: a macro expansion, unless it is one the language provides.
    fn macro_invocation(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let mut nargs = 255u8;
        if let Some(s) = self.first_of(n, K::CallSuffix) {
            nargs = self.count_args(s);
        }
        self.each(c, |w, c| {
            let ch = c.node();
            match w.k(ch) {
                K::SimpleIdentifier => {
                    let text = w.txt(ch);
                    if !text.is_empty() && !is_builtin_macro(text) {
                        let id = w.cx.intern(text);
                        w.cx.add_ref(id, RefKind::MacroCall, line_of(ch), NONE, nargs);
                    }
                }
                K::CallSuffix => w.call_suffix(c),
                _ if ch.is_named() => w.visit(c),
                _ => {}
            }
        });
    }

    /// `a == .member`: the implicit member has the type of the other side.
    fn equality(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let (lhs, rhs) = (child(n, t.f_lhs), child(n, t.f_rhs));
        let implicit = |w: &Self, x: Option<Node<'t>>| x.is_some_and(|x| w.k(x) == K::PrefixExpression && w.implicit_member(x).is_some());
        let (first, second) = if implicit(self, rhs) {
            (lhs, rhs)
        } else if implicit(self, lhs) {
            (rhs, lhs)
        } else {
            return self.children(c);
        };
        let mut ty = NONE;
        if let Some(x) = first {
            let r = self.ex_node(x);
            let v = self.value_type(r);
            ty = self.recv_q(v);
        }
        if let Some(x) = second {
            self.want = ty;
            self.ex_node(x);
        }
    }

    /// `target = value`, `target += value`: writes, a contextual type for the value, and a
    /// `Bind` when a function is stored into a member.
    fn assignment(&mut self, n: Node<'t>) {
        let t = self.t;
        let target = child(n, t.f_target).and_then(|d| d.named_child(0));
        let value = child(n, t.f_result);
        let mut ty = NONE;
        let mut slot = NONE;
        if let Some(tg) = target {
            match self.k(tg) {
                K::SimpleIdentifier => {
                    let r = self.ident(tg, false, true);
                    let v = self.value_type(r);
                    ty = self.recv_q(v);
                    let text = self.txt(tg);
                    if !text.is_empty() {
                        let id = self.cx.intern(text);
                        if !self.cx.is_local(id) {
                            let owner = self.member_owner(id).unwrap_or_else(|| if self.top_flags(id) & F_VALUE != 0 { NONE } else { self.self_ty() });
                            slot = self.slot(owner, id);
                        }
                    }
                }
                K::NavigationExpression => {
                    let mut tc = self.cur(tg);
                    let (r, owner) = self.navigation(tg, &mut tc, true);
                    self.put(tc);
                    let v = self.value_type(r);
                    ty = self.recv_q(v);
                    if let Some(s) = child(tg, t.f_suffix).and_then(|s| child(s, t.f_suffix)).filter(|s| self.k(*s) == K::SimpleIdentifier) {
                        let id = self.nm(s);
                        slot = self.slot(owner, id);
                    }
                }
                _ => {
                    self.ex_node(tg);
                }
            }
        }
        if let Some(v) = value {
            let f = if slot != NONE { self.function_value(v) } else { None };
            self.want = ty;
            self.ex_node(v);
            if let Some(f) = f {
                self.cx.add_ref(f, RefKind::Bind, line_of(v), slot, 255);
            }
        }
    }
}

/// A name that reads like a protocol: the conventions of the standard library and of Cocoa.
fn looks_like_protocol(s: &[u8]) -> bool {
    s.ends_with(b"Protocol")
        || s.ends_with(b"Delegate")
        || s.ends_with(b"DataSource")
        || s.ends_with(b"able")
        || s.ends_with(b"ible")
        || s.ends_with(b"Convertible")
        || s.ends_with(b"Representable")
        || s.starts_with(b"Expressible")
        || is_builtin_type(s) && !matches!(s, b"NSObject")
}

fn is_system_module(m: &[u8]) -> bool {
    matches!(
        m,
        b"Foundation"
            | b"FoundationEssentials"
            | b"UIKit"
            | b"AppKit"
            | b"SwiftUI"
            | b"Combine"
            | b"XCTest"
            | b"Testing"
            | b"Dispatch"
            | b"Darwin"
            | b"Glibc"
            | b"Musl"
            | b"WinSDK"
            | b"os"
            | b"CoreFoundation"
            | b"CoreGraphics"
            | b"CoreData"
            | b"CoreLocation"
            | b"MapKit"
            | b"Network"
            | b"Security"
            | b"Swift"
            | b"_Concurrency"
            | b"Observation"
            | b"WatchKit"
            | b"Cocoa"
            | b"AVFoundation"
            | b"QuartzCore"
            | b"SystemConfiguration"
            | b"CFNetwork"
            | b"MobileCoreServices"
            | b"UniformTypeIdentifiers"
            | b"Synchronization"
    )
}

/// Standard library types: no reference is recorded for them (a declared type of that name
/// still names the receiver, so that extensions of it are found).
fn is_builtin_type(s: &[u8]) -> bool {
    matches!(
        s,
        b"Int"
            | b"Int8"
            | b"Int16"
            | b"Int32"
            | b"Int64"
            | b"UInt"
            | b"UInt8"
            | b"UInt16"
            | b"UInt32"
            | b"UInt64"
            | b"Float"
            | b"Float16"
            | b"Float80"
            | b"Double"
            | b"Bool"
            | b"String"
            | b"Substring"
            | b"Character"
            | b"Array"
            | b"ArraySlice"
            | b"ContiguousArray"
            | b"Dictionary"
            | b"Set"
            | b"Optional"
            | b"Any"
            | b"AnyObject"
            | b"AnyClass"
            | b"AnyHashable"
            | b"Void"
            | b"Never"
            | b"Self"
            | b"Error"
            | b"Equatable"
            | b"Hashable"
            | b"Comparable"
            | b"Codable"
            | b"Encodable"
            | b"Decodable"
            | b"Sendable"
            | b"Identifiable"
            | b"CaseIterable"
            | b"RawRepresentable"
            | b"CustomStringConvertible"
            | b"CustomDebugStringConvertible"
            | b"Sequence"
            | b"Collection"
            | b"IteratorProtocol"
            | b"BidirectionalCollection"
            | b"RandomAccessCollection"
            | b"MutableCollection"
            | b"RangeReplaceableCollection"
            | b"Range"
            | b"ClosedRange"
            | b"Result"
            | b"Task"
            | b"MainActor"
            | b"Actor"
            | b"AsyncSequence"
            | b"AsyncIteratorProtocol"
            | b"AsyncStream"
            | b"AsyncThrowingStream"
            | b"UnsafePointer"
            | b"UnsafeMutablePointer"
            | b"UnsafeRawPointer"
            | b"UnsafeMutableRawPointer"
            | b"UnsafeBufferPointer"
            | b"UnsafeMutableBufferPointer"
            | b"UnsafeRawBufferPointer"
            | b"UnsafeMutableRawBufferPointer"
            | b"OpaquePointer"
            | b"MemoryLayout"
            | b"ObjectIdentifier"
            | b"StaticString"
            | b"Numeric"
            | b"BinaryInteger"
            | b"FixedWidthInteger"
            | b"SignedInteger"
            | b"UnsignedInteger"
            | b"FloatingPoint"
            | b"BinaryFloatingPoint"
            | b"Strideable"
            | b"ExpressibleByStringLiteral"
            | b"ExpressibleByIntegerLiteral"
            | b"ExpressibleByArrayLiteral"
            | b"ExpressibleByDictionaryLiteral"
            | b"ExpressibleByBooleanLiteral"
            | b"ExpressibleByNilLiteral"
            | b"ExpressibleByFloatLiteral"
            | b"TextOutputStream"
            | b"NSObject"
            | b"Data"
            | b"Date"
            | b"URL"
            | b"UUID"
            | b"CGFloat"
    )
}

/// Free functions of the standard library.
fn is_builtin_fn(s: &[u8]) -> bool {
    matches!(
        s,
        b"print"
            | b"debugPrint"
            | b"dump"
            | b"fatalError"
            | b"precondition"
            | b"preconditionFailure"
            | b"assert"
            | b"assertionFailure"
            | b"min"
            | b"max"
            | b"abs"
            | b"zip"
            | b"stride"
            | b"sequence"
            | b"repeatElement"
            | b"swap"
            | b"type"
            | b"withUnsafePointer"
            | b"withUnsafeMutablePointer"
            | b"withUnsafeBytes"
            | b"withUnsafeMutableBytes"
            | b"withExtendedLifetime"
            | b"withoutActuallyEscaping"
            | b"numericCast"
            | b"unsafeBitCast"
            | b"unsafeDowncast"
            | b"autoreleasepool"
            | b"withTaskGroup"
            | b"withThrowingTaskGroup"
            | b"withDiscardingTaskGroup"
            | b"withThrowingDiscardingTaskGroup"
            | b"withCheckedContinuation"
            | b"withCheckedThrowingContinuation"
            | b"withUnsafeContinuation"
            | b"withUnsafeThrowingContinuation"
            | b"withTaskCancellationHandler"
            | b"readLine"
            | b"isKnownUniquelyReferenced"
            | b"defer"
    )
}

/// Names that are values of the language rather than of a program.
fn is_builtin_value(s: &[u8]) -> bool {
    matches!(s, b"self" | b"super" | b"nil" | b"true" | b"false" | b"some" | b"none" | b"fallthrough")
}

fn is_builtin_attr(s: &[u8]) -> bool {
    matches!(s, b"Sendable" | b"MainActor" | b"Test" | b"Suite")
}

fn is_builtin_macro(s: &[u8]) -> bool {
    matches!(
        s,
        b"expect"
            | b"require"
            | b"warning"
            | b"error"
            | b"file"
            | b"fileID"
            | b"filePath"
            | b"line"
            | b"column"
            | b"function"
            | b"dsohandle"
            | b"available"
            | b"unavailable"
            | b"colorLiteral"
            | b"imageLiteral"
            | b"fileLiteral"
            | b"isolation"
            | b"externalMacro"
            | b"Preview"
    )
}
