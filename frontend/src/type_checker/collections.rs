use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError,
    Acceptable
};

/// Collections type checking implementation (arrays, dictionaries, tuples, slices)
impl<'a> TypeCheckerVisitor<'a> {
    /// Type check slice access - implementation
    pub fn visit_slice_access_impl(&mut self, object: &ExprRef, slice_info: &SliceInfo) -> Result<TypeDecl, TypeCheckError> {
        let object_type = self.visit_expr(object)?;

        match object_type {
            TypeDecl::Array(ref element_types, _size) => {
                // Simplified type checking for slice indices
                match slice_info.slice_type {
                    SliceType::SingleElement => {
                        // For single element access, be more strict with type checking
                        if let Some(start_expr) = &slice_info.start {
                            let original_hint = self.type_inference.type_hint.clone();
                            self.type_inference.type_hint = Some(TypeDecl::Int64); // Allow negative indices
                            let start_type = self.visit_expr(start_expr)?;
                            self.type_inference.type_hint = original_hint;

                            if start_type == TypeDecl::UInt64 {
                                self.transform_numeric_expr(start_expr, &TypeDecl::Int64)?;
                            }

                            // Allow UInt64, Int64, or transform Number
                            match start_type {
                                TypeDecl::UInt64 | TypeDecl::Int64 | TypeDecl::Unknown => {
                                    // Valid types
                                }
                                TypeDecl::Number => {
                                    // Transform Number to Int64 (could be negative)
                                    self.transform_numeric_expr(start_expr, &TypeDecl::Int64)?;
                                }
                                _ => {
                                    return Err(TypeCheckError::array_error(&format!(
                                        "Array index must be an integer type, but got {:?}", start_type
                                    )));
                                }
                            }
                        }
                    }
                    SliceType::RangeSlice => {
                        // For range slices, set Int64 hint for potential negative indices
                        let original_hint = self.type_inference.type_hint.clone();
                        self.type_inference.type_hint = Some(TypeDecl::Int64);

                        // Visit start expression if present
                        if let Some(start_expr) = &slice_info.start {
                            let _ = self.visit_expr(start_expr)?;
                        }

                        // Visit end expression if present
                        if let Some(end_expr) = &slice_info.end {
                            let _ = self.visit_expr(end_expr)?;
                        }

                        // Restore original hint
                        self.type_inference.type_hint = original_hint;
                    }
                }

                if element_types.is_empty() {
                    return Err(TypeCheckError::array_error("Cannot slice empty array"));
                }

                // Use SliceInfo to distinguish single element access vs range slice
                match slice_info.slice_type {
                    SliceType::SingleElement => {
                        // Single element access: arr[i] returns element type
                        Ok(element_types[0].clone())
                    }
                    SliceType::RangeSlice => {
                        // Range slice: arr[start..end] returns array type
                        let single_element_type = element_types[0].clone();

                        // For dynamic arrays (size 0), return a dynamic array type
                        if _size == 0 {
                            // Dynamic array: return [T] (dynamic array of same element type)
                            return Ok(TypeDecl::Array(vec![single_element_type], 0));
                        }

                        // Try to calculate slice size using array size for open-ended slices
                        let array_size = _size;
                        let slice_size = self.calculate_slice_size(slice_info, array_size);

                        // If slice_size is 0, return dynamic array type
                        if slice_size == 0 {
                            return Ok(TypeDecl::Array(vec![single_element_type], 0));
                        }

                        // Create element_types with the correct number of elements
                        let result_element_types = vec![single_element_type; slice_size];
                        Ok(TypeDecl::Array(result_element_types, slice_size))
                    }
                }
            }
            TypeDecl::Dict(ref key_type, ref value_type) => {
                // Dictionary access: dict[key] (only single element access, not slicing)
                if slice_info.is_valid_for_dict() {
                    // Single element access: dict[key]
                    if let Some(index_expr) = &slice_info.start {
                        let index_type = self.visit_expr(index_expr)?;

                        // Verify the index type matches the key type
                        if index_type != **key_type {
                            return Err(TypeCheckError::type_mismatch(
                                *key_type.clone(), index_type
                            ));
                        }

                        Ok(*value_type.clone())
                    } else {
                        Err(TypeCheckError::generic_error("Dictionary access requires key index"))
                    }
                } else {
                    // Range slicing is not supported for dictionaries
                    Err(TypeCheckError::generic_error("Dictionary slicing is not supported - use single key access dict[key]"))
                }
            }
            TypeDecl::Identifier(struct_name) => {
                // Struct access: check for __getitem__ method (only single element access)
                if slice_info.is_valid_for_dict() {
                    // Single element access: struct[key]
                    if let Some(index_expr) = &slice_info.start {
                        let struct_name_str = self.core.string_interner.resolve(struct_name)
                            .ok_or_else(|| TypeCheckError::generic_error("Unknown struct name"))?;

                        // Type check the index first to avoid borrowing conflicts
                        let index_type = self.visit_expr(index_expr)?;

                        // Look for __getitem__ method
                        if let Some(getitem_method) = self.context.get_method_function_by_name(struct_name_str, "__getitem__", self.core.string_interner) {
                            // Check if method has correct signature: __getitem__(self, index: T) -> U
                            if getitem_method.parameter.len() >= 2 {
                                let index_param_type = &getitem_method.parameter[1].1;
                                if index_type != *index_param_type {
                                    return Err(TypeCheckError::type_mismatch(
                                        index_param_type.clone(), index_type
                                    ));
                                }

                                // Return the method's return type
                                if let Some(return_type) = &getitem_method.return_type {
                                    Ok(return_type.clone())
                                } else {
                                    Err(TypeCheckError::generic_error("__getitem__ method must have return type"))
                                }
                            } else {
                                Err(TypeCheckError::generic_error("__getitem__ method must have at least 2 parameters (self, index)"))
                            }
                        } else {
                            Err(TypeCheckError::generic_error(&format!(
                                "Cannot index into type {:?} - no __getitem__ method found", object_type
                            )))
                        }
                    } else {
                        Err(TypeCheckError::generic_error("Struct access requires index"))
                    }
                } else {
                    // Range slicing: check for __getslice__ method
                    self.check_struct_getslice_method(struct_name, slice_info, &object_type)
                }
            }
            TypeDecl::Struct(struct_name, ref _type_params) => {
                // Struct type access: check for __getitem__ or __getslice__ method
                let struct_name_str = self.core.string_interner.resolve(struct_name)
                    .ok_or_else(|| TypeCheckError::generic_error("Unknown struct name"))?;

                if slice_info.is_valid_for_dict() {
                    // Single element access: struct[key] - use __getitem__
                    if let Some(index_expr) = &slice_info.start {
                        let index_type = self.visit_expr(index_expr)?;

                        if let Some(getitem_method) = self.context.get_method_function_by_name(struct_name_str, "__getitem__", self.core.string_interner) {
                            if getitem_method.parameter.len() >= 2 {
                                let index_param_type = &getitem_method.parameter[1].1;
                                if index_type != *index_param_type && !self.are_types_compatible(index_param_type, &index_type) {
                                    return Err(TypeCheckError::type_mismatch(
                                        index_param_type.clone(), index_type
                                    ));
                                }

                                if let Some(return_type) = &getitem_method.return_type {
                                    Ok(return_type.clone())
                                } else {
                                    Err(TypeCheckError::generic_error("__getitem__ method must have return type"))
                                }
                            } else {
                                Err(TypeCheckError::generic_error("__getitem__ method must have at least 2 parameters (self, index)"))
                            }
                        } else {
                            Err(TypeCheckError::generic_error(&format!(
                                "Cannot index into type {:?} - no __getitem__ method found", object_type
                            )))
                        }
                    } else {
                        Err(TypeCheckError::generic_error("Struct access requires index"))
                    }
                } else {
                    // Range slicing: check for __getslice__ method
                    self.check_struct_getslice_method(struct_name, slice_info, &object_type)
                }
            }
            _ => {
                Err(TypeCheckError::generic_error(&format!(
                    "Cannot access type {:?} - only arrays, dictionaries, and structs with __getitem__ are supported", object_type
                )))
            }
        }
    }

