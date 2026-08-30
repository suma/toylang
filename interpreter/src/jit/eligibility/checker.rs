use std::collections::HashMap;

use frontend::ast::{
    BuiltinFunction, Expr, ExprRef, MethodFunction, Operator, Pattern, File, Stmt, StmtRef,
    UnaryOp,
};
use frontend::type_decl::TypeDecl;
use string_interner::DefaultSymbol;

use super::analyze::{expr_kind_name, MonoCall};
use super::collection::{find_impl_target_args, find_method};
use super::extern_dispatch::{
    concat_sym, enum_layout_for, jit_extern_dispatch_for,
    primitive_target_sym_for_scalar, primitive_type_decl_for_target_sym,
    scalar_ty_for_enum_decl,
};
use super::layout::{CompoundLocals, StructLayout};
use super::resolver::{
    infer_substitutions, payload_ty_from_annotation, resolve_param_ty,
    resolve_struct_type_args, self_type_decl, substitute_to_scalar,
};
use super::scalar::{EnumLocalInfo, FieldRepr, PayloadRepr, ScalarTy, StructLocalInfo};
use super::signature::{FuncSignature, MonoTarget, MonomorphSource, ParamTy};

/// Records the *first* reason eligibility analysis rejected the program.
/// Subsequent rejections deeper in the recursion are ignored — the user
/// only needs the closest hint to the surface.
pub(super) fn note(reason: &mut Option<String>, msg: impl FnOnce() -> String) {
    if reason.is_none() {
        *reason = Some(msg());
    }
}

/// Walks a function or method body to confirm it only uses supported
/// constructs and reports every callee found via `callees`. Returns
/// false on the first unsupported construct.
pub(super) fn check_callable_body(
    program: &File,
    source: &MonomorphSource,
    sig: &FuncSignature,
    substitutions: &HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &HashMap<DefaultSymbol, StructLayout>,
    callees: &mut Vec<MonoCall>,
    ptr_read_hints: &mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &mut Option<String>,
) -> bool {
    let code = source.code();
    // Generic sources are forbidden from using `__builtin_ptr_read`
    // because the hint table is keyed by ExprRef, which is shared across
    // monomorphs of the same body. Reject early so the diagnostic is
    // clearer than a per-arm rejection deep inside the body.
    if !source.generic_params().is_empty() && body_has_ptr_read(program, &code) {
        note(reject_reason, || {
            "generic functions cannot use __builtin_ptr_read in JIT".to_string()
        });
        return false;
    }
    let mut locals: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
    let mut compound_locals = CompoundLocals::new();
    for (n, t) in &sig.params {
        match t {
            ParamTy::Scalar(s) => {
                locals.insert(*n, *s);
            }
            ParamTy::Struct { base_name, type_args } => {
                compound_locals
                    .structs
                    .insert(*n, StructLocalInfo::new(*base_name, type_args.clone()));
            }
            ParamTy::Tuple(elements) => {
                compound_locals.tuples.insert(*n, elements.clone());
            }
            // Phase JE-2d/JE-5: enum-typed param registers as an
            // enum local. `ParamTy::Enum` now carries the resolved
            // per-monomorph `payload_ty` (JE-5), so generic enums
            // at the boundary (`Opt<i64>` / `Result<T, E>`) also
            // work — the boundary expansion uses the same payload
            // type as the local does.
            ParamTy::Enum { base_name, payload_ty } => {
                compound_locals.enums.insert(*n, EnumLocalInfo::new(*base_name, *payload_ty));
            }
        }
    }

    let mut checker = Checker::new(
        program,
        substitutions,
        struct_layouts,
        &mut locals,
        &mut compound_locals,
        callees,
        ptr_read_hints,
        reject_reason,
    );

    // Phase JE-2d: enum-returning bodies use a separate validator
    // mirroring struct/tuple — the body's tail expression must be
    // an enum producer (identifier of an enum local, constructor,
    // or a Match whose arms each produce the right enum).
    if let ParamTy::Enum { base_name: enum_name, payload_ty: enum_payload_ty } = &sig.ret {
        return checker.check_enum_returning_body(&code, *enum_name, *enum_payload_ty);
    }
    // For struct-returning functions, the body's terminal expression
    // must produce a struct value (Identifier of a struct local, or a
    // StructLiteral). check_expr rejects struct literals in arbitrary
    // positions, so we process the leading statements normally and then
    // validate the trailing expression by hand.
    if let ParamTy::Struct { base_name, type_args } = &sig.ret {
        return checker.check_struct_returning_body(
            &code,
            &StructLocalInfo::new(*base_name, type_args.clone()),
        );
    }
    // Tuple-returning functions follow a similar shape — last expression
    // must produce a tuple value, fields are gathered for multi-return.
    if let ParamTy::Tuple(element_tys) = &sig.ret {
        return checker.check_tuple_returning_body(&code, element_tys);
    }

    checker.check_stmt(&code)
}


/// Everything the eligibility walk threads through itself.
///
/// The walk is one recursive descent over a callable's body, and every
/// step of it needs the same seven things: the AST to look into, the
/// monomorph's type substitutions and struct layouts to resolve against,
/// the local types it has learned so far, and the three outputs it
/// accumulates (callees to enqueue, `__builtin_ptr_read` type hints, and
/// the rejection reason). Passing those as parameters made every one of
/// the eighteen functions below take nine to eleven arguments, of which
/// eight were always the same eight names in the same order.
///
/// They live here instead. The immutable three are borrowed for the
/// checker's lifetime; the mutable five stay owned by the caller so the
/// two entry points (`check_callable_body` and codegen's type probe)
/// keep deciding what happens to the results.
pub(crate) struct Checker<'a> {
    program: &'a File,
    substitutions: &'a HashMap<DefaultSymbol, ScalarTy>,
    struct_layouts: &'a HashMap<DefaultSymbol, StructLayout>,
    locals: &'a mut HashMap<DefaultSymbol, ScalarTy>,
    compound_locals: &'a mut CompoundLocals,
    callees: &'a mut Vec<MonoCall>,
    ptr_read_hints: &'a mut HashMap<ExprRef, ScalarTy>,
    reject_reason: &'a mut Option<String>,
}

