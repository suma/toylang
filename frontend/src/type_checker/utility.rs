use std::collections::HashMap;
use std::rc::Rc;
use string_interner::{DefaultSymbol, DefaultStringInterner};
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{TypeCheckerVisitor, TypeCheckError, SourceLocation};

/// Utility methods for TypeCheckerVisitor
impl<'a> TypeCheckerVisitor<'a> {
    /// Calculate line and column from offset position in source code
    pub fn calculate_line_col_from_offset(&self, offset: usize) -> (u32, u32) {
        if let Some(source) = self.source_code {
            let mut line = 1u32;
            let mut column = 1u32;
            
            for (i, ch) in source.char_indices() {
                if i >= offset {
                    break;
                }
                if ch == '\n' {
                    line += 1;
                    column = 1;
                } else {
                    column += 1;
                }
            }
            
            (line, column)
        } else {
            // Fallback if source code is not available
            (1, 1)
        }
    }
    
    /// Create SourceLocation from Node with calculated line and column
    pub fn node_to_source_location(&self, node: &Node) -> SourceLocation {
        let (line, column) = self.calculate_line_col_from_offset(node.start);
        SourceLocation {
            line,
            column,
            offset: node.start as u32,
        }
    }
    
    /// Get expression location from pool
    pub fn get_expr_location(&self, expr_ref: &ExprRef) -> Option<SourceLocation> {
        self.core.location_pool.get_expr_location(expr_ref).cloned()
    }
    
    /// Get statement location from pool
    pub fn get_stmt_location(&self, stmt_ref: &StmtRef) -> Option<SourceLocation> {
        self.core.location_pool.get_stmt_location(stmt_ref).cloned()
    }
    
    /// Helper method to resolve symbol names safely
    pub fn resolve_symbol_name(&self, symbol: DefaultSymbol) -> &str {
        self.core.string_interner.resolve(symbol).unwrap_or("<unknown>")
    }
    
    /// Handle shift operations type resolution
    pub fn resolve_shift_operand_types(&self, lhs_ty: &TypeDecl, rhs_ty: &TypeDecl) -> (TypeDecl, TypeDecl) {
        // For shift operations, right operand must be UInt64
        let resolved_rhs = if *rhs_ty == TypeDecl::Number {
            TypeDecl::UInt64
        } else {
            rhs_ty.clone()
        };
        
        // Left operand can be Int64 or UInt64
        let resolved_lhs = if *lhs_ty == TypeDecl::Number {
            // Default to UInt64 for Number type on left side
            if let Some(hint) = &self.type_inference.type_hint {
                match hint {
                    TypeDecl::Int64 => TypeDecl::Int64,
                    TypeDecl::UInt64 => TypeDecl::UInt64,
                    _ => TypeDecl::UInt64,
                }
            } else {
                TypeDecl::UInt64
            }
        } else {
            lhs_ty.clone()
        };
        
        (resolved_lhs, resolved_rhs)
    }
    
    /// Add location information to an error if available
    pub fn error_with_location(&self, mut error: TypeCheckError, expr: &ExprRef) -> TypeCheckError {
        if error.location.is_none() {
            if let Some(location) = self.get_expr_location(expr) {
                error = error.with_location(location);
            }
        }
        error
    }
    
    /// Check if two types are compatible for assignment/operations
    pub fn are_types_compatible(&self, expected: &TypeDecl, actual: &TypeDecl) -> bool {
        if expected == actual {
            return true;
        }
        
        // Handle explicit type conversions that are allowed
        match (expected, actual) {
            // Number type can be converted to numeric types
            (TypeDecl::UInt64, TypeDecl::Number) | (TypeDecl::Int64, TypeDecl::Number) => true,
            (TypeDecl::Number, TypeDecl::UInt64) | (TypeDecl::Number, TypeDecl::Int64) => true,
            (TypeDecl::Number, TypeDecl::Number) => true,
            
            // Generic types are compatible with any type during type inference
            (TypeDecl::Generic(_), _) | (_, TypeDecl::Generic(_)) => true,
            
            // Unknown types are only compatible in limited contexts
            (TypeDecl::Unknown, _) => true,  // Unknown can accept any value
            (_, TypeDecl::Unknown) => false, // But we can't convert any type to Unknown
            
            // Numeric conversions between compatible types
            (TypeDecl::UInt64, TypeDecl::Int64) => true,  // Allow signed/unsigned conversion
            (TypeDecl::Int64, TypeDecl::UInt64) => true,  // Allow signed/unsigned conversion
            
            // No other implicit conversions allowed (including bool -> numeric)
            _ => false,
        }
    }
    
