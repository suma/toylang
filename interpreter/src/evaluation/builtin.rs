use std::cell::RefCell;
use std::rc::Rc;
use frontend::ast::*;
use frontend::type_decl::TypeDecl;
use crate::object::{Object, RcObject};
use crate::error::InterpreterError;
use crate::try_value;
use super::{EvaluationContext, EvaluationResult};

/// Compute the byte size of a runtime value by walking its Object tree.
/// Primitives have fixed widths; composite values sum their components.
/// Enum variants include a 1-byte tag — note that this yields a
/// variant-specific size, so `List<SomeEnum>` users who need a uniform
/// stride should probe the largest variant.
fn object_byte_size(value: &Object) -> Option<u64> {
    match value {
        Object::Int64(_) | Object::UInt64(_) | Object::Float64(_) | Object::Pointer(_) => Some(8),
        Object::Bool(_) => Some(1),
        Object::Unit => Some(0),
        Object::Struct { fields, .. } => {
            let mut total: u64 = 0;
            for v in fields.values() {
                total = total.saturating_add(object_byte_size(&v.borrow())?);
            }
            Some(total)
        }
        Object::Tuple(elements) => {
            let mut total: u64 = 0;
            for e in elements.iter() {
                total = total.saturating_add(object_byte_size(&e.borrow())?);
            }
            Some(total)
        }
        Object::Array(elements) => {
            let mut total: u64 = 0;
            for e in elements.iter() {
                total = total.saturating_add(object_byte_size(&e.borrow())?);
            }
            Some(total)
        }
        Object::EnumVariant { values, .. } => {
            // 1-byte tag + payload sizes.
            let mut total: u64 = 1;
            for v in values.iter() {
                total = total.saturating_add(object_byte_size(&v.borrow())?);
            }
            Some(total)
        }
        // Opaque / non-serialisable values have no canonical byte size.
        Object::ConstString(_) | Object::String(_) | Object::Dict(_)
        | Object::Null(_) | Object::Allocator(_) | Object::Range { .. } => None,
    }
}