    /// Type check slice assignment - implementation
    pub fn visit_slice_assign_impl(&mut self, object: &ExprRef, start: &Option<ExprRef>, end: &Option<ExprRef>, value: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let object_type = self.visit_expr(object)?;
        let value_type = self.visit_expr(value)?;

        match object_type {
            TypeDecl::Array(ref element_types, _size) => {
                return self.handle_array_slice_assign(element_types, start, end, &value_type);
            }
            TypeDecl::Dict(ref key_type, ref dict_value_type) => {
                // Dictionary assignment: dict[key] = value (only single element assignment)
                if start.is_some() && end.is_none() {
                    // Single element assignment: dict[key] = value
                    if let Some(key_expr) = start {
                        let key_type_result = self.visit_expr(key_expr)?;

                        // Verify the key type matches the dictionary key type
                        if key_type_result != **key_type {
                            return Err(TypeCheckError::type_mismatch(
                                *key_type.clone(), key_type_result
                            ));
                        }

                        // Check value type compatibility with dictionary value type
                        let expected_dict_value_type = &**dict_value_type;
                        if *expected_dict_value_type != TypeDecl::Unknown {
                            let resolved_value_type = if value_type == TypeDecl::Number {
                                // Transform Number value to expected dict value type
                                self.transform_numeric_expr(value, expected_dict_value_type)?;
                                expected_dict_value_type.clone()
                            } else {
                                value_type.clone()
                            };

                            if *expected_dict_value_type != resolved_value_type {
                                return Err(TypeCheckError::generic_error(&format!(
                                    "Dict value type mismatch: expected {:?}, found {:?}",
                                    expected_dict_value_type, resolved_value_type
                                )));
                            }
                            Ok(resolved_value_type)
                        } else {
                            Ok(value_type.clone())
                        }
                    } else {
                        Err(TypeCheckError::generic_error("Dictionary assignment requires key index"))
                    }
                } else {
                    // Range slice assignment not supported for dictionaries
                    Err(TypeCheckError::generic_error("Dictionary slice assignment not supported - use single key assignment dict[key] = value"))
                }
            }
            TypeDecl::Identifier(struct_name) => {
                // Struct assignment: check for __setitem__ method (only single element assignment)
                if start.is_some() && end.is_none() {
                    // Single element assignment: struct[key] = value
                    if let Some(key_expr) = start {
                        let struct_name_str = self.core.string_interner.resolve(struct_name)
                            .ok_or_else(|| TypeCheckError::generic_error("Unknown struct name"))?;

                        // Type check the key and value
                        let key_type_result = self.visit_expr(key_expr)?;

                        // Look for __setitem__ method
                        if let Some(setitem_method) = self.context.get_method_function_by_name(struct_name_str, "__setitem__", self.core.string_interner) {
                            // Check if method has correct signature: __setitem__(self, key: T, value: U)
                            if setitem_method.parameter.len() >= 3 {
                                let key_param_type = &setitem_method.parameter[1].1;
                                let value_param_type = &setitem_method.parameter[2].1;

                                // Check key type matches
                                if key_type_result != *key_param_type {
                                    return Err(TypeCheckError::type_mismatch(
                                        key_param_type.clone(), key_type_result
                                    ));
                                }

                                // Check value type matches
                                if value_type != *value_param_type {
                                    return Err(TypeCheckError::type_mismatch(
                                        value_param_type.clone(), value_type
                                    ));
                                }

                                // Assignment returns the value type
                                Ok(value_type)
                            } else {
                                Err(TypeCheckError::generic_error("__setitem__ method must have at least 3 parameters (self, key, value)"))
                            }
                        } else {
                            Err(TypeCheckError::generic_error(&format!(
                                "Cannot assign to struct type {:?} - no __setitem__ method found", object_type
                            )))
                        }
                    } else {
                        Err(TypeCheckError::generic_error("Struct assignment requires key index"))
                    }
                } else {
                    // Range slice assignment: check for __setslice__ method
                    self.check_struct_setslice_method(struct_name, start, end, &value_type, &object_type)
                }
            }
            TypeDecl::Struct(struct_name, ref _type_params) => {
                // Struct type assignment: check for __setitem__ or __setslice__ method
                let struct_name_str = self.core.string_interner.resolve(struct_name)
                    .ok_or_else(|| TypeCheckError::generic_error("Unknown struct name"))?;

                if start.is_some() && end.is_none() {
                    // Single element assignment: struct[key] = value - use __setitem__
                    if let Some(key_expr) = start {
                        let key_type_result = self.visit_expr(key_expr)?;

                        if let Some(setitem_method) = self.context.get_method_function_by_name(struct_name_str, "__setitem__", self.core.string_interner) {
                            if setitem_method.parameter.len() >= 3 {
                                let key_param_type = &setitem_method.parameter[1].1;
                                let value_param_type = &setitem_method.parameter[2].1;

                                if key_type_result != *key_param_type && !self.are_types_compatible(key_param_type, &key_type_result) {
                                    return Err(TypeCheckError::type_mismatch(
                                        key_param_type.clone(), key_type_result
                                    ));
                                }

                                if value_type != *value_param_type && !self.are_types_compatible(value_param_type, &value_type) {
                                    return Err(TypeCheckError::type_mismatch(
                                        value_param_type.clone(), value_type
                                    ));
                                }

                                Ok(value_type)
                            } else {
                                Err(TypeCheckError::generic_error("__setitem__ method must have at least 3 parameters (self, key, value)"))
                            }
                        } else {
                            Err(TypeCheckError::generic_error(&format!(
                                "Cannot assign to struct type {:?} - no __setitem__ method found", object_type
                            )))
                        }
                    } else {
                        Err(TypeCheckError::generic_error("Struct assignment requires key index"))
                    }
                } else {
                    // Range slice assignment: check for __setslice__ method
                    self.check_struct_setslice_method(struct_name, start, end, &value_type, &object_type)
                }
            }
            _ => {
                Err(TypeCheckError::generic_error(&format!(
                    "Cannot assign to type {:?} - only arrays, dictionaries, and structs with __setitem__ are supported", object_type
                )))
            }
        }
    }