impl<'a> Checker<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        program: &'a File,
        substitutions: &'a HashMap<DefaultSymbol, ScalarTy>,
        struct_layouts: &'a HashMap<DefaultSymbol, StructLayout>,
        locals: &'a mut HashMap<DefaultSymbol, ScalarTy>,
        compound_locals: &'a mut CompoundLocals,
        callees: &'a mut Vec<MonoCall>,
        ptr_read_hints: &'a mut HashMap<ExprRef, ScalarTy>,
        reject_reason: &'a mut Option<String>,
    ) -> Self {
        Self {
            program,
            substitutions,
            struct_layouts,
            locals,
            compound_locals,
            callees,
            ptr_read_hints,
            reject_reason,
        }
    }

    /// Record the *first* rejection reason. See [`note`].
    fn reject(&mut self, msg: impl FnOnce() -> String) {
        note(self.reject_reason, msg);
    }

    /// Type-check a builtin's argument list against the ScalarTys it
    /// expects. Was a closure inside the `BuiltinCall` arm, which meant
    /// re-declaring five of the threaded parameters to get them past the
    /// borrow checker.
    fn check_builtin_args(&mut self, expected: &[ScalarTy], args: &[ExprRef]) -> bool {
        if args.len() != expected.len() {
            self.reject(|| {
                format!(
                    "builtin called with {} arg(s), expected {}",
                    args.len(),
                    expected.len()
                )
            });
            return false;
        }
        for (a, want) in args.iter().zip(expected.iter()) {
            match self.check_expr(a) {
                Some(t) if t == *want => {}
                _ => return false,
            }
        }
        true
    }

    /// Run `f` over a copy of the current locals, so bindings a block
    /// introduces do not outlive it. `Expr::Block` used to express this
    /// by cloning the map and passing the clone as the `locals`
    /// argument; with the map living on the checker, the scope becomes
    /// the thing being said rather than a side effect of an argument.
    fn in_block_scope<R>(&mut self, f: impl FnOnce(&mut Checker<'_>) -> R) -> R {
        let mut scoped = self.locals.clone();
        let mut inner = Checker {
            program: self.program,
            substitutions: self.substitutions,
            struct_layouts: self.struct_layouts,
            locals: &mut scoped,
            compound_locals: self.compound_locals,
            callees: self.callees,
            ptr_read_hints: self.ptr_read_hints,
            reject_reason: self.reject_reason,
        };
        f(&mut inner)
    }

    fn check_struct_returning_body(
        &mut self,
        body_stmt_ref: &StmtRef,
        struct_info: &StructLocalInfo,
    ) -> bool {
        let body_stmt = match self.program.statement.get(body_stmt_ref) {
            Some(s) => s,
            None => return false,
        };
        let body_expr_ref = match body_stmt {
            Stmt::Expression(e) => e,
            _ => {
                self.reject(|| {
                    "struct-returning function body must be an expression".to_string()
                });
                return false;
            }
        };
        let body_expr = match self.program.expression.get(&body_expr_ref) {
            Some(e) => e,
            None => return false,
        };
        let block_stmts = match body_expr {
            Expr::Block(stmts) => stmts,
            _ => {
                self.reject(|| {
                    "struct-returning function body must be a block".to_string()
                });
                return false;
            }
        };
        if block_stmts.is_empty() {
            self.reject(|| {
                "struct-returning function body cannot be empty".to_string()
            });
            return false;
        }
        let (last_ref, leading) = block_stmts.split_last().unwrap();
        for s in leading {
            if !self.check_stmt(s) {
                return false;
            }
        }
        // The trailing statement must produce the declared struct value.
        let last_stmt = match self.program.statement.get(last_ref) {
            Some(s) => s,
            None => return false,
        };
        let result_expr_ref = match last_stmt {
            Stmt::Expression(e) => e,
            _ => {
                self.reject(|| {
                    "struct-returning function body must end in an expression".to_string()
                });
                return false;
            }
        };
        let result_expr = match self.program.expression.get(&result_expr_ref) {
            Some(e) => e,
            None => return false,
        };
        match result_expr {
            Expr::Identifier(name) => {
                match self.compound_locals.structs.get(&name).cloned() {
                    Some(s) if s == *struct_info => true,
                    Some(_) => {
                        self.reject(|| {
                            "returned struct local has a different type than declared"
                                .to_string()
                        });
                        false
                    }
                    None => {
                        self.reject(|| {
                            "returned identifier is not a known struct local".to_string()
                        });
                        false
                    }
                }
            }
            Expr::StructLiteral(lit_name, _) => {
                if lit_name != struct_info.base_name {
                    self.reject(|| {
                        "returned struct literal does not match declared return type"
                            .to_string()
                    });
                    return false;
                }
                // Validate fields against the layout. The declared return
                // type already pins the monomorph down, so pass its type
                // args rather than re-inferring them from the initializers.
                self.check_struct_literal(&result_expr_ref, struct_info.base_name, Some(&struct_info.type_args))
                .is_some()
            }
            _ => {
                self.reject(|| {
                    "struct return value must be an identifier or struct literal"
                        .to_string()
                });
                false
            }
        }
    }

    /// Tuple-returning analog of `check_struct_returning_body`. The body's
    /// last expression must be either a TupleLiteral with the declared
    /// element types or an Identifier of a tuple local with the same shape.
    /// Phase JE-2d: enum-returning function body validator. The body's
    /// tail expression must be an enum producer that codegen's
    /// `gather_enum_values` can lower:
    ///   - `Identifier` of a known enum local
    ///   - `QualifiedIdentifier([enum, variant])` for unit constructors
    ///   - `AssociatedFunctionCall(enum, variant, [arg])` for tuple constructors
    ///   - `Match` whose every arm body is itself a valid enum producer
    ///     for `enum_name` (recursive).
    fn check_enum_returning_body(
        &mut self,
        body_stmt_ref: &StmtRef,
        enum_name: DefaultSymbol,
        enum_payload_ty: Option<ScalarTy>,
    ) -> bool {
        let body_stmt = match self.program.statement.get(body_stmt_ref) {
            Some(s) => s,
            None => return false,
        };
        let body_expr_ref = match body_stmt {
            Stmt::Expression(e) => e,
            _ => {
                self.reject(|| {
                    "enum-returning function body must be an expression".to_string()
                });
                return false;
            }
        };
        let body_expr = match self.program.expression.get(&body_expr_ref) {
            Some(e) => e,
            None => return false,
        };
        let block_stmts = match body_expr {
            Expr::Block(stmts) => stmts,
            _ => {
                self.reject(|| {
                    "enum-returning function body must be a block".to_string()
                });
                return false;
            }
        };
        if block_stmts.is_empty() {
            self.reject(|| {
                "enum-returning function body cannot be empty".to_string()
            });
            return false;
        }
        let (last_ref, leading) = block_stmts.split_last().unwrap();
        for s in leading {
            if !self.check_stmt(s) {
                return false;
            }
        }
        let last_stmt = match self.program.statement.get(last_ref) {
            Some(s) => s,
            None => return false,
        };
        let result_expr_ref = match last_stmt {
            Stmt::Expression(e) => e,
            _ => {
                self.reject(|| {
                    "enum-returning function body must end in an expression".to_string()
                });
                return false;
            }
        };
        self.check_enum_producing_expr(&result_expr_ref, enum_name, enum_payload_ty)
    }

    /// Phase JE-2d: validate an enum-producing expression for a target
    /// enum type. Recurses through `Match` so each arm body is checked
    /// against the same target.
    fn check_enum_producing_expr(
        &mut self,
        expr_ref: &ExprRef,
        enum_name: DefaultSymbol,
        enum_payload_ty: Option<ScalarTy>,
    ) -> bool {
        let expr = match self.program.expression.get(expr_ref) {
            Some(e) => e,
            None => return false,
        };
        match expr {
            Expr::Identifier(name) => {
                match self.compound_locals.enums.get(&name).copied() {
                    Some(info) if info.base_name == enum_name => true,
                    _ => {
                        self.reject(|| {
                            "returned identifier is not an enum local of the declared type".to_string()
                        });
                        false
                    }
                }
            }
            Expr::QualifiedIdentifier(path) if path.len() == 2 && path[0] == enum_name => {
                // Unit constructor; layout existence already verified at
                // signature-resolution time. variant_tag must succeed.
                match enum_layout_for(enum_name).and_then(|l| l.variant_tag(path[1])) {
                    Some(_) => true,
                    None => {
                        self.reject(|| {
                            "returned constructor variant is not declared on this enum".to_string()
                        });
                        false
                    }
                }
            }
            Expr::AssociatedFunctionCall(callee_enum, variant, args) if callee_enum == enum_name => {
                // Tuple constructor — validate the payload arg.
                let layout = match enum_layout_for(enum_name) {
                    Some(l) => l,
                    None => return false,
                };
                let idx = match layout.variants.iter().position(|n| *n == variant) {
                    Some(i) => i,
                    None => {
                        self.reject(|| {
                            "returned constructor variant is not declared on this enum".to_string()
                        });
                        return false;
                    }
                };
                if !layout.variant_has_payload[idx] {
                    self.reject(|| {
                        "returned unit variant constructed with `(...)`".to_string()
                    });
                    return false;
                }
                // Phase JE-5: prefer the per-monomorph payload_ty
                // (provided by the caller — e.g. the function's
                // `ParamTy::Enum.payload_ty` for return-type checking)
                // over the layout's, so generic enums work.
                let payload_ty = match enum_payload_ty.or_else(|| layout.payload_ty()) {
                    Some(t) => t,
                    None => return false,
                };
                if args.len() != 1 {
                    self.reject(|| {
                        "tuple constructor must have one payload arg".to_string()
                    });
                    return false;
                }
                match self.check_expr(&args[0]) {
                    Some(t) if t == payload_ty => true,
                    _ => {
                        self.reject(|| {
                            "tuple constructor payload type does not match declared".to_string()
                        });
                        false
                    }
                }
            }
            Expr::Match(scrutinee, arms) => {
                // Type-check the scrutinee + patterns, then recurse on
                // each arm body against the same enum target.
                let scrut_ty = match self.check_expr(&scrutinee) {
                    Some(t) => t,
                    None => return false,
                };
                let scrut_enum_info = match self.program.expression.get(&scrutinee) {
                    Some(Expr::Identifier(s)) => self.compound_locals.enums.get(&s).copied(),
                    _ => None,
                };
                for arm in &arms {
                    let payload_binding = match self.check_match_pattern(&arm.pattern, scrut_ty, scrut_enum_info) {
                        Some(b) => b,
                        None => return false,
                    };
                    if arm.guard.is_some() {
                        self.reject(|| {
                            "JIT match arm guards are not yet supported".to_string()
                        });
                        return false;
                    }
                    let prev_local = if let Some((name, ty)) = payload_binding {
                        Some((name, self.locals.insert(name, ty)))
                    } else {
                        None
                    };
                    let ok = self.check_enum_producing_expr(&arm.body, enum_name, enum_payload_ty);
                    if let Some((name, prior)) = prev_local {
                        match prior {
                            Some(t) => { self.locals.insert(name, t); }
                            None => { self.locals.remove(&name); }
                        }
                    }
                    if !ok {
                        return false;
                    }
                }
                true
            }
            _ => {
                self.reject(|| {
                    "enum return expression must be an enum local, constructor, or match".to_string()
                });
                false
            }
        }
    }

    fn check_tuple_returning_body(
        &mut self,
        body_stmt_ref: &StmtRef,
        element_tys: &[ScalarTy],
    ) -> bool {
        let body_stmt = match self.program.statement.get(body_stmt_ref) {
            Some(s) => s,
            None => return false,
        };
        let body_expr_ref = match body_stmt {
            Stmt::Expression(e) => e,
            _ => {
                self.reject(|| {
                    "tuple-returning function body must be an expression".to_string()
                });
                return false;
            }
        };
        let body_expr = match self.program.expression.get(&body_expr_ref) {
            Some(e) => e,
            None => return false,
        };
        let block_stmts = match body_expr {
            Expr::Block(stmts) => stmts,
            _ => {
                self.reject(|| {
                    "tuple-returning function body must be a block".to_string()
                });
                return false;
            }
        };
        if block_stmts.is_empty() {
            self.reject(|| {
                "tuple-returning function body cannot be empty".to_string()
            });
            return false;
        }
        let (last_ref, leading) = block_stmts.split_last().unwrap();
        for s in leading {
            if !self.check_stmt(s) {
                return false;
            }
        }
        let last_stmt = match self.program.statement.get(last_ref) {
            Some(s) => s,
            None => return false,
        };
        let result_expr_ref = match last_stmt {
            Stmt::Expression(e) => e,
            _ => {
                self.reject(|| {
                    "tuple-returning function body must end in an expression".to_string()
                });
                return false;
            }
        };
        let result_expr = match self.program.expression.get(&result_expr_ref) {
            Some(e) => e,
            None => return false,
        };
        match result_expr {
            Expr::Identifier(name) => match self.compound_locals.tuples.get(&name).cloned() {
                Some(shape) if shape.as_slice() == element_tys => true,
                Some(_) => {
                    self.reject(|| {
                        "returned tuple local has a different shape than declared".to_string()
                    });
                    false
                }
                None => {
                    self.reject(|| {
                        "returned identifier is not a known tuple local".to_string()
                    });
                    false
                }
            },
            Expr::TupleLiteral(elems) => self.check_tuple_literal_fields(&elems, element_tys),
            _ => {
                self.reject(|| {
                    "tuple return value must be an identifier or tuple literal".to_string()
                });
                false
            }
        }
    }

    /// Validate every element of a tuple literal against the expected
    /// element types. Records callees / ptr_read hints encountered while
    /// typing the individual element initializers.
    fn check_tuple_literal_fields(
        &mut self,
        elements: &[ExprRef],
        expected: &[ScalarTy],
    ) -> bool {
        if elements.len() != expected.len() {
            self.reject(|| {
                format!(
                    "tuple literal has {} element(s), expected {}",
                    elements.len(),
                    expected.len()
                )
            });
            return false;
        }
        for (e, want) in elements.iter().zip(expected.iter()) {
            let actual = match self.check_expr(e) {
                Some(t) => t,
                None => return false,
            };
            if actual != *want {
                self.reject(|| {
                    format!(
                        "tuple literal element type {actual:?} does not match expected {want:?}"
                    )
                });
                return false;
            }
        }
        true
    }

    /// If `value_ref` is a `TupleLiteral`, derive its element types by
    /// type-checking each child expression. Returns the shape only when all
    /// elements are JIT scalars.
    fn tuple_literal_target(&mut self, value_ref: &ExprRef) -> Option<Vec<ScalarTy>> {
        let expr = self.program.expression.get(value_ref)?;
        let elems = match expr {
            Expr::TupleLiteral(es) => es,
            _ => return None,
        };
        if elems.len() < 2 {
            return None;
        }
        let mut shape: Vec<ScalarTy> = Vec::with_capacity(elems.len());
        for e in &elems {
            let t = self.check_expr(e)?;
            if t == ScalarTy::Unit {
                self.reject(|| {
                    "tuple element of Unit type is not supported in JIT".to_string()
                });
                return None;
            }
            shape.push(t);
        }
        Some(shape)
    }

    /// If `value_ref` is a `Call(callee, args)` whose callee returns a
    /// scalar tuple, validate args and record the call site, returning the
    /// tuple's element-type vector for the caller to register as a tuple
    /// local.
    fn check_tuple_returning_call(&mut self, value_ref: &ExprRef) -> Option<Vec<ScalarTy>> {
        let expr = self.program.expression.get(value_ref)?;
        let callee_name = match expr {
            Expr::Call(n, _) => n,
            _ => return None,
        };
        let callee = self.program
            .function
            .iter()
            .find(|f| f.name == callee_name)
            .cloned()?;
        // Only proceed when the callee's return is a tuple of scalars.
        let element_tys = match &callee.return_type {
            Some(td) => match resolve_param_ty(td, self.substitutions, self.struct_layouts) {
                Some(ParamTy::Tuple(t)) => t,
                _ => return None,
            },
            None => return None,
        };
        // Reuse the regular Call analysis for argument validation and
        // monomorph recording.
        let saved_callees_len = self.callees.len();
        let _ = self.check_expr(value_ref);
        if self.callees.len() == saved_callees_len {
            return None;
        }
        Some(element_tys)
    }

    /// If the value-position expression is a `StructLiteral` whose struct
    /// name has a registered scalar layout, return that struct name plus
    /// (#159) the type arguments the annotation pins down, if any. Used to
    /// special-case `val p = Point { … }` / `var p = Point { … }`.
    ///
    /// `None` for the type-argument half means "the annotation didn't say" —
    /// either there is no annotation or the struct isn't generic. A generic
    /// struct with no annotation has its arguments inferred from the field
    /// initializers in `check_struct_literal`.
    fn struct_literal_target(
        &mut self,
        value_ref: &ExprRef,
        type_decl: Option<&TypeDecl>,
    ) -> Option<(DefaultSymbol, Option<Vec<ScalarTy>>)> {
        let expr = self.program.expression.get(value_ref)?;
        let lit_name = match expr {
            Expr::StructLiteral(name, _) => name,
            _ => return None,
        };
        let layout = match self.struct_layouts.get(&lit_name) {
            Some(l) => l,
            None => {
                self.reject(|| {
                    "struct literal references a struct without a JIT-eligible scalar layout"
                        .to_string()
                });
                return None;
            }
        };
        // If a type annotation is present, it must agree with the literal's
        // struct name. Unknown is the parser's placeholder for "no annotation"
        // (the type checker leaves it in place for many shapes), so accept it
        // as if it weren't there. #159: a generic annotation (`Cell<u64>`)
        // is now the primary source of the binding's type arguments.
        let mut annotated_args: Option<Vec<ScalarTy>> = None;
        if let Some(td) = type_decl {
            match td {
                // The parser leaves Unknown when the user writes
                // `var p = Point { … }` without an annotation, so accept it.
                TypeDecl::Unknown => {}
                TypeDecl::Identifier(s) if *s == lit_name => {}
                TypeDecl::Struct(s, args) if *s == lit_name => {
                    if !args.is_empty() || layout.is_generic() {
                        match resolve_struct_type_args(layout, args, self.substitutions) {
                            Some(a) => annotated_args = Some(a),
                            None => {
                                self.reject(|| {
                                    "struct literal annotation's type arguments are not \
                                     representable in the JIT"
                                        .to_string()
                                });
                                return None;
                            }
                        }
                    }
                }
                _ => {
                    self.reject(|| {
                        "struct literal type annotation does not match literal name".to_string()
                    });
                    return None;
                }
            }
        }
        Some((lit_name, annotated_args))
    }

    /// If `value_ref` is a `Call(callee, args)` whose callee returns a known
    /// struct type, validate each argument against the callee's parameters
    /// (Identifier-of-struct-local for struct params; ScalarTy for scalar
    /// params), record the monomorphization, and return the resulting
    /// struct binding. Caller registers the struct local.
    fn check_struct_returning_call(&mut self, value_ref: &ExprRef) -> Option<StructLocalInfo> {
        let expr = self.program.expression.get(value_ref)?;
        let callee_name = match expr {
            Expr::Call(n, _) => n,
            _ => return None,
        };
        let callee = self.program
            .function
            .iter()
            .find(|f| f.name == callee_name)
            .cloned()?;
        // Only proceed when the callee returns a known struct.
        let ret_td = match &callee.return_type {
            Some(td) => match td {
                TypeDecl::Identifier(s) | TypeDecl::Struct(s, _)
                    if self.struct_layouts.contains_key(s) =>
                {
                    td.clone()
                }
                _ => return None,
            },
            None => return None,
        };
        // Reuse the regular Call analysis by delegating to check_expr; it
        // populates self.callees/call_targets and validates arguments. The
        // expected return type from check_expr will be None (struct returns
        // aren't representable as ScalarTy), but the side effects we need
        // already happened.
        //
        // We re-run the call's argument validation manually here so the
        // overall eligibility analysis stays in sync. The check_expr's
        // existing Call branch handles struct-typed parameters, generic
        // inference, and call_targets registration.
        let saved_callees_len = self.callees.len();
        let result = self.check_expr(value_ref);
        // For struct-returning calls, check_expr returns None (since its
        // result type isn't a ScalarTy). That's fine — we only care that
        // the side-effects (call recording, argument validation) succeeded.
        // If check_expr failed before recording the call, treat that as a
        // genuine eligibility failure; otherwise propagate the struct
        // return type.
        if result.is_none() && self.callees.len() == saved_callees_len {
            return None;
        }
        // #159: the return type is written in the *callee's* generic
        // vocabulary (`fn wrap<T>(v: T) -> Cell<T>`), so resolve it against
        // the substitution the call site just inferred rather than the
        // caller's. `check_expr` recorded that inference as the MonoCall's
        // `mono_args`, ordered by the callee's generic params.
        let callee_subs: HashMap<DefaultSymbol, ScalarTy> = self.callees[saved_callees_len..]
            .iter()
            .find(|c| c.call_expr == *value_ref)
            .map(|c| {
                callee
                    .generic_params
                    .iter()
                    .copied()
                    .zip(c.mono_args.iter().copied())
                    .collect()
            })
            .unwrap_or_default();
        match resolve_param_ty(&ret_td, &callee_subs, self.struct_layouts) {
            Some(ParamTy::Struct { base_name, type_args }) => {
                Some(StructLocalInfo::new(base_name, type_args))
            }
            _ => {
                self.reject(|| {
                    "struct-returning call's return type is not resolvable for this \
                     monomorph"
                        .to_string()
                });
                None
            }
        }
    }

    /// Phase JE-2d: detect an enum-returning call as a val/var rhs.
    /// Mirrors `check_struct_returning_call` / `check_tuple_returning_call`
    /// — recurse into `check_expr` so the call's args are validated and
    /// `callees` is populated, then return the enum-type-name when the
    /// call's return is `ParamTy::Enum`.
    fn check_enum_returning_call(&mut self, value_ref: &ExprRef) -> Option<EnumLocalInfo> {
        let expr = self.program.expression.get(value_ref)?;
        let callee_name = match expr {
            Expr::Call(n, _) => n,
            _ => return None,
        };
        let callee = self.program
            .function
            .iter()
            .find(|f| f.name == callee_name)
            .cloned()?;
        // Phase JE-5: accept generic enum returns. Resolve the
        // monomorph payload_ty from the return TypeDecl's args via
        // `payload_ty_from_annotation`.
        // Note the parser-ambiguous `Struct(name, args)` form for
        // user-named types.
        let (ret_enum, ret_payload_ty) = match &callee.return_type {
            Some(td) => match td {
                TypeDecl::Identifier(s) if enum_layout_for(*s).is_some() => {
                    let layout = enum_layout_for(*s).unwrap();
                    (*s, layout.payload_ty())
                }
                TypeDecl::Enum(s, _) | TypeDecl::Struct(s, _)
                    if enum_layout_for(*s).is_some() =>
                {
                    let layout = enum_layout_for(*s).unwrap();
                    // Synthesize a TypeDecl::Enum form so
                    // payload_ty_from_annotation resolves correctly
                    // regardless of which variant the parser emitted.
                    let synthetic_td = match td {
                        TypeDecl::Enum(_, args) => TypeDecl::Enum(*s, args.clone()),
                        TypeDecl::Struct(_, args) => TypeDecl::Enum(*s, args.clone()),
                        _ => unreachable!(),
                    };
                    let args_empty = matches!(
                        td,
                        TypeDecl::Enum(_, a) | TypeDecl::Struct(_, a) if a.is_empty()
                    );
                    let pty = if args_empty {
                        layout.payload_ty()
                    } else {
                        payload_ty_from_annotation(&synthetic_td, &layout)
                    };
                    // For payload-bearing enums the resolution must
                    // produce a scalar — otherwise the boundary is
                    // undefined.
                    if layout.variant_payloads.iter().any(|v| v.is_some())
                        && pty.is_none()
                    {
                        return None;
                    }
                    (*s, pty)
                }
                _ => return None,
            },
            None => return None,
        };
        let saved_callees_len = self.callees.len();
        let result = self.check_expr(value_ref);
        if result.is_none() && self.callees.len() == saved_callees_len {
            return None;
        }
        Some(EnumLocalInfo::new(ret_enum, ret_payload_ty))
    }

    /// Phase JE-2b/JE-3: detect an enum constructor RHS and validate
    /// the payload. Returns `Some(EnumLocalInfo)` when `value_ref` is
    /// one of:
    ///   - `Expr::QualifiedIdentifier([enum, variant])` — unit constructor
    ///   - `Expr::AssociatedFunctionCall(enum, variant, args)` — tuple
    ///     constructor; the single arg's type must match the enum's
        ///     payload_repr (Concrete or Generic resolved per call-site).
        ///
        /// Returns `None` for everything else (the regular check_expr path
        /// runs). Side effect: validates the payload arg via `check_expr`,
        /// which recursively records callees / ptr_read hints.
    ///
    /// `annotation_hint` is the val/var annotation if available (the
    /// declared enum type with type args). Used when the rhs is a unit
    /// constructor of a generic enum (e.g. `val o: Option<i64> = Option::None`)
    /// — the payload_ty has to come from somewhere.
    fn check_enum_constructor_rhs(
        &mut self,
        value_ref: &ExprRef,
        annotation_hint: Option<&TypeDecl>,
    ) -> Option<EnumLocalInfo> {
        let expr = self.program.expression.get(value_ref)?;
        match expr {
            Expr::QualifiedIdentifier(path) if path.len() == 2 => {
                let enum_name = path[0];
                let variant = path[1];
                let layout = enum_layout_for(enum_name)?;
                // Variant must exist and be a unit variant.
                let idx = layout.variants.iter().position(|n| *n == variant)?;
                if layout.variant_has_payload[idx] {
                    // Caller wrote `Status::Ok` (no parens) but the
                    // variant carries a payload — reject so the regular
                    // path can produce the right error.
                    return None;
                }
                // Phase JE-3/JE-4: payload_ty resolution depends on
                // whether the enum has any tuple variant. Unit-only
                // enums report `None`. Otherwise the per-monomorph
                // uniform payload comes from the val/var annotation
                // (via `resolve_uniform_payload`). For non-generic
                // enums the annotation is unnecessary because
                // `resolve_uniform_payload(empty subst)` already
                // produces the right answer.
                let payload_ty = if !layout.variant_payloads.iter().any(|v| v.is_some()) {
                    None
                } else if let Some(t) = layout.resolve_uniform_payload(&HashMap::new()) {
                    Some(t)
                } else {
                    match annotation_hint.and_then(|td| payload_ty_from_annotation(td, &layout)) {
                        Some(t) => Some(t),
                        None => {
                            self.reject(|| {
                                "JIT generic enum unit constructor needs an annotation \
                                 with concrete type args (e.g. `val o: Option<i64> = Option::None`)"
                                    .to_string()
                            });
                            return None;
                        }
                    }
                };
                Some(EnumLocalInfo::new(enum_name, payload_ty))
            }
            Expr::AssociatedFunctionCall(enum_name, variant, args) => {
                let layout = enum_layout_for(enum_name)?;
                let idx = layout.variants.iter().position(|n| *n == variant)?;
                if !layout.variant_has_payload[idx] {
                    // `Status::Bad(5i64)` — variant doesn't take a payload.
                    return None;
                }
                if args.len() != 1 {
                    self.reject(|| {
                        "JIT enum tuple constructor: only single-payload \
                         variants are supported (JE-2a scope; see JIT-enum-1)".to_string()
                    });
                    return None;
                }
                let arg_ty = self.check_expr(&args[0])?;
                // Phase JE-3/JE-4: variant payload comes from
                // `variant_payloads[idx]`. For Concrete the arg type
                // must match; for Generic the arg type itself fixes
                // that variant's payload (and we cross-check against
                // the annotation when present so multi-generic enums
                // like `Result<T, E>` get the consistency check).
                let variant_repr = layout.variant_payloads[idx].clone()?;
                let payload_ty = match variant_repr {
                    PayloadRepr::Concrete(declared) => {
                        if arg_ty != declared {
                            self.reject(|| {
                                format!(
                                    "JIT enum tuple constructor: payload type {arg_ty:?} does not \
                                     match declared {declared:?}"
                                )
                            });
                            return None;
                        }
                        arg_ty
                    }
                    PayloadRepr::Generic(_) => {
                        if let Some(td) = annotation_hint {
                            if let Some(t) = payload_ty_from_annotation(td, &layout) {
                                if t != arg_ty {
                                    self.reject(|| {
                                        format!(
                                            "JIT generic enum constructor: arg type {arg_ty:?} does \
                                             not match annotation type {t:?}"
                                        )
                                    });
                                    return None;
                                }
                            }
                        }
                        arg_ty
                    }
                    PayloadRepr::None => return None,
                };
                Some(EnumLocalInfo::new(enum_name, Some(payload_ty)))
            }
            _ => None,
        }
    }

    /// Validate every field of a struct literal against the registered
    /// layout and (#159) determine the binding's type arguments. Records
    /// callees / ptr_read hints encountered while typing the individual
    /// field initializers.
    ///
    /// `expected_args` is `Some(args)` when the position already pins the
    /// monomorph down (a `Cell<u64>` annotation, or a declared return
    /// type). For a generic struct without one, the arguments are inferred
    /// from the field initializers: a field declared as the type parameter
    /// `T` binds `T` to whatever scalar its initializer produced. Every
    /// parameter must end up bound — a generic that appears in no field
    /// (phantom) has nothing to infer from and stays on the interpreter.
    fn check_struct_literal(
        &mut self,
        value_ref: &ExprRef,
        struct_name: DefaultSymbol,
        expected_args: Option<&[ScalarTy]>,
    ) -> Option<StructLocalInfo> {
        let layout = match self.struct_layouts.get(&struct_name) {
            Some(l) => l.clone(),
            None => {
                self.reject(|| "struct layout missing in JIT analysis".to_string());
                return None;
            }
        };
        let expr = self.program.expression.get(value_ref)?;
        let lit_fields = match expr {
            Expr::StructLiteral(_, fields) => fields,
            _ => return None,
        };
        if lit_fields.len() != layout.fields.len() {
            self.reject(|| {
                format!(
                    "struct literal has {} field(s), layout expects {}",
                    lit_fields.len(),
                    layout.fields.len()
                )
            });
            return None;
        }
        // Type each initializer once; the same list drives both the
        // inference of the type args and the per-field validation below.
        let mut actual_fields: Vec<(DefaultSymbol, ScalarTy)> =
            Vec::with_capacity(lit_fields.len());
        for (field_sym, field_expr) in &lit_fields {
            if !layout.fields.iter().any(|(n, _)| n == field_sym) {
                self.reject(|| "unknown field in struct literal".to_string());
                return None;
            }
            let actual = self.check_expr(field_expr)?;
            actual_fields.push((*field_sym, actual));
        }
        // The type args always have to be recoverable from the field
        // initializers, even when an annotation states them: codegen
        // re-derives them the same way (it has no annotation at hand), so
        // requiring it here keeps the two passes in lockstep. The cost is
        // that a phantom parameter — one no field mentions — stays on the
        // interpreter.
        let inferred = infer_struct_type_args(&layout, &actual_fields, self.reject_reason)?;
        if let Some(expected) = expected_args {
            if expected != inferred.as_slice() {
                self.reject(|| {
                    "struct literal's inferred type arguments disagree with its annotation"
                        .to_string()
                });
                return None;
            }
        }
        let type_args = inferred;
        for (field_sym, actual) in &actual_fields {
            let want = match layout.field(*field_sym, &type_args) {
                Some(t) => t,
                None => {
                    self.reject(|| {
                        "struct literal field type is not resolvable for this monomorph"
                            .to_string()
                    });
                    return None;
                }
            };
            if *actual != want {
                self.reject(|| {
                    format!("struct literal field type {actual:?} does not match layout {want:?}")
                });
                return None;
            }
        }
        Some(StructLocalInfo::new(struct_name, type_args))
    }


    /// Phase JE-1b: validate that a `Pattern` is supported by the JIT
    /// match codegen for the given scrutinee scalar type. Patterns
    /// reduce to one of:
    ///   - `Wildcard` — accepted for any scrutinee type
    ///   - `Literal(ExprRef)` — accepted when the literal's scalar type
    ///     matches the scrutinee
    ///   - `EnumVariant(enum, variant, [])` — accepted when the enum is
    ///     in `enum_layouts`, the variant is unit, and the scrutinee is
        ///     a U64 tag (the JIT representation of unit-only enums)
        ///
        /// Tuple patterns and named bindings (which require payload
        /// extraction) are rejected.
        ///
        /// Returns `Ok(payload_binding)` when the pattern is accepted.
        /// `payload_binding` is `Some((name, ty))` when an EnumVariant
        /// pattern carries a single Pattern::Name sub-pattern (JE-2b
        /// payload binding); otherwise `None`. Caller installs the
        /// binding in `locals` for the arm body and removes it after.
    ///
    /// `scrut_enum` carries the per-local enum info when the
    /// scrutinee is an enum identifier — the per-local `payload_ty`
    /// determines what type the pattern's Name binds to (important
    /// for generic enums where the layout's payload_repr is `Generic`).
    fn check_match_pattern(
        &mut self,
        pat: &Pattern,
        scrut_ty: ScalarTy,
        scrut_enum: Option<EnumLocalInfo>,
    ) -> Option<Option<(DefaultSymbol, ScalarTy)>> {
        match pat {
            Pattern::Wildcard => Some(None),
            Pattern::Literal(eref) => {
                let lit_ty = match self.program.expression.get(eref) {
                    Some(Expr::Int64(_)) => ScalarTy::I64,
                    Some(Expr::UInt64(_)) => ScalarTy::U64,
                    Some(Expr::True) | Some(Expr::False) => ScalarTy::Bool,
                    _ => {
                        self.reject(|| {
                            "JIT match: unsupported literal pattern shape".to_string()
                        });
                        return None;
                    }
                };
                if lit_ty != scrut_ty {
                    self.reject(|| {
                        format!(
                            "JIT match: literal pattern type {lit_ty:?} does not match scrutinee {scrut_ty:?}"
                        )
                    });
                    return None;
                }
                Some(None)
            }
            Pattern::EnumVariant(enum_sym, variant_sym, sub_pats) => {
                if scrut_ty != ScalarTy::U64 {
                    self.reject(|| {
                        format!(
                            "JIT match: enum variant pattern but scrutinee is {scrut_ty:?}, expected U64 tag"
                        )
                    });
                    return None;
                }
                let layout = match enum_layout_for(*enum_sym) {
                    Some(l) => l,
                    None => {
                        self.reject(|| {
                            "JIT match: enum is not JIT-eligible (generic / mixed payloads; \
                         see JIT-enum-1)".to_string()
                        });
                        return None;
                    }
                };
                let idx = match layout.variants.iter().position(|n| *n == *variant_sym) {
                    Some(i) => i,
                    None => {
                        self.reject(|| {
                            "JIT match: variant not declared on this enum".to_string()
                        });
                        return None;
                    }
                };
                // Phase JE-2b: variant with payload — accept Pattern::Name
                // for binding the payload value, or empty sub_pats when
                // the user wrote `Status::Ok =>` (which would still match
                // semantically; treat as no binding).
                if layout.variant_has_payload[idx] {
                    // Phase JE-3: prefer the scrutinee's per-local
                    // payload_ty (which knows the resolved generic
                    // monomorph) over the layout's. Fall back to layout
                    // payload_ty for non-generic enums when scrut_enum
                    // is missing (legacy callers).
                    let payload_ty = scrut_enum
                        .and_then(|e| e.payload_ty)
                        .or_else(|| layout.payload_ty())?;
                    match sub_pats.as_slice() {
                        [] => Some(None),
                        [single] => match single {
                            Pattern::Name(payload_name) => {
                                Some(Some((*payload_name, payload_ty)))
                            }
                            Pattern::Wildcard => Some(None),
                            _ => {
                                self.reject(|| {
                                    "JIT match: only Name / Wildcard sub-pattern \
                                     supported for tuple-variant payload (JE-2b)".to_string()
                                });
                                None
                            }
                        },
                        _ => {
                            self.reject(|| {
                                "JIT match: multi-payload tuple variants not supported \
                                 (JE-2a scope)".to_string()
                            });
                            None
                        }
                    }
                } else {
                    if !sub_pats.is_empty() {
                        self.reject(|| {
                            "JIT match: unit variant cannot bind payload".to_string()
                        });
                        return None;
                    }
                    Some(None)
                }
            }
            // PATTERN-STRUCT / PATTERN-EXTEND: struct, tuple and `n @ pat`
            // patterns fall back to the interpreter.
            Pattern::Struct(_, _, _)
            | Pattern::Tuple(_)
            | Pattern::Name(_)
            | Pattern::Binding(_, _)
            | Pattern::Range(_, _) => {
                self.reject(|| {
                    "JIT match: tuple / top-level name / `@` / range patterns not yet supported"
                        .to_string()
                });
                None
            }
        }
    }


    /// #159: bind a method's generic parameters from the receiver's type
    /// arguments.
    ///
    /// A method on `impl<T> Cell<T>` carries `T` in its `generic_params`
    /// (the parser merges the impl block's parameters into every method),
    /// so the receiver's monomorph decides what `T` is. Two routes bind it:
    ///
    ///   * by position through the impl block's `target_type_args` — the
    ///     impl may spell the parameter differently from the declaration
    ///     (`impl<U> Cell<U>`), and a *concrete* argument there
    ///     (`impl Foo for Cell<u8>`) instead has to agree with the
    ///     receiver, or this call isn't for that impl;
    ///   * by symbol identity with the struct declaration's own parameters,
    ///     which covers the common case where both spell it `T` and the
    ///     parser left `target_type_args` empty.
    ///
    /// Returns `None` (with a reason) when a parameter stays unbound — a
    /// method-only generic (`fn map<U>(…)`) has nothing at the call site to
    /// infer from and remains interpreter-only.
    fn method_substitution(
        &mut self,
        receiver: &StructLocalInfo,
        method: &MethodFunction,
    ) -> Option<HashMap<DefaultSymbol, ScalarTy>> {
        let mut subst: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
        if let Some(layout) = self.struct_layouts.get(&receiver.base_name) {
            if layout.generic_params.len() == receiver.type_args.len() {
                for (p, t) in layout.generic_params.iter().zip(receiver.type_args.iter()) {
                    subst.insert(*p, *t);
                }
            }
        }
        if let Some(target_args) = find_impl_target_args(self.program, receiver.base_name, method.name) {
            if !target_args.is_empty() {
                if target_args.len() != receiver.type_args.len() {
                    self.reject(|| {
                        "impl block's type arguments do not match the receiver's".to_string()
                    });
                    return None;
                }
                for (arg, actual) in target_args.iter().zip(receiver.type_args.iter()) {
                    match arg {
                        TypeDecl::Generic(sym) | TypeDecl::Identifier(sym)
                            if method.generic_params.contains(sym) =>
                        {
                            subst.insert(*sym, *actual);
                        }
                        concrete => {
                            // A specialised impl (`impl Trait for Cell<u8>`):
                            // it only applies when the receiver's argument is
                            // that very type.
                            if ScalarTy::from_type_decl(concrete) != Some(*actual) {
                                self.reject(|| {
                                    "receiver's type arguments do not match this impl block"
                                        .to_string()
                                });
                                return None;
                            }
                        }
                    }
                }
            }
        }
        for p in &method.generic_params {
            if !subst.contains_key(p) {
                self.reject(|| {
                    "generic methods are not yet JIT-compatible".to_string()
                });
                return None;
            }
        }
        Some(subst)
    }

    fn check_stmt(&mut self, stmt_ref: &StmtRef) -> bool {
        let stmt = match self.program.statement.get(stmt_ref) {
            Some(s) => s,
            None => return false,
        };
        match stmt {
            Stmt::Expression(e) => {
                self.check_expr(&e).is_some()
            }
            Stmt::Val(name, type_decl, value) => {
                // Special-case: a struct-literal RHS registers `name` as a
                // struct local. Field-by-field types are validated against the
                // struct's known layout; everything else falls through to the
                // scalar path.
                if let Some((struct_name, annotated_args)) = self.struct_literal_target(&value, type_decl.as_ref()) {
                    let info = match self.check_struct_literal(&value, struct_name, annotated_args.as_deref()) {
                        Some(i) => i,
                        None => return false,
                    };
                    self.compound_locals.structs.insert(name, info);
                    return true;
                }
                // Special-case: a struct-returning function call also lands as
                // a fresh struct local. Validate the call site (and its args)
                // through the normal Call eligibility path.
                if let Some(struct_name) = self.check_struct_returning_call(&value) {
                    self.compound_locals.structs.insert(name, struct_name);
                    return true;
                }
                // Tuple literal RHS — `val pair = (1i64, 2u64)` — registers
                // `name` as a tuple local with the inferred element shape.
                if let Some(shape) = self.tuple_literal_target(&value) {
                    self.compound_locals.tuples.insert(name, shape);
                    return true;
                }
                // Tuple-returning call — `val pair = make_pair()`.
                if let Some(shape) = self.check_tuple_returning_call(&value) {
                    self.compound_locals.tuples.insert(name, shape);
                    return true;
                }
                // Tuple alias — `val q = pair` where `pair` is already a
                // known tuple local.
                if let Some(Expr::Identifier(rhs_name)) = self.program.expression.get(&value) {
                    if let Some(shape) = self.compound_locals.tuples.get(&rhs_name).cloned() {
                        self.compound_locals.tuples.insert(name, shape);
                        return true;
                    }
                }
                // Phase JE-2b: enum constructor RHS registers `name` as
                // an enum local so subsequent match scrutinees can recover
                // both the variant tag and the payload (if any). Both
                // unit-variant (`Color::Red` / `Status::Bad`) and tuple-
                // variant (`Status::Ok(5i64)`) constructors land here when
                // the enum is in `enum_layouts`. Tuple-variant arg type
                // is validated against `EnumLayout::payload_ty`.
                //
                // Phase JE-3: pass the val/var annotation as a hint so
                // generic-enum unit constructors (`Option::None`) can
                // resolve T from the annotation.
                if let Some(info) = self.check_enum_constructor_rhs(&value, type_decl.as_ref()) {
                    self.compound_locals.enums.insert(name, info);
                    return true;
                }
                // Enum alias — `val n: Box = b` where `b` is already a
                // known enum local of the same type.
                if let Some(Expr::Identifier(rhs_name)) = self.program.expression.get(&value) {
                    if let Some(info) = self.compound_locals.enums.get(&rhs_name).copied() {
                        self.compound_locals.enums.insert(name, info);
                        return true;
                    }
                }
                // Phase JE-2d: enum-returning call as val/var rhs.
                // Mirrors `check_struct_returning_call` /
                // `check_tuple_returning_call` — runs the call's
                // arg-validation through check_expr (which records
                // self.callees) and registers the enum local.
                if let Some(info) = self.check_enum_returning_call(&value) {
                    self.compound_locals.enums.insert(name, info);
                    return true;
                }
                let declared_hint = type_decl.as_ref().and_then(ScalarTy::from_type_decl);
                // If both the annotation and the RHS are PtrRead-shaped, record
                // the expected return type before recursing so check_expr can
                // accept the otherwise type-polymorphic builtin.
                if let Some(t) = declared_hint {
                    self.register_ptr_read_hint(&value, t);
                }
                let val_ty = match self.check_expr(&value) {
                    Some(t) => t,
                    None => return false,
                };
                let declared = match type_decl {
                    // The parser leaves Unknown when the user wrote no
                    // annotation; treat it as "infer from rhs".
                    Some(TypeDecl::Unknown) | None => val_ty,
                    Some(td) => match ScalarTy::from_type_decl(&td) {
                        Some(t) => t,
                        None => match scalar_ty_for_enum_decl(&td) {
                            // Phase JE-1b: a JIT-eligible enum
                            // annotation (`val c: Color = ...`)
                            // resolves to ScalarTy::U64 because the
                            // enum's representation at the JIT layer
                            // is just its tag.
                            Some(t) => t,
                            None => return false,
                        },
                    },
                };
                if declared != val_ty {
                    return false;
                }
                // Reject Unit and Never RHS: there is no value to bind, and
                // recording `Never` in `self.locals` would poison subsequent
                // expressions that read `name`. `val x = panic(...)` is the
                // typical Never case — silent-fallback is fine since the
                // expression does the same observable thing in the
                // interpreter.
                if matches!(declared, ScalarTy::Unit | ScalarTy::Never) {
                    return false;
                }
                self.locals.insert(name, declared);
                true
            }
            Stmt::Var(name, type_decl, value) => {
                // Mirror the Val struct-literal special case — `var p = Point { ... }`
                // also registers a struct local.
                if let Some(v) = value {
                    if let Some((struct_name, annotated_args)) = self.struct_literal_target(&v, type_decl.as_ref()) {
                        let info = match self.check_struct_literal(&v, struct_name, annotated_args.as_deref()) {
                            Some(i) => i,
                            None => return false,
                        };
                        self.compound_locals.structs.insert(name, info);
                        return true;
                    }
                    if let Some(struct_name) = self.check_struct_returning_call(&v) {
                        self.compound_locals.structs.insert(name, struct_name);
                        return true;
                    }
                    if let Some(shape) = self.tuple_literal_target(&v) {
                        self.compound_locals.tuples.insert(name, shape);
                        return true;
                    }
                    if let Some(shape) = self.check_tuple_returning_call(&v) {
                        self.compound_locals.tuples.insert(name, shape);
                        return true;
                    }
                    if let Some(Expr::Identifier(rhs_name)) = self.program.expression.get(&v) {
                        if let Some(shape) = self.compound_locals.tuples.get(&rhs_name).cloned() {
                            self.compound_locals.tuples.insert(name, shape);
                            return true;
                        }
                    }
                }
                let declared = match (type_decl.as_ref(), value) {
                    // Treat `Some(Unknown)` like `None` — the parser inserts
                    // it when the user wrote no annotation.
                    (Some(TypeDecl::Unknown), Some(v)) | (None, Some(v)) => {
                        match self.check_expr(&v) {
                            Some(t) => t,
                            None => return false,
                        }
                    }
                    (Some(td), _) => match ScalarTy::from_type_decl(td) {
                        Some(t) => t,
                        None => return false,
                    },
                    (None, None) => return false,
                };
                if let Some(v) = value {
                    if type_decl.is_some() {
                        self.register_ptr_read_hint(&v, declared);
                    }
                    let val_ty = match self.check_expr(&v) {
                        Some(t) => t,
                        None => return false,
                    };
                    if val_ty != declared {
                        return false;
                    }
                }
                if matches!(declared, ScalarTy::Unit | ScalarTy::Never) {
                    return false;
                }
                self.locals.insert(name, declared);
                true
            }
            Stmt::Return(value) => {
                if let Some(v) = value {
                    self.check_expr(&v).is_some()
                } else {
                    true
                }
            }
            Stmt::Break(_) | Stmt::Continue(_) => true,
            Stmt::For(_label, var, start, end, block) => {
                let start_ty = match self.check_expr(&start) {
                    Some(t) => t,
                    None => return false,
                };
                let end_ty = match self.check_expr(&end) {
                    Some(t) => t,
                    None => return false,
                };
                if start_ty != end_ty {
                    return false;
                }
                if !matches!(start_ty, ScalarTy::I64 | ScalarTy::U64) {
                    return false;
                }
                let prev = self.locals.insert(var, start_ty);
                let body_ok =
                    self.check_expr(&block).is_some();
                match prev {
                    Some(t) => {
                        self.locals.insert(var, t);
                    }
                    None => {
                        self.locals.remove(&var);
                    }
                }
                body_ok
            }
            Stmt::While(_label, cond, block) => {
                let cond_ty = match self.check_expr(&cond) {
                    Some(t) => t,
                    None => return false,
                };
                if cond_ty != ScalarTy::Bool {
                    return false;
                }
                self.check_expr(&block).is_some()
            }
            // No struct / impl / enum declarations are tolerated inside an
            // eligible function body. Top-level decls live outside of any
            // function so they don't affect us here.
            Stmt::StructDecl { .. } | Stmt::ImplBlock { .. } | Stmt::EnumDecl { .. } | Stmt::TraitDecl { .. } => false,
            // Type aliases are resolved at parse time; their presence inside
            // a function body (which the parser doesn't actually allow) is
            // a no-op and would not disqualify the body either way.
            Stmt::TypeAlias { .. } => true,
        }
    }

    /// If `value_ref` is a direct `__builtin_ptr_read(...)` call, register
    /// `expected` as the read's return type so check_expr can accept it. The
    /// JIT only supports PtrRead in positions where the expected type is
    /// statically known (val/var with annotation, assignment to a typed
    /// identifier).
    fn register_ptr_read_hint(&mut self, value_ref: &ExprRef, expected: ScalarTy) {
        if let Some(Expr::BuiltinCall(BuiltinFunction::PtrRead, _)) =
            self.program.expression.get(value_ref)
        {
            self.ptr_read_hints.insert(*value_ref, expected);
        }
    }

    /// Returns the type produced by the expression, or `None` if the expression
    /// uses an unsupported construct. As a side effect, populates `callees` with
    /// names of user-defined functions invoked by this expression and
    /// `ptr_read_hints` with PtrRead expected return types where statically
    /// derivable from context.
    pub(crate) fn check_expr(&mut self, expr_ref: &ExprRef) -> Option<ScalarTy> {
        let expr = self.program.expression.get(expr_ref)?;
        match expr {
            // Phase JE-1b: unit-variant constructor (`Color::Red`).
            // Reduce to `ScalarTy::U64` so the rest of eligibility +
            // codegen treats the value as just the tag — that's the
            // entire representation for unit variants. Generic enums
            // and enums with payload variants miss the layout map and
            // fall through to the catch-all reject.
            Expr::QualifiedIdentifier(path)
                if path.len() == 2
                    && enum_layout_for(path[0])
                        .and_then(|l| l.variant_tag(path[1]))
                        .is_some() =>
            {
                        Some(ScalarTy::U64)
                    }
            Expr::Int64(..)
            | Expr::UInt64(..)
            | Expr::Int8(..)
            | Expr::Int16(..)
            | Expr::Int32(..)
            | Expr::UInt8(..)
            | Expr::UInt16(..)
            | Expr::UInt32(..)
            | Expr::CharLiteral(..)
            | Expr::Float64(..)
            | Expr::True
            | Expr::False
            | Expr::String(..)
            | Expr::Identifier(..) => self.check_literals_and_identifiers(expr),
            Expr::Match(..) => self.check_match_expr(expr),
            Expr::Binary(..)
            | Expr::Unary(..) => self.check_operators(expr),
            Expr::Block(..)
            | Expr::IfElifElse(..)
            | Expr::Assign(..) => self.check_blocks_and_assignment(expr),
            Expr::AssociatedFunctionCall(..)
            | Expr::Call(..) => self.check_calls(expr, expr_ref),
            Expr::BuiltinCall(..) => self.check_builtins(expr, expr_ref),
            Expr::With(..) => self.check_with_scope(expr),
            Expr::MethodCall(..) => self.check_method_calls(expr, expr_ref),
            Expr::FieldAccess(..)
            | Expr::TupleAccess(..) => self.check_member_access(expr),
            Expr::Cast(..) => self.check_casts(expr),
            other => {
                // Phase JE-1a: a `QualifiedIdentifier` whose head is a
                // JIT-eligible enum (non-generic, unit-only) corresponds
                // to a unit-variant constructor like `Color::Red`. The
                // tag layout is already in `enum_layouts`; the missing
                // piece is constructor + match codegen (Phase JE-1b).
                // Surface a precise "infra ready, codegen pending"
                // message instead of the generic "qualified identifier"
                // catch-all so the next phase knows which programs to
                // enable.
                let precise = match &other {
                    Expr::QualifiedIdentifier(path)
                        if path.len() == 2
                            && enum_layout_for(path[0])
                                .and_then(|l| l.variant_tag(path[1]))
                                .is_some() =>
                    {
                        "JIT enum support pending: unit-variant constructor codegen \
                         (Phase JE-1b will lower this via the existing tag layout)"
                            .to_string()
                    }
                    Expr::QualifiedIdentifier(path)
                        if !path.is_empty()
                            && enum_decl_lookup_by_name(self.program, path[0]).is_some() =>
                    {
                        "JIT does not yet model enum values \
                         (constructors / match / methods)"
                            .to_string()
                    }
                    // Closures Phase 4: explicit reject reason. The JIT
                    // doesn't model `Object::Closure` values — each
                    // closure literal would need a captured-environment
                    // representation + indirect-call dispatch the JIT
                    // doesn't have. The interpreter handles closures
                    // natively (Phase 3); JIT-eligible programs simply
                    // fall back to interpretation when they contain a
                    // closure literal.
                    Expr::Closure { .. } => {
                        "JIT does not yet support closure / lambda values \
                         (interpreter handles them; AOT support is a later phase)"
                            .to_string()
                    }
                    _ => format!("uses unsupported expression {}", expr_kind_name(&other)),
                };
                self.reject(move || precise);
                None
            }
        }
    }

    /// Literals of every width, string literals, and a bare name.
    fn check_literals_and_identifiers(&mut self, expr: Expr) -> Option<ScalarTy> {
        match expr {
            Expr::Int64(_) => Some(ScalarTy::I64),
            Expr::UInt64(_) => Some(ScalarTy::U64),
            // NUM-W narrow integer literals.
            Expr::Int8(_) => Some(ScalarTy::I8),
            Expr::Int16(_) => Some(ScalarTy::I16),
            Expr::Int32(_) => Some(ScalarTy::I32),
            Expr::UInt8(_) => Some(ScalarTy::U8),
            Expr::UInt16(_) => Some(ScalarTy::U16),
            Expr::UInt32(_) | Expr::CharLiteral(_) => Some(ScalarTy::U32),
            Expr::Float64(_) => Some(ScalarTy::F64),
            Expr::True | Expr::False => Some(ScalarTy::Bool),
            // STR-INTERP-INTERP-JIT: a string literal in an expression
            // position lowers to a `jit_string_literal(sym_id)` call
            // that materialises a heap str. The literal flows through
            // the JIT as a `ScalarTy::Str` value (i64 pointer).
            Expr::String(_) => Some(ScalarTy::Str),
            Expr::Identifier(sym) => {
                // Phase JE-2b: enum-typed self.locals report their tag type
                // (U64) so they participate in match scrutinees and bare
                // value-position uses transparently. Codegen distinguishes
                // them via `enum_locals`; eligibility downstream just sees
                // a U64.
                if self.locals.get(&sym).copied().is_some() {
                    return self.locals.get(&sym).copied();
                }
                if self.compound_locals.enums.contains_key(&sym) {
                    return Some(ScalarTy::U64);
                }
                None
            }
            _ => unreachable!("check_literals_and_identifiers was handed an expression it does not own"),
        }
    }

    /// Pattern matching.
    fn check_match_expr(&mut self, expr: Expr) -> Option<ScalarTy> {
        match expr {
            // Phase JE-1b: `match scrutinee { ... }` for scalar / unit-
            // enum-tag scrutinees. Each arm's body must produce a
            // value-typed result; all arms must agree on the result
            // type. Variant patterns over a JIT-eligible enum reduce
            // to a u64 tag comparison, just like the constructor
            // returns the tag.
            Expr::Match(scrutinee, arms) => {
                let scrut_ty = self.check_expr(&scrutinee)?;
                // Phase JE-3: peek the scrutinee for its enum-local info
                // so payload-binding patterns can use the per-local
                // payload_ty (resolved monomorph) instead of the layout's
                // generic placeholder.
                let scrut_enum_info = match self.program.expression.get(&scrutinee) {
                    Some(Expr::Identifier(s)) => self.compound_locals.enums.get(&s).copied(),
                    _ => None,
                };
                // All arms unify to a single type. Walk each arm's
                // pattern (rejecting unsupported shapes) and body.
                let mut result_ty: Option<ScalarTy> = None;
                for arm in &arms {
                    let payload_binding = self.check_match_pattern(&arm.pattern, scrut_ty, scrut_enum_info)?;
                    if arm.guard.is_some() {
                        self.reject(|| {
                            "JIT match arm guards are not yet supported".to_string()
                        });
                        return None;
                    }
                    // Phase JE-2b: install the payload binding (if any)
                    // for the duration of arm body checking, then remove
                    // it so subsequent arms / siblings don't see it.
                    let prev_local = if let Some((name, ty)) = payload_binding {
                        Some((name, self.locals.insert(name, ty)))
                    } else {
                        None
                    };
                    let body_ty = self.check_expr(&arm.body)?;
                    if let Some((name, prior)) = prev_local {
                        match prior {
                            Some(t) => { self.locals.insert(name, t); }
                            None => { self.locals.remove(&name); }
                        }
                    }
                    match result_ty {
                        None => result_ty = Some(body_ty),
                        Some(prev) if prev == body_ty => {}
                        Some(prev) => {
                            self.reject(|| {
                                format!(
                                    "match arms disagree on result type: {prev:?} vs {body_ty:?}"
                                )
                            });
                            return None;
                        }
                    }
                }
                result_ty
            }
            _ => unreachable!("check_match_expr was handed an expression it does not own"),
        }
    }

    /// Binary and unary operators.
    fn check_operators(&mut self, expr: Expr) -> Option<ScalarTy> {
        match expr {
            Expr::Binary(op, lhs, rhs) => {
                let lt = self.check_expr(&lhs)?;
                let rt = self.check_expr(&rhs)?;
                if lt != rt {
                    return None;
                }
                // NUM-W: narrow integers (I8/I16/I32/U8/U16/U32) accept
                // the same operator set as their I64/U64 cousins. cranelift's
                // `iadd` / `isub` / `imul` / `udiv` / `sdiv` / `urem` /
                // `srem` are all width-polymorphic on operand type, and
                // `icmp` likewise picks the right width from the operands.
                let int_like = lt.is_narrow_int()
                    || matches!(lt, ScalarTy::I64 | ScalarTy::U64);
                match op {
                    Operator::IAdd | Operator::ISub | Operator::IMul | Operator::IDiv => {
                        if int_like || lt == ScalarTy::F64 {
                            Some(lt)
                        } else {
                            None
                        }
                    }
                    Operator::IMod => {
                        // Cranelift exposes srem/urem for ints but no native f64
                        // remainder; reject f64 mod here so codegen never sees it.
                        if int_like {
                            Some(lt)
                        } else {
                            None
                        }
                    }
                    Operator::EQ | Operator::NE => {
                        if lt == ScalarTy::Unit {
                            None
                        } else {
                            Some(ScalarTy::Bool)
                        }
                    }
                    Operator::LT | Operator::LE | Operator::GT | Operator::GE => {
                        if int_like || lt == ScalarTy::F64 {
                            Some(ScalarTy::Bool)
                        } else {
                            None
                        }
                    }
                    Operator::LogicalAnd | Operator::LogicalOr => {
                        if lt == ScalarTy::Bool {
                            Some(ScalarTy::Bool)
                        } else {
                            None
                        }
                    }
                    Operator::BitwiseAnd | Operator::BitwiseOr | Operator::BitwiseXor => {
                        if int_like || lt == ScalarTy::Bool {
                            Some(lt)
                        } else {
                            None
                        }
                    }
                    Operator::LeftShift | Operator::RightShift => {
                        if int_like {
                            Some(lt)
                        } else {
                            None
                        }
                    }
                }
            }
            Expr::Unary(op, operand) => {
                let t = self.check_expr(&operand)?;
                match op {
                    UnaryOp::BitwiseNot => {
                        // NUM-W: every integer width, matching what the
                        // type checker now accepts. Cranelift's `bnot`
                        // is width-generic, so nothing downstream cares.
                        if t.is_integer() || t == ScalarTy::Bool {
                            Some(t)
                        } else {
                            None
                        }
                    }
                    UnaryOp::LogicalNot => {
                        if t == ScalarTy::Bool {
                            Some(ScalarTy::Bool)
                        } else {
                            None
                        }
                    }
                    UnaryOp::Negate => {
                        // Negation of an unsigned width is rejected at the
                        // type-check phase already. Allow the signed widths
                        // and f64 (cranelift `fneg`).
                        if t.is_signed_integer() || t == ScalarTy::F64 {
                            Some(t)
                        } else {
                            None
                        }
                    }
                    // REF-Stage-2: borrow ops are erased — eligibility
                    // simply forwards the operand type, codegen emits
                    // the operand value.
                    UnaryOp::Borrow | UnaryOp::BorrowMut => Some(t),
                }
            }
            _ => unreachable!("check_operators was handed an expression it does not own"),
        }
    }

    /// Blocks, `if` / `elif` / `else`, and assignment.
    fn check_blocks_and_assignment(&mut self, expr: Expr) -> Option<ScalarTy> {
        match expr {
            Expr::Block(stmts) => self.in_block_scope(|scope| {
                let mut last_ty = ScalarTy::Unit;
                for s in &stmts {
                    let stmt = scope.program.statement.get(s)?;
                    if let Stmt::Expression(e) = &stmt {
                        last_ty = scope.check_expr(e)?;
                    } else {
                        if !scope.check_stmt(s) {
                            return None;
                        }
                        last_ty = ScalarTy::Unit;
                    }
                }
                Some(last_ty)
            }),
            Expr::IfElifElse(cond, if_block, elif_pairs, else_block) => {
                let ct = self.check_expr(&cond)?;
                if ct != ScalarTy::Bool {
                    return None;
                }
                // Unify each branch's type via `ScalarTy::unify_branch`, which
                // treats `Never` (panic / divergence) as a wildcard — so
                // `if cond { panic("...") } else { 5i64 }` types as I64.
                let then_ty = self.check_expr(&if_block)?;
                let mut unified = then_ty;
                for (ec, eb) in &elif_pairs {
                    let et = self.check_expr(ec)?;
                    if et != ScalarTy::Bool {
                        return None;
                    }
                    let bt = self.check_expr(eb)?;
                    unified = ScalarTy::unify_branch(unified, bt)?;
                }
                let else_ty = self.check_expr(&else_block)?;
                ScalarTy::unify_branch(unified, else_ty)
            }
            Expr::Assign(lhs, rhs) => {
                // Two assignment shapes are supported:
                //   1) `name = value` for a previously declared scalar local
                //   2) `name.field = value` for a struct local's field
                let lhs_expr = self.program.expression.get(&lhs)?;
                match lhs_expr {
                    Expr::Identifier(name) => {
                        let lhs_ty = self.locals.get(&name).copied()?;
                        let rhs_ty = self.check_expr(&rhs)?;
                        if rhs_ty != lhs_ty {
                            return None;
                        }
                        Some(ScalarTy::Unit)
                    }
                    Expr::FieldAccess(receiver, field_name) => {
                        let receiver_expr = self.program.expression.get(&receiver)?;
                        let recv_name = match receiver_expr {
                            Expr::Identifier(s) => s,
                            _ => {
                                self.reject(|| {
                                    "field-assign receiver must be a struct local".to_string()
                                });
                                return None;
                            }
                        };
                        let struct_info = match self.compound_locals.structs.get(&recv_name).cloned() {
                            Some(s) => s,
                            None => {
                                self.reject(|| {
                                    "field-assign target is not a struct local".to_string()
                                });
                                return None;
                            }
                        };
                        let field_ty = self.struct_layouts
                            .get(&struct_info.base_name)
                            .and_then(|l| l.field(field_name, &struct_info.type_args))?;
                        let rhs_ty = self.check_expr(&rhs)?;
                        if rhs_ty != field_ty {
                            self.reject(|| {
                                format!(
                                    "field assign rhs type {rhs_ty:?} does not match field type {field_ty:?}"
                                )
                            });
                            return None;
                        }
                        Some(ScalarTy::Unit)
                    }
                    _ => {
                        self.reject(|| {
                            "assignment target must be an identifier or struct field".to_string()
                        });
                        None
                    }
                }
            }
            _ => unreachable!("check_blocks_and_assignment was handed an expression it does not own"),
        }
    }

    /// Free-function and associated-function calls.
    fn check_calls(&mut self, expr: Expr, expr_ref: &ExprRef) -> Option<ScalarTy> {
        match expr {
            Expr::AssociatedFunctionCall(struct_name, function_name, args) => {
                // Phase JE-2b: tuple-variant enum constructor in expression
                // position (`Status::Ok(x + x)` as a match arm body). When
                // the qualifier names a JIT-eligible enum and the variant
                // exists, validate the payload type and report the value
                // as `ScalarTy::U64` (its tag, the simple scalar lens at
                // the eligibility layer). Codegen routes the actual
                // (tag, payload) lowering through `gather_enum_values` /
                // `lower_into_enum_return`.
                if let Some(layout) = enum_layout_for(struct_name) {
                    if let Some(idx) = layout.variants.iter().position(|n| *n == function_name) {
                        if layout.variant_has_payload[idx] {
                            let payload_ty = layout.payload_ty()?;
                            if args.len() == 1 {
                                let arg_ty = self.check_expr(&args[0])?;
                                if arg_ty == payload_ty {
                                    return Some(ScalarTy::U64);
                                }
                            }
                        }
                    }
                }
                // Module-qualified call (`math::add(args)`): when the
                // qualifier doesn't refer to a struct / enum but the
                // function name lives in the (post-import) flat function
                // table, treat it as a plain `Call(function_name, args)`
                // and reuse the same eligibility logic. Real associated
                // function calls (`Container::new(args)` with `Container`
                // a struct) keep the unsupported reject path because the
                // JIT doesn't lower instance methods yet.
                if self.struct_layouts.contains_key(&struct_name)
                    || !self.program.function.iter().any(|f| f.name == function_name)
                {
                    // Differentiate the common "enum constructor" case
                    // (`Option::Some(...)`, `Result::Err(...)`, etc.)
                    // from the generic struct-associated-function reject
                    // so the verbose JIT log points at the actual blocker.
                    // Enum values aren't represented in the JIT yet —
                    // adding them would mean a `ParamTy::Enum`,
                    // `enum_locals` map, tag-dispatch codegen, etc.
                    // (essentially the AOT compiler's enum phases). The
                    // interpreter handles the call correctly via
                    // fallback; this just makes the reason precise.
                    let is_enum_qualifier = enum_decl_lookup_by_name(self.program, struct_name).is_some();
                    // Phase JE-1a: a JIT-eligible enum (non-generic,
                    // unit-variant-only) shows up in `enum_layouts`. The
                    // architecture for tag-based dispatch is in place
                    // (EnumLayout + ENUM_LAYOUTS thread-local), but
                    // constructor / match codegen hasn't landed yet —
                    // use a precise "infrastructure ready, codegen
                    // pending" message so a later JE-1b commit knows
                    // which programs to enable.
                    self.reject(|| {
                        if is_enum_qualifier {
                            if enum_layout_for(struct_name).is_some() {
                                "JIT enum support pending: unit-variant constructor codegen \
                                 (Phase JE-1b will lower this via the existing tag layout)"
                                    .to_string()
                            } else {
                                "JIT does not yet model enum values \
                                 (constructors / match / methods; see JIT-enum-1)"
                                    .to_string()
                            }
                        } else {
                            "uses unsupported expression associated function call".to_string()
                        }
                    });
                    return None;
                }
                self.check_plain_call(expr_ref, function_name, &args)
            }
            Expr::Call(name, args_ref) => {
                let args_expr = self.program.expression.get(&args_ref)?;
                let arg_list = match args_expr {
                    Expr::ExprList(v) => v,
                    _ => return None,
                };

                self.check_plain_call(expr_ref, name, &arg_list)
            }
            _ => unreachable!("check_calls was handed an expression it does not own"),
        }
    }

    /// The `__builtin_*` surface.
    fn check_builtins(&mut self, expr: Expr, expr_ref: &ExprRef) -> Option<ScalarTy> {
        match expr {
            Expr::BuiltinCall(func, args) => {
                match func {
                    // DEBUG-OBS D5: `__builtin_backtrace()` reads the
                    // shadow stack, which this JIT keeps but has no
                    // str-returning helper for. Declining sends the
                    // program to the tree-walker, which answers it —
                    // a silent fallback, like the rest of this JIT's
                    // gaps, and not an observable difference.
                    BuiltinFunction::Backtrace => {
                        self.reject(|| {
                            "__builtin_backtrace is not supported in the interpreter JIT"
                                .to_string()
                        });
                        None
                    }
                    // SIMD: this JIT's `ScalarTy` has no vector, so a
                    // program that touches one goes to the tree-walker.
                    // Same silent fallback as `dyn Trait` — a gap in
                    // coverage, not an observable difference.
                    BuiltinFunction::Simd(op) => {
                        let name = op.builtin_name();
                        self.reject(move || {
                            format!("{name} is not supported in the interpreter JIT")
                        });
                        None
                    }
                    // DATA-ORIENTED Phase 2: the `SoaVec<T>` accessors
                    // expand to one read / write per leaf of `T`, which
                    // needs the monomorphised layout this JIT does not
                    // carry. Same silent fallback as `dyn Trait` — the
                    // function runs on the tree-walker instead.
                    BuiltinFunction::SoaRead | BuiltinFunction::SoaWrite => {
                        self.reject(|| {
                            "__builtin_soa_read / __builtin_soa_write are not supported in \
                             the interpreter JIT (silently falls back)"
                                .to_string()
                        });
                        None
                    }
                    BuiltinFunction::SizeOfType(_) => {
                        // POINTER P1: the type-argument form. Resolving
                        // the written `TypeDecl` through the monomorph
                        // substitution at codegen time would need a
                        // second resolver alongside the AST's own type
                        // info, so this falls back instead — the
                        // function simply runs on the tree-walker.
                        self.reject(|| {
                            "__builtin_sizeof::<T> is not supported in JIT (silently \
                             falls back)"
                                .to_string()
                        });
                        None
                    }
                    BuiltinFunction::Panic => {
                        // `panic("literal")` is the only form the JIT can lower:
                        // the message has to be a parse-time `Expr::String(sym)`
                        // so codegen can pass the symbol id as a u64 immediate
                        // to `jit_panic`. Anything dynamic (a const, a runtime
                        // str, etc.) falls back to the interpreter where the
                        // value is already a real Object::ConstString / String.
                        //
                        // Returns `Never` (the bottom type) so that an
                        // expression-position panic — e.g. the then-branch of
                        // `if cond { panic("...") } else { 5i64 }` — unifies
                        // with the other branch's value type instead of
                        // forcing the if-expression to be Unit.
                        if args.len() != 1 {
                            self.reject(|| "panic takes 1 argument".to_string());
                            return None;
                        }
                        let arg = self.program.expression.get(&args[0])?;
                        if !matches!(arg, Expr::String(_)) {
                            self.reject(|| {
                                "panic argument must be a string literal in JIT".to_string()
                            });
                            return None;
                        }
                        Some(ScalarTy::Never)
                    }
                    BuiltinFunction::Assert => {
                        // `assert(cond, "literal")` — same constraint on the
                        // message as `panic` (literal only). The condition is a
                        // regular bool expression and is checked recursively.
                        if args.len() != 2 {
                            self.reject(|| {
                                "assert takes 2 arguments (cond, msg)".to_string()
                            });
                            return None;
                        }
                        let cond_ty = self.check_expr(&args[0])?;
                        if cond_ty != ScalarTy::Bool {
                            self.reject(|| {
                                "assert condition must be bool".to_string()
                            });
                            return None;
                        }
                        let msg_arg = self.program.expression.get(&args[1])?;
                        if !matches!(msg_arg, Expr::String(_)) {
                            self.reject(|| {
                                "assert message must be a string literal in JIT".to_string()
                            });
                            return None;
                        }
                        Some(ScalarTy::Unit)
                    }
                    // RUNTIME-LIB P0-A: this JIT's print helpers are
                    // stdout-only, so a program that writes to stderr
                    // goes to the tree-walker — a silent fallback like
                    // the rest of this JIT's gaps.
                    BuiltinFunction::EPrint | BuiltinFunction::EPrintln => {
                        self.reject(|| {
                            "eprint / eprintln are not supported in the interpreter JIT"
                                .to_string()
                        });
                        None
                    }
                    BuiltinFunction::Print | BuiltinFunction::Println => {
                        if args.len() != 1 {
                            return None;
                        }
                        // Phase JE-3: reject `println(enum_local)` —
                        // enum identifiers report as U64 (their tag) so
                        // they would otherwise type-check, but the JIT
                        // would print just the tag whereas the
                        // interpreter / AOT print the full formatted
                        // enum value. Skip so the fallback handles it.
                        if let Some(Expr::Identifier(s)) = self.program.expression.get(&args[0]) {
                            if self.compound_locals.enums.contains_key(&s) {
                                self.reject(|| {
                                    "JIT does not yet format enum values for print/println \
                                     (would print only the tag); see JIT-enum-1".to_string()
                                });
                                return None;
                            }
                        }
                        let t = self.check_expr(&args[0])?;
                        // STR-INTERP-INTERP-JIT: Str is now a supported
                        // print value via `jit_print_str` / `jit_println_str`.
                        if !matches!(
                            t,
                            ScalarTy::I64 | ScalarTy::U64 | ScalarTy::F64
                                | ScalarTy::Bool | ScalarTy::Str
                        ) && !t.is_narrow_int()
                        {
                            return None;
                        }
                        Some(ScalarTy::Unit)
                    }
                    BuiltinFunction::HeapAlloc => {
                        if !self.check_builtin_args(&[ScalarTy::U64], &args) {
                            return None;
                        }
                        Some(ScalarTy::Ptr)
                    }
                    BuiltinFunction::HeapFree => {
                        if !self.check_builtin_args(&[ScalarTy::Ptr], &args) {
                            return None;
                        }
                        Some(ScalarTy::Unit)
                    }
                    BuiltinFunction::HeapRealloc => {
                        if !self.check_builtin_args(&[ScalarTy::Ptr, ScalarTy::U64], &args) {
                            return None;
                        }
                        Some(ScalarTy::Ptr)
                    }
                    BuiltinFunction::PtrIsNull => {
                        if !self.check_builtin_args(&[ScalarTy::Ptr], &args) {
                            return None;
                        }
                        Some(ScalarTy::Bool)
                    }
                    BuiltinFunction::PtrEq => {
                        if !self.check_builtin_args(&[ScalarTy::Ptr, ScalarTy::Ptr], &args) {
                            return None;
                        }
                        Some(ScalarTy::Bool)
                    }
                    BuiltinFunction::NullPtr => {
                        if !args.is_empty() {
                            *self.reject_reason = Some(
                                "__builtin_null_ptr takes no args".to_string(),
                            );
                            return None;
                        }
                        Some(ScalarTy::Ptr)
                    }
                    BuiltinFunction::MemStat(stat) => {
                        if !args.is_empty() {
                            *self.reject_reason =
                                Some(format!("{} takes no args", stat.builtin_name()));
                            return None;
                        }
                        Some(ScalarTy::U64)
                    }
                    BuiltinFunction::StrFromBytes => {
                        // Same reason as `StrToPtr`: the interpreter JIT
                        // models str values only inside a function body,
                        // and building one is a runtime-helper call it has
                        // no signature for yet. Falls back.
                        *self.reject_reason = Some(
                            "__builtin_str_from_bytes (JIT does not build str values)".to_string(),
                        );
                        None
                    }
                    BuiltinFunction::StrToPtr => {
                        // `__builtin_str_to_ptr(s: str) -> ptr` is not yet
                        // hot-path JIT-eligible: the JIT has no `ScalarTy::Str`
                        // yet (string values aren't modelled as scalars in the
                        // JIT IR), so the call falls back to the interpreter
                        // path where the helper does its work.
                        *self.reject_reason = Some(
                            "__builtin_str_to_ptr (JIT does not yet model str scalar values)".to_string(),
                        );
                        None
                    }
                    BuiltinFunction::StrLen => {
                        *self.reject_reason = Some(
                            "__builtin_str_len (JIT does not yet model str scalar values)".to_string(),
                        );
                        None
                    }
                    BuiltinFunction::MemCopy | BuiltinFunction::MemMove => {
                        if !self.check_builtin_args(&[ScalarTy::Ptr, ScalarTy::Ptr, ScalarTy::U64], &args) {
                            return None;
                        }
                        Some(ScalarTy::Unit)
                    }
                    BuiltinFunction::MemSet => {
                        if !self.check_builtin_args(&[ScalarTy::Ptr, ScalarTy::U64, ScalarTy::U64], &args) {
                            return None;
                        }
                        Some(ScalarTy::Unit)
                    }
                    BuiltinFunction::PtrRead => {
                        // Args must be (ptr, u64). Return type is decided at the
                        // call site context — we look it up in the hint map. If
                        // the read appears in a position where eligibility never
                        // got to register a hint, fail.
                        if !self.check_builtin_args(&[ScalarTy::Ptr, ScalarTy::U64], &args) {
                            return None;
                        }
                        let resolved = self.ptr_read_hints.get(expr_ref).copied();
                        if resolved.is_none() {
                            self.reject(|| {
                                "ptr_read used outside a typed val/var/assign — JIT \
                                 needs the result type to be statically known"
                                    .to_string()
                            });
                        }
                        resolved
                    }
                    BuiltinFunction::SizeOf => {
                        if args.len() != 1 {
                            self.reject(|| {
                                format!(
                                    "__builtin_sizeof takes 1 argument, got {}",
                                    args.len()
                                )
                            });
                            return None;
                        }
                        let t = self.check_expr(&args[0])?;
                        if !matches!(
                            t,
                            ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Bool | ScalarTy::Ptr
                                | ScalarTy::F64
                        ) && !t.is_narrow_int()
                        {
                            self.reject(|| {
                                format!("__builtin_sizeof of {t:?} is not supported in JIT")
                            });
                            return None;
                        }
                        Some(ScalarTy::U64)
                    }
                    BuiltinFunction::ToString => {
                        // STR-INTERP-INTERP-JIT: __builtin_to_string(value)
                        // produces a heap-allocated str via the matching
                        // `jit_to_string_<ty>` runtime helper. Accept any
                        // scalar arg whose ScalarTy maps to a known
                        // helper — primitives only (struct / tuple / enum
                        // formatting still falls back).
                        if args.len() != 1 {
                            self.reject(|| {
                                format!("__builtin_to_string takes 1 arg, got {}", args.len())
                            });
                            return None;
                        }
                        // An enum-typed local reports its *tag* type (U64)
                        // — see the `Identifier` arm above — so the
                        // primitive check alone would accept it and
                        // format the tag instead of the value
                        // (`Option::Some(5)` prints as `1`). Reject
                        // explicitly so enum interpolation falls back to
                        // the tree-walker.
                        if let Some(Expr::Identifier(sym)) = self.program.expression.get(&args[0]) {
                            if self.compound_locals.enums.contains_key(&sym) {
                                self.reject(|| {
                                    "__builtin_to_string of an enum value is not supported in JIT \
                                     (enum interpolation falls back to the tree-walker)"
                                        .to_string()
                                });
                                return None;
                            }
                        }
                        let arg_ty = self.check_expr(&args[0])?;
                        if !matches!(
                            arg_ty,
                            ScalarTy::I64 | ScalarTy::U64 | ScalarTy::F64 | ScalarTy::Bool
                                | ScalarTy::Str | ScalarTy::I8 | ScalarTy::U8
                                | ScalarTy::I16 | ScalarTy::U16
                                | ScalarTy::I32 | ScalarTy::U32
                        ) {
                            self.reject(|| {
                                format!(
                                    "__builtin_to_string of {arg_ty:?} not supported in JIT \
                                     (only primitives lower to runtime helpers)"
                                )
                            });
                            return None;
                        }
                        Some(ScalarTy::Str)
                    }
                    BuiltinFunction::Format => {
                        // STR-INTERP-FMT: `__builtin_format(value, spec)`
                        // needs a `jit_format_<ty>` counterpart to the
                        // `jit_to_string_<ty>` helpers. Until that exists,
                        // reject so a spec-carrying interpolation falls
                        // back to the tree-walker — the compiler-side JIT
                        // and AOT both lower it natively.
                        self.reject(|| {
                            "__builtin_format (interpolation format spec) is not supported \
                             in the interpreter JIT (falls back to the tree-walker)"
                                .to_string()
                        });
                        None
                    }
                    BuiltinFunction::PtrWrite => {
                        if args.len() != 3 {
                            return None;
                        }
                        let p = self.check_expr(&args[0])?;
                        let off = self.check_expr(&args[1])?;
                        let v = self.check_expr(&args[2])?;
                        if p != ScalarTy::Ptr || off != ScalarTy::U64 {
                            return None;
                        }
                        if !matches!(
                            v,
                            ScalarTy::I64 | ScalarTy::U64 | ScalarTy::Bool | ScalarTy::Ptr
                        ) {
                            return None;
                        }
                        Some(ScalarTy::Unit)
                    }
                    BuiltinFunction::DefaultAllocator | BuiltinFunction::CurrentAllocator => {
                        if !args.is_empty() {
                            self.reject(|| {
                                format!(
                                    "{:?} expects no arguments, got {}",
                                    func,
                                    args.len()
                                )
                            });
                            return None;
                        }
                        Some(ScalarTy::Allocator)
                    }
                    BuiltinFunction::Abs => {
                        if args.len() != 1 {
                            self.reject(|| {
                                format!("abs expects 1 argument, got {}", args.len())
                            });
                            return None;
                        }
                        let t = self.check_expr(&args[0])?;
                        // Polymorphic: i64 / f64 both produce same-type
                        // result. f64 lowers to cranelift's `fabs`
                        // instruction, i64 to `select(x < 0, -x, x)`.
                        match t {
                            ScalarTy::I64 => Some(ScalarTy::I64),
                            ScalarTy::F64 => Some(ScalarTy::F64),
                            _ => {
                                self.reject(|| {
                                    "abs expects an i64 or f64 argument".to_string()
                                });
                                None
                            }
                        }
                    }
                    // NOTE: f64 math arms (Sqrt/Pow and Sin..=Ceil) lived
                    // here before Phase 4. Each is now an `extern fn
                    // __extern_*_f64` declaration whose JIT lowering goes
                    // through `try_gen_extern_call` (codegen) +
                    // `JIT_EXTERN_DISPATCH` (eligibility's extern table).
                    BuiltinFunction::Min | BuiltinFunction::Max => {
                        if args.len() != 2 {
                            let name = if matches!(func, BuiltinFunction::Min) { "min" } else { "max" };
                            self.reject(|| {
                                format!("{name} expects 2 arguments, got {}", args.len())
                            });
                            return None;
                        }
                        let a = self.check_expr(&args[0])?;
                        let b = self.check_expr(&args[1])?;
                        if !matches!(a, ScalarTy::I64 | ScalarTy::U64) || a != b {
                            let name = if matches!(func, BuiltinFunction::Min) { "min" } else { "max" };
                            self.reject(|| {
                                format!("{name} expects matching i64 or u64 operands")
                            });
                            return None;
                        }
                        Some(a)
                    }
                    // MEMORY_PROFILING M3 residual. Not JIT-eligible: the
                    // stdlib region allocator that uses it is struct-backed,
                    // so the surrounding function already falls back; this
                    // arm just says so rather than letting the match fail.
                    BuiltinFunction::RecordAllocatorLayout => {
                        self.reject(|| {
                            "record_allocator_layout is not JIT-eligible".to_string()
                        });
                        None
                    }
                    // Pointer arithmetic (interior pointers). `ptr` values
                    // are opaque to the JIT scalar model; reject and fall
                    // back to the tree-walker.
                    BuiltinFunction::PtrOffset => {
                        self.reject(|| {
                            "ptr_offset is not JIT-eligible".to_string()
                        });
                        None
                    }
                }
            }
            _ => unreachable!("check_builtins was handed an expression it does not own"),
        }
    }

    /// The `with allocator = ...` scope.
    fn check_with_scope(&mut self, expr: Expr) -> Option<ScalarTy> {
        match expr {
            Expr::With(allocator_expr, body_expr) => {
                // Validate the allocator producer first; it must yield an
                // Allocator handle.
                let alloc_ty = self.check_expr(&allocator_expr)?;
                if alloc_ty != ScalarTy::Allocator {
                    self.reject(|| {
                        "`with allocator =` requires an Allocator-typed expression".to_string()
                    });
                    return None;
                }
                // Early exits inside the body (`return` / `break` /
                // `continue`) are now supported: the codegen tracks the
                // active `with` depth and emits the matching pops before
                // each early-exit terminator.
                self.check_expr(&body_expr)
            }
            _ => unreachable!("check_with_scope was handed an expression it does not own"),
        }
    }

    /// Method calls, including the stdlib receivers.
    fn check_method_calls(&mut self, expr: Expr, expr_ref: &ExprRef) -> Option<ScalarTy> {
        match expr {
            Expr::MethodCall(receiver, method_name, args) => {
                // STR-INTERP-INTERP-JIT: `s.concat(t)` where the receiver
                // and arg are str. Codegen emits a direct call to the
                // `jit_str_concat` runtime helper. Intercept here before
                // the extension-trait lookup (which would miss it
                // because `concat` is registered as a `BuiltinMethod`
                // in the type checker, not as a stdlib trait impl).
                if Some(method_name) == concat_sym() && args.len() == 1 {
                    let recv_ty = self.check_expr(&receiver)?;
                    if matches!(recv_ty, ScalarTy::Str) {
                        let arg_ty = self.check_expr(&args[0])?;
                        if !matches!(arg_ty, ScalarTy::Str) {
                            self.reject(|| {
                                format!("str.concat argument must be str, got {arg_ty:?}")
                            });
                            return None;
                        }
                        return Some(ScalarTy::Str);
                    }
                }
                // The receiver may be:
                //   1. A struct local (existing struct-method dispatch).
                //   2. A primitive scalar local (Step C extension-trait
                //      dispatch — `i64.neg()` etc.).
                //   3. An arbitrary primitive-scalar-typed expression
                //      (#194 chained-call relaxation — `x.abs().abs()`,
                //      `make_i64().abs()`, etc.) where the receiver is
                //      not a bare identifier but type-checks to a known
                //      primitive scalar.
                // Eligibility rejects anything that doesn't reduce to
                // one of these.
                let recv_expr = self.program.expression.get(&receiver)?;
                let recv_name_opt = match recv_expr {
                    Expr::Identifier(s) => Some(s),
                    _ => None,
                };

                // Resolve the receiver's primitive type. Identifier
                // receivers consult `self.locals` directly; non-identifier
                // receivers re-enter `check_expr` so a chained
                // `MethodCall` / `Call` / `BinaryOp` etc. that returns a
                // scalar gets its scalar type back.
                let recv_prim_ty: Option<ScalarTy> = if let Some(name) = recv_name_opt {
                    self.locals.get(&name).copied()
                } else {
                    self.check_expr(&receiver)
                };

                // Step C / #194: extension-trait dispatch on a primitive
                // receiver. The receiver scalar type keys into the
                // same `find_method` lookup the struct path uses. On
                // success we register the call as a `MonoTarget::Method`
                // so the analyzer queues the method body for compilation,
                // mirroring the struct path.
                if let Some(prim_ty) = recv_prim_ty {
                    let target_sym = match primitive_target_sym_for_scalar(prim_ty) {
                        Some(s) => s,
                        None => {
                            self.reject(|| {
                                "method receiver primitive has no extension impls".to_string()
                            });
                            return None;
                        }
                    };
                    let method = match find_method(self.program, target_sym, method_name) {
                        Some(m) => m,
                        None => {
                            self.reject(|| {
                                "method not found on primitive type".to_string()
                            });
                            return None;
                        }
                    };
                    if !method.generic_params.is_empty() {
                        self.reject(|| {
                            "generic methods are not yet JIT-compatible".to_string()
                        });
                        return None;
                    }
                    if method.parameter.is_empty() {
                        self.reject(|| {
                            "method has no parameters; expected `self`".to_string()
                        });
                        return None;
                    }
                    let expected_param_count = method.parameter.len() - 1;
                    if args.len() != expected_param_count {
                        self.reject(|| {
                            format!(
                                "primitive method call has {} arg(s), expects {}",
                                args.len(),
                                expected_param_count
                            )
                        });
                        return None;
                    }
                    // Type-check each arg against the parameter, with
                    // `Self_` resolved to the receiver's primitive type.
                    for (i, arg) in args.iter().enumerate() {
                        let raw_param_td = &method.parameter[i + 1].1;
                        let resolved_param_td = match raw_param_td {
                            TypeDecl::Self_ => primitive_type_decl_for_target_sym(target_sym)
                                .unwrap_or_else(|| raw_param_td.clone()),
                            other => other.clone(),
                        };
                        let actual = self.check_expr(arg)?;
                        let want = match resolve_param_ty(&resolved_param_td, self.substitutions, self.struct_layouts) {
                            Some(ParamTy::Scalar(s)) => s,
                            _ => {
                                self.reject(|| {
                                    "primitive method parameter type unsupported".to_string()
                                });
                                return None;
                            }
                        };
                        if actual != want {
                            self.reject(|| {
                                format!("primitive method arg type mismatch: got {actual:?}, want {want:?}")
                            });
                            return None;
                        }
                    }
                    self.callees.push(MonoCall {
                        call_expr: *expr_ref,
                        target: MonoTarget::Method(target_sym, method_name),
                        mono_args: Vec::new(),
                    });
                    // Resolve the return type, with `Self_` mapping to the
                    // receiver's primitive type.
                    let ret_td = match &method.return_type {
                        Some(td) => match td {
                            TypeDecl::Self_ => primitive_type_decl_for_target_sym(target_sym)
                                .unwrap_or_else(|| td.clone()),
                            other => other.clone(),
                        },
                        None => TypeDecl::Unit,
                    };
                    return match resolve_param_ty(&ret_td, self.substitutions, self.struct_layouts) {
                        Some(ParamTy::Scalar(s)) => Some(s),
                        _ => {
                            self.reject(|| {
                                "primitive method return type unsupported".to_string()
                            });
                            None
                        }
                    };
                }

                // Struct-method dispatch: the existing path requires a
                // bare `Identifier` receiver because struct values flow
                // through `struct_locals` (per-field SSA Variables) and
                // there's no machinery to materialise a chained struct
                // value back into that representation. Reject anything
                // that didn't come through the Identifier shortcut.
                let recv_name = match recv_name_opt {
                    Some(s) => s,
                    None => {
                        self.reject(|| {
                            "non-primitive method receiver must be a local identifier".to_string()
                        });
                        return None;
                    }
                };
                // Phase JE-6: enum receiver method dispatch. When the
                // receiver is an enum local, look up the method on the
                // enum's base name. Generic methods (`impl<T>
                // Option<T>`) are instantiated by zipping the layout's
                // `generic_params` with the receiver's per-monomorph
                // type args (currently always `[payload_ty]` because
                // the JIT's single-payload-slot representation requires
                // all variants to share one scalar). Self_ resolves to
                // the enum name; the substitution map carries T from
                // the receiver.
                if let Some(enum_info) = self.compound_locals.enums.get(&recv_name).copied() {
                    let method = match find_method(self.program, enum_info.base_name, method_name) {
                        Some(m) => m,
                        None => {
                            self.reject(|| {
                                "method not found on enum".to_string()
                            });
                            return None;
                        }
                    };
                    if method.parameter.is_empty() {
                        self.reject(|| {
                            "enum method has no parameters; expected `self`".to_string()
                        });
                        return None;
                    }
                    let layout = enum_layout_for(enum_info.base_name)?;
                    // Build subst from method.generic_params. For the
                    // common single-generic-param case (impl<T>
                    // Option<T>) the method's [T] aligns with the
                    // layout's [T]; we map T -> receiver.payload_ty.
                    // For unit-only enums there's no payload_ty so
                    // no substitution is bound (acceptable when the
                    // method doesn't reference any generic param).
                    let mut method_subst: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
                    if !method.generic_params.is_empty() {
                        if layout.generic_params.is_empty() {
                            self.reject(|| {
                                "JIT enum method: generic method on non-generic enum is not yet supported".to_string()
                            });
                            return None;
                        }
                        // For multi-generic enums (Result<T, E>) the
                        // single-payload-slot constraint forces all
                        // params to bind to the same scalar. This is a
                        // strong restriction but matches the JE-4 layout.
                        let payload_ty = match enum_info.payload_ty {
                            Some(t) => t,
                            None => {
                                self.reject(|| {
                                    "JIT enum method: receiver has no payload type to bind".to_string()
                                });
                                return None;
                            }
                        };
                        for p in &method.generic_params {
                            method_subst.insert(*p, payload_ty);
                        }
                    }
                    // Merge in the receiver-context subst (rare; methods
                    // usually only see their own generics).
                    for (k, v) in self.substitutions.iter() {
                        method_subst.entry(*k).or_insert(*v);
                    }
                    let expected_param_count = method.parameter.len() - 1;
                    if args.len() != expected_param_count {
                        self.reject(|| {
                            format!(
                                "enum method call has {} arg(s), expects {}",
                                args.len(),
                                expected_param_count
                            )
                        });
                        return None;
                    }
                    // Validate each arg type against the (substituted)
                    // method param type.
                    for (i, arg) in args.iter().enumerate() {
                        let raw_param_td = &method.parameter[i + 1].1;
                        let resolved_param_td = match raw_param_td {
                            TypeDecl::Self_ => TypeDecl::Identifier(enum_info.base_name),
                            other => other.clone(),
                        };
                        let want = match resolve_param_ty(
                            &resolved_param_td, &method_subst, self.struct_layouts,
                        ) {
                            Some(ParamTy::Scalar(s)) => s,
                            _ => {
                                self.reject(|| {
                                    "JIT enum method: only scalar parameters are supported"
                                        .to_string()
                                });
                                return None;
                            }
                        };
                        let actual = self.check_expr(arg)?;
                        if actual != want {
                            self.reject(|| {
                                format!(
                                    "enum method arg type mismatch: got {actual:?}, want {want:?}"
                                )
                            });
                            return None;
                        }
                    }
                    // Build mono_args from the layout's generic_params
                    // resolved through method_subst. Empty for non-
                    // generic enums.
                    let mono_args: Vec<ScalarTy> = layout
                        .generic_params
                        .iter()
                        .filter_map(|p| method_subst.get(p).copied())
                        .collect();
                    self.callees.push(MonoCall {
                        call_expr: *expr_ref,
                        target: MonoTarget::Method(enum_info.base_name, method_name),
                        mono_args,
                    });
                    // Return type with Self_ -> enum, generics
                    // substituted.
                    match &method.return_type {
                        Some(td) => {
                            let resolved = match td {
                                TypeDecl::Self_ => TypeDecl::Identifier(enum_info.base_name),
                                other => other.clone(),
                            };
                            match resolve_param_ty(&resolved, &method_subst, self.struct_layouts) {
                                Some(ParamTy::Scalar(s)) => return Some(s),
                                Some(ParamTy::Enum { .. }) => {
                                    self.reject(|| {
                                        "JIT enum method returning enum must be the rhs of a val/var \
                                         (JE-6 expression-position scope)".to_string()
                                    });
                                    return None;
                                }
                                _ => {
                                    self.reject(|| {
                                        "enum method return type unsupported".to_string()
                                    });
                                    return None;
                                }
                            }
                        }
                        None => return Some(ScalarTy::Unit),
                    };
                }
                let struct_info = match self.compound_locals.structs.get(&recv_name).cloned() {
                    Some(s) => s,
                    None => {
                        self.reject(|| {
                            "method receiver is not a known struct local".to_string()
                        });
                        return None;
                    }
                };
                let struct_name = struct_info.base_name;
                // Validate each argument's type against the corresponding
                // method parameter (skipping `self`).
                // Linear scan over top-level ImplBlock decls is fine — only
                // run once per call site, and the analyzer already pre-built
                // a method_map for the work-stack pass.
                let method = match find_method(self.program, struct_name, method_name) {
                    Some(m) => m,
                    None => {
                        self.reject(|| {
                            "method not found on struct".to_string()
                        });
                        return None;
                    }
                };
                // #159: a method on a generic struct carries the impl
                // block's type parameters in `generic_params`. Bind them
                // from the receiver's type args; anything left unbound (a
                // method-only generic) still can't be monomorphised from
                // the call site.
                let method_subst = self.method_substitution(&struct_info, &method)?;
                let mono_args: Vec<ScalarTy> = method
                    .generic_params
                    .iter()
                    .map(|p| method_subst[p])
                    .collect();
                // The first parameter is the receiver (`self: Self` in the
                // language's preferred style). Remaining parameters must
                // line up with the explicit arguments at the call site.
                if method.parameter.is_empty() {
                    self.reject(|| {
                        "method has no parameters; expected `self`".to_string()
                    });
                    return None;
                }
                let expected_param_count = method.parameter.len() - 1;
                if args.len() != expected_param_count {
                    self.reject(|| {
                        format!(
                            "method call has {} arg(s), expects {}",
                            args.len(),
                            expected_param_count
                        )
                    });
                    return None;
                }
                for (i, arg) in args.iter().enumerate() {
                    let param_td = &method.parameter[i + 1].1;
                    let arg_expr = self.program.expression.get(arg)?;
                    if let Expr::Identifier(id) = arg_expr {
                        if let Some(arg_struct) = self.compound_locals.structs.get(&id).cloned() {
                            let want = match param_td {
                                TypeDecl::Self_ => resolve_param_ty(
                                    &self_type_decl(&struct_info), &method_subst, self.struct_layouts,
                                ),
                                other => resolve_param_ty(other, &method_subst, self.struct_layouts),
                            };
                            match want {
                                Some(ParamTy::Struct { base_name, type_args })
                                    if base_name == arg_struct.base_name
                                        && type_args == arg_struct.type_args =>
                                {
                                    continue;
                                }
                                _ => {
                                    self.reject(|| {
                                        "method struct argument type mismatch".to_string()
                                    });
                                    return None;
                                }
                            }
                        }
                    }
                    // Scalar arg path
                    let actual = self.check_expr(arg)?;
                    let want = match resolve_param_ty(param_td, &method_subst, self.struct_layouts) {
                        Some(ParamTy::Scalar(s)) => s,
                        _ => {
                            self.reject(|| {
                                "method parameter type unsupported".to_string()
                            });
                            return None;
                        }
                    };
                    if actual != want {
                        self.reject(|| {
                            format!("method arg type mismatch: got {actual:?}, want {want:?}")
                        });
                        return None;
                    }
                }
                self.callees.push(MonoCall {
                    call_expr: *expr_ref,
                    target: MonoTarget::Method(struct_name, method_name),
                    mono_args,
                });
                // Compute method's return type.
                match &method.return_type {
                    Some(td) => {
                        let resolved = match td {
                            TypeDecl::Self_ => self_type_decl(&struct_info),
                            other => other.clone(),
                        };
                        match resolve_param_ty(&resolved, &method_subst, self.struct_layouts) {
                            Some(ParamTy::Scalar(s)) => Some(s),
                            Some(ParamTy::Struct { .. }) => {
                                // Struct-returning methods only flow through
                                // val/var rhs, similar to free-function struct
                                // returns. Reject in arbitrary positions.
                                self.reject(|| {
                                    "struct-returning method must be the rhs of a val/var"
                                        .to_string()
                                });
                                None
                            }
                            Some(ParamTy::Tuple(_)) => {
                                self.reject(|| {
                                    "tuple-returning method must be the rhs of a val/var"
                                        .to_string()
                                });
                                None
                            }
                            Some(ParamTy::Enum { .. }) => {
                                self.reject(|| {
                                    "enum-returning method must be the rhs of a val/var"
                                        .to_string()
                                });
                                None
                            }
                            None => None,
                        }
                    }
                    None => Some(ScalarTy::Unit),
                }
            }
            _ => unreachable!("check_method_calls was handed an expression it does not own"),
        }
    }

    /// Struct field and tuple element reads.
    fn check_member_access(&mut self, expr: Expr) -> Option<ScalarTy> {
        match expr {
            Expr::FieldAccess(receiver, field_name) => {
                // Read access on a struct local: returns the field's scalar
                // type. Anything else (FieldAccess on a function call result,
                // nested FieldAccess, etc.) falls through to ineligible.
                let receiver_expr = self.program.expression.get(&receiver)?;
                let recv_name = match receiver_expr {
                    Expr::Identifier(s) => s,
                    _ => {
                        self.reject(|| {
                            "field access receiver must be a struct local".to_string()
                        });
                        return None;
                    }
                };
                let struct_info = match self.compound_locals.structs.get(&recv_name).cloned() {
                    Some(s) => s,
                    None => {
                        self.reject(|| {
                            "field access on a non-struct local".to_string()
                        });
                        return None;
                    }
                };
                let field_ty = self.struct_layouts
                    .get(&struct_info.base_name)
                    .and_then(|l| l.field(field_name, &struct_info.type_args));
                if field_ty.is_none() {
                    self.reject(|| "unknown field on struct".to_string());
                }
                field_ty
            }
            Expr::TupleAccess(tuple, idx) => {
                // Read access on a tuple local. The receiver must be an
                // identifier already bound to a known tuple shape; the
                // index must be in-range.
                let recv_expr = self.program.expression.get(&tuple)?;
                let recv_name = match recv_expr {
                    Expr::Identifier(s) => s,
                    _ => {
                        self.reject(|| {
                            "tuple access receiver must be a tuple local".to_string()
                        });
                        return None;
                    }
                };
                let shape = match self.compound_locals.tuples.get(&recv_name).cloned() {
                    Some(v) => v,
                    None => {
                        self.reject(|| {
                            "tuple access on a non-tuple local".to_string()
                        });
                        return None;
                    }
                };
                if idx >= shape.len() {
                    self.reject(|| {
                        format!("tuple index {idx} out of bounds for shape {shape:?}")
                    });
                    return None;
                }
                Some(shape[idx])
            }
            _ => unreachable!("check_member_access was handed an expression it does not own"),
        }
    }

    /// `as` casts between the scalar widths.
    fn check_casts(&mut self, expr: Expr) -> Option<ScalarTy> {
        match expr {
            Expr::Cast(inner, target) => {
                // Casts allowed: any-width int ↔ any-width int (sextend /
                // uextend / ireduce in codegen), and i64/u64 ↔ f64 (real
                // fcvt instructions). bool casts are intentionally
                // excluded.
                let inner_ty = self.check_expr(&inner)?;
                let target_ty = ScalarTy::from_type_decl(&target)?;
                let int_or_float = |t: ScalarTy| {
                    matches!(t, ScalarTy::I64 | ScalarTy::U64 | ScalarTy::F64) || t.is_narrow_int()
                };
                if !int_or_float(inner_ty) {
                    return None;
                }
                if !int_or_float(target_ty) {
                    return None;
                }
                // Reject narrow-int <-> f64 for now: those require fcvt
                // pre-extending the narrow side, which the cast codegen
                // doesn't yet handle. Stay safe and fall back.
                let narrow_to_float = inner_ty.is_narrow_int() && target_ty == ScalarTy::F64;
                let float_to_narrow = inner_ty == ScalarTy::F64 && target_ty.is_narrow_int();
                if narrow_to_float || float_to_narrow {
                    self.reject(|| {
                        "JIT cast between narrow int and f64 is not yet supported".to_string()
                    });
                    return None;
                }
                Some(target_ty)
            }
            // Everything else is unsupported in this iteration.
            _ => unreachable!("check_casts was handed an expression it does not own"),
        }
    }


    /// Shared eligibility check for plain function calls. Used by both
    /// `Expr::Call(name, ExprList(args))` (the bare-name form) and
    /// `Expr::AssociatedFunctionCall(module, name, args)` (the
    /// module-qualified form, after the module-alias guard has confirmed
    /// the qualifier doesn't refer to a struct). Locates the callee in
    /// the program's function table, type-checks each argument against
    /// the matching parameter (handling identifier-of-struct, identifier-
    /// of-tuple-local, inline tuple literal, and scalar fall-throughs),
    /// records the monomorphisation key, and returns the substituted
    /// callee return type.
    fn check_plain_call(
        &mut self,
        expr_ref: &ExprRef,
        name: DefaultSymbol,
        arg_list: &Vec<ExprRef>,
    ) -> Option<ScalarTy> {
        // Locate the callee in the self.program's function table.
        let callee = self.program.function.iter().find(|f| f.name == name).cloned();
        let callee = match callee {
            Some(f) => f,
            None => {
                self.reject(|| "calls an unknown function".to_string());
                return None;
            }
        };

        // `extern fn` self.callees: validate against the JIT extern dispatch
        // table. The table is keyed by interned symbol via the
        // thread-local set up by `analyze` (see `with_extern_dispatch`).
        // Names that aren't in the table fall back to the interpreter
        // call path.
        if callee.is_extern {
            let entry = match jit_extern_dispatch_for(name) {
                Some(e) => e,
                None => {
                    self.reject(|| {
                        "calls extern fn not registered with the JIT".to_string()
                    });
                    return None;
                }
            };
            if arg_list.len() != entry.params.len() {
                self.reject(|| {
                    format!(
                        "extern fn called with {} arg(s), expected {}",
                        arg_list.len(),
                        entry.params.len()
                    )
                });
                return None;
            }
            for (a, want) in arg_list.iter().zip(entry.params.iter()) {
                match self.check_expr(a) {
                    Some(t) if t == *want => {}
                    _ => {
                        self.reject(|| {
                            "extern fn argument type mismatch".to_string()
                        });
                        return None;
                    }
                }
            }
            // Extern fns skip the monomorphisation pipeline; codegen
            // recognises the `is_extern` flag and emits the matching
            // helper / native op.
            return Some(entry.ret);
        }

        // Resolve each argument's type, allowing struct identifiers
        // when the callee's parameter at that position is a struct.
        // Generic self.substitutions are inferred only from scalar args;
        // generic-over-struct functions aren't supported in this
        // iteration.
        if arg_list.len() != callee.parameter.len() {
            self.reject(|| {
                format!(
                    "call has {} arg(s), callee expects {}",
                    arg_list.len(),
                    callee.parameter.len()
                )
            });
            return None;
        }
        let mut scalar_arg_tys: Vec<ScalarTy> = Vec::with_capacity(arg_list.len());
        let mut callee_param_tys: Vec<ParamTy> = Vec::with_capacity(arg_list.len());
        // #159: generics the callee picks up through a struct-typed
        // parameter. Seeded into `infer_substitutions`, which only looks at
        // scalar argument positions.
        let mut struct_arg_bindings: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
        for (a, (_, param_td)) in arg_list.iter().zip(callee.parameter.iter()) {
            let arg_expr = self.program.expression.get(a)?;
            // Phase JE-2d: enum-typed argument matching. Either the
            // arg is a direct identifier of an enum local, or it's a
            // unit / tuple constructor for the matching enum (the same
            // shapes the boundary expansion supports).
            if let Some(ParamTy::Enum { base_name: want_enum, payload_ty: want_payload }) =
                resolve_param_ty(param_td, self.substitutions, self.struct_layouts)
            {
                // Identifier of an enum local.
                if let Expr::Identifier(id) = arg_expr {
                    if let Some(local_info) = self.compound_locals.enums.get(&id).copied() {
                        if local_info.base_name != want_enum {
                            self.reject(|| {
                                "enum argument's type does not match callee parameter".to_string()
                            });
                            return None;
                        }
                        // JE-5: per-monomorph payload type must agree
                        // (e.g. you can't pass `Opt<i64>` to a `Opt<u64>`
                        // parameter — both are `Opt` but the cranelift
                        // payload widths differ).
                        if local_info.payload_ty != want_payload {
                            self.reject(|| {
                                "enum argument's monomorph payload type does not match \
                                 callee parameter".to_string()
                            });
                            return None;
                        }
                        callee_param_tys.push(ParamTy::Enum {
                            base_name: local_info.base_name,
                            payload_ty: local_info.payload_ty,
                        });
                        scalar_arg_tys.push(ScalarTy::Unit);
                        continue;
                    }
                }
                // Inline unit / tuple constructor — reuse the
                // val/var rhs helper to validate the variant + payload.
                // Build a synthetic annotation hint from the param type
                // so generic-enum unit constructors can resolve T.
                if let Some(info) = self.check_enum_constructor_rhs(a, Some(param_td)) {
                    if info.payload_ty != want_payload {
                        self.reject(|| {
                            "inline enum constructor's payload monomorph does not match \
                             callee parameter".to_string()
                        });
                        return None;
                    }
                    callee_param_tys.push(ParamTy::Enum {
                        base_name: want_enum,
                        payload_ty: want_payload,
                    });
                    scalar_arg_tys.push(ScalarTy::Unit);
                    continue;
                }
                self.reject(|| {
                    "enum argument must be a local identifier or a constructor for the matching enum"
                        .to_string()
                });
                return None;
            }
            if let Expr::Identifier(id) = arg_expr {
                if let Some(arg_struct) = self.compound_locals.structs.get(&id).cloned() {
                    match param_td {
                        TypeDecl::Identifier(s) | TypeDecl::Struct(s, _)
                            if *s == arg_struct.base_name && self.struct_layouts.contains_key(s) =>
                        {
                            // #159: the parameter's own type arguments
                            // (`Cell<T>` / `Cell<u64>`) line up positionally
                            // with the argument's. A callee generic in that
                            // position is inferred from the argument — this
                            // is the only place a generic can be bound
                            // through a struct, since `infer_substitutions`
                            // sees struct args as opaque.
                            let declared_args: &[TypeDecl] = match param_td {
                                TypeDecl::Struct(_, a) => a,
                                _ => &[],
                            };
                            if declared_args.len() != arg_struct.type_args.len() {
                                self.reject(|| {
                                    "struct argument's type arguments do not match callee \
                                     parameter"
                                        .to_string()
                                });
                                return None;
                            }
                            for (declared, actual) in
                                declared_args.iter().zip(arg_struct.type_args.iter())
                            {
                                let is_callee_generic = match declared {
                                    TypeDecl::Generic(g) | TypeDecl::Identifier(g) => {
                                        callee.generic_params.contains(g).then_some(*g)
                                    }
                                    _ => None,
                                };
                                match is_callee_generic {
                                    Some(g) => {
                                        if let Some(prev) =
                                            struct_arg_bindings.insert(g, *actual)
                                        {
                                            if prev != *actual {
                                                self.reject(|| {
                                                    "struct arguments bind one type parameter \
                                                     to conflicting types"
                                                        .to_string()
                                                });
                                                return None;
                                            }
                                        }
                                    }
                                    None => {
                                        if substitute_to_scalar(declared, self.substitutions)
                                            != Some(*actual)
                                        {
                                            self.reject(|| {
                                                "struct argument's type arguments do not match \
                                                 callee parameter"
                                                    .to_string()
                                            });
                                            return None;
                                        }
                                    }
                                }
                            }
                            callee_param_tys.push(ParamTy::Struct {
                                base_name: arg_struct.base_name,
                                type_args: arg_struct.type_args.clone(),
                            });
                            scalar_arg_tys.push(ScalarTy::Unit);
                            continue;
                        }
                        _ => {
                            self.reject(|| {
                                "struct argument's type does not match callee parameter"
                                    .to_string()
                            });
                            return None;
                        }
                    }
                }
                if let Some(shape) = self.compound_locals.tuples.get(&id).cloned() {
                    let want = match resolve_param_ty(param_td, self.substitutions, self.struct_layouts) {
                        Some(ParamTy::Tuple(ts)) => ts,
                        _ => {
                            self.reject(|| {
                                "tuple argument's type does not match callee parameter".to_string()
                            });
                            return None;
                        }
                    };
                    if want != shape {
                        self.reject(|| {
                            "tuple argument shape does not match callee parameter".to_string()
                        });
                        return None;
                    }
                    callee_param_tys.push(ParamTy::Tuple(shape));
                    scalar_arg_tys.push(ScalarTy::Unit);
                    continue;
                }
            }
            if let Expr::TupleLiteral(elements) = arg_expr {
                let want = match resolve_param_ty(param_td, self.substitutions, self.struct_layouts) {
                    Some(ParamTy::Tuple(ts)) => ts,
                    _ => {
                        self.reject(|| {
                            "inline tuple literal argument needs a tuple parameter".to_string()
                        });
                        return None;
                    }
                };
                if elements.len() != want.len() {
                    self.reject(|| {
                        "inline tuple literal argument arity does not match callee parameter".to_string()
                    });
                    return None;
                }
                let mut shape: Vec<ScalarTy> = Vec::with_capacity(elements.len());
                for e in &elements {
                    let t = self.check_expr(e)?;
                    shape.push(t);
                }
                if shape != want {
                    self.reject(|| {
                        "inline tuple literal argument element types do not match callee parameter"
                            .to_string()
                    });
                    return None;
                }
                callee_param_tys.push(ParamTy::Tuple(shape));
                scalar_arg_tys.push(ScalarTy::Unit);
                continue;
            }
            let t = self.check_expr(a)?;
            scalar_arg_tys.push(t);
            callee_param_tys.push(ParamTy::Scalar(t));
        }

        let callee_subs = infer_substitutions(
            &callee, &scalar_arg_tys, self.substitutions, struct_arg_bindings, self.reject_reason,
        )?;

        let mono_args: Vec<ScalarTy> = callee
            .generic_params
            .iter()
            .map(|g| callee_subs.get(g).copied().unwrap_or(ScalarTy::Unit))
            .collect();
        self.callees.push(MonoCall {
            call_expr: *expr_ref,
            target: MonoTarget::Function(name),
            mono_args,
        });

        let _ = callee_param_tys;

        match &callee.return_type {
            Some(td) => substitute_to_scalar(td, &callee_subs),
            None => Some(ScalarTy::Unit),
        }
    }
}

