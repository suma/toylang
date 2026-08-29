use std::rc::Rc;
use string_interner::DefaultSymbol;
use crate::type_checker::SourceLocation;
use crate::type_decl::TypeDecl;
use super::{
    Expr, Stmt, ExprPool, StmtPool, LocationPool,
    ExprRef, StmtRef,
    Operator, UnaryOp, SliceInfo,
    BuiltinMethod, BuiltinFunction,
    StructField, Visibility, MethodFunction,
};

pub struct AstBuilder {
    pub expr_pool: ExprPool,
    pub stmt_pool: StmtPool,
    pub location_pool: LocationPool,
}

impl Default for AstBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AstBuilder {
    pub fn new() -> Self {
        AstBuilder {
            expr_pool: ExprPool::new(),
            stmt_pool: StmtPool::new(),
            location_pool: LocationPool::new(),
        }
    }

    pub fn with_capacity(expr_cap: usize, stmt_cap: usize) -> Self {
        AstBuilder {
            expr_pool: ExprPool::with_capacity(expr_cap),
            stmt_pool: StmtPool::with_capacity(stmt_cap),
            location_pool: LocationPool::with_capacity(expr_cap, stmt_cap),
        }
    }

    // Legacy methods for compatibility
    pub fn add_expr(&mut self, expr: Expr) -> ExprRef {
        let expr_ref = self.expr_pool.add(expr);
        self.location_pool.add_expr_location(None);
        expr_ref
    }

