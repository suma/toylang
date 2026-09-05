use std::rc::Rc;
use std::collections::HashSet;
use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::source_map::SourceMap;
use crate::type_decl::TypeDecl;
use crate::token::Kind;
use crate::parser::error::{ParserError, ParserResult, MultipleParserResult};
use super::core::Parser;

/// Map a primitive-type token to the canonical string it should be
/// interned as for `impl Trait for <PrimitiveType>` blocks. Returns
/// `None` for non-type tokens. The returned name is the same string
/// the type-checker resolves primitive-method receivers to, so the
/// `Stmt::ImplBlock { target_type }` symbol round-trips through the
/// existing `DefaultSymbol`-keyed method registry without any new
/// indirection. (Step A of the extension-trait work — full
/// primitive method dispatch lands in Step B.)
/// NUM-W-ENUMERATION: the one projection that cannot be derived. It
/// starts from `Kind`, which has ~100 variants, so an exhaustive match
/// is not the guard here — `primitive_target_coverage` in the frontend
/// tests is: it asserts every name in
/// `TypeDecl::PRIMITIVE_IMPL_TARGETS` is produced by some `Kind`, so a
/// width added to the canonical list and not here fails loudly.
fn primitive_type_canonical_name(kind: &Kind) -> Option<&'static str> {
    Some(match kind {
        Kind::Bool => "bool",
        Kind::U64 => "u64",
        Kind::I64 => "i64",
        Kind::F64 => "f64",
        Kind::F32 => "f32",
        Kind::USize => "usize",
        Kind::Str => "str",
        Kind::Ptr => "ptr",
        // NUM-W: narrow primitive impl targets (`impl Hash for u8 { ... }`).
        Kind::U8 => "u8",
        Kind::U16 => "u16",
        Kind::U32 => "u32",
        Kind::I8 => "i8",
        Kind::I16 => "i16",
        Kind::I32 => "i32",
        _ => return None,
    })
}

/// The top-level declarations one `parse_program` run accumulates, plus
/// the source span they cover.
///
/// These were five locals threaded through a 770-line loop, two of them
/// reachable only through closures -- `update_start_pos` /
/// `update_end_pos` captured `start_pos` / `end_pos` mutably, which is
/// what kept the loop body from being lifted out at all.
struct TopLevel {
    functions: Vec<Rc<Function>>,
    consts: Vec<ConstDecl>,
    tests: Vec<TestCase>,
    start_pos: Option<usize>,
    end_pos: Option<usize>,
}

impl TopLevel {
    fn new() -> Self {
        TopLevel {
            functions: Vec::new(),
            consts: Vec::new(),
            tests: Vec::new(),
            start_pos: None,
            end_pos: None,
        }
    }

    /// Record a declaration's start. Keeps the *later* of the two, which
    /// is what the closure this replaces did.
    fn saw_start(&mut self, start: usize) {
        if self.start_pos.is_none() || self.start_pos.unwrap() < start {
            self.start_pos = Some(start);
        }
    }

    fn saw_end(&mut self, end: usize) {
        self.end_pos = Some(end);
    }
}

