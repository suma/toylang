use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use frontend::ast::*;
use frontend::format_spec::FormatSpec;
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;
use crate::object::{Object, RcObject};
use crate::error::InterpreterError;
use crate::try_value;
use super::{EvaluationContext, EvaluationResult};

/// Width of an enum's discriminant, mirroring
/// `compiler_lower::expr::TAG_BYTE_SIZE`.
const ENUM_TAG_BYTE_SIZE: u64 = 8;

/// Compute the byte size of a runtime value by walking its Object tree.
/// Primitives have fixed widths; composite values sum their components.
///
/// Must agree with `compiler_lower::expr::compute_byte_size`, which
/// answers the same `__builtin_sizeof(v)` on every backend that goes
/// through the shared IR (the IR VM, the AOT compiler, the compiler
/// JIT). This is the tree-walker's own answer.
/// The value walk gets there for every shape except an enum, where the
/// value only knows its own variant: `ctx.enum_byte_size` reads the
/// declaration instead, so `Shape::Point` and `Shape::Rect(1, 2)`
/// report the same width. They did not — the tree-walker returned 1 and
/// 17 for those two while the compiler returned 17 for both — and
/// The `(base, index)` pair both DATA-ORIENTED Phase 2 accessors open
/// with: evaluate the pointer, the element index, and the capacity.
///
/// A macro rather than a method because the argument evaluation uses
/// `try_value!`, which returns an `EvaluationResult` from the
/// *enclosing* function when an argument hits control flow — a helper
/// would have to invent a way to hand that back.
///
/// `cap` is evaluated and dropped: this engine keys an element on its
/// index rather than on a byte offset (see the arms that use this),
/// but a side-effecting capacity expression must still behave the
/// same here as on the lanes that need the number.
macro_rules! soa_buffer_slot {
    ($self:expr, $args:expr, $who:literal) => {{
        let ptr_result = $self.evaluate(&$args[0])?;
        let ptr_obj = try_value!(Ok(ptr_result));
        let addr = ptr_obj.borrow().try_unwrap_pointer().map_err(|_| {
            InterpreterError::InternalError(concat!($who, " expects pointer as first argument").to_string())
        })?;
        let index_result = $self.evaluate(&$args[1])?;
        let index_obj = try_value!(Ok(index_result));
        let index = index_obj.borrow().try_unwrap_uint64().map_err(|_| {
            InterpreterError::InternalError(concat!($who, " expects u64 index as second argument").to_string())
        })? as usize;
        let cap_result = $self.evaluate(&$args[2])?;
        let _cap = try_value!(Ok(cap_result));
        (addr, index)
    }};
}

/// `core/std/collections/vec.t` takes its `elem_size` from whichever
/// element happened to be pushed first, so a `Vec<Option<T>>` built
/// `None`-first got a different stride than one built `Some`-first
/// (PTR-READ-ENUM).
fn object_byte_size(ctx: &EvaluationContext<'_>, value: &Object) -> Option<u64> {
    match value {
        Object::Int64(_) | Object::UInt64(_) | Object::Float64(_) | Object::Pointer(_) => Some(8),
        // NUM-W narrow widths: 1 byte for u8/i8, 2 for u16/i16,
        // 4 for u32/i32. Native sizes — packing into structs is
        // the caller's concern.
        Object::Int8(_) | Object::UInt8(_) => Some(1),
        Object::Int16(_) | Object::UInt16(_) => Some(2),
        // SIMD-F32: native single-precision width.
        Object::Float32(_) => Some(4),
        Object::Int32(_) | Object::UInt32(_) => Some(4),
        // SIMD: 128 bits, whatever the lane type.
        Object::Simd(_) => Some(16),
        Object::Bool(_) => Some(1),
        Object::Unit => Some(0),
        Object::Struct { fields, .. } => {
            let mut total: u64 = 0;
            for v in fields.values() {
                total = total.saturating_add(object_byte_size(ctx, &v.borrow())?);
            }
            Some(total)
        }
        Object::Tuple(elements) => {
            let mut total: u64 = 0;
            for e in elements.iter() {
                total = total.saturating_add(object_byte_size(ctx, &e.borrow())?);
            }
            Some(total)
        }
        Object::Array(elements) => {
            let mut total: u64 = 0;
            for e in elements.iter() {
                total = total.saturating_add(object_byte_size(ctx, &e.borrow())?);
            }
            Some(total)
        }
        Object::EnumVariant { enum_name, type_args, .. } => {
            ctx.enum_byte_size(*enum_name, type_args)
        }
        // A `str` is a pointer-sized handle on every compiled lane
        // (`compiler_lower` answers 8 for `Type::Str`), and this walk
        // is required to agree with it. Returning `None` here made
        // `Dict<str, V>` — which asks for its key width on the first
        // insert — an internal error in the tree-walker while the same
        // program ran everywhere else.
        Object::ConstString(_) | Object::String(_) => Some(8),
        // Opaque / non-serialisable values have no canonical byte size.
        Object::Dict(_)
        | Object::Null(_) | Object::Allocator(_) | Object::Range { .. }
        | Object::Closure { .. } => None,
    }
}