    /// Type check dict literals - implementation
    pub fn visit_dict_literal_impl(&mut self, entries: &Vec<(ExprRef, ExprRef)>) -> Result<TypeDecl, TypeCheckError> {
        if entries.is_empty() {
            // Empty dict - type will be inferred from usage or type hint
            if let Some(TypeDecl::Dict(key_type, value_type)) = &self.type_inference.type_hint {
                return Ok(TypeDecl::Dict(key_type.clone(), value_type.clone()));
            }
            return Ok(TypeDecl::Dict(Box::new(TypeDecl::Unknown), Box::new(TypeDecl::Unknown)));
        }

        // Save the original type hint to restore later
        let original_hint = self.type_inference.type_hint.clone();

        // Extract expected types from type hint if available (clone to avoid borrow issues)
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
        self.type_inference.type_hint = original_hint.clone();

        // If we have type hints and the inferred types are Unknown, use the hint types
        let final_key_type = if key_type == TypeDecl::Unknown && expected_key_type.is_some() {
            expected_key_type.clone().unwrap()
        } else {
            // Convert Number to concrete type
            if key_type == TypeDecl::Number {
                TypeDecl::UInt64  // Default numeric type for keys
            } else {
                key_type
            }
        };

        let final_value_type = if value_type == TypeDecl::Unknown && expected_value_type.is_some() {
            expected_value_type.clone().unwrap()
        } else {
            // Convert Number to concrete type
            if value_type == TypeDecl::Number {
                TypeDecl::UInt64  // Default numeric type for values
            } else {
                value_type
            }
        };

        // Verify all entries have consistent types - static typing requirement
        for (entry_index, (key_ref, value_ref)) in entries.iter().skip(1).enumerate() {
            // Set type hints for consistency checking
            if let Some(expected_key) = &expected_key_type {
                self.type_inference.type_hint = Some(expected_key.clone());
            }
            let k_type = self.visit_expr(key_ref)?;

            if let Some(expected_value) = &expected_value_type {
                self.type_inference.type_hint = Some(expected_value.clone());
            }
            let v_type = self.visit_expr(value_ref)?;

            // Restore original hint
            self.type_inference.type_hint = original_hint.clone();

            // Use final types for consistency checking
            let check_key_type = if k_type == TypeDecl::Unknown && expected_key_type.is_some() {
                expected_key_type.clone().unwrap()
            } else {
                // Convert Number to concrete type
                if k_type == TypeDecl::Number {
                    TypeDecl::UInt64
                } else {
                    k_type
                }
            };

            let check_value_type = if v_type == TypeDecl::Unknown && expected_value_type.is_some() {
                expected_value_type.clone().unwrap()
            } else {
                // Convert Number to concrete type
                if v_type == TypeDecl::Number {
                    TypeDecl::UInt64
                } else {
                    v_type
                }
            };

            if check_key_type != final_key_type {
                return Err(TypeCheckError::generic_error(&format!(
                    "Dict key type mismatch at entry {}: expected {:?}, found {:?}. All keys must have the same type.",
                    entry_index + 1, final_key_type, check_key_type
                )));
            }
            if check_value_type != final_value_type {
                return Err(TypeCheckError::generic_error(&format!(
                    "Dict value type mismatch at entry {}: expected {:?}, found {:?}. All values must have the same type.",
                    entry_index + 1, final_value_type, check_value_type
                )));
            }
        }

        Ok(TypeDecl::Dict(Box::new(final_key_type), Box::new(final_value_type)))
    }

