use crate::ast::*;
use crate::type_decl::TypeDecl;
use super::{TypeCheckError, LiteralTypeChecker, TypeCheckerVisitor};
use string_interner::DefaultSymbol;

impl<'a> LiteralTypeChecker for TypeCheckerVisitor<'a> {
    fn check_int64_literal(&mut self, _value: &i64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Int64)
    }

    fn check_uint64_literal(&mut self, _value: &u64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::UInt64)
    }

    fn check_number_literal(&mut self, value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        let num_str = self.core.string_interner.resolve(value)
            .ok_or_else(|| TypeCheckError::generic_error("Failed to resolve number literal"))?;
        
        // If we have a type hint from val/var declaration, validate and return the hint type
        if let Some(hint) = self.type_inference.type_hint.clone() {
            match hint {
                TypeDecl::Int64 => {
                    if let Ok(_val) = num_str.parse::<i64>() {
                        return Ok(hint);
                    } else {
                        return Err(TypeCheckError::conversion_error(num_str, "Int64"));
                    }
                },
                TypeDecl::UInt64 => {
                    if let Ok(_val) = num_str.parse::<u64>() {
                        return Ok(hint);
                    } else {
                        return Err(TypeCheckError::conversion_error(num_str, "UInt64"));
                    }
                },
                _ => {}
            }
        }
        
        // Parse the number and determine appropriate type
        if let Ok(val) = num_str.parse::<i64>() {
            if val >= 0 && val <= (i64::MAX) {
                // Positive number that fits in both i64 and u64 - use Number for inference
                Ok(TypeDecl::Number)
            } else {
                // Negative number or very large positive - must be i64
                Ok(TypeDecl::Int64)
            }
        } else if let Ok(_val) = num_str.parse::<u64>() {
            // Very large positive number that doesn't fit in i64 - must be u64
            Ok(TypeDecl::UInt64)
        } else {
            Err(TypeCheckError::invalid_literal(num_str, "number"))
        }
    }

    fn check_string_literal(&mut self, _value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::String)
    }

    fn check_boolean_literal(&mut self, value: &Expr) -> Result<TypeDecl, TypeCheckError> {
        match value {
            Expr::True | Expr::False => Ok(TypeDecl::Bool),
            _ => Ok(TypeDecl::Bool), // Default for boolean expressions
        }
    }

    fn check_null_literal(&mut self) -> Result<TypeDecl, TypeCheckError> {
        // Null value type is determined by context
        Ok(TypeDecl::Unknown)
    }
}