    pub fn add_stmt(&mut self, stmt: Stmt) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(stmt);
        self.location_pool.add_stmt_location(None);
        stmt_ref
    }

    // New methods with location support
    pub fn add_expr_with_location(&mut self, expr: Expr, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(expr);
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn add_stmt_with_location(&mut self, stmt: Stmt, location: Option<SourceLocation>) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(stmt);
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }

    pub fn get_expr_pool(&self) -> &ExprPool {
        &self.expr_pool
    }

    pub fn get_stmt_pool(&self) -> &StmtPool {
        &self.stmt_pool
    }

    pub fn get_expr_pool_mut(&mut self) -> &mut ExprPool {
        &mut self.expr_pool
    }

    pub fn get_stmt_pool_mut(&mut self) -> &mut StmtPool {
        &mut self.stmt_pool
    }

    pub fn get_location_pool(&self) -> &LocationPool {
        &self.location_pool
    }

    pub fn get_location_pool_mut(&mut self) -> &mut LocationPool {
        &mut self.location_pool
    }

    pub fn extract_pools(self) -> (ExprPool, StmtPool, LocationPool) {
        (self.expr_pool, self.stmt_pool, self.location_pool)
    }

    // --- Variants that need custom body (not macro-friendly) ---

    pub fn call_expr(&mut self, fn_name: DefaultSymbol, args: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let args_ref = self.expr_pool.add(Expr::ExprList(args));
        // DEBUG-OBS D1: the argument list carries the call's own
        // location, not `None`. `evaluate_function_call` reads the
        // frame's call site off this node, so the `None` that used to
        // be pushed here made `(called at line N)` in the backtrace
        // unreachable code — a rendering branch that could never fire
        // (`DEBUG_OBSERVABILITY.md` 実測 4).
        self.location_pool.add_expr_location(location);
        let expr_ref = self.expr_pool.add(Expr::Call(fn_name, args_ref));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn expr_list(&mut self, exprs: Vec<ExprRef>, location: Option<SourceLocation>) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::ExprList(exprs));
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn closure_expr(
        &mut self,
        params: crate::ast::ParameterList,
        return_type: Option<crate::type_decl::TypeDecl>,
        body: ExprRef,
        location: Option<SourceLocation>,
    ) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Closure {
            params,
            return_type,
            body,
            // CLOSURE-CAPTURE E3: the parser cannot know whether the
            // closure outlives its captures; the type checker decides.
            captures_by_ref: false,
        });
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn try_expr(
        &mut self,
        inner: ExprRef,
        scrutinee_binding: DefaultSymbol,
        success_binding: DefaultSymbol,
        error_binding: DefaultSymbol,
        panic_msg: DefaultSymbol,
        converted_binding: DefaultSymbol,
        result_binding: DefaultSymbol,
        location: Option<SourceLocation>,
    ) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::Try {
            inner,
            scrutinee_binding,
            success_binding,
            error_binding,
            panic_msg,
            converted_binding,
            result_binding,
        });
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    /// `a ?? b` — null-coalesce. Synthetic binding symbols are
    /// pre-interned by the parser; see `Expr::NullCoalesce`.
    pub fn null_coalesce_expr(
        &mut self,
        lhs: ExprRef,
        rhs: ExprRef,
        scrutinee_binding: DefaultSymbol,
        success_binding: DefaultSymbol,
        error_binding: DefaultSymbol,
        location: Option<SourceLocation>,
    ) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::NullCoalesce {
            lhs,
            rhs,
            scrutinee_binding,
            success_binding,
            error_binding,
        });
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    pub fn struct_update_expr(
        &mut self,
        type_name: DefaultSymbol,
        fields: Vec<(DefaultSymbol, ExprRef)>,
        base: ExprRef,
        base_binding: DefaultSymbol,
        location: Option<SourceLocation>,
    ) -> ExprRef {
        let expr_ref = self.expr_pool.add(Expr::StructUpdate {
            type_name,
            fields,
            base,
            base_binding,
        });
        self.location_pool.add_expr_location(location);
        expr_ref
    }

    // --- Complex statement builders that need custom body ---

    pub fn struct_decl_stmt(
        &mut self,
        name: DefaultSymbol,
        generic_params: Vec<DefaultSymbol>,
        generic_bounds: std::collections::HashMap<DefaultSymbol, crate::type_decl::TypeDecl>,
        fields: Vec<StructField>,
        visibility: Visibility,
        location: Option<SourceLocation>,
    ) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::StructDecl {
            name,
            generic_params,
            generic_bounds,
            fields,
            visibility,
        });
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }

    pub fn impl_block_stmt(&mut self, target_type: DefaultSymbol, methods: Vec<Rc<MethodFunction>>, location: Option<SourceLocation>) -> StmtRef {
        self.impl_block_stmt_with_trait(target_type, Vec::new(), methods, None, location)
    }

    pub fn impl_block_stmt_with_trait(
        &mut self,
        target_type: DefaultSymbol,
        target_type_args: Vec<crate::type_decl::TypeDecl>,
        methods: Vec<Rc<MethodFunction>>,
        trait_name: Option<DefaultSymbol>,
        location: Option<SourceLocation>,
    ) -> StmtRef {
        // Default no trait_type_args; callers that supply concrete
        // generic-trait args go through the explicit-args helper.
        self.impl_block_stmt_with_trait_args(
            target_type,
            target_type_args,
            methods,
            trait_name,
            Vec::new(),
            location,
        )
    }

    /// ITER-PROTOCOL-TRAIT: full-shape builder used when the trait
    /// itself carries concrete type args at the impl site
    /// (`impl Iterator<i64> for Counter` → `trait_type_args = [i64]`).
    pub fn impl_block_stmt_with_trait_args(
        &mut self,
        target_type: DefaultSymbol,
        target_type_args: Vec<crate::type_decl::TypeDecl>,
        methods: Vec<Rc<MethodFunction>>,
        trait_name: Option<DefaultSymbol>,
        trait_type_args: Vec<crate::type_decl::TypeDecl>,
        location: Option<SourceLocation>,
    ) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::ImplBlock {
            target_type,
            target_type_args,
            methods,
            trait_name,
            trait_type_args,
        });
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }

    pub fn trait_decl_stmt(
        &mut self,
        name: DefaultSymbol,
        methods: Vec<crate::ast::TraitMethodSignature>,
        visibility: Visibility,
        location: Option<SourceLocation>,
    ) -> StmtRef {
        // Backward compat for non-generic traits: empty generic_params.
        self.trait_decl_stmt_with_generics(name, Vec::new(), methods, visibility, location)
    }

    /// ITER-PROTOCOL-TRAIT: generic-aware trait declaration builder.
    /// `generic_params` carries the symbol names introduced by
    /// `trait Foo<T, U, ...>`; each occurrence of `T` / `U` in a
    /// method signature shows up as `TypeDecl::Generic(P)` (the
    /// existing convention for struct / function generics).
    pub fn trait_decl_stmt_with_generics(
        &mut self,
        name: DefaultSymbol,
        generic_params: Vec<DefaultSymbol>,
        methods: Vec<crate::ast::TraitMethodSignature>,
        visibility: Visibility,
        location: Option<SourceLocation>,
    ) -> StmtRef {
        let stmt_ref = self.stmt_pool.add(Stmt::TraitDecl {
            name,
            generic_params,
            methods,
            visibility,
        });
        self.location_pool.add_stmt_location(location);
        stmt_ref
    }
}

