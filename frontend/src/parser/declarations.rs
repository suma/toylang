use std::collections::{HashMap, HashSet};
use string_interner::DefaultSymbol;
use crate::ast::{ExprRef, Parameter, PackageDecl, ImportDecl};
use crate::type_decl::TypeDecl;
use crate::token::Kind;
use crate::parser::error::{ParserError, ParserResult};
use super::core::Parser;

impl<'a> Parser<'a> {
    pub fn parse_param_def(&mut self) -> ParserResult<Parameter> {
        self.parse_param_def_with_generic_context(&[])
    }

    pub fn parse_param_def_with_generic_context(&mut self, generic_params: &[DefaultSymbol]) -> ParserResult<Parameter> {
        let current_token = self.peek().cloned();
        match current_token {
            Some(Kind::Identifier(s)) => {
                let name = self.string_interner.get_or_intern(s);
                self.next();
                self.expect_err(&Kind::Colon)?;
                // Convert to HashSet for generic context
                let generic_context: HashSet<DefaultSymbol> = generic_params.iter().cloned().collect();
                let typ = self.parse_type_declaration_with_generic_context(&generic_context)?;
                Ok((name, typ))
            }
            x => {
                let location = self.current_source_location();
                Err(ParserError::generic_error(location, format!("expect type parameter of function but: {:?}", x)))
            },
        }
    }

    pub fn parse_param_def_list(&mut self, args: Vec<Parameter>) -> ParserResult<Vec<Parameter>> {
        self.parse_param_def_list_with_generic_context(args, &[])
    }

    pub fn parse_param_def_list_with_generic_context(&mut self, mut args: Vec<Parameter>, generic_params: &[DefaultSymbol]) -> ParserResult<Vec<Parameter>> {
        // Limit maximum number of parameters to prevent infinite loops
        const MAX_PARAMS: usize = 255;

        loop {
            if self.peek() == Some(&Kind::ParenClose) || args.len() >= MAX_PARAMS {
                if args.len() >= MAX_PARAMS {
                    self.collect_error(&format!("too many parameters (max: {})", MAX_PARAMS));
                }
                return Ok(args);
            }

            let def = self.parse_param_def_with_generic_context(generic_params);
            if def.is_err() {
                return Ok(args);
            }
            args.push(def?);

            match self.peek() {
                Some(Kind::Comma) => {
                    self.next();
                    // Continue loop to parse next parameter
                }
                _ => {
                    return Ok(args);
                }
            }
        }
    }

    /// Parse package declaration: package math.basic
    pub fn parse_package_decl(&mut self) -> ParserResult<PackageDecl> {
        self.expect_err(&Kind::Package)?;

        let mut name_parts = Vec::new();

        // Parse first identifier
        if let Some(Kind::Identifier(s)) = self.peek().cloned() {
            let symbol = self.string_interner.get_or_intern(s);
            name_parts.push(symbol);
            self.next();
        } else {
            return Err(ParserError::generic_error(self.current_source_location(), "expected package name".to_string()));
        }

        // Parse additional parts separated by dots
        while matches!(self.peek(), Some(Kind::Dot)) {
            self.next(); // consume dot
            if let Some(Kind::Identifier(s)) = self.peek().cloned() {
                let symbol = self.string_interner.get_or_intern(s);
                name_parts.push(symbol);
                self.next();
            } else {
                return Err(ParserError::generic_error(self.current_source_location(), "expected identifier after '.'".to_string()));
            }
        }

        self.skip_newlines();
        Ok(PackageDecl { name: name_parts })
    }

    /// Parse import declaration: import math.basic [as alias]
    pub fn parse_import_decl(&mut self) -> ParserResult<ImportDecl> {
        self.expect_err(&Kind::Import)?;

        let mut module_path = Vec::new();

        if let Some(Kind::Identifier(s)) = self.peek().cloned() {
            let symbol = self.string_interner.get_or_intern(s);
            module_path.push(symbol);
            self.next();
        } else {
            return Err(ParserError::generic_error(self.current_source_location(), "expected module name".to_string()));
        }

        while matches!(self.peek(), Some(Kind::Dot)) {
            self.next(); // consume dot
            if let Some(Kind::Identifier(s)) = self.peek().cloned() {
                let symbol = self.string_interner.get_or_intern(s);
                module_path.push(symbol);
                self.next();
            } else {
                return Err(ParserError::generic_error(self.current_source_location(), "expected identifier after '.'".to_string()));
            }
        }

        // Parse optional alias: as alias_name
        let alias = if matches!(self.peek(), Some(Kind::As)) {
            self.next(); // consume 'as'
            if let Some(Kind::Identifier(s)) = self.peek().cloned() {
                let alias_symbol = self.string_interner.get_or_intern(s);
                self.next();
                Some(alias_symbol)
            } else {
                return Err(ParserError::generic_error(self.current_source_location(), "expected alias name after 'as'".to_string()));
            }
        } else {
            None
        };

        self.skip_newlines();
        Ok(ImportDecl { module_path, alias })
    }