impl<'a> Parser<'a> {
    pub fn parse_program(&mut self) -> ParserResult<File> {
        let mut out = TopLevel::new();

        // Parse package declaration (optional, at beginning of file)
        let package_decl = if matches!(self.peek(), Some(Kind::Package)) {
            Some(self.parse_package_decl()?)
        } else {
            None
        };

        // Parse import declarations (multiple allowed)
        //
        // MODULE-IMPORTS D1: an `as` alias is recorded before any body
        // is parsed, so `h::f(...)` in a function below can be spelled
        // back to the module it names. See `Parser::import_aliases`.
        let mut imports = Vec::new();
        while matches!(self.peek(), Some(Kind::Import)) {
            let import = self.parse_import_decl()?;
            if let (Some(alias), Some(&last)) = (import.alias, import.module_path.last()) {
                self.import_aliases.insert(alias, last);
            }
            imports.push(import);
        }

        loop {
            // Start a fresh error budget only when a real declaration
            // begins. The catch-all arm below skips one unrecognised
            // token per iteration, and resetting there would hand every
            // skipped token its own report — which is exactly the
            // cascade this rule exists to suppress.
            if matches!(
                self.peek(),
                Some(Kind::Function)
                    | Some(Kind::Extern)
                    | Some(Kind::Public)
                    | Some(Kind::Struct)
                    | Some(Kind::Impl)
                    | Some(Kind::Enum)
                    | Some(Kind::Trait)
                    | Some(Kind::Const)
                    | Some(Kind::Type)
            ) {
                self.begin_declaration();
            }

            // Check for visibility modifier first
            let visibility = if matches!(self.peek(), Some(Kind::Public)) {
                self.next(); // consume 'pub'
                Visibility::Public
            } else {
                Visibility::Private
            };

            // Prefix modifiers on a function declaration, in any
            // order: `never_allocates` (NEVER-ALLOCATES), `const`
            // (COMPILE-TIME-EVAL C1), and `unsafe` (POINTER P6).
            //
            // `never_allocates` is contextual like `test` — only one
            // immediately followed by `fn` / `extern` / another
            // modifier is a modifier, so a program with its own
            // `never_allocates` name is unaffected. `const` is already
            // a keyword, and the token after it separates the two
            // meanings without ambiguity: a declaration names a
            // binding (`const N: u64 = ...`), a modifier is followed
            // by `fn`. `unsafe` is contextual the same way, so a
            // program may still name a binding `unsafe`. The shared
            // gate is "the token after this one still leads to `fn`",
            // which covers every order (`unsafe const fn`,
            // `never_allocates unsafe fn`, ...).
            let mut never_allocates = false;
            let mut const_fn = false;
            let mut is_unsafe = false;
            loop {
                let next_leads_to_fn = matches!(
                    self.peek_n(1),
                    Some(Kind::Function) | Some(Kind::Extern) | Some(Kind::Const)
                ) || matches!(self.peek_n(1), Some(Kind::Identifier(s)) if s == "never_allocates" || s == "unsafe");
                let is_never_allocates =
                    matches!(self.peek(), Some(Kind::Identifier(s)) if s == "never_allocates");
                let is_unsafe_mod =
                    matches!(self.peek(), Some(Kind::Identifier(s)) if s == "unsafe");
                let is_const_mod = matches!(self.peek(), Some(Kind::Const));
                if next_leads_to_fn && is_never_allocates && !never_allocates {
                    self.next();
                    never_allocates = true;
                } else if next_leads_to_fn && is_const_mod && !const_fn {
                    self.next();
                    const_fn = true;
                } else if next_leads_to_fn && is_unsafe_mod && !is_unsafe {
                    self.next();
                    is_unsafe = true;
                } else {
                    break;
                }
            }

            // LLM-LOOP P4: `test "name" { ... }`. Recognised
            // contextually — `test` stays an ordinary identifier
            // everywhere else, so existing code with a `fn test(..)` or
            // a variable named `test` keeps working. Only the exact
            // shape `test <string> {` at top level is a test block.
            // TEST-TOOL T4: `panics` may sit between the name and the
            // block, optionally with the message the panic must
            // contain. Contextual like `test` itself.
            if matches!(self.peek(), Some(Kind::Identifier(s)) if s == "test")
                && matches!(self.peek_n(1), Some(Kind::String(_)))
                && (matches!(self.peek_n(2), Some(Kind::BraceOpen))
                    || matches!(self.peek_n(2), Some(Kind::Identifier(s)) if s == "panics"))
            {
                let test_start_pos = self.peek_position_n(0).unwrap().start;
                let location = self.current_source_location();
                out.saw_start(test_start_pos);
                self.next(); // consume `test`
                let display_name = match self.peek() {
                    Some(Kind::String(s)) => s.clone(),
                    _ => unreachable!("peeked above"),
                };
                self.next(); // consume the name
                let expect_panic = if matches!(
                    self.peek(),
                    Some(Kind::Identifier(s)) if s == "panics"
                ) {
                    self.next(); // consume `panics`
                    match self.peek() {
                        Some(Kind::String(msg)) => {
                            let msg = msg.clone();
                            self.next();
                            Some(Some(msg))
                        }
                        _ => Some(None),
                    }
                } else {
                    None
                };
                let outer_function = self
                    .current_function
                    .replace(format!("test \"{display_name}\""));
                let block = super::expr::parse_block(self)?;
                self.current_function = outer_function;
                let test_end_pos = self.peek_position_n(0).unwrap_or(&(0..0)).end;
                out.saw_end(test_end_pos);

                // Lowered to a regular function so the type checker and
                // every backend need no test-specific handling.
                let fn_name = self
                    .string_interner
                    .get_or_intern(format!("__test_{}", out.tests.len()));
                out.functions.push(Rc::new(Function {
                    node: Node::new(test_start_pos, test_end_pos),
                    name: fn_name,
                    generic_params: vec![],
                    generic_bounds: std::collections::HashMap::new(),
                    parameter: vec![],
                    return_type: None,
                    requires: vec![],
                    ensures: vec![],
                    ensures_kinds: vec![],
                    never_allocates: false,
                    const_fn: false,
                    // POINTER P6: `unsafe test "..." { ... }` — the
                    // modifier is parsed by the shared loop above and
                    // rides on the synthesized function.
                    is_unsafe,
                    old_exprs: vec![],
                    code: self.ast_builder.expression_stmt(block, Some(location)),
                    is_extern: false,
                    extern_link: None,
                    visibility: Visibility::Private,
                }));
                out.tests.push(TestCase {
                    name: display_name,
                    function: fn_name,
                    line: location.line,
                    file: None,
                    expect_panic,
                });
                continue;
            }

            match self.peek() {
                Some(Kind::Extern) => self.parse_toplevel_extern_decl(&mut out, visibility, never_allocates, is_unsafe)?,
                Some(Kind::Function) => self.parse_toplevel_function(&mut out, visibility, never_allocates, is_unsafe, const_fn)?,
                Some(Kind::Const) => self.parse_toplevel_const_decl(&mut out, visibility)?,
                Some(Kind::Type) => self.parse_toplevel_type_alias(&mut out, visibility)?,
                Some(Kind::Struct) => self.parse_toplevel_struct_decl(&mut out, visibility)?,
                Some(Kind::Enum) => self.parse_toplevel_enum_decl(&mut out, visibility)?,
                Some(Kind::Impl) => self.parse_toplevel_impl_block(&mut out)?,
                Some(Kind::Trait) => self.parse_toplevel_trait_decl(&mut out, visibility)?,
                Some(Kind::NewLine) => {
                    self.next()
                }
                None | Some(Kind::EOF) => {
                    // Check if 'pub' was used without any declaration
                    if matches!(visibility, Visibility::Public) {
                        self.collect_error("'pub' keyword must be followed by a function or struct declaration");
                    }
                    break;
                }
                x => {
                    let x_cloned = x.cloned();
                    // Check if 'pub' was used with unsupported elements
                    if matches!(visibility, Visibility::Public) {
                        match &x_cloned {
                            Some(Kind::Impl) => {
                                self.collect_error("'pub' is not yet supported for impl blocks");
                            }
                            _ => {
                                self.collect_error("'pub' can only be used with function and struct declarations");
                            }
                        }
                    }
                    self.collect_error(&format!("unexpected token: {:?}", x_cloned));
                    self.next(); // Skip invalid token and continue
                }
            }
        }

        // Any error collected while parsing means the AST below is not
        // what the user wrote, so surface it rather than handing back a
        // silently wrong tree. Only "reserved keyword" errors used to
        // propagate; everything else was dropped, and the damage showed
        // up later as an unrelated diagnostic (a function swallowed by a
        // bad `else if` was reported as "Function 'main' not found") or
        // not at all until runtime.
        self.merge_lex_errors();
        if let Some(error) = self.errors.first() {
            return Err(error.clone());
        }

        let mut ast_builder = AstBuilder::new();
        std::mem::swap(&mut ast_builder, &mut self.ast_builder);
        let (expr, stmt, location_pool) = ast_builder.extract_pools();
        let function_module_paths = vec![None; out.functions.len()];
        let function_module_ranks = vec![0u32; out.functions.len()];
        Ok(File {
            id: crate::ast::program::next_file_id(),
            node: Node::new(out.start_pos.unwrap_or(0usize), out.end_pos.unwrap_or(0usize)),
            package_decl,
            imports,
            function: out.functions,
            function_module_paths,
            function_module_ranks,
            consts: out.consts,
            tests: out.tests,
            transferred_bindings: std::collections::HashSet::new(),
            statement: stmt,
            expression: expr,
            location_pool,
            // DEBUG-OBS D2: the parser knows the text but not what it
            // is called, so the entry slot is seeded with the source
            // and an empty path for the driver to name. Carrying the
            // text here is what lets an integrated module's excerpt be
            // drawn from a *cached* parse, where nothing re-reads the
            // file from disk.
            source_map: SourceMap::with_entry(String::new(), self.input),
        })
    }


