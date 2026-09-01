use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError
};
use crate::type_checker::method::MethodProcessing;
use string_interner::DefaultSymbol;

/// Collections type checking implementation (arrays, dictionaries, tuples, slices)
impl<'a> TypeCheckerVisitor<'a> {
    /// Type check slice access - implementation
    pub fn visit_slice_access_impl(&mut self, object: &ExprRef, slice_info: &SliceInfo) -> Result<TypeDecl, TypeCheckError> {
        let object_type = self.visit_expr(object)?;

        match object_type {
            TypeDecl::Array(ref element_types, _size, _) => {
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
                                        "Array index must be an integer type, but got {}",
                                        self.type_name_for_error(&start_type)
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
                        if matches!(_size, ArraySize::Literal(0)) {
                            // Dynamic array: return [T] (dynamic array of same element type)
                            return Ok(TypeDecl::Array(vec![single_element_type], ArraySize::Literal(0), false));
                        }

                        // Try to calculate slice size using array size for open-ended slices
                        let array_size = _size.literal_value().unwrap_or(0);
                        let slice_size = self.calculate_slice_size(slice_info, array_size);

                        // If slice_size is 0, return dynamic array type
                        if slice_size == 0 {
                            return Ok(TypeDecl::Array(vec![single_element_type], ArraySize::Literal(0), false));
                        }

                        // Create element_types with the correct number of elements
                        let result_element_types = vec![single_element_type; slice_size];
                        Ok(TypeDecl::Array(result_element_types, ArraySize::Literal(slice_size), false))
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
                if slice_info.is_valid_for_dict() {
                    self.check_struct_getitem_access(struct_name, slice_info, &object_type)
                } else {
                    self.check_struct_getslice_method(struct_name, slice_info, &object_type)
                }
            }
            TypeDecl::Struct(struct_name, ref _type_params) => {
                if slice_info.is_valid_for_dict() {
                    self.check_struct_getitem_access(struct_name, slice_info, &object_type)
                } else {
                    self.check_struct_getslice_method(struct_name, slice_info, &object_type)
                }
            }
            _ => {
                Err(TypeCheckError::generic_error(&format!(
                    "Cannot access type {} - only arrays, dictionaries, and structs with __getitem__ are supported",
                    self.type_name_for_error(&object_type)
                )))
            }
        }
    }

