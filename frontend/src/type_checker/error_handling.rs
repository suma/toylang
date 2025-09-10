use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError, SourceLocation};

/// Enhanced error handling and diagnostic capabilities for type checker
pub trait ErrorHandling {
    /// Create a detailed type mismatch error with context information
    fn create_detailed_type_mismatch_error(&self, expected: TypeDecl, actual: TypeDecl, operation: &str, location: Option<SourceLocation>) -> TypeCheckError;
    
    /// Create an undefined variable error with context
    fn create_undefined_variable_error(&self, var_name: DefaultSymbol, context: &str) -> TypeCheckError;
    
    /// Create a method-related error with detailed information
    fn create_method_error(&self, method_name: DefaultSymbol, obj_type: &TypeDecl, reason: &str, context: Option<&str>) -> TypeCheckError;
    
    /// Create an unsupported operation error with type information
    fn create_unsupported_operation_error(&self, operation: &str, type_decl: &TypeDecl, context: Option<&str>) -> TypeCheckError;
    
    /// Create a generic error with enhanced context and location information
    fn create_contextual_error(&self, message: &str, context: &str, location: Option<SourceLocation>) -> TypeCheckError;
    
    /// Wrap and enhance existing errors with additional context
    fn wrap_error_with_context(&self, original_error: TypeCheckError, context: &str) -> TypeCheckError;
    
    /// Create array-related errors with detailed information
    fn create_array_error(&self, message: &str, array_type: Option<&TypeDecl>, index_type: Option<&TypeDecl>) -> TypeCheckError;
    
    /// Create field access errors with struct and field information
    fn create_field_access_error(&self, struct_name: DefaultSymbol, field_name: DefaultSymbol, reason: &str) -> TypeCheckError;
    
    /// Create conversion errors with detailed type information
    fn create_conversion_error(&self, from_type: &TypeDecl, to_type: &TypeDecl, context: &str) -> TypeCheckError;
    
    /// Create function call errors with parameter information
    fn create_function_call_error(&self, function_name: DefaultSymbol, expected_params: usize, actual_params: usize, context: &str) -> TypeCheckError;
    
    /// Add source location information to errors when available
    fn add_source_location(&self, error: TypeCheckError, expr_ref: Option<&ExprRef>) -> TypeCheckError;
    
    /// Format type information for error messages
    fn format_type_for_error(&self, type_decl: &TypeDecl) -> String;
    
    /// Check if error can be enhanced with additional diagnostic information
    fn can_enhance_error(&self, error: &TypeCheckError) -> bool;
    
    /// Enhance error with debugging information in development mode
    fn enhance_error_with_debug_info(&self, error: TypeCheckError, debug_info: &str) -> TypeCheckError;
}

/// Implementation of enhanced error handling for TypeCheckerVisitor
impl<'a> ErrorHandling for TypeCheckerVisitor<'a> {
    /// Create a detailed type mismatch error with context information
    fn create_detailed_type_mismatch_error(&self, expected: TypeDecl, actual: TypeDecl, operation: &str, location: Option<SourceLocation>) -> TypeCheckError {
        let mut error = if operation.is_empty() {
            TypeCheckError::type_mismatch(expected, actual)
        } else {
            TypeCheckError::type_mismatch_operation(operation, expected, actual)
        };
        
        if let Some(loc) = location {
            error = error.with_location(loc);
        }
        
        error
    }
    
    /// Create an undefined variable error with context
    fn create_undefined_variable_error(&self, var_name: DefaultSymbol, context: &str) -> TypeCheckError {
        let var_name_str = self.core.string_interner.resolve(var_name).unwrap_or("<unknown>");
        TypeCheckError::not_found("variable", var_name_str)
            .with_context(context)
    }
    
    /// Create a method-related error with detailed information
    fn create_method_error(&self, method_name: DefaultSymbol, obj_type: &TypeDecl, reason: &str, context: Option<&str>) -> TypeCheckError {
        let method_name_str = self.core.string_interner.resolve(method_name).unwrap_or("<unknown>");
        let mut error = TypeCheckError::method_error(method_name_str, obj_type.clone(), reason);
        
        if let Some(ctx) = context {
            error = error.with_context(ctx);
        }
        
        error
    }
    
    /// Create an unsupported operation error with type information
    fn create_unsupported_operation_error(&self, operation: &str, type_decl: &TypeDecl, context: Option<&str>) -> TypeCheckError {
        let mut error = TypeCheckError::unsupported_operation(operation, type_decl.clone());
        
        if let Some(ctx) = context {
            error = error.with_context(ctx);
        }
        
        error
    }
    
    /// Create a generic error with enhanced context and location information
    fn create_contextual_error(&self, message: &str, context: &str, location: Option<SourceLocation>) -> TypeCheckError {
        let mut error = TypeCheckError::generic_error(message).with_context(context);
        
        if let Some(loc) = location {
            error = error.with_location(loc);
        }
        
        error
    }
    
    /// Wrap and enhance existing errors with additional context
    fn wrap_error_with_context(&self, original_error: TypeCheckError, context: &str) -> TypeCheckError {
        // If error already has context, append to it
        if let Some(existing_context) = original_error.context.clone() {
            original_error.with_context(&format!("{} ({})", context, existing_context))
        } else {
            original_error.with_context(context)
        }
    }
    