// ------------------------------------------------------------------
//  Declarative macros for boilerplate-free builder methods
//  Defined at module level so they are visible to other crates,
//  but invoked inside the `impl AstBuilder` block above.
// ------------------------------------------------------------------

/// Unit variant expression (no payload): `Expr::True`, `Expr::Null`, …
macro_rules! unit_expr_builder {
    ($method:ident, $variant:ident) => {
        pub fn $method(&mut self, location: Option<SourceLocation>) -> ExprRef {
            let expr_ref = self.expr_pool.add(Expr::$variant);
            self.location_pool.add_expr_location(location);
            expr_ref
        }
    };
}

/// Single-argument tuple-variant expression: `Expr::UInt64(v)`, …
macro_rules! simple_expr_builder {
    ($method:ident, $variant:ident, $ty:ty) => {
        pub fn $method(&mut self, value: $ty, location: Option<SourceLocation>) -> ExprRef {
            let expr_ref = self.expr_pool.add(Expr::$variant(value));
            self.location_pool.add_expr_location(location);
            expr_ref
        }
    };
}

/// Multi-argument expression where every parameter is forwarded in
/// the same order to the `Expr` variant.
macro_rules! multi_arg_expr_builder {
    ($method:ident, $variant:ident, $($param:ident: $ty:ty),+ $(,)?) => {
        pub fn $method(&mut self, $($param: $ty,)+ location: Option<SourceLocation>) -> ExprRef {
            let expr_ref = self.expr_pool.add(Expr::$variant($($param),+));
            self.location_pool.add_expr_location(location);
            expr_ref
        }
    };
}

/// Single-argument tuple-variant statement: `Stmt::Expression(e)`, …
macro_rules! simple_stmt_builder {
    ($method:ident, $variant:ident, $ty:ty) => {
        pub fn $method(&mut self, value: $ty, location: Option<SourceLocation>) -> StmtRef {
            let stmt_ref = self.stmt_pool.add(Stmt::$variant(value));
            self.location_pool.add_stmt_location(location);
            stmt_ref
        }
    };
}

/// Multi-argument statement where every parameter is forwarded in
/// the same order to the `Stmt` variant.
macro_rules! multi_arg_stmt_builder {
    ($method:ident, $variant:ident, $($param:ident: $ty:ty),+ $(,)?) => {
        pub fn $method(&mut self, $($param: $ty,)+ location: Option<SourceLocation>) -> StmtRef {
            let stmt_ref = self.stmt_pool.add(Stmt::$variant($($param),+));
            self.location_pool.add_stmt_location(location);
            stmt_ref
        }
    };
}

// Re-invoke the macros to generate the methods inside the impl block.
// These must appear *after* the `impl AstBuilder { ... }` block and
// *after* the macro definitions so the compiler sees them.
// NOTE: This pattern generates items at module scope; we therefore
// wrap the generated methods in a second `impl AstBuilder` block.
impl AstBuilder {
    // --- Literal / unit variants ---
    simple_expr_builder!(uint64_expr, UInt64, u64);
    simple_expr_builder!(int64_expr,  Int64,  i64);
    simple_expr_builder!(float64_expr, Float64, f64);
    simple_expr_builder!(float32_expr, Float32, f32);

    // NUM-W narrow-integer literal builders. Same shape as
    // int64_expr / uint64_expr; the parser hands the lexer-validated
    // value straight into the pool.
    simple_expr_builder!(int8_expr,   Int8,   i8);
    simple_expr_builder!(int16_expr,  Int16,  i16);
    simple_expr_builder!(int32_expr,  Int32,  i32);
    simple_expr_builder!(uint8_expr,  UInt8,  u8);
    simple_expr_builder!(uint16_expr, UInt16, u16);
    simple_expr_builder!(uint32_expr, UInt32, u32);

    unit_expr_builder!(bool_true_expr,  True);
    unit_expr_builder!(bool_false_expr, False);
    unit_expr_builder!(null_expr,       Null);

    // --- Single-argument identifier-like variants ---
    simple_expr_builder!(identifier_expr, Identifier, DefaultSymbol);
    simple_expr_builder!(string_expr,     String,     DefaultSymbol);
    simple_expr_builder!(number_expr,     Number,     DefaultSymbol);