// ---------------------------------------------------------------------
// Helpers that read the AST and nothing else. They stay free functions
// because `check_callable_body` needs `body_has_ptr_read` before it has
// a `Checker` to ask, and because a pure walk that cannot reject or
// learn anything is easier to trust when it visibly cannot reach the
// checker's state.
// ---------------------------------------------------------------------

/// #159: recover a generic struct's type arguments from the scalar
/// types its literal's field initializers produced. Non-generic
/// structs short-circuit to an empty vec.
fn infer_struct_type_args(
    layout: &StructLayout,
    actual_fields: &[(DefaultSymbol, ScalarTy)],
    reject_reason: &mut Option<String>,
) -> Option<Vec<ScalarTy>> {
    if !layout.is_generic() {
        return Some(Vec::new());
    }
    let mut bound: HashMap<DefaultSymbol, ScalarTy> = HashMap::new();
    for (field_sym, repr) in &layout.fields {
        let FieldRepr::Generic(param) = repr else {
            continue;
        };
        let Some((_, actual)) = actual_fields.iter().find(|(n, _)| n == field_sym) else {
            continue;
        };
        if let Some(prev) = bound.insert(*param, *actual) {
            if prev != *actual {
                note(reject_reason, || {
                    format!(
                        "struct literal binds one type parameter to conflicting types \
                         {prev:?} and {actual:?}"
                    )
                });
                return None;
            }
        }
    }
    let mut out = Vec::with_capacity(layout.generic_params.len());
    for p in &layout.generic_params {
        match bound.get(p) {
            Some(t) => out.push(*t),
            None => {
                note(reject_reason, || {
                    "cannot infer the struct literal's type arguments; add a type \
                     annotation"
                        .to_string()
                });
                return None;
            }
        }
    }
    Some(out)
}