impl EvaluationContext<'_> {
    /// Byte size of `enum_name` instantiated at `type_args`:
    /// `u64` tag followed by *every* variant's payload, matching
    /// `compiler_lower::expr::compute_byte_size` and the layout an enum
    /// occupies at a function boundary (PTR-READ-ENUM).
    ///
    /// Read from the declaration rather than the value, because the
    /// value knows only its own variant and a size that changes with
    /// the variant is not a size — it is what made `Vec<Option<T>>`
    /// stride differently depending on which element was pushed first.
    fn enum_byte_size(&self, enum_name: DefaultSymbol, type_args: &[TypeDecl]) -> Option<u64> {
        let entry = self.enum_definitions.get(&enum_name)?;
        let subst: HashMap<DefaultSymbol, TypeDecl> = entry
            .generic_params
            .iter()
            .copied()
            .zip(type_args.iter().cloned())
            .collect();
        let mut total: u64 = ENUM_TAG_BYTE_SIZE;
        for variant in &entry.variants {
            for payload in &variant.payload_types {
                total = total.saturating_add(self.type_decl_byte_size(payload, &subst)?);
            }
        }
        Some(total)
    }

    /// Byte size of a declared type, for the members a value cannot be
    /// asked about (an enum's inactive variants).
    ///
    /// Terminates because a type that reaches itself is rejected before
    /// anything runs — `[E0013]`, `frontend::type_checker::
    /// check_recursive_types` — so the declaration graph is acyclic.
    fn type_decl_byte_size(
        &self,
        ty: &TypeDecl,
        subst: &HashMap<DefaultSymbol, TypeDecl>,
    ) -> Option<u64> {
        match ty {
            TypeDecl::Bool | TypeDecl::Int8 | TypeDecl::UInt8 => Some(1),
            TypeDecl::Int16 | TypeDecl::UInt16 => Some(2),
            TypeDecl::Int32 | TypeDecl::UInt32 => Some(4),
            TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Number | TypeDecl::Float64 => Some(8),
            // SIMD-F32: native single-precision width.
            TypeDecl::Float32 => Some(4),
            // SIMD: 128 bits, whatever the lane type. The value form
            // answers the same through `Object::Simd` (16), and the
            // compiled lanes' `compute_byte_size` has the matching
            // `Type::Vector` arm — without this the type-argument
            // form (POINTER P1) disagreed between engines.
            TypeDecl::Vector(_) => Some(16),
            // Pointer-width handles. `str` is one too on the compiler
            // side (`Type::Str` is an address into the string blob), so
            // it has a width *as a member* even though a `str` value on
            // its own has no byte size here.
            TypeDecl::Ptr | TypeDecl::String | TypeDecl::Allocator => Some(8),
            TypeDecl::Unit => Some(0),
            TypeDecl::Tuple(elements) => {
                let mut total: u64 = 0;
                for e in elements {
                    total = total.saturating_add(self.type_decl_byte_size(e, subst)?);
                }
                Some(total)
            }
            // `&T` is erased to `T` everywhere below the type checker.
            TypeDecl::Ref { inner, .. } => self.type_decl_byte_size(inner, subst),
            // A generic parameter is whatever this instance bound it
            // to; without a binding there is no width to report.
            TypeDecl::Generic(p) => self.type_decl_byte_size(subst.get(p)?, subst),
            TypeDecl::Identifier(name) => {
                if let Some(bound) = subst.get(name) {
                    return self.type_decl_byte_size(bound, subst);
                }
                self.named_type_byte_size(*name, &[], subst)
            }
            TypeDecl::Struct(name, args) | TypeDecl::Enum(name, args) => {
                self.named_type_byte_size(*name, args, subst)
            }
            _ => None,
        }
    }

    /// Size of a struct or enum named in a member position, with its
    /// own type arguments applied.
    fn named_type_byte_size(
        &self,
        name: DefaultSymbol,
        args: &[TypeDecl],
        subst: &HashMap<DefaultSymbol, TypeDecl>,
    ) -> Option<u64> {
        // Resolve the arguments through the *outer* substitution first,
        // so `Wrapper<T>` inside a `Foo<i64>` payload sees `i64`.
        let resolved: Vec<TypeDecl> = args
            .iter()
            .map(|a| match a {
                TypeDecl::Generic(p) | TypeDecl::Identifier(p) => {
                    subst.get(p).cloned().unwrap_or_else(|| a.clone())
                }
                other => other.clone(),
            })
            .collect();
        if self.enum_definitions.contains_key(&name) {
            return self.enum_byte_size(name, &resolved);
        }
        let entry = self.struct_definitions.get(&name)?;
        let inner: HashMap<DefaultSymbol, TypeDecl> = entry
            .generic_params
            .iter()
            .copied()
            .zip(resolved)
            .collect();
        let mut total: u64 = 0;
        for (_field, field_ty) in &entry.fields {
            total = total.saturating_add(self.type_decl_byte_size(field_ty, &inner)?);
        }
        Some(total)
    }

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
                Ok(EvaluationResult::Value((Object::Bool(is_null)).into()))
            }

            BuiltinMethod::StrLen => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "len() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len()
                    });
                }

                let string_value = receiver.borrow().to_string_value(self.string_interner);
                let length = string_value.len() as u64;
                Ok(EvaluationResult::Value((Object::UInt64(length)).into()))
            }

            BuiltinMethod::StrConcat => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "concat(str) takes exactly one string argument".to_string(),
                        expected: 1,
                        found: args.len()
                    });
                }

                let string_value = receiver.borrow().to_string_value(self.string_interner);

                let arg_value = self.evaluate(&args[0])?;
                let arg_obj = try_value!(Ok(arg_value));
                let arg_string = arg_obj.borrow().to_string_value(self.string_interner);

                let concatenated = format!("{}{}", string_value, arg_string);
                // Return as dynamic String, not interned - this is the key improvement
                Ok(EvaluationResult::Value((Object::String(concatenated)).into()))
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
                Ok(EvaluationResult::Value((Object::String(substring)).into()))
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
                Ok(EvaluationResult::Value((Object::Bool(contains)).into()))
            }

            BuiltinMethod::StrTrim => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "trim() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len()
                    });
                }

                let string_value = receiver.borrow().to_string_value(self.string_interner);
                let trimmed = string_value.trim().to_string();
                // Return as dynamic String, not interned
                Ok(EvaluationResult::Value((Object::String(trimmed)).into()))
            }

            BuiltinMethod::StrToUpper => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "to_upper() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len()
                    });
                }

                let string_value = receiver.borrow().to_string_value(self.string_interner);
                let upper = string_value.to_uppercase();
                // Return as dynamic String, not interned
                Ok(EvaluationResult::Value((Object::String(upper)).into()))
            }

            BuiltinMethod::StrToLower => {
                if !args.is_empty() {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "to_lower() takes no arguments".to_string(),
                        expected: 0,
                        found: args.len()
                    });
                }

                let string_value = receiver.borrow().to_string_value(self.string_interner);
                let lower = string_value.to_lowercase();
                // Return as dynamic String, not interned
                Ok(EvaluationResult::Value((Object::String(lower)).into()))
            }

            BuiltinMethod::StrSplit => {
                if args.len() != 1 {
                    return Err(InterpreterError::FunctionParameterMismatch {
                        message: "split(str) takes exactly one string argument".to_string(),
                        expected: 1,
                        found: args.len()
                    });
                }

                let string_value = receiver.borrow().to_string_value(self.string_interner);

                let separator_value = self.evaluate(&args[0])?;
                let separator_obj = try_value!(Ok(separator_value));
                let separator = separator_obj.borrow().to_string_value(self.string_interner);

                let parts: Vec<_> = string_value.split(&separator)
                    .map(|part| {
                        // Return split parts as dynamic Strings, not interned
                        Rc::new(RefCell::new(Object::String(part.to_string())))
                    })
                    .collect();

                Ok(EvaluationResult::Value(Object::Array(Box::new(parts)).into()))
            }

            // NOTE: numeric value-method arms (`I64Abs` / `F64Abs` /
            // `F64Sqrt`) lived here before Step F. The prelude's
            // extension-trait impls now cover the same surface; the
            // call-eval primitive-receiver path (Step B) routes to
            // them through the regular `method_registry`, then the
            // body forwards to `__extern_abs_i64` / `__extern_abs_f64`
            // / `__extern_sqrt_f64` (registered in
            // `evaluation/extern_math::build_default_registry`).
        }
    }

    /// Reject a builtin call of the wrong arity.
    ///
    /// Every arm below used to spell this out: a seven-line `if
    /// args.len() != n` returning a `FunctionParameterMismatch` whose
    /// message restated the name and the count already in the condition.
    /// The width and bit pattern of a fixed-width scalar, for the
    /// byte-buffer mirror in `PtrWrite` (EXTERN-BUF). `None` for
    /// anything whose bytes this engine does not model (strings,
    /// structs, enums — they live in the typed-slot map alone).
    fn scalar_bits(obj: &crate::object::Object) -> Option<(usize, u64)> {
        use crate::object::Object;
        Some(match obj {
            Object::Bool(b) => (1, *b as u64),
            Object::UInt8(v) => (1, *v as u64),
            Object::Int8(v) => (1, *v as u8 as u64),
            Object::UInt16(v) => (2, *v as u64),
            Object::Int16(v) => (2, *v as u16 as u64),
            Object::UInt32(v) => (4, *v as u64),
            Object::Int32(v) => (4, *v as u32 as u64),
            Object::UInt64(v) => (8, *v),
            Object::Int64(v) => (8, *v as u64),
            Object::Float64(v) => (8, v.to_bits()),
            Object::Float32(v) => (4, v.to_bits() as u64),
            _ => return None,
        })
    }

    /// EXTERN-BUF: read a scalar of the width the pending `val`
    /// annotation asks for straight out of the byte buffer.
    ///
    /// The typed-slot map is this engine's normal record of what a
    /// `__builtin_ptr_write` stored, and reads consult it first. Bytes
    /// that arrived some other way — a native extern filling a
    /// caller-owned buffer — leave no slot, and the only fallback was
    /// an 8-byte read, so a `Vec<u8>` filled that way was unreadable
    /// here while the other engines managed fine.
    ///
    /// `None` when the annotation is missing or is not a fixed-width
    /// scalar, so the caller keeps its existing behaviour.
    fn read_annotated_scalar_bytes(&self, addr: usize, offset: usize) -> Option<crate::object::Object> {
        let annotation = self.pending_annotation.clone()?;
        self.read_scalar_bytes_as(addr, offset, &annotation)
    }

    /// The same read against a type named outright — the shape
    /// `__builtin_ptr_read::<T>(p, off)` asks for (MEMORY-ACCESS M1).
    ///
    /// `None` when `ty` is not a fixed-width scalar, so a caller that
    /// reads a struct / `String` / enum keeps going to the typed-slot
    /// map where those live.
    fn read_scalar_bytes_as(
        &self,
        addr: usize,
        offset: usize,
        ty: &TypeDecl,
    ) -> Option<crate::object::Object> {
        use crate::object::Object;
        // Inside a generic body the type is the parameter (`val v: T
        // = ...` in `Vec::get`, or `__builtin_ptr_read::<T>` in the
        // same place), so resolve it the way `__builtin_sizeof::<T>()`
        // does.
        let resolved = match ty {
            TypeDecl::Generic(g) | TypeDecl::Identifier(g) => self
                .merged_generic_scope()
                .get(g)
                .cloned()
                .unwrap_or_else(|| ty.clone()),
            other => other.clone(),
        };
        let (width, build): (usize, fn(u64) -> Object) = match resolved {
            TypeDecl::Bool => (1, |v| Object::Bool(v != 0)),
            TypeDecl::UInt8 => (1, |v| Object::UInt8(v as u8)),
            TypeDecl::Int8 => (1, |v| Object::Int8(v as i8)),
            TypeDecl::UInt16 => (2, |v| Object::UInt16(v as u16)),
            TypeDecl::Int16 => (2, |v| Object::Int16(v as i16)),
            TypeDecl::UInt32 => (4, |v| Object::UInt32(v as u32)),
            TypeDecl::Int32 => (4, |v| Object::Int32(v as i32)),
            TypeDecl::Int64 => (8, |v| Object::Int64(v as i64)),
            TypeDecl::UInt64 => (8, Object::UInt64),
            // Floats are mirrored into the byte buffer by
            // `scalar_bits` like every other scalar; leaving them out
            // here meant a float with no typed slot fell through to
            // the 8-byte integer read and came back as a `u64` of its
            // bit pattern.
            TypeDecl::Float64 => (8, |v| Object::Float64(f64::from_bits(v))),
            TypeDecl::Float32 => (4, |v| Object::Float32(f32::from_bits(v as u32))),
            _ => return None,
        };
        let raw = self
            .heap_manager
            .borrow()
            .read_scalar_bytes(addr, offset, width)?;
        Some(build(raw))
    }

    /// One argument of a memory builtin, evaluated and unwrapped.
    ///
    /// The `mem_*` builtins take only scalars, so a control-flow
    /// result (`break` / `return` inside an argument) has nowhere
    /// sensible to go and is reported rather than silently dropped --
    /// the older arms wrote this out per argument with `try_value!`.
    fn eval_scalar_arg(
        &mut self,
        arg: &ExprRef,
        who: &str,
    ) -> Result<crate::object::RcObject, InterpreterError> {
        match self.evaluate(arg)? {
            EvaluationResult::Value(v) => Ok(v.into_rc()),
            _ => Err(InterpreterError::InternalError(format!(
                "{who}: argument did not produce a value"
            ))),
        }
    }

    fn eval_pointer_arg(&mut self, arg: &ExprRef, who: &str) -> Result<usize, InterpreterError> {
        let obj = self.eval_scalar_arg(arg, who)?;
        let addr = obj.borrow().try_unwrap_pointer().map_err(|_| {
            InterpreterError::InternalError(format!("{who} expects a pointer argument"))
        })?;
        Ok(addr)
    }

    fn eval_u64_arg(&mut self, arg: &ExprRef, who: &str) -> Result<u64, InterpreterError> {
        let obj = self.eval_scalar_arg(arg, who)?;
        let v = obj.borrow().try_unwrap_uint64().map_err(|_| {
            InterpreterError::InternalError(format!("{who} expects a u64 argument"))
        })?;
        Ok(v)
    }

    fn eval_u8_arg(&mut self, arg: &ExprRef, who: &str) -> Result<u8, InterpreterError> {
        let obj = self.eval_scalar_arg(arg, who)?;
        let v = obj.borrow().try_unwrap_uint8().map_err(|_| {
            InterpreterError::InternalError(format!("{who} expects a u8 argument"))
        })?;
        Ok(v)
    }

    fn expect_args(name: &str, args: &[ExprRef], n: usize) -> Result<(), InterpreterError> {
        if args.len() == n {
            return Ok(());
        }
        Err(InterpreterError::FunctionParameterMismatch {
            message: format!(
                "{name} takes {n} argument{}",
                if n == 1 { "" } else { "s" }
            ),
            expected: n,
            found: args.len(),
        })
    }

    /// Same, for the builtins whose message names their parameters --
    /// `ptr_offset takes 2 arguments (base, offset)`.
    fn expect_args_named(
        name: &str,
        args: &[ExprRef],
        params: &[&str],
    ) -> Result<(), InterpreterError> {
        if args.len() == params.len() {
            return Ok(());
        }
        Err(InterpreterError::FunctionParameterMismatch {
            message: format!(
                "{name} takes {} argument{} ({})",
                params.len(),
                if params.len() == 1 { "" } else { "s" },
                params.join(", ")
            ),
            expected: params.len(),
            found: args.len(),
        })
    }

    /// Evaluate builtin function calls
    /// `site` is the location of the builtin call itself, so `panic`
    /// and a failed `assert` can say where they fired.
    pub(super) fn evaluate_builtin_call(
        &mut self,
        func: &BuiltinFunction,
        args: &[ExprRef],
        site: Option<frontend::type_checker::SourceLocation>,
    ) -> Result<EvaluationResult, InterpreterError> {
        match func {
            BuiltinFunction::HeapAlloc
            | BuiltinFunction::HeapFree
            | BuiltinFunction::HeapRealloc
            | BuiltinFunction::PtrRead
            | BuiltinFunction::PtrReadTyped(_)
            | BuiltinFunction::PtrWrite
            | BuiltinFunction::PtrOffset
            | BuiltinFunction::SoaRead
            | BuiltinFunction::SoaWrite => self.builtin_heap_and_pointer(func, args, site),
            BuiltinFunction::StrLen
            | BuiltinFunction::StrToPtr
            | BuiltinFunction::StrFromBytes
            | BuiltinFunction::PtrIsNull
            | BuiltinFunction::PtrEq
            | BuiltinFunction::NullPtr => self.builtin_str_and_ptr_conversion(func, args),
            BuiltinFunction::MemStat(_)
            | BuiltinFunction::RecordAllocatorLayout
            | BuiltinFunction::MemCopy
            | BuiltinFunction::MemMove
            | BuiltinFunction::MemSet
            | BuiltinFunction::MemEq
            | BuiltinFunction::MemFind
            | BuiltinFunction::MemFindSeq
            | BuiltinFunction::CurrentAllocator
            | BuiltinFunction::DefaultAllocator => self.builtin_allocator_and_memory(func, args),
            BuiltinFunction::SizeOf
            | BuiltinFunction::SizeOfType(_)
            | BuiltinFunction::ToString
            | BuiltinFunction::Backtrace
            | BuiltinFunction::Format => self.builtin_reflection(func, args),
            BuiltinFunction::Panic
            | BuiltinFunction::Assert
            | BuiltinFunction::Print
            | BuiltinFunction::Println
            | BuiltinFunction::EPrint
            | BuiltinFunction::EPrintln => self.builtin_diagnostics(func, args, site),
            BuiltinFunction::Abs
            | BuiltinFunction::Min
            | BuiltinFunction::Max => self.builtin_numeric(func, args),
            BuiltinFunction::Simd(op) => self.builtin_simd(*op, args),
        }
    }

    /// Heap allocation and raw pointer access -- the builtins that go
    /// through the active allocator or dereference a `ptr`.
    fn builtin_heap_and_pointer(
        &mut self,
        func: &BuiltinFunction,
        args: &[ExprRef],
        site: Option<frontend::type_checker::SourceLocation>,
    ) -> Result<EvaluationResult, InterpreterError> {
        match func {
        // Memory management
        BuiltinFunction::HeapAlloc => {
            Self::expect_args("heap_alloc", args, 1)?;

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
            // MEMORY_PROFILING M2: the same packed `(line << 32) |
            // column` the compiled backends pass, read from the same
            // location pool, so attribution matches without a shared
            // id table. `site` is already the call's own position.
            let packed = site
                .map(|loc| ((loc.line as u64) << 32) | (loc.column as u64))
                .unwrap_or(0);
            // MEMORY_PROFILING M2 + DEBUG-OBS D2: the position is the
            // key; the file is remembered beside it so the report can
            // say which file `88:20` is in.
            if let Some(loc) = site {
                if let Some(path) = self.source_map.and_then(|m| m.path(loc.file)) {
                    crate::heap::note_site_file(packed, path);
                }
            }
            let addr = allocator.alloc_at(size as usize, packed);
            Ok(EvaluationResult::Value((Object::Pointer(addr)).into()))
        }

        BuiltinFunction::HeapFree => {
            Self::expect_args("heap_free", args, 1)?;

            let ptr_result = self.evaluate(&args[0])?;
            let ptr_obj = try_value!(Ok(ptr_result));
            let addr = ptr_obj.borrow().try_unwrap_pointer()
                .map_err(|_| InterpreterError::InternalError("heap_free expects pointer".to_string()))?;

            let allocator = self.allocator_stack
                .last()
                .expect("allocator_stack must always contain the global allocator")
                .clone();
            allocator.free(addr);
            Ok(EvaluationResult::Value((Object::Unit).into()))
        }

        BuiltinFunction::HeapRealloc => {
            Self::expect_args("heap_realloc", args, 2)?;

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
            // A null resize is an allocation, and gets the call site
            // the way `heap_alloc` does (M2 + D2): most stdlib
            // collections grow through exactly this shape, so a leak
            // report would otherwise attribute their first block to
            // `0:0`. A real resize keeps the site its block already
            // had.
            let new_addr = if old_addr == 0 {
                let packed = site
                    .map(|loc| ((loc.line as u64) << 32) | (loc.column as u64))
                    .unwrap_or(0);
                if let Some(loc) = site {
                    if let Some(path) = self.source_map.and_then(|m| m.path(loc.file)) {
                        crate::heap::note_site_file(packed, path);
                    }
                }
                allocator.alloc_at(new_size as usize, packed)
            } else {
                allocator.realloc(old_addr, new_size as usize)
            };
            Ok(EvaluationResult::Value((Object::Pointer(new_addr)).into()))
        }

        // Pointer operations
        BuiltinFunction::PtrRead => {
            Self::expect_args("ptr_read", args, 2)?;

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
                return Ok(EvaluationResult::Value(value.into()));
            }
            // EXTERN-BUF: bytes a native `extern fn` wrote have no
            // typed slot — it deposited raw bytes and the stale slots
            // over the range were dropped, or there never were any.
            // Read the width the annotation asks for. Without this the
            // only fallback was an 8-byte read, so a `Vec<u8>` filled
            // by `io::read_file_into` could not be read back on this
            // engine while the IR VM (which has the same fallback in
            // `ir_vm::host`) and the compiled lanes could.
            if let Some(value) = self.read_annotated_scalar_bytes(addr, offset as usize) {
                return Ok(EvaluationResult::Value(value.into()));
            }
            match self.heap_manager.borrow().read_u64(addr, offset as usize) {
                Some(value) => Ok(EvaluationResult::Value((Object::UInt64(value)).into())),
                None => Err(InterpreterError::InternalError("Invalid memory access in ptr_read".to_string())),
            }
        }

        // MEMORY-ACCESS M1: `__builtin_ptr_read::<T>(p, off)`. The
        // width is the written type, so unlike `PtrRead` above this
        // needs no `pending_annotation` and reads the same in any
        // expression position.
        //
        // The order of the two sources is also the other way round.
        // `PtrRead` consults the typed-slot map first, which makes
        // the read answer with whatever *type* was last written
        // there; asking for a `u64` over bytes written as `u8` got
        // this engine 65 where the AOT lane read 0x4241. When the
        // type is named, the bytes are the answer -- every scalar
        // write is mirrored into the byte buffer -- and the slot map
        // is the fallback for the values that live only there
        // (struct / enum / `String` / `Allocator`).
        BuiltinFunction::PtrReadTyped(ty) => {
            Self::expect_args("ptr_read", args, 2)?;

            let ptr_result = self.evaluate(&args[0])?;
            let ptr_obj = try_value!(Ok(ptr_result));
            let addr = ptr_obj.borrow().try_unwrap_pointer()
                .map_err(|_| InterpreterError::InternalError("ptr_read expects pointer as first argument".to_string()))?;

            let offset_result = self.evaluate(&args[1])?;
            let offset_obj = try_value!(Ok(offset_result));
            let offset = offset_obj.borrow().try_unwrap_uint64()
                .map_err(|_| InterpreterError::InternalError("ptr_read expects u64 offset as second argument".to_string()))?;

            if let Some(value) = self.read_scalar_bytes_as(addr, offset as usize, ty) {
                return Ok(EvaluationResult::Value(value.into()));
            }
            if let Some(value) = self.heap_manager.borrow().typed_read(addr, offset as usize) {
                return Ok(EvaluationResult::Value(value.into()));
            }
            Err(InterpreterError::InternalError(
                "Invalid memory access in ptr_read".to_string(),
            ))
        }

        BuiltinFunction::PtrWrite => {
            Self::expect_args("ptr_write", args, 3)?;

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
            // EXTERN-BUF: mirror narrower scalars into the byte buffer
            // too. Reads take the typed slot first, so this changes
            // nothing for toylang code — but a native `extern fn`
            // lent the same range (`io::write_file_bytes`) sees only
            // the bytes, and used to find zeros where a `u8` had been
            // written. The u64 case above already did this, for the
            // same reason.
            if !bytes_written {
                let scalar = Self::scalar_bits(&value_obj.borrow());
                if let Some((width, bits)) = scalar {
                    self.heap_manager
                        .borrow_mut()
                        .write_scalar_bytes(addr, offset as usize, width, bits);
                }
            }
            // For typed reads we always store into the slot map so
            // subsequent `ptr_read` calls can recover the original
            // `RcObject` (needed for bool / i64 / user structs / enums).
            self.heap_manager.borrow_mut().typed_write(addr, offset as usize, value_obj.clone());

            if matches!(value_type, TypeDecl::UInt64) && !bytes_written {
                return Err(InterpreterError::InternalError("Invalid memory access in ptr_write".to_string()));
            }
            Ok(EvaluationResult::Value((Object::Unit).into()))
        }

        // DATA-ORIENTED Phase 2: the `SoaVec<T>` column accessors.
        //
        // The compiled lanes split the buffer into one column per leaf
        // scalar and address leaf `j` of element `i` at
        // `prefix_j * cap + i * stride_j`. The tree-walker has no
        // bytes to split — its heap is a map from `(addr, offset)` to
        // whole `Object`s, which is also why `[T; N]` is a
        // `Vec<Object>` here and why `soa [T; N]` needed no
        // tree-walker change at all (Phase 0). So an element is one
        // slot keyed by its index, and `cap` is accepted and ignored:
        // it is a *layout* parameter, and layout is what this engine
        // deliberately does not model. What it must agree on is
        // values, and it does — every consistency test runs the
        // tree-walker against the three lanes that do split columns.
        //
        // Keying on the index rather than a byte offset also means a
        // grow does not move anything: `SoaVec::push` copies element
        // by element into the new buffer, and the new buffer's keys
        // are the same indices.
        BuiltinFunction::SoaRead => {
            Self::expect_args("soa_read", args, 3)?;
            let (addr, index) = soa_buffer_slot!(self, args, "soa_read");
            match self.heap_manager.borrow().typed_read(addr, index) {
                Some(value) => Ok(EvaluationResult::Value(value.into())),
                // An unwritten element. `Vec::get` / `pop` guard the
                // length before reading, so reaching this means the
                // caller read past what it wrote.
                None => Err(InterpreterError::InternalError(
                    "Invalid memory access in soa_read (element never written)".to_string(),
                )),
            }
        }

        BuiltinFunction::SoaWrite => {
            Self::expect_args("soa_write", args, 4)?;
            let (addr, index) = soa_buffer_slot!(self, args, "soa_write");
            let value_result = self.evaluate(&args[3])?;
            let value_obj = try_value!(Ok(value_result));
            self.heap_manager.borrow_mut().typed_write(addr, index, value_obj);
            Ok(EvaluationResult::Value((Object::Unit).into()))
        }

        BuiltinFunction::PtrOffset => {
            Self::expect_args_named("ptr_offset", args, &["base", "offset"])?;
            let base_result = self.evaluate(&args[0])?;
            let base_obj = try_value!(Ok(base_result));
            let base = base_obj.borrow().try_unwrap_pointer().map_err(|_| {
                InterpreterError::InternalError(
                    "ptr_offset expects pointer as first argument".to_string(),
                )
            })?;
            let offset_result = self.evaluate(&args[1])?;
            let offset_obj = try_value!(Ok(offset_result));
            let offset = offset_obj.borrow().try_unwrap_uint64().map_err(|_| {
                InterpreterError::InternalError(
                    "ptr_offset expects u64 offset as second argument".to_string(),
                )
            })?;
            // The pointer value is the raw address; an interior
            // pointer is just `base + offset`. Wrapping keeps the
            // arithmetic total, matching the AOT `iadd` lowering.
            let addr = base.wrapping_add(offset as usize);
            Ok(EvaluationResult::Value((Object::Pointer(addr)).into()))
        }
            _ => unreachable!("builtin_heap_and_pointer was handed a builtin it does not own"),
        }
    }

    /// The `str` <-> `ptr` boundary, plus the pointer predicates that
    /// only look at addresses.
    fn builtin_str_and_ptr_conversion(
        &mut self,
        func: &BuiltinFunction,
        args: &[ExprRef],
    ) -> Result<EvaluationResult, InterpreterError> {
        match func {
        BuiltinFunction::StrLen => {
            // `__builtin_str_len(s: str) -> u64` — interpreter
            // semantic: just return `s.bytes().len()`. Object
            // strings already know their length natively.
            Self::expect_args("str_len", args, 1)?;
            let s_result = self.evaluate(&args[0])?;
            let s_obj = try_value!(Ok(s_result));
            let len: u64 = match &*s_obj.borrow() {
                Object::String(s) => s.len() as u64,
                Object::ConstString(sym) => self
                    .string_interner
                    .resolve(*sym)
                    .map(|s| s.len() as u64)
                    .unwrap_or(0),
                other => {
                    return Err(InterpreterError::InternalError(format!(
                        "str_len expects str arg, got {:?}",
                        other
                    )));
                }
            };
            Ok(EvaluationResult::Value((Object::UInt64(len)).into()))
        }

        BuiltinFunction::StrToPtr => {
            // `__builtin_str_to_ptr(s: str) -> ptr` — interpreter
            // semantic: allocate a heap buffer of (len + 1) bytes
            // via the active allocator and write each UTF-8 byte
            // into it. Index `len` holds the NUL terminator (so
            // C-style cstrings work too).
            //
            // The bytes go into *both* the raw buffer and the
            // typed-slot map (MEMORY-ACCESS M2). Slots alone were
            // enough while every read consulted them first; a read
            // that names its width reads the bytes, and found a
            // zeroed buffer where the string was.
            Self::expect_args("str_to_ptr", args, 1)?;
            let s_result = self.evaluate(&args[0])?;
            let s_obj = try_value!(Ok(s_result));
            let s_borrowed = s_obj.borrow();
            let bytes: Vec<u8> = match &*s_borrowed {
                Object::String(s) => s.as_bytes().to_vec(),
                Object::ConstString(sym) => self
                    .string_interner
                    .resolve(*sym)
                    .map(|s| s.as_bytes().to_vec())
                    .unwrap_or_default(),
                other => {
                    return Err(InterpreterError::InternalError(format!(
                        "str_to_ptr expects str arg, got {:?}",
                        other
                    )));
                }
            };
            drop(s_borrowed);
            let total = bytes.len() + 1; // +1 for NUL terminator
            // Uncounted, like the IR VM's `alloc_str_bytes` and unlike
            // a program's own `__builtin_heap_alloc`: the compiled
            // backends keep literals in `.rodata`, so counting the
            // buffer this engine needs to hand out an address made the
            // allocation counters lane-dependent (MEM-COUNTER-INTERP-
            // DRIFT settled that they report what the *program* asked
            // for). It also never came back -- `as_ptr` hands out a
            // view, nobody frees it -- so every call added `len + 1`
            // live bytes and no steady-state promise could be checked
            // on this engine.
            let addr = self.heap_manager.borrow_mut().alloc_uncounted(total);
            if addr == 0 && total != 0 {
                return Err(InterpreterError::InternalError(
                    "str_to_ptr: heap allocation failed".to_string(),
                ));
            }
            {
                let mut hm = self.heap_manager.borrow_mut();
                for (i, b) in bytes.iter().enumerate() {
                    hm.typed_write(addr, i, std::rc::Rc::new(std::cell::RefCell::new(Object::UInt8(*b))));
                    hm.write_scalar_bytes(addr, i, 1, *b as u64);
                }
                // NUL terminator at offset == bytes.len().
                hm.typed_write(
                    addr,
                    bytes.len(),
                    std::rc::Rc::new(std::cell::RefCell::new(Object::UInt8(0))),
                );
                hm.write_scalar_bytes(addr, bytes.len(), 1, 0);
            }
            Ok(EvaluationResult::Value((Object::Pointer(addr)).into()))
        }

        BuiltinFunction::StrFromBytes => {
            // `__builtin_str_from_bytes(p: ptr, len: u64) -> str`,
            // the inverse of `str_to_ptr`. `read_byte_at` knows
            // where a byte actually lives (typed slot or raw
            // buffer); see its doc comment.
            Self::expect_args_named("str_from_bytes", args, &["ptr", "u64"])?;
            let ptr_result = self.evaluate(&args[0])?;
            let ptr_obj = try_value!(Ok(ptr_result));
            let addr = ptr_obj.borrow().try_unwrap_pointer().map_err(|_| {
                InterpreterError::InternalError(
                    "str_from_bytes expects a pointer as its first argument".to_string(),
                )
            })?;
            let len_result = self.evaluate(&args[1])?;
            let len_obj = try_value!(Ok(len_result));
            let len = len_obj.borrow().try_unwrap_uint64().map_err(|_| {
                InterpreterError::InternalError(
                    "str_from_bytes expects a u64 length".to_string(),
                )
            })? as usize;

            let bytes: Vec<u8> = {
                let hm = self.heap_manager.borrow();
                (0..len).map(|i| hm.read_byte_at(addr, i)).collect()
            };
            // STDLIB-TEXT §2: `str` holds valid UTF-8, and this is one
            // of the three doors into the type. A lossy conversion
            // here was the reason the same program answered `6` on
            // the tree-walker and `2` on the compiled lanes: two
            // 0xFF bytes became two U+FFFD, three bytes each, and
            // `len()` counted them. Refusing the buffer is the answer
            // that can be the same everywhere.
            let s = match String::from_utf8(bytes) {
                Ok(s) => s,
                Err(_) => {
                    return Err(self.panic_error(
                        "str_from_bytes: the bytes are not valid UTF-8".to_string(),
                        None,
                    ));
                }
            };
            Ok(EvaluationResult::Value((Object::String(s)).into()))
        }

        BuiltinFunction::PtrIsNull => {
            Self::expect_args("ptr_is_null", args, 1)?;

            let ptr_result = self.evaluate(&args[0])?;
            let ptr_obj = try_value!(Ok(ptr_result));
            let addr = ptr_obj.borrow().try_unwrap_pointer()
                .map_err(|_| InterpreterError::InternalError("ptr_is_null expects pointer".to_string()))?;
            Ok(EvaluationResult::Value((Object::Bool(addr == 0)).into()))
        }

        BuiltinFunction::PtrEq => {
            Self::expect_args("ptr_eq", args, 2)?;

            let a_result = self.evaluate(&args[0])?;
            let a_obj = try_value!(Ok(a_result));
            let a_addr = a_obj.borrow().try_unwrap_pointer()
                .map_err(|_| InterpreterError::InternalError("ptr_eq expects pointer (arg 0)".to_string()))?;
            let b_result = self.evaluate(&args[1])?;
            let b_obj = try_value!(Ok(b_result));
            let b_addr = b_obj.borrow().try_unwrap_pointer()
                .map_err(|_| InterpreterError::InternalError("ptr_eq expects pointer (arg 1)".to_string()))?;
            Ok(EvaluationResult::Value((Object::Bool(a_addr == b_addr)).into()))
        }

        BuiltinFunction::NullPtr => {
            if !args.is_empty() {
                return Err(InterpreterError::FunctionParameterMismatch {
                    message: "null_ptr takes 0 arguments".to_string(),
                    expected: 0,
                    found: args.len(),
                });
            }
            Ok(EvaluationResult::Value((Object::Pointer(0)).into()))
        }
            _ => unreachable!("builtin_str_and_ptr_conversion was handed a builtin it does not own"),
        }
    }

    /// Allocation counters, allocator introspection, and the bulk memory
    /// operations that copy or fill a region.
    fn builtin_allocator_and_memory(
        &mut self,
        func: &BuiltinFunction,
        args: &[ExprRef],
    ) -> Result<EvaluationResult, InterpreterError> {
        match func {
        // Allocation counters (MEMORY_PROFILING M4). Read from the
        // per-thread totals every `HeapManager` folds into, which
        // is what `--profile=mem` reports at exit — so a contract
        // asserting on these is asserting on the same numbers the
        // report will show.
        BuiltinFunction::MemStat(stat) => {
            if !args.is_empty() {
                return Err(InterpreterError::FunctionParameterMismatch {
                    message: format!("{}() takes 0 arguments", stat.builtin_name()),
                    expected: 0,
                    found: args.len(),
                });
            }
            let value = crate::heap::profile().field(*stat);
            Ok(EvaluationResult::Value((Object::UInt64(value)).into()))
        }

        // Allocator layout registry (MEMORY_PROFILING M3 residual).
        // A region-owning allocator pushes its final layout so
        // `--profile=mem` can report fragmentation. Populated from
        // the stdlib `SlotRegion`'s `Drop`.
        BuiltinFunction::RecordAllocatorLayout => {
            Self::expect_args_named(
                "__builtin_record_allocator_layout",
                args,
                &["name", "managed", "live", "free_blocks", "largest"],
            )?;
            let name_result = self.evaluate(&args[0])?;
            let name_obj = try_value!(Ok(name_result));
            let name = name_obj.borrow().to_display_string(self.string_interner);

            let m = self.evaluate(&args[1])?;
            let m = try_value!(Ok(m));
            let managed = m
                .borrow()
                .try_unwrap_uint64()
                .map_err(|_| {
                    InterpreterError::InternalError(
                        "record_allocator_layout expects u64 managed".to_string(),
                    )
                })?;
            let lv = self.evaluate(&args[2])?;
            let lv = try_value!(Ok(lv));
            let live = lv
                .borrow()
                .try_unwrap_uint64()
                .map_err(|_| {
                    InterpreterError::InternalError(
                        "record_allocator_layout expects u64 live".to_string(),
                    )
                })?;
            let fb = self.evaluate(&args[3])?;
            let fb = try_value!(Ok(fb));
            let free_blocks = fb
                .borrow()
                .try_unwrap_uint64()
                .map_err(|_| {
                    InterpreterError::InternalError(
                        "record_allocator_layout expects u64 free_blocks".to_string(),
                    )
                })?;
            let lg = self.evaluate(&args[4])?;
            let lg = try_value!(Ok(lg));
            let largest = lg
                .borrow()
                .try_unwrap_uint64()
                .map_err(|_| {
                    InterpreterError::InternalError(
                        "record_allocator_layout expects u64 largest".to_string(),
                    )
                })?;
            crate::heap::record_allocator_layout(&name, managed, live, free_blocks, largest);
            Ok(EvaluationResult::Value((Object::Unit).into()))
        }

        // Memory operations
        BuiltinFunction::MemCopy => {
            Self::expect_args("mem_copy", args, 3)?;

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
                Ok(EvaluationResult::Value((Object::Unit).into()))
            } else {
                Err(InterpreterError::InternalError("Invalid memory access in mem_copy".to_string()))
            }
        }

        BuiltinFunction::MemMove => {
            Self::expect_args("mem_move", args, 3)?;

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
                Ok(EvaluationResult::Value((Object::Unit).into()))
            } else {
                Err(InterpreterError::InternalError("Invalid memory access in mem_move".to_string()))
            }
        }

        BuiltinFunction::MemSet => {
            Self::expect_args("mem_set", args, 3)?;

            let ptr_result = self.evaluate(&args[0])?;
            let ptr_obj = try_value!(Ok(ptr_result));
            let addr = ptr_obj.borrow().try_unwrap_pointer()
                .map_err(|_| InterpreterError::InternalError("mem_set expects pointer as first argument".to_string()))?;

            let value_result = self.evaluate(&args[1])?;
            let value_obj = try_value!(Ok(value_result));
            // MEMORY-ACCESS M0: the fill value is a `u8` (the type
            // checker enforces it), so read it as one instead of
            // truncating a u64 here.
            let value = value_obj.borrow().try_unwrap_uint8()
                .map_err(|_| InterpreterError::InternalError("mem_set expects u8 value as second argument".to_string()))?;

            let size_result = self.evaluate(&args[2])?;
            let size_obj = try_value!(Ok(size_result));
            let size = size_obj.borrow().try_unwrap_uint64()
                .map_err(|_| InterpreterError::InternalError("mem_set expects u64 size as third argument".to_string()))?;

            if self.heap_manager.borrow_mut().set_memory(addr, value, size as usize) {
                Ok(EvaluationResult::Value((Object::Unit).into()))
            } else {
                Err(InterpreterError::InternalError("Invalid memory access in mem_set".to_string()))
            }
        }

        // MEMORY-ACCESS M3: the range questions. Same definitions as
        // `toylang_rt`'s `toy_mem_*` -- one call per range, and one
        // answer for every lane.
        BuiltinFunction::MemEq => {
            Self::expect_args("mem_eq", args, 3)?;
            let a = self.eval_pointer_arg(&args[0], "mem_eq")?;
            let b = self.eval_pointer_arg(&args[1], "mem_eq")?;
            let size = self.eval_u64_arg(&args[2], "mem_eq")? as usize;
            if size == 0 {
                return Ok(EvaluationResult::Value((Object::Bool(true)).into()));
            }
            let hm = self.heap_manager.borrow();
            let eq = match (hm.read_bytes_raw(a, size), hm.read_bytes_raw(b, size)) {
                (Some(x), Some(y)) => x == y,
                _ => false,
            };
            Ok(EvaluationResult::Value((Object::Bool(eq)).into()))
        }

        BuiltinFunction::MemFind => {
            Self::expect_args("mem_find", args, 3)?;
            let p = self.eval_pointer_arg(&args[0], "mem_find")?;
            let len = self.eval_u64_arg(&args[1], "mem_find")?;
            let byte = self.eval_u8_arg(&args[2], "mem_find")?;
            if len == 0 {
                return Ok(EvaluationResult::Value((Object::UInt64(0)).into()));
            }
            let hm = self.heap_manager.borrow();
            let at = match hm.read_bytes_raw(p, len as usize) {
                Some(hay) => hay.iter().position(|&b| b == byte).map(|i| i as u64).unwrap_or(len),
                None => len,
            };
            Ok(EvaluationResult::Value((Object::UInt64(at)).into()))
        }

        BuiltinFunction::MemFindSeq => {
            Self::expect_args("mem_find_seq", args, 4)?;
            let hay = self.eval_pointer_arg(&args[0], "mem_find_seq")?;
            let hay_len = self.eval_u64_arg(&args[1], "mem_find_seq")?;
            let needle = self.eval_pointer_arg(&args[2], "mem_find_seq")?;
            let needle_len = self.eval_u64_arg(&args[3], "mem_find_seq")?;
            if needle_len == 0 {
                return Ok(EvaluationResult::Value((Object::UInt64(0)).into()));
            }
            if needle_len > hay_len {
                return Ok(EvaluationResult::Value((Object::UInt64(hay_len)).into()));
            }
            let hm = self.heap_manager.borrow();
            let at = match (
                hm.read_bytes_raw(hay, hay_len as usize),
                hm.read_bytes_raw(needle, needle_len as usize),
            ) {
                (Some(h), Some(n)) => h
                    .windows(n.len())
                    .position(|w| w == n.as_slice())
                    .map(|i| i as u64)
                    .unwrap_or(hay_len),
                _ => hay_len,
            };
            Ok(EvaluationResult::Value((Object::UInt64(at)).into()))
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
            Ok(EvaluationResult::Value((Object::Allocator(top)).into()))
        }

        BuiltinFunction::DefaultAllocator => {
            if !args.is_empty() {
                return Err(InterpreterError::FunctionParameterMismatch {
                    message: "default_allocator() takes no arguments".to_string(),
                    expected: 0,
                    found: args.len(),
                });
            }
            Ok(EvaluationResult::Value(Object::Allocator(self.global_allocator.clone()).into()))
        }

        // `BuiltinFunction::ArenaAllocator` / `FixedBufferAllocator` /
        // `ArenaDrop` / `FixedBufferDrop` removed when the runtime
        // arena/fixed_buffer infrastructure was retired. The toylang
        // stdlib `Arena` / `FixedBuffer` (`core/std/allocator.t`) now
        // implements both policies on top of the default allocator.
            _ => unreachable!("builtin_allocator_and_memory was handed a builtin it does not own"),
        }
    }

    /// Questions a value can answer about itself: its size, its
    /// rendering, its formatted rendering.
    fn builtin_reflection(
        &mut self,
        func: &BuiltinFunction,
        args: &[ExprRef],
    ) -> Result<EvaluationResult, InterpreterError> {
        match func {
        BuiltinFunction::SizeOf => {
            Self::expect_args("__builtin_sizeof", args, 1)?;
            // Evaluate the probe expression, then walk its runtime
            // Object recursively to accumulate a byte size.
            let value = self.evaluate(&args[0])?;
            let value = try_value!(Ok(value));
            let size = object_byte_size(self, &value.borrow()).ok_or_else(|| {
                InterpreterError::InternalError(format!(
                    "__builtin_sizeof: size of value {:?} is not supported",
                    value.borrow()
                ))
            })?;
            Ok(EvaluationResult::Value((Object::UInt64(size)).into()))
        }

        BuiltinFunction::SizeOfType(ty) => {
            // POINTER P1: `__builtin_sizeof::<T>()` — the size comes
            // from the written type, resolved through the active
            // generic scopes the call boundaries pushed (a `Ptr<u64>`
            // receiver, the arguments of a generic call, the pending
            // `val` annotation). An unbound generic parameter fails
            // loudly rather than guessing a width.
            Self::expect_args("__builtin_sizeof", args, 0)?;
            let merged = self.merged_generic_scope();
            let size = self.type_decl_byte_size(ty, &merged).ok_or_else(|| {
                InterpreterError::InternalError(format!(
                    "__builtin_sizeof::<T>: cannot resolve the byte size of {ty:?} \
                     (unbound generic parameter or unsupported shape)"
                ))
            })?;
            Ok(EvaluationResult::Value((Object::UInt64(size)).into()))
        }

        // DEBUG-OBS D5: the same text a panic prints, without dying.
        // Read straight off the frames the panic path would have used,
        // and rendered by the one shared formatter.
        BuiltinFunction::Backtrace => {
            Self::expect_args("__builtin_backtrace", args, 0)?;
            let mut frames = self.call_stack.clone();
            frames.reverse();
            let entries: Vec<compiler_ir::BacktraceEntry<'_>> = frames
                .iter()
                .map(|f| compiler_ir::BacktraceEntry {
                    name: f.function.as_str(),
                    line: f.call_site.as_ref().map(|loc| loc.line),
                })
                .collect();
            let text = compiler_ir::render_backtrace(&entries);
            Ok(EvaluationResult::Value(
                Object::String(text.trim_start_matches('\n').to_string()).into(),
            ))
        }

        BuiltinFunction::ToString => {
            Self::expect_args("__builtin_to_string", args, 1)?;
            // Same display formatting as `print` / `println`.
            // Powers the string-interpolation desugaring at the
            // parser level (`"hello {x}"` → `"hello ".concat(
            // __builtin_to_string(x))`).
            let value = self.evaluate(&args[0])?;
            let value = try_value!(Ok(value));
            let rendered = value.borrow().to_display_string(self.string_interner);
            Ok(EvaluationResult::Value(Object::String(rendered).into()))
        }

        // STR-INTERP-FMT: `__builtin_format(value, spec)`. The
        // spec is a parser-packed constant, so decoding it here
        // costs one `unpack` per call and nothing at the call
        // site. Rendering lives in
        // `frontend::format_spec::FormatSpec` so the interpreter
        // and the type checker agree on the grammar; the AOT / JIT
        // runtime (`toylang_rt`) reimplements the same rules
        // because it cannot depend on this crate, and the
        // cross-backend tests pin the two together.
        BuiltinFunction::Format => {
            Self::expect_args("__builtin_format", args, 2)?;
            let value = self.evaluate(&args[0])?;
            let value = try_value!(Ok(value));
            let spec_obj = self.evaluate(&args[1])?;
            let spec_obj = try_value!(Ok(spec_obj));
            let code = match &*spec_obj.borrow() {
                Object::UInt64(v) => *v,
                Object::Int64(v) => *v as u64,
                other => {
                    return Err(InterpreterError::InternalError(format!(
                        "__builtin_format: spec must be a u64 constant, got {other:?}"
                    )));
                }
            };
            let spec = FormatSpec::unpack(code);
            let rendered = format_object(&value.borrow(), &spec, self.string_interner)
                .ok_or_else(|| {
                    InterpreterError::InternalError(format!(
                        "__builtin_format: a format spec does not apply to {:?}",
                        value.borrow()
                    ))
                })?;
            Ok(EvaluationResult::Value(Object::String(rendered).into()))
        }
            _ => unreachable!("builtin_reflection was handed a builtin it does not own"),
        }
    }

    /// Builtins that talk to the user or stop the program.
    fn builtin_diagnostics(
        &mut self,
        func: &BuiltinFunction,
        args: &[ExprRef],
        site: Option<frontend::type_checker::SourceLocation>,
    ) -> Result<EvaluationResult, InterpreterError> {
        match func {
        BuiltinFunction::Panic => {
            Self::expect_args("panic", args, 1)?;
            // The message arg is required to be `str` by the type
            // checker, but we render via `to_display_string` so any
            // accidental type mismatch still produces something
            // human-readable (defensive fallback).
            let value = self.evaluate(&args[0])?;
            let value = try_value!(Ok(value));
            let message = value.borrow().to_display_string(self.string_interner);
            Err(self.panic_error(message, site))
        }

        BuiltinFunction::Assert => {
            Self::expect_args_named("assert", args, &["cond", "msg"])?;
            // Evaluate the condition first; only build the message
            // string when it actually fails so the happy path stays
            // cheap. The type checker guarantees `bool` and `str`.
            let cond_val = self.evaluate(&args[0])?;
            let cond_val = try_value!(Ok(cond_val));
            let passed = cond_val
                .borrow()
                .try_unwrap_bool()
                .map_err(InterpreterError::ObjectError)?;
            if passed {
                return Ok(EvaluationResult::Value((Object::Unit).into()));
            }
            let msg_val = self.evaluate(&args[1])?;
            let msg_val = try_value!(Ok(msg_val));
            let message = msg_val.borrow().to_display_string(self.string_interner);
            Err(self.panic_error(message, site))
        }

        BuiltinFunction::Print
        | BuiltinFunction::Println
        | BuiltinFunction::EPrint
        | BuiltinFunction::EPrintln => {
            // RUNTIME-LIB P0-A: the stderr pair renders identically —
            // only the sink differs.
            let name = match func {
                BuiltinFunction::Print => "print",
                BuiltinFunction::Println => "println",
                BuiltinFunction::EPrint => "eprint",
                _ => "eprintln",
            };
            Self::expect_args(name, args, 1)?;
            let value = self.evaluate(&args[0])?;
            let value = try_value!(Ok(value));
            let rendered = value.borrow().to_display_string(self.string_interner);
            match func {
                BuiltinFunction::Print => crate::output::print_text(&rendered),
                BuiltinFunction::Println => crate::output::println_text(&rendered),
                BuiltinFunction::EPrint => crate::output::eprint_text(&rendered),
                _ => crate::output::eprintln_text(&rendered),
            }
            Ok(EvaluationResult::Value((Object::Unit).into()))
        }
            _ => unreachable!("builtin_diagnostics was handed a builtin it does not own"),
        }
    }

    /// The numeric builtins that stayed here when the rest moved to the
    /// `math` module's extension-trait impls.
    fn builtin_numeric(
        &mut self,
        func: &BuiltinFunction,
        args: &[ExprRef],
    ) -> Result<EvaluationResult, InterpreterError> {
        match func {
        BuiltinFunction::Abs => {
            Self::expect_args("abs", args, 1)?;
            let v = self.evaluate(&args[0])?;
            let v = try_value!(Ok(v));
            // Polymorphic dispatch: i64 -> wrapping_abs (so
            // `i64::MIN` stays at `i64::MIN` instead of
            // panicking), f64 -> IEEE 754 abs (matches C's
            // `fabs`; preserves NaN, flips the sign bit).
            let v_borrow = v.borrow();
            if let Ok(n) = v_borrow.try_unwrap_int64() {
                return Ok(EvaluationResult::Value(
                    Object::Int64(n.wrapping_abs()).into(),
                ));
            }
            if let Ok(x) = v_borrow.try_unwrap_float64() {
                return Ok(EvaluationResult::Value(Object::Float64(x.abs()).into()));
            }
            Err(InterpreterError::InternalError(
                "abs expects an i64 or f64 argument".to_string(),
            ))
        }

        // NOTE: f64 math dispatch arms (Pow/Sqrt and Sin..=Ceil)
        // lived here before Phase 4. The `math` module now
        // declares each as `extern fn __extern_*_f64` and the
        // interpreter routes them through
        // `evaluation/extern_math::build_default_registry`.

        BuiltinFunction::Min | BuiltinFunction::Max => {
            let name = if matches!(func, BuiltinFunction::Min) { "min" } else { "max" };
            Self::expect_args(name, args, 2)?;
            let a = self.evaluate(&args[0])?;
            let a = try_value!(Ok(a));
            let b = self.evaluate(&args[1])?;
            let b = try_value!(Ok(b));
            let pick_min = matches!(func, BuiltinFunction::Min);
            // The type-checker has already enforced matching i64
            // or u64 operands, so a borrow + concrete unwrap pair
            // is enough.
            let a_borrow = a.borrow();
            if let Ok(av) = a_borrow.try_unwrap_int64() {
                let bv = b.borrow().try_unwrap_int64().map_err(|_| {
                    InterpreterError::InternalError(
                        "min/max operands must agree on i64 / u64".to_string(),
                    )
                })?;
                let result = if pick_min { av.min(bv) } else { av.max(bv) };
                return Ok(EvaluationResult::Value(Object::Int64(result).into()));
            }
            if let Ok(av) = a_borrow.try_unwrap_uint64() {
                let bv = b.borrow().try_unwrap_uint64().map_err(|_| {
                    InterpreterError::InternalError(
                        "min/max operands must agree on i64 / u64".to_string(),
                    )
                })?;
                let result = if pick_min { av.min(bv) } else { av.max(bv) };
                return Ok(EvaluationResult::Value(Object::UInt64(result).into()));
            }
            Err(InterpreterError::InternalError(
                "min/max expects i64 or u64 operands".to_string(),
            ))
        }
            _ => unreachable!("builtin_numeric was handed a builtin it does not own"),
        }
    }

}

