use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use frontend::ast::*;
use frontend::type_checker::SourceLocation;
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::object::{Object, RcObject};
use crate::error::InterpreterError;
use crate::try_value;
use super::{EnumRegistryEntry, EnumRegistryVariant, EvaluationContext, EvaluationResult, StructRegistryEntry};
use std::collections::HashMap as HashMapStd;

/// Map a runtime value to the canonical-name `DefaultSymbol` the
/// extension-trait machinery uses as an impl target. Returns `None`
/// for non-primitive values (struct / enum / heap composite) — those
/// reach `evaluate_method_call`'s existing per-variant arms.
///
/// The interner lookup is `get` rather than `get_or_intern`: if a
/// program never wrote `impl Foo for i64 { ... }`, the canonical name
/// `"i64"` was never interned, so there is no symbol and no registry
/// entry to find. Falling back to `None` keeps the dispatch cost a
/// single hashmap probe per primitive method call.
/// Whether a value's rendering is the same in every engine
/// (DEBUG-OBS). The compiled backends report a contract's values
/// through `InstKind::ToString` on a scalar local; anything held as a
/// compound has no single value to hand a formatter there.
fn is_scalar_for_contract_report(obj: &Object) -> bool {
    matches!(
        obj,
        Object::Bool(_)
            | Object::Int64(_)
            | Object::UInt64(_)
            | Object::Int8(_)
            | Object::Int16(_)
            | Object::Int32(_)
            | Object::UInt8(_)
            | Object::UInt16(_)
            | Object::UInt32(_)
            | Object::Float64(_)
            | Object::String(_)
            | Object::ConstString(_)
    )
}

fn primitive_target_symbol(
    obj: &Object,
    interner: &DefaultStringInterner,
) -> Option<DefaultSymbol> {
    let name = match obj {
        Object::Bool(_) => "bool",
        Object::Int64(_) => "i64",
        Object::UInt64(_) => "u64",
        // NUM-W: narrow primitive method dispatch.
        Object::Int8(_) => "i8",
        Object::Int16(_) => "i16",
        Object::Int32(_) => "i32",
        Object::UInt8(_) => "u8",
        Object::UInt16(_) => "u16",
        Object::UInt32(_) => "u32",
        Object::Float64(_) => "f64",
        // SIMD-F32: `f32` was missing from every primitive-dispatch
        // table (this one, the two in the type checker, and the
        // lowering's), so `impl <Trait> for f32` was accepted by the
        // parser and unreachable afterwards.
        Object::Float32(_) => "f32",
        Object::ConstString(_) | Object::String(_) => "str",
        Object::Pointer(_) => "ptr",
        _ => return None,
    };
    interner.get(name)
}

/// Walk the struct field type-decls looking for `Generic(P)`
/// occurrences and bind each generic parameter to the runtime
/// type of the matching field. Returns `type_args` in the order
/// declared by `entry.generic_params`. Falls back to an empty
/// vector when no generic parameter binding can be derived (e.g.
/// non-generic struct, or generic param appearing only in nested
/// positions we don't drill into).
/// `ty` with each of `params` (as a parameter or a bare name) replaced.
fn substitute_params(ty: &TypeDecl, params: &HashMapStd<DefaultSymbol, TypeDecl>) -> TypeDecl {
    match ty {
        TypeDecl::Generic(s) | TypeDecl::Identifier(s) => {
            params.get(s).cloned().unwrap_or_else(|| ty.clone())
        }
        TypeDecl::Struct(n, args) => {
            TypeDecl::Struct(*n, args.iter().map(|a| substitute_params(a, params)).collect())
        }
        TypeDecl::Enum(n, args) => {
            TypeDecl::Enum(*n, args.iter().map(|a| substitute_params(a, params)).collect())
        }
        TypeDecl::Tuple(args) => {
            TypeDecl::Tuple(args.iter().map(|a| substitute_params(a, params)).collect())
        }
        TypeDecl::Ref { is_mut, inner } => {
            TypeDecl::Ref { is_mut: *is_mut, inner: Box::new(substitute_params(inner, params)) }
        }
        other => other.clone(),
    }
}

/// Whether `ty` names any of `params` (as a parameter or a bare name).
fn mentions_any(ty: &TypeDecl, params: &[DefaultSymbol]) -> bool {
    match ty {
        TypeDecl::Generic(s) | TypeDecl::Identifier(s) => params.contains(s),
        TypeDecl::Struct(_, args) | TypeDecl::Enum(_, args) | TypeDecl::Tuple(args) => {
            args.iter().any(|a| mentions_any(a, params))
        }
        TypeDecl::Ref { inner, .. } => mentions_any(inner, params),
        TypeDecl::Array(elems, _, _) => elems.iter().any(|a| mentions_any(a, params)),
        _ => false,
    }
}

fn derive_struct_type_args(
    entry: &StructRegistryEntry,
    field_values: &std::collections::HashMap<DefaultSymbol, RcObject>,
    active_scope: &HashMapStd<DefaultSymbol, TypeDecl>,
) -> Vec<TypeDecl> {
    if entry.generic_params.is_empty() {
        return Vec::new();
    }
    let mut bindings: HashMapStd<DefaultSymbol, TypeDecl> = HashMapStd::new();
    for (field_name, field_ty) in &entry.fields {
        if let Some(value) = field_values.get(field_name) {
            collect_generic_bindings(field_ty, &value.borrow(), &mut bindings);
        }
    }
    entry
        .generic_params
        .iter()
        .map(|p| {
            bindings
                .get(p)
                .cloned()
                // A *phantom* parameter — one no field mentions, like
                // `Ptr<T>`'s, which keeps `T` in the type and only an
                // untyped address in the struct — cannot be read back
                // off the field values. Ask the scope the literal is
                // being evaluated in: inside `impl<T> Ptr<T>` that is
                // the instance's own `T`, so the value carries the
                // argument its constructor was instantiated with.
                //
                // Without this the value is tagged `Unknown` and every
                // later method whose `T` appears only in the *return*
                // type (`fn get(&self, i: u64) -> T`) fails on an
                // unbound parameter — reachable as soon as the value
                // arrives through something other than an annotated
                // binding, e.g. out of an `Option<Ptr<T>>` payload.
                //
                // The scope can itself say `T -> T`: inside a generic
                // body that has not been instantiated, a parameter
                // stands for itself. Recording that would make the
                // value's own type arguments self-referential, and a
                // later `__builtin_sizeof::<T>()` chases the binding
                // forever. `Unknown` is the honest answer there.
                .or_else(|| {
                    active_scope.get(p).cloned().filter(|t| match t {
                        TypeDecl::Generic(g) => g != p,
                        TypeDecl::Identifier(g) => g != p,
                        _ => true,
                    })
                })
                .unwrap_or(TypeDecl::Unknown)
        })
        .collect()
}

/// Variant counterpart to `derive_struct_type_args`. Walks the
/// variant payload type list against the constructed argument
/// values to derive each generic parameter binding.
fn derive_enum_type_args(
    entry: &EnumRegistryEntry,
    variant: &EnumRegistryVariant,
    arg_values: &[RcObject],
) -> Vec<TypeDecl> {
    if entry.generic_params.is_empty() {
        return Vec::new();
    }
    let mut bindings: HashMapStd<DefaultSymbol, TypeDecl> = HashMapStd::new();
    for (declared, value) in variant.payload_types.iter().zip(arg_values.iter()) {
        collect_generic_bindings(declared, &value.borrow(), &mut bindings);
    }
    entry
        .generic_params
        .iter()
        .map(|p| bindings.get(p).cloned().unwrap_or(TypeDecl::Unknown))
        .collect()
}

/// Recursively match a declared `TypeDecl` against the actual
/// runtime value to populate `bindings` with `Generic(P) -> Type`
/// pairs. Handles the common cases (`T`, `Cell<T>`, `(T, U)`) so
/// generic struct / enum / tuple instantiations infer correctly
/// without re-running the type-checker.
fn collect_generic_bindings(
    declared: &TypeDecl,
    value: &Object,
    bindings: &mut HashMapStd<DefaultSymbol, TypeDecl>,
) {
    match declared {
        TypeDecl::Generic(sym) => {
            // `get_type` answers a generic struct or enum with no type
            // arguments (`Option`, not `Option<String>`), which binds
            // `T` to a type nothing can size -- `Box::new(Option::Some(s))`
            // then failed at its first `sizeof::<T>()`. The value
            // carries its own arguments; use them.
            let runtime_ty = match value {
                Object::Struct { type_name, type_args, .. } => {
                    TypeDecl::Struct(*type_name, type_args.clone())
                }
                Object::EnumVariant { enum_name, type_args, .. } => {
                    TypeDecl::Enum(*enum_name, type_args.clone())
                }
                _ => value.get_type(),
            };
            bindings.entry(*sym).or_insert(runtime_ty);
        }
        TypeDecl::Struct(_, args) | TypeDecl::Enum(_, args) => {
            // Pull the value's own type-arg vector if it has one.
            // Recurse element-wise so `Cell<T>` against a
            // `Cell<i64>` value resolves T = i64.
            let runtime_args = match value {
                Object::Struct { type_args, .. } => type_args.clone(),
                Object::EnumVariant { type_args, .. } => type_args.clone(),
                _ => Vec::new(),
            };
            for (decl_arg, runtime_arg) in args.iter().zip(runtime_args.iter()) {
                if let TypeDecl::Generic(sym) = decl_arg {
                    bindings.entry(*sym).or_insert(runtime_arg.clone());
                }
            }
        }
        TypeDecl::Tuple(decl_elems) => {
            if let Object::Tuple(value_elems) = value {
                for (decl, val) in decl_elems.iter().zip(value_elems.iter()) {
                    collect_generic_bindings(decl, &val.borrow(), bindings);
                }
            }
        }
        // TREE-WALKER-GENERIC-SCOPE: a method-level parameter named only
        // by a function argument -- `map<U>(&self, f: fn (T) -> U)` --
        // is the closure's own type. Without this `U` was never bound,
        // so the `MapIter<T, U>` it built carried `U` unresolved and a
        // `Vec<U>` made from it could not size its elements.
        TypeDecl::Function(decl_params, decl_ret) => {
            if let Object::Closure { params, return_ty, .. } = value {
                for (decl, (_, actual)) in decl_params.iter().zip(params.iter()) {
                    bind_declared_type(decl, actual, bindings);
                }
                bind_declared_type(decl_ret, return_ty, bindings);
            }
        }
        _ => {}
    }
}

/// `collect_generic_bindings` for a declared type against a *type*
/// (a closure's signature) rather than a value.
fn bind_declared_type(
    declared: &TypeDecl,
    actual: &TypeDecl,
    bindings: &mut HashMapStd<DefaultSymbol, TypeDecl>,
) {
    match (declared, actual) {
        (TypeDecl::Generic(sym), _) if !matches!(actual, TypeDecl::Unknown) => {
            bindings.entry(*sym).or_insert(actual.clone());
        }
        (TypeDecl::Struct(_, d), TypeDecl::Struct(_, a)) | (TypeDecl::Enum(_, d), TypeDecl::Enum(_, a)) => {
            for (dd, aa) in d.iter().zip(a.iter()) {
                bind_declared_type(dd, aa, bindings);
            }
        }
        (TypeDecl::Tuple(d), TypeDecl::Tuple(a)) => {
            for (dd, aa) in d.iter().zip(a.iter()) {
                bind_declared_type(dd, aa, bindings);
            }
        }
        _ => {}
    }
}

/// POINTER P1: the generic-parameter scope a call's *arguments*
/// determine — each declared parameter type matched against its
/// runtime value through [`collect_generic_bindings`]. This is the
/// tree-walker's counterpart of what the compiled lanes do when they
/// infer a generic instantiation from argument types.
///
/// `params` must be aligned with `args` — the caller strips an
/// explicit `self: Self` entry first, exactly like the binding loop
/// it sits next to.
fn args_generic_scope(
    params: &[(DefaultSymbol, TypeDecl)],
    args: &[RcObject],
) -> HashMap<DefaultSymbol, TypeDecl> {
    let mut bindings: HashMapStd<DefaultSymbol, TypeDecl> = HashMapStd::new();
    for ((_, declared), value) in params.iter().zip(args.iter()) {
        collect_generic_bindings(declared, &value.borrow(), &mut bindings);
    }
    bindings
}

/// REF-Stage-2 (i)+(iii): caller-side target of an explicit
/// `&mut <lvalue>` borrow argument. Captured at the call site so
/// the post-body parameter value can flow back into the caller's
/// storage with the right shape.
#[derive(Debug, Clone)]
pub(super) enum WritebackTarget {
    /// The argument was not `&mut <lvalue>`, or the lvalue shape
    /// isn't supported yet (e.g. `&mut arr[i]`).
    None,
    /// `&mut <name>` — write back into the local binding `name`.
    Name(DefaultSymbol),
    /// `&mut <expr>.<field>` — `obj` is the parent struct value
    /// (captured at call time so we keep a stable Rc to the
    /// underlying `Object::Struct`); `field` is the field symbol
    /// to overwrite via `borrow_mut`.
    StructField {
        obj: RcObject,
        field: DefaultSymbol,
    },
    /// `&mut <expr>.<index>` — `obj` is the parent tuple value
    /// (captured Rc to the `Object::Tuple` cell); `index` is
    /// the element position to overwrite via `borrow_mut` +
    /// indexed assignment.
    TupleElement {
        obj: RcObject,
        index: usize,
    },
    /// `&mut <name>[i]` — `obj` is the parent array value
    /// (captured Rc to the `Object::Array` cell); `index` is
    /// the position to overwrite via `borrow_mut`.
    ArrayElement {
        obj: RcObject,
        index: usize,
    },
}