    // --- Multi-argument expression variants ---
    multi_arg_expr_builder!(binary_expr, Binary, op: Operator, lhs: ExprRef, rhs: ExprRef);
    multi_arg_expr_builder!(unary_expr, Unary, op: UnaryOp, operand: ExprRef);
    multi_arg_expr_builder!(assign_expr, Assign, lhs: ExprRef, rhs: ExprRef);
    multi_arg_expr_builder!(if_elif_else_expr, IfElifElse, cond: ExprRef, if_block: ExprRef, elif_pairs: Vec<(ExprRef, ExprRef)>, else_block: ExprRef);
    multi_arg_expr_builder!(block_expr, Block, statements: Vec<StmtRef>);
    multi_arg_expr_builder!(array_literal_expr, ArrayLiteral, elements: Vec<ExprRef>);
    multi_arg_expr_builder!(slice_assign_expr, SliceAssign, object: ExprRef, start: Option<ExprRef>, end: Option<ExprRef>, value: ExprRef);
    multi_arg_expr_builder!(associated_function_call_expr, AssociatedFunctionCall, struct_name: DefaultSymbol, function_name: DefaultSymbol, args: Vec<ExprRef>);
    multi_arg_expr_builder!(slice_access_expr, SliceAccess, object: ExprRef, slice_info: SliceInfo);
    multi_arg_expr_builder!(dict_literal_expr, DictLiteral, entries: Vec<(ExprRef, ExprRef)>);
    multi_arg_expr_builder!(tuple_literal_expr, TupleLiteral, elements: Vec<ExprRef>);
    multi_arg_expr_builder!(tuple_access_expr, TupleAccess, tuple: ExprRef, index: usize);
    multi_arg_expr_builder!(cast_expr, Cast, expr: ExprRef, target_type: TypeDecl);
    multi_arg_expr_builder!(with_expr, With, allocator: ExprRef, body: ExprRef);
    multi_arg_expr_builder!(field_access_expr, FieldAccess, object: ExprRef, field: DefaultSymbol);
    multi_arg_expr_builder!(method_call_expr, MethodCall, object: ExprRef, method: DefaultSymbol, args: Vec<ExprRef>);
    multi_arg_expr_builder!(struct_literal_expr, StructLiteral, type_name: DefaultSymbol, fields: Vec<(DefaultSymbol, ExprRef)>);
    multi_arg_expr_builder!(qualified_identifier_expr, QualifiedIdentifier, path: Vec<DefaultSymbol>);
    multi_arg_expr_builder!(builtin_method_call_expr, BuiltinMethodCall, receiver: ExprRef, method: BuiltinMethod, args: Vec<ExprRef>);
    multi_arg_expr_builder!(builtin_call_expr, BuiltinCall, func: BuiltinFunction, args: Vec<ExprRef>);

    // ------------------------------------------------------------------
    //  Statement builders (generated via macros where possible)
    // ------------------------------------------------------------------

    simple_stmt_builder!(expression_stmt, Expression, ExprRef);
    simple_stmt_builder!(return_stmt, Return, Option<ExprRef>);

    multi_arg_stmt_builder!(val_stmt, Val, name: DefaultSymbol, type_decl: Option<TypeDecl>, value: ExprRef);
    multi_arg_stmt_builder!(var_stmt, Var, name: DefaultSymbol, type_decl: Option<TypeDecl>, value: Option<ExprRef>);
    multi_arg_stmt_builder!(break_stmt_with_label, Break, label: Option<DefaultSymbol>);
    multi_arg_stmt_builder!(continue_stmt_with_label, Continue, label: Option<DefaultSymbol>);
    multi_arg_stmt_builder!(for_stmt_with_label, For, label: Option<DefaultSymbol>, var: DefaultSymbol, start: ExprRef, end: ExprRef, block: ExprRef);
    multi_arg_stmt_builder!(while_stmt_with_label, While, label: Option<DefaultSymbol>, cond: ExprRef, block: ExprRef);

    pub fn break_stmt(&mut self, location: Option<SourceLocation>) -> StmtRef {
        self.break_stmt_with_label(None, location)
    }

    pub fn continue_stmt(&mut self, location: Option<SourceLocation>) -> StmtRef {
        self.continue_stmt_with_label(None, location)
    }

    pub fn for_stmt(&mut self, var: DefaultSymbol, start: ExprRef, end: ExprRef, block: ExprRef, location: Option<SourceLocation>) -> StmtRef {
        self.for_stmt_with_label(None, var, start, end, block, location)
    }

    pub fn while_stmt(&mut self, cond: ExprRef, block: ExprRef, location: Option<SourceLocation>) -> StmtRef {
        self.while_stmt_with_label(None, cond, block, location)
    }
}