    /// An `extern fn` declaration.
    fn parse_toplevel_extern_decl(
        &mut self,
        out: &mut TopLevel,
        visibility: Visibility,
        never_allocates: bool,
        is_unsafe: bool,
    ) -> ParserResult<()> {
        // `extern fn name(params) -> ret` — declares a
        // function whose body is provided by the runtime
        // / linker (interpreter registry / JIT helper /
        // libm). No body block; no contract clauses.
        let fn_start_pos = self.peek_position_n(0).unwrap().start;
        let location = self.current_source_location();
        out.saw_start(fn_start_pos);
        self.next(); // consume 'extern'
        if !matches!(self.peek(), Some(Kind::Function)) {
            self.collect_error("expected `fn` after `extern`");
            return Ok(());
        }
        self.next(); // consume 'fn'
        let fn_name = match self.peek() {
            Some(Kind::Identifier(s)) => {
                let s = s.to_string();
                let n = self.string_interner.get_or_intern(s);
                self.next();
                n
            }
            _ => {
                self.collect_error("expected function name after `extern fn`");
                self.next();
                return Ok(());
            }
        };
        // #195: optional generic params on extern fn
        // (`extern fn pick<T>(a: T, b: T) -> T`).  Parsed
        // here so the AST shape matches non-extern fns,
        // but each backend's actual dispatch decides
        // whether to accept the call: the interpreter
        // walks the typed args at call time (works
        // unconditionally), the JIT and AOT compiler
        // need name-mangled monomorph entries (rejected
        // with a clear error until they're wired).
        let (generic_params, generic_bounds) = if matches!(self.peek(), Some(Kind::LT)) {
            self.parse_generic_params()?
        } else {
            (vec![], std::collections::HashMap::new())
        };
        self.expect_err(&Kind::ParenOpen)?;
        let params = self.parse_param_def_list_with_generic_context(vec![], &generic_params)?;
        self.expect_err(&Kind::ParenClose)?;
        let mut ret_ty: Option<TypeDecl> = None;
        if let Some(Kind::Arrow) = self.peek() {
            self.expect_err(&Kind::Arrow)?;
            let generic_context: HashSet<DefaultSymbol> = generic_params.iter().cloned().collect();
            ret_ty = Some(self.parse_type_declaration_with_generic_context(
                &generic_context,
            )?);
        }
        self.skip_newlines();
        // FFI_PLAN P1: `from "lib" [as "sym"]` — the
        // declaration carries the linker symbol instead of
        // relying on the backend's built-in dispatch.
        // `from` is a contextual keyword here (it stays an
        // ordinary identifier everywhere else), `as` is
        // the existing keyword.
        let mut extern_link = None;
        let starts_with_from =
            matches!(self.peek(), Some(Kind::Identifier(s)) if s == "from");
        if starts_with_from {
            self.next(); // consume `from`
            // Extract the literal text before interning:
            // `self.peek()` holds a borrow of the token
            // stream, which `get_or_intern` (a `&mut self`
            // call) must not overlap.
            let lib_text = match self.peek() {
                Some(Kind::String(s)) => Some(s.clone()),
                _ => None,
            };
            let lib = match lib_text {
                Some(text) => {
                    let sym = self.string_interner.get_or_intern(text);
                    self.next();
                    sym
                }
                None => {
                    self.collect_error(
                        "expected a string literal after `from` (e.g. `from \"mylib\"`)",
                    );
                    self.next();
                    return Ok(());
                }
            };
            let as_next = matches!(self.peek(), Some(Kind::As));
            let symbol = if as_next {
                self.next(); // consume `as`
                let sym_text = match self.peek() {
                    Some(Kind::String(s)) => Some(s.clone()),
                    _ => None,
                };
                match sym_text {
                    Some(text) => {
                        let sym = self.string_interner.get_or_intern(text);
                        self.next();
                        Some(sym)
                    }
                    None => {
                        self.collect_error(
                            "expected a string literal after `as` (e.g. `as \"sym\"`)",
                        );
                        self.next();
                        return Ok(());
                    }
                }
            } else {
                None
            };
            extern_link = Some(ExternLink { lib, symbol });
        }
        let fn_end_pos = self.peek_position_n(0).unwrap_or(&(0..0)).end;
        out.saw_end(fn_end_pos);
        // Use a placeholder `Stmt::Break` as the body slot.
        // Backends consult `is_extern` before walking it, so
        // the placeholder never executes.
        let placeholder_body_expr = self
            .ast_builder
            .add_expr(crate::ast::Expr::Block(vec![]));
        let placeholder_body = self
            .ast_builder
            .expression_stmt(placeholder_body_expr, Some(location));
        out.functions.push(Rc::new(Function {
            node: Node::new(fn_start_pos, fn_end_pos),
            name: fn_name,
            generic_params,
            generic_bounds,
            parameter: params,
            return_type: ret_ty,
            requires: vec![],
            ensures: vec![],
            ensures_kinds: vec![],
            // NEVER-ALLOCATES: on an `extern fn` this is a
            // declaration, not a check — the body is
            // outside the language, so the compiler takes
            // the author's word and lets a
            // `never_allocates` caller through.
            never_allocates,
            // POINTER P6: on an `extern fn` this is a declaration,
            // not a check — the body is outside the language.
            is_unsafe,
            // An `extern fn` body is outside the language, so it can
            // never be evaluated at compile time.
            const_fn: false,
            old_exprs: vec![],
            code: placeholder_body,
            is_extern: true,
            extern_link,
            visibility,
        }));
        Ok(())
    }