    /// Parse zero or more `requires <expr>` / `ensures <expr>` Design-by-
    /// Contract clauses. Called between the optional return type and the
    /// `{` body in `fn` declarations and `impl` methods. Multiple clauses of
    /// the same kind are allowed; the type checker AND-composes them.
    /// Returns `(requires, ensures)` in declaration order.
    pub fn parse_contract_clauses(&mut self) -> ParserResult<(Vec<ExprRef>, Vec<ExprRef>)> {
        let mut requires = Vec::new();
        let mut ensures = Vec::new();
        loop {
            // Tolerate blank lines between clauses; the body's `{` ends the
            // run. Newlines are not significant inside this parser.
            self.skip_newlines();
            match self.peek() {
                Some(Kind::Requires) => {
                    self.next();
                    // `Condition` context disables struct-literal parsing so
                    // the trailing `{ body }` of the function isn't mistaken
                    // for a struct literal at the tail of the predicate.
                    self.push_context(crate::parser::core::ParseContext::Condition);
                    let cond = self.parse_expr_impl();
                    self.pop_context();
                    requires.push(cond?);
                }
                Some(Kind::Ensures) => {
                    self.next();
                    self.push_context(crate::parser::core::ParseContext::Condition);
                    let cond = self.parse_expr_impl();
                    self.pop_context();
                    ensures.push(cond?);
                }
                _ => break,
            }
        }
        Ok((requires, ensures))
    }

    /// Parse generic type parameters: `<T>`, `<T, U>`, or with bounds `<T: Bound, U>`.
    /// Returns the parameter names (in declaration order) and a map of optional bounds.
    /// A parameter without a bound simply has no entry in the map.
    pub fn parse_generic_params(&mut self) -> ParserResult<(Vec<DefaultSymbol>, HashMap<DefaultSymbol, TypeDecl>)> {
        let mut params = Vec::new();
        let mut bounds: HashMap<DefaultSymbol, TypeDecl> = HashMap::new();
        let mut seen = HashSet::new();

        // Expect '<'
        self.expect_err(&Kind::LT)?;

        loop {
            match self.peek() {
                Some(Kind::Identifier(s)) => {
                    let s = s.to_string();
                    let param_symbol = self.string_interner.get_or_intern(&s);

                    // Check for duplicate type parameters
                    if !seen.insert(param_symbol) {
                        let location = self.current_source_location();
                        return Err(ParserError::generic_error(location, format!("Duplicate generic type parameter '{}'", s)));
                    }

                    params.push(param_symbol);
                    self.next();

                    // Optional bound: `: <Type>`. Currently only the concrete type
                    // after the colon is parsed; trait-like sum bounds (`A + B`)
                    // are out of scope until we have real traits.
                    if self.peek() == Some(&Kind::Colon) {
                        self.next(); // consume ':'
                        let bound_ty = self.parse_type_declaration()?;
                        bounds.insert(param_symbol, bound_ty);
                    }

                    match self.peek() {
                        Some(Kind::Comma) => {
                            self.next(); // consume comma, continue to next param
                        }
                        Some(Kind::GT) => {
                            break; // end of generic params
                        }
                        _ => {
                            let location = self.current_source_location();
                            return Err(ParserError::generic_error(location, "Expected ',' or '>' in generic parameters".to_string()));
                        }
                    }
                }
                Some(Kind::GT) => {
                    break; // empty or trailing comma case
                }
                _ => {
                    let location = self.current_source_location();
                    return Err(ParserError::generic_error(location, "Expected generic type parameter identifier".to_string()));
                }
            }
        }

        // Expect '>'
        self.expect_err(&Kind::GT)?;

        // Reject empty generic parameter lists: struct Foo<> is invalid
        if params.is_empty() {
            let location = self.current_source_location();
            return Err(ParserError::generic_error(location, "Empty generic parameter list is not allowed".to_string()));
        }

        Ok((params, bounds))
    }
}
