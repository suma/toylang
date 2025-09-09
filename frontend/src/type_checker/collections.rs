use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError
};

/// Collections type checking implementation (arrays, dictionaries, tuples, slices)
impl<'a> TypeCheckerVisitor<'a> {
    /// Type check array literals
    pub fn visit_array_literal_new(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        if elements.is_empty() {
            return Err(TypeCheckError::array_error("Empty array literals are not supported"));
        }

        // Check recursion depth to prevent stack overflow
        if self.type_inference.recursion_depth >= self.type_inference.max_recursion_depth {
            return Err(TypeCheckError::generic_error(
                "Maximum recursion depth reached in array literal type inference - possible circular reference"
            ));
        }
        
        self.type_inference.recursion_depth += 1;
        
        // Execute the main logic and capture result
        let result = self.visit_array_literal_impl_new(elements);
        
        // Always decrement recursion depth before returning
        self.type_inference.recursion_depth -= 1;
        
        result
    }

    /// Implementation of array literal type checking
    pub fn visit_array_literal_impl_new(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        if elements.is_empty() {
            return Err(TypeCheckError::array_error("Empty array literals are not supported"));
        }

        // Determine element type from first element
        let first_element_type = self.visit_expr(&elements[0])?;
        
        // Check all elements have the same type
        for (i, element) in elements.iter().enumerate().skip(1) {
            let element_type = self.visit_expr(element)?;
            if element_type != first_element_type && element_type != TypeDecl::Unknown && first_element_type != TypeDecl::Unknown {
                return Err(TypeCheckError::generic_error(&format!(
                    "Array element type mismatch: element {} has type {:?}, expected {:?}",
                    i, element_type, first_element_type
                )));
            }
        }

        // Return array type with the determined element type and size
        Ok(TypeDecl::Array(vec![first_element_type], elements.len()))
    }

    /// Type check slice access
    pub fn visit_slice_access_new(&mut self, object: &ExprRef, slice_info: &SliceInfo) -> Result<TypeDecl, TypeCheckError> {
        let object_type = self.visit_expr(object)?;
        
        match object_type {
            TypeDecl::Array(element_types, _) => {
                // Validate slice indices
                if let Some(ref start) = slice_info.start {
                    let start_type = self.visit_expr(start)?;
                    if start_type != TypeDecl::Int64 && start_type != TypeDecl::UInt64 {
                        return Err(TypeCheckError::type_mismatch_operation(
                            "slice start index",
                            start_type,
                            TypeDecl::Int64
                        ));
                    }
                }
                
                if let Some(ref end) = slice_info.end {
                    let end_type = self.visit_expr(end)?;
                    if end_type != TypeDecl::Int64 && end_type != TypeDecl::UInt64 {
                        return Err(TypeCheckError::type_mismatch_operation(
                            "slice end index",
                            end_type,
                            TypeDecl::Int64
                        ));
                    }
                }
                
                // Return array type with same element types but unknown size
                Ok(TypeDecl::Array(element_types, 0))
            }
            TypeDecl::String => {
                // String slicing
                if let Some(ref start) = slice_info.start {
                    let start_type = self.visit_expr(start)?;
                    if start_type != TypeDecl::Int64 && start_type != TypeDecl::UInt64 {
                        return Err(TypeCheckError::type_mismatch_operation(
                            "slice start index",
                            start_type,
                            TypeDecl::Int64
                        ));
                    }
                }
                
                if let Some(ref end) = slice_info.end {
                    let end_type = self.visit_expr(end)?;
                    if end_type != TypeDecl::Int64 && end_type != TypeDecl::UInt64 {
                        return Err(TypeCheckError::type_mismatch_operation(
                            "slice end index",
                            end_type,
                            TypeDecl::Int64
                        ));
                    }
                }
                
                Ok(TypeDecl::String)
            }
            _ => {
                Err(TypeCheckError::type_mismatch_operation(
                    "slice access",
                    object_type,
                    TypeDecl::Array(vec![TypeDecl::Unknown], 0)
                ))
            }
        }
    }

    /// Type check slice assignment
    pub fn visit_slice_assign_new(&mut self, object: &ExprRef, start: &Option<ExprRef>, end: &Option<ExprRef>, value: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let object_type = self.visit_expr(object)?;
        let value_type = self.visit_expr(value)?;
        
        // Validate indices
        if let Some(start_expr) = start {
            let start_type = self.visit_expr(start_expr)?;
            if start_type != TypeDecl::Int64 && start_type != TypeDecl::UInt64 {
                return Err(TypeCheckError::type_mismatch_operation(
                    "slice assignment start index",
                    start_type,
                    TypeDecl::Int64
                ));
            }
        }
        
        if let Some(end_expr) = end {
            let end_type = self.visit_expr(end_expr)?;
            if end_type != TypeDecl::Int64 && end_type != TypeDecl::UInt64 {
                return Err(TypeCheckError::type_mismatch_operation(
                    "slice assignment end index",
                    end_type,
                    TypeDecl::Int64
                ));
            }
        }
        
        // Check compatibility
        match object_type {
            TypeDecl::Array(element_types, _) => {
                let element_type = element_types.first().unwrap_or(&TypeDecl::Unknown);
                if value_type != *element_type && value_type != TypeDecl::Unknown {
                    return Err(TypeCheckError::type_mismatch_operation(
                        "slice assignment value",
                        value_type,
                        element_type.clone()
                    ));
                }
                Ok(TypeDecl::Unit)
            }
            _ => {
                Err(TypeCheckError::type_mismatch_operation(
                    "slice assignment target",
                    object_type,
                    TypeDecl::Array(vec![TypeDecl::Unknown], 0)
                ))
            }
        }
    }