    /// Handle array slice assignment (both single element and range)
    pub fn handle_array_slice_assign(&mut self, element_types: &Vec<TypeDecl>, start: &Option<ExprRef>, end: &Option<ExprRef>, value_type: &TypeDecl) -> Result<TypeDecl, TypeCheckError> {
        if start.is_some() && end.is_none() {
            // Single element assignment: arr[index] = value
            if element_types.len() == 1 {
                let element_type = &element_types[0];
                if element_type != value_type && !self.are_types_compatible(element_type, value_type) {
                    return Err(TypeCheckError::type_mismatch(
                        element_type.clone(),
                        value_type.clone()
                    ));
                }
            }
        } else {
            // Range assignment: arr[start..end] = value or arr[start..] = value
            // Value must be an array with compatible element types
            match value_type {
                TypeDecl::Array(value_elements, _) => {
                    if element_types.len() == 1 && value_elements.len() == 1 {
                        let element_type = &element_types[0];
                        let value_element = &value_elements[0];
                        if element_type != value_element && !self.are_types_compatible(element_type, value_element) {
                            return Err(TypeCheckError::type_mismatch(
                                element_type.clone(),
                                value_element.clone()
                            ));
                        }
                    }
                }
                _ => {
                    return Err(TypeCheckError::type_mismatch(
                        TypeDecl::Array(element_types.clone(), 0),
                        value_type.clone()
                    ));
                }
            }
        }
        
        Ok(TypeDecl::Unit)
    }
    
    /// Context management utilities
    pub fn push_context(&mut self) {
        self.context.vars.push(HashMap::new());
    }

    pub fn pop_context(&mut self) {
        self.context.vars.pop();
    }

    pub fn add_function(&mut self, f: Rc<Function>) {
        self.context.set_fn(f.name, f.clone());
    }
    
    /// Extract expression type mappings after type checking
    pub fn get_expr_types(&self) -> HashMap<crate::ast::ExprRef, crate::type_decl::TypeDecl> {
        // Return a clone of the comprehensive expr_types mapping
        self.type_inference.expr_types.clone()
    }
    
    /// Get human-readable type name for error messages
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
                    format!("[{}; {}]", self.type_name_for_error(&element_types[0]), size)
                } else {
                    format!("[{:?}; {}]", element_types, size)
                }
            },
            TypeDecl::Struct(name, _) => {
                self.core.string_interner.resolve(*name)
                    .unwrap_or("struct")
                    .to_string()
            },
            TypeDecl::Dict(key_type, value_type) => {
                format!("dict<{}, {}>", 
                    self.type_name_for_error(key_type), 
                    self.type_name_for_error(value_type))
            },
            _ => format!("{:?}", type_decl).to_lowercase(),
        }
    }
    
    /// Get struct variable mappings for debugging/analysis
    pub fn get_struct_var_mappings(&self, interner: &DefaultStringInterner) -> HashMap<DefaultSymbol, String> {
        let mut mappings = HashMap::new();
        
        // Iterate through all struct definitions
        for (struct_symbol, struct_def) in &self.context.struct_definitions {
            if let Some(struct_name) = interner.resolve(*struct_symbol) {
                for field in &struct_def.fields {
                    if let Some(field_symbol) = interner.get(&field.name) {
                        mappings.insert(field_symbol, format!("{}.{}", struct_name, field.name));
                    }
                }
            }
        }
        
        mappings
    }
}