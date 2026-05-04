use std::collections::HashSet;
use string_interner::DefaultSymbol;
use crate::token::Kind;
use crate::type_decl::*;
use crate::parser::error::{ParserError, ParserResult};
use super::core::Parser;

impl<'a> Parser<'a> {
    pub fn parse_type_declaration(&mut self) -> ParserResult<TypeDecl> {
        self.parse_type_declaration_with_generic_context(&HashSet::new())
    }

    pub fn parse_type_declaration_with_generic_context(&mut self, generic_params: &HashSet<DefaultSymbol>) -> ParserResult<TypeDecl> {
        match self.peek() {
            // REF-Stage-2: `&T` and `&mut T` reference types at any
            // type-annotation position (parameter type, val annotation,
            // return type, struct field type, ...). The `&self` /
            // `&mut self` receiver path lives in
            // `parser/stmt.rs::parse_method_signature` and is not
            // reached from here, so the two arms don't conflict.
            Some(Kind::And) => {
                self.next(); // consume `&`
                let is_mut = if self.peek() == Some(&Kind::Mut) {
                    self.next(); // consume `mut`
                    true
                } else {
                    false
                };
                let inner = self.parse_type_declaration_with_generic_context(generic_params)?;
                Ok(TypeDecl::Ref { is_mut, inner: Box::new(inner) })
            }
            Some(Kind::BracketOpen) => {
                self.next();
                let element_type = self.parse_type_declaration_with_generic_context(generic_params)?;

                // Check for semicolon - if present, parse size; if not, it's a dynamic array [T]
                if self.peek() == Some(&Kind::Semicolon) {
                    self.next(); // consume semicolon

                    let size = match self.peek().cloned() {
                        Some(Kind::UInt64(n)) => {
                            self.next();
                            n as usize
                        }
                        Some(Kind::Integer(s)) => {
                            self.next();
                            s.parse::<usize>().map_err(|_| {
                                let location = self.current_source_location();
                                ParserError::generic_error(location, format!("Invalid array size: {}", s))
                            })?
                        }
                        _ => {
                            let location = self.current_source_location();
                            return Err(ParserError::generic_error(location, "Expected array size or underscore".to_string()))
                        }
                    };

                    self.expect_err(&Kind::BracketClose)?;
                    Ok(TypeDecl::Array(vec![element_type; size], size))
                } else {
                    // Dynamic array type [T] with no size specified
                    self.expect_err(&Kind::BracketClose)?;
                    Ok(TypeDecl::Array(vec![element_type], 0))
                }
            }
            Some(Kind::Bool) => {
                self.next();
                Ok(TypeDecl::Bool)
            }
            Some(Kind::U64) => {
                self.next();
                Ok(TypeDecl::UInt64)
            }
            Some(Kind::I64) => {
                self.next();
                Ok(TypeDecl::Int64)
            }
            Some(Kind::U32) => {
                self.next();
                Ok(TypeDecl::UInt32)
            }
            Some(Kind::I32) => {
                self.next();
                Ok(TypeDecl::Int32)
            }
            Some(Kind::U16) => {
                self.next();
                Ok(TypeDecl::UInt16)
            }
            Some(Kind::I16) => {
                self.next();
                Ok(TypeDecl::Int16)
            }
            Some(Kind::U8) => {
                self.next();
                Ok(TypeDecl::UInt8)
            }
            Some(Kind::I8) => {
                self.next();
                Ok(TypeDecl::Int8)
            }
            Some(Kind::F64) => {
                self.next();
                Ok(TypeDecl::Float64)
            }
            Some(Kind::Ptr) => {
                self.next();
                Ok(TypeDecl::Ptr)
            }
            Some(Kind::Identifier(s)) => {
                let s_owned = s.to_string();
                let ident = self.string_interner.get_or_intern(s_owned.clone());
                self.next();

                // Check if this identifier is a generic type parameter
                if generic_params.contains(&ident) {
                    return Ok(TypeDecl::Generic(ident));
                }

                // `Allocator` is a built-in opaque type for the allocator handle.
                // Treat the bare identifier as the built-in type so bounds like
                // `<A: Allocator>` work without introducing a full keyword.
                if s_owned == "Allocator" {
                    return Ok(TypeDecl::Allocator);
                }

                // Check if this is a generic struct with type arguments: Container<T>
                if matches!(self.peek(), Some(Kind::LT)) {
                    self.expect_err(&Kind::LT)?;

                    let mut type_args = Vec::new();
                    loop {
                        // Parse each type argument recursively
                        let type_arg = self.parse_type_declaration_with_generic_context(generic_params)?;
                        type_args.push(type_arg);

                        match self.peek() {
                            Some(Kind::Comma) => {
                                self.next(); // consume comma, continue to next type arg
                            }
                            Some(Kind::GT) => {
                                break; // end of type arguments
                            }
                            Some(Kind::RightShift) => {
                                // C++11 style: treat >> as two > tokens for nested generics
                                self.next(); // consume >>
                                // Insert TWO GT tokens: one for this level, one for outer level
                                self.insert_token(Kind::GT); // for outer level (consumed second)
                                self.insert_token(Kind::GT); // for this level (consumed first)
                                break; // treat first > as closing this type argument list
                            }
                            _ => {
                                let location = self.current_source_location();
                                return Err(ParserError::generic_error(
                                    location,
                                    "Expected ',' or '>' in generic type arguments".to_string(),
                                ));
                            }
                        }
                    }

                    self.expect_err(&Kind::GT)?;
                    Ok(TypeDecl::Struct(ident, type_args))
                } else {
                    // No type arguments, just an identifier — first
                    // check if a top-level `type Name = ...` alias
                    // resolves it. Aliases are eagerly substituted so
                    // downstream layers see the fully-expanded type and
                    // need no special handling. Generic positional
                    // aliases (`type Pair<T> = (T, T)`) are out of scope;
                    // we only substitute when the identifier appears
                    // bare (no `<...>`).
                    if let Some(target) = self.type_aliases.get(&ident) {
                        return Ok(target.clone());
                    }
                    Ok(TypeDecl::Identifier(ident))
                }
            }
            Some(Kind::Str) => {
                self.next();
                Ok(TypeDecl::String)
            }
            Some(Kind::Self_) => {
                self.next();
                Ok(TypeDecl::Self_)
            }
            Some(Kind::Dict) => {
                self.next();
                self.expect_err(&Kind::BracketOpen)?;

                let key_type = self.parse_type_declaration_with_generic_context(generic_params)?;

                self.expect_err(&Kind::Comma)?;

                let value_type = self.parse_type_declaration_with_generic_context(generic_params)?;

                self.expect_err(&Kind::BracketClose)?;
                Ok(TypeDecl::Dict(Box::new(key_type), Box::new(value_type)))
            }
            Some(Kind::ParenOpen) => {
                // Parse tuple type: (type1, type2, ...)
                self.next();
                self.skip_newlines();

                // Handle empty tuple: ()
                if self.peek() == Some(&Kind::ParenClose) {
                    self.next();
                    return Ok(TypeDecl::Tuple(vec![]));
                }

                let mut element_types = Vec::new();

                // Parse first type
                let first_type = self.parse_type_declaration_with_generic_context(generic_params)?;
                element_types.push(first_type);
                self.skip_newlines();

                // Parse remaining types
                while self.peek() == Some(&Kind::Comma) {
                    self.next(); // consume comma
                    self.skip_newlines();

                    // Allow trailing comma
                    if self.peek() == Some(&Kind::ParenClose) {
                        break;
                    }

                    let elem_type = self.parse_type_declaration_with_generic_context(generic_params)?;
                    element_types.push(elem_type);
                    self.skip_newlines();
                }

                self.expect_err(&Kind::ParenClose)?;
                Ok(TypeDecl::Tuple(element_types))
            }
            Some(_) | None => {
                let location = self.current_source_location();
                Err(ParserError::generic_error(location, format!("parse_type_declaration: unexpected token {:?}", self.peek())))
            }
        }
    }

