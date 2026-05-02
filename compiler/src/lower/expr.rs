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
                Ok(self.emit(InstKind::ConstStr { message: sym }, Some(Type::Str)))
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
                if !is_struct
                    && self.module.function_index.contains_key(&fn_name)
                {
                    let target = *self
                        .module
                        .function_index
                        .get(&fn_name)
                        .expect("function_index hit just confirmed");
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
            Expr::BuiltinMethodCall(receiver, method, args) => {
                // `x.abs()` / `x.sqrt()` style method calls. Both
                // forward to the matching `__builtin_*` intrinsic so
                // the lowering reuses the same `UnaryOp` variants the
                // bare-call path already supports. String / pointer
                // / `is_null` methods aren't lowered yet — they
                // remain interpreter-only.
                use frontend::ast::BuiltinMethod;
                match method {
                    BuiltinMethod::I64Abs => {
                        if !args.is_empty() {
                            return Err(format!(
                                "i64.abs() takes no arguments, got {}",
                                args.len()
                            ));
                        }
                        let operand = self
                            .lower_expr(&receiver)?
                            .ok_or_else(|| "i64.abs() receiver produced no value".to_string())?;
                        Ok(self.emit(
                            InstKind::UnaryOp { op: crate::ir::UnaryOp::Abs, operand },
                            Some(Type::I64),
                        ))
                    }
                    BuiltinMethod::F64Sqrt => {
                        if !args.is_empty() {
                            return Err(format!(
                                "f64.sqrt() takes no arguments, got {}",
                                args.len()
                            ));
                        }
                        let operand = self
                            .lower_expr(&receiver)?
                            .ok_or_else(|| "f64.sqrt() receiver produced no value".to_string())?;
                        Ok(self.emit(
                            InstKind::UnaryOp { op: crate::ir::UnaryOp::Sqrt, operand },
                            Some(Type::F64),
                        ))
                    }
                    other => Err(format!(
                        "compiler MVP cannot lower builtin method yet: {:?}",
                        other
                    )),
                }
            }
            Expr::SliceAccess(obj, info) => self.lower_slice_access(&obj, &info),
            Expr::SliceAssign(obj, start, end, value) => {
                self.lower_slice_assign(&obj, start.as_ref(), end.as_ref(), &value)
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
                Ok(self.emit(
                    InstKind::UnaryOp { op: crate::ir::UnaryOp::Abs, operand },
                    Some(Type::I64),
                ))
            }
            BuiltinFunction::Sqrt => {
                if args.len() != 1 {
                    return Err(format!("sqrt expects 1 argument, got {}", args.len()));
                }
                let operand = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "sqrt operand produced no value".to_string())?;
                Ok(self.emit(
                    InstKind::UnaryOp { op: crate::ir::UnaryOp::Sqrt, operand },
                    Some(Type::F64),
                ))
            }
            BuiltinFunction::Pow => {
                if args.len() != 2 {
                    return Err(format!("pow expects 2 arguments, got {}", args.len()));
                }
                let lhs = self
                    .lower_expr(&args[0])?
                    .ok_or_else(|| "pow base produced no value".to_string())?;
                let rhs = self
                    .lower_expr(&args[1])?
                    .ok_or_else(|| "pow exponent produced no value".to_string())?;
                Ok(self.emit(
                    InstKind::BinOp { op: crate::ir::BinOp::Pow, lhs, rhs },
                    Some(Type::F64),
                ))
            }
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
            other => Err(format!(
                "compiler MVP cannot lower builtin yet: {:?}",
                other
            )),
        }
    }

}