    /// Type check tuple literals - implementation
    pub fn visit_tuple_literal_impl(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        if elements.is_empty() {
            return Ok(TypeDecl::Tuple(vec![]));
        }

        let original_hint = self.type_inference.type_hint.clone();

        let expected_types = if let Some(TypeDecl::Tuple(types)) = &self.type_inference.type_hint {
            Some(types.clone())
        } else {
            None
        };

        let mut element_types = Vec::new();
        for (index, elem_ref) in elements.iter().enumerate() {
            if let Some(ref expected) = expected_types {
                if index < expected.len() {
                    self.type_inference.type_hint = Some(expected[index].clone());
                }
            }

            let elem_type = self.visit_expr(elem_ref)?;

            let final_elem_type = if elem_type == TypeDecl::Number {
                if let Some(ref expected) = expected_types {
                    if index < expected.len() && expected[index] != TypeDecl::Unknown {
                        expected[index].clone()
                    } else {
                        TypeDecl::UInt64
                    }
                } else {
                    TypeDecl::UInt64
                }
            } else {
                elem_type
            };

            element_types.push(final_elem_type);
        }

        self.type_inference.type_hint = original_hint;
        Ok(TypeDecl::Tuple(element_types))
    }

    /// Type check tuple access - implementation
    pub fn visit_tuple_access_impl(&mut self, tuple: &ExprRef, index: usize) -> Result<TypeDecl, TypeCheckError> {
        let tuple_type = self.visit_expr(tuple)?;

        match tuple_type {
            TypeDecl::Tuple(ref types) => {
                if index >= types.len() {
                    return Err(TypeCheckError::generic_error(&format!(
                        "Tuple index {} out of bounds for tuple with {} elements",
                        index, types.len()
                    )));
                }
                Ok(types[index].clone())
            }
            _ => {
                Err(TypeCheckError::generic_error(&format!(
                    "Cannot access index {} on non-tuple type {:?}",
                    index, tuple_type
                )))
            }
        }
    }