/// Quick syntactic walk to detect any PtrRead within a function body.
fn body_has_ptr_read(program: &File, stmt_ref: &StmtRef) -> bool {
    let mut found = false;
    walk_stmt_for_ptr_read(program, stmt_ref, &mut found);
    found
}

/// Returns true if `name` matches any top-level `enum` declaration
/// in the program. Used by the AssociatedFunctionCall reject path so
/// it can distinguish enum constructors (`Option::Some(...)`) from
/// other unsupported associated calls and report a precise reason.
fn enum_decl_lookup_by_name(
    program: &File,
    name: DefaultSymbol,
) -> Option<()> {
    for i in 0..program.statement.len() {
        if let Some(Stmt::EnumDecl { name: n, .. }) = program.statement.get(&StmtRef(i as u32)) {
            if n == name {
                return Some(());
            }
        }
    }
    None
}

fn walk_stmt_for_ptr_read(program: &File, stmt_ref: &StmtRef, found: &mut bool) {
    if *found {
        return;
    }
    let Some(stmt) = program.statement.get(stmt_ref) else {
        return;
    };
    match stmt {
        Stmt::Expression(e) => walk_expr_for_ptr_read(program, &e, found),
        Stmt::Val(_, _, e) => walk_expr_for_ptr_read(program, &e, found),
        Stmt::Var(_, _, Some(e)) => walk_expr_for_ptr_read(program, &e, found),
        Stmt::Return(Some(e)) => walk_expr_for_ptr_read(program, &e, found),
        Stmt::For(_, _, s, e, body) => {
            walk_expr_for_ptr_read(program, &s, found);
            walk_expr_for_ptr_read(program, &e, found);
            walk_expr_for_ptr_read(program, &body, found);
        }
        Stmt::While(_, c, body) => {
            walk_expr_for_ptr_read(program, &c, found);
            walk_expr_for_ptr_read(program, &body, found);
        }
        _ => {}
    }
}