    /// Type check slice assignment - implementation
    pub fn visit_slice_assign_impl(&mut self, object: &ExprRef, start: &Option<ExprRef>, end: &Option<ExprRef>, value: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        // CLOSURE-CAPTURE E2: `a[i] = v` is its own expression rather
        // than an `Assign` with a slice target, so the capture rule
        // has to be applied here too. Without it this shape reached
        // the backends and died as an internal error on every engine
        // ("Expr::Number should be transformed to concrete type"),
        // even though the identical write outside a closure works.
        if let Some((_, root)) = self.captured_assign_target(object) {
            let target = format!("{root}[..]");
            let err = TypeCheckError::captured_assign(target, root);
            // The indexed identifier carries no recorded position, so
            // anchoring on it is a no-op and the statement recovery
            // puts the caret on the block's tail expression instead.
            // The index does carry one, and it is inside the target.
            let anchor = start.filter(|s| self.get_expr_location(s).is_some());
            return Err(self.error_with_location(err, anchor.as_ref().unwrap_or(object)));
        }
        let object_type = self.visit_expr(object)?;
        let value_type = self.visit_expr(value)?;

        match object_type {
            TypeDecl::Array(ref element_types, _size, _) => {
                // The index is an expression like any other and has to
                // be visited: an unsuffixed literal that nothing looks
                // at keeps the `Number` placeholder, and the backends
                // meet it as an internal error
                // ("Expr::Number should be transformed to concrete
                // type"). Only the dict branch below used to do this,
                // so `a[0] = v` survived on the function-level
                // default and failed inside a closure body.
                for bound in [start, end].iter().copied().flatten() {
                    self.visit_expr(bound)?;
                }
                self.handle_array_slice_assign(element_types, start, end, &value_type)
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
                                    "Dict value type mismatch: expected {}, found {}",
                                    self.type_name_for_error(expected_dict_value_type),
                                    self.type_name_for_error(&resolved_value_type)
                                )));
                            }
                            // No value: an assignment is `Unit` at
                            // every form (see `visit_assign`).
                            Ok(TypeDecl::Unit)
                        } else {
                            Ok(TypeDecl::Unit)
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
                        // Type check the key, then verify the
                        // `__setitem__` signature (POINTER P2: either
                        // receiver spelling).
                        let key_type_result = self.visit_expr(key_expr)?;
                        self.check_struct_setitem_access(struct_name, key_type_result, &value_type, &object_type)?;
                        // No value: an assignment is `Unit` at every
                        // form (see `visit_assign`).
                        Ok(TypeDecl::Unit)
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
                if start.is_some() && end.is_none() {
                    // Single element assignment: struct[key] = value - use __setitem__
                    if let Some(key_expr) = start {
                        let key_type_result = self.visit_expr(key_expr)?;
                        self.check_struct_setitem_access(struct_name, key_type_result, &value_type, &object_type)?;
                        Ok(TypeDecl::Unit)
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
                    "Cannot assign to type {} - only arrays, dictionaries, and structs with __setitem__ are supported",
                    self.type_name_for_error(&object_type)
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
        // NUMBER-HINT: the annotation's key / value type names what
        // an unsuffixed literal in the entry should become; the
        // default applies only where no annotation did. `coerce_number_expr`
        // settles the literal on the default for a non-integer target
        // too, so a `dict[str, str]` with a numeric value reports
        // `found u64` rather than the internal placeholder.
        let final_key_type = if key_type == TypeDecl::Unknown && expected_key_type.is_some() {
            expected_key_type.clone().unwrap()
        } else if key_type == TypeDecl::Number {
            let target = expected_key_type.clone().unwrap_or(TypeDecl::UInt64);
            self.coerce_number_expr(first_key, &key_type, &target)?
        } else {
            key_type
        };

        let final_value_type = if value_type == TypeDecl::Unknown && expected_value_type.is_some() {
            expected_value_type.clone().unwrap()
        } else if value_type == TypeDecl::Number {
            let target = expected_value_type.clone().unwrap_or(TypeDecl::UInt64);
            self.coerce_number_expr(first_value, &value_type, &target)?
        } else {
            value_type
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
            // NUMBER-HINT: later entries take their type from the
            // annotation, else from the type the first entry settled
            // on, so `dict{"a": 1i64, "b": 2}` needs the suffix once.
            let check_key_type = if k_type == TypeDecl::Unknown && expected_key_type.is_some() {
                expected_key_type.clone().unwrap()
            } else if k_type == TypeDecl::Number {
                let target = expected_key_type.clone().unwrap_or_else(|| final_key_type.clone());
                self.coerce_number_expr(key_ref, &k_type, &target)?
            } else {
                k_type
            };

            let check_value_type = if v_type == TypeDecl::Unknown && expected_value_type.is_some() {
                expected_value_type.clone().unwrap()
            } else if v_type == TypeDecl::Number {
                let target = expected_value_type.clone().unwrap_or_else(|| final_value_type.clone());
                self.coerce_number_expr(value_ref, &v_type, &target)?
            } else {
                v_type
            };

            if check_key_type != final_key_type {
                return Err(TypeCheckError::generic_error(&format!(
                    "Dict key type mismatch at entry {}: expected {}, found {}. All keys must have the same type.",
                    entry_index + 1,
                    self.type_name_for_error(&final_key_type),
                    self.type_name_for_error(&check_key_type)
                )));
            }
            if check_value_type != final_value_type {
                return Err(TypeCheckError::generic_error(&format!(
                    "Dict value type mismatch at entry {}: expected {}, found {}. All values must have the same type.",
                    entry_index + 1,
                    self.type_name_for_error(&final_value_type),
                    self.type_name_for_error(&check_value_type)
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
            if let Some(ref expected) = expected_types
                && index < expected.len() {
                    self.type_inference.type_hint = Some(expected[index].clone());
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
            // NEWTYPE: `m.0` on a tuple struct is the positional field
            // the parser named `"0"`. Typing is delegated to the field
            // path so `Self`, `&T` receivers and generic substitution
            // behave exactly as they do for `p.x`.
            _ => match self.tuple_struct_field_symbol(&tuple_type, index)? {
                Some(field_symbol) => {
                    self.tuple_struct_rewrites.accesses.insert(*tuple, field_symbol);
                    self.visit_field_access_impl(tuple, &field_symbol)
                }
                None => Err(TypeCheckError::generic_error(&format!(
                    "Cannot access index {} on non-tuple type {}",
                    index, self.type_name_for_error(&tuple_type)
                ))),
            },
        }
    }

    /// NEWTYPE: the field symbol `.index` names on a tuple struct, or
    /// `None` when `receiver_ty` isn't one (so the caller can report the
    /// access against the type the user actually wrote).
    ///
    /// An `Err` is reserved for a receiver that *is* a tuple struct but
    /// was indexed past its arity -- saying so beats "non-tuple type".
    fn tuple_struct_field_symbol(
        &mut self,
        receiver_ty: &TypeDecl,
        index: usize,
    ) -> Result<Option<DefaultSymbol>, TypeCheckError> {
        let resolved = match receiver_ty {
            TypeDecl::Self_ => self.resolve_self_type(receiver_ty),
            TypeDecl::Ref { inner, .. } => (**inner).clone(),
            other => other.clone(),
        };
        let struct_symbol = match resolved {
            TypeDecl::Struct(symbol, _) | TypeDecl::Identifier(symbol) => symbol,
            _ => return Ok(None),
        };
        let Some(fields) = self.context.get_struct_fields(struct_symbol) else {
            return Ok(None);
        };
        if !fields.first().is_some_and(|f| f.is_positional()) {
            let struct_name = self.resolve_symbol_name(struct_symbol);
            return Err(TypeCheckError::generic_error(&format!(
                "`{struct_name}` has named fields, so its fields are reached by name \
                 (`value.field`), not by index"
            )));
        }
        let Some(field_name) = fields.get(index).map(|f| f.name.clone()) else {
            let arity = fields.len();
            let struct_name = self.resolve_symbol_name(struct_symbol);
            return Err(TypeCheckError::generic_error(&format!(
                "index {index} is out of bounds for `{struct_name}`, which has {arity} field(s)"
            )));
        };
        // Interned by the parser when it read the declaration.
        Ok(self.core.string_interner.get(field_name.as_str()))
    }

    /// Type check array literal - implementation (moved from type_checker.rs)
    pub fn visit_array_literal_impl(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Save the original type hint to restore later
        let original_hint = self.type_inference.type_hint.clone();
        if std::env::var("TOY_DEBUG_ARRAY_HINT").is_ok() {
            let hint_str = original_hint
                .as_ref()
                .map(|t| self.type_name_for_error(t))
                .unwrap_or_else(|| "none".to_string());
            eprintln!("[debug] array literal hint: {hint_str}");
        }

        // If we have a type hint for the array element type, use it for element type inference
        let element_type_hint = if let Some(TypeDecl::Array(element_types, _, _)) = &self.type_inference.type_hint {
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
        if let Some(TypeDecl::Array(ref expected_element_types, _, _)) = original_hint
            && !expected_element_types.is_empty() {
                let expected_element_type = &expected_element_types[0];

                // Nesting level mismatch detection: if the hint expects array elements
                // but actual elements are scalars, the hint is for an outer array, so skip
                let hint_expects_array = matches!(expected_element_type, TypeDecl::Array(..));
                let actual_has_non_array = !element_types.is_empty()
                    && !matches!(&element_types[0], TypeDecl::Array(..));

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
                        TypeDecl::Bool
                            // Bool literals - check type compatibility
                            if expected_element_type != &TypeDecl::Bool => {
                                return Err(TypeCheckError::array_error(&format!(
                                    "Array element {} has type bool but expected {}",
                                    i, self.type_name_for_error(expected_element_type)
                                )));
                            },
                        TypeDecl::Identifier(actual_struct) => {
                            // Struct literals - check type compatibility.
                            // An annotation names a user type as
                            // `Identifier(name)` while a literal's
                            // inferred type may spell the same thing
                            // `Struct(name, [])` / `Enum(name, [])`
                            // (or vice versa) — the two spellings the
                            // return-type check and `is_equivalent`
                            // already unify.
                            let spelled_same = match expected_element_type {
                                TypeDecl::Identifier(expected_struct)
                                | TypeDecl::Struct(expected_struct, _)
                                | TypeDecl::Enum(expected_struct, _) => {
                                    actual_struct == expected_struct
                                }
                                _ => false,
                            };
                            if !spelled_same {
                                return Err(TypeCheckError::array_error(&format!(
                                    "Array element {} has struct type {} but expected {}",
                                    i,
                                    self.resolve_symbol_name(*actual_struct),
                                    self.type_name_for_error(expected_element_type)
                                )));
                            }
                        },
                        actual_type if actual_type == expected_element_type => {
                            if let Some(expr) = self.core.expr_pool.get(element)
                                && matches!(expr, Expr::Number(_)) {
                                    self.transform_numeric_expr(element, expected_element_type)?;
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
                                        "Cannot mix signed and unsigned integers in array. Element {} has type {} but expected {}",
                                        i,
                                        self.type_name_for_error(actual_type),
                                        self.type_name_for_error(expected_element_type)
                                    )));
                                },
                                (TypeDecl::Bool, _other_type) | (_other_type, TypeDecl::Bool) => {
                                    return Err(TypeCheckError::array_error(&format!(
                                        "Cannot mix bool with other types in array. Element {} has type {} but expected {}",
                                        i,
                                        self.type_name_for_error(actual_type),
                                        self.type_name_for_error(expected_element_type)
                                    )));
                                },
                                (TypeDecl::Identifier(struct1), TypeDecl::Identifier(struct2)) => {
                                    if struct1 != struct2 {
                                        return Err(TypeCheckError::array_error(&format!(
                                            "Array element {} has struct type {} but expected {}",
                                            i,
                                            self.resolve_symbol_name(*struct1),
                                            self.resolve_symbol_name(*struct2)
                                        )));
                                    }
                                },
                                // Same user type in its two spellings
                                // (`Struct(name, [])` inferred vs
                                // `Identifier(name)` annotated) —
                                // accept, mirroring the arm above and
                                // `is_equivalent`.
                                (TypeDecl::Struct(a, params), TypeDecl::Identifier(b))
                                | (TypeDecl::Identifier(b), TypeDecl::Struct(a, params)) => {
                                    if !(a == b && params.is_empty()) {
                                        return Err(TypeCheckError::array_error(&format!(
                                            "Cannot mix struct type {} with {} in array. Element {} has incompatible type",
                                            self.resolve_symbol_name(*b),
                                            self.type_name_for_error(&TypeDecl::Struct(*a, params.clone())),
                                            i
                                        )));
                                    }
                                },
                                (TypeDecl::Enum(a, params), TypeDecl::Identifier(b))
                                | (TypeDecl::Identifier(b), TypeDecl::Enum(a, params)) => {
                                    if !(a == b && params.is_empty()) {
                                        return Err(TypeCheckError::array_error(&format!(
                                            "Cannot mix enum type {} with {} in array. Element {} has incompatible type",
                                            self.resolve_symbol_name(*b),
                                            self.type_name_for_error(&TypeDecl::Enum(*a, params.clone())),
                                            i
                                        )));
                                    }
                                },
                                // DATA-ORIENTED Phase 3: a *generic*
                                // user type in its two spellings. The
                                // parser writes an annotation's
                                // `Option<i64>` as `Struct(Option,
                                // [i64])` — it cannot tell a struct
                                // from an enum — while the literal's
                                // inferred type is `Enum(Option,
                                // [i64])`. The arms above unify the
                                // two only when the type arguments
                                // are empty, which left an annotated
                                // array of a generic enum unusable.
                                (TypeDecl::Enum(a, a_params), TypeDecl::Struct(b, b_params))
                                | (TypeDecl::Struct(b, b_params), TypeDecl::Enum(a, a_params)) => {
                                    if !(a == b && a_params == b_params) {
                                        return Err(TypeCheckError::array_error(&format!(
                                            "Cannot mix enum type {} with {} in array. Element {} has incompatible type",
                                            self.type_name_for_error(&TypeDecl::Enum(*a, a_params.clone())),
                                            self.type_name_for_error(&TypeDecl::Struct(*b, b_params.clone())),
                                            i
                                        )));
                                    }
                                },
                                (TypeDecl::Identifier(struct_name), other_type) | (other_type, TypeDecl::Identifier(struct_name)) => {
                                    return Err(TypeCheckError::array_error(&format!(
                                        "Cannot mix struct type {} with {} in array. Element {} has incompatible type",
                                        self.resolve_symbol_name(*struct_name),
                                        self.type_name_for_error(other_type),
                                        i
                                    )));
                                },
                                _ => {
                                    if actual_type == expected_element_type {
                                        // Already matches
                                    } else {
                                        return Err(TypeCheckError::array_error(&format!(
                                            "Array element {} has type {} but expected {}",
                                            i,
                                            self.type_name_for_error(actual_type),
                                            self.type_name_for_error(expected_element_type)
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

        // NUMBER-HINT: settle any element still carrying the
        // unresolved-literal placeholder. It gets here when no
        // annotation named the element type — including when the
        // ambient hint is not an array hint at all, as in
        // `fn main() -> bool { val a = [true, 1] ... }`, where the
        // `bool` belongs to the function, not the array.
        //
        // A sibling element that does carry an integer type names the
        // array's element type, so `[1i64, 2, 3]` needs the suffix
        // once rather than on every element. Failing that the
        // literals take the default, which also keeps a genuinely
        // mixed array's homogeneity error below naming a type the
        // reader can write rather than the placeholder.
        if element_types.contains(&TypeDecl::Number) {
            let element_target = element_types
                .iter()
                .find(|t| Self::is_integer_target(t))
                .cloned()
                .unwrap_or(TypeDecl::UInt64);
            for (i, element) in elements.iter().enumerate() {
                if element_types[i] == TypeDecl::Number {
                    element_types[i] =
                        self.coerce_number_expr(element, &TypeDecl::Number, &element_target)?;
                }
            }
        }

        // Restore the original type hint
        self.type_inference.type_hint = original_hint;

        let first_type = &element_types[0];
        for (i, element_type) in element_types.iter().enumerate() {
            if element_type != first_type {
                return Err(TypeCheckError::array_error(&format!(
                    "Array elements must have the same type, but element {} has type {} while first element has type {}",
                    i,
                    self.type_name_for_error(element_type),
                    self.type_name_for_error(first_type)
                )));
            }
        }

        Ok(TypeDecl::Array(element_types, ArraySize::Literal(elements.len()), false))
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

        // NUM-W cast matrix: any numeric primitive can cast to
        // any other numeric primitive. Runtime semantics
        // (`evaluate_cast`) match Rust's `as`: int-int truncates
        // / sign-extends to the target width, int-float
        // round-to-nearest, float-int saturates with NaN→0.
        // The classifier below replaces the per-pair allowlist
        // the i64/u64/f64-only era used.
        if expr_type.is_numeric() && target_type.is_numeric() {
            return Ok(target_type.clone());
        }
        match (&expr_type, target_type) {
            // LLM-LOOP P1/P3: `Unknown` marks an operand whose real type
            // could not be determined -- it diverges, or its defining
            // statement already reported an error and recovery bound it
            // to `Unknown` to keep checking. Complaining about it here
            // would add "Cannot cast Unknown to UInt64" on top of the
            // real diagnostic, naming an internal type the user never
            // wrote. Take the declared target and stay quiet.
            (TypeDecl::Unknown, _) => Ok(target_type.clone()),

            // Allow Number to specific numeric types (parser
            // emits `Number` for unsuffixed integer literals
            // before type inference fixes them).
            (TypeDecl::Number, t) if t.is_numeric() => Ok(target_type.clone()),

            // Identity cast for other types
            (from, to) if from == to => Ok(target_type.clone()),

            // Invalid cast
            _ => Err(TypeCheckError::generic_error(&format!(
                "Cannot cast {} to {}",
                self.type_name_for_error(&expr_type),
                self.type_name_for_error(target_type)
            )))
        }
    }
}