    /// Parse `T1, T2, ...>` after the opening `<` has been consumed.
    /// Used by impl-target parsing to capture concrete type args
    /// (e.g., `<u8>` in `impl FromStr for Vec<u8>`).
    pub(super) fn parse_type_args_after_lt(
        &mut self,
        generic_params: &HashSet<DefaultSymbol>,
    ) -> ParserResult<Vec<TypeDecl>> {
        let mut type_args = Vec::new();
        loop {
            let type_arg = self.parse_type_declaration_with_generic_context(generic_params)?;
            type_args.push(type_arg);
            match self.peek() {
                Some(Kind::Comma) => {
                    self.next();
                }
                Some(Kind::GT) => {
                    self.next(); // consume '>'
                    break;
                }
                Some(Kind::RightShift) => {
                    self.next(); // consume '>>'
                    self.insert_token(Kind::GT); // outer level
                    break;
                }
                _ => {
                    let location = self.current_source_location();
                    return Err(ParserError::generic_error(
                        location,
                        "Expected ',' or '>' in impl-target type arguments".to_string(),
                    ));
                }
            }
        }
        Ok(type_args)
    }

    /// Skip tokens until matching '>' is found (for generic argument parsing)
    pub(super) fn skip_until_matching_gt(&mut self) {
        let mut depth = 1;
        self.next(); // Skip the initial '<'

        while let Some(token) = self.peek() {
            match token {
                Kind::LT => depth += 1,
                Kind::GT => {
                    depth -= 1;
                    if depth == 0 {
                        self.next(); // Consume the matching '>'
                        break;
                    }
                }
                Kind::EOF => break,
                _ => {}
            }
            self.next();
        }
    }
}