    /// A `fn` definition.
    fn parse_toplevel_function(
        &mut self,
        out: &mut TopLevel,
        visibility: Visibility,
        never_allocates: bool,
        is_unsafe: bool,
        const_fn: bool,
    ) -> ParserResult<()> {
        let fn_start_pos = self.peek_position_n(0).unwrap().start;
        let location = self.current_source_location();
        out.saw_start(fn_start_pos);
        self.next();
        match self.peek() {
            Some(Kind::Identifier(s)) => {
                let s = s.to_string();
                let fn_name = self.string_interner.get_or_intern(s);
                self.next();

                // Parse generic parameters if present: <T> or <A: Allocator>
                let (generic_params, generic_bounds) = if matches!(self.peek(), Some(Kind::LT)) {
                    self.parse_generic_params()?
                } else {
                    (vec![], std::collections::HashMap::new())
                };

                self.expect_err(&Kind::ParenOpen)?;
                let params = self.parse_param_def_list_with_generic_context(vec![], &generic_params)?;
                self.expect_err(&Kind::ParenClose)?;
                let mut ret_ty: Option<TypeDecl> = None;
                if let Some(Kind::Arrow) = self.peek() {
                    self.expect_err(&Kind::Arrow)?;
                    // Convert to HashSet for generic context
                    let generic_context: HashSet<DefaultSymbol> = generic_params.iter().cloned().collect();
                    ret_ty = Some(self.parse_type_declaration_with_generic_context(&generic_context)?);
                }
                // Design-by-Contract clauses live between the
                // return type and the body block, mirroring how
                // `<T: Bound>` annotates a generic param. They are
                // optional and may repeat; multiple clauses of the
                // same kind are AND-composed by the type checker.
                let clauses = self.parse_contract_clauses()?;
                // DEBUG-OBS D5: `__builtin_function_name()` inside the
                // body resolves to this.
                let outer_function = self
                    .current_function
                    .replace(self.string_interner.resolve(fn_name).unwrap_or("<fn>").to_string());
                let block = super::expr::parse_block(self)?;
                self.current_function = outer_function;
                let fn_end_pos = self.peek_position_n(0).unwrap_or(&(0..0)).end;
                out.saw_end(fn_end_pos);

                out.functions.push(Rc::new(Function {
                    node: Node::new(fn_start_pos, fn_end_pos),
                    name: fn_name,
                    generic_params,
                    generic_bounds,
                    parameter: params,
                    return_type: ret_ty,
                    requires: clauses.requires,
                    ensures: clauses.ensures,
                    ensures_kinds: clauses.ensures_kinds,
                    never_allocates,
                    is_unsafe,
                    const_fn,
                    old_exprs: clauses.old_exprs,
                    code: self.ast_builder.expression_stmt(block, Some(location)),
                    is_extern: false,
                    extern_link: None,
                    visibility,
                }));
            }
            _ => {
                self.collect_error("expected function name");
                self.next(); // Skip invalid token and continue
            }
        }
        Ok(())
    }