/// STR-INTERP-FMT: render one `Object` under a format spec.
///
/// Returns `None` for a value no spec applies to. The type checker
/// rejects those at the call site, so reaching `None` means a
/// compound slipped through and the caller reports it as an internal
/// error rather than printing something arbitrary.
///
/// Signed values hand their magnitude and sign to `render_uint`
/// separately, so a non-decimal radix can show the two's-complement
/// pattern at the value's own width (`{-1i32:x}` is `ffffffff`, not
/// 16 digits) while decimal keeps the `-` prefix.
fn format_object(
    value: &Object,
    spec: &FormatSpec,
    interner: &string_interner::StringInterner<string_interner::DefaultBackend>,
) -> Option<String> {
    let signed = |v: i64, bits: u32| spec.render_uint(v.unsigned_abs(), v < 0, bits);
    Some(match value {
        Object::Int64(v) => signed(*v, 64),
        Object::Int32(v) => signed(*v as i64, 32),
        Object::Int16(v) => signed(*v as i64, 16),
        Object::Int8(v) => signed(*v as i64, 8),
        Object::UInt64(v) => spec.render_uint(*v, false, 64),
        Object::UInt32(v) => spec.render_uint(*v as u64, false, 32),
        Object::UInt16(v) => spec.render_uint(*v as u64, false, 16),
        Object::UInt8(v) => spec.render_uint(*v as u64, false, 8),
        Object::Float64(v) => spec.render_f64(*v),
        // STDLIB-NUMERIC N5.
        Object::Float32(v) => spec.render_f32(*v),
        Object::Bool(v) => spec.render_text(if *v { "true" } else { "false" }),
        Object::String(s) => spec.render_text(s),
        Object::ConstString(sym) => {
            spec.render_text(interner.resolve(*sym).unwrap_or(""))
        }
        _ => return None,
    })
}
