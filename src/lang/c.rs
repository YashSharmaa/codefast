//! C extractor (tree-sitter-c).
//!
//! Beyond plain definitions and call sites it understands the idioms that make large C code
//! bases hard to navigate with text search:
//!   * functions and variables defined through macros (`SYSCALL_DEFINE3(read, ...) { }`,
//!     `static DEFINE_MUTEX(lock);`),
//!   * function pointers bound into operation tables (`.read_iter = ext4_file_read_iter`),
//!   * calls dispatched through those tables (`file->f_op->read_iter(...)`), recorded with the
//!     receiver's type path so they can be linked to the bound functions at query time,
//!   * references hidden inside macro bodies, recovered with a small lexical scan.

use crate::extract::*;
use crate::facts::*;
use crate::lang::Lang;
use std::sync::OnceLock;
use tree_sitter::{Node, Tree, TreeCursor};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    Other,
    Skip,
    Identifier,
    TypeIdentifier,
    FunctionDefinition,
    Declaration,
    TypeDefinition,
    StructSpecifier,
    UnionSpecifier,
    EnumSpecifier,
    FieldDeclaration,
    Enumerator,
    PreprocDef,
    PreprocFunctionDef,
    PreprocInclude,
    CallExpression,
    FieldExpression,
    AssignmentExpression,
    UpdateExpression,
    InitializerList,
    InitializerPair,
    ExpressionStatement,
    CompoundStatement,
    MacroTypeSpecifier,
    Error,
    ParameterDeclaration,
    CompoundLiteral,
    FunctionDeclarator,
    PointerDeclarator,
    ParenthesizedDeclarator,
    ArrayDeclarator,
    AttributedDeclarator,
    InitDeclarator,
    ParenthesizedExpression,
    PointerExpression,
    SubscriptExpression,
    CastExpression,
    FieldDesignator,
    StorageClass,
    VariadicParameter,
    TypeDescriptor,
    FieldIdentifier,
    Comment,
}

struct Tables {
    kinds: KindTable<K>,
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
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let lang: tree_sitter::Language = tree_sitter_c::LANGUAGE.into();
        let kinds = KindTable::build(
            &lang,
            K::Other,
            &[
                ("identifier", K::Identifier),
                ("type_identifier", K::TypeIdentifier),
                ("field_identifier", K::FieldIdentifier),
                ("function_definition", K::FunctionDefinition),
                ("declaration", K::Declaration),
                ("type_definition", K::TypeDefinition),
                ("struct_specifier", K::StructSpecifier),
                ("union_specifier", K::UnionSpecifier),
                ("enum_specifier", K::EnumSpecifier),
                ("field_declaration", K::FieldDeclaration),
                ("enumerator", K::Enumerator),
                ("preproc_def", K::PreprocDef),
                ("preproc_function_def", K::PreprocFunctionDef),
                ("preproc_include", K::PreprocInclude),
                ("call_expression", K::CallExpression),
                ("field_expression", K::FieldExpression),
                ("assignment_expression", K::AssignmentExpression),
                ("update_expression", K::UpdateExpression),
                ("initializer_list", K::InitializerList),
                ("initializer_pair", K::InitializerPair),
                ("expression_statement", K::ExpressionStatement),
                ("compound_statement", K::CompoundStatement),
                ("macro_type_specifier", K::MacroTypeSpecifier),
                ("ERROR", K::Error),
                ("parameter_declaration", K::ParameterDeclaration),
                ("compound_literal_expression", K::CompoundLiteral),
                ("function_declarator", K::FunctionDeclarator),
                ("pointer_declarator", K::PointerDeclarator),
                ("parenthesized_declarator", K::ParenthesizedDeclarator),
                ("array_declarator", K::ArrayDeclarator),
                ("attributed_declarator", K::AttributedDeclarator),
                ("init_declarator", K::InitDeclarator),
                ("parenthesized_expression", K::ParenthesizedExpression),
                ("pointer_expression", K::PointerExpression),
                ("subscript_expression", K::SubscriptExpression),
                ("cast_expression", K::CastExpression),
                ("field_designator", K::FieldDesignator),
                ("storage_class_specifier", K::StorageClass),
                ("variadic_parameter", K::VariadicParameter),
                ("type_descriptor", K::TypeDescriptor),
                ("comment", K::Comment),
                // leaves and subtrees that never contain references
                ("string_literal", K::Skip),
                ("char_literal", K::Skip),
                ("number_literal", K::Skip),
                ("system_lib_string", K::Skip),
                ("preproc_arg", K::Skip),
                ("preproc_call", K::Skip),
                ("preproc_params", K::Skip),
                ("preproc_directive", K::Skip),
                ("attribute_specifier", K::Skip),
                ("attribute_declaration", K::Skip),
                ("ms_declspec_modifier", K::Skip),
                ("gnu_asm_expression", K::Skip),
                ("statement_identifier", K::Skip),
                ("primitive_type", K::Skip),
                ("sized_type_specifier", K::Skip),
                ("type_qualifier", K::Skip),
                ("true", K::Skip),
                ("false", K::Skip),
                ("null", K::Skip),
            ],
        );
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
        }
    })
}

/// A `{ ... }` block at file scope that belongs to a definition recognised just before it.
struct PendingBody<'t> {
    at: u32,
    def: u32,
    fdecl: Option<Node<'t>>,
}

struct W<'a, 't> {
    cx: Ctx<'a>,
    t: &'static Tables,
    fn_depth: u32,
    /// Enclosing struct / union / enum / typedef definitions (owners of members).
    containers: Vec<u32>,
    /// Type path for designated initializers in the current initializer list, or `NONE`.
    init_ctx: Vec<u32>,
    pending_body: Option<PendingBody<'t>>,
    /// Identifier arguments of a function-defining macro: treated as locals in its body.
    macro_params: Vec<u32>,
    /// Parameters of the `#define` being scanned.
    pp_params: Vec<u32>,
    /// Names passed to `EXPORT_SYMBOL*`.
    exports: Vec<u32>,
    /// Lines whose symbol-defining or registering macro was handled by the lexical pass, with
    /// the first definition created there (`NONE` for registrations).
    lex: Vec<(u32, u32)>,
}

pub fn extract(src: &[u8], tree: &Tree, path: &str, facts: &mut FileFacts, scratch: &mut Scratch) {
    let mut cx = Ctx::new(src, path, facts, scratch);
    cx.is_header = Lang::is_header(path);
    let mut w = W {
        cx,
        t: tables(),
        fn_depth: 0,
        containers: Vec::new(),
        init_ctx: Vec::new(),
        pending_body: None,
        macro_params: Vec::new(),
        pp_params: Vec::new(),
        exports: Vec::new(),
        lex: Vec::new(),
    };
    w.lexical_macros();
    let mut c = tree.walk();
    w.visit(&mut c);
    w.finish();
}

