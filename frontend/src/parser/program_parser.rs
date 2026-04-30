use std::rc::Rc;
use std::collections::HashSet;
use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::TypeDecl;
use crate::token::Kind;
use crate::parser::error::{ParserErrorKind, ParserResult, MultipleParserResult};
use super::core::Parser;

impl<'a> Parser<'a> {
    pub fn parse_program(&mut self) -> ParserResult<Program> {
        let mut start_pos: Option<usize> = None;
        let mut end_pos: Option<usize> = None;
        let mut update_start_pos = |start: usize| {
            if start_pos.is_none() || start_pos.unwrap() < start {
                start_pos = Some(start);
            }
        };
        let mut update_end_pos = |end: usize| {
            end_pos = Some(end);
        };
        let mut def_func = vec![];
        let mut consts: Vec<ConstDecl> = vec![];

        // Parse package declaration (optional, at beginning of file)
        let package_decl = if matches!(self.peek(), Some(Kind::Package)) {
            Some(self.parse_package_decl()?)
        } else {
            None
        };

        // Parse import declarations (multiple allowed)
        let mut imports = Vec::new();
        while matches!(self.peek(), Some(Kind::Import)) {
            imports.push(self.parse_import_decl()?);
        }

        loop {
            // Check for visibility modifier first
            let visibility = if matches!(self.peek(), Some(Kind::Public)) {
                self.next(); // consume 'pub'
                Visibility::Public
            } else {
                Visibility::Private
            };

            match self.peek() {
                Some(Kind::Function) => {
                    let fn_start_pos = self.peek_position_n(0).unwrap().start;
                    let location = self.current_source_location();
                    update_start_pos(fn_start_pos);
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
                            let (requires, ensures) = self.parse_contract_clauses()?;
                            let block = super::expr::parse_block(self)?;
                            let fn_end_pos = self.peek_position_n(0).unwrap_or(&(0..0)).end;
                            update_end_pos(fn_end_pos);

                            def_func.push(Rc::new(Function {
                                node: Node::new(fn_start_pos, fn_end_pos),
                                name: fn_name,
                                generic_params,
                                generic_bounds,
                                parameter: params,
                                return_type: ret_ty,
                                requires,
                                ensures,
                                code: self.ast_builder.expression_stmt(block, Some(location)),
                                visibility,
                            }));
                        }
                        _ => {
                            self.collect_error("expected function name");
                            self.next(); // Skip invalid token and continue
                        }
                    }
                }
                Some(Kind::Const) => {
                    // Top-level `const NAME: Type = expr` declaration. Type
                    // annotation is mandatory (no inference) so that const
                    // signatures stay greppable. The value expression goes
                    // through the regular expression parser, which lets it
                    // see other const names that have already been declared
                    // (forward references are not allowed).
                    let const_start_pos = self.peek_position_n(0).unwrap().start;
                    update_start_pos(const_start_pos);
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
                            continue;
                        }
                    };

                    self.expect_err(&Kind::Colon)?;
                    let const_ty = self.parse_type_declaration()?;
                    self.expect_err(&Kind::Equal)?;
                    let value = self.parse_expr_impl()?;
                    let const_end_pos = self.peek_position_n(0).unwrap_or(&(0..0)).end;
                    update_end_pos(const_end_pos);