fn walk_expr_for_ptr_read(program: &File, expr_ref: &ExprRef, found: &mut bool) {
    if *found {
        return;
    }
    let Some(expr) = program.expression.get(expr_ref) else {
        return;
    };
    match expr {
        Expr::BuiltinCall(BuiltinFunction::PtrRead, _) => *found = true,
        Expr::Block(stmts) => {
            for s in &stmts {
                walk_stmt_for_ptr_read(program, s, found);
            }
        }
        Expr::Binary(_, l, r) | Expr::Assign(l, r) | Expr::Range(l, r) => {
            walk_expr_for_ptr_read(program, &l, found);
            walk_expr_for_ptr_read(program, &r, found);
        }
        Expr::Unary(_, e) | Expr::Cast(e, _) => {
            walk_expr_for_ptr_read(program, &e, found);
        }
        Expr::IfElifElse(c, t, elifs, el) => {
            walk_expr_for_ptr_read(program, &c, found);
            walk_expr_for_ptr_read(program, &t, found);
            for (ec, eb) in &elifs {
                walk_expr_for_ptr_read(program, ec, found);
                walk_expr_for_ptr_read(program, eb, found);
            }
            walk_expr_for_ptr_read(program, &el, found);
        }
        Expr::Call(_, args) => walk_expr_for_ptr_read(program, &args, found),
        Expr::ExprList(es) | Expr::ArrayLiteral(es) | Expr::TupleLiteral(es) => {
            for e in &es {
                walk_expr_for_ptr_read(program, e, found);
            }
        }
        Expr::BuiltinCall(_, args) => {
            for a in &args {
                walk_expr_for_ptr_read(program, a, found);
            }
        }
        _ => {}
    }
}