    /// A top-level `const`.
    fn parse_toplevel_const_decl(
        &mut self,
        out: &mut TopLevel,
        visibility: Visibility,
    ) -> ParserResult<()> {
        // Top-level `const NAME: Type = expr` declaration. Type
        // annotation is mandatory (no inference) so that const
        // signatures stay greppable. The value expression goes
        // through the regular expression parser, which lets it
        // see other const names that have already been declared
        // (forward references are not allowed).
        let const_start_pos = self.peek_position_n(0).unwrap().start;
        out.saw_start(const_start_pos);
        self.next(); // consume `const`

        let const_name = match self.peek().cloned() {
            Some(Kind::Identifier(s)) => {
                let sym = self.string_interner.get_or_intern(s);
                self.next();
                sym
            }
            _ => {
                self.collect_error("expected identifier after `const`");
                self.next();
                return Ok(());
            }
        };

        self.expect_err(&Kind::Colon)?;
        let const_ty = self.parse_type_declaration()?;
        self.expect_err(&Kind::Equal)?;
        let value = self.parse_expr_impl()?;
        let const_end_pos = self.peek_position_n(0).unwrap_or(&(0..0)).end;
        out.saw_end(const_end_pos);

        // COMPILE-TIME-EVAL C5: remember the value if it is one an
        // array length could use. See `Parser::const_lengths`.
        if let Some(length) = self.const_length_of(&value) {
            self.const_lengths.insert(const_name, length);
        }

        out.consts.push(ConstDecl {
            node: Node::new(const_start_pos, const_end_pos),
            name: const_name,
            type_decl: const_ty,
            value,
            visibility,
        });
        Ok(())
    }

    /// COMPILE-TIME-EVAL C5: the non-negative integer a `const`
    /// initialiser names outright, or `None` when working it out would
    /// take an evaluator.
    fn const_length_of(&self, value: &ExprRef) -> Option<u64> {
        match self.ast_builder.get_expr_pool().get(value)? {
            Expr::UInt64(v) => Some(v),
            Expr::UInt8(v) => Some(v as u64),
            Expr::UInt16(v) => Some(v as u64),
            Expr::UInt32(v) => Some(v as u64),
            Expr::Int64(v) => u64::try_from(v).ok(),
            Expr::Int8(v) => u64::try_from(v).ok(),
            Expr::Int16(v) => u64::try_from(v).ok(),
            Expr::Int32(v) => u64::try_from(v).ok(),
            // A suffix-less literal: the type checker decides its type
            // later, but the digits are already here.
            Expr::Number(sym) => self.string_interner.resolve(sym)?.parse::<u64>().ok(),
            // `const M: u64 = N` — one more hop, still no arithmetic.
            Expr::Identifier(sym) => self.const_lengths.get(&sym).copied(),
            _ => None,
        }
    }

    /// A `type` alias.
    fn parse_toplevel_type_alias(
        &mut self,
        out: &mut TopLevel,
        visibility: Visibility,
    ) -> ParserResult<()> {
        // `type Name = TargetType` — top-level alias.
        // Optional generic parameters `type Name<T, U> = ...`
        // turn the alias parameterised: occurrences of
        // `Name<i64>` substitute `T` -> `i64` in the target
        // at parse time. Bounds on the parameters are
        // accepted but ignored — they don't make sense for
        // a pure substitution alias.
        let alias_start_pos = self.peek_position_n(0).unwrap().start;
        let location = self.current_source_location();
        out.saw_start(alias_start_pos);
        self.next(); // consume `type`

        let alias_name = match self.peek().cloned() {
            Some(Kind::Identifier(s)) => {
                let sym = self.string_interner.get_or_intern(s);
                self.next();
                sym
            }
            _ => {
                self.collect_error("expected identifier after `type`");
                self.next();
                return Ok(());
            }
        };

        let alias_generic_params: Vec<DefaultSymbol> = if matches!(self.peek(), Some(Kind::LT)) {
            // `parse_generic_params` consumes the leading
            // `<` and the trailing `>` itself, so no
            // bracket-balancing required here.
            let (params, _bounds) = self.parse_generic_params()?;
            params
        } else {
            Vec::new()
        };

        self.expect_err(&Kind::Equal)?;
        let generic_context: HashSet<DefaultSymbol> =
            alias_generic_params.iter().copied().collect();
        let target_ty =
            self.parse_type_declaration_with_generic_context(&generic_context)?;
        let alias_end_pos = self.peek_position_n(0).unwrap_or(&(0..0)).end;
        out.saw_end(alias_end_pos);

        // Register before emitting so the AST node carries
        // the already-resolved target (anonymous alias chains
        // — `type A = u8; type B = A` — collapse to the
        // leaf). Generic aliases keep `Generic(T)` markers
        // in the target; the substitution happens at the
        // use site.
        self.type_aliases.insert(alias_name, (alias_generic_params.clone(), target_ty.clone()));
        self.ast_builder.add_stmt_with_location(Stmt::TypeAlias {
            name: alias_name,
            generic_params: alias_generic_params,
            target: target_ty,
            visibility,
        }, Some(location));
        Ok(())
    }