                    consts.push(ConstDecl {
                        node: Node::new(const_start_pos, const_end_pos),
                        name: const_name,
                        type_decl: const_ty,
                        value,
                        visibility,
                    });
                }
                Some(Kind::Struct) => {
                    let struct_start_pos = self.peek_position_n(0).unwrap().start;
                    let location = self.current_source_location();
                    update_start_pos(struct_start_pos);
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

                            self.expect_err(&Kind::BraceOpen)?;
                            let fields = super::stmt::parse_struct_fields_with_generic_context(self, vec![], &generic_params)?;
                            self.expect_err(&Kind::BraceClose)?;
                            let struct_end_pos = self.peek_position_n(0).unwrap_or(&(0..0)).end;
                            update_end_pos(struct_end_pos);

                            self.ast_builder.struct_decl_stmt(struct_symbol, generic_params, generic_bounds, fields, visibility, Some(location));
                        }
                        _ => {
                            self.collect_error("expected struct name");
                            self.next(); // Skip invalid token and continue
                        }
                    }
                }
                Some(Kind::Enum) => {
                    let enum_start_pos = self.peek_position_n(0).unwrap().start;
                    let location = self.current_source_location();
                    update_start_pos(enum_start_pos);
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
                            update_end_pos(enum_end_pos);
                            self.ast_builder.add_stmt_with_location(Stmt::EnumDecl {
                                name: enum_symbol,
                                generic_params,
                                variants,
                                visibility: visibility.clone(),
                            }, Some(location));
                        }
                        _ => {
                            self.collect_error("expected enum name");
                            self.next();
                        }
                    }
                }
                Some(Kind::Impl) => {
                    let impl_start_pos = self.peek_position_n(0).unwrap().start;
                    let location = self.current_source_location();
                    update_start_pos(impl_start_pos);
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

                            // For now, we skip parsing generic arguments on
                            // the (first) identifier — both inherent and
                            // trait impls accept `Name<...>` after the name.
                            if self.peek() == Some(&Kind::LT) {
                                self.skip_until_matching_gt();
                            }

                            // `impl Trait for Type` — the `for` keyword is
                            // contextually reused here. If present, the
                            // identifier we just consumed was the trait name
                            // and the next identifier is the target type.
                            let (trait_name, target_type_symbol) = if matches!(self.peek(), Some(Kind::For)) {
                                self.next(); // consume `for`
                                let target_sym = match self.peek() {
                                    Some(Kind::Identifier(name)) => {
                                        let name_copy = name.clone();
                                        let sym = self.string_interner.get_or_intern(&name_copy);
                                        self.next();
                                        if self.peek() == Some(&Kind::LT) {
                                            self.skip_until_matching_gt();
                                        }
                                        sym
                                    }
                                    _ => {
                                        self.collect_error("expected target type after `for` in impl-trait");
                                        self.next();
                                        continue;
                                    }
                                };
                                (Some(first_ident_symbol), target_sym)
                            } else {
                                (None, first_ident_symbol)
                            };

                            self.expect_err(&Kind::BraceOpen)?;
                            let methods = super::stmt::parse_impl_methods_with_generic_context(self, vec![], &generic_params, &generic_bounds)?;
                            self.expect_err(&Kind::BraceClose)?;
                            let impl_end_pos = self.peek_position_n(0).unwrap_or(&(0..0)).end;
                            update_end_pos(impl_end_pos);

                            self.ast_builder.impl_block_stmt_with_trait(target_type_symbol, methods, trait_name, Some(location));
                        }
                        _ => {
                            self.collect_error("expected type name for impl block");
                            self.next(); // Skip invalid token and continue
                        }
                    }
                }
                Some(Kind::Trait) => {
                    let trait_start_pos = self.peek_position_n(0).unwrap().start;
                    let location = self.current_source_location();
                    update_start_pos(trait_start_pos);
                    self.next(); // consume `trait`
                    match self.peek() {
                        Some(Kind::Identifier(s)) => {
                            let s_copy = s.clone();
                            let trait_symbol = self.string_interner.get_or_intern(&s_copy);
                            self.next();
                            self.expect_err(&Kind::BraceOpen)?;
                            let methods = super::stmt::parse_trait_method_signatures(self)?;
                            self.expect_err(&Kind::BraceClose)?;
                            let trait_end_pos = self.peek_position_n(0).unwrap_or(&(0..0)).end;
                            update_end_pos(trait_end_pos);
                            self.ast_builder.trait_decl_stmt(
                                trait_symbol,
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
                }
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

        // Check if there were critical errors during parsing (like keyword usage)
        for error in &self.errors {
            // Check both direct GenericError and nested errors in UnexpectedToken
            match &error.kind {
                ParserErrorKind::GenericError { message } => {
                    if message.contains("reserved keyword") {
                        return Err(error.clone());
                    }
                }
                ParserErrorKind::UnexpectedToken { expected } => {
                    if expected.contains("reserved keyword") {
                        return Err(error.clone());
                    }
                }
                _ => {}
            }
        }

        let mut ast_builder = AstBuilder::new();
        std::mem::swap(&mut ast_builder, &mut self.ast_builder);
        let (expr, stmt, location_pool) = ast_builder.extract_pools();
        Ok(Program {
            node: Node::new(start_pos.unwrap_or(0usize), end_pos.unwrap_or(0usize)),
            package_decl,
            imports,
            function: def_func,
            consts,
            statement: stmt,
            expression: expr,
            location_pool,
        })
    }

    /// Parse program with multiple error collection
    pub fn parse_program_multiple_errors(&mut self) -> MultipleParserResult<Program> {
        self.errors.clear();

        match self.parse_program() {
            Ok(program) => {
                if self.errors.is_empty() {
                    MultipleParserResult::success(program)
                } else {
                    MultipleParserResult::with_errors(program, self.errors.clone())
                }
            }
            Err(_) => {
                MultipleParserResult::failure(self.errors.clone())
            }
        }
    }
}
