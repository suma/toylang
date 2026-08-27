use std::cell::RefCell;
use std::rc::Rc;
use frontend::ast::*;
use crate::object::{Object, ObjectKey, RcObject};
use crate::error::InterpreterError;
use crate::try_value;
use super::{EvaluationContext, EvaluationResult};

impl EvaluationContext<'_> {
    /// Convert index (positive or negative) to array index.
    ///
    /// `allow_len == true` marks an exclusive end bound (`arr[a..b]`
    /// permits `b == array_len`); `false` is a single-element access
    /// (`arr[i]` requires `i < array_len`). The check is the only
    /// difference between the two callers, so it lives here.
    /// An out-of-range index, reported the way every other trap is
    /// (DEBUG-OBS): a panic carrying the position and the backtrace,
    /// with the two numbers in the message. It used to be a bare
    /// `IndexOutOfBounds` error, which arrived with neither.
    fn index_out_of_bounds(
        &self,
        index: isize,
        size: usize,
        site: Option<frontend::type_checker::SourceLocation>,
    ) -> InterpreterError {
        self.panic_error(
            compiler_ir::panic_values_message(
                compiler_ir::panic_kind::INDEX_OUT_OF_BOUNDS,
                index as i64,
                size as u64,
            ),
            site,
        )
    }

    fn resolve_array_index(
        &self,
        index_obj: &RcObject,
        array_len: usize,
        allow_len: bool,
        site: Option<frontend::type_checker::SourceLocation>,
    ) -> Result<usize, InterpreterError> {
        let in_range = |idx: usize| if allow_len { idx <= array_len } else { idx < array_len };
        let borrowed = index_obj.borrow();
        match &*borrowed {
            Object::UInt64(idx) => {
                let idx = *idx as usize;
                if !in_range(idx) {
                    return Err(self.index_out_of_bounds(idx as isize, array_len, site));
                }
                Ok(idx)
            }
            Object::Int64(idx) => {
                if *idx >= 0 {
                    // Positive i64, treat as u64
                    let idx = *idx as usize;
                    if !in_range(idx) {
                        return Err(self.index_out_of_bounds(idx as isize, array_len, site));
                    }
                    Ok(idx)
                } else {
                    // Negative index: convert to positive
                    let abs_idx = (-*idx) as usize;
                    if abs_idx > array_len {
                        return Err(self.index_out_of_bounds(*idx as isize, array_len, site));
                    }
                    Ok(array_len - abs_idx)
                }
            }
            _ => Err(InterpreterError::InternalError("Array index must be an integer".to_string()))
        }
    }

    pub(super) fn evaluate_slice_access_with_info(
        &mut self,
        object: &ExprRef,
        slice_info: &SliceInfo,
        whole: Option<ExprRef>,
    ) -> Result<EvaluationResult, InterpreterError> {
        // DEBUG-OBS: the whole `arr[i]`, which is what the compiled
        // backends attribute their bounds trap to. Falling back
        // through the object and then the index keeps the older
        // callers (which have neither) reporting *something*.
        let site = whole
            .and_then(|e| self.expr_location(&e))
            .or_else(|| self.expr_location(object))
            .or_else(|| slice_info.start.and_then(|e| self.expr_location(&e)));
        let object_val = self.evaluate(object)?;
        let object_obj = try_value!(Ok(object_val));

        let obj_borrowed = object_obj.borrow();
        match &*obj_borrowed {
            Object::Array(elements) => {
                let array_len = elements.len();

                // Evaluate start index (default to 0)
                let start_idx = if let Some(start_expr) = &slice_info.start {
                    let start_val = self.evaluate(start_expr)?;
                    let start_obj = try_value!(Ok(start_val));
                    self.resolve_array_index(&start_obj, array_len, false, site)?
                } else {
                    0
                };

                // Evaluate end index (default to array length)
                let end_idx = if let Some(end_expr) = &slice_info.end {
                    let end_val = self.evaluate(end_expr)?;
                    let end_obj = try_value!(Ok(end_val));
                    self.resolve_array_index(&end_obj, array_len, true, site)?
                } else {
                    array_len
                };

                // Validate indices
                if start_idx > array_len {
                    return Err(self.index_out_of_bounds(start_idx as isize, array_len, site));
                }
                if end_idx > array_len {
                    return Err(self.index_out_of_bounds(end_idx as isize, array_len, site));
                }
                if start_idx > end_idx {
                    return Err(InterpreterError::InternalError(
                        format!("Invalid slice range: start ({}) > end ({})", start_idx, end_idx)
                    ));
                }

                // Use SliceInfo to distinguish single element vs range slice
                match slice_info.slice_type {
                    SliceType::SingleElement => {
                        // Single element access: arr[i] returns the element directly
                        if start_idx >= array_len {
                            return Err(self.index_out_of_bounds(start_idx as isize, array_len, site));
                        }
                        Ok(EvaluationResult::Value(elements[start_idx].clone().into()))
                    }
                    SliceType::RangeSlice => {
                        // Range slice: arr[start..end] returns array
                        let slice_elements = elements[start_idx..end_idx].to_vec();
                        Ok(EvaluationResult::Value(Object::Array(Box::new(slice_elements)).into()))
                    }
                }
            }
            Object::Dict(_dict) => {
                // Dictionary access uses the original method
                self.evaluate_slice_access(object, &slice_info.start, &slice_info.end)
            }
            Object::Struct { type_name, .. } => {
                // Struct access: check for __getitem__ method (only single element access)
                match slice_info.slice_type {
                    SliceType::SingleElement => {
                        // Single element access: struct[key]
                        if let Some(start_expr) = &slice_info.start {
                            let struct_name_val = *type_name;
                            drop(obj_borrowed); // Release borrow before method call

                            let start_val = self.evaluate(start_expr)?;
                            let start_obj = try_value!(Ok(start_val));

                            // Resolve names first before method call
                            let struct_name_str = self.string_interner.resolve(struct_name_val)
                                .ok_or_else(|| InterpreterError::InternalError("Failed to resolve struct name".to_string()))?
                                .to_string();
                            let getitem_method = self.string_interner.get_or_intern("__getitem__");

                            // Call __getitem__(self, index)
                            let args = vec![start_obj];
                            self.call_struct_method(object_obj, getitem_method, &args, &struct_name_str, self.expr_location(object))
                        } else {
                            Err(InterpreterError::InternalError("Struct access requires index".to_string()))
                        }
                    }
                    SliceType::RangeSlice => {
                        // Range slicing: struct[start..end] calls __getslice__(self, start, end)
                        let struct_name_val = *type_name;
                        drop(obj_borrowed);

                        let start_obj = if let Some(start_expr) = &slice_info.start {
                            let start_val = self.evaluate(start_expr)?;
                            try_value!(Ok(start_val))
                        } else {
                            Rc::new(RefCell::new(Object::Int64(0)))
                        };

                        let end_obj = if let Some(end_expr) = &slice_info.end {
                            let end_val = self.evaluate(end_expr)?;
                            try_value!(Ok(end_val))
                        } else {
                            Rc::new(RefCell::new(Object::Int64(i64::MAX)))
                        };

                        let struct_name_str = self.string_interner.resolve(struct_name_val)
                            .ok_or_else(|| InterpreterError::InternalError("Failed to resolve struct name".to_string()))?
                            .to_string();
                        let getslice_method = self.string_interner.get_or_intern("__getslice__");

                        let args = vec![start_obj, end_obj];
                        self.call_struct_method(object_obj, getslice_method, &args, &struct_name_str, self.expr_location(object))
                    }
                }
            }
            _ => Err(InterpreterError::InternalError(
                format!("Cannot access type: {:?} - only arrays, dictionaries, and structs with __getitem__ are supported", obj_borrowed.get_type())
            ))
        }
    }

    fn evaluate_slice_access(&mut self, object: &ExprRef, start: &Option<ExprRef>, end: &Option<ExprRef>) -> Result<EvaluationResult, InterpreterError> {
        // DEBUG-OBS: the indexed expression's position, which the
        // compiled backends attribute their bounds trap to as well.
        let site = self.expr_location(object);
        let object_val = self.evaluate(object)?;
        let object_obj = try_value!(Ok(object_val));

        let obj_borrowed = object_obj.borrow();
        match &*obj_borrowed {
            Object::Array(elements) => {
                let array_len = elements.len();

                // Evaluate start index (default to 0)
                let start_idx = if let Some(start_expr) = start {
                    let start_val = self.evaluate(start_expr)?;
                    let start_obj = try_value!(Ok(start_val));
                    self.resolve_array_index(&start_obj, array_len, false, site)?
                } else {
                    0
                };

                // Evaluate end index (default to array length)
                let end_idx = if let Some(end_expr) = end {
                    let end_val = self.evaluate(end_expr)?;
                    let end_obj = try_value!(Ok(end_val));
                    self.resolve_array_index(&end_obj, array_len, true, site)?
                } else {
                    array_len
                };

                // Validate indices
                if start_idx > array_len {
                    return Err(self.index_out_of_bounds(start_idx as isize, array_len, site));
                }
                if end_idx > array_len {
                    return Err(self.index_out_of_bounds(end_idx as isize, array_len, site));
                }
                if start_idx > end_idx {
                    return Err(InterpreterError::InternalError(
                        format!("Invalid slice range: start ({}) > end ({})", start_idx, end_idx)
                    ));
                }

                // Check if this is single element access (start provided, end is None)
                if start.is_some() && end.is_none() {
                    // Single element access: arr[i] returns the element directly
                    if start_idx >= array_len {
                        return Err(self.index_out_of_bounds(start_idx as isize, array_len, site));
                    }
                    Ok(EvaluationResult::Value(elements[start_idx].clone().into()))
                } else {
                    // Range slice: arr[start..end] returns array
                    let slice_elements = elements[start_idx..end_idx].to_vec();
                    Ok(EvaluationResult::Value(Object::Array(Box::new(slice_elements)).into()))
                }
            }
            Object::Dict(dict) => {
                // Dictionary access: dict[key] (only single element access)
                if start.is_some() && end.is_none() {
                    // Single element access: dict[key]
                    if let Some(start_expr) = start {
                        let start_val = self.evaluate(start_expr)?;
                        let start_obj = try_value!(Ok(start_val));

                        // Create ObjectKey for dictionary lookup
                        let key_borrowed = start_obj.borrow();
                        let key_object = key_borrowed.clone();
                        let object_key = ObjectKey::new(key_object);

                        dict.get(&object_key)
                            .cloned()
                            .map(|rc| EvaluationResult::Value(rc.into()))
                            .ok_or_else(|| InterpreterError::InternalError(format!("Key not found: {:?}", object_key)))
                    } else {
                        Err(InterpreterError::InternalError("Dictionary access requires key index".to_string()))
                    }
                } else {
                    // Range slicing not supported for dictionaries
                    Err(InterpreterError::InternalError("Dictionary slicing not supported - use single key access dict[key]".to_string()))
                }
            }
            Object::Struct { type_name, .. } => {
                // Struct access: check for __getitem__ method (only single element access)
                if start.is_some() && end.is_none() {
                    // Single element access: struct[key]
                    if let Some(start_expr) = start {
                        let struct_name_val = *type_name;
                        drop(obj_borrowed); // Release borrow before method call

                        let start_val = self.evaluate(start_expr)?;
                        let start_obj = try_value!(Ok(start_val));

                        // Resolve names first before method call
                        let struct_name_str = self.string_interner.resolve(struct_name_val)
                            .ok_or_else(|| InterpreterError::InternalError("Failed to resolve struct name".to_string()))?
                            .to_string();
                        let getitem_method = self.string_interner.get_or_intern("__getitem__");

                        // Call __getitem__(self, index)
                        let args = vec![start_obj];
                        self.call_struct_method(object_obj, getitem_method, &args, &struct_name_str, self.expr_location(object))
                    } else {
                        Err(InterpreterError::InternalError("Struct access requires index".to_string()))
                    }
                } else {
                    // Range slicing: struct[start..end] calls __getslice__(self, start, end)
                    let struct_name_val = *type_name;
                    drop(obj_borrowed);

                    let start_obj = if let Some(start_expr) = start {
                        let start_val = self.evaluate(start_expr)?;
                        try_value!(Ok(start_val))
                    } else {
                        Rc::new(RefCell::new(Object::Int64(0)))
                    };

                    let end_obj = if let Some(end_expr) = end {
                        let end_val = self.evaluate(end_expr)?;
                        try_value!(Ok(end_val))
                    } else {
                        Rc::new(RefCell::new(Object::Int64(-1)))
                    };

                    let struct_name_str = self.string_interner.resolve(struct_name_val)
                        .ok_or_else(|| InterpreterError::InternalError("Failed to resolve struct name".to_string()))?
                        .to_string();
                    let getslice_method = self.string_interner.get_or_intern("__getslice__");

                    let args = vec![start_obj, end_obj];
                    self.call_struct_method(object_obj, getslice_method, &args, &struct_name_str, self.expr_location(object))
                }
            }
            _ => Err(InterpreterError::InternalError(
                format!("Cannot access type: {:?} - only arrays, dictionaries, and structs with __getitem__ are supported", obj_borrowed.get_type())
            ))
        }
    }

    pub(super) fn evaluate_slice_assign(&mut self, object: &ExprRef, start: &Option<ExprRef>, end: &Option<ExprRef>, value: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        // Get the object being indexed
        let object_val = self.evaluate(object)?;
        let object_obj = try_value!(Ok(object_val));

        // Evaluate the value to assign
        let value_val = self.evaluate(value)?;
        let value_obj = try_value!(Ok(value_val));

        let obj_borrowed = object_obj.borrow();
        match &*obj_borrowed {
            Object::Array(elements) => {
                let array_len = elements.len();
                drop(obj_borrowed);

                // Check if this is single element assignment (start provided, end is None)
                if start.is_some() && end.is_none() {
                    // Single element assignment: arr[i] = value
                    if let Some(start_expr) = start {
                        let site = self
                            .expr_location(object)
                            .or_else(|| self.expr_location(start_expr));
                        let start_val = self.evaluate(start_expr)?;
                        let start_obj = try_value!(Ok(start_val));
                        let resolved_idx = self.resolve_array_index(&start_obj, array_len, false, site)?;

                        let mut obj_borrowed = object_obj.borrow_mut();
                        if let Object::Array(elements) = &mut *obj_borrowed {
                            elements[resolved_idx] = value_obj.clone();
                            Ok(EvaluationResult::Value(value_obj.into()))
                        } else {
                            Err(InterpreterError::InternalError("Expected array for slice assignment".to_string()))
                        }
                    } else {
                        Err(InterpreterError::InternalError("Single element assignment requires start index".to_string()))
                    }
                } else {
                    // Range slice assignment: arr[start..end] = value (not implemented yet)
                    Err(InterpreterError::InternalError("Range slice assignment not yet implemented".to_string()))
                }
            }
            Object::Dict(_) => {
                drop(obj_borrowed);
                // Dictionary assignment: dict[key] = value (only single element assignment)
                if start.is_some() && end.is_none() {
                    // Single element assignment: dict[key] = value
                    if let Some(start_expr) = start {
                        let start_val = self.evaluate(start_expr)?;
                        let start_obj = try_value!(Ok(start_val));

                        // Create ObjectKey for dictionary assignment
                        let key_borrowed = start_obj.borrow();
                        let key_object = key_borrowed.clone();
                        let object_key = ObjectKey::new(key_object);

                        let mut obj_borrowed = object_obj.borrow_mut();
                        if let Object::Dict(dict) = &mut *obj_borrowed {
                            dict.insert(object_key, value_obj.clone());
                            Ok(EvaluationResult::Value(value_obj.into()))
                        } else {
                            Err(InterpreterError::InternalError("Expected dict for assignment".to_string()))
                        }
                    } else {
                        Err(InterpreterError::InternalError("Dictionary assignment requires key index".to_string()))
                    }
                } else {
                    // Range slice assignment not supported for dictionaries
                    Err(InterpreterError::InternalError("Dictionary slice assignment not supported - use single key assignment dict[key] = value".to_string()))
                }
            }
            Object::Struct { type_name, .. } => {
                // Struct assignment: check for __setitem__ method (only single element assignment)
                let struct_name_val = *type_name;
                drop(obj_borrowed);

                if start.is_some() && end.is_none() {
                    // Single element assignment: struct[key] = value
                    if let Some(start_expr) = start {
                        let start_val = self.evaluate(start_expr)?;
                        let start_obj = try_value!(Ok(start_val));

                        // Resolve names first before method call
                        let struct_name_str = self.string_interner.resolve(struct_name_val)
                            .ok_or_else(|| InterpreterError::InternalError("Failed to resolve struct name".to_string()))?
                            .to_string();
                        let setitem_method = self.string_interner.get_or_intern("__setitem__");

                        // Call __setitem__(self, index, value)
                        let args = vec![start_obj, value_obj.clone()];
                        self.call_struct_method(object_obj, setitem_method, &args, &struct_name_str, self.expr_location(object))?;

                        // Return the assigned value
                        Ok(EvaluationResult::Value(value_obj.into()))
                    } else {
                        Err(InterpreterError::InternalError("Struct assignment requires index".to_string()))
                    }
                } else {
                    // Range slice assignment: struct[start..end] = value calls __setslice__(self, start, end, value)
                    let start_obj = if let Some(start_expr) = start {
                        let start_val = self.evaluate(start_expr)?;
                        try_value!(Ok(start_val))
                    } else {
                        Rc::new(RefCell::new(Object::Int64(0)))
                    };

                    let end_obj = if let Some(end_expr) = end {
                        let end_val = self.evaluate(end_expr)?;
                        try_value!(Ok(end_val))
                    } else {
                        Rc::new(RefCell::new(Object::Int64(i64::MAX)))
                    };

                    let struct_name_str = self.string_interner.resolve(struct_name_val)
                        .ok_or_else(|| InterpreterError::InternalError("Failed to resolve struct name".to_string()))?
                        .to_string();
                    let setslice_method = self.string_interner.get_or_intern("__setslice__");

                    // Call __setslice__(self, start, end, value)
                    let args = vec![start_obj, end_obj, value_obj.clone()];
                    self.call_struct_method(object_obj, setslice_method, &args, &struct_name_str, self.expr_location(object))?;

                    Ok(EvaluationResult::Value(value_obj.into()))
                }
            }
            _ => {
                drop(obj_borrowed);
                Err(InterpreterError::InternalError(
                    "Cannot assign to type - only arrays, dictionaries, and structs with __setitem__ are supported".to_string()
                ))
            }
        }
    }
}