    /// A `struct` declaration.
    fn parse_toplevel_struct_decl(
        &mut self,
        out: &mut TopLevel,
        visibility: Visibility,
    ) -> ParserResult<()> {
        let struct_start_pos = self.peek_position_n(0).unwrap().start;
        let location = self.current_source_location();
        out.saw_start(struct_start_pos);
        self.next();
        match self.peek() {
            Some(Kind::Identifier(s)) => {
                let s_copy = s.clone();
                let struct_symbol = self.string_interner.get_or_intern(&s_copy);
                self.next();

                // Parse generic parameters if present: struct Foo<T> or struct Foo<A: Allocator>
                let (generic_params, generic_bounds) = if matches!(self.peek(), Some(Kind::LT)) {
                    self.parse_generic_params()?
                } else {
                    (vec![], std::collections::HashMap::new())
                };

                if !generic_params.is_empty() {
                    self.declared_type_generics
                        .insert(struct_symbol, generic_params.clone());
                }
                // NEWTYPE: `struct Meters(i64)` is sugar for a
                // struct whose fields are named by position
                // (`"0"`, `"1"`, ...). Everything downstream --
                // the type checker's struct registry, all three
                // backends, drop glue, `--api` -- then handles it
                // as an ordinary struct. The two sugared *uses*
                // (`Meters(v)` construction and `m.0` access) are
                // rewritten in the type checker, which is where
                // the struct table is available.
                let fields = if matches!(self.peek(), Some(Kind::ParenOpen)) {
                    super::stmt::parse_tuple_struct_fields(self, &generic_params)?
                } else {
                    self.expect_err(&Kind::BraceOpen)?;
                    let fields = super::stmt::parse_struct_fields_with_generic_context(self, vec![], &generic_params)?;
                    self.expect_err(&Kind::BraceClose)?;
                    fields
                };
                let struct_end_pos = self.peek_position_n(0).unwrap_or(&(0..0)).end;
                out.saw_end(struct_end_pos);

                self.ast_builder.struct_decl_stmt(struct_symbol, generic_params, generic_bounds, fields, visibility, Some(location));
            }
            _ => {
                self.collect_error("expected struct name");
                self.next(); // Skip invalid token and continue
            }
        }
        Ok(())
    }

    /// An `enum` declaration.
    fn parse_toplevel_enum_decl(
        &mut self,
        out: &mut TopLevel,
        visibility: Visibility,
    ) -> ParserResult<()> {
        let enum_start_pos = self.peek_position_n(0).unwrap().start;
        let location = self.current_source_location();
        out.saw_start(enum_start_pos);
        self.next(); // consume 'enum'
        match self.peek() {
            Some(Kind::Identifier(s)) => {
                let s_copy = s.clone();
                let enum_symbol = self.string_interner.get_or_intern(&s_copy);
                self.next();
                // Optional generic parameters: `enum Name<T, U>`.
                // Bounds aren't meaningful for enums yet; we drop
                // the bounds map returned by parse_generic_params.
                let generic_params: Vec<DefaultSymbol> = if matches!(self.peek(), Some(Kind::LT)) {
                    let (params, _bounds) = self.parse_generic_params()?;
                    params
                } else {
                    Vec::new()
                };
                if !generic_params.is_empty() {
                    self.declared_type_generics
                        .insert(enum_symbol, generic_params.clone());
                }
                let generic_context: HashSet<DefaultSymbol> = generic_params.iter().cloned().collect();
                self.expect_err(&Kind::BraceOpen)?;
                self.skip_newlines();
                let mut variants: Vec<crate::ast::EnumVariantDef> = Vec::new();
                loop {
                    self.skip_newlines();
                    match self.peek() {
                        Some(Kind::BraceClose) => break,
                        Some(Kind::Identifier(name)) => {
                            let variant_name = name.clone();
                            let variant_sym = self.string_interner.get_or_intern(&variant_name);
                            self.next();
                            // Optional tuple payload: `Name(Type, Type, ...)`.
                            let mut payload_types: Vec<TypeDecl> = Vec::new();
                            if matches!(self.peek(), Some(Kind::ParenOpen)) {
                                self.next(); // consume '('
                                loop {
                                    self.skip_newlines();
                                    if matches!(self.peek(), Some(Kind::ParenClose)) {
                                        break;
                                    }
                                    let ty = self.parse_type_declaration_with_generic_context(&generic_context)?;
                                    payload_types.push(ty);
                                    self.skip_newlines();
                                    if matches!(self.peek(), Some(Kind::Comma)) {
                                        self.next();
                                    } else {
                                        break;
                                    }
                                }
                                self.expect_err(&Kind::ParenClose)?;
                            }
                            variants.push(crate::ast::EnumVariantDef {
                                name: variant_sym,
                                payload_types,
                            });
                            self.skip_newlines();
                            if matches!(self.peek(), Some(Kind::Comma)) {
                                self.next();
                                self.skip_newlines();
                            }
                        }
                        other => {
                            let other_str = format!("{:?}", other);
                            self.collect_error(&format!(
                                "expected variant name in enum body, got {}", other_str
                            ));
                            break;
                        }
                    }
                }
                self.expect_err(&Kind::BraceClose)?;
                let enum_end_pos = self.peek_position_n(0).unwrap_or(&(0..0)).end;
                out.saw_end(enum_end_pos);
                self.ast_builder.add_stmt_with_location(Stmt::EnumDecl {
                    name: enum_symbol,
                    generic_params,
                    variants,
                    visibility,
                }, Some(location));
            }
            _ => {
                self.collect_error("expected enum name");
                self.next();
            }
        }
        Ok(())
    }

