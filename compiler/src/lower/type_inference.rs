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
            Expr::Float64(_) => Some(Type::F64),
            Expr::String(_) => Some(Type::Str),
            Expr::True | Expr::False => Some(Type::Bool),
            Expr::Cast(_, target_ty) => lower_scalar(&target_ty),
            Expr::Identifier(sym) => match self.bindings.get(&sym) {
                Some(Binding::Scalar { ty, .. }) => Some(*ty),
                Some(_) => None,
                None => self.const_values.get(&sym).map(|c| c.ty()),
            },
            Expr::FieldAccess(obj, field) => {
                let inner = self.resolve_field_chain(&obj).ok()?;
                let fields = match inner {
                    FieldChainResult::Struct { fields } => fields,
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
                .function_index
                .get(&fn_name)
                .map(|id| self.module.function(*id).return_type),
            Expr::AssociatedFunctionCall(_struct_name, fn_name, _) => {
                // Module-qualified call: same lookup path as Call
                // because module integration flattens imported `pub fn`s
                // into the main function table. Real associated method
                // calls aren't supported in expression position so the
                // None return at the bottom is the correct fallback.
                self.module
                    .function_index
                    .get(&fn_name)
                    .map(|id| self.module.function(*id).return_type)
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
                // f64 math (sqrt/pow/sin/cos/tan/log/log2/exp
                // /floor/ceil) used to be `BuiltinFunction` arms.
                // Phase 4 moved them onto `extern fn`, so type
                // inference for those calls flows through the
                // regular `Expr::Call` path instead.
                _ => None,
            },
            Expr::BuiltinMethodCall(_receiver, method, _args) => match method {
                frontend::ast::BuiltinMethod::I64Abs => Some(Type::I64),
                frontend::ast::BuiltinMethod::F64Abs
                | frontend::ast::BuiltinMethod::F64Sqrt => Some(Type::F64),
                _ => None,
            },
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
                let (target_sym, recv_struct_id) = match self.bindings.get(&recv_sym)? {
                    Binding::Struct { struct_id, .. } => (
                        self.module.struct_def(*struct_id).base_name,
                        Some(*struct_id),
                    ),
                    Binding::Enum(storage) => (
                        self.module.enum_def(storage.enum_id).base_name,
                        None,
                    ),
                    // Step D: extension-trait dispatch — primitive
                    // receiver. Map the binding's IR type back to the
                    // canonical-name symbol; the rest of the lookup
                    // falls through into the same `method_func_ids`
                    // branch struct receivers use.
                    Binding::Scalar { ty, .. } => {
                        let name = match ty {
                            Type::Bool => "bool",
                            Type::I64 => "i64",
                            Type::U64 => "u64",
                            Type::F64 => "f64",
                            _ => return None,
                        };
                        let sym = self.interner.get(name)?;
                        (sym, None)
                    }
                    _ => return None,
                };
                if let Some(func_id) = self.method_func_ids.get(&(target_sym, method)) {
                    return Some(self.module.function(*func_id).return_type);
                }
                if let (Some(template), Some(struct_id)) =
                    (self.generic_methods.get(&(target_sym, method)), recv_struct_id)
                {
                    let recv_type_args =
                        self.module.struct_def(struct_id).type_args.clone();
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
                            return self.peek_method_return_type(ret, &subst, struct_id);
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