impl<'a, 't> W<'a, 't> {
    #[inline]
    fn k(&self, n: Node<'t>) -> K {
        self.t.kinds.get(n.kind_id())
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

    fn visit(&mut self, c: &mut TreeCursor<'t>) {
        let n = c.node();
        match self.k(n) {
            K::Skip | K::FieldIdentifier | K::StorageClass | K::VariadicParameter | K::Comment => {}
            K::Identifier => self.identifier(n),
            K::TypeIdentifier => self.type_identifier(n),
            K::FunctionDefinition => self.function_definition(n, c),
            K::Declaration => self.declaration(n, c),
            K::TypeDefinition => self.type_definition(n, c),
            K::StructSpecifier => self.record_specifier(n, c, DefKind::Struct),
            K::UnionSpecifier => self.record_specifier(n, c, DefKind::Union),
            K::EnumSpecifier => self.record_specifier(n, c, DefKind::Enum),
            K::FieldDeclaration => self.field_declaration(n, c),
            K::Enumerator => self.enumerator(n, c),
            K::PreprocDef => self.preproc_def(n, false),
            K::PreprocFunctionDef => self.preproc_def(n, true),
            K::PreprocInclude => self.include(n),
            K::CallExpression => self.call_expression(n, c),
            K::FieldExpression => self.field_expression(n, c),
            K::AssignmentExpression => self.assignment(n, c),
            K::UpdateExpression => self.update(n, c),
            K::InitializerList => self.initializer_list(c),
            K::ExpressionStatement => self.expression_statement(n, c),
            K::CompoundStatement => self.compound(n, c),
            K::MacroTypeSpecifier => self.macro_type_specifier(n, c),
            K::Error => self.error(c),
            K::ParameterDeclaration => self.parameter_declaration(n, c),
            K::CompoundLiteral => self.compound_literal(n, c),
            _ => self.children(c),
        }
    }

    // ----- leaves -----

    fn identifier(&mut self, n: Node<'t>) {
        let pos = n.start_byte() as u32;
        if self.cx.s.handled.contains(&pos) {
            return;
        }
        // Macro argument soup can surface keywords as identifiers.
        if is_c_keyword(self.cx.text(n)) {
            return;
        }
        // Mis-parsed remains of a macro invocation the lexical pass already accounted for.
        if self.fn_depth == 0 && self.cx.s.def_stack.is_empty() && self.lex_at(line_of(n)).is_some() {
            return;
        }
        let name = self.cx.name(n);
        if self.fn_depth > 0 && self.cx.is_local(name) {
            return;
        }
        let kind = if self.cx.s.writes.contains(&pos) { RefKind::Write } else { RefKind::Read };
        self.cx.add_ref(name, kind, line_of(n), NONE, 255);
    }

    fn type_identifier(&mut self, n: Node<'t>) {
        if self.cx.is_marked(n) {
            return;
        }
        if self.fn_depth == 0 && self.cx.s.def_stack.is_empty() && self.lex_at(line_of(n)).is_some() {
            return;
        }
        let name = self.cx.name(n);
        self.cx.add_ref(name, RefKind::Type, line_of(n), NONE, 255);
    }

    // ----- declarator helpers -----

    /// Find the function declarator inside a declarator chain. Returns the declarator, the
    /// declared name and whether the name sits behind a pointer inside the function declarator
    /// (`int (*fp)(int)` is a function pointer, not a function).
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
                K::ArrayDeclarator | K::AttributedDeclarator | K::InitDeclarator => {
                    d = child(d, t.f_declarator).or_else(|| d.named_child(0))?;
                }
                K::ParenthesizedDeclarator => {
                    d = d.named_child(0)?;
                }
                K::Identifier | K::FieldIdentifier | K::TypeIdentifier => {
                    return fdecl.map(|f| (f, d, ptr_inside));
                }
                _ => return None,
            }
        }
        None
    }

    /// The identifier a declarator declares (variable, parameter, field or typedef name).
    fn declared_ident(&self, mut d: Node<'t>) -> Option<Node<'t>> {
        let t = self.t;
        for _ in 0..16 {
            match self.k(d) {
                K::Identifier | K::FieldIdentifier | K::TypeIdentifier => return Some(d),
                K::FunctionDeclarator | K::PointerDeclarator | K::ArrayDeclarator | K::AttributedDeclarator | K::InitDeclarator => {
                    d = child(d, t.f_declarator).or_else(|| d.named_child(0))?;
                }
                K::ParenthesizedDeclarator => {
                    d = d.named_child(0)?;
                }
                _ => return None,
            }
        }
        None
    }

    /// Name of the type a type node denotes, when it has one (`struct foo` -> `foo`).
    fn type_name_of(&mut self, ty: Node<'t>) -> u32 {
        match self.k(ty) {
            K::StructSpecifier | K::UnionSpecifier | K::EnumSpecifier => child(ty, self.t.f_name).map(|n| self.cx.name(n)).unwrap_or(NONE),
            K::TypeIdentifier => self.cx.name(ty),
            K::TypeDescriptor => match child(ty, self.t.f_type) {
                Some(x) => self.type_name_of(x),
                None => NONE,
            },
            _ => NONE,
        }
    }

    fn arity(&self, fdecl: Node<'t>) -> (u8, u8) {
        let Some(params) = child(fdecl, self.t.f_parameters) else { return (0, 255) };
        let mut n = 0u32;
        let mut variadic = false;
        let mut c = params.walk();
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                match self.k(ch) {
                    K::ParameterDeclaration => {
                        let is_void = child(ch, self.t.f_declarator).is_none() && self.cx.text(ch) == b"void";
                        if !is_void {
                            n += 1;
                        }
                    }
                    K::VariadicParameter => variadic = true,
                    _ => {}
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
        }
        let n = n.min(254) as u8;
        (n, if variadic { 255 } else { n })
    }

    fn has_storage(&self, n: Node<'t>, kw: &[u8]) -> bool {
        let mut c = n.walk();
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if self.k(ch) == K::StorageClass && self.cx.text(ch) == kw {
                    return true;
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
        }
        false
    }

    fn declare_params(&mut self, fdecl: Node<'t>) {
        let Some(params) = child(fdecl, self.t.f_parameters) else { return };
        let mut c = params.walk();
        if c.goto_first_child() {
            loop {
                let ch = c.node();
                if self.k(ch) == K::ParameterDeclaration {
                    let ty = match child(ch, self.t.f_type) {
                        Some(x) => self.type_name_of(x),
                        None => NONE,
                    };
                    if let Some(id) = child(ch, self.t.f_declarator).and_then(|d| self.declared_ident(d)) {
                        let nm = self.cx.name(id);
                        self.cx.declare(nm, ty);
                        self.cx.mark(id);
                    }
                }
                if !c.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    // ----- definitions -----

    fn function_definition(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let found = child(n, t.f_declarator).and_then(|d| self.function_declarator(d));
        let Some((fdecl, name_node, _)) = found else { return self.macro_function(n, c) };
        if self.fn_depth == 0 && self.containers.is_empty() && self.cx.s.handled.len() > 2048 {
            self.cx.s.handled.clear();
            self.cx.s.writes.clear();
        }
        let sig_end = child(n, t.f_body).map(|b| b.start_byte()).unwrap_or(n.end_byte());
        let name = self.cx.name(name_node);
        let d = self.cx.def(name, DefKind::Function, n, sig_end);
        let (amin, amax) = self.arity(fdecl);
        let ty = match child(n, t.f_type) {
            Some(x) => self.type_name_of(x),
            None => NONE,
        };
        let local = self.has_storage(n, b"static") && !self.cx.is_header;
        {
            let def = self.cx.def_mut(d);
            def.arity_min = amin;
            def.arity_max = amax;
            def.type_name = ty;
            if local {
                def.flags |= dflag::LOCAL;
            }
        }
        self.cx.mark(name_node);
        self.cx.push_def(d);
        self.cx.push_scope();
        self.fn_depth += 1;
        self.declare_params(fdecl);
        self.children(c);
        self.fn_depth -= 1;
        self.cx.pop_scope();
        self.cx.pop_def();
    }

    /// `MACRO(name) { ... }` parses as a function definition whose "type" is the macro and
    /// whose declarator is the parenthesised argument list.
    fn macro_function(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let (Some(body), true) = (child(n, t.f_body), self.fn_depth == 0) else { return self.children(c) };
        let ty = child(n, t.f_type);
        let ident = child(n, t.f_declarator).and_then(|d| self.declared_ident(d));
        let d = match self.lex_at(line_of(n)).filter(|&d| d != NONE) {
            Some(d) => d,
            None => {
                let Some(id) = ident else { return self.children(c) };
                let name = self.cx.name(id);
                let d = self.cx.def(name, DefKind::Function, n, body.start_byte());
                let dm = self.cx.def_mut(d);
                dm.flags |= dflag::MACRO_DEFINED;
                dm.arity_max = 255;
                if let Some(ty) = ty.filter(|x| self.k(*x) == K::TypeIdentifier) {
                    let m = self.cx.name(ty);
                    self.cx.push_def(d);
                    self.cx.add_ref(m, RefKind::Call, line_of(ty), NONE, 255);
                    self.cx.pop_def();
                }
                d
            }
        };
        self.cx.def_mut(d).end_line = end_line_of(body);
        if let Some(ty) = ty {
            self.cx.mark(ty);
        }
        if let Some(id) = ident {
            self.cx.mark(id);
        }
        self.cx.push_def(d);
        self.cx.push_scope();
        self.fn_depth += 1;
        self.children(c);
        self.fn_depth -= 1;
        self.cx.pop_scope();
        self.cx.pop_def();
    }

    /// The variable's own name when the declarator the grammar found is an attribute macro
    /// written after it (`int counter __read_mostly;`, `struct ops t __initdata = {..}`).
    fn name_before_attribute(&self, d: Node<'t>, id: Node<'t>) -> Option<Node<'t>> {
        let text = self.cx.text(id);
        let macro_like = text.starts_with(b"__") || (text.len() > 2 && text.iter().all(|b| b.is_ascii_uppercase() || *b == b'_' || b.is_ascii_digit()));
        if !macro_like {
            return None;
        }
        let prev = d.prev_sibling()?;
        if self.k(prev) != K::Error || prev.named_child_count() != 1 {
            return None;
        }
        // The name itself, or a declarator around it (`table[] __initconst`, `*ptr __percpu`).
        let x = prev.named_child(0)?;
        let x = if self.k(x) == K::Identifier { x } else { self.declared_ident(x)? };
        (self.k(x) == K::Identifier && x.start_byte() < x.end_byte()).then_some(x)
    }

    fn declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let tname = match child(n, t.f_type) {
            Some(x) => self.type_name_of(x),
            None => NONE,
        };
        if self.fn_depth > 0 {
            let mut cc = n.walk();
            if cc.goto_first_child() {
                loop {
                    if is_field(&cc, t.f_declarator) {
                        if let Some(id) = self.declared_ident(cc.node()) {
                            let nm = self.cx.name(id);
                            self.cx.declare(nm, tname);
                            self.cx.mark(id);
                        }
                    }
                    if !cc.goto_next_sibling() {
                        break;
                    }
                }
            }
            self.init_ctx.push(tname);
            self.children(c);
            self.init_ctx.pop();
            return;
        }

        // `static DEVICE_ATTR_RW(foo);` parses as a declaration whose "type" is the macro; the
        // lexical pass has already created its symbols.
        if let Some(d) = self.lex_at(line_of(n)) {
            if let Some(ty) = child(n, t.f_type) {
                self.cx.mark(ty);
            }
            let mut cc = n.walk();
            if cc.goto_first_child() {
                loop {
                    if is_field(&cc, t.f_declarator) {
                        if let Some(id) = self.declared_ident(cc.node()) {
                            self.cx.mark(id);
                        }
                    }
                    if !cc.goto_next_sibling() {
                        break;
                    }
                }
            }
            if d != NONE {
                self.cx.push_def(d);
                self.children(c);
                self.cx.pop_def();
            }
            return;
        }

        // File scope: prototypes and global variables.
        let is_static = self.has_storage(n, b"static");
        let is_extern = self.has_storage(n, b"extern");
        let local = is_static && !self.cx.is_header;
        let mut first = NONE;
        let mut cc = n.walk();
        if cc.goto_first_child() {
            loop {
                if is_field(&cc, t.f_declarator) {
                    let d = cc.node();
                    let has_init = self.k(d) == K::InitDeclarator;
                    let fd = self.function_declarator(d);
                    let def = match fd {
                        Some((fdecl, id, false)) => {
                            let nm = self.cx.name(id);
                            let di = self.cx.def(nm, DefKind::Function, n, n.end_byte());
                            let (a, b) = self.arity(fdecl);
                            // `int f(void) __attr { ... }` parses as a declaration followed by a
                            // block: the block is this function's body.
                            let body = next_non_comment(n, self).filter(|x| self.k(*x) == K::CompoundStatement);
                            let dm = self.cx.def_mut(di);
                            dm.arity_min = a;
                            dm.arity_max = b;
                            dm.type_name = tname;
                            if local {
                                dm.flags |= dflag::LOCAL;
                            }
                            match body {
                                Some(b) => {
                                    dm.end_line = end_line_of(b);
                                    self.pending_body = Some(PendingBody { at: b.start_byte() as u32, def: di, fdecl: Some(fdecl) });
                                }
                                None => dm.flags |= dflag::DECL,
                            }
                            self.cx.mark(id);
                            di
                        }
                        other => {
                            let id = match other {
                                Some((_, id, _)) => Some(id),
                                None => self.declared_ident(d),
                            };
                            match id {
                                Some(id) => {
                                    // `struct ops table __initdata = {..}`: the grammar takes
                                    // the attribute macro for the declarator and leaves the
                                    // real name in an error node in front of it.
                                    let real = self.name_before_attribute(d, id);
                                    if let Some(r) = real {
                                        self.cx.mark(r);
                                    }
                                    let nm = self.cx.name(real.unwrap_or(id));
                                    let sig_end = if has_init { child(d, t.f_value).map(|v| v.start_byte()).unwrap_or(n.end_byte()) } else { n.end_byte() };
                                    let di = self.cx.def(nm, DefKind::Variable, n, sig_end);
                                    let dm = self.cx.def_mut(di);
                                    dm.type_name = tname;
                                    if local {
                                        dm.flags |= dflag::LOCAL;
                                    }
                                    if is_extern && !has_init {
                                        dm.flags |= dflag::DECL;
                                    }
                                    if other.is_some() {
                                        dm.flags |= dflag::FNPTR;
                                    }
                                    self.cx.mark(id);
                                    di
                                }
                                None => NONE,
                            }
                        }
                    };
                    if first == NONE {
                        first = def;
                    }
                }
                if !cc.goto_next_sibling() {
                    break;
                }
            }
        }
        if first != NONE {
            self.cx.push_def(first);
        }
        self.init_ctx.push(tname);
        self.children(c);
        self.init_ctx.pop();
        if first != NONE {
            self.cx.pop_def();
        }
    }

    fn type_definition(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let ty = child(n, t.f_type);
        let tname = match ty {
            Some(x) => self.type_name_of(x),
            None => NONE,
        };
        let body = ty.and_then(|x| child(x, t.f_body));
        let anon_body = body.is_some() && ty.is_some_and(|x| child(x, t.f_name).is_none());
        let sig_end = body.map(|b| b.start_byte()).unwrap_or(n.end_byte());
        let mut first = NONE;
        let mut cc = n.walk();
        if cc.goto_first_child() {
            loop {
                if is_field(&cc, t.f_declarator) {
                    if let Some(id) = self.declared_ident(cc.node()) {
                        let nm = self.cx.name(id);
                        let di = self.cx.def(nm, DefKind::TypeAlias, n, sig_end);
                        self.cx.def_mut(di).type_name = tname;
                        self.cx.mark(id);
                        if first == NONE {
                            first = di;
                        }
                    }
                }
                if !cc.goto_next_sibling() {
                    break;
                }
            }
        }
        if first != NONE {
            self.cx.push_def(first);
            if anon_body {
                self.containers.push(first);
            }
        }
        self.children(c);
        if first != NONE {
            if anon_body {
                self.containers.pop();
            }
            self.cx.pop_def();
        }
    }

    fn record_specifier(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>, kind: DefKind) {
        let t = self.t;
        let name_node = child(n, t.f_name);
        let body = child(n, t.f_body);
        match (name_node, body) {
            (Some(nm), Some(b)) => {
                let name = self.cx.name(nm);
                let d = self.cx.def(name, kind, n, b.start_byte());
                self.cx.mark(nm);
                self.containers.push(d);
                self.cx.push_def(d);
                self.children(c);
                self.cx.pop_def();
                self.containers.pop();
            }
            // Anonymous record: its members belong to whatever encloses it.
            (None, Some(_)) => self.children(c),
            (Some(nm), None) => {
                if !self.cx.is_marked(nm) {
                    let name = self.cx.name(nm);
                    self.cx.add_ref(name, RefKind::Type, line_of(nm), NONE, 255);
                }
            }
            (None, None) => {}
        }
    }

    fn field_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let tname = match child(n, t.f_type) {
            Some(x) => self.type_name_of(x),
            None => NONE,
        };
        let parent = *self.containers.last().unwrap_or(&NONE);
        let mut cc = n.walk();
        if cc.goto_first_child() {
            loop {
                if is_field(&cc, t.f_declarator) {
                    let d = cc.node();
                    let fd = self.function_declarator(d);
                    let id = match fd {
                        Some((_, id, _)) => Some(id),
                        None => self.declared_ident(d),
                    };
                    if let Some(id) = id {
                        let nm = self.cx.name(id);
                        let di = self.cx.def(nm, DefKind::Field, n, n.end_byte());
                        let ar = fd.map(|(f, _, _)| self.arity(f));
                        let dm = self.cx.def_mut(di);
                        dm.parent = parent;
                        dm.type_name = tname;
                        if let Some((a, b)) = ar {
                            dm.flags |= dflag::FNPTR;
                            dm.arity_min = a;
                            dm.arity_max = b;
                        }
                        self.cx.mark(id);
                    }
                }
                if !cc.goto_next_sibling() {
                    break;
                }
            }
        }
        self.children(c);
    }

    fn enumerator(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        if let Some(nm) = child(n, self.t.f_name) {
            let name = self.cx.name(nm);
            let d = self.cx.def(name, DefKind::EnumMember, n, n.end_byte());
            self.cx.def_mut(d).parent = *self.containers.last().unwrap_or(&NONE);
            self.cx.mark(nm);
        }
        self.children(c);
    }

    fn parameter_declaration(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        if let Some(id) = child(n, self.t.f_declarator).and_then(|d| self.declared_ident(d)) {
            self.cx.mark(id);
        }
        self.children(c);
    }

    // ----- preprocessor -----

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

    fn preproc_def(&mut self, n: Node<'t>, function_like: bool) {
        let t = self.t;
        let Some(nm_node) = child(n, t.f_name) else { return };
        let value = child(n, t.f_value);
        if value.is_none() && !function_like && is_header_guard(self.cx.text(nm_node)) {
            return;
        }
        let name = self.cx.name(nm_node);
        let sig_end = n.end_byte().min(n.start_byte() + 200);
        let d = self.cx.def(name, DefKind::Macro, n, sig_end);
        self.pp_params.clear();
        if function_like {
            let mut variadic = false;
            if let Some(ps) = child(n, t.f_parameters) {
                let mut pc = ps.walk();
                if pc.goto_first_child() {
                    loop {
                        let ch = pc.node();
                        if self.k(ch) == K::Identifier {
                            let id = self.cx.name(ch);
                            self.pp_params.push(id);
                        } else if self.cx.text(ch) == b"..." {
                            variadic = true;
                        }
                        if !pc.goto_next_sibling() {
                            break;
                        }
                    }
                }
            }
            let a = self.pp_params.len().min(254) as u8;
            let dm = self.cx.def_mut(d);
            dm.arity_min = a;
            dm.arity_max = if variadic { 255 } else { a };
        } else {
            // Object-like macros are not callable: arity 0/0 would wrongly match `FOO()`.
            self.cx.def_mut(d).arity_max = 0;
        }
        if let Some(v) = value {
            self.cx.push_def(d);
            self.scan_macro_body(v);
            self.cx.pop_def();
        }
    }

    /// Macro bodies are opaque text to the grammar. A lexical pass recovers the names they use,
    /// so that `#define kmalloc(...) alloc_hooks(kmalloc_noprof(__VA_ARGS__))` links `kmalloc`
    /// to `kmalloc_noprof`.
    fn scan_macro_body(&mut self, v: Node<'t>) {
        let text = self.cx.text(v);
        let n = text.len();
        let mut line = line_of(v);
        let mut i = 0usize;
        let mut prev: u8 = 0;
        let mut prev2: u8 = 0;
        let mut prev_word: &[u8] = b"";
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
                _ if b == b'_' || b.is_ascii_alphabetic() => {
                    let s = i;
                    while i < n && (text[i] == b'_' || text[i].is_ascii_alphanumeric()) {
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
                    if !pasted && !stringified && !is_c_keyword(word) {
                        let id = self.cx.intern(word);
                        if matches!(prev_word, b"struct" | b"union" | b"enum") {
                            self.cx.add_ref(id, RefKind::Type, line, NONE, 255);
                        } else if member {
                            let k = if next == b'(' { RefKind::MemberCall } else { RefKind::MemberRead };
                            self.cx.add_ref(id, k, line, NONE, 255);
                        } else if !self.pp_params.contains(&id) {
                            let k = if next == b'(' { RefKind::Call } else { RefKind::Read };
                            self.cx.add_ref(id, k, line, NONE, 255);
                        }
                    }
                    prev_word = word;
                    prev2 = prev;
                    prev = b'a';
                }
                _ if b.is_ascii_digit() => {
                    while i < n && (text[i].is_ascii_alphanumeric() || text[i] == b'.') {
                        i += 1;
                    }
                    prev_word = b"";
                    prev2 = prev;
                    prev = b'0';
                }
                _ => {
                    prev2 = prev;
                    prev = b;
                    prev_word = b"";
                    i += 1;
                }
            }
        }
    }

    // ----- expressions -----

    /// Non-local names handed to a call as arguments are recorded with the callee's name:
    /// for functions this is how callbacks are registered (`request_irq(irq, handler, ...)`).
    fn arg_refs(&mut self, callee: u32, args: Option<Node<'t>>) {
        let Some(a) = args else { return };
        let mut ac = a.walk();
        if !ac.goto_first_child() {
            return;
        }
        loop {
            let mut e = ac.node();
            if self.k(e) == K::PointerExpression {
                if let Some(x) = child(e, self.t.f_argument) {
                    e = x;
                }
            }
            if self.k(e) == K::Identifier && !self.cx.is_marked(e) && !is_c_keyword(self.cx.text(e)) {
                let name = self.cx.name(e);
                if !(self.fn_depth > 0 && self.cx.is_local(name)) {
                    self.cx.add_ref(name, RefKind::Arg, line_of(e), callee, 255);
                    self.cx.mark(e);
                }
            }
            if !ac.goto_next_sibling() {
                break;
            }
        }
    }

    /// Macros and helpers that store into their first argument: `WRITE_ONCE(p->on_rq, 1)`.
    fn store_target(&mut self, callee: &[u8], args: Option<Node<'t>>) {
        if !is_store_call(callee) {
            return;
        }
        let Some(first) = args.and_then(|a| a.named_child(0)) else { return };
        let mut e = first;
        for _ in 0..4 {
            match self.k(e) {
                K::PointerExpression | K::ParenthesizedExpression => match child(e, self.t.f_argument).or_else(|| e.named_child(0)) {
                    Some(x) => e = x,
                    None => break,
                },
                _ => break,
            }
        }
        let target = self.lvalue_target(e);
        match self.k(target) {
            K::Identifier => {
                self.cx.s.writes.insert(target.start_byte() as u32);
            }
            K::FieldExpression => {
                if let Some(fl) = child(target, self.t.f_field) {
                    self.cx.s.writes.insert(fl.start_byte() as u32);
                }
            }
            _ => {}
        }
    }

    /// Linux security-module hooks are registered and dispatched through macros that name the
    /// hook: `LSM_HOOK_INIT(file_permission, fn)` and `call_int_hook(file_permission, ...)`.
    fn lsm_hook(&mut self, callee: &[u8], call: Node<'t>) -> bool {
        let dispatch = matches!(callee, b"call_int_hook" | b"call_void_hook" | b"call_int_hook_idx");
        if callee != b"LSM_HOOK_INIT" && !dispatch {
            return false;
        }
        let (args, _) = macro_args(self.cx.text(call));
        let Some(&hook) = args.first() else { return false };
        if !is_plain_ident(hook) {
            return false;
        }
        let line = line_of(call);
        if dispatch {
            let name = self.cx.intern(hook);
            let owner = self.cx.intern(b"%LSM");
            self.cx.add_ref(name, RefKind::MemberCall, line, owner, 255);
            return true;
        }
        let Some(&func) = args.get(1) else { return false };
        if !is_plain_ident(func) {
            return false;
        }
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        buf.extend_from_slice(b"%LSM.");
        buf.extend_from_slice(hook);
        let slot = self.cx.intern(&buf);
        self.cx.s.buf = buf;
        let f = self.cx.intern(func);
        self.cx.add_ref(f, RefKind::Bind, line, slot, 255);
        true
    }

    fn call_expression(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        let nargs = match child(n, t.f_arguments) {
            Some(a) => a.named_child_count().min(254) as u8,
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
                    let callee = self.cx.text(f);
                    self.store_target(callee, child(n, t.f_arguments));
                    if self.lsm_hook(callee, n) {
                        // Arguments naming the hook and the function are accounted for.
                        self.mark_ident_args(child(n, t.f_arguments), false);
                    }
                    let name = self.cx.name(f);
                    let seen = self.cx.is_marked(f);
                    self.cx.mark(f);
                    if !seen && !(self.fn_depth > 0 && self.cx.is_local(name)) && !is_c_keyword(self.cx.text(f)) {
                        self.cx.add_ref(name, RefKind::Call, line_of(f), NONE, nargs);
                    }
                    self.arg_refs(name, child(n, t.f_arguments));
                }
                K::FieldExpression => {
                    if let Some(fl) = child(f, t.f_field) {
                        let q = match child(f, t.f_argument) {
                            Some(a) => self.qual_of(a),
                            None => NONE,
                        };
                        let name = self.cx.name(fl);
                        self.cx.add_ref(name, RefKind::MemberCall, line_of(fl), q, nargs);
                        self.cx.mark(fl);
                    }
                }
                _ => {}
            }
        }
        self.children(c);
    }

    fn field_expression(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        if let Some(fl) = child(n, t.f_field) {
            let pos = fl.start_byte() as u32;
            if !self.cx.s.handled.contains(&pos) {
                let q = match child(n, t.f_argument) {
                    Some(a) => self.qual_of(a),
                    None => NONE,
                };
                let kind = if self.cx.s.writes.contains(&pos) { RefKind::MemberWrite } else { RefKind::MemberRead };
                let name = self.cx.name(fl);
                self.cx.add_ref(name, kind, line_of(fl), q, 255);
            }
        }
        self.children(c);
    }

    /// Type path of an expression used as a receiver, interned; `NONE` when unknown.
    fn qual_of(&mut self, e: Node<'t>) -> u32 {
        let mut buf = std::mem::take(&mut self.cx.s.buf);
        buf.clear();
        let ok = self.qual_path(e, &mut buf, 0);
        let q = if ok && !buf.is_empty() { self.cx.intern(&buf) } else { NONE };
        self.cx.s.buf = buf;
        q
    }

    fn qual_path(&mut self, e: Node<'t>, buf: &mut Vec<u8>, depth: u32) -> bool {
        if depth > 4 {
            return false;
        }
        let t = self.t;
        match self.k(e) {
            K::Identifier => {
                let name = self.cx.name(e);
                if self.fn_depth > 0 {
                    if let Some(ty) = self.cx.local_type(name) {
                        if ty == NONE {
                            return false;
                        }
                        buf.extend_from_slice(self.cx.name_bytes(ty));
                        return true;
                    }
                }
                buf.push(b'$');
                buf.extend_from_slice(self.cx.text(e));
                true
            }
            K::FieldExpression => {
                let (Some(arg), Some(fl)) = (child(e, t.f_argument), child(e, t.f_field)) else { return false };
                if !self.qual_path(arg, buf, depth + 1) {
                    return false;
                }
                buf.extend_from_slice(b"::");
                buf.extend_from_slice(self.cx.text(fl));
                true
            }
            K::ParenthesizedExpression => match e.named_child(0) {
                Some(x) => self.qual_path(x, buf, depth + 1),
                None => false,
            },
            K::PointerExpression | K::SubscriptExpression => match child(e, t.f_argument) {
                Some(x) => self.qual_path(x, buf, depth + 1),
                None => false,
            },
            K::CastExpression => {
                let ty = match child(e, t.f_type) {
                    Some(x) => self.type_name_of(x),
                    None => NONE,
                };
                if ty == NONE {
                    return false;
                }
                buf.extend_from_slice(self.cx.name_bytes(ty));
                true
            }
            K::CallExpression => match child(e, t.f_function) {
                Some(f) if self.k(f) == K::Identifier => {
                    buf.extend_from_slice(self.cx.text(f));
                    buf.extend_from_slice(b"()");
                    true
                }
                _ => false,
            },
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

    /// A value that can be bound into a slot: a non-local identifier, possibly behind `&`.
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
        if self.k(r) != K::Identifier {
            return None;
        }
        let name = self.cx.name(r);
        if self.fn_depth > 0 && self.cx.is_local(name) {
            return None;
        }
        Some(r)
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

    fn assignment(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let t = self.t;
        if let Some(l) = child(n, t.f_left) {
            let target = self.lvalue_target(l);
            match self.k(target) {
                K::Identifier => {
                    self.cx.s.writes.insert(target.start_byte() as u32);
                }
                K::FieldExpression => {
                    if let Some(fl) = child(target, t.f_field) {
                        self.cx.s.writes.insert(fl.start_byte() as u32);
                        let plain = child(n, t.f_operator).is_none_or(|op| self.cx.text(op) == b"=");
                        if plain {
                            if let Some(v) = child(n, t.f_right).and_then(|r| self.bindable(r)) {
                                let q = match child(target, t.f_argument) {
                                    Some(a) => self.qual_of(a),
                                    None => NONE,
                                };
                                let slot = self.slot(q, self.cx.text(fl));
                                let vn = self.cx.name(v);
                                self.cx.add_ref(vn, RefKind::Bind, line_of(v), slot, 255);
                                self.cx.mark(v);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        self.children(c);
    }

    fn update(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        if let Some(a) = child(n, self.t.f_argument) {
            let target = self.lvalue_target(a);
            match self.k(target) {
                K::Identifier => {
                    self.cx.s.writes.insert(target.start_byte() as u32);
                }
                K::FieldExpression => {
                    if let Some(fl) = child(target, self.t.f_field) {
                        self.cx.s.writes.insert(fl.start_byte() as u32);
                    }
                }
                _ => {}
            }
        }
        self.children(c);
    }

    fn compound_literal(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let ty = match child(n, self.t.f_type) {
            Some(x) => self.type_name_of(x),
            None => NONE,
        };
        self.init_ctx.push(ty);
        self.children(c);
        self.init_ctx.pop();
    }

    /// `{ .field = value, ... }`: record member writes with the owning type, and bind function
    /// names to their slots. The cursor is on the initializer_list.
    fn initializer_list(&mut self, c: &mut TreeCursor<'t>) {
        if self.cx.depth >= MAX_DEPTH {
            return;
        }
        self.cx.depth += 1;
        let ctx = *self.init_ctx.last().unwrap_or(&NONE);
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
                if self.k(ch) == K::FieldDesignator {
                    if designators == 1 {
                        if let Some(fl) = ch.named_child(0) {
                            let name = self.cx.name(fl);
                            self.cx.add_ref(name, RefKind::MemberWrite, line_of(fl), ctx, 255);
                            field_name = Some(self.cx.text(fl));
                        }
                    }
                } else {
                    // `[IDX] = ...`: the index expression may reference constants.
                    self.visit(c);
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
                        self.init_ctx.push(nested);
                        self.initializer_list(c);
                        self.init_ctx.pop();
                    }
                    (K::InitializerList, _) => self.initializer_list(c),
                    (_, Some(f)) if designators == 1 => match self.bindable(ch) {
                        Some(v) => {
                            let slot = self.slot(ctx, f);
                            let vn = self.cx.name(v);
                            self.cx.add_ref(vn, RefKind::Bind, line_of(v), slot, 255);
                            self.cx.mark(v);
                            // Nothing else of interest inside `&ident` / `(cast)ident`.
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
        if self.fn_depth > 0 || !self.cx.s.def_stack.is_empty() {
            return self.children(c);
        }
        let t = self.t;
        let Some(call) = n.named_child(0).filter(|x| self.k(*x) == K::CallExpression) else {
            return self.children(c);
        };
        let Some(func) = child(call, t.f_function).filter(|f| self.k(*f) == K::Identifier) else {
            return self.children(c);
        };
        let mname = self.cx.text(func);
        let args = child(call, t.f_arguments);

        // `MACRO(name, ...) { body }` defines a function.
        if let Some(body) = next_non_comment(n, self).filter(|x| self.k(*x) == K::CompoundStatement) {
            let mut first_id: Option<Node<'t>> = None;
            self.macro_params.clear();
            if let Some(a) = args {
                let mut ac = a.walk();
                if ac.goto_first_child() {
                    loop {
                        let ch = ac.node();
                        if self.k(ch) == K::Identifier {
                            if first_id.is_none() {
                                first_id = Some(ch);
                            } else {
                                let id = self.cx.name(ch);
                                self.macro_params.push(id);
                            }
                        }
                        if !ac.goto_next_sibling() {
                            break;
                        }
                    }
                }
            }
            // A known symbol-defining macro was already named by the lexical pass; otherwise the
            // function is named after the first identifier argument.
            if let Some(d) = self.lex_at(line_of(n)).filter(|&d| d != NONE) {
                let end = end_line_of(body);
                self.cx.def_mut(d).end_line = end;
                self.pending_body = Some(PendingBody { at: body.start_byte() as u32, def: d, fdecl: None });
                return;
            }
            let d = {
                let name_node = first_id.unwrap_or(func);
                let name = self.cx.name(name_node);
                self.cx.add_def(Def {
                    name,
                    parent: NONE,
                    qual: NONE,
                    line: line_of(n),
                    end_line: end_line_of(body),
                    sig_off: n.start_byte() as u32,
                    sig_len: (n.end_byte() - n.start_byte()).min(u16::MAX as usize) as u16,
                    kind: DefKind::Function,
                    flags: dflag::MACRO_DEFINED,
                    type_name: NONE,
                    arity_min: 0,
                    arity_max: 255,
                })
            };
            let m = self.cx.name(func);
            self.cx.push_def(d);
            self.cx.add_ref(m, RefKind::Call, line_of(func), NONE, 255);
            self.cx.pop_def();
            self.pending_body = Some(PendingBody { at: body.start_byte() as u32, def: d, fdecl: None });
            return;
        }

        // Registrations and symbol-defining macros were handled by the lexical pass: keep the
        // argument expressions as references of the symbol defined here, nothing else.
        if let Some(d) = self.lex_at(line_of(n)) {
            self.cx.mark(func);
            self.mark_ident_args(args, d != NONE);
            if d != NONE {
                self.cx.push_def(d);
                self.children(c);
                self.cx.pop_def();
            }
            return;
        }

        if mname.starts_with(b"EXPORT_SYMBOL") {
            if let Some(id) = args.and_then(|a| a.named_child(0)).filter(|x| self.k(*x) == K::Identifier) {
                let name = self.cx.name(id);
                self.exports.push(name);
            }
            return;
        }

        // `DEFINE_MUTEX(name);`, `LIST_HEAD(name);`, `DEFINE_PER_CPU(type, name);`
        if is_define_macro(mname) {
            if let Some(a) = args {
                let want_last = contains(mname, b"PER_CPU");
                let mut pick: Option<Node<'t>> = None;
                let mut ac = a.walk();
                if ac.goto_first_child() {
                    loop {
                        let ch = ac.node();
                        if self.k(ch) == K::Identifier && (pick.is_none() || want_last) {
                            pick = Some(ch);
                        }
                        if !ac.goto_next_sibling() {
                            break;
                        }
                    }
                }
                if let Some(id) = pick {
                    let name = self.cx.name(id);
                    let d = self.cx.def(name, DefKind::Variable, n, n.end_byte());
                    self.cx.def_mut(d).flags |= dflag::MACRO_DEFINED;
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

    /// Symbol-defining and registering macros at the start of a line, recognised lexically.
    /// The grammar parses these invocations in many shapes (and as errors when the trailing
    /// semicolon is omitted, which is common), so the syntax tree is not consulted here.
    fn lexical_macros(&mut self) {
        let src = self.cx.src;
        let mut line = 0u32;
        let mut pos = 0usize;
        while pos < src.len() {
            line += 1;
            let eol = memchr::memchr(b'\n', &src[pos..]).map(|i| pos + i).unwrap_or(src.len());
            let l = &src[pos..eol];
            let start = pos;
            pos = eol + 1;
            if l.len() < 4 || !(l[0] == b'_' || l[0].is_ascii_alphabetic()) {
                continue;
            }
            let mut i = 0usize;
            if l.starts_with(b"static ") {
                i = 7;
                while i < l.len() && l[i] == b' ' {
                    i += 1;
                }
            }
            let s0 = i;
            while i < l.len() && (l[i] == b'_' || l[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            if i == s0 || i >= l.len() || l[i] != b'(' {
                continue;
            }
            let m = &l[s0..i];
            let entry = entry_macro(m);
            let rule = find_rule(m);
            if entry.is_none() && rule.is_none() {
                continue;
            }
            let from = start + s0;
            let text = &src[from..(from + 4096).min(src.len())];
            let (args, end) = macro_args(text);
            if let Some(argi) = entry {
                if let Some(&a) = args.get(argi) {
                    if is_plain_ident(a) {
                        let mut buf = std::mem::take(&mut self.cx.s.buf);
                        buf.clear();
                        buf.push(b'@');
                        buf.extend_from_slice(m);
                        let slot = self.cx.intern(&buf);
                        self.cx.s.buf = buf;
                        let target = self.cx.intern(a);
                        let mid = self.cx.intern(m);
                        self.cx.add_ref(mid, RefKind::Call, line, NONE, 255);
                        self.cx.add_ref(target, RefKind::Bind, line, slot, 255);
                        self.lex.push((line, NONE));
                    }
                }
                continue;
            }
            let Some(rule) = rule else { continue };
            let nl = memchr::memchr_iter(b'\n', &text[..end.min(text.len())]).count() as u32;
            let mut first = NONE;
            for made in rule.made {
                let Some(&a) = args.get(made.a as usize) else { continue };
                if !is_plain_ident(a) {
                    continue;
                }
                let mut buf = std::mem::take(&mut self.cx.s.buf);
                buf.clear();
                buf.extend_from_slice(made.prefix.as_bytes());
                buf.extend_from_slice(a);
                let mut ok = true;
                if made.b != 255 {
                    match args.get(made.b as usize) {
                        Some(&b) if is_plain_ident(b) => buf.extend_from_slice(b),
                        _ => ok = false,
                    }
                }
                buf.extend_from_slice(made.suffix.as_bytes());
                let name = if ok { self.cx.intern(&buf) } else { NONE };
                self.cx.s.buf = buf;
                if !ok {
                    continue;
                }
                let d = self.cx.add_def(Def {
                    name,
                    parent: NONE,
                    qual: NONE,
                    line,
                    end_line: line + nl,
                    sig_off: start as u32,
                    sig_len: (s0 + end).min(240) as u16,
                    kind: made.kind,
                    flags: dflag::MACRO_DEFINED
                        | if l.starts_with(b"static ") && !self.cx.is_header { dflag::LOCAL } else { 0 }
                        | if m.starts_with(b"DECLARE_")
                            && made.kind == DefKind::Variable
                            && m != b"DECLARE_BITMAP"
                            && !m.starts_with(b"DECLARE_WORK")
                            && !m.starts_with(b"DECLARE_DELAYED_WORK")
                            && !m.starts_with(b"DECLARE_TASKLET")
                            && !m.starts_with(b"DECLARE_WAIT_QUEUE_HEAD")
                            && !m.starts_with(b"DECLARE_COMPLETION")
                            && !m.starts_with(b"DECLARE_RWSEM")
                            && !m.starts_with(b"DECLARE_FAULT_ATTR")
                            && !m.starts_with(b"DECLARE_HASHTABLE")
                        {
                            dflag::DECL
                        } else {
                            0
                        },
                    type_name: NONE,
                    arity_min: 0,
                    arity_max: if made.kind == DefKind::Function { 255 } else { 0 },
                });
                if first == NONE {
                    first = d;
                }
            }
            if first != NONE {
                let mid = self.cx.intern(m);
                self.cx.push_def(first);
                self.cx.add_ref(mid, RefKind::Call, line, NONE, 255);
                self.cx.pop_def();
                self.lex.push((line, first));
            }
        }
    }

    /// The lexical pass's result for `line`: `Some(def)` (possibly `NONE`) when it handled it.
    #[inline]
    fn lex_at(&self, line: u32) -> Option<u32> {
        if self.lex.is_empty() {
            return None;
        }
        self.lex.binary_search_by_key(&line, |&(l, _)| l).ok().map(|i| self.lex[i].1)
    }

    /// Mark identifier arguments so they are not also emitted as plain reads.
    fn mark_ident_args(&mut self, args: Option<Node<'t>>, only_first: bool) {
        let Some(a) = args else { return };
        let mut ac = a.walk();
        if ac.goto_first_child() {
            loop {
                let ch = ac.node();
                if self.k(ch) == K::Identifier {
                    self.cx.mark(ch);
                    if only_first {
                        break;
                    }
                }
                if !ac.goto_next_sibling() {
                    break;
                }
            }
        }
    }

    fn compound(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let pending = match &self.pending_body {
            Some(p) if self.fn_depth == 0 && p.at == n.start_byte() as u32 => self.pending_body.take(),
            _ => None,
        };
        let Some(p) = pending else { return self.children(c) };
        self.cx.push_def(p.def);
        self.cx.push_scope();
        self.fn_depth += 1;
        match p.fdecl {
            Some(fdecl) => self.declare_params(fdecl),
            None => {
                let params = std::mem::take(&mut self.macro_params);
                for &id in &params {
                    self.cx.declare(id, NONE);
                }
                self.macro_params = params;
            }
        }
        self.children(c);
        self.fn_depth -= 1;
        self.cx.pop_scope();
        self.cx.pop_def();
    }

    /// `DEFINE_PER_CPU(struct foo, name);` parses as a macro type specifier.
    fn macro_type_specifier(&mut self, n: Node<'t>, c: &mut TreeCursor<'t>) {
        let Some(nm) = child(n, self.t.f_name) else { return self.children(c) };
        let m = self.cx.name(nm);
        self.cx.mark(nm);
        if self.fn_depth == 0 && self.cx.s.def_stack.is_empty() {
            if let Some(d) = self.lex_at(line_of(n)) {
                if let Some(id) = self.last_identifier(n, nm) {
                    self.cx.mark(id);
                }
                if d != NONE {
                    self.cx.push_def(d);
                    self.children(c);
                    self.cx.pop_def();
                }
                return;
            }
        }
        if self.fn_depth == 0 && self.cx.s.def_stack.is_empty() && is_define_macro(self.cx.text(nm)) {
            if let Some(id) = self.last_identifier(n, nm) {
                let name = self.cx.name(id);
                let d = self.cx.def(name, DefKind::Variable, n, n.end_byte());
                self.cx.def_mut(d).flags |= dflag::MACRO_DEFINED;
                self.cx.mark(id);
                self.cx.push_def(d);
                self.cx.add_ref(m, RefKind::Call, line_of(nm), NONE, 255);
                self.children(c);
                self.cx.pop_def();
                return;
            }
        }
        self.cx.add_ref(m, RefKind::Call, line_of(nm), NONE, 255);
        self.children(c);
    }

    fn last_identifier(&self, n: Node<'t>, exclude: Node<'t>) -> Option<Node<'t>> {
        let mut best: Option<Node<'t>> = None;
        let mut c = n.walk();
        let mut depth = 0u32;
        'outer: loop {
            let x = c.node();
            if self.k(x) == K::Identifier && x.start_byte() != exclude.start_byte() {
                best = Some(x);
            }
            if depth < 6 && c.goto_first_child() {
                depth += 1;
                continue;
            }
            loop {
                if depth == 0 {
                    break 'outer;
                }
                if c.goto_next_sibling() {
                    break;
                }
                if !c.goto_parent() {
                    break 'outer;
                }
                depth -= 1;
            }
        }
        best
    }

    /// Annotation macros the grammar cannot place (`__init`, `__user`) surface as identifiers
    /// directly under ERROR nodes: they are noise, not references.
    fn error(&mut self, c: &mut TreeCursor<'t>) {
        self.cx.facts.parse_errors += 1;
        if self.cx.depth >= MAX_DEPTH {
            return;
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
    }

    fn finish(&mut self) {
        self.restore_order();
        if self.exports.is_empty() {
            return;
        }
        let exports = &self.exports;
        for d in self.cx.facts.defs.iter_mut() {
            if matches!(d.kind, DefKind::Function | DefKind::Variable) && exports.contains(&d.name) {
                d.flags |= dflag::EXPORTED;
            }
        }
    }
}

impl<'a, 't> W<'a, 't> {
    /// Definitions created by the lexical pass come first in the list; put everything back in
    /// source order (parents before children) and renumber the indices that refer to them.
    fn restore_order(&mut self) {
        if self.lex.iter().all(|&(_, d)| d == NONE) {
            return;
        }
        let n = self.cx.facts.defs.len();
        let mut order: Vec<u32> = (0..n as u32).collect();
        order.sort_by_key(|&i| self.cx.facts.defs[i as usize].line);
        let mut new_index = vec![0u32; n];
        for (new, &old) in order.iter().enumerate() {
            new_index[old as usize] = new as u32;
        }
        let old_defs = std::mem::take(&mut self.cx.facts.defs);
        let mut defs: Vec<Def> = Vec::with_capacity(n);
        for &old in &order {
            let mut d = old_defs[old as usize];
            if d.parent != NONE {
                d.parent = new_index[d.parent as usize];
            }
            defs.push(d);
        }
        self.cx.facts.defs = defs;
        for r in self.cx.facts.refs.iter_mut() {
            if r.encl != NONE {
                r.encl = new_index[r.encl as usize];
            }
        }
    }
}

/// Next named sibling that is not a comment.
fn next_non_comment<'a, 't>(n: Node<'t>, w: &W<'a, 't>) -> Option<Node<'t>> {
    let mut x = n.next_named_sibling()?;
    for _ in 0..8 {
        if w.k(x) != K::Comment {
            return Some(x);
        }
        x = x.next_named_sibling()?;
    }
    None
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(hay, needle).is_some()
}

// ---------------------------------------------------------------------------------------------
// Symbol-defining macros
// ---------------------------------------------------------------------------------------------
//
// Large C code bases define symbols through macros: `SYSCALL_DEFINE3(read, ...)` creates
// `sys_read`, `DEVICE_ATTR_RW(foo)` creates `dev_attr_foo`. Text search cannot find those
// names. The table below covers the Linux kernel's own list (scripts/tags.sh) plus a few
// more; macros not listed but shaped like `DEFINE_*(name)` fall back to their first argument.

/// One symbol a macro creates: `prefix + arg[a] (+ arg[b]) + suffix`.
struct Made {
    a: u8,
    b: u8,
    prefix: &'static str,
    suffix: &'static str,
    kind: DefKind,
}

const fn f(a: u8, prefix: &'static str, suffix: &'static str) -> Made {
    Made { a, b: 255, prefix, suffix, kind: DefKind::Function }
}

const fn v(a: u8, prefix: &'static str, suffix: &'static str) -> Made {
    Made { a, b: 255, prefix, suffix, kind: DefKind::Variable }
}

const fn f2(a: u8, b: u8, prefix: &'static str) -> Made {
    Made { a, b, prefix, suffix: "", kind: DefKind::Function }
}

#[derive(Clone, Copy, PartialEq)]
enum Fit {
    Exact,
    /// The rule name followed by digits (`SYSCALL_DEFINE3`).
    Digits,
    /// The rule name followed by anything (`DEFINE_IDTENTRY_RAW`).
    Prefix,
}

struct MacroRule {
    name: &'static str,
    fit: Fit,
    made: &'static [Made],
}

const fn r(name: &'static str, fit: Fit, made: &'static [Made]) -> MacroRule {
    MacroRule { name, fit, made }
}

static MACRO_RULES: &[MacroRule] = &[
    r("SYSCALL_DEFINE", Fit::Digits, &[f(0, "sys_", "")]),
    r("COMPAT_SYSCALL_DEFINE", Fit::Digits, &[f(0, "compat_sys_", "")]),
    r("BPF_CALL_", Fit::Digits, &[f(0, "", "")]),
    r("TRACE_EVENT", Fit::Exact, &[f(0, "trace_", "")]),
    r("DEFINE_EVENT", Fit::Exact, &[f(1, "trace_", "")]),
    r("DEFINE_INSN_CACHE_OPS", Fit::Exact, &[f(0, "get_", "_slot"), f(0, "free_", "_slot")]),
    r("PAGEFLAG", Fit::Exact, &[f(0, "Page", ""), f(0, "SetPage", ""), f(0, "ClearPage", "")]),
    r("TESTSETFLAG", Fit::Exact, &[f(0, "TestSetPage", "")]),
    r("TESTPAGEFLAG", Fit::Exact, &[f(0, "Page", "")]),
    r("SETPAGEFLAG", Fit::Exact, &[f(0, "SetPage", "")]),
    r("__SETPAGEFLAG", Fit::Exact, &[f(0, "__SetPage", "")]),
    r("TESTCLEARFLAG", Fit::Exact, &[f(0, "TestClearPage", "")]),
    r("__TESTCLEARFLAG", Fit::Exact, &[f(0, "TestClearPage", "")]),
    r("CLEARPAGEFLAG", Fit::Exact, &[f(0, "ClearPage", "")]),
    r("__CLEARPAGEFLAG", Fit::Exact, &[f(0, "__ClearPage", "")]),
    r("__PAGEFLAG", Fit::Exact, &[f(0, "__SetPage", ""), f(0, "__ClearPage", "")]),
    r("PAGEFLAG_FALSE", Fit::Exact, &[f(0, "Page", "")]),
    r("TESTSCFLAG", Fit::Exact, &[f(0, "TestSetPage", ""), f(0, "TestClearPage", "")]),
    r("SETPAGEFLAG_NOOP", Fit::Exact, &[f(0, "SetPage", "")]),
    r("CLEARPAGEFLAG_NOOP", Fit::Exact, &[f(0, "ClearPage", "")]),
    r("__CLEARPAGEFLAG_NOOP", Fit::Exact, &[f(0, "__ClearPage", "")]),
    r("TESTCLEARFLAG_FALSE", Fit::Exact, &[f(0, "TestClearPage", "")]),
    r("PAGE_TYPE_OPS", Fit::Exact, &[f(0, "Page", ""), f(0, "__SetPage", ""), f(0, "__ClearPage", "")]),
    r("FOLIO_FLAG", Fit::Exact, &[f(1, "folio_test_", ""), f(1, "folio_set_", ""), f(1, "folio_clear_", "")]),
    r("FOLIO_TEST_FLAG", Fit::Exact, &[f(1, "folio_test_", "")]),
    r("FOLIO_SET_FLAG", Fit::Exact, &[f(1, "folio_set_", "")]),
    r("FOLIO_CLEAR_FLAG", Fit::Exact, &[f(1, "folio_clear_", "")]),
    r("TASK_PFA_TEST", Fit::Exact, &[f(1, "task_", "")]),
    r("TASK_PFA_SET", Fit::Exact, &[f(1, "task_set_", "")]),
    r("TASK_PFA_CLEAR", Fit::Exact, &[f(1, "task_clear_", "")]),
    r("DEF_MMIO_IN_", Fit::Prefix, &[f(0, "", "")]),
    r("DEF_MMIO_OUT_", Fit::Prefix, &[f(0, "", "")]),
    r("DEBUGGER_BOILERPLATE", Fit::Exact, &[f(0, "", "")]),
    r("DEF_PCI_AC_RET", Fit::Exact, &[f(0, "", "")]),
    r("DEF_PCI_AC_NORET", Fit::Exact, &[f(0, "", "")]),
    r("PCI_OP_READ", Fit::Exact, &[f(0, "pci_bus_read_config_", "")]),
    r("PCI_OP_WRITE", Fit::Exact, &[f(0, "pci_bus_write_config_", "")]),
    r("DEFINE_PER_CPU", Fit::Prefix, &[v(1, "", "")]),
    r("DECLARE_PER_CPU", Fit::Prefix, &[v(1, "", "")]),
    r("DEVICE_ATTR_RW", Fit::Exact, &[v(0, "dev_attr_", "")]),
    r("DEVICE_ATTR_RO", Fit::Exact, &[v(0, "dev_attr_", "")]),
    r("DEVICE_ATTR_WO", Fit::Exact, &[v(0, "dev_attr_", "")]),
    r("DEVICE_ATTR_ADMIN_RW", Fit::Exact, &[v(0, "dev_attr_", "")]),
    r("DEVICE_ATTR_ADMIN_RO", Fit::Exact, &[v(0, "dev_attr_", "")]),
    r("DEVICE_ATTR", Fit::Exact, &[v(0, "dev_attr_", "")]),
    r("DRIVER_ATTR_RW", Fit::Exact, &[v(0, "driver_attr_", "")]),
    r("DRIVER_ATTR_RO", Fit::Exact, &[v(0, "driver_attr_", "")]),
    r("DRIVER_ATTR_WO", Fit::Exact, &[v(0, "driver_attr_", "")]),
    r("BUS_ATTR_RW", Fit::Exact, &[v(0, "bus_attr_", "")]),
    r("BUS_ATTR_RO", Fit::Exact, &[v(0, "bus_attr_", "")]),
    r("BUS_ATTR_WO", Fit::Exact, &[v(0, "bus_attr_", "")]),
    r("CLASS_ATTR_RW", Fit::Exact, &[v(0, "class_attr_", "")]),
    r("CLASS_ATTR_RO", Fit::Exact, &[v(0, "class_attr_", "")]),
    r("CLASS_ATTR_WO", Fit::Exact, &[v(0, "class_attr_", "")]),
    r("SEQCOUNT_LOCKTYPE", Fit::Exact, &[Made { a: 1, b: 255, prefix: "seqcount_", suffix: "_t", kind: DefKind::TypeAlias }, f(1, "seqcount_", "_init")]),
    r("DECLARE_IDTENTRY", Fit::Prefix, &[f(1, "", "")]),
    r("DEFINE_IDTENTRY", Fit::Prefix, &[f(0, "", "")]),
    r("DEFINE_FREE", Fit::Exact, &[f(0, "cleanup_", "")]),
    r("DEFINE_CLASS", Fit::Exact, &[f(0, "class_", "")]),
    r("EXTEND_CLASS", Fit::Exact, &[f2(0, 1, "class_")]),
    r("DEFINE_GUARD_COND", Fit::Exact, &[f2(0, 1, "class_")]),
    r("DEFINE_GUARD", Fit::Exact, &[f(0, "class_", "")]),
    r("DEFINE_LOCK_GUARD_", Fit::Prefix, &[f(0, "class_", "")]),
    r("DEFINE_SHOW_ATTRIBUTE", Fit::Exact, &[v(0, "", "_fops"), f(0, "", "_open")]),
    r("DEFINE_SHOW_STORE_ATTRIBUTE", Fit::Exact, &[v(0, "", "_fops"), f(0, "", "_open")]),
    r("DEFINE_PROC_SHOW_ATTRIBUTE", Fit::Exact, &[v(0, "", "_proc_ops"), f(0, "", "_open")]),
    r("DEFINE_SEQ_ATTRIBUTE", Fit::Exact, &[v(0, "", "_fops"), f(0, "", "_open")]),
    r("DEFINE_STATIC_CALL", Fit::Prefix, &[v(0, "", "")]),
    r("module_param_named", Fit::Exact, &[v(0, "", "")]),
    r("module_param", Fit::Exact, &[v(0, "", "")]),
];

/// Macros that register a function to be run by the system: `module_init(fn)`. The argument at
/// the given index is bound to an `@macro` entry slot, which answers "who calls this".
fn entry_macro(m: &[u8]) -> Option<usize> {
    if matches!(m, b"module_init" | b"module_exit" | b"__initcall" | b"__exitcall" | b"late_initcall_sync")
        || m.ends_with(b"_initcall")
        || m.ends_with(b"_initcall_sync")
    {
        return Some(0);
    }
    if matches!(m, b"__setup" | b"early_param" | b"__setup_param") {
        return Some(1);
    }
    if (m.starts_with(b"module_") || m.starts_with(b"builtin_")) && m.ends_with(b"_driver") {
        return Some(0);
    }
    None
}

fn find_rule(m: &[u8]) -> Option<&'static MacroRule> {
    let mut best: Option<&'static MacroRule> = None;
    for rule in MACRO_RULES {
        let rn = rule.name.as_bytes();
        let ok = match rule.fit {
            Fit::Exact => m == rn,
            Fit::Digits => m.len() > rn.len() && m.starts_with(rn) && m[rn.len()..].iter().all(|b| b.is_ascii_digit()),
            Fit::Prefix => m.starts_with(rn),
        };
        // The longest matching rule name wins (`DEFINE_GUARD_COND` over `DEFINE_GUARD`).
        if ok && best.is_none_or(|b| rn.len() > b.name.len()) {
            best = Some(rule);
        }
    }
    best
}

/// Top-level arguments of the first parenthesised list in `text`, trimmed, and the offset
/// just past its closing parenthesis.
fn macro_args(text: &[u8]) -> (Vec<&[u8]>, usize) {
    let mut out = Vec::new();
    let Some(open) = memchr::memchr(b'(', text) else { return (out, 0) };
    let mut depth = 0i32;
    let mut start = open + 1;
    for (i, &b) in text.iter().enumerate().skip(open) {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    out.push(text[start..i].trim_ascii());
                    return (out, i + 1);
                }
            }
            b',' if depth == 1 => {
                out.push(text[start..i].trim_ascii());
                start = i + 1;
            }
            _ => {}
        }
    }
    (out, text.len())
}

fn is_store_call(m: &[u8]) -> bool {
    matches!(
        m,
        b"WRITE_ONCE"
            | b"rcu_assign_pointer"
            | b"RCU_INIT_POINTER"
            | b"rcu_replace_pointer"
            | b"smp_store_release"
            | b"smp_store_mb"
            | b"xchg"
            | b"cmpxchg"
            | b"try_cmpxchg"
            | b"atomic_set"
            | b"atomic_inc"
            | b"atomic_dec"
            | b"atomic_add"
            | b"atomic_sub"
            | b"atomic64_set"
            | b"atomic_long_set"
            | b"refcount_set"
            | b"swap"
    )
}

fn is_plain_ident(s: &[u8]) -> bool {
    !s.is_empty() && !s[0].is_ascii_digit() && s.iter().all(|&b| b == b'_' || b.is_ascii_alphanumeric())
}

fn is_define_macro(m: &[u8]) -> bool {
    m.starts_with(b"DEFINE_") || m.starts_with(b"DECLARE_") || m.ends_with(b"_HEAD") && m.iter().all(|b| !b.is_ascii_lowercase())
}

fn is_header_guard(name: &[u8]) -> bool {
    name.ends_with(b"_H") || name.ends_with(b"_H_") || name.ends_with(b"_H__") || name.ends_with(b"_HPP") || name.ends_with(b"_INCLUDED")
}

fn is_c_keyword(w: &[u8]) -> bool {
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
    )
}
