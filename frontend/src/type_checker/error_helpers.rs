use crate::ast::ExprRef;
use crate::type_decl::TypeDecl;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError};

/// Error-reporting helpers for `TypeCheckerVisitor`.
impl<'a> TypeCheckerVisitor<'a> {
    /// Add location information to an error if available.
    pub fn error_with_location(
        &self,
        mut error: TypeCheckError,
        expr: &ExprRef,
    ) -> TypeCheckError {
        if error.location.is_none() {
            if let Some(location) = self.get_expr_location(expr) {
                error = error.with_location(location);
            }
        }
        error
    }

    /// Get human-readable type name for error messages.
    pub fn type_name_for_error(&self, type_decl: &TypeDecl) -> String {
        match type_decl {
            TypeDecl::Bool => "bool".to_string(),
            TypeDecl::UInt64 => "u64".to_string(),
            TypeDecl::Int64 => "i64".to_string(),
            TypeDecl::String => "string".to_string(),
            TypeDecl::Number => "number".to_string(),
            TypeDecl::Unit => "unit".to_string(),
            TypeDecl::Unknown => "unknown".to_string(),
            TypeDecl::Array(element_types, size) => {
                if element_types.len() == 1 {
                    format!(
                        "[{}; {}]",
                        self.type_name_for_error(&element_types[0]),
                        size
                    )
                } else {
                    format!("[{:?}; {}]", element_types, size)
                }
            }
            TypeDecl::Struct(name, _) => self
                .core
                .string_interner
                .resolve(*name)
                .unwrap_or("struct")
                .to_string(),
            TypeDecl::Dict(key_type, value_type) => {
                format!(
                    "dict<{}, {}>",
                    self.type_name_for_error(key_type),
                    self.type_name_for_error(value_type)
                )
            }
            _ => format!("{:?}", type_decl).to_lowercase(),
        }
    }
}
