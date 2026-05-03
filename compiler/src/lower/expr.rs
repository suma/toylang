//! Expression-side dispatchers and the call-arg expander.
//!
//! - `lower_call_args`: evaluate a call's argument list
//!   (`Expr::ExprList(items)`) into a flat `Vec<ValueId>`.
//!   Each argument is lowered through the regular expression
//!   path; struct- / tuple- / enum-typed identifier arguments
//!   are expanded into per-leaf values matching the callee
//!   signature.
//! - `lower_expr`: per-`Expr` switch. Routes literals through
//!   `Const`, identifiers through `LoadLocal`, and delegates
//!   compound shapes (binary / unary / call / method / field /
//!   tuple / struct / enum / match / if / cast / array / slice
//!   / range / block) to the matching `lower_*` helper. Stash
//!   slots (`pending_struct_value` / `pending_tuple_value` /
//!   `pending_enum_storage`) carry compound results that don't
//!   fit a single `ValueId`.
//! - `lower_builtin_call`: dispatcher for `BuiltinFunction`
//!   invocations. Routes `print` / `println` / `panic` /
//!   `assert` / `__builtin_*` to the matching helper.

use frontend::ast::{BuiltinFunction, Expr, ExprRef};

use super::bindings::{flatten_struct_locals, flatten_tuple_element_locals, Binding};
use super::FunctionLower;
use crate::ir::{Const, InstKind, Terminator, Type, ValueId};

impl<'a> FunctionLower<'a> {
    pub(super) fn lower_call_args(&mut self, args_ref: &ExprRef) -> Result<Vec<ValueId>, String> {
        let args_expr = self
            .program
            .expression
            .get(args_ref)
            .ok_or_else(|| "call args missing".to_string())?;
        let items: Vec<ExprRef> = match args_expr {
            Expr::ExprList(items) => items,
            _ => return Err("call arguments must be an ExprList".to_string()),
        };
        let mut values: Vec<ValueId> = Vec::with_capacity(items.len());
        for a in &items {
            // Struct-typed identifier argument: expand into per-field
            // values in declaration order. Anything else flows through
            // `lower_expr`.
            if let Some(Expr::Identifier(sym)) = self.program.expression.get(a) {
                if let Some(Binding::Struct { fields, .. }) = self.bindings.get(&sym).cloned() {
                    let leaves = flatten_struct_locals(&fields);
                    for (local, ty) in &leaves {
                        let v = self
                            .emit(InstKind::LoadLocal(*local), Some(*ty))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                    }
                    continue;
                }
                if let Some(Binding::Tuple { elements }) = self.bindings.get(&sym).cloned() {
                    // Tuple-typed identifier argument: expand into
                    // one value per leaf scalar, in declaration order
                    // (recursing through compound elements).
                    for (local, ty) in flatten_tuple_element_locals(&elements) {
                        let v = self
                            .emit(InstKind::LoadLocal(local), Some(ty))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                    }
                    continue;
                }
                if let Some(Binding::Enum(storage)) = self.bindings.get(&sym).cloned() {
                    // Enum-typed identifier argument: same shape as
                    // the function-boundary flattening — tag first,
                    // then each variant's payloads in declaration
                    // order, recursing through nested enum slots.
                    let vs = self.load_enum_locals(&storage);
                    values.extend(vs);
                    continue;
                }
            }
            let v = self
                .lower_expr(a)?
                .ok_or_else(|| "call argument produced no value".to_string())?;
            values.push(v);
        }
        Ok(values)
    }


    // -- expression lowering -------------------------------------------------------