    /// Create array-related errors with detailed information
    fn create_array_error(&self, message: &str, array_type: Option<&TypeDecl>, index_type: Option<&TypeDecl>) -> TypeCheckError {
        let detailed_message = match (array_type, index_type) {
            (Some(arr_ty), Some(idx_ty)) => {
                format!("{} (array type: {}, index type: {})", 
                       message, 
                       self.format_type_for_error(arr_ty),
                       self.format_type_for_error(idx_ty))
            },
            (Some(arr_ty), None) => {
                format!("{} (array type: {})", message, self.format_type_for_error(arr_ty))
            },
            (None, Some(idx_ty)) => {
                format!("{} (index type: {})", message, self.format_type_for_error(idx_ty))
            },
            (None, None) => message.to_string(),
        };
        
        TypeCheckError::array_error(&detailed_message)
    }
    
    /// Create field access errors with struct and field information
    fn create_field_access_error(&self, struct_name: DefaultSymbol, field_name: DefaultSymbol, reason: &str) -> TypeCheckError {
        let struct_name_str = self.core.string_interner.resolve(struct_name).unwrap_or("<unknown>");
        let field_name_str = self.core.string_interner.resolve(field_name).unwrap_or("<unknown>");
        
        TypeCheckError::not_found("field", field_name_str)
            .with_context(&format!("in struct '{}': {}", struct_name_str, reason))
    }
    
    /// Create conversion errors with detailed type information
    fn create_conversion_error(&self, from_type: &TypeDecl, to_type: &TypeDecl, context: &str) -> TypeCheckError {
        TypeCheckError::conversion_error(
            &self.format_type_for_error(from_type),
            &self.format_type_for_error(to_type)
        ).with_context(context)
    }
    
    /// Create function call errors with parameter information
    fn create_function_call_error(&self, function_name: DefaultSymbol, expected_params: usize, actual_params: usize, context: &str) -> TypeCheckError {
        let function_name_str = self.core.string_interner.resolve(function_name).unwrap_or("<unknown>");
        let message = format!(
            "function '{}' expects {} parameters, but {} were provided",
            function_name_str, expected_params, actual_params
        );
        
        TypeCheckError::generic_error(&message).with_context(context)
    }
    
    /// Add source location information to errors when available
    fn add_source_location(&self, mut error: TypeCheckError, expr_ref: Option<&ExprRef>) -> TypeCheckError {
        if let Some(expr) = expr_ref {
            if let Some(location) = self.get_expr_location(expr) {
                error = error.with_location(location);
            }
        }
        error
    }
    
    /// Format type information for error messages
    fn format_type_for_error(&self, type_decl: &TypeDecl) -> String {
        match type_decl {
            TypeDecl::Int64 => "i64".to_string(),
            TypeDecl::UInt64 => "u64".to_string(),
            TypeDecl::Bool => "bool".to_string(),
            TypeDecl::String => "str".to_string(),
            TypeDecl::Unit => "()".to_string(),
            TypeDecl::Array(element_types, size) => {
                if element_types.len() == 1 {
                    format!("[{}; {}]", self.format_type_for_error(&element_types[0]), size)
                } else {
                    format!("[mixed; {}]", size)
                }
            },
            TypeDecl::Tuple(types) => {
                let type_strs: Vec<String> = types.iter()
                    .map(|t| self.format_type_for_error(t))
                    .collect();
                format!("({})", type_strs.join(", "))
            },
            TypeDecl::Dict(key_type, value_type) => {
                format!("Dict<{}, {}>", 
                       self.format_type_for_error(key_type), 
                       self.format_type_for_error(value_type))
            },
            TypeDecl::Struct(name, type_params) => {
                let name_str = self.core.string_interner.resolve(*name).unwrap_or("<unknown>");
                if type_params.is_empty() {
                    name_str.to_string()
                } else {
                    let param_strs: Vec<String> = type_params.iter()
                        .map(|t| self.format_type_for_error(t))
                        .collect();
                    format!("{}<{}>", name_str, param_strs.join(", "))
                }
            },
            TypeDecl::Generic(param) => {
                let param_str = self.core.string_interner.resolve(*param).unwrap_or("<unknown>");
                format!("Generic({})", param_str)
            },
            TypeDecl::Self_ => "Self".to_string(),
            TypeDecl::Identifier(name) => {
                let name_str = self.core.string_interner.resolve(*name).unwrap_or("<unknown>");
                format!("Identifier({})", name_str)
            },
            TypeDecl::Unknown => "Unknown".to_string(),
            TypeDecl::Number => "Number".to_string(),
            TypeDecl::Ptr => "Ptr".to_string(),
        }
    }
    
    /// Check if error can be enhanced with additional diagnostic information
    fn can_enhance_error(&self, error: &TypeCheckError) -> bool {
        error.context.is_none() || error.location.is_none()
    }
    
    /// Enhance error with debugging information in development mode
    fn enhance_error_with_debug_info(&self, mut error: TypeCheckError, debug_info: &str) -> TypeCheckError {
        if cfg!(debug_assertions) {
            let enhanced_context = if let Some(existing) = &error.context {
                format!("{} [DEBUG: {}]", existing, debug_info)
            } else {
                format!("DEBUG: {}", debug_info)
            };
            error = error.with_context(&enhanced_context);
        }
        error
    }
}