    /// An `impl` block.
    fn parse_toplevel_impl_block(
        &mut self,
        out: &mut TopLevel,
    ) -> ParserResult<()> {
        let impl_start_pos = self.peek_position_n(0).unwrap().start;
        let location = self.current_source_location();
        out.saw_start(impl_start_pos);
        self.next();

        // Parse optional generic parameters: impl<T> or impl<A: Allocator>
        let (generic_params, generic_bounds) = if self.peek() == Some(&Kind::LT) {
            self.parse_generic_params()?
        } else {
            (vec![], std::collections::HashMap::new())
        };

        match self.peek() {
            Some(Kind::Identifier(s)) => {
                let s_copy = s.clone();
                let first_ident_symbol = self.string_interner.get_or_intern(&s_copy);
                self.next();

                // CONCRETE-IMPL Phase 2 follow-up: capture
                // type args on the *first* identifier too so
                // inherent `impl Vec<u8>` (no `for`) ends up
                // with `target_type_args = [u8]`, parallel to
                // the trait-impl branch below. Without this,
                // the inherent path falls back to
                // `skip_until_matching_gt` and CONCRETE-IMPL
                // dispatch loses its key. Trait impls
                // overwrite this from the parsed `Type<...>`
                // following `for` (the first identifier was
                // the trait name, not the target).
                let generic_params_set: std::collections::HashSet<DefaultSymbol> = generic_params.iter().copied().collect();
                let mut first_target_args = if self.peek() == Some(&Kind::LT) {
                    self.next(); // consume '<'
                    self.parse_type_args_after_lt(&generic_params_set)?
                } else {
                    Vec::new()
                };
                // Implicit type-parameter list: `impl
                // Container<T>` re-uses what `struct
                // Container<T>` declared, as the language
                // reference specifies.
                //
                // Decided *after* parsing the args, on the
                // args themselves: a name is a type
                // parameter only if the declaration lists
                // it. `u8` in `impl Vec<u8>` lexes as a
                // type keyword and can never match, so the
                // concrete-args form (CONCRETE-IMPL) is
                // untouched — and `impl C<i64>` alongside
                // `impl C<u8>` keeps dispatching to two
                // separate specs. Adopting the declaration
                // wholesale instead would turn those into
                // generic templates and lose the methods.
                let mut generic_params = generic_params;
                if generic_params.is_empty()
                    && let Some(declared) =
                        self.declared_type_generics.get(&first_ident_symbol)
                {
                    let declared = declared.clone();
                    let implicit: Vec<DefaultSymbol> = first_target_args
                        .iter()
                        .filter_map(|a| match a {
                            TypeDecl::Identifier(sym) if declared.contains(sym) => {
                                Some(*sym)
                            }
                            _ => None,
                        })
                        .collect();
                    if !implicit.is_empty() {
                        for arg in first_target_args.iter_mut() {
                            if let TypeDecl::Identifier(sym) = arg
                                && implicit.contains(sym)
                            {
                                *arg = TypeDecl::Generic(*sym);
                            }
                        }
                        generic_params = implicit;
                    }
                }

                // `impl Trait for Type` — the `for` keyword is
                // contextually reused here. If present, the
                // identifier we just consumed was the trait name
                // and the next identifier (or primitive type
                // keyword) is the target type. Primitive types
                // (`i64`, `f64`, …) interned by their canonical
                // name string so the same `DefaultSymbol`
                // identifies the impl target across the
                // type-checker / interpreter / compiler — they
                // are reserved keywords so there's no clash with
                // a user struct of the same name.
                // ITER-PROTOCOL-TRAIT: when `for` follows,
                // `first_target_args` actually carries the
                // trait's concrete type args (`<i64>` in
                // `impl Iterator<i64> for Counter`). Pass
                // them through as `trait_type_args` so the
                // type checker can substitute the trait's
                // generic params at conformance time.
                let (trait_name, trait_type_args, target_type_symbol, target_type_args) =
                    if matches!(self.peek(), Some(Kind::For)) {
                        self.next(); // consume `for`
                        let (target_sym, target_args) = match self.peek() {
                            Some(Kind::Identifier(name)) => {
                                let name_copy = name.clone();
                                let sym = self.string_interner.get_or_intern(&name_copy);
                                self.next();
                                let args = if self.peek() == Some(&Kind::LT) {
                                    self.next(); // consume '<'
                                    self.parse_type_args_after_lt(&generic_params_set)?
                                } else {
                                    Vec::new()
                                };
                                (sym, args)
                            }
                            Some(kind) if primitive_type_canonical_name(kind).is_some() => {
                                let name = primitive_type_canonical_name(kind).unwrap();
                                let sym = self.string_interner.get_or_intern(name);
                                self.next();
                                (sym, Vec::new())
                            }
                            _ => {
                                self.collect_error("expected target type after `for` in impl-trait");
                                self.next();
                                return Ok(());
                            }
                        };
                        (Some(first_ident_symbol), first_target_args, target_sym, target_args)
                    } else {
                        // Inherent impl: first identifier is the
                        // target type; its `<...>` (if any) was
                        // captured into `first_target_args`.
                        (None, Vec::new(), first_ident_symbol, first_target_args)
                    };

                self.expect_err(&Kind::BraceOpen)?;
                let outer_target = self.current_impl_target.replace(
                    self.string_interner
                        .resolve(target_type_symbol)
                        .unwrap_or("<impl>")
                        .to_string(),
                );
                let methods = super::stmt::parse_impl_methods_with_generic_context(self, vec![], &generic_params, &generic_bounds)?;
                self.current_impl_target = outer_target;
                self.expect_err(&Kind::BraceClose)?;
                let impl_end_pos = self.peek_position_n(0).unwrap_or(&(0..0)).end;
                out.saw_end(impl_end_pos);

                self.ast_builder.impl_block_stmt_with_trait_args(
                    target_type_symbol,
                    target_type_args,
                    methods,
                    trait_name,
                    trait_type_args,
                    Some(location),
                );
            }
            _ => {
                self.collect_error("expected type name for impl block");
                self.next(); // Skip invalid token and continue
            }
        }
        Ok(())
    }