    /// Type check array literal - implementation (moved from type_checker.rs)
    pub fn visit_array_literal_impl(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Save the original type hint to restore later
        let original_hint = self.type_inference.type_hint.clone();

        // If we have a type hint for the array element type, use it for element type inference
        let element_type_hint = if let Some(TypeDecl::Array(element_types, _)) = &self.type_inference.type_hint {
            if !element_types.is_empty() {
                Some(element_types[0].clone())
            } else {
                None
            }
        } else {
            None
        };

        // Type check all elements with proper type hint for each element
        let mut element_types = Vec::new();

        for element in elements {
            // Set the element type hint for each element individually
            if let Some(ref hint) = element_type_hint {
                self.type_inference.type_hint = Some(hint.clone());
            }

            // For variable references, temporarily clear the type hint to get the actual stored type
            let element_type = if let Some(expr) = self.core.expr_pool.get(element) {
                if let Expr::Identifier(_var_name) = expr {
                    // Clear type hint for variable references to get their actual type
                    let saved_hint = self.type_inference.type_hint.take();
                    let result = self.visit_expr(element)?;
                    self.type_inference.type_hint = saved_hint;
                    result
                } else {
                    self.visit_expr(element)?
                }
            } else {
                self.visit_expr(element)?
            };

            element_types.push(element_type);

            // Restore original hint after processing each element
            self.type_inference.type_hint = original_hint.clone();
        }

        // If we have array type hint, handle type inference for all elements
        if let Some(TypeDecl::Array(ref expected_element_types, _)) = original_hint {
            if !expected_element_types.is_empty() {
                let expected_element_type = &expected_element_types[0];

                // Nesting level mismatch detection: if the hint expects array elements
                // but actual elements are scalars, the hint is for an outer array, so skip
                let hint_expects_array = matches!(expected_element_type, TypeDecl::Array(_, _));
                let actual_has_non_array = !element_types.is_empty()
                    && !matches!(&element_types[0], TypeDecl::Array(_, _));

                if hint_expects_array && actual_has_non_array {
                    // Skip: hint is for an outer array, not applicable to this inner array
                } else {

                // Handle type inference for each element
                for (i, element) in elements.iter().enumerate() {
                    match &element_types[i] {
                        TypeDecl::Number => {
                            // Transform Number literals to the expected type
                            self.transform_numeric_expr(element, expected_element_type)?;
                            element_types[i] = expected_element_type.clone();
                        },
                        TypeDecl::Bool => {
                            // Bool literals - check type compatibility
                            if expected_element_type != &TypeDecl::Bool {
                                return Err(TypeCheckError::array_error(&format!(
                                    "Array element {} has type Bool but expected {:?}",
                                    i, expected_element_type
                                )));
                            }
                        },
                        TypeDecl::Identifier(actual_struct) => {
                            // Struct literals - check type compatibility
                            if let TypeDecl::Identifier(expected_struct) = expected_element_type {
                                if actual_struct != expected_struct {
                                    return Err(TypeCheckError::array_error(&format!(
                                        "Array element {} has struct type {:?} but expected {:?}",
                                        i, actual_struct, expected_struct
                                    )));
                                }
                            } else {
                                return Err(TypeCheckError::array_error(&format!(
                                    "Array element {} has struct type {:?} but expected {:?}",
                                    i, actual_struct, expected_element_type
                                )));
                            }
                        },
                        actual_type if actual_type == expected_element_type => {
                            if let Some(expr) = self.core.expr_pool.get(&element) {
                                if matches!(expr, Expr::Number(_)) {
                                    self.transform_numeric_expr(element, expected_element_type)?;
                                }
                            }
                        },
                        TypeDecl::Unknown => {
                            element_types[i] = expected_element_type.clone();
                        },
                        actual_type if actual_type != expected_element_type => {
                            match (actual_type, expected_element_type) {
                                (TypeDecl::Int64, TypeDecl::UInt64) |
                                (TypeDecl::UInt64, TypeDecl::Int64) => {
                                    return Err(TypeCheckError::array_error(&format!(
                                        "Cannot mix signed and unsigned integers in array. Element {} has type {:?} but expected {:?}",
                                        i, actual_type, expected_element_type
                                    )));
                                },
                                (TypeDecl::Bool, _other_type) | (_other_type, TypeDecl::Bool) => {
                                    return Err(TypeCheckError::array_error(&format!(
                                        "Cannot mix Bool with other types in array. Element {} has type {:?} but expected {:?}",
                                        i, actual_type, expected_element_type
                                    )));
                                },
                                (TypeDecl::Identifier(struct1), TypeDecl::Identifier(struct2)) => {
                                    if struct1 != struct2 {
                                        return Err(TypeCheckError::array_error(&format!(
                                            "Array element {} has struct type {:?} but expected {:?}",
                                            i, struct1, struct2
                                        )));
                                    }
                                },
                                (TypeDecl::Identifier(struct_name), other_type) | (other_type, TypeDecl::Identifier(struct_name)) => {
                                    return Err(TypeCheckError::array_error(&format!(
                                        "Cannot mix struct type {:?} with {:?} in array. Element {} has incompatible type",
                                        struct_name, other_type, i
                                    )));
                                },
                                _ => {
                                    if actual_type == expected_element_type {
                                        // Already matches
                                    } else {
                                        return Err(TypeCheckError::array_error(&format!(
                                            "Array element {} has type {:?} but expected {:?}",
                                            i, actual_type, expected_element_type
                                        )));
                                    }
                                }
                            }
                        },
                        _ => {
                            // Type already matches expected type
                        }
                    }
                }

                } // end of nesting level guard
            }
        }

        // Handle Number types when no type hint was provided
        if original_hint.is_none() {
            for (i, element) in elements.iter().enumerate() {
                if element_types[i] == TypeDecl::Number {
                    self.transform_numeric_expr(element, &TypeDecl::UInt64)?;
                    element_types[i] = TypeDecl::UInt64;
                }
            }
        }

        // Restore the original type hint
        self.type_inference.type_hint = original_hint;

        let first_type = &element_types[0];
        for (i, element_type) in element_types.iter().enumerate() {
            if element_type != first_type {
                return Err(TypeCheckError::array_error(&format!(
                    "Array elements must have the same type, but element {} has type {:?} while first element has type {:?}",
                    i, element_type, first_type
                )));
            }
        }

        Ok(TypeDecl::Array(element_types, elements.len()))
    }