    /// Type check dictionary literals
    pub fn visit_dict_literal_new(&mut self, entries: &Vec<(ExprRef, ExprRef)>) -> Result<TypeDecl, TypeCheckError> {
        if entries.is_empty() {
            // Empty dict - type will be inferred from usage or type hint
            if let Some(TypeDecl::Dict(key_type, value_type)) = &self.type_inference.type_hint {
                return Ok(TypeDecl::Dict(key_type.clone(), value_type.clone()));
            }
            return Ok(TypeDecl::Dict(Box::new(TypeDecl::Unknown), Box::new(TypeDecl::Unknown)));
        }
        
        // Save the original type hint to restore later
        let original_hint = self.type_inference.type_hint.clone();
        
        // Extract expected types from type hint if available
        let (expected_key_type, expected_value_type) = if let Some(TypeDecl::Dict(key_type, value_type)) = &self.type_inference.type_hint {
            (Some(key_type.as_ref().clone()), Some(value_type.as_ref().clone()))
        } else {
            (None, None)
        };
        
        // Check first entry to determine key and value types
        let (first_key, first_value) = &entries[0];
        
        // Set type hints for key and value if we have them
        if let Some(expected_key) = &expected_key_type {
            self.type_inference.type_hint = Some(expected_key.clone());
        }
        let key_type = self.visit_expr(first_key)?;
        
        if let Some(expected_value) = &expected_value_type {
            self.type_inference.type_hint = Some(expected_value.clone());
        }
        let value_type = self.visit_expr(first_value)?;
        
        // Restore original hint
        self.type_inference.type_hint = original_hint;
        
        // Determine final types
        let final_key_type = if key_type == TypeDecl::Unknown && expected_key_type.is_some() {
            expected_key_type.unwrap()
        } else if key_type == TypeDecl::Number {
            TypeDecl::UInt64  // Default numeric type for keys
        } else {
            key_type
        };
        
        let final_value_type = if value_type == TypeDecl::Unknown && expected_value_type.is_some() {
            expected_value_type.unwrap()
        } else if value_type == TypeDecl::Number {
            TypeDecl::UInt64  // Default numeric type for values
        } else {
            value_type
        };
        
        // Validate all entries have consistent types
        for (i, (key, value)) in entries.iter().enumerate().skip(1) {
            let entry_key_type = self.visit_expr(key)?;
            let entry_value_type = self.visit_expr(value)?;
            
            if entry_key_type != final_key_type && entry_key_type != TypeDecl::Unknown {
                return Err(TypeCheckError::generic_error(&format!(
                    "Dictionary key type mismatch at entry {}: expected {:?}, found {:?}",
                    i, final_key_type, entry_key_type
                )));
            }
            
            if entry_value_type != final_value_type && entry_value_type != TypeDecl::Unknown {
                return Err(TypeCheckError::generic_error(&format!(
                    "Dictionary value type mismatch at entry {}: expected {:?}, found {:?}",
                    i, final_value_type, entry_value_type
                )));
            }
        }
        
        Ok(TypeDecl::Dict(Box::new(final_key_type), Box::new(final_value_type)))
    }

    /// Type check tuple literals
    pub fn visit_tuple_literal_new(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        if elements.is_empty() {
            return Ok(TypeDecl::Tuple(vec![]));
        }

        let mut element_types = Vec::new();
        for element in elements {
            let element_type = self.visit_expr(element)?;
            element_types.push(element_type);
        }

        Ok(TypeDecl::Tuple(element_types))
    }

    /// Type check tuple access
    pub fn visit_tuple_access_new(&mut self, tuple: &ExprRef, index: usize) -> Result<TypeDecl, TypeCheckError> {
        let tuple_type = self.visit_expr(tuple)?;
        
        match tuple_type {
            TypeDecl::Tuple(element_types) => {
                if index >= element_types.len() {
                    return Err(TypeCheckError::generic_error(&format!(
                        "Tuple index {} out of bounds for tuple with {} elements",
                        index, element_types.len()
                    )));
                }
                Ok(element_types[index].clone())
            }
            _ => {
                Err(TypeCheckError::type_mismatch_operation(
                    "tuple access",
                    tuple_type,
                    TypeDecl::Tuple(vec![])
                ))
            }
        }
    }
}