//! Structural type inference used by the lowering pass.
//!
//! `value_scalar` is a cheap, conservative inference that picks the
//! IR `Type` of an expression *without* materialising any IR
//! instructions. It runs on `&self` and exists primarily so val/var
//! sites can name the right local slot when the user omits the type
//! annotation. The full type-checker has already validated the
//! program; this pass just needs enough local information to
//! disambiguate (e.g.) signed-vs-unsigned division at codegen time.
//!
//! Lives in its own `impl` block on `super::FunctionLower<'a>` —
//! Rust permits multiple `impl`s of the same type across files of
//! the same module tree, which lets us split this big struct's
//! methods by topic without touching its public API.

use std::collections::HashMap;

use frontend::ast::{Expr, ExprRef, Operator, Stmt, UnaryOp};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

use super::bindings::{Binding, FieldChainResult, FieldShape, TupleElementShape};
use super::types::lower_scalar;
use super::FunctionLower;
use crate::ir::{Type, TupleId};

impl<'a> FunctionLower<'a> {
    pub(super) fn value_scalar(&self, expr_ref: &ExprRef) -> Option<Type> {
        let e = self.program.expression.get(expr_ref)?;
        match e {
            Expr::Int64(_) => Some(Type::I64),
            Expr::UInt64(_) => Some(Type::U64),
            // NUM-W-AOT: narrow integer literal types.
            Expr::Int8(_) => Some(Type::I8),
            Expr::UInt8(_) => Some(Type::U8),
            Expr::Int16(_) => Some(Type::I16),
            Expr::UInt16(_) => Some(Type::U16),
            Expr::Int32(_) => Some(Type::I32),
            Expr::UInt32(_) => Some(Type::U32),
            Expr::Float64(_) => Some(Type::F64),
            Expr::String(_) => Some(Type::Str),
            Expr::True | Expr::False => Some(Type::Bool),
            // #121 Phase B-min: a `with allocator = ... { body }`
            // expression takes its value from the body, so peek the
            // body for type inference. This lets `val x = with ... { e }`
            // bind to the right scalar type.
            Expr::With(_, body) => self.value_scalar(&body),
            Expr::Cast(_, target_ty) => lower_scalar(&target_ty),
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Scalar { ty, .. }) => Some(*ty),
                Some(_) => None,
                None => self.const_values.get(&sym).map(|c| c.ty()),
            },
            Expr::FieldAccess(obj, field) => {
                let inner = self.resolve_field_chain(&obj).ok()?;
                let fields = match inner {
                    FieldChainResult::Struct { fields, .. } => fields,
                    FieldChainResult::Scalar { .. }
                    | FieldChainResult::Tuple { .. } => return None,
                };
                let field_str = self.interner.resolve(field)?;
                fields
                    .iter()
                    .find(|f| f.name == field_str)
                    .and_then(|f| match &f.shape {
                        FieldShape::Scalar { ty, .. } => Some(*ty),
                        FieldShape::Struct { .. } | FieldShape::Tuple { .. } => None,
                    })
            }
            Expr::TupleAccess(tuple, index) => {
                let elements = self.resolve_tuple_chain_elements(&tuple).ok()?;
                elements
                    .iter()
                    .find(|e| e.index == index)
                    .and_then(|e| match &e.shape {
                        TupleElementShape::Scalar { ty, .. } => Some(*ty),
                        TupleElementShape::Struct { struct_id, .. } => {
                            Some(Type::Struct(*struct_id))
                        }
                        TupleElementShape::Tuple { tuple_id, .. } => {
                            Some(Type::Tuple(*tuple_id))
                        }
                    })
            }
            Expr::TupleLiteral(elems) => {
                // We can't intern a fresh tuple shape here (this method
                // is `&self`), so fall back to looking up the existing
                // shape if it's already in the IR module's table.
                let mut element_tys: Vec<Type> = Vec::with_capacity(elems.len());
                for e in &elems {
                    element_tys.push(self.value_scalar(e)?);
                }
                self.module
                    .tuple_defs
                    .iter()
                    .position(|t| *t == element_tys)
                    .map(|i| Type::Tuple(TupleId(i as u32)))
            }
            Expr::Binary(op, lhs, _rhs) => match op {
                Operator::EQ
                | Operator::NE
                | Operator::LT
                | Operator::LE
                | Operator::GT
                | Operator::GE
                | Operator::LogicalAnd
                | Operator::LogicalOr => Some(Type::Bool),
                _ => self.value_scalar(&lhs),
            },
            Expr::Unary(op, operand) => match op {
                UnaryOp::LogicalNot => Some(Type::Bool),
                _ => self.value_scalar(&operand),
            },
            Expr::Block(stmts) => {
                if let Some(last) = stmts.last() {
                    if let Some(Stmt::Expression(e)) = self.program.statement.get(last) {
                        return self.value_scalar(&e);
                    }
                }
                None
            }
            Expr::IfElifElse(_, then_body, _, _) => self.value_scalar(&then_body),
            Expr::Match(_, arms) => arms.iter().find_map(|a| self.value_scalar(&a.body)),
            Expr::Call(fn_name, _) => self
                .module
                .lookup_function(None, fn_name)
                .map(|id| self.module.function(id).return_type),
            Expr::AssociatedFunctionCall(struct_name, fn_name, _) => {
                // Module-qualified call: prefer
                // `(Some(struct_name), fn_name)` so cross-module
                // collisions resolve unambiguously, then fall back to
                // the bare lookup. Real associated method calls
                // aren't supported in expression position so the
                // None return at the bottom is the correct fallback.
                self.module
                    .lookup_function(Some(struct_name), fn_name)
                    .or_else(|| self.module.lookup_function(None, fn_name))
                    .map(|id| self.module.function(id).return_type)
            }
            Expr::BuiltinCall(func, args) => match func {
                frontend::ast::BuiltinFunction::Abs => {
                    // Polymorphic: forwards the operand's type.
                    args.first().and_then(|a| self.value_scalar(a))
                }
                frontend::ast::BuiltinFunction::Min
                | frontend::ast::BuiltinFunction::Max => {
                    args.first().and_then(|a| self.value_scalar(a))
                }
                // #121 Phase A: heap_alloc / heap_realloc return a
                // pointer-sized value. Pointer is U64 in the IR
                // (matches the `ptr` keyword's lowering).
                frontend::ast::BuiltinFunction::HeapAlloc
                | frontend::ast::BuiltinFunction::HeapRealloc => Some(Type::U64),
                // DICT-AOT-NEW Phase C: __builtin_sizeof returns u64.
                frontend::ast::BuiltinFunction::SizeOf => Some(Type::U64),
                // #121 Phase B-min: allocator handles are u64
                // sentinel values.
                frontend::ast::BuiltinFunction::DefaultAllocator
                | frontend::ast::BuiltinFunction::CurrentAllocator
                | frontend::ast::BuiltinFunction::ArenaAllocator
                | frontend::ast::BuiltinFunction::FixedBufferAllocator => Some(Type::U64),
                frontend::ast::BuiltinFunction::PtrIsNull => Some(Type::Bool),
                // `__builtin_str_to_ptr(s) -> ptr` returns a u64-sized
                // pointer value.
                frontend::ast::BuiltinFunction::StrToPtr => Some(Type::U64),
                // `__builtin_str_len(s) -> u64`.
                frontend::ast::BuiltinFunction::StrLen => Some(Type::U64),
                // SizeOf handled above already; no other builtins
                // currently route through value_scalar.
                // f64 math (sqrt/pow/sin/cos/tan/log/log2/exp
                // /floor/ceil) used to be `BuiltinFunction` arms.
                // Phase 4 moved them onto `extern fn`, so type
                // inference for those calls flows through the
                // regular `Expr::Call` path instead.
                _ => None,
            },
            Expr::BuiltinMethodCall(_receiver, _method, _args) => {
                // NOTE: `BuiltinMethod::{I64Abs, F64Abs, F64Sqrt}`
                // arms used to live here. Step F removed them;
                // numeric value-method type inference now flows
                // through the regular `MethodCall` arm against the
                // prelude's extension-trait impls.
                None
            }
            Expr::SliceAccess(obj, info) => {
                if !matches!(info.slice_type, frontend::ast::SliceType::SingleElement) {
                    return None;
                }
                let obj_expr = self.program.expression.get(&obj)?;
                let arr_sym = match obj_expr {
                    Expr::Identifier(s) => s,
                    _ => return None,
                };
                match self.bindings.get(&arr_sym)? {
                    Binding::Array { element_ty, .. } => Some(*element_ty),
                    _ => None,
                }
            }
            Expr::MethodCall(obj, method, args) => {
                // Numeric method calls (`x.abs()` for i64,
                // `x.sqrt()` for f64) reach the AST as `MethodCall`.
                // Peek through them so cast / let inference works on
                // call sites like `x.abs() as u64` without needing an
                // intermediate `val: i64` annotation.
                if args.is_empty() {
                    if let Some(name) = self.interner.resolve(method) {
                        if let Some(recv_ty) = self.value_scalar(&obj) {
                            match (name, recv_ty) {
                                ("abs", Type::I64) => return Some(Type::I64),
                                ("abs", Type::F64) => return Some(Type::F64),
                                ("sqrt", Type::F64) => return Some(Type::F64),
                                _ => {}
                            }
                        }
                    }
                }
                let obj_expr = self.program.expression.get(&obj)?;
                let recv_sym = match obj_expr {
                    Expr::Identifier(s) => s,
                    _ => return None,
                };
                // Track receiver self-type and per-receiver type
                // args separately so the generic-method peek path
                // below can handle struct AND enum receivers
                // uniformly.
                let (target_sym, recv_self): (DefaultSymbol, Option<(Type, Vec<Type>)>) =
                    match self.bindings.get(&recv_sym)? {
                        Binding::Struct { struct_id, .. } => {
                            let def = self.module.struct_def(*struct_id);
                            (
                                def.base_name,
                                Some((Type::Struct(*struct_id), def.type_args.clone())),
                            )
                        }
                        Binding::Enum(storage) => {
                            let def = self.module.enum_def(storage.enum_id);
                            (
                                def.base_name,
                                Some((Type::Enum(storage.enum_id), def.type_args.clone())),
                            )
                        }
                        // Step D: extension-trait dispatch — primitive
                        // receiver. Map the binding's IR type back to
                        // the canonical-name symbol; the rest of the
                        // lookup falls through into the same
                        // `method_func_ids` branch struct receivers use.
                        Binding::Scalar { ty, .. } => {
                            let name = match ty {
                                Type::Bool => "bool",
                                Type::I64 => "i64",
                                Type::U64 => "u64",
                                Type::F64 => "f64",
                                // `core/std/str.t::AsPtr` and
                                // `core/std/hash.t::Hash for str`
                                // dispatch through this path.
                                Type::Str => "str",
                                // Narrow int extension traits live
                                // in `core/std/hash.t` (`Hash for
                                // u8 / u16 / u32 / i8 / i16 / i32`).
                                Type::U8 => "u8",
                                Type::U16 => "u16",
                                Type::U32 => "u32",
                                Type::I8 => "i8",
                                Type::I16 => "i16",
                                Type::I32 => "i32",
                                _ => return None,
                            };
                            let sym = self.interner.get(name)?;
                            (sym, None)
                        }
                        _ => return None,
                    };
                // CONCRETE-IMPL Phase 2b: pick FuncId by receiver
                // type args (extracted above as part of recv_self).
                let recv_args_for_lookup: Vec<Type> = recv_self
                    .as_ref()
                    .map(|(_, args)| args.clone())
                    .unwrap_or_default();
                if let Some(func_id) = super::method_registry::lookup_method_func(
                    self.method_func_ids, target_sym, method, &recv_args_for_lookup,
                ) {
                    return Some(self.module.function(func_id).return_type);
                }
                let template_opt = super::method_registry::lookup_method_template(
                    self.generic_methods, target_sym, method, &[],
                );
                if let (Some(template), Some((self_ty, recv_type_args))) =
                    (template_opt, recv_self)
                {
                    if template.generic_params.len() >= recv_type_args.len() {
                        let mut subst: HashMap<DefaultSymbol, Type> = HashMap::new();
                        for (i, p) in template.generic_params.iter().enumerate() {
                            if let Some(t) = recv_type_args.get(i).copied() {
                                subst.insert(*p, t);
                            }
                        }
                        let method_only_params: Vec<DefaultSymbol> = template
                            .generic_params
                            .iter()
                            .skip(recv_type_args.len())
                            .copied()
                            .collect();
                        if !method_only_params.is_empty() {
                            for (i, arg_ref) in args.iter().enumerate() {
                                let param_idx = i + 1;
                                if let Some((_, decl)) = template.parameter.get(param_idx) {
                                    if let Some(arg_ty) = self.value_scalar(arg_ref) {
                                        if let TypeDecl::Generic(p) | TypeDecl::Identifier(p) =
                                            decl
                                        {
                                            if method_only_params.contains(p) {
                                                subst.entry(*p).or_insert(arg_ty);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(ret) = &template.return_type {
                            return self.peek_method_return_type_with_self(
                                ret, &subst, self_ty,
                            );
                        }
                        return Some(Type::Unit);
                    }
                }
                None
            }
            _ => None,
        }
    }
}