    /// Calculate slice size from constant literals if possible
    pub fn calculate_slice_size(&self, slice_info: &SliceInfo, array_size: usize) -> usize {
        let arr_size = array_size as i64;

        let start_val = match &slice_info.start {
            Some(expr) => self.extract_constant_value(expr),
            None => Some(0),
        };
        let end_val = match &slice_info.end {
            Some(expr) => self.extract_constant_value(expr),
            None => Some(arr_size),
        };

        match (start_val, end_val) {
            (Some(start), Some(end)) => {
                let actual_start = if start < 0 { arr_size + start } else { start };
                let actual_end = if end < 0 { arr_size + end } else { end };

                if actual_start >= 0 && actual_end >= actual_start && actual_end <= arr_size {
                    (actual_end - actual_start) as usize
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    /// Extract constant integer value from an expression
    pub fn extract_constant_value(&self, expr_ref: &ExprRef) -> Option<i64> {
        let expr = self.core.expr_pool.get(expr_ref)?;
        match expr {
            Expr::UInt64(val) => Some(val as i64),
            Expr::Int64(val) => Some(val),
            Expr::Number(symbol) => {
                let num_str = self.core.string_interner.resolve(symbol)?;
                num_str.parse::<i64>().ok()
            }
            _ => None,
        }
    }

    /// Type check cast expressions - implementation
    pub fn visit_cast_impl(&mut self, expr: &ExprRef, target_type: &TypeDecl) -> Result<TypeDecl, TypeCheckError> {
        let expr_type = self.visit_expr(expr)?;

        match (&expr_type, target_type) {
            // Allow i64 <-> u64 casts
            (TypeDecl::Int64, TypeDecl::UInt64) |
            (TypeDecl::UInt64, TypeDecl::Int64) |
            (TypeDecl::Int64, TypeDecl::Int64) |
            (TypeDecl::UInt64, TypeDecl::UInt64) => Ok(target_type.clone()),

            // Allow Number to specific numeric types
            (TypeDecl::Number, TypeDecl::Int64) |
            (TypeDecl::Number, TypeDecl::UInt64) => Ok(target_type.clone()),

            // Identity cast for other types
            (from, to) if from == to => Ok(target_type.clone()),

            // Invalid cast
            _ => Err(TypeCheckError::generic_error(&format!(
                "Cannot cast {:?} to {:?}",
                expr_type, target_type
            )))
        }
    }
}