impl EvaluationContext<'_> {
    /// Evaluate builtin method calls
    pub(super) fn evaluate_builtin_method_call(&mut self, receiver: &ExprRef, method: &BuiltinMethod, args: &Vec<ExprRef>) -> Result<EvaluationResult, InterpreterError> {
        let receiver_value = self.evaluate(receiver)?;
        let receiver_obj = try_value!(Ok(receiver_value));

        self.execute_builtin_method(&receiver_obj, method, args)
    }

    /// Execute builtin method with table-driven approach
    fn execute_builtin_method(&mut self, receiver: &RcObject, method: &BuiltinMethod, args: &Vec<ExprRef>) -> Result<EvaluationResult, InterpreterError> {
        match method {
            BuiltinMethod::IsNull => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "is_null() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len()
                    });
                }
                let is_null = receiver.borrow().is_null();
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Bool(is_null)))))
            }

            BuiltinMethod::StrLen => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "len() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len()
                    });
                }

                let string_value = receiver.borrow().to_string_value(&self.string_interner);
                let length = string_value.len() as u64;
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::UInt64(length)))))
            }

            BuiltinMethod::StrConcat => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "concat(str) takes exactly one string argument".to_string(),
                        expected: 1,
                        found: args.len()
                    });
                }

                let string_value = receiver.borrow().to_string_value(&self.string_interner);

                let arg_value = self.evaluate(&args[0])?;
                let arg_obj = try_value!(Ok(arg_value));
                let arg_string = arg_obj.borrow().to_string_value(&self.string_interner);

                let concatenated = format!("{}{}", string_value, arg_string);
                // Return as dynamic String, not interned - this is the key improvement
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(concatenated)))))
            }

            BuiltinMethod::StrSubstring => {
                if args.len() != 2 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "substring(start, end) takes exactly two u64 arguments".to_string(),
                        expected: 2,
                        found: args.len()
                    });
                }

                let string_symbol = receiver.borrow().try_unwrap_string().map_err(InterpreterError::ObjectError)?;
                let string_value = self.string_interner.resolve(string_symbol)
                    .ok_or_else(|| InterpreterError::InternalError("String symbol not found in interner".to_string()))?
                    .to_string();

                let start_value = self.evaluate(&args[0])?;
                let start_obj = try_value!(Ok(start_value));
                let start = start_obj.borrow().try_unwrap_uint64().map_err(InterpreterError::ObjectError)? as usize;

                let end_value = self.evaluate(&args[1])?;
                let end_obj = try_value!(Ok(end_value));
                let end = end_obj.borrow().try_unwrap_uint64().map_err(InterpreterError::ObjectError)? as usize;

                if start >= string_value.len() || end > string_value.len() || start > end {
                    return Err(InterpreterError::InternalError("Invalid substring indices".to_string()));
                }

                let substring = string_value[start..end].to_string();
                // Return as dynamic String, not interned
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(substring)))))
            }

            BuiltinMethod::StrContains => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "contains(str) takes exactly one string argument".to_string(),
                        expected: 1,
                        found: args.len()
                    });
                }

                let string_symbol = receiver.borrow().try_unwrap_string().map_err(InterpreterError::ObjectError)?;
                let string_value = self.string_interner.resolve(string_symbol)
                    .ok_or_else(|| InterpreterError::InternalError("String symbol not found in interner".to_string()))?
                    .to_string();

                let arg_value = self.evaluate(&args[0])?;
                let arg_obj = try_value!(Ok(arg_value));
                let arg_symbol = arg_obj.borrow().try_unwrap_string().map_err(InterpreterError::ObjectError)?;
                let arg_string = self.string_interner.resolve(arg_symbol)
                    .ok_or_else(|| InterpreterError::InternalError("Argument string symbol not found in interner".to_string()))?
                    .to_string();

                let contains = string_value.contains(&arg_string);
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Bool(contains)))))
            }

            BuiltinMethod::StrTrim => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "trim() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len()
                    });
                }

                let string_value = receiver.borrow().to_string_value(&self.string_interner);
                let trimmed = string_value.trim().to_string();
                // Return as dynamic String, not interned
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(trimmed)))))
            }

            BuiltinMethod::StrToUpper => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "to_upper() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len()
                    });
                }

                let string_value = receiver.borrow().to_string_value(&self.string_interner);
                let upper = string_value.to_uppercase();
                // Return as dynamic String, not interned
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(upper)))))
            }

            BuiltinMethod::StrToLower => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "to_lower() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len()
                    });
                }

                let string_value = receiver.borrow().to_string_value(&self.string_interner);
                let lower = string_value.to_lowercase();
                // Return as dynamic String, not interned
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::String(lower)))))
            }

            BuiltinMethod::StrSplit => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "split(str) takes exactly one string argument".to_string(),
                        expected: 1,
                        found: args.len()
                    });
                }

                let string_value = receiver.borrow().to_string_value(&self.string_interner);

                let separator_value = self.evaluate(&args[0])?;
                let separator_obj = try_value!(Ok(separator_value));
                let separator = separator_obj.borrow().to_string_value(&self.string_interner);

                let parts: Vec<_> = string_value.split(&separator)
                    .map(|part| {
                        // Return split parts as dynamic Strings, not interned
                        Rc::new(RefCell::new(Object::String(part.to_string())))
                    })
                    .collect();

                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Array(Box::new(parts))))))
            }
        }
    }

    /// Evaluate builtin function calls
    pub(super) fn evaluate_builtin_call(&mut self, func: &BuiltinFunction, args: &[ExprRef]) -> Result<EvaluationResult, InterpreterError> {
        match func {
            // Memory management
            BuiltinFunction::HeapAlloc => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "heap_alloc takes 1 argument".to_string(),
                        expected: 1,
                        found: args.len(),
                    });
                }

                let size_result = self.evaluate(&args[0])?;
                let size_obj = try_value!(Ok(size_result));
                let size = size_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("heap_alloc expects u64 size".to_string()))?;

                // Route allocation through the innermost `with`-bound allocator.
                // `allocator_stack.last()` is guaranteed to be Some because the
                // global allocator sits at the bottom of the stack.
                let allocator = self.allocator_stack
                    .last()
                    .expect("allocator_stack must always contain the global allocator")
                    .clone();
                let addr = allocator.alloc(size as usize);
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Pointer(addr)))))
            }

            BuiltinFunction::HeapFree => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "heap_free takes 1 argument".to_string(),
                        expected: 1,
                        found: args.len(),
                    });
                }

                let ptr_result = self.evaluate(&args[0])?;
                let ptr_obj = try_value!(Ok(ptr_result));
                let addr = ptr_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("heap_free expects pointer".to_string()))?;

                let allocator = self.allocator_stack
                    .last()
                    .expect("allocator_stack must always contain the global allocator")
                    .clone();
                allocator.free(addr);
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit))))
            }

            BuiltinFunction::HeapRealloc => {
                if args.len() != 2 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "heap_realloc takes 2 arguments".to_string(),
                        expected: 2,
                        found: args.len(),
                    });
                }

                let ptr_result = self.evaluate(&args[0])?;
                let ptr_obj = try_value!(Ok(ptr_result));
                let old_addr = ptr_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("heap_realloc expects pointer as first argument".to_string()))?;

                let size_result = self.evaluate(&args[1])?;
                let size_obj = try_value!(Ok(size_result));
                let new_size = size_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("heap_realloc expects u64 size as second argument".to_string()))?;

                let allocator = self.allocator_stack
                    .last()
                    .expect("allocator_stack must always contain the global allocator")
                    .clone();
                let new_addr = allocator.realloc(old_addr, new_size as usize);
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Pointer(new_addr)))))
            }

            // Pointer operations
            BuiltinFunction::PtrRead => {
                if args.len() != 2 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "ptr_read takes 2 arguments".to_string(),
                        expected: 2,
                        found: args.len(),
                    });
                }

                let ptr_result = self.evaluate(&args[0])?;
                let ptr_obj = try_value!(Ok(ptr_result));
                let addr = ptr_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("ptr_read expects pointer as first argument".to_string()))?;

                let offset_result = self.evaluate(&args[1])?;
                let offset_obj = try_value!(Ok(offset_result));
                let offset = offset_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("ptr_read expects u64 offset as second argument".to_string()))?;

                // Prefer a previously-stashed typed slot (non-u64 writes and
                // generic `List<T>` reads both round-trip through this map).
                // Fall back to the byte-level u64 read so the classic
                // List<u64> path keeps working.
                if let Some(value) = self.heap_manager.borrow().typed_read(addr, offset as usize) {
                    return Ok(EvaluationResult::Value(value));
                }
                match self.heap_manager.borrow().read_u64(addr, offset as usize) {
                    Some(value) => Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::UInt64(value))))),
                    None => Err(InterpreterError::InternalError("Invalid memory access in ptr_read".to_string())),
                }
            }

            BuiltinFunction::PtrWrite => {
                if args.len() != 3 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "ptr_write takes 3 arguments".to_string(),
                        expected: 3,
                        found: args.len(),
                    });
                }

                let ptr_result = self.evaluate(&args[0])?;
                let ptr_obj = try_value!(Ok(ptr_result));
                let addr = ptr_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("ptr_write expects pointer as first argument".to_string()))?;

                let offset_result = self.evaluate(&args[1])?;
                let offset_obj = try_value!(Ok(offset_result));
                let offset = offset_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("ptr_write expects u64 offset as second argument".to_string()))?;

                let value_result = self.evaluate(&args[2])?;
                let value_obj = try_value!(Ok(value_result));

                // Snapshot the value type so u64 writes can continue to land
                // in the byte buffer (for existing consumers / future native
                // codegen), while everything else is recorded only in the
                // typed-slot map.
                let value_type = value_obj.borrow().get_type();
                let bytes_written = matches!(value_type, TypeDecl::UInt64) && {
                    let v = value_obj.borrow().try_unwrap_uint64().unwrap();
                    self.heap_manager.borrow_mut().write_u64(addr, offset as usize, v)
                };
                // For typed reads we always store into the slot map so
                // subsequent `ptr_read` calls can recover the original
                // `RcObject` (needed for bool / i64 / user structs / enums).
                self.heap_manager.borrow_mut().typed_write(addr, offset as usize, value_obj.clone());

                if matches!(value_type, TypeDecl::UInt64) && !bytes_written {
                    return Err(InterpreterError::InternalError("Invalid memory access in ptr_write".to_string()));
                }
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit))))
            }

            BuiltinFunction::PtrIsNull => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "ptr_is_null takes 1 argument".to_string(),
                        expected: 1,
                        found: args.len(),
                    });
                }

                let ptr_result = self.evaluate(&args[0])?;
                let ptr_obj = try_value!(Ok(ptr_result));
                let addr = ptr_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("ptr_is_null expects pointer".to_string()))?;
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Bool(addr == 0)))))
            }

            // Memory operations
            BuiltinFunction::MemCopy => {
                if args.len() != 3 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "mem_copy takes 3 arguments".to_string(),
                        expected: 3,
                        found: args.len(),
                    });
                }

                let src_result = self.evaluate(&args[0])?;
                let src_obj = try_value!(Ok(src_result));
                let src_addr = src_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("mem_copy expects pointer as first argument".to_string()))?;

                let dest_result = self.evaluate(&args[1])?;
                let dest_obj = try_value!(Ok(dest_result));
                let dest_addr = dest_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("mem_copy expects pointer as second argument".to_string()))?;

                let size_result = self.evaluate(&args[2])?;
                let size_obj = try_value!(Ok(size_result));
                let size = size_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("mem_copy expects u64 size as third argument".to_string()))?;

                if self.heap_manager.borrow_mut().copy_memory(src_addr, dest_addr, size as usize) {
                    Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit))))
                } else {
                    Err(InterpreterError::InternalError("Invalid memory access in mem_copy".to_string()))
                }
            }

            BuiltinFunction::MemMove => {
                if args.len() != 3 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "mem_move takes 3 arguments".to_string(),
                        expected: 3,
                        found: args.len(),
                    });
                }

                let src_result = self.evaluate(&args[0])?;
                let src_obj = try_value!(Ok(src_result));
                let src_addr = src_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("mem_move expects pointer as first argument".to_string()))?;

                let dest_result = self.evaluate(&args[1])?;
                let dest_obj = try_value!(Ok(dest_result));
                let dest_addr = dest_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("mem_move expects pointer as second argument".to_string()))?;

                let size_result = self.evaluate(&args[2])?;
                let size_obj = try_value!(Ok(size_result));
                let size = size_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("mem_move expects u64 size as third argument".to_string()))?;

                if self.heap_manager.borrow_mut().move_memory(src_addr, dest_addr, size as usize) {
                    Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit))))
                } else {
                    Err(InterpreterError::InternalError("Invalid memory access in mem_move".to_string()))
                }
            }

            BuiltinFunction::MemSet => {
                if args.len() != 3 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "mem_set takes 3 arguments".to_string(),
                        expected: 3,
                        found: args.len(),
                    });
                }

                let ptr_result = self.evaluate(&args[0])?;
                let ptr_obj = try_value!(Ok(ptr_result));
                let addr = ptr_obj.borrow().try_unwrap_pointer()
                    .map_err(|_| InterpreterError::InternalError("mem_set expects pointer as first argument".to_string()))?;

                let value_result = self.evaluate(&args[1])?;
                let value_obj = try_value!(Ok(value_result));
                let value = value_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("mem_set expects u64 value as second argument".to_string()))?;

                let size_result = self.evaluate(&args[2])?;
                let size_obj = try_value!(Ok(size_result));
                let size = size_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError("mem_set expects u64 size as third argument".to_string()))?;

                if self.heap_manager.borrow_mut().set_memory(addr, value as u8, size as usize) {
                    Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit))))
                } else {
                    Err(InterpreterError::InternalError("Invalid memory access in mem_set".to_string()))
                }
            }

            BuiltinFunction::CurrentAllocator => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "current_allocator() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len(),
                    });
                }
                // `allocator_stack.last()` is guaranteed non-None because the global
                // allocator is always at the bottom.
                let top = self.allocator_stack
                    .last()
                    .expect("allocator_stack must always contain the global allocator")
                    .clone();
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Allocator(top)))))
            }

            BuiltinFunction::DefaultAllocator => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "default_allocator() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len(),
                    });
                }
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(
                    Object::Allocator(self.global_allocator.clone()),
                ))))
            }

            BuiltinFunction::ArenaAllocator => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "arena_allocator() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len(),
                    });
                }
                // Fresh arena sharing the same underlying HeapManager. Bulk free
                // happens when the last Rc to this arena is dropped.
                let arena: Rc<dyn crate::heap::Allocator> = Rc::new(
                    crate::heap::ArenaAllocator::new(self.heap_manager.clone()),
                );
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(
                    Object::Allocator(arena),
                ))))
            }

            BuiltinFunction::SizeOf => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "__builtin_sizeof takes 1 argument".to_string(),
                        expected: 1,
                        found: args.len(),
                    });
                }
                // Evaluate the probe expression, then walk its runtime
                // Object recursively to accumulate a byte size.
                let value = self.evaluate(&args[0])?;
                let value = try_value!(Ok(value));
                let size = object_byte_size(&value.borrow()).ok_or_else(|| {
                    InterpreterError::InternalError(format!(
                        "__builtin_sizeof: size of value {:?} is not supported",
                        value.borrow()
                    ))
                })?;
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::UInt64(size)))))
            }

            BuiltinFunction::Panic => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "panic takes 1 argument".to_string(),
                        expected: 1,
                        found: args.len(),
                    });
                }
                // The message arg is required to be `str` by the type
                // checker, but we render via `to_display_string` so any
                // accidental type mismatch still produces something
                // human-readable (defensive fallback).
                let value = self.evaluate(&args[0])?;
                let value = try_value!(Ok(value));
                let message = value.borrow().to_display_string(&self.string_interner);
                Err(InterpreterError::Panic { message })
            }

            BuiltinFunction::Print | BuiltinFunction::Println => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: format!(
                            "{} takes 1 argument",
                            if matches!(func, BuiltinFunction::Print) { "print" } else { "println" }
                        ),
                        expected: 1,
                        found: args.len(),
                    });
                }
                let value = self.evaluate(&args[0])?;
                let value = try_value!(Ok(value));
                let rendered = value.borrow().to_display_string(&self.string_interner);
                if matches!(func, BuiltinFunction::Println) {
                    println!("{}", rendered);
                } else {
                    print!("{}", rendered);
                }
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(Object::Unit))))
            }

            BuiltinFunction::FixedBufferAllocator => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "fixed_buffer_allocator(capacity) takes 1 argument".to_string(),
                        expected: 1,
                        found: args.len(),
                    });
                }
                let capacity_result = self.evaluate(&args[0])?;
                let capacity_obj = try_value!(Ok(capacity_result));
                let capacity = capacity_obj.borrow().try_unwrap_uint64()
                    .map_err(|_| InterpreterError::InternalError(
                        "fixed_buffer_allocator expects u64 capacity".to_string()
                    ))?;
                let allocator: Rc<dyn crate::heap::Allocator> = Rc::new(
                    crate::heap::FixedBufferAllocator::new(self.heap_manager.clone(), capacity as usize),
                );
                Ok(EvaluationResult::Value(Rc::new(RefCell::new(
                    Object::Allocator(allocator),
                ))))
            }
        }
    }
}