    pub(super) fn lower_expr(&mut self, expr_ref: &ExprRef) -> Result<Option<ValueId>, String> {
        let expr = self
            .program
            .expression
            .get(expr_ref)
            .ok_or_else(|| "missing expr".to_string())?;
        if self.is_unreachable() {
            return Ok(None);
        }
        match expr {
            Expr::Block(stmts) => {
                let mut last: Option<ValueId> = None;
                for s in &stmts {
                    last = self.lower_stmt(s)?;
                    if self.is_unreachable() {
                        break;
                    }
                }
                Ok(last)
            }
            Expr::Int64(v) => Ok(self.emit(InstKind::Const(Const::I64(v)), Some(Type::I64))),
            Expr::UInt64(v) => Ok(self.emit(InstKind::Const(Const::U64(v)), Some(Type::U64))),
            Expr::Float64(v) => Ok(self.emit(InstKind::Const(Const::F64(v)), Some(Type::F64))),
            Expr::Number(_) => Err(
                "compiler MVP requires explicit numeric type annotations or suffixes".to_string(),
            ),
            Expr::True => Ok(self.emit(InstKind::Const(Const::Bool(true)), Some(Type::Bool))),
            Expr::False => Ok(self.emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))),
            Expr::String(sym) => {
                // String literals in value position emit `ConstStr`,
                // which materialises a pointer-sized handle to the
                // shared `.rodata` blob (the same one `PrintStr` uses
                // for `print("literal")`).
                let bytes_len = self
                    .interner
                    .resolve(sym)
                    .map(|s| s.as_bytes().len() as u64)
                    .unwrap_or(0);
                Ok(self.emit(
                    InstKind::ConstStr { message: sym, bytes_len },
                    Some(Type::Str),
                ))
            }
            Expr::Identifier(sym) => {
                match self.bindings.get(&sym).cloned() {
                    Some(Binding::Scalar { local, ty }) => {
                        self.pending_struct_value = None;
                        Ok(self.emit(InstKind::LoadLocal(local), Some(ty)))
                    }
                    Some(Binding::Struct { fields, .. }) => {
                        // Tail-position use: stash the struct's field
                        // list so `emit_implicit_return` can return it.
                        // Non-tail uses (e.g. `5 + p`) will fail at
                        // arithmetic lowering when no scalar value
                        // materialises.
                        self.pending_struct_value = Some(fields);
                        Ok(None)
                    }
                    Some(Binding::Tuple { elements }) => {
                        // Tail-position use: stash the elements list
                        // so `emit_implicit_return` can pull element
                        // values out for a tuple-returning function.
                        // Non-tail uses fall through to errors when a
                        // scalar value is later required.
                        self.pending_tuple_value = Some(elements);
                        Ok(None)
                    }
                    Some(Binding::Enum(storage)) => {
                        // Tail-position use: stash the enum storage
                        // so `emit_implicit_return` can flatten it
                        // into a multi-value Return for an enum-
                        // returning function. Other uses (passing to
                        // a function, explicit Return) handle the
                        // binding via a direct lookup, so the
                        // channel is purely for the tail-implicit-
                        // return path.
                        self.pending_enum_value = Some(storage);
                        Ok(None)
                    }
                    Some(Binding::Array { .. }) => {
                        // Bare-identifier use of an array binding is
                        // not supported in expression position yet —
                        // arrays don't flow through the IR's value
                        // graph. The user must access an element.
                        Err(format!(
                            "compiler MVP cannot use array `{}` as a value; access an element with `{}[i]`",
                            self.interner.resolve(sym).unwrap_or("?"),
                            self.interner.resolve(sym).unwrap_or("?"),
                        ))
                    }
                    None => {
                        // Fall back to top-level `const` lookup. This
                        // mirrors what the type-checker does: a name
                        // that wasn't introduced by a local binding
                        // can still resolve to a global const value.
                        if let Some(c) = self.const_values.get(&sym).copied() {
                            self.pending_struct_value = None;
                            let ty = c.ty();
                            return Ok(self.emit(InstKind::Const(c), Some(ty)));
                        }
                        Err(format!(
                            "undefined identifier `{}`",
                            self.interner.resolve(sym).unwrap_or("?")
                        ))
                    }
                }
            }
            Expr::FieldAccess(obj, field) => {
                self.pending_struct_value = None;
                self.lower_field_access(&obj, field)
            }
            Expr::TupleAccess(tuple, index) => {
                self.pending_struct_value = None;
                self.lower_tuple_access(&tuple, index)
            }
            Expr::TupleLiteral(elems) => {
                // Tail-position tuple literal — materialise each
                // element into a fresh local and stash the resulting
                // element list as the pending tuple value. The IR
                // never sees a tuple value flow through SSA — the
                // implicit-return path consumes the element locals
                // directly. Non-tail uses (e.g. arithmetic on the
                // result) hit the value-required check downstream.
                self.lower_tuple_literal_tail(elems)
            }
            Expr::StructLiteral(struct_name, fields) => {
                // Tail-position struct literal: materialise each field
                // into a fresh local and stash the resulting field
                // binding list as the pending struct value. The IR
                // never sees a struct value flow through SSA — the
                // implicit-return path consumes the field locals
                // directly.
                self.lower_struct_literal_tail(struct_name, fields)
            }
            Expr::Binary(op, lhs, rhs) => self.lower_binary(&op, &lhs, &rhs),
            Expr::Unary(op, operand) => self.lower_unary(&op, &operand),
            Expr::Assign(lhs, rhs) => self.lower_assign(&lhs, &rhs),
            Expr::IfElifElse(cond, then_blk, elif_pairs, else_blk) => {
                self.lower_if_chain(&cond, &then_blk, &elif_pairs, &else_blk)
            }
            Expr::Call(fn_name, args_ref) => self.lower_call(fn_name, &args_ref),
            Expr::AssociatedFunctionCall(struct_name, fn_name, args) => {
                // Module-qualified call (`math::add(args)`): when the
                // qualifier doesn't refer to a struct / enum and the
                // function exists in the (post-import) main function
                // table, treat it as a plain `Call`. Module
                // integration flattens imported `pub fn`s in, so the
                // bare lookup hits without needing the qualifier.
                // Real associated calls (`Container::new(...)`) keep
                // the unsupported reject path below.
                let is_struct = self.struct_defs.contains_key(&struct_name)
                    || self.enum_defs.contains_key(&struct_name);
                // Qualified call (`math::add(args)`): try
                // `(Some(struct_name), fn_name)` first so cross-
                // module collisions resolve unambiguously. Fall back
                // to the bare lookup so legacy code paths that
                // pre-date the per-module key still work — e.g.
                // an integration that didn't record a module path
                // (None entry) but flattened the function into the
                // table by name.
                let target_opt = if !is_struct {
                    self.module
                        .lookup_function(Some(struct_name), fn_name)
                        .or_else(|| self.module.lookup_function(None, fn_name))
                } else {
                    None
                };
                if let Some(target) = target_opt {
                    let ret_ty = self.module.function(target).return_type;
                    if matches!(ret_ty, Type::Struct(_) | Type::Tuple(_) | Type::Enum(_)) {
                        return Err(format!(
                            "compiler MVP cannot use a compound-returning module call (`{}::{}`) in expression position; bind the result with `val`",
                            self.interner.resolve(struct_name).unwrap_or("?"),
                            self.interner.resolve(fn_name).unwrap_or("?"),
                        ));
                    }
                    let mut arg_values: Vec<ValueId> = Vec::with_capacity(args.len());
                    for a in &args {
                        let v = self
                            .lower_expr(a)?
                            .ok_or_else(|| {
                                "module function arg produced no value".to_string()
                            })?;
                        arg_values.push(v);
                    }
                    let result_ty = if ret_ty.produces_value() {
                        Some(ret_ty)
                    } else {
                        None
                    };
                    return Ok(self.emit(
                        InstKind::Call { target, args: arg_values },
                        result_ty,
                    ));
                }
                Err(format!(
                    "compiler MVP cannot lower expression yet: {:?}",
                    Expr::AssociatedFunctionCall(struct_name, fn_name, args)
                ))
            }
            Expr::BuiltinCall(func, args) => self.lower_builtin_call(&func, &args),
            Expr::Cast(inner, target_ty) => self.lower_cast(&inner, &target_ty),
            Expr::Match(scrutinee, arms) => self.lower_match(&scrutinee, &arms),
            Expr::MethodCall(obj, method, args) => self.lower_method_call(&obj, method, &args),
            Expr::BuiltinMethodCall(_receiver, method, _args) => {
                // NOTE: `BuiltinMethod::{I64Abs, F64Abs, F64Sqrt}`
                // arms used to live here, lowering directly to
                // `UnaryOp::{Abs, Sqrt}` cranelift instructions.
                // Step F removed them — `x.abs()` / `x.sqrt()` now
                // resolve through the prelude's extension-trait
                // impls and reach `lower_method_call`'s
                // primitive-receiver path (Step D), which emits a
                // regular call into the prelude's wrapper body that
                // forwards to `__extern_*` (resolved by
                // `libm_import_name_for` to the matching libm
                // symbol). String / `is_null` methods stay
                // interpreter-only as before.
                Err(format!(
                    "compiler MVP cannot lower builtin method yet: {:?}",
                    method
                ))
            }
            Expr::SliceAccess(obj, info) => self.lower_slice_access(&obj, &info),
            Expr::SliceAssign(obj, start, end, value) => {
                self.lower_slice_assign(&obj, start.as_ref(), end.as_ref(), &value)
            }
            // NUM-W-AOT (T5 follow-up to Phase 5): narrow integer
            // literals lower to the matching `Const::*` IR
            // instruction. The cranelift codegen consumes that
            // and emits an `iconst` with the right cranelift
            // integer type (I8 / I16 / I32). All arithmetic /
            // comparison / cast paths from there reuse the
            // existing wide-int code paths since cranelift's
            // `iadd` etc. pick up width from the operand types.
            Expr::Int8(n) => Ok(self.emit(InstKind::Const(Const::I8(n)), Some(Type::I8))),
            Expr::UInt8(n) => Ok(self.emit(InstKind::Const(Const::U8(n)), Some(Type::U8))),
            Expr::Int16(n) => Ok(self.emit(InstKind::Const(Const::I16(n)), Some(Type::I16))),
            Expr::UInt16(n) => Ok(self.emit(InstKind::Const(Const::U16(n)), Some(Type::U16))),
            Expr::Int32(n) => Ok(self.emit(InstKind::Const(Const::I32(n)), Some(Type::I32))),
            Expr::UInt32(n) => Ok(self.emit(InstKind::Const(Const::U32(n)), Some(Type::U32))),
            // #121 Phase B-min: `with allocator = expr { body }` —
            // push the allocator handle on entry, lower the body,
            // pop on exit. The body can produce a value (the
            // surrounding expression position) so we hand back
            // whatever the body yielded. Only the linear exit path
            // is supported in Phase B-min — early `return` /
            // `break` / `continue` from inside a `with` body
            // wouldn't pop the stack and would corrupt nesting; a
            // future enhancement (interpreter / JIT already
            // handle this) will install a panic-style cleanup hook.
            Expr::With(allocator_expr, body_expr) => {
                let handle = self
                    .lower_expr(&allocator_expr)?
                    .ok_or_else(|| "with-allocator handle expression produced no value".to_string())?;
                self.emit(InstKind::AllocPush { handle }, None);
                let body_value = self.lower_expr(&body_expr)?;
                self.emit(InstKind::AllocPop, None);
                Ok(body_value)
            }
            other => Err(format!(
                "compiler MVP cannot lower expression yet: {:?}",
                other
            )),
        }
    }

    /// Lower the user-facing builtins this MVP supports. Today that's
    /// just `panic("literal")` and `assert(cond, "literal")`. Both are
    /// restricted to a string-literal message because the codegen lays
    /// the message bytes into a static data segment; non-literal
    /// messages would require formatting at runtime.
    pub(super) fn lower_builtin_call(
        &mut self,
        func: &BuiltinFunction,
        args: &Vec<ExprRef>,
    ) -> Result<Option<ValueId>, String> {
        match func {
            BuiltinFunction::Panic => {
                if args.len() != 1 {
                    return Err(format!("panic expects 1 argument, got {}", args.len()));
                }
                let msg_sym = self.expect_string_literal(&args[0], "panic")?;
                self.terminate(Terminator::Panic { message: msg_sym });
                Ok(None)
            }
            BuiltinFunction::Assert => {
                if args.len() != 2 {
                    return Err(format!("assert expects 2 arguments, got {}", args.len()));
                }
                let msg_sym = self.expect_string_literal(&args[1], "assert")?;
                let cond = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "assert condition produced no value".to_string())?;
                let pass = self.fresh_block();
                let fail = self.fresh_block();
                self.terminate(Terminator::Branch {
                    cond,
                    then_blk: pass,
                    else_blk: fail,
                });
                // Failure block: panic with the assertion message.
                self.switch_to(fail);
                self.terminate(Terminator::Panic { message: msg_sym });
                // Continue lowering after the assert in the success block.
                self.switch_to(pass);
                Ok(None)
            }
            BuiltinFunction::Print => self.lower_print(args, false),
            BuiltinFunction::Println => self.lower_print(args, true),
            BuiltinFunction::Abs => {
                if args.len() != 1 {
                    return Err(format!("abs expects 1 argument, got {}", args.len()));
                }
                let operand = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "abs operand produced no value".to_string())?;
                // Result type matches the operand: i64 -> i64,
                // f64 -> f64. Codegen branches on the operand IR
                // type to pick `fabs` vs the integer select chain.
                let result_ty = self
                    .value_ir_type_for(operand)
                    .filter(|t| matches!(t, Type::I64 | Type::F64))
                    .ok_or_else(|| {
                        "abs expects an i64 or f64 operand".to_string()
                    })?;
                Ok(self.emit(
                    InstKind::UnaryOp { op: crate::ir::UnaryOp::Abs, operand },
                    Some(result_ty),
                ))
            }
            // NOTE: f64 math arms (Sqrt/Pow and Sin..=Ceil) lived
            // here before Phase 4. Each is now declared as
            // `extern fn __extern_*_f64` in math.t and lowered
            // through `lower/program::libm_import_name_for` —
            // the call site emits a regular cranelift call against
            // the imported libm symbol.
            BuiltinFunction::Min | BuiltinFunction::Max => {
                if args.len() != 2 {
                    let name = if matches!(func, BuiltinFunction::Min) { "min" } else { "max" };
                    return Err(format!("{name} expects 2 arguments, got {}", args.len()));
                }
                let lhs = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "min/max arg0 produced no value".to_string())?;
                let rhs = self
                    .lower_expr(&args[1])?
                    .ok_or_else(|| "min/max arg1 produced no value".to_string())?;
                let result_ty = self
                    .value_ir_type_for(lhs)
                    .ok_or_else(|| "min/max operand type unknown".to_string())?;
                let op = if matches!(func, BuiltinFunction::Min) {
                    crate::ir::BinOp::Min
                } else {
                    crate::ir::BinOp::Max
                };
                Ok(self.emit(
                    InstKind::BinOp { op, lhs, rhs },
                    Some(result_ty),
                ))
            }
            BuiltinFunction::HeapAlloc => {
                // #121 Phase A: lower to InstKind::HeapAlloc which
                // codegen turns into a libc malloc call. Default
                // global allocator only — `with allocator = ...`
                // scope plumbing comes in a later phase.
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_heap_alloc takes 1 arg (size), got {}",
                        args.len()
                    ));
                }
                let size = self.lower_expr(&args[0])?
                    .ok_or_else(|| "heap_alloc size produced no value".to_string())?;
                Ok(self.emit(InstKind::HeapAlloc { size }, Some(Type::U64)))
            }
            BuiltinFunction::HeapRealloc => {
                if args.len() != 2 {
                    return Err(format!(
                        "__builtin_heap_realloc takes 2 args (ptr, new_size), got {}",
                        args.len()
                    ));
                }
                let ptr = self.lower_expr(&args[0])?
                    .ok_or_else(|| "heap_realloc ptr produced no value".to_string())?;
                let new_size = self.lower_expr(&args[1])?
                    .ok_or_else(|| "heap_realloc new_size produced no value".to_string())?;
                Ok(self.emit(InstKind::HeapRealloc { ptr, new_size }, Some(Type::U64)))
            }
            BuiltinFunction::HeapFree => {
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_heap_free takes 1 arg (ptr), got {}",
                        args.len()
                    ));
                }
                let ptr = self.lower_expr(&args[0])?
                    .ok_or_else(|| "heap_free ptr produced no value".to_string())?;
                Ok(self.emit(InstKind::HeapFree { ptr }, None))
            }
            BuiltinFunction::PtrWrite => {
                // `__builtin_ptr_write(ptr, offset, value)` — the
                // value's IR type is captured here so codegen can
                // pick the matching `store.<cl_ty>`. The interpreter
                // routes through a typed_slots map; the AOT path
                // takes a direct address and trusts the type-checker
                // to keep reads/writes type-consistent at the same
                // offset (`Dict<K, V>` always reads/writes K-typed
                // values to `keys` and V-typed values to `vals`, so
                // there's no tag-mismatch worry in well-typed code).
                if args.len() != 3 {
                    return Err(format!(
                        "__builtin_ptr_write takes 3 args (ptr, offset, value), got {}",
                        args.len()
                    ));
                }
                let ptr = self.lower_expr(&args[0])?
                    .ok_or_else(|| "ptr_write ptr produced no value".to_string())?;
                let offset = self.lower_expr(&args[1])?
                    .ok_or_else(|| "ptr_write offset produced no value".to_string())?;
                // Peek the value's IR type before lowering so the
                // store width is preserved; lower the value as usual.
                let value_ty = self
                    .value_scalar(&args[2])
                    .ok_or_else(|| {
                        "__builtin_ptr_write value type unsupported (needs scalar)"
                            .to_string()
                    })?;
                let value = self.lower_expr(&args[2])?
                    .ok_or_else(|| "ptr_write value produced no value".to_string())?;
                Ok(self.emit(
                    InstKind::PtrWrite { ptr, offset, value, value_ty },
                    None,
                ))
            }
            BuiltinFunction::SizeOf => {
                // `__builtin_sizeof(value) -> u64` — at AOT we
                // resolve the byte size at lower time from the
                // value's IR type via `value_scalar`. The active
                // monomorph subst already shows on the value's
                // type because parameter / let bindings store the
                // substituted type. Compound types (struct /
                // tuple / enum) aren't reached today by the
                // user-space collections that drive this; reject
                // them with a precise message rather than
                // silently summing fields.
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_sizeof takes 1 arg, got {}",
                        args.len()
                    ));
                }
                let arg_ty = self
                    .value_scalar(&args[0])
                    .ok_or_else(|| {
                        "__builtin_sizeof: could not infer arg type at AOT".to_string()
                    })?;
                let size = match arg_ty {
                    Type::I8 | Type::U8 | Type::Bool => 1u64,
                    Type::I16 | Type::U16 => 2,
                    Type::I32 | Type::U32 => 4,
                    Type::I64 | Type::U64 | Type::F64 | Type::Str => 8,
                    Type::Struct(_) | Type::Tuple(_) | Type::Enum(_) | Type::Unit => {
                        return Err(format!(
                            "compiler MVP cannot lower __builtin_sizeof of compound type \
                             {arg_ty:?} yet"
                        ));
                    }
                };
                Ok(self.emit(InstKind::Const(crate::ir::Const::U64(size)), Some(Type::U64)))
            }
            BuiltinFunction::MemCopy => {
                // `__builtin_mem_copy(src: ptr, dest: ptr, size: u64)`
                // — emit `InstKind::MemCopy` which codegen lowers
                // to a libc memcpy call (with (dest, src, n)
                // argument-order swap).
                if args.len() != 3 {
                    return Err(format!(
                        "__builtin_mem_copy takes 3 args (src, dest, size), got {}",
                        args.len()
                    ));
                }
                let src = self.lower_expr(&args[0])?
                    .ok_or_else(|| "mem_copy src produced no value".to_string())?;
                let dest = self.lower_expr(&args[1])?
                    .ok_or_else(|| "mem_copy dest produced no value".to_string())?;
                let size = self.lower_expr(&args[2])?
                    .ok_or_else(|| "mem_copy size produced no value".to_string())?;
                Ok(self.emit(InstKind::MemCopy { src, dest, size }, None))
            }
            BuiltinFunction::StrLen => {
                // `__builtin_str_len(s: str) -> u64` — emits an
                // `InstKind::StrLen` that codegen lowers to a libc
                // `strlen` call. The per-literal `.rodata` layout
                // (`[bytes][NUL][u64 len]`) keeps the trailing NUL
                // so strlen's walk terminates correctly; the stored
                // u64 len at the layout's tail is informational
                // for now.
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_str_len takes 1 arg (str), got {}",
                        args.len()
                    ));
                }
                let v = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "str_len arg produced no value".to_string())?;
                Ok(self.emit(InstKind::StrLen { value: v }, Some(Type::U64)))
            }
            BuiltinFunction::StrToPtr => {
                // `__builtin_str_to_ptr(s: str) -> ptr`. AOT
                // representation: `Type::Str` is already a pointer-
                // sized handle (i64) into the `.rodata` blob (or a
                // heap-allocated copy). Returning the same value
                // with a `Type::U64` annotation is identity at
                // cranelift level (`ir_to_cranelift_ty(Str)` = I64
                // = `ir_to_cranelift_ty(U64)`); the user's `ptr`
                // can then be fed into `__builtin_ptr_read(p, i)`
                // with a `val: u8` annotation to walk the bytes.
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_str_to_ptr takes 1 arg (str), got {}",
                        args.len()
                    ));
                }
                let v = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "str_to_ptr arg produced no value".to_string())?;
                // The str runtime value points at the u64 len field
                // (see ConstStr codegen). Layout `[bytes][NUL][u64
                // len LE]`: byte_start = len_field_addr - 1 (NUL)
                // - len. Compute as a single chain of
                // `load.i64(s, 0)` + `iadd_imm(-1)` + `isub`.
                let len = self
                    .emit(InstKind::StrLen { value: v }, Some(Type::U64))
                    .expect("StrLen returns a value");
                let one = self
                    .emit(
                        InstKind::Const(crate::ir::Const::U64(1)),
                        Some(Type::U64),
                    )
                    .expect("Const returns a value");
                let nul_offset = self
                    .emit(
                        InstKind::BinOp {
                            op: crate::ir::BinOp::Add,
                            lhs: len,
                            rhs: one,
                        },
                        Some(Type::U64),
                    )
                    .expect("Add returns a value");
                Ok(self.emit(
                    InstKind::BinOp {
                        op: crate::ir::BinOp::Sub,
                        lhs: v,
                        rhs: nul_offset,
                    },
                    Some(Type::U64),
                ))
            }
            BuiltinFunction::PtrRead => {
                // `__builtin_ptr_read(ptr, offset)` — return type comes
                // from the surrounding `val`/`var` annotation. The
                // generic version (no annotation) is rejected here so
                // the user gets a clear error pointing at the missing
                // type hint. The let-binding lowering path
                // (`let_lowering.rs::lower_let`) handles
                // `val x: T = __builtin_ptr_read(...)` directly and
                // never reaches this arm.
                Err(
                    "compiler MVP requires `val NAME: TYPE = __builtin_ptr_read(...)` \
                     (the read width is taken from the annotation; bare expression-position \
                     uses are not supported in AOT yet)"
                        .to_string(),
                )
            }
            BuiltinFunction::DefaultAllocator => {
                // #121 Phase B-min: the default global allocator is
                // represented as the sentinel u64 = 0. The heap path
                // already routes 0-handles to libc malloc.
                if !args.is_empty() {
                    return Err(format!(
                        "__builtin_default_allocator takes no args, got {}",
                        args.len()
                    ));
                }
                Ok(self.emit(InstKind::Const(crate::ir::Const::U64(0)), Some(Type::U64)))
            }
            BuiltinFunction::CurrentAllocator => {
                // #121 Phase B-min: read the top of the runtime
                // active-allocator stack (or 0 when empty).
                if !args.is_empty() {
                    return Err(format!(
                        "__builtin_current_allocator takes no args, got {}",
                        args.len()
                    ));
                }
                Ok(self.emit(InstKind::AllocCurrent, Some(Type::U64)))
            }
            BuiltinFunction::PtrIsNull => {
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_ptr_is_null takes 1 arg (ptr), got {}",
                        args.len()
                    ));
                }
                let p = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "ptr_is_null arg produced no value".to_string())?;
                Ok(self.emit(InstKind::PtrIsNull { ptr: p }, Some(Type::Bool)))
            }
            BuiltinFunction::ArenaAllocator => {
                // #121 Phase B-rest Item 1: allocate an arena slot
                // in the runtime registry and return its handle.
                // The handle is a non-zero u64 so heap_alloc /
                // realloc / free can dispatch on it.
                if !args.is_empty() {
                    return Err(format!(
                        "__builtin_arena_allocator takes no args, got {}",
                        args.len()
                    ));
                }
                Ok(self.emit(InstKind::AllocArena, Some(Type::U64)))
            }
            BuiltinFunction::FixedBufferAllocator => {
                // #121 Phase B-rest Item 1: capacity-limited allocator.
                // Subsequent allocations through this handle that
                // would exceed `capacity` return 0 (null).
                if args.len() != 1 {
                    return Err(format!(
                        "__builtin_fixed_buffer_allocator takes 1 arg (capacity), got {}",
                        args.len()
                    ));
                }
                let cap = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "fixed_buffer capacity arg produced no value".to_string())?;
                Ok(self.emit(
                    InstKind::AllocFixedBuffer { capacity: cap },
                    Some(Type::U64),
                ))
            }
            other => Err(format!(
                "compiler MVP cannot lower builtin yet: {:?}",
                other
            )),
        }
    }

}
