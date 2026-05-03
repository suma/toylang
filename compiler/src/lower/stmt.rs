//! Statement lowering dispatcher.
//!
//! `lower_stmt` is the per-statement switch: it pulls the
//! `Stmt` from the AST pool, dispatches on its kind, and
//! delegates to the right sub-module:
//!
//! - `Stmt::Expression` -> `lower_expr` (carries the value
//!   through if any).
//! - `Stmt::Val` / `Stmt::Var` -> `lower_let`.
//! - `Stmt::Assign` -> `lower_assign`.
//! - `Stmt::Return` -> emit a `Terminator::Return` with the
//!   correct number of result values for the function's
//!   return type (scalar / unit / multi-value compound).
//! - `Stmt::While` / `Stmt::For` -> `lower_while` / `lower_for`.
//! - `Stmt::Break` / `Stmt::Continue` -> jump to the active
//!   loop header / exit.
//! - `Stmt::Block` -> recurse into nested statements, returning
//!   the last expression's value if the block ends in one.
//! - Anything else (impl / enum / trait declarations inside a
//!   function body) is rejected with a clear error since the
//!   compiler MVP doesn't support them.

use frontend::ast::{Expr, Stmt, StmtRef};

use super::bindings::{flatten_struct_locals, flatten_tuple_element_locals, Binding};
use super::types::lower_scalar;
use super::FunctionLower;
use crate::ir::{Const, InstKind, Terminator, Type, ValueId};

impl<'a> FunctionLower<'a> {
    // -- statement lowering --------------------------------------------------------