impl EvaluationContext<'_> {
    /// Classify a `&mut <lvalue>` operand into a `WritebackTarget`.
    /// Walks `Expr::Identifier` and `Expr::FieldAccess` (one level
    /// from the root) — anything else (deeper chains, tuple
    /// access, index access) currently falls back to `None` so
    /// the call still runs but no writeback fires. Future phases
    /// can broaden the supported lvalue shapes.
    pub(super) fn classify_writeback_target(
        &mut self,
        operand: &ExprRef,
    ) -> Result<WritebackTarget, InterpreterError> {
        let expr = self
            .expr_pool
            .get(operand)
            .ok_or_else(|| InterpreterError::InternalError("classify_writeback_target: unbound operand".to_string()))?;
        match expr {
            Expr::Identifier(sym) => Ok(WritebackTarget::Name(sym)),
            Expr::FieldAccess(obj, field) => {
                let obj_value = self.evaluate(&obj);
                let obj_value = match obj_value {
                    Ok(EvaluationResult::Value(v)) => v,
                    Ok(_) => return Ok(WritebackTarget::None),
                    Err(e) => return Err(e),
                };
                // Coerce the Value to a `RcObject` (no-op when it
                // was already `Heap(_)`; primitives wrap into a
                // fresh cell, but field-target writeback only
                // makes sense for struct values, which always
                // ride `Heap`).
                Ok(WritebackTarget::StructField {
                    obj: obj_value.clone_to_rc(),
                    field,
                })
            }
            Expr::TupleAccess(obj, index) => {
                let obj_value = self.evaluate(&obj);
                let obj_value = match obj_value {
                    Ok(EvaluationResult::Value(v)) => v,
                    Ok(_) => return Ok(WritebackTarget::None),
                    Err(e) => return Err(e),
                };
                Ok(WritebackTarget::TupleElement {
                    obj: obj_value.clone_to_rc(),
                    index,
                })
            }
            Expr::SliceAccess(obj, info) => {
                if !matches!(info.slice_type, frontend::ast::SliceType::SingleElement) {
                    return Ok(WritebackTarget::None);
                }
                let idx_expr = match info.start {
                    Some(e) => e,
                    None => return Ok(WritebackTarget::None),
                };
                // Evaluate the array (parent) and the index in
                // user order so any side effects in the index
                // expression run exactly once at the call site.
                let obj_value = match self.evaluate(&obj)? {
                    EvaluationResult::Value(v) => v,
                    _ => return Ok(WritebackTarget::None),
                };
                let idx_value = match self.evaluate(&idx_expr)? {
                    EvaluationResult::Value(v) => v,
                    _ => return Ok(WritebackTarget::None),
                };
                let index = match idx_value {
                    crate::value::Value::UInt64(n) => n as usize,
                    crate::value::Value::Int64(n) => n as usize,
                    crate::value::Value::UInt8(n) => n as usize,
                    crate::value::Value::UInt16(n) => n as usize,
                    crate::value::Value::UInt32(n) => n as usize,
                    crate::value::Value::Int8(n) => n as usize,
                    crate::value::Value::Int16(n) => n as usize,
                    crate::value::Value::Int32(n) => n as usize,
                    _ => return Ok(WritebackTarget::None),
                };
                Ok(WritebackTarget::ArrayElement {
                    obj: obj_value.clone_to_rc(),
                    index,
                })
            }
            _ => Ok(WritebackTarget::None),
        }
    }

    /// Apply a captured `WritebackTarget` with the post-body
    /// `value`. Identifier targets go through `set_var` /
    /// `Overwrite` (mirroring `var` reassignment); struct field
    /// targets borrow the captured `Rc` and overwrite the field
    /// in place (mirroring `obj.field = value` user code).
    pub(super) fn apply_writeback(
        &mut self,
        target: &WritebackTarget,
        value: crate::value::Value,
    ) -> Result<(), InterpreterError> {
        match target {
            WritebackTarget::None => Ok(()),
            WritebackTarget::Name(sym) => {
                let _ = self.environment.set_var(
                    *sym,
                    value,
                    crate::environment::VariableSetType::Overwrite,
                    self.string_interner,
                );
                Ok(())
            }
            WritebackTarget::StructField { obj, field } => {
                let new_value: RcObject = value.clone_to_rc();
                let mut obj_borrowed = obj.borrow_mut();
                match &mut *obj_borrowed {
                    Object::Struct { fields, .. } => {
                        if !fields.contains_key(field) {
                            let field_name = self
                                .string_interner
                                .resolve(*field)
                                .unwrap_or("<unknown>");
                            return Err(InterpreterError::InternalError(format!(
                                "writeback: unknown field '{}'", field_name
                            )));
                        }
                        fields.insert(*field, new_value);
                        Ok(())
                    }
                    other => Err(InterpreterError::InternalError(format!(
                        "writeback: parent is not a struct: {:?}", other
                    ))),
                }
            }
            WritebackTarget::TupleElement { obj, index } => {
                let new_value: RcObject = value.clone_to_rc();
                let mut obj_borrowed = obj.borrow_mut();
                match &mut *obj_borrowed {
                    Object::Tuple(elements) => {
                        if *index >= elements.len() {
                            return Err(InterpreterError::IndexOutOfBounds {
                                index: *index as isize,
                                size: elements.len(),
                            });
                        }
                        elements[*index] = new_value;
                        Ok(())
                    }
                    other => Err(InterpreterError::InternalError(format!(
                        "writeback: parent is not a tuple: {:?}", other
                    ))),
                }
            }
            WritebackTarget::ArrayElement { obj, index } => {
                let new_value: RcObject = value.clone_to_rc();
                let mut obj_borrowed = obj.borrow_mut();
                match &mut *obj_borrowed {
                    Object::Array(elements) => {
                        if *index >= elements.len() {
                            return Err(InterpreterError::IndexOutOfBounds {
                                index: *index as isize,
                                size: elements.len(),
                            });
                        }
                        elements[*index] = new_value;
                        Ok(())
                    }
                    other => Err(InterpreterError::InternalError(format!(
                        "writeback: parent is not an array: {:?}", other
                    ))),
                }
            }
        }
    }

    /// MEMORY-ACCESS M5: the stdlib `Ptr<T>`'s `get` / `set` (and
    /// `__getitem__` / `__setitem__`) performed here instead of entering
    /// the method -- the tree-walker's side of the compiled lanes'
    /// intrinsic. Each method call here costs a frame of native stack
    /// and a scope, so a collection written over `Ptr<T>` would pay one
    /// more of each per element access; with JSON's recursion on top,
    /// that overflowed the stack. The read and write are the body's:
    /// `i * sizeof::<T>()` bytes from `addr`.
    ///
    /// Only a method written in `std.ptr` qualifies; `Ok(None)` means
    /// "call it".
    fn ptr_access_intrinsic(
        &mut self,
        method: &MethodFunction,
        self_obj: &RcObject,
        args: &[RcObject],
    ) -> Result<Option<EvaluationResult>, InterpreterError> {
        let from_std_ptr = method.module_path.as_deref().is_some_and(|path| {
            let names: Vec<&str> =
                path.iter().filter_map(|s| self.string_interner.resolve(*s)).collect();
            names == ["std", "ptr"]
        });
        if !from_std_ptr {
            return Ok(None);
        }
        // `Ptr<u8>::load16` / `store16`: the vector load or store at
        // element (= byte) `i`, exactly as `__simd_load` / `__simd_store`.
        let vector_access = match self.string_interner.resolve(method.name) {
            Some("load16") => Some(false),
            Some("store16") => Some(true),
            _ => None,
        };
        if let Some(store) = vector_access {
            if args.len() != if store { 2 } else { 1 } {
                return Ok(None);
            }
            let Some(addr_sym) = self.string_interner.get("addr") else {
                return Ok(None);
            };
            let addr = match &*self_obj.borrow() {
                Object::Struct { fields, .. } => {
                    match fields.get(&addr_sym).and_then(|v| v.borrow().try_unwrap_pointer().ok()) {
                        Some(a) => a,
                        None => return Ok(None),
                    }
                }
                _ => return Ok(None),
            };
            let index = args[0].borrow().try_unwrap_uint64().map_err(|_| {
                InterpreterError::InternalError("Ptr::load16 index is not a u64".to_string())
            })?;
            if store {
                let vector = match &*args[1].borrow() {
                    Object::Simd(v) => *v,
                    _ => {
                        return Err(InterpreterError::InternalError(
                            "Ptr::store16 value is not a vector".to_string(),
                        ))
                    }
                };
                self.simd_write(vector, addr, index);
                return Ok(Some(EvaluationResult::Value(crate::object::Object::Unit.into())));
            }
            let v = self.simd_read(frontend::type_decl::VectorType::U8x16, addr, index);
            return Ok(Some(EvaluationResult::Value(crate::object::Object::Simd(v).into())));
        }
        // `borrow` reads the same slot: `__builtin_ptr_ref` and
        // `__builtin_ptr_read` evaluate alike here, and the binding
        // that catches a borrow is kept off the drop list by its type.
        let is_set = match self.string_interner.resolve(method.name) {
            Some("get" | "__getitem__" | "borrow") => false,
            Some("set" | "__setitem__") => true,
            _ => return Ok(None),
        };
        if args.len() != if is_set { 2 } else { 1 } {
            return Ok(None);
        }
        let Some(addr_sym) = self.string_interner.get("addr") else {
            return Ok(None);
        };
        // `SoaPtr<T>`: the tree-walker keys a column-split buffer by
        // element index and keeps the whole value there, exactly as
        // `__builtin_soa_read` / `__builtin_soa_write` do, so `cap` is
        // not needed to find the slot.
        let is_soa = match &*self_obj.borrow() {
            Object::Struct { type_name, .. } => {
                self.string_interner.resolve(*type_name) == Some("SoaPtr")
            }
            _ => false,
        };
        if is_soa {
            let addr = match &*self_obj.borrow() {
                Object::Struct { fields, .. } => {
                    match fields.get(&addr_sym).and_then(|v| v.borrow().try_unwrap_pointer().ok()) {
                        Some(a) => a,
                        None => return Ok(None),
                    }
                }
                _ => return Ok(None),
            };
            let index = args[0].borrow().try_unwrap_uint64().map_err(|_| {
                InterpreterError::InternalError("SoaPtr index is not a u64".to_string())
            })? as usize;
            if is_set {
                self.heap_manager.borrow_mut().typed_write(addr, index, args[1].clone());
                return Ok(Some(EvaluationResult::Value(crate::object::Object::Unit.into())));
            }
            return match self.heap_manager.borrow().typed_read(addr, index) {
                Some(v) => Ok(Some(EvaluationResult::Value(v.into()))),
                None => Err(InterpreterError::InternalError(
                    "Invalid memory access in soa_read (element never written)".to_string(),
                )),
            };
        }
        let (addr, elem_ty) = match &*self_obj.borrow() {
            Object::Struct { fields, type_args, .. } => {
                let Some(addr) = fields.get(&addr_sym).and_then(|v| v.borrow().try_unwrap_pointer().ok())
                else {
                    return Ok(None);
                };
                let Some(elem_ty) = type_args.first().cloned() else {
                    return Ok(None);
                };
                (addr, elem_ty)
            }
            _ => return Ok(None),
        };
        let scope = self.merged_generic_scope();
        let elem_ty = match &elem_ty {
            TypeDecl::Identifier(s) | TypeDecl::Generic(s) => {
                scope.get(s).cloned().unwrap_or(elem_ty)
            }
            _ => elem_ty,
        };
        let Some(size) = self.type_decl_byte_size(&elem_ty, &scope) else {
            return Ok(None);
        };
        let index = args[0].borrow().try_unwrap_uint64().map_err(|_| {
            InterpreterError::InternalError("Ptr index is not a u64".to_string())
        })?;
        let offset = index.wrapping_mul(size);
        if is_set {
            self.write_value_at(addr, offset, args[1].clone())?;
            Ok(Some(EvaluationResult::Value(crate::object::Object::Unit.into())))
        } else {
            Ok(Some(EvaluationResult::Value(self.read_typed_at(addr, offset, &elem_ty)?)))
        }
    }

    pub(super) fn call_method(
        &mut self,
        method: Rc<MethodFunction>,
        self_obj: RcObject,
        args: Vec<RcObject>,
        call_site: Option<SourceLocation>,
    ) -> Result<EvaluationResult, InterpreterError> {
        if let Some(result) = self.ptr_access_intrinsic(&method, &self_obj, &args)? {
            return Ok(result);
        }
        // DEBUG-OBS D1: this is the choke point every method reaches —
        // `s.boom()`, an operator overload, a `dyn` dispatch, drop
        // glue — so the frame goes here rather than at each of the
        // callers that used to leave the innermost frame off the
        // backtrace entirely (実測 3).
        let frame = self.method_frame_name(&self_obj, method.name);
        self.push_frame(frame, call_site);

        // Create new scope for method execution
        // STDLIB-FN-SHADOWED-BY-USER-FN: a bare call in this
        // body resolves in the module the method was written
        // in. Restored beside every `exit_block` below.
        let prev_home = std::mem::replace(
            &mut self.current_module_path,
            method.module_path.clone(),
        );
        self.environment.enter_block();

        // Stage 1 of `&` references: implicit `&self` / `&mut self`
        // receivers don't appear in `method.parameter` (the parser
        // only flips `has_self_param=true`). Bind the `self`
        // identifier explicitly to the same Rc the caller passed
        // — RefCell semantics give reference behaviour for free,
        // so `&self` / `&mut self` / `self: Self` are runtime-
        // equivalent here. The frontend type checker enforces
        // mutability at compile time.
        let first_param_is_self = method
            .parameter
            .first()
            .and_then(|(sym, _)| self.string_interner.resolve(*sym))
            .map(|name| name == "self")
            .unwrap_or(false);
        let bind_implicit_self = method.has_self_param && !first_param_is_self;
        if bind_implicit_self {
            if let Some(self_sym) = self.string_interner.get("self") {
                self.environment.set_val(self_sym, self_obj.clone().into());
            }
        }

        // Set up method parameters
        let mut param_index = 0;

        // Bind method parameters - first parameter should be self
        // (when the source uses the `self: Self` form)
        for (param_symbol, _param_type) in &method.parameter {
            if param_index == 0 && first_param_is_self {
                // First parameter is `self: Self` -- **by value**, so
                // the callee gets its own copy of the compound spine.
                // Sharing the `Rc` here let a write inside the method
                // reach the caller's binding, which no compiled lane
                // does (BY-VALUE-SELF-ALIAS).
                self.environment.set_val(
                    *param_symbol,
                    crate::object::copy_for_by_value_param(&self_obj).into(),
                );
            } else {
                // Subsequent parameters are regular args. With
                // an implicit self receiver the first method.parameter
                // entry IS the first user arg, so use param_index
                // directly when self isn't in the list.
                let arg_idx = if first_param_is_self {
                    if param_index == 0 { continue; } else { param_index - 1 }
                } else {
                    param_index
                };
                if arg_idx < args.len() {
                    self.environment.set_val(*param_symbol, args[arg_idx].clone().into());
                }
            }
            param_index += 1;
        }

        // POINTER P1: the generic-parameter scope this call can see —
        // the receiver's runtime `type_args` (`impl<T> Ptr<T>` methods
        // get `T` from the value), the arguments (method-only
        // generics, `fn map<U>`), then the caller's own scope for
        // anything still unbound. Pushed for the whole body so
        // `__builtin_sizeof::<T>()` inside it answers what this
        // instance was instantiated with. Non-generic callees push an
        // empty scope so every exit path can pop unconditionally.
        let mut generic_scope = self.receiver_generic_scope(&self_obj.borrow());
        if !method.generic_params.is_empty() {
            let arg_params: &[(DefaultSymbol, TypeDecl)] = if first_param_is_self {
                &method.parameter[1..]
            } else {
                &method.parameter
            };
            for (param, ty) in args_generic_scope(arg_params, &args) {
                generic_scope.entry(param).or_insert(ty);
            }
            self.fill_scope_from_caller(&mut generic_scope, &method.generic_params);
        }
        self.push_generic_type_scope(generic_scope);

        // Pre-body `requires` checks. `self` and named args are visible above.
        if let Err(e) = self.evaluate_requires_clauses(method.name, &method.requires, &method.parameter) {
            self.pop_generic_type_scope();
            self.environment.exit_block();
                self.current_module_path = prev_home.clone();
            return Err(e);
        }
        if let Err(e) = self.evaluate_old_snapshots(&method.old_exprs) {
            self.pop_generic_type_scope();
            self.environment.exit_block();
                self.current_module_path = prev_home.clone();
            return Err(e);
        }

        // Execute method body
        let result = self.evaluate_method(&method);

        // Post-body `ensures` checks with `result` bound to the method's
        // produced value. Skip if the body already errored or propagated a
        // non-value flow (e.g. break/continue would be a bug at this layer
        // anyway, but we don't want to mask the original error).
        self.pop_generic_type_scope();
        let result = match result {
            Ok(EvaluationResult::Value(v)) => {
                if let Err(e) = self.evaluate_ensures_clauses(method.name, &method.ensures, &method.ensures_kinds, v.clone_to_rc(), &method.parameter) {
                    self.environment.exit_block();
                self.current_module_path = prev_home.clone();
                    return Err(e);
                }
                Ok(EvaluationResult::Value(v))
            }
            Ok(EvaluationResult::Return(v)) => {
                let ret = v.clone().map(|val| val.into_rc()).unwrap_or_else(|| Rc::new(RefCell::new(Object::Unit)));
                if let Err(e) = self.evaluate_ensures_clauses(method.name, &method.ensures, &method.ensures_kinds, ret, &method.parameter) {
                    self.environment.exit_block();
                self.current_module_path = prev_home.clone();
                    return Err(e);
                }
                // DICT-RETURN-WHILE follow-up: an explicit `return v`
                // inside the method body bubbles up here as
                // EvaluationResult::Return. Convert to Value at the
                // method boundary — Return is a control-flow signal
                // for the *callee's* enclosing control structure
                // (loop, block) and shouldn't leak into the
                // caller's scope. Without this, a `return` inside
                // a callee's `while` loop (now propagating
                // correctly per the `evaluate_block::While` arm
                // fix) would also unwind the *caller's* function.
                Ok(EvaluationResult::Value(v.unwrap_or(crate::value::Value::Unit)))
            }
            other => other,
        };

        // Clean up scope
        self.environment.exit_block();
                self.current_module_path = prev_home.clone();
        if result.is_ok() {
            self.pop_frame();
        }

        result
    }

    /// `S::boom` — a method frame named by the receiver's *runtime*
    /// type, so a `dyn Trait` call says which impl actually ran and a
    /// bare `boom` never appears on its own.
    fn method_frame_name(&self, self_obj: &RcObject, method: DefaultSymbol) -> String {
        let method_name = self.string_interner.resolve(method).unwrap_or("<unknown>");
        let obj = self_obj.borrow();
        let owner = match &*obj {
            Object::Struct { type_name, .. } => Some(*type_name),
            Object::EnumVariant { enum_name, .. } => Some(*enum_name),
            other => primitive_target_symbol(other, self.string_interner),
        };
        match owner.and_then(|sym| self.string_interner.resolve(sym)) {
            Some(owner) => format!("{owner}::{method_name}"),
            None => method_name.to_string(),
        }
    }

    /// Values the failing predicate was looking at: every parameter,
    /// plus `result` for an `ensures`.
    ///
    /// LLM-LOOP P6: `requires clause #0 violated` says which predicate
    /// failed but not what made it fail; with `b = 0i64` the reader has
    /// the counterexample without instrumenting the call.
    fn capture_contract_bindings(
        &mut self,
        params: &ParameterList,
        include_result: bool,
    ) -> Vec<(String, String)> {
        let mut names: Vec<DefaultSymbol> = params.iter().map(|(name, _)| *name).collect();
        if include_result {
            names.push(self.result_symbol);
        }
        names
            .into_iter()
            .filter_map(|sym| {
                let value = self.environment.get_val(sym)?;
                let name = self.string_interner.resolve(sym)?.to_string();
                let obj = value.into_rc();
                let borrowed = obj.borrow();
                // DEBUG-OBS: scalars only, matching what the compiled
                // backends can put in the same sentence. The rule is
                // shared rather than a limitation of one engine — a
                // diagnostic that lists a struct's fields here and
                // omits them there is worse than one that consistently
                // names what every engine can render.
                if !is_scalar_for_contract_report(&borrowed) {
                    return None;
                }
                let rendered = borrowed.to_display_string(self.string_interner);
                Some((name, rendered))
            })
            .collect()
    }

    /// Evaluate every `requires` clause for the given callable against the
    /// current environment (parameters and, for methods, `self` already
    /// bound). Returns the first violation as a ContractViolation error.
    /// No-op when the active `INTERPRETER_CONTRACTS` mode disables
    /// pre-checks. Shared by `evaluate_function_with_values`,
    /// `call_method`, and `call_associated_method`.
    fn evaluate_requires_clauses(
        &mut self,
        fn_name: DefaultSymbol,
        clauses: &[ExprRef],
        params: &ParameterList,
    ) -> Result<(), InterpreterError> {
        if !self.contract_mode.check_pre || clauses.is_empty() {
            return Ok(());
        }
        for (idx, cond) in clauses.iter().enumerate() {
            // Contract predicates are bool expressions; control flow
            // (Return / Break / Continue) inside them is meaningless and is
            // rejected as an internal error rather than propagated.
            let cond_res = self.evaluate(cond)?;
            let cond_obj = self.unwrap_value(cond_res)?;
            let passed = cond_obj.borrow().try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
            if !passed {
                let mut backtrace = self.call_stack.clone();
                backtrace.reverse();
                return Err(InterpreterError::ContractViolation(Box::new(
                    crate::error::ContractViolation {
                    detail: None,
                    kind: "requires",
                    function: self.string_interner.resolve(fn_name).unwrap_or("<unknown>").to_string(),
                    clause_index: idx,
                    bindings: self.capture_contract_bindings(params, false),
                    backtrace,
                    location: self.expr_location(cond),
                    },
                )));
            }
        }
        Ok(())
    }

    /// Bind `result` to the callable's produced value and evaluate every
    /// `ensures` clause. The caller is responsible for cleaning up the
    /// environment block; we don't enter/exit a new scope here so the
    /// `result` binding lives in the same scope as the parameters.
    /// ALLOC-CONTRACT: evaluate the `old(...)` snapshots and bind each
    /// to the `__old_N` name its `ensures` clause refers to.
    ///
    /// Runs on entry, after `requires` (a precondition may be what
    /// makes the snapshot expression legal) and before the body, so
    /// the value recorded is genuinely the pre-state. Skipped entirely
    /// when postconditions are off: the snapshot would have no reader,
    /// and the expressions can be as costly as any other call.
    fn evaluate_old_snapshots(&mut self, old_exprs: &[ExprRef]) -> Result<(), InterpreterError> {
        if !self.contract_mode.check_post || old_exprs.is_empty() {
            return Ok(());
        }
        for (index, expr) in old_exprs.iter().enumerate() {
            let value = self.evaluate(expr)?;
            let obj = self.unwrap_value(value)?;
            let sym = self.string_interner.get_or_intern(format!("__old_{index}"));
            self.environment.set_val(sym, obj.into());
        }
        Ok(())
    }

    fn evaluate_ensures_clauses(
        &mut self,
        fn_name: DefaultSymbol,
        clauses: &[ExprRef],
        kinds: &[EnsuresKind],
        return_value: RcObject,
        params: &ParameterList,
    ) -> Result<(), InterpreterError> {
        if !self.contract_mode.check_post || clauses.is_empty() {
            return Ok(());
        }
        self.environment.set_val(self.result_symbol, (return_value).into());
        for (idx, cond) in clauses.iter().enumerate() {
            let cond_res = self.evaluate(cond)?;
            let cond_obj = self.unwrap_value(cond_res)?;
            let passed = cond_obj.borrow().try_unwrap_bool().map_err(InterpreterError::ObjectError)?;
            if !passed {
                // ALLOC-CONTRACT-SUGAR: a budget clause knows what it
                // was counting, so it can report the amount instead of
                // leaving the reader to run `--profile=mem` and compare.
                let detail = match kinds.get(idx) {
                    Some(EnsuresKind::AllocBudget { stat, old_index }) => {
                        self.alloc_budget_detail(*stat, *old_index, cond)
                    }
                    _ => None,
                };
                let mut backtrace = self.call_stack.clone();
                backtrace.reverse();
                return Err(InterpreterError::ContractViolation(Box::new(
                    crate::error::ContractViolation {
                    detail,
                    kind: "ensures",
                    function: self.string_interner.resolve(fn_name).unwrap_or("<unknown>").to_string(),
                    clause_index: idx,
                    bindings: self.capture_contract_bindings(params, true),
                    backtrace,
                    location: self.expr_location(cond),
                    },
                )));
            }
        }
        Ok(())
    }

    /// ALLOC-CONTRACT-SUGAR: render "retained 128 bytes, budget 0
    /// bytes" for a violated budget clause.
    ///
    /// The clause is `counter() <= __old_N + budget` by construction,
    /// so the amount consumed is the counter's current value minus the
    /// entry snapshot, and the allowance is the right-hand side minus
    /// the same snapshot. Anything unexpected about that shape yields
    /// `None` and the generic message, since a wrong number would be
    /// worse than no number.
    fn alloc_budget_detail(
        &mut self,
        stat: frontend::ast::MemStat,
        old_index: usize,
        clause: &ExprRef,
    ) -> Option<String> {
        let entry_sym = self.string_interner.get(format!("__old_{old_index}"))?;
        let entry = match self.environment.get_val(entry_sym)? {
            crate::value::Value::UInt64(v) => v,
            _ => return None,
        };
        let current = crate::heap::profile().field(stat);
        let limit = match self.expr_pool.get(clause)? {
            frontend::ast::Expr::Binary(_, _, rhs) => {
                let value = self.evaluate(&rhs).ok()?;
                let obj = self.unwrap_value(value).ok()?;
                let limit = obj.borrow().try_unwrap_uint64().ok()?;
                limit
            }
            _ => return None,
        };
        let used = current.saturating_sub(entry);
        let budget = limit.saturating_sub(entry);
        Some(match stat {
            frontend::ast::MemStat::CumulativeBytes => {
                format!("requested {used} bytes, budget {budget} bytes")
            }
            frontend::ast::MemStat::LiveBytes => {
                format!("retained {used} bytes, budget {budget} bytes")
            }
            frontend::ast::MemStat::AllocCount => {
                format!("made {used} allocations, budget {budget}")
            }
            _ => return None,
        })
    }

    /// Call an associated method (without self parameter)
    pub(super) fn call_associated_method(
        &mut self,
        method: Rc<MethodFunction>,
        args: Vec<RcObject>,
        owner: Option<DefaultSymbol>,
        call_site: Option<SourceLocation>,
    ) -> Result<EvaluationResult, InterpreterError> {
        // DEBUG-OBS D1. No receiver to read a type off, so the owner
        // comes from the call site (`String::from_str` was written
        // that way); unqualified only when the caller had none.
        let name = self.string_interner.resolve(method.name).unwrap_or("<unknown>").to_string();
        let frame = match owner.and_then(|sym| self.string_interner.resolve(sym)) {
            Some(owner) => format!("{owner}::{name}"),
            None => name,
        };
        self.push_frame(frame, call_site);

        // Create new scope for method execution
        self.environment.enter_block();

        // Set up method parameters - skip self parameter for associated functions.
        // Stage 1 of `&` references: with `&self` / `&mut self` (the
        // implicit form), `self` is not in `method.parameter`, so
        // there is nothing to skip — every entry is a user arg.
        let first_param_is_self = method
            .parameter
            .first()
            .and_then(|(sym, _)| self.string_interner.resolve(*sym))
            .map(|name| name == "self")
            .unwrap_or(false);
        let skip_self = method.has_self_param && first_param_is_self;
        let mut param_index = 0;

        // Bind method parameters
        for (param_symbol, _param_type) in &method.parameter {
            if skip_self && param_index == 0 {
                // Skip self parameter for associated functions
                param_index += 1;
                continue;
            }

            let arg_index = if skip_self { param_index - 1 } else { param_index };
            if arg_index < args.len() {
                self.environment.set_val(*param_symbol, args[arg_index].clone().into());
            }
            param_index += 1;
        }

        // POINTER P1: the generic-parameter scope this call can see —
        // arguments first (`Box::new(value)` binds `T` from the
        // value), then the pending `val` / `var` annotation
        // (`val h: Holder<u64> = Holder::make(n)` — `T` appears only
        // in the return type, so the annotation is the one source,
        // exactly what the compiled lanes' let-lowering reads), then
        // the caller's scope. Pushed for the whole body so
        // `__builtin_sizeof::<T>()` inside it resolves. Non-generic
        // callees push an empty scope so every exit path can pop
        // unconditionally.
        let mut generic_scope = HashMap::new();
        if !method.generic_params.is_empty() {
            for (param, ty) in args_generic_scope(
                if skip_self { &method.parameter[1..] } else { &method.parameter },
                &args,
            ) {
                generic_scope.insert(param, ty);
            }
            if let Some(owner) = owner {
                let anno_scope =
                    self.annotation_generic_scope(owner, self.pending_annotation.as_ref());
                for (param, ty) in anno_scope {
                    // An argument's value can only say so much: an
                    // `Option::Some(String::from_str(..))` built in
                    // argument position carries no type arguments of
                    // its own, so `Box::new` read `T = Option` and its
                    // body could not size it. The annotation names the
                    // whole type; it wins over an argument that did not.
                    match generic_scope.get(&param) {
                        Some(from_arg) if !self.type_is_incomplete(from_arg) => {}
                        _ => {
                            generic_scope.insert(param, ty);
                        }
                    }
                }
            }
            self.fill_scope_from_caller(&mut generic_scope, &method.generic_params);
        }
        self.push_generic_type_scope(generic_scope);

        // Same contract evaluation flow as `call_method`. Associated functions
        // have no `self`, but `requires` / `ensures` predicates may still
        // reference the named parameters and `result`.
        if let Err(e) = self.evaluate_requires_clauses(method.name, &method.requires, &method.parameter) {
            self.pop_generic_type_scope();
            self.environment.exit_block();
            return Err(e);
        }
        if let Err(e) = self.evaluate_old_snapshots(&method.old_exprs) {
            self.pop_generic_type_scope();
            self.environment.exit_block();
            return Err(e);
        }

        let result = self.evaluate_method(&method);

        self.pop_generic_type_scope();
        let result = match result {
            Ok(EvaluationResult::Value(v)) => {
                if let Err(e) = self.evaluate_ensures_clauses(method.name, &method.ensures, &method.ensures_kinds, v.clone_to_rc(), &method.parameter) {
                    self.environment.exit_block();
                    return Err(e);
                }
                Ok(EvaluationResult::Value(v))
            }
            Ok(EvaluationResult::Return(v)) => {
                let ret = v.clone().map(|val| val.into_rc()).unwrap_or_else(|| Rc::new(RefCell::new(Object::Unit)));
                if let Err(e) = self.evaluate_ensures_clauses(method.name, &method.ensures, &method.ensures_kinds, ret, &method.parameter) {
                    self.environment.exit_block();
                    return Err(e);
                }
                // Same Return → Value boundary conversion as
                // call_method above — see that comment for the
                // full rationale (DICT-RETURN-WHILE follow-up).
                Ok(EvaluationResult::Value(v.unwrap_or(crate::value::Value::Unit)))
            }
            other => other,
        };

        // Clean up scope
        self.environment.exit_block();
        if result.is_ok() {
            self.pop_frame();
        }

        result
    }

    fn evaluate_method(&mut self, method: &MethodFunction) -> Result<EvaluationResult, InterpreterError> {
        // Get the method body from the statement pool
        let stmt = self.stmt_pool.get(&method.code)
            .ok_or_else(|| InterpreterError::InternalError("Invalid method code reference".to_string()))?;

        // Execute the method body, with the declared return type
        // standing in as the pending annotation
        // (TREE-WALKER-SELF-TYPE-ARG).
        let return_type = method.return_type.clone();
        match stmt {
            frontend::ast::Stmt::Expression(expr_ref) => {
                self.with_return_annotation(return_type.as_ref(), |ctx| {
                    if let Some(Expr::Block(statements)) = ctx.expr_pool.get(&expr_ref) {
                        ctx.evaluate_block(&statements)
                    } else {
                        // Single expression method body
                        ctx.evaluate(&expr_ref)
                    }
                })
            }
            _ => Err(InterpreterError::InternalError(format!("evaluate_method: unexpected method body type: {stmt:?}")))
        }
    }

    /// Evaluates function calls
    pub(super) fn evaluate_function_call(&mut self, name: &DefaultSymbol, args: &ExprRef) -> Result<EvaluationResult, InterpreterError> {
        // Captured before `args` is shadowed by the destructured
        // `Expr::ExprList` below; used for the panic backtrace.
        let call_site = self.expr_location(args);
        // Lexical scoping: a local binding holding a closure shadows a
        // top-level function of the same name. Mirrors the resolution
        // order in the type checker's `visit_call` -- consulting the
        // function table first would let a user-defined `fn f(..)`
        // hijack stdlib call sites such as the `f(v)` in
        // `core/std/option.t::map`, which call a closure parameter.
        // Non-closure bindings do not shadow, so ordinary calls keep
        // resolving to the function table.
        if let Some(callee_val) = self.environment.get_val(*name) {
            let callee_rc = callee_val.into_rc();
            let is_closure = matches!(&*callee_rc.borrow(), Object::Closure { .. });
            if is_closure {
                return self.evaluate_indirect_call(callee_rc, name, args);
            }
        }

        // Bare-name resolution: prefer the user-authored
        // `(None, name)` slot so a user `fn add(Point, Point)`
        // wins over an auto-loaded stdlib `pub fn add(u64, u64)`
        // (#193b). Falls back to the legacy flat `function` map
        // when the qualified table isn't populated (e.g. tests
        // that built the context with the older constructor).
        let resolved = self
            .lookup_function_qualified(None, *name)
            .or_else(|| self.function.get::<DefaultSymbol>(name).cloned());
        if let Some(func) = resolved {
            let args = self.expr_pool.get(args)
                .ok_or_else(|| InterpreterError::InternalError("Invalid arguments reference".to_string()))?;
            match args {
                Expr::ExprList(args) => {
                    if args.len() != func.parameter.len() {
                        return Err(
                            InterpreterError::FunctionParameterMismatch {
                                message: format!("evaluate_function: bad function parameter length: {:?}", args.len()),
                                expected: func.parameter.len(),
                                found: args.len()
                            }
                        );
                    }

                    // Evaluate arguments once and perform type checking. Phase 5:
                    // collect as `Value` rather than `RcObject` so primitive
                    // arguments stay inline through the call boundary.
                    use crate::try_value_v;
                    let mut evaluated_args: Vec<crate::value::Value> = Vec::new();
                    let is_generic_function = !func.generic_params.is_empty();
                    // REF-Stage-2 (i): track which positional args are
                    // an explicit `&mut <name>` borrow expression so
                    // we can write the post-body parameter value back
                    // to the caller's binding once the call returns.
                    // Other arg shapes contribute `None` (no
                    // writeback target).
                    let mut writeback_targets: Vec<WritebackTarget> = Vec::with_capacity(args.len());

                    for (i, (arg_expr, (_param_name, expected_type))) in args.iter().zip(func.parameter.iter()).enumerate() {
                        // Detect `&mut <lvalue>` arg shape before
                        // evaluating, so the caller-side target is
                        // captured even when the argument expression
                        // itself mutates other state during evaluation.
                        let mut target = WritebackTarget::None;
                        if let Some(Expr::Unary(op, inner)) = self.expr_pool.get(arg_expr) {
                            if matches!(op, UnaryOp::BorrowMut) {
                                target = self.classify_writeback_target(&inner)?;
                            }
                        }
                        writeback_targets.push(target);

                        let arg_result = self.evaluate(arg_expr);
                        let arg_value = try_value_v!(arg_result);
                        let actual_type = arg_value.get_type();

                        // Skip type checking for generic functions since type checking was already done.
                        // REF-Stage-2: references are erased at runtime, so peel the
                        // expected `&T` / `&mut T` to `T` before comparing — the static
                        // type checker already enforced the call-site mut/borrow rules
                        // (REF-Stage-2 (f) requires explicit `&mut <var>` for `&mut T`
                        // parameters), so this runtime check is purely defence-in-depth
                        // against the inner value type.
                        // A5: `dyn Trait` is a static-only abstraction — the type
                        // checker has already verified that the concrete argument
                        // type implements the trait. At runtime the value is just
                        // the underlying Object so a structural `is_equivalent`
                        // against `Dyn(...)` always returns false. Skip the check
                        // when the (deref'd) expected type is a trait object.
                        let expected_runtime = expected_type.deref_ref();
                        let expected_is_dyn = matches!(expected_runtime, frontend::type_decl::TypeDecl::Dyn(_));
                        if !is_generic_function && !expected_is_dyn && !actual_type.is_equivalent(expected_runtime) {
                            let func_name = self.string_interner.resolve(*name).unwrap_or("<unknown>");
                            return Err(InterpreterError::TypeError {
                                expected: expected_type.clone(),
                                found: actual_type,
                                message: format!("Function '{}' argument {} type mismatch", func_name, i + 1)
                            });
                        }

                        evaluated_args.push(arg_value);
                    }

                    // LLM-LOOP P6: record the frame for panic backtraces.
                    // Left in place on the error path on purpose — a
                    // panic unwinds to the top and the stack captured at
                    // the failure is exactly what the report needs.
                    let fn_name = self
                        .string_interner
                        .resolve(*name)
                        .unwrap_or("<unknown>")
                        .to_string();
                    self.push_frame(fn_name, call_site);

                    // Call function with pre-evaluated arguments and collect
                    // post-body `&mut T` parameter values.
                    let (ret_val, writebacks) = self
                        .evaluate_function_with_values_writeback(func, &evaluated_args)?;
                    self.pop_frame();

                    // REF-Stage-2 (i)+(iii): apply writebacks. Each
                    // entry pairs the caller-side target (identifier
                    // or struct field) with the post-body value of
                    // the corresponding `&mut T` parameter. The type
                    // checker has already enforced that the root
                    // binding is `var` (so `set_var` w/ Overwrite
                    // succeeds, and field assignment via `borrow_mut`
                    // mirrors the semantics of `obj.field = x` in
                    // user code).
                    for (target, modified) in writeback_targets.iter().zip(writebacks.iter()) {
                        if let Some(val) = modified {
                            self.apply_writeback(target, val.clone())?;
                        }
                    }

                    Ok(EvaluationResult::Value(ret_val))
                }
                _ => Err(InterpreterError::InternalError("evaluate_function: expected ExprList".to_string())),
            }
        } else {
            // Closures Phase 3: indirect call. When the bare name
            // doesn't resolve to a fn decl but does resolve to a
            // local variable holding `Object::Closure`, dispatch
            // through the closure body. Mirrors the type-checker
            // fallback in `visit_call`.
            if let Some(callee_val) = self.environment.get_val(*name) {
                let callee_rc = callee_val.into_rc();
                let is_closure = matches!(&*callee_rc.borrow(), Object::Closure { .. });
                if is_closure {
                    return self.evaluate_indirect_call(callee_rc, name, args);
                }
            }
            let name = self.string_interner.resolve(*name).unwrap_or("<NOT_FOUND>");
            Err(InterpreterError::FunctionNotFound(name.to_string()))
        }
    }

    /// Closures Phase 3: dispatch through an `Object::Closure` value
    /// at the given binding. Evaluates each argument expression
    /// against the caller's environment, then opens a fresh block
    /// scope, binds captures + params, and evaluates the body.
    /// Mirrors the values-based call path used by `evaluate_function`
    /// without the writeback / contract / generic-monomorphisation
    /// machinery — closures don't carry contract clauses or generic
    /// params (Phase 2 reject), so the simpler shape suffices.
    /// Closures Phase 8: dispatch a `obj.field(args)` call when
    /// the field's value is a closure. Same shape as
    /// `evaluate_indirect_call` but takes the args as a slice
    /// (method-call ABI) instead of an `ExprList` ref.
    fn evaluate_field_closure_call(
        &mut self,
        callee: RcObject,
        field_name: DefaultSymbol,
        args: &[ExprRef],
    ) -> Result<EvaluationResult, InterpreterError> {
        let (params, body, captures, shared_scope) = {
            let borrowed = callee.borrow();
            match &*borrowed {
                Object::Closure { params, body, captures, shared_scope, .. } => (
                    params.clone(),
                    *body,
                    captures.clone(),
                    *shared_scope,
                ),
                _ => return Err(InterpreterError::InternalError(
                    "evaluate_field_closure_call: callee was not a closure".to_string(),
                )),
            }
        };
        if args.len() != params.len() {
            let name_str = self
                .string_interner
                .resolve(field_name)
                .unwrap_or("<closure-field>");
            return Err(InterpreterError::FunctionParameterMismatch {
                message: format!(
                    "closure field '{}' arg count mismatch: expected {}, found {}",
                    name_str,
                    params.len(),
                    args.len()
                ),
                expected: params.len(),
                found: args.len(),
            });
        }
        let mut evaluated: Vec<crate::value::Value> = Vec::with_capacity(args.len());
        for arg in args {
            let v = self.evaluate(arg)?;
            let v = match v {
                EvaluationResult::Value(v) => v,
                _ => {
                    return Err(InterpreterError::InternalError(
                        "field closure argument produced control-flow value".to_string(),
                    ));
                }
            };
            evaluated.push(v);
        }
        // A closure reached through a struct field has been stored,
        // so it is one that copies (CLOSURE-CAPTURE E3) — the match
        // is here so the two dispatch paths stay the same shape.
        let hidden = match shared_scope {
            Some(depth) => self.environment.detach_scopes_above(depth),
            None => Vec::new(),
        };
        self.environment.enter_block();
        for (name, val) in &captures {
            self.environment
                .set_val(*name, crate::value::Value::from_rc(val));
        }
        for ((param_sym, _), arg_val) in params.iter().zip(evaluated) {
            self.environment.set_val(*param_sym, arg_val);
        }
        let body_expr = self.expr_pool.get(&body).ok_or_else(|| {
            InterpreterError::InternalError("closure body ExprRef not in pool".to_string())
        })?;
        // DEBUG-OBS D1, as in `evaluate_indirect_call`. The frame is
        // the field the closure was reached through.
        let frame = self
            .string_interner
            .resolve(field_name)
            .unwrap_or("<closure-field>")
            .to_string();
        self.push_frame(frame, args.first().and_then(|a| self.expr_location(a)));
        let result = match body_expr {
            Expr::Block(stmts) => self.evaluate_block(&stmts),
            _ => self.evaluate(&body),
        };
        self.environment.exit_block();
        self.environment.restore_scopes(hidden);
        if result.is_ok() {
            self.pop_frame();
        }
        match result {
            Ok(EvaluationResult::Value(v)) => Ok(EvaluationResult::Value(v)),
            Ok(EvaluationResult::Return(v)) => {
                Ok(EvaluationResult::Value(v.unwrap_or(crate::value::Value::Unit)))
            }
            Ok(other) => Ok(other),
            Err(e) => Err(e),
        }
    }

    fn evaluate_indirect_call(
        &mut self,
        callee: RcObject,
        callee_name: &DefaultSymbol,
        args: &ExprRef,
    ) -> Result<EvaluationResult, InterpreterError> {
        let call_site = self.expr_location(args);
        let args_list = match self.expr_pool.get(args) {
            Some(Expr::ExprList(args)) => args,
            _ => return Err(InterpreterError::InternalError(
                "evaluate_indirect_call: expected ExprList".to_string(),
            )),
        };
        // Snapshot closure parts under a borrow + drop pattern so we
        // can mutate the environment afterwards without reborrowing.
        let (params, body, captures, shared_scope) = {
            let borrowed = callee.borrow();
            match &*borrowed {
                Object::Closure { params, body, captures, shared_scope, .. } => (
                    params.clone(),
                    *body,
                    captures.clone(),
                    *shared_scope,
                ),
                _ => return Err(InterpreterError::InternalError(
                    "evaluate_indirect_call: callee was not a closure".to_string(),
                )),
            }
        };
        if args_list.len() != params.len() {
            let name_str = self
                .string_interner
                .resolve(*callee_name)
                .unwrap_or("<closure>");
            return Err(InterpreterError::FunctionParameterMismatch {
                message: format!(
                    "closure '{}' arg count mismatch: expected {}, found {}",
                    name_str,
                    params.len(),
                    args_list.len()
                ),
                expected: params.len(),
                found: args_list.len(),
            });
        }
        // Evaluate args in caller scope FIRST — they may reference
        // bindings that aren't in the closure's capture set.
        let mut evaluated: Vec<crate::value::Value> = Vec::with_capacity(args_list.len());
        for arg in &args_list {
            let v = self.evaluate(arg)?;
            let v = match v {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(_)
                | EvaluationResult::Break(_)
                | EvaluationResult::Continue(_)
                | EvaluationResult::None => {
                    return Err(InterpreterError::InternalError(
                        "closure argument produced control-flow value".to_string(),
                    ));
                }
            };
            evaluated.push(v);
        }
        // CLOSURE-CAPTURE E3: a sharing closure runs in the scopes
        // that were open where it was written, so anything the caller
        // opened since is set aside for the call. Otherwise a name
        // shadowed at the *call* site would reach into the body,
        // which is dynamic scoping — a closure over `n` called inside
        // a block declaring its own `n` answered with the block's.
        // `captures` is empty for these, so the loop below is a no-op.
        let hidden = match shared_scope {
            Some(depth) => self.environment.detach_scopes_above(depth),
            None => Vec::new(),
        };
        // Open a fresh scope and bind captures + params. Args take
        // precedence (a param shadowing a captured name is fine) —
        // params are inserted last so they win on lookup.
        self.environment.enter_block();
        for (name, val) in &captures {
            self.environment
                .set_val(*name, crate::value::Value::from_rc(val));
        }
        for ((param_sym, _), arg_val) in params.iter().zip(evaluated) {
            self.environment.set_val(*param_sym, arg_val);
        }
        // Evaluate the body. Body is a block ExprRef per the parser
        // (`parse_closure_expr` always uses `parse_block`), so this
        // exercises the standard block evaluator.
        let body_expr = self.expr_pool.get(&body).ok_or_else(|| {
            InterpreterError::InternalError("closure body ExprRef not in pool".to_string())
        })?;
        // DEBUG-OBS D1: a closure frame is named by the binding the
        // user called through — `f(x)` reads as `f` in the backtrace
        // whether `f` is a fn or a closure, which is the distinction
        // the reader does *not* need at that moment.
        let frame = self
            .string_interner
            .resolve(*callee_name)
            .unwrap_or("<closure>")
            .to_string();
        self.push_frame(frame, call_site);
        let result = match body_expr {
            Expr::Block(stmts) => self.evaluate_block(&stmts),
            _ => self.evaluate(&body),
        };
        self.environment.exit_block();
        self.environment.restore_scopes(hidden);
        if result.is_ok() {
            self.pop_frame();
        }
        // Convert a Return result back into a plain Value at the
        // closure boundary — the body's `return` shouldn't leak
        // into the caller's control flow.
        match result {
            Ok(EvaluationResult::Value(v)) => Ok(EvaluationResult::Value(v)),
            Ok(EvaluationResult::Return(v)) => {
                Ok(EvaluationResult::Value(v.unwrap_or(crate::value::Value::Unit)))
            }
            Ok(other) => Ok(other),
            Err(e) => Err(e),
        }
    }

    /// Evaluates field access expressions
    pub(super) fn evaluate_field_access(&mut self, obj: &ExprRef, field: &DefaultSymbol) -> Result<EvaluationResult, InterpreterError> {
        // First check if this is a module qualified name (e.g., math.add)
        if let Some(Expr::Identifier(module_name)) = self.expr_pool.get(obj) {
            if let Some(module_value) = self.resolve_module_qualified_name(module_name, *field) {
                return Ok(EvaluationResult::Value(module_value.into()));
            }
        }

        // If not a module qualified name, evaluate as struct field access
        let obj_val = self.evaluate(obj)?;
        let obj_val = try_value!(Ok(obj_val));
        let obj_borrowed = obj_val.borrow();

        match &*obj_borrowed {
            // RANGE-FOR: a range value's bounds. The checker admits
            // only these two names.
            Object::Range { start, end } => {
                match self.string_interner.resolve(*field) {
                    Some("start") => Ok(EvaluationResult::Value(start.clone().into())),
                    Some("end") => Ok(EvaluationResult::Value(end.clone().into())),
                    other => Err(InterpreterError::InternalError(format!(
                        "Field '{}' not found on a range",
                        other.unwrap_or("<unknown>")
                    ))),
                }
            }
            Object::Struct { type_name, fields, .. } => {
                if let Some(rc) = fields.get(field) {
                    return Ok(EvaluationResult::Value(rc.clone().into()));
                }
                // DATA-ORIENTED Phase 1: the heap column window.
                // `vs.mass` on a `SoaVec` is every element's `mass`,
                // reached only after the vec's own fields have been
                // ruled out (so `vs.len` still means the field).
                if self.string_interner.resolve(*type_name) == Some("SoaVec") {
                    let window = self.build_column_view(obj_val.clone(), *field);
                    return Ok(EvaluationResult::Value(window.into()));
                }
                let field_name = self.string_interner.resolve(*field).unwrap_or("<unknown>");
                Err(InterpreterError::InternalError(format!(
                    "Field '{field_name}' not found"
                )))
            }
            // DATA-ORIENTED Phase 1: a field name on an array is the
            // column window, `ps.mass`. The compiled lanes hand back a
            // `Column<T>` over the column's address; this engine has
            // no addresses for an array (it holds one as a `Vec` of
            // element values), so the window is the array itself plus
            // the field to look at. Sharing the array's `Rc` is what
            // makes it a *view*: a later `ps[0].mass = ...` is seen
            // through it, as it is on every other lane.
            //
            // The result is spelled as an ordinary `Column` struct so
            // it prints, compares and passes like the type the checker
            // says it is; `evaluate_method_call` intercepts the four
            // methods before they can reach the stdlib bodies, which
            // read an address this shape does not have.
            Object::Array(_) => {
                let window = self.build_column_view(obj_val.clone(), *field);
                Ok(EvaluationResult::Value(window.into()))
            }
            _ => Err(InterpreterError::InternalError(format!("Cannot access field on non-struct object: {obj_borrowed:?}")))
        }
    }

    /// Evaluates method call expressions
    pub(super) fn evaluate_method_call(&mut self, obj: &ExprRef, method: &DefaultSymbol, args: &[ExprRef]) -> Result<EvaluationResult, InterpreterError> {
        // DEBUG-OBS D1: the receiver's position is the call's position
        // — `s.boom()` puts both on one line — and it is the only
        // location this layer is handed.
        let call_site = self.expr_location(obj);
        let obj_val = self.evaluate(obj)?;
        let obj_val = try_value!(Ok(obj_val));
        let obj_borrowed = obj_val.borrow();
        let method_name = self.string_interner.resolve(*method).unwrap_or("<unknown>");

        // Handle universal is_null() method first
        if method_name == "is_null" {
            if !args.is_empty() {
                return Err(InterpreterError::InternalError(format!(
                    "is_null() method takes no arguments, but {} provided",
                    args.len()
                )));
            }
            let is_null = obj_borrowed.is_null();
            return Ok(EvaluationResult::Value((Object::Bool(is_null)).into()));
        }

        // DATA-ORIENTED Phase 1: the column window's four methods,
        // answered before the registry can route them to the stdlib
        // bodies in `core/std/column.t`. Those read `self.addr`, which
        // is the compiled lanes' representation; here a window is the
        // array plus a field name (see `evaluate_field_access`), so
        // the same call has to be answered by indexing that array.
        let column_window = match &*obj_borrowed {
            Object::Struct { type_name, fields, .. }
                if self.string_interner.resolve(*type_name) == Some("Column") =>
            {
                super::column::column_source(self.string_interner, fields)
            }
            _ => None,
        };
        if let Some((array, field)) = column_window {
            // `method_name` borrows the interner, which the argument
            // evaluation below needs mutably.
            let called = method_name.to_string();
            drop(obj_borrowed);
            let mut arg_values = Vec::new();
            for arg in args {
                let arg_val = self.evaluate(arg)?;
                let arg_val = try_value!(Ok(arg_val));
                arg_values.push(arg_val);
            }
            return self.column_method(&called, array, field, &arg_values);
        }

        // Step B of extension-trait support: dispatch through the
        // user-registered method registry first when the receiver is
        // a primitive. Mirrors what `Object::Struct { type_name, .. }`
        // already does — looks `(target_symbol, method_name)` up in
        // `method_registry` and, on hit, evaluates args and calls the
        // method body with `self` as the first parameter.
        //
        // This runs *before* the hardcoded `Object::Int64`/`Float64`
        // arms below, so a user `impl Foo for i64 { fn abs(self) -> i64 { ... } }`
        // takes precedence over the legacy `BuiltinMethod::I64Abs`
        // path. Steps E + F migrate the legacy methods onto extension
        // traits and remove the hardcoded arms entirely.
        if let Some(target_sym) = primitive_target_symbol(&obj_borrowed, self.string_interner) {
            // Primitive receivers have no type args; pass empty
            // slice. CONCRETE-IMPL Phase 2: any future
            // `impl Foo for u8` etc. always registers with empty
            // target_type_args, so the empty-args lookup is
            // exhaustive for the primitive path.
            if let Some(method_func) = self.get_method(target_sym, *method, &[]) {
                drop(obj_borrowed);
                let mut arg_values = Vec::new();
                for arg in args {
                    let arg_val = self.evaluate(arg)?;
                    let arg_val = try_value!(Ok(arg_val));
                    arg_values.push(arg_val);
                }
                return self.call_method(method_func, obj_val, arg_values, call_site);
            }
        }

        match &*obj_borrowed {
            Object::ConstString(_) | Object::String(_) => {
                // Handle built-in String methods
                match method_name {
                    "len" => {
                        // String.len() method - no arguments required, returns u64
                        if !args.is_empty() {
                            return Err(InterpreterError::InternalError(format!(
                                "String.len() method takes no arguments, but {} provided",
                                args.len()
                            )));
                        }

                        // Get the actual string value regardless of internal representation
                        let string_value = obj_borrowed.to_string_value(self.string_interner);
                        let len = string_value.len() as u64;

                        Ok(EvaluationResult::Value((Object::UInt64(len)).into()))
                    }
                    "contains" => {
                        if args.len() != 1 {
                            return Err(InterpreterError::InternalError(format!(
                                "String.contains() method takes 1 argument, but {} provided",
                                args.len()
                            )));
                        }

                        let string_value = obj_borrowed.to_string_value(self.string_interner);

                        let arg_value = self.evaluate(&args[0])?;
                        let arg_obj = try_value!(Ok(arg_value));
                        let arg_borrowed = arg_obj.borrow();
                        let arg_string = arg_borrowed.to_string_value(self.string_interner);

                        let contains = string_value.contains(&arg_string);
                        Ok(EvaluationResult::Value((Object::Bool(contains)).into()))
                    }
                    "concat" => {
                        if args.len() != 1 {
                            return Err(InterpreterError::InternalError(format!(
                                "String.concat() method takes 1 argument, but {} provided",
                                args.len()
                            )));
                        }

                        let string_value = obj_borrowed.to_string_value(self.string_interner);

                        let arg_value = self.evaluate(&args[0])?;
                        let arg_obj = try_value!(Ok(arg_value));
                        let arg_borrowed = arg_obj.borrow();
                        let arg_string = arg_borrowed.to_string_value(self.string_interner);

                        let concatenated = format!("{}{}", string_value, arg_string);
                        // Return as dynamic String, not interned - this is the key improvement
                        Ok(EvaluationResult::Value((Object::String(concatenated)).into()))
                    }
                    "substring" => {
                        if args.len() != 2 {
                            return Err(InterpreterError::InternalError(format!(
                                "String.substring() method takes 2 arguments, but {} provided",
                                args.len()
                            )));
                        }

                        let string_value = obj_borrowed.to_string_value(self.string_interner);

                        let start_value = self.evaluate(&args[0])?;
                        let start_obj = try_value!(Ok(start_value));
                        let start = start_obj.borrow().try_unwrap_uint64().map_err(InterpreterError::ObjectError)? as usize;

                        let end_value = self.evaluate(&args[1])?;
                        let end_obj = try_value!(Ok(end_value));
                        let end = end_obj.borrow().try_unwrap_uint64().map_err(InterpreterError::ObjectError)? as usize;

                        if start >= string_value.len() || end > string_value.len() || start > end {
                            return Err(InterpreterError::InternalError(format!(
                                "Invalid substring indices: start={start}, end={end}, len={}",
                                string_value.len()
                            )));
                        }

                        let substring = string_value[start..end].to_string();
                        Ok(EvaluationResult::Value((Object::String(substring)).into()))
                    }
                    "split" => {
                        if args.len() != 1 {
                            return Err(InterpreterError::InternalError(format!(
                                "String.split() method takes 1 argument, but {} provided",
                                args.len()
                            )));
                        }

                        let string_value = obj_borrowed.to_string_value(self.string_interner);

                        let sep_value = self.evaluate(&args[0])?;
                        let sep_obj = try_value!(Ok(sep_value));
                        let sep_borrowed = sep_obj.borrow();
                        let separator = sep_borrowed.to_string_value(self.string_interner);

                        let parts: Vec<_> = string_value.split(&separator)
                            .map(|part| Rc::new(RefCell::new(Object::String(part.to_string()))))
                            .collect();

                        Ok(EvaluationResult::Value(Object::Array(Box::new(parts)).into()))
                    }
                    "trim" => {
                        if !args.is_empty() {
                            return Err(InterpreterError::InternalError(format!(
                                "String.trim() method takes no arguments, but {} provided",
                                args.len()
                            )));
                        }

                        let string_value = obj_borrowed.to_string_value(self.string_interner);
                        let trimmed = string_value.trim().to_string();
                        // Return as dynamic String, not interned
                        Ok(EvaluationResult::Value((Object::String(trimmed)).into()))
                    }
                    "to_upper" => {
                        if !args.is_empty() {
                            return Err(InterpreterError::InternalError(format!(
                                "String.to_upper() method takes no arguments, but {} provided",
                                args.len()
                            )));
                        }

                        let string_value = obj_borrowed.to_string_value(self.string_interner);
                        let upper = string_value.to_uppercase();
                        // Return as dynamic String, not interned
                        Ok(EvaluationResult::Value((Object::String(upper)).into()))
                    }
                    "to_lower" => {
                        if !args.is_empty() {
                            return Err(InterpreterError::InternalError(format!(
                                "String.to_lower() method takes no arguments, but {} provided",
                                args.len()
                            )));
                        }

                        let string_value = obj_borrowed.to_string_value(self.string_interner);
                        let lower = string_value.to_lowercase();
                        // Return as dynamic String, not interned
                        Ok(EvaluationResult::Value((Object::String(lower)).into()))
                    }
                    _ => {
                        Err(InterpreterError::InternalError(format!(
                            "Method '{method_name}' not found for String type"
                        )))
                    }
                }
            }
            Object::Array(elements) => {
                // Handle built-in Array methods
                match method_name {
                    "len" => {
                        // Array.len() method - no arguments required, returns u64
                        if !args.is_empty() {
                            return Err(InterpreterError::InternalError(format!(
                                "Array.len() method takes no arguments, but {} provided",
                                args.len()
                            )));
                        }
                        let len = elements.len() as u64;
                        Ok(EvaluationResult::Value((Object::UInt64(len)).into()))
                    }
                    _ => {
                        Err(InterpreterError::InternalError(format!(
                            "Method '{method_name}' not found for Array type"
                        )))
                    }
                }
            }
            // NOTE: hardcoded `Object::Int64.abs()` /
            // `Object::Float64.{abs,sqrt}` arms lived here before
            // Step F. The Step B primitive-receiver dispatch path
            // earlier in this function intercepts these calls and
            // routes through the prelude's extension-trait impls
            // (`impl Abs for i64 { fn abs(self) -> i64 { ... } }`
            // / `impl Sqrt for f64 { ... }`). The arms below are
            // unreachable for `abs` / `sqrt` now; they only fire
            // when a user calls some unknown method on a
            // primitive, which produces the same "method not
            // found" diagnostic as before.
            Object::Int64(_) => Err(InterpreterError::InternalError(format!(
                "Method '{method_name}' not found for i64"
            ))),
            Object::Float64(_) => Err(InterpreterError::InternalError(format!(
                "Method '{method_name}' not found for f64"
            ))),
            Object::Struct { type_name, type_args, .. } => {
                let struct_name_symbol = *type_name;
                // CONCRETE-IMPL Phase 2: dispatch picks the impl
                // matching the receiver's concrete type args
                // (e.g. `impl FromStr for Vec<u8>` for a `Vec<u8>`),
                // falling back to a generic-parameterised impl with
                // empty target_type_args.
                let receiver_type_args = type_args.clone();

                if let Some(method_func) = self.get_method(struct_name_symbol, *method, &receiver_type_args) {
                    drop(obj_borrowed); // Release borrow before method call

                    // Evaluate method arguments
                    let mut arg_values = Vec::new();
                    for arg in args {
                        let arg_val = self.evaluate(arg)?;
                        let arg_val = try_value!(Ok(arg_val));
                        arg_values.push(arg_val);
                    }

                    // Call method with self as first argument
                    self.call_method(method_func, obj_val, arg_values, call_site)
                } else {
                    // Closures Phase 8: when no method matches,
                    // try the field-call fallback. If the struct
                    // has a field whose name matches and whose
                    // value is a closure (`Object::Closure`),
                    // dispatch through the indirect-call path —
                    // the same one a `val f = fn(...); f(x)`
                    // call would take.
                    if let Object::Struct { fields, .. } = &*obj_borrowed {
                        if let Some(field_rc) = fields.get(method).cloned() {
                            let is_closure = matches!(
                                &*field_rc.borrow(),
                                Object::Closure { .. }
                            );
                            if is_closure {
                                drop(obj_borrowed);
                                return self.evaluate_field_closure_call(
                                    field_rc, *method, args,
                                );
                            }
                        }
                    }
                    Err(InterpreterError::InternalError(format!("Method '{method_name}' not found for struct '{type_name:?}'")))
                }
            }
            Object::EnumVariant { enum_name, type_args, .. } => {
                // Enum receivers reuse the same `(target_symbol,
                // method_name)` `method_registry` lookup the struct
                // path uses; `impl<T> Option<T> { fn unwrap_or(...) }`
                // registers under the enum's name symbol. Mirrors how
                // primitive extension-trait dispatch piggy-backs on
                // the same registry above. CONCRETE-IMPL Phase 2:
                // pass enum's runtime type_args so any future
                // `impl Foo for Option<u8>` would dispatch correctly;
                // generic `impl<T> Option<T>` falls through with
                // empty target_type_args.
                let enum_name_symbol = *enum_name;
                let receiver_type_args = type_args.clone();
                if let Some(method_func) = self.get_method(enum_name_symbol, *method, &receiver_type_args) {
                    drop(obj_borrowed);
                    let mut arg_values = Vec::new();
                    for arg in args {
                        let arg_val = self.evaluate(arg)?;
                        let arg_val = try_value!(Ok(arg_val));
                        arg_values.push(arg_val);
                    }
                    self.call_method(method_func, obj_val, arg_values, call_site)
                } else {
                    Err(InterpreterError::InternalError(format!(
                        "Method '{method_name}' not found for enum '{enum_name:?}'"
                    )))
                }
            }
            _ => {
                Err(InterpreterError::InternalError(format!("Cannot call method '{method_name}' on non-struct object: {obj_borrowed:?}")))
            }
        }
    }

    /// Evaluates struct literal expressions
    pub(super) fn evaluate_struct_literal(&mut self, struct_name: &DefaultSymbol, fields: &[(DefaultSymbol, ExprRef)]) -> Result<EvaluationResult, InterpreterError> {
        // Create a struct instance. Field keys flow through unchanged as
        // interned `DefaultSymbol`s — there is no need to resolve to a
        // textual name during construction.
        let mut field_values: HashMap<DefaultSymbol, RcObject> = HashMap::new();

        for (field_name, field_expr) in fields {
            // Handle null expressions specially in struct literals
            let expr = self.expr_pool.get(field_expr)
                .ok_or_else(|| InterpreterError::InternalError(format!("Unbound error: {:?}", field_expr)))?;

            let field_value = match expr {
                Expr::Null => {
                    // Use pre-created null object for struct fields
                    self.null_object.clone()
                }
                _ => {
                    let field_value = self.evaluate(field_expr)?;
                    try_value!(Ok(field_value))
                }
            };

            field_values.insert(*field_name, field_value);
        }

        // MEMORY-ACCESS M5: a field's declared type is its value's
        // annotation. `H { nodes: Vec::new() }` built the `Vec` with no
        // type to learn `T` from, so the value carried `T` unbound and
        // anything later asking `sizeof::<T>()` of it failed -- which
        // `val v: Vec<u64> = Vec::new()` never did, because a `val`
        // stamps its annotation onto its value. The struct's own
        // parameters in a declared type (`v: Vec<T>` in a `Bag<T>`) are
        // what the literal is being built as: the `val` annotation it
        // is bound by (`var b: Bag<Key> = Bag { v: Vec::new() }`), else
        // the generic scope it is evaluated in (`impl<T> Bag<T>`). A
        // declared type still naming an unknown parameter is not
        // stamped -- it says nothing about the value yet.
        let mut annotated: HashMapStd<DefaultSymbol, TypeDecl> = HashMapStd::new();
        if let Some(entry) = self.struct_definitions.get(struct_name).cloned() {
            if let Some(TypeDecl::Struct(name, args) | TypeDecl::Enum(name, args)) =
                self.pending_annotation.as_ref()
            {
                if name == struct_name && args.len() == entry.generic_params.len() {
                    for (p, a) in entry.generic_params.iter().zip(args) {
                        annotated.insert(*p, a.clone());
                    }
                }
            }
            let mut params = annotated.clone();
            let scope = self.merged_generic_scope();
            for p in &entry.generic_params {
                if params.contains_key(p) {
                    continue;
                }
                if let Some(bound) = scope.get(p) {
                    if !mentions_any(bound, &entry.generic_params) && !matches!(bound, TypeDecl::Unknown) {
                        params.insert(*p, bound.clone());
                    }
                }
            }
            for (name, declared) in &entry.fields {
                let declared = substitute_params(declared, &params);
                if mentions_any(&declared, &entry.generic_params) {
                    continue;
                }
                if let Some(value) = field_values.get(name) {
                    let _ = super::statement::apply_annotation_type_args(
                        crate::value::Value::from(value.clone()),
                        Some(&declared),
                    );
                }
            }
        }

        let active_scope = self.merged_generic_scope();
        let mut type_args = self
            .struct_definitions
            .get(struct_name)
            .map(|entry| derive_struct_type_args(entry, &field_values, &active_scope))
            .unwrap_or_default();
        // The binding's annotation outranks the scope for a phantom
        // parameter. The scope is every active call's, not the
        // literal's own: `val bytes: Ptr<u8> = Ptr { addr: .. }` inside
        // `String::push`, reached from `Vec<T>::clone` with `T` =
        // `String`, otherwise tagged the window `Ptr<String>`.
        // An annotation that names a parameter itself (`val p: Ptr<T>`
        // inside `impl<T> Vec<T>`) is read through the scope first,
        // and left alone if that does not make it concrete.
        if let Some(entry) = self.struct_definitions.get(struct_name) {
            for (slot, p) in type_args.iter_mut().zip(&entry.generic_params) {
                if let Some(a) = annotated.get(p) {
                    let resolved = substitute_params(a, &active_scope);
                    if !mentions_any(&resolved, &entry.generic_params)
                        && !mentions_any(&resolved, &active_scope.keys().copied().collect::<Vec<_>>())
                        && !matches!(resolved, TypeDecl::Unknown)
                    {
                        *slot = resolved;
                    }
                }
            }
        }
        let struct_obj = Object::Struct {
            type_name: *struct_name,
            fields: Box::new(field_values),
            type_args,
        };

        Ok(EvaluationResult::Value((struct_obj).into()))
    }

    /// Evaluates associated function calls (like Container::new)
    pub(super) fn evaluate_associated_function_call(&mut self, struct_name: &DefaultSymbol, function_name: &DefaultSymbol, args: &[ExprRef]) -> Result<EvaluationResult, InterpreterError> {
        // DEBUG-OBS D1: `S::make(x)` has no receiver expression, so the
        // first argument's position is the closest thing to the call's
        // own; an argument-less call keeps `None` (D2/D3 give this
        // layer a real site).
        let call_site = args.first().and_then(|a| self.expr_location(a));
        // Enum tuple-variant construction: `Enum::Variant(args)` shares parse
        // structure with associated function calls. Intercept it here before
        // falling through to struct method dispatch.
        if let Some(entry) = self.enum_definitions.get(struct_name).cloned() {
            if let Some(variant) = entry.variants.iter().find(|v| v.name == *function_name) {
                let mut arg_values = Vec::new();
                for arg_expr in args {
                    let arg_value = self.evaluate(arg_expr)?;
                    let arg_obj = try_value!(Ok(arg_value));
                    arg_values.push(arg_obj);
                }
                let type_args = derive_enum_type_args(
                    &entry,
                    variant,
                    &arg_values,
                );
                let obj = Object::EnumVariant {
                    enum_name: *struct_name,
                    variant_name: *function_name,
                    values: arg_values,
                    type_args,
                };
                return Ok(EvaluationResult::Value((obj).into()));
            }
        }

        // Convert struct_name and function_name to strings for lookup and clone them to avoid borrow issues
        let struct_name_str = self.string_interner.resolve(*struct_name)
            .ok_or_else(|| InterpreterError::InternalError(format!("Struct name {:?} not found in string interner", struct_name)))?
            .to_string();

        let function_name_str = self.string_interner.resolve(*function_name)
            .ok_or_else(|| InterpreterError::InternalError(format!("Function name {:?} not found in string interner", function_name)))?
            .to_string();

        // Evaluate arguments first
        let mut arg_values = Vec::new();
        for arg_expr in args {
            let arg_value = self.evaluate(arg_expr)?;
            let arg_obj = try_value!(Ok(arg_value));
            arg_values.push(arg_obj);
        }

        // Call the associated function as if it's a static method
        // This is similar to call_struct_method but without self
        self.call_associated_function(
            *struct_name,
            *function_name,
            &arg_values,
            &struct_name_str,
            &function_name_str,
            call_site,
        )
    }

    /// Look up an `extern fn` in the registry and invoke it. Surfaces
    /// a targeted "not yet implemented" error when no Rust impl is
    /// registered for the declared name. Shared by both the
    /// `evaluate_function` (RcObject-result) and
    /// `evaluate_function_with_values` (Value-result) call paths.
    /// Materialise a literal `str` argument as a heap String before it
    /// crosses into a registry implementation, which has no interner
    /// to resolve a `ConstString` symbol with. Shared by both extern
    /// tables so they see the same value shapes (RUNTIME-IO).
    fn normalize_extern_args(&self, args: &[crate::value::Value]) -> Vec<crate::value::Value> {
        args.iter()
            .map(|a| match a {
                crate::value::Value::ConstString(sym) => {
                    let text = self.string_interner.resolve(*sym).unwrap_or("").to_string();
                    crate::value::Value::from(crate::object::Object::String(text))
                }
                other => other.clone(),
            })
            .collect()
    }

    fn dispatch_extern_fn(
        &mut self,
        function: &Rc<Function>,
        args: &[crate::value::Value],
    ) -> Result<crate::value::Value, InterpreterError> {
        let name = self
            .string_interner
            .resolve(function.name)
            .ok_or_else(|| InterpreterError::InternalError(
                "extern fn name failed to resolve in interner".to_string(),
            ))?;
        // EXTERN-BUF: an extern that reaches toylang memory needs the
        // context, so it lives in its own table and is looked up
        // first. The argument normalisation below applies here too —
        // these take a `str` path alongside the buffer.
        if let Some(impl_fn) = self.extern_buf_registry.get(name).copied() {
            let normalized = self.normalize_extern_args(args);
            return impl_fn(self, &normalized);
        }
        match self.extern_registry.get(name) {
            Some(impl_fn) => {
                // Normalise `str` arguments: a literal arrives as a
                // `ConstString` symbol, which the registry closures
                // have no interner to resolve. Materialise it as a
                // heap String so every backend's extern sees the same
                // value shape (RUNTIME-IO).
                let normalized = self.normalize_extern_args(args);
                impl_fn(&normalized)
            }
            // FFI_PLAN P1: a `from`-declared extern the registry does
            // not serve is a genuine C ABI call — dlopen the library
            // and trampoline into the symbol. The interpreter's own
            // io externs (`extern_io`) are registry-served, so this
            // path is for user code.
            None if function.extern_link.is_some() => {
                let link = function.extern_link.as_ref().expect("checked above");
                let lib = self
                    .string_interner
                    .resolve(link.lib)
                    .ok_or_else(|| InterpreterError::InternalError(
                        "FFI: extern fn lib name failed to resolve in interner".to_string(),
                    ))?;
                let symbol = match link.symbol {
                    Some(sym) => self.string_interner.resolve(sym).unwrap_or("").to_string(),
                    None => name.to_string(),
                };
                let fn_ptr =
                    crate::evaluation::extern_ffi::resolve_symbol(lib, &symbol)?;
                let return_type = function.return_type.clone().unwrap_or(frontend::type_decl::TypeDecl::Unit);
                crate::evaluation::extern_ffi::call_extern(
                    fn_ptr,
                    &function.parameter,
                    args,
                    &return_type,
                )
            }
            None => Err(InterpreterError::FunctionNotFound(format!(
                "extern fn `{name}` is not yet implemented in the interpreter"
            ))),
        }
    }

    pub fn evaluate_function(&mut self, function: Rc<Function>, args: &[ExprRef]) -> Result<RcObject, InterpreterError> {
        if function.is_extern {
            // Evaluate args eagerly, then route to the extern dispatch
            // shared with the values-based call path. Keeps the
            // implementation lookup in one place.
            let mut arg_values: Vec<crate::value::Value> = Vec::with_capacity(args.len());
            for arg_expr in args {
                let result = self.evaluate(arg_expr)?;
                let v = match result {
                    EvaluationResult::Value(v) => v,
                    EvaluationResult::Return(_)
                    | EvaluationResult::Break(_)
                    | EvaluationResult::Continue(_)
                    | EvaluationResult::None => {
                        return Err(InterpreterError::InternalError(
                            "extern fn argument produced control-flow value".to_string(),
                        ));
                    }
                };
                arg_values.push(v);
            }
            return self.dispatch_extern_fn(&function, &arg_values).map(|v| v.into_rc());
        }
        let block = match self.stmt_pool.get(&function.code) {
            Some(Stmt::Expression(e)) => {
                match self.expr_pool.get(&e) {
                    Some(Expr::Block(statements)) => statements,
                    _ => return Err(InterpreterError::FunctionNotFound(format!("evaluate_function: Not handled yet {:?}", function.code))),
                }
            }
            _ => return Err(InterpreterError::FunctionNotFound(format!("evaluate_function: Not handled yet {:?}", function.code))),
        };

        self.environment.enter_block();
        for (i, arg) in args.iter().enumerate() {
            let name = function.parameter.get(i)
                .ok_or_else(|| InterpreterError::InternalError("Invalid parameter index".to_string()))?.0;
            let value: RcObject = match self.evaluate(arg) {
                Ok(EvaluationResult::Value(v)) => v.into_rc(),
                Ok(EvaluationResult::Return(v)) => {
                    self.environment.exit_block();
                    return Ok(v.map(|x| x.into_rc()).unwrap_or_else(|| Rc::new(RefCell::new(Object::null_unknown()))));
                },
                Ok(EvaluationResult::Break(_)) | Ok(EvaluationResult::Continue(_)) => {
                    self.environment.exit_block();
                    return Ok(Rc::new(RefCell::new(Object::Unit)));
                },
                Ok(EvaluationResult::None) => Rc::new(RefCell::new(Object::Unit)),
                Err(e) => {
                    self.environment.exit_block();
                    return Err(e);
                },
            };
            self.environment.set_val(name, (value).into());
        }

        // TREE-WALKER-SELF-TYPE-ARG: the declared return type is the
        // annotation for whatever the body returns.
        let res = self
            .with_return_annotation(function.return_type.as_ref(), |ctx| {
                ctx.evaluate_block(&block)
            })?;
        self.environment.exit_block();

        if function.return_type.as_ref().is_none_or(|t| *t == TypeDecl::Unit) {
            Ok(Rc::new(RefCell::new(Object::Unit)))
        } else {
            let value = match res {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(Some(v)) => v,
                EvaluationResult::Return(None) => crate::value::Value::Unit,
                EvaluationResult::Break(_) | EvaluationResult::Continue(_) | EvaluationResult::None => {
                    crate::value::Value::Unit
                }
            };
            // TREE-WALKER-SELF-TYPE-ARG: stamp the declared return
            // type's arguments onto the value leaving the function,
            // the same way a `val`'s annotation stamps the value
            // entering a binding. A payload built from a bare struct
            // literal (`Option::Some(Win { addr: p })`) has no other
            // evidence for its `T`.
            Ok(super::statement::apply_annotation_type_args(
                value,
                function.return_type.as_ref(),
            )
            .into_rc())
        }
    }

    /// Evaluates function with pre-evaluated argument values (used when type checking has already been done).
    /// Phase 5: takes `&[Value]` and returns `Value` so primitive arguments
    /// and return values stay inline through the call boundary.
    pub fn evaluate_function_with_values(&mut self, function: Rc<Function>, args: &[crate::value::Value]) -> Result<crate::value::Value, InterpreterError> {
        // Forwarding form for callers that don't care about the
        // post-body value of `&mut T` parameters. The full form
        // (used by `evaluate_function_call`) collects writebacks
        // so the caller can propagate mutations back to the
        // borrowed locals (REF-Stage-2 (i)).
        let (val, _writebacks) = self.evaluate_function_with_values_writeback(function, args)?;
        Ok(val)
    }

    /// Evaluate a method with pre-evaluated argument values
    /// (DBC-CHECK-METHODS).
    ///
    /// The property checker invokes methods with a generated receiver
    /// and sampled args, mirroring what `evaluate_function_with_values`
    /// does for free functions. `self` is handed over as an `RcObject`
    /// (a generated struct value) and `args` carries the remaining
    /// parameters only — the receiver never appears in the arg slice.
    /// `call_method` performs the same `requires` / `ensures` /
    /// `old(...)` evaluation a real method call would, so contract
    /// violations come back as the same `ContractViolation` errors the
    /// function path produces.
    pub(crate) fn evaluate_method_with_values(
        &mut self,
        method: Rc<MethodFunction>,
        self_obj: RcObject,
        args: &[crate::value::Value],
    ) -> Result<crate::value::Value, InterpreterError> {
        let rc_args: Vec<RcObject> = args.iter().map(crate::value::Value::clone_to_rc).collect();
        // No syntactic call site: the property checker invents the
        // call, so there is no line in the user's source to name.
        let result = self.call_method(method, self_obj, rc_args, None)?;
        Ok(match result {
            EvaluationResult::Value(v) => v,
            EvaluationResult::Return(Some(v)) => v,
            EvaluationResult::Return(None)
            | EvaluationResult::Break(_)
            | EvaluationResult::Continue(_)
            | EvaluationResult::None => crate::value::Value::Unit,
        })
    }

    /// Like `evaluate_function_with_values` but also returns the
    /// post-body value of every `&mut T` parameter, indexed by
    /// parameter position. The returned Vec has the same length
    /// as `function.parameter`; entries for non-mut-ref params are
    /// `None`. The caller (`evaluate_function_call`) pairs these
    /// with the original `&mut <name>` arg expression to write
    /// the modified value back to the caller's binding —
    /// REF-Stage-2 (i) interpreter mutation propagation.
    pub fn evaluate_function_with_values_writeback(
        &mut self,
        function: Rc<Function>,
        args: &[crate::value::Value],
    ) -> Result<(crate::value::Value, Vec<Option<crate::value::Value>>), InterpreterError> {
        if function.is_extern {
            // Extern fns can't take `&mut T` parameters that need
            // writeback (the runtime registry signature doesn't
            // expose them), so an empty writeback list is correct.
            let v = self.dispatch_extern_fn(&function, args)?;
            return Ok((v, vec![None; function.parameter.len()]));
        }
        let block = match self.stmt_pool.get(&function.code) {
            Some(Stmt::Expression(e)) => {
                match self.expr_pool.get(&e) {
                    Some(Expr::Block(statements)) => statements,
                    _ => return Err(InterpreterError::FunctionNotFound(format!("evaluate_function_with_values: Not handled yet {:?}", function.code))),
                }
            }
            _ => return Err(InterpreterError::FunctionNotFound(format!("evaluate_function_with_values: Not handled yet {:?}", function.code))),
        };

        // Recursion guard: the tree-walker burns one host stack frame
        // per toylang call, and a deep (or unliftable-to-IR-VM)
        // recursion overflows the host stack *before* the
        // `recursion_depth` expression-nesting guard fires. Count
        // call frames explicitly and trip first, so the failure is a
        // plain error rather than `fatal runtime error: stack
        // overflow` (exit 134). Decrement on every exit path below.
        self.call_depth += 1;
        if self.call_depth > self.max_call_depth {
            self.call_depth -= 1;
            // DEBUG-OBS D6: reported as a panic, not an internal
            // error. It is a program that ran away, not a defect in
            // the interpreter — and going through `panic_error` is
            // what gives it the backtrace, which folds a runaway
            // recursion into one line with its count.
            //
            // The ceiling is lower here than `compiler_ir::
            // RECURSION_LIMIT` on purpose: this engine spends a *host*
            // stack frame per toylang call and dies around 200 in a
            // debug build, so the limit that keeps it from aborting
            // has to sit under that.
            return Err(self.panic_error(
                compiler_ir::recursion_limit_message(self.max_call_depth as u64),
                None,
            ));
        }

        // STDLIB-FN-SHADOWED-BY-USER-FN: a bare call inside this body
        // resolves in the module the function was written in. Restored
        // at every exit below, beside the recursion counter.
        let prev_home = std::mem::replace(
            &mut self.current_module_path,
            function.module_path.clone(),
        );
        self.environment.enter_block();
        // Track which params are `&mut T` so we can snapshot their
        // post-body values just before `exit_block` clears the
        // function's scope.
        let mut mut_ref_params: Vec<Option<DefaultSymbol>> = Vec::with_capacity(args.len());
        for (i, value) in args.iter().enumerate() {
            let param = function.parameter.get(i)
                .ok_or_else(|| InterpreterError::InternalError("Invalid parameter index".to_string()))?;
            let is_mut_ref = matches!(
                &param.1,
                frontend::type_decl::TypeDecl::Ref { is_mut: true, .. }
            );
            if is_mut_ref {
                self.environment.set_val_mutable(param.0, value.clone());
                mut_ref_params.push(Some(param.0));
            } else {
                // BY-VALUE-SELF-ALIAS: a reference shares the caller's
                // storage, which is the point of it. Everything else
                // is by value, and the compiled lanes hand the callee
                // a copy of the leaves -- so a write inside must not
                // reach the caller's binding here either.
                let is_ref = matches!(
                    &param.1,
                    frontend::type_decl::TypeDecl::Ref { .. }
                );
                let bound = match (is_ref, value) {
                    (false, crate::value::Value::Heap(rc)) => crate::value::Value::Heap(
                        crate::object::copy_for_by_value_param(rc),
                    ),
                    _ => value.clone(),
                };
                self.environment.set_val(param.0, bound);
                mut_ref_params.push(None);
            }
        }

        // POINTER P1: the generic-parameter scope the arguments
        // determine (`fn id<T>(x: T)` called with a u64 binds `T`),
        // with the caller's scope filling anything the arguments
        // cannot name. The compiled lanes monomorphise with the same
        // two sources. Non-generic functions skip the walk entirely
        // (the empty scope costs nothing) so ordinary calls — the
        // `--check` trial hot path — pay for none of this.
        let mut generic_scope = HashMap::new();
        if !function.generic_params.is_empty() {
            let rc_args: Vec<RcObject> = args.iter().map(crate::value::Value::clone_to_rc).collect();
            for (param, ty) in args_generic_scope(&function.parameter, &rc_args) {
                generic_scope.entry(param).or_insert(ty);
            }
            // STDLIB-TRAIT-BASE B5: then the binding's annotation, for
            // a parameter that appears only in the return type.
            let declared_return = function.return_type.clone();
            self.fill_scope_from_annotation(
                &mut generic_scope,
                &function.generic_params,
                declared_return.as_ref(),
            );
            self.fill_scope_from_caller(&mut generic_scope, &function.generic_params);
        }
        self.push_generic_type_scope(generic_scope);

        // Pre-body `requires` checks. Shares the same helper as the method
        // path, so contract evaluation behaves identically across function
        // and method calls.
        if let Err(e) = self.evaluate_requires_clauses(function.name, &function.requires, &function.parameter) {
            self.pop_generic_type_scope();
            self.environment.exit_block();
            self.call_depth -= 1;
            self.current_module_path = prev_home.clone();
            return Err(e);
        }
        if let Err(e) = self.evaluate_old_snapshots(&function.old_exprs) {
            self.pop_generic_type_scope();
            self.environment.exit_block();
            self.call_depth -= 1;
            self.current_module_path = prev_home.clone();
            return Err(e);
        }

        // TREE-WALKER-SELF-TYPE-ARG: see the sibling path above.
        let res = self
            .with_return_annotation(function.return_type.as_ref(), |ctx| {
                ctx.evaluate_block(&block)
            })?;
        self.pop_generic_type_scope();

        let return_value: crate::value::Value = if function.return_type.as_ref().is_none_or(|t| *t == TypeDecl::Unit) {
            crate::value::Value::Unit
        } else {
            // TREE-WALKER-SELF-TYPE-ARG: same stamping as the sibling
            // path — the declared return type is the annotation for
            // the value leaving the function.
            let value = match res {
                EvaluationResult::Value(v) => v,
                EvaluationResult::Return(None) => crate::value::Value::Unit,
                EvaluationResult::Return(v) => v.unwrap_or_else(crate::value::Value::null_unknown),
                EvaluationResult::Break(_) | EvaluationResult::Continue(_) | EvaluationResult::None => crate::value::Value::Unit,
            };
            super::statement::apply_annotation_type_args(value, function.return_type.as_ref())
        };

        // Post-body `ensures` checks with `result` bound to the return value.
        // The contract helper still takes `RcObject`; bridge the value once.
        if let Err(e) = self.evaluate_ensures_clauses(function.name, &function.ensures, &function.ensures_kinds, return_value.clone_to_rc(), &function.parameter) {
            self.environment.exit_block();
            self.call_depth -= 1;
            self.current_module_path = prev_home.clone();
            return Err(e);
        }

        // REF-Stage-2 (i): snapshot the post-body value of each
        // `&mut T` parameter BEFORE `exit_block` discards the
        // function scope. The caller (`evaluate_function_call`)
        // applies these to the corresponding caller-side bindings.
        let writebacks: Vec<Option<crate::value::Value>> = mut_ref_params
            .iter()
            .map(|maybe_sym| maybe_sym.and_then(|s| self.environment.get_val(s)))
            .collect();

        self.call_depth -= 1;
        self.current_module_path = prev_home.clone();
        self.environment.exit_block();
        Ok((return_value, writebacks))
    }

    /// Call a struct method by name
    pub fn call_struct_method(
        &mut self,
        object: RcObject,
        method_name: DefaultSymbol,
        args: &[RcObject],
        struct_name: &str,
        call_site: Option<SourceLocation>,
    ) -> Result<EvaluationResult, InterpreterError> {
        // Look for the method in the function map first
        if let Some(method_func) = self.function.get(&method_name).cloned() {
            // This is a regular function, call it directly. Convert
            // legacy `RcObject` arguments to `Value` at the boundary.
            let object_for_frame = object.clone();
            let mut method_args: Vec<crate::value::Value> = vec![object.into()];
            method_args.extend(args.iter().cloned().map(Into::into));
            // DEBUG-OBS D1: a method that resolved to a plain function
            // still entered user code, so it still gets a frame — the
            // shape of the dispatch is not the reader's problem.
            let frame = self.method_frame_name(&object_for_frame, method_name);
            self.push_frame(frame, call_site);
            let result = self.evaluate_function_with_values(method_func, &method_args)?;
            self.pop_frame();
            return Ok(EvaluationResult::Value(result));
        }

        // Look for struct method. CONCRETE-IMPL Phase 2:
        // `call_struct_method` is hit by `call.rs` paths that have
        // an `RcObject` receiver but no compile-time type args
        // hint; extract type_args from the receiver itself when
        // it's a struct/enum so concrete-impls dispatch correctly.
        let struct_symbol = self.string_interner.get(struct_name)
            .ok_or_else(|| InterpreterError::InternalError(format!("Unknown struct: {}", struct_name)))?;
        let receiver_type_args: Vec<TypeDecl> = match &*object.borrow() {
            Object::Struct { type_args, .. } => type_args.clone(),
            Object::EnumVariant { type_args, .. } => type_args.clone(),
            _ => Vec::new(),
        };

        if let Some(method) = self.get_method(struct_symbol, method_name, &receiver_type_args) {
            let method_args = args.to_vec();
            return self.call_method(method, object, method_args, call_site);
        }

        Err(InterpreterError::FunctionNotFound(
            format!("Method '{}' not found for struct '{}'",
                    self.string_interner.resolve(method_name).unwrap_or("<unknown>"),
                    struct_name)
        ))
    }

    /// Call an associated function (static method) by name
    pub fn call_associated_function(
        &mut self,
        struct_name: DefaultSymbol,
        function_name: DefaultSymbol,
        args: &[RcObject],
        struct_name_str: &str,
        function_name_str: &str,
        call_site: Option<SourceLocation>,
    ) -> Result<EvaluationResult, InterpreterError> {
        // Look for the associated function in the function map first
        // (as a regular function). #193b: try the module-qualified
        // slot `(Some(struct_name), function_name)` first so
        // `math::add(...)` resolves to the stdlib's u64 version
        // even when a user `fn add(Point, Point)` exists. Falls
        // back to the bare-name lookup, then to the legacy flat
        // map for back-compat.
        //
        // FREE-FN-VS-ASSOC-COLLISION: the bare-name fallbacks below
        // must not run before the *type's own* method when the
        // qualifier names a declared type. They did, so a stdlib
        // module gaining a `pub fn join` silently redirected
        // `String::join(parts, sep)` to it -- type-checked, wrong at
        // run time, and only on this engine. A module-qualified
        // lookup (`math::add`) is still tried first: that names a
        // module, not a type.
        let names_a_type = self.struct_definitions.contains_key(&struct_name)
            || self.enum_definitions.contains_key(&struct_name);
        let resolved = if names_a_type {
            let own = self.get_method(struct_name, function_name, &[]);
            if let Some(method) = own {
                return self.call_associated_method(
                    method,
                    args.to_vec(),
                    Some(struct_name),
                    call_site,
                );
            }
            self.lookup_function_qualified(Some(&[struct_name]), function_name)
        } else {
            self.lookup_function_qualified(Some(&[struct_name]), function_name)
                .or_else(|| self.lookup_function_qualified(None, function_name))
                .or_else(|| self.function.get(&function_name).cloned())
        };
        if let Some(func) = resolved {
            // This is a regular function, call it directly without self.
            // Bridge `RcObject` args to `Value` at the boundary.
            let value_args: Vec<crate::value::Value> = args.iter().cloned().map(Into::into).collect();
            // DEBUG-OBS D1, same reasoning as `call_struct_method`.
            self.push_frame(format!("{struct_name_str}::{function_name_str}"), call_site);
            let result = self.evaluate_function_with_values(func, &value_args)?;
            self.pop_frame();
            return Ok(EvaluationResult::Value(result));
        }

        // Look for associated function in struct methods (but call
        // without self). CONCRETE-IMPL Phase 2: associated function
        // calls (`Vec::from_str(...)`) have no receiver to read
        // type_args off of; pass empty so the generic-parameterised
        // impl is preferred. The caller-side annotation hint
        // (`var v: Vec<u8> = ...`) isn't threaded into this layer
        // yet — that's a Phase 2b refinement.
        if let Some(method) = self.get_method(struct_name, function_name, &[]) {
            return self.call_associated_method(method, args.to_vec(), Some(struct_name), call_site);
        }

        // STDLIB-TRAIT-BASE B5: `T::assoc()` inside a generic body.
        // `T` is not a declared type, it is a name the active call's
        // generic scope binds to one -- the same scope
        // `__builtin_sizeof::<T>()` reads. The compiled lanes reach
        // the same answer by substituting the qualifier before
        // lowering.
        if let Some(concrete) = self
            .merged_generic_scope()
            .get(&struct_name)
            .and_then(|ty| self.type_decl_target_symbol(ty))
            .filter(|c| *c != struct_name)
        {
            if let Some(method) = self.get_method(concrete, function_name, &[]) {
                return self
                    .call_associated_method(method, args.to_vec(), Some(concrete), call_site);
            }
        }

        Err(InterpreterError::FunctionNotFound(
            format!("Associated function '{}' not found for struct '{}'",
                    function_name_str, struct_name_str)
        ))
    }
}