    /// A `trait` declaration.
    fn parse_toplevel_trait_decl(
        &mut self,
        out: &mut TopLevel,
        visibility: Visibility,
    ) -> ParserResult<()> {
        let trait_start_pos = self.peek_position_n(0).unwrap().start;
        let location = self.current_source_location();
        out.saw_start(trait_start_pos);
        self.next(); // consume `trait`
        match self.peek() {
            Some(Kind::Identifier(s)) => {
                let s_copy = s.clone();
                let trait_symbol = self.string_interner.get_or_intern(&s_copy);
                self.next();
                // ITER-PROTOCOL-TRAIT: optional generic
                // parameter list `<T, U, ...>`. We discard
                // any per-parameter bounds here — trait
                // generics don't (yet) participate in the
                // bound-check pipeline; treating them as
                // unbounded is identical to how struct
                // generics start out.
                let (trait_generic_params, _trait_generic_bounds) =
                    if matches!(self.peek(), Some(Kind::LT)) {
                        self.parse_generic_params()?
                    } else {
                        (Vec::new(), std::collections::HashMap::new())
                    };
                // STDLIB-TRAIT-BASE B0: `trait B: A { ... }` is not
                // supported, and saying so beats `expect_err`'s bare
                // `BraceOpen` -- which names the token the parser
                // wanted and nothing about what was written.
                if matches!(self.peek(), Some(Kind::Colon)) {
                    let location = self.current_source_location();
                    return Err(ParserError::generic_error(
                        location,
                        "trait inheritance (`trait B: A`) is not supported; declare the methods `B` needs on `B` itself, or take both bounds at the use site (`fn f<T: A + B>(...)`)"
                            .to_string(),
                    ));
                }
                self.expect_err(&Kind::BraceOpen)?;
                let methods = super::stmt::parse_trait_method_signatures_with_generics(
                    self,
                    &trait_generic_params,
                )?;
                self.expect_err(&Kind::BraceClose)?;
                let trait_end_pos = self.peek_position_n(0).unwrap_or(&(0..0)).end;
                out.saw_end(trait_end_pos);
                self.ast_builder.trait_decl_stmt_with_generics(
                    trait_symbol,
                    trait_generic_params,
                    methods,
                    visibility,
                    Some(location),
                );
            }
            _ => {
                self.collect_error("expected trait name");
                self.next();
            }
        }
        Ok(())
    }

    pub fn parse_program_multiple_errors(&mut self) -> MultipleParserResult<File> {
        self.errors.clear();

        match self.parse_program() {
            Ok(program) => {
                // `parse_program` merges lex errors itself on its
                // success path; on a `?` early return it never reached
                // the merge, so the failure path below drains whatever
                // is left.
                self.merge_lex_errors();
                if self.errors.is_empty() {
                    MultipleParserResult::success(program)
                } else {
                    MultipleParserResult::with_errors(program, self.collected_errors_for_report())
                }
            }
            Err(hard_failure) => {
                // A `?` bubbling out of a sub-parser can return before
                // anything was collected. Report that one rather than an
                // empty list — "the parse failed, and here is nothing"
                // is the least useful diagnostic there is.
                self.merge_lex_errors();
                if self.errors.is_empty() {
                    MultipleParserResult::failure(vec![hard_failure])
                } else {
                    MultipleParserResult::failure(self.collected_errors_for_report())
                }
            }
        }
    }

    /// Collected parse errors, in source order and without duplicates.
    ///
    /// LLM-LOOP P1: same treatment the type checker's errors get. The
    /// parser recovers and keeps going, so one mistake can be recorded
    /// more than once as the recovery path re-enters; reporting the same
    /// line twice reads as two separate problems.
    fn collected_errors_for_report(&self) -> Vec<ParserError> {
        let mut errors = self.errors.clone();
        errors.sort_by_key(|e| (e.location.line, e.location.column, e.location.offset));
        // One report per line. Unlike the type checker — which recovers
        // at statement boundaries and produces genuinely independent
        // errors — the parser resumes mid-expression and piles on
        // consequences of the same bad token: "maximum parse iterations
        // reached", "unexpected token", "expected statement in block",
        // each at its own offset but all describing one mistake. A line
        // is the coarsest unit that still separates mistakes a reader
        // would fix separately; two errors on one line collapse into the
        // first, which is the one that says what is actually wrong.
        errors.dedup_by(|a, b| a.location.line == b.location.line);
        errors
    }
}