    pub(super) fn lower_stmt(&mut self, stmt_ref: &StmtRef) -> Result<Option<ValueId>, String> {
        let stmt = self
            .program
            .statement
            .get(stmt_ref)
            .ok_or_else(|| "missing stmt".to_string())?;
        if self.is_unreachable() {
            // Code after a terminator is dropped, mirroring how the
            // interpreter and JIT behave.
            return Ok(None);
        }
        match stmt {
            Stmt::Expression(e) => self.lower_expr(&e),
            Stmt::Val(name, ty, e) | Stmt::Var(name, ty, Some(e)) => {
                self.lower_let(name, ty.as_ref(), &e)
            }
            Stmt::Var(name, ty, None) => {
                let scalar = ty
                    .as_ref()
                    .and_then(lower_scalar)
                    .ok_or_else(|| {
                        format!(
                            "var `{}` needs a scalar type annotation",
                            self.interner.resolve(name).unwrap_or("?")
                        )
                    })?;
                let local = self.module.function_mut(self.func_id).add_local(scalar);
                self.bindings
                    .insert(name, Binding::Scalar { local, ty: scalar });
                // Initialise to zero / false so reads before assignment
                // are still well-defined.
                let zero = match scalar {
                    Type::Bool => self
                        .emit(InstKind::Const(Const::Bool(false)), Some(Type::Bool))
                        .unwrap(),
                    Type::I64 => self
                        .emit(InstKind::Const(Const::I64(0)), Some(Type::I64))
                        .unwrap(),
                    Type::U64 => self
                        .emit(InstKind::Const(Const::U64(0)), Some(Type::U64))
                        .unwrap(),
                    // NUM-W-AOT: zero-init for narrow widths.
                    Type::I32 => self
                        .emit(InstKind::Const(Const::I32(0)), Some(Type::I32)).unwrap(),
                    Type::U32 => self
                        .emit(InstKind::Const(Const::U32(0)), Some(Type::U32)).unwrap(),
                    Type::I16 => self
                        .emit(InstKind::Const(Const::I16(0)), Some(Type::I16)).unwrap(),
                    Type::U16 => self
                        .emit(InstKind::Const(Const::U16(0)), Some(Type::U16)).unwrap(),
                    Type::I8 => self
                        .emit(InstKind::Const(Const::I8(0)), Some(Type::I8)).unwrap(),
                    Type::U8 => self
                        .emit(InstKind::Const(Const::U8(0)), Some(Type::U8)).unwrap(),
                    Type::F64 => self
                        .emit(InstKind::Const(Const::F64(0.0)), Some(Type::F64))
                        .unwrap(),
                    Type::Unit => return Ok(None),
                    Type::Struct(_) => {
                        return Err(format!(
                            "var `{}` of struct type cannot be declared without an initializer",
                            self.interner.resolve(name).unwrap_or("?")
                        ));
                    }
                    Type::Tuple(_) => {
                        return Err(format!(
                            "var `{}` of tuple type cannot be declared without an initializer",
                            self.interner.resolve(name).unwrap_or("?")
                        ));
                    }
                    Type::Enum(_) => {
                        return Err(format!(
                            "var `{}` of enum type cannot be declared without an initializer",
                            self.interner.resolve(name).unwrap_or("?")
                        ));
                    }
                    Type::Str => {
                        return Err(format!(
                            "var `{}` of str type cannot be declared without an initializer",
                            self.interner.resolve(name).unwrap_or("?")
                        ));
                    }
                };
                self.emit(InstKind::StoreLocal { dst: local, src: zero }, None);
                Ok(None)
            }
            Stmt::Return(e) => {
                let ret_ty = self.module.function(self.func_id).return_type;
                // Tuple returns: the rhs must be a bare identifier
                // referring to a tuple binding (or a tuple literal we
                // route through the tail-position path). Expand into
                // per-element loads either way.
                if let (Type::Tuple(_), Some(er)) = (ret_ty, &e) {
                    let rhs_expr = self
                        .program
                        .expression
                        .get(er)
                        .ok_or_else(|| "return rhs missing".to_string())?;
                    if let Expr::Identifier(sym) = rhs_expr {
                        let elements = match self.bindings.get(&sym).cloned() {
                            Some(Binding::Tuple { elements }) => elements,
                            _ => {
                                return Err(format!(
                                    "`{}` is not a tuple binding of the expected return type",
                                    self.interner.resolve(sym).unwrap_or("?")
                                ));
                            }
                        };
                        let leaves = flatten_tuple_element_locals(&elements);
                        let mut values = Vec::with_capacity(leaves.len());
                        for (local, ty) in leaves {
                            let v = self
                                .emit(InstKind::LoadLocal(local), Some(ty))
                                .expect("LoadLocal returns a value");
                            values.push(v);
                        }
                        self.emit_ensures_checks(&values)?;
                        self.terminate_return(values);
                        return Ok(None);
                    }
                    // Tuple literal in explicit return: lower it
                    // through the tail-position helper, then emit
                    // the actual return reading the just-set pending
                    // values back out.
                    if let Expr::TupleLiteral(_) = rhs_expr {
                        let _ = self.lower_expr(er)?;
                        let elements = self.pending_tuple_value.take().ok_or_else(|| {
                            "tuple literal in explicit return produced no pending value"
                                .to_string()
                        })?;
                        let leaves = flatten_tuple_element_locals(&elements);
                        let mut values = Vec::with_capacity(leaves.len());
                        for (local, ty) in leaves {
                            let v = self
                                .emit(InstKind::LoadLocal(local), Some(ty))
                                .expect("LoadLocal returns a value");
                            values.push(v);
                        }
                        self.emit_ensures_checks(&values)?;
                        self.terminate_return(values);
                        return Ok(None);
                    }
                    return Err(
                        "explicit `return` of a tuple value must be a bare identifier or tuple literal in the compiler MVP"
                            .to_string(),
                    );
                }
                // Struct returns: the rhs must be a bare identifier
                // referring to a struct binding; expand into per-field
                // loads. Scalar / Unit returns share the regular
                // expression path.
                if let (Type::Struct(ret_struct_id), Some(er)) = (ret_ty, &e) {
                    let rhs_expr = self
                        .program
                        .expression
                        .get(er)
                        .ok_or_else(|| "return rhs missing".to_string())?;
                    let sym = match rhs_expr {
                        Expr::Identifier(s) => s,
                        _ => {
                            return Err(
                                "explicit `return` of a struct value must be a bare identifier in the compiler MVP"
                                    .to_string(),
                            );
                        }
                    };
                    let fields = match self.bindings.get(&sym).cloned() {
                        Some(Binding::Struct { struct_id: bn, fields }) if bn == ret_struct_id => {
                            fields
                        }
                        _ => {
                            return Err(format!(
                                "`{}` is not a struct binding of the expected return type",
                                self.interner.resolve(sym).unwrap_or("?")
                            ));
                        }
                    };
                    let leaves = flatten_struct_locals(&fields);
                    let mut values = Vec::with_capacity(leaves.len());
                    for (local, ty) in &leaves {
                        let v = self
                            .emit(InstKind::LoadLocal(*local), Some(*ty))
                            .expect("LoadLocal returns a value");
                        values.push(v);
                    }
                    self.emit_ensures_checks(&values)?;
                    self.terminate_return(values);
                    return Ok(None);
                }
                // Enum returns: rhs must be a bare identifier of an
                // Enum binding for the matching enum (or a tail-form
                // construction we route through the implicit-return
                // helper). Same pattern as struct/tuple — explicit
                // `return Enum::Variant(args)` is handled via
                // lower_expr setting pending_enum_value below.
                if let (Type::Enum(ret_enum_id), Some(er)) = (ret_ty, &e) {
                    let rhs_expr = self
                        .program
                        .expression
                        .get(er)
                        .ok_or_else(|| "return rhs missing".to_string())?;
                    if let Expr::Identifier(sym) = rhs_expr {
                        let storage = match self.bindings.get(&sym).cloned() {
                            Some(Binding::Enum(s)) if s.enum_id == ret_enum_id => s,
                            _ => {
                                return Err(format!(
                                    "`{}` is not an enum binding of the expected return type",
                                    self.interner.resolve(sym).unwrap_or("?")
                                ));
                            }
                        };
                        let values = self.load_enum_locals(&storage);
                        self.emit_ensures_checks(&values)?;
                        self.terminate_return(values);
                        return Ok(None);
                    }
                    return Err(
                        "explicit `return` of an enum value must be a bare identifier in the compiler MVP"
                            .to_string(),
                    );
                }
                let val = match e {
                    Some(er) => self.lower_expr(&er)?,
                    None => None,
                };
                match (ret_ty, val) {
                    (Type::Unit, _) => {
                        self.emit_ensures_checks(&[])?;
                        self.terminate_return(vec![]);
                    }
                    (_, Some(v)) => {
                        self.emit_ensures_checks(&[v])?;
                        self.terminate_return(vec![v]);
                    }
                    (_, None) => {
                        return Err("return without value in non-Unit function".to_string());
                    }
                }
                Ok(None)
            }
            Stmt::Break => {
                let (_cont, brk) = *self
                    .loop_stack
                    .last()
                    .ok_or_else(|| "`break` outside of a loop".to_string())?;
                self.terminate(Terminator::Jump(brk));
                Ok(None)
            }
            Stmt::Continue => {
                let (cont, _brk) = *self
                    .loop_stack
                    .last()
                    .ok_or_else(|| "`continue` outside of a loop".to_string())?;
                self.terminate(Terminator::Jump(cont));
                Ok(None)
            }
            Stmt::While(cond, body) => self.lower_while(&cond, &body),
            Stmt::For(var_name, start, end, body) => self.lower_for(var_name, &start, &end, &body),
            // Struct declarations are picked up by `collect_struct_defs`
            // before any function body is lowered; their presence inside
            // a function body (which the parser doesn't actually allow)
            // would be a no-op here. The same goes for trait / enum /
            // impl declarations until those features land in codegen.
            Stmt::StructDecl { .. } => Ok(None),
            Stmt::ImplBlock { .. } | Stmt::EnumDecl { .. } | Stmt::TraitDecl { .. } => Err(
                "compiler MVP cannot lower impl / enum / trait declarations yet".to_string(),
            ),
        }
    }
}
