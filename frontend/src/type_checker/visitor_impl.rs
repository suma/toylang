use std::rc::Rc;
use string_interner::DefaultSymbol;
use crate::ast::*;
use crate::type_decl::*;
use crate::visitor::{AstVisitor, ProgramVisitor};
use crate::type_checker::{
    TypeCheckerVisitor, TypeCheckError, TypeCheckContext, TypeInferenceState,
    CoreReferences, method,
};
use crate::type_checker::{Acceptable, TypeCheckerCore, TypeInferenceManager};

impl Acceptable for Expr {
    fn accept(&mut self, visitor: &mut dyn AstVisitor) -> Result<TypeDecl, TypeCheckError> {
        match self {
            Expr::Binary(op, lhs, rhs) => visitor.visit_binary(op, lhs, rhs),
            Expr::Unary(op, operand) => visitor.visit_unary(op, operand),
            Expr::Block(statements) => visitor.visit_block(statements),
            Expr::IfElifElse(cond, then_block, elif_pairs, else_block) => visitor.visit_if_elif_else(cond, then_block, elif_pairs, else_block),
            Expr::Assign(lhs, rhs) => visitor.visit_assign(lhs, rhs),
            Expr::Identifier(name) => visitor.visit_identifier(*name),
            Expr::Call(fn_name, args) => visitor.visit_call(*fn_name, args),
            Expr::Int64(val) => visitor.visit_int64_literal(val),
            Expr::UInt64(val) => visitor.visit_uint64_literal(val),
            Expr::Int8(val) => visitor.visit_int8_literal(val),
            Expr::Int16(val) => visitor.visit_int16_literal(val),
            Expr::Int32(val) => visitor.visit_int32_literal(val),
            Expr::UInt8(val) => visitor.visit_uint8_literal(val),
            Expr::UInt16(val) => visitor.visit_uint16_literal(val),
            Expr::UInt32(val) => visitor.visit_uint32_literal(val),
            Expr::Float64(val) => visitor.visit_float64_literal(val),
            Expr::Number(val) => visitor.visit_number_literal(*val),
            Expr::String(val) => visitor.visit_string_literal(*val),
            Expr::True | Expr::False => visitor.visit_boolean_literal(self),
            Expr::Null => visitor.visit_null_literal(),
            Expr::ExprList(items) => visitor.visit_expr_list(items),
            Expr::ArrayLiteral(elements) => visitor.visit_array_literal(elements),
            Expr::FieldAccess(obj, field) => visitor.visit_field_access(obj, field),
            Expr::MethodCall(obj, method, args) => visitor.visit_method_call(obj, method, args),
            Expr::StructLiteral(struct_name, fields) => visitor.visit_struct_literal(struct_name, fields),
            Expr::QualifiedIdentifier(path) => visitor.visit_qualified_identifier(path),
            Expr::BuiltinMethodCall(receiver, method, args) => visitor.visit_builtin_method_call(receiver, method, args),
            Expr::SliceAssign(object, start, end, value) => {
                visitor.visit_slice_assign(object, start, end, value)
            },
            Expr::AssociatedFunctionCall(struct_name, function_name, args) => {
                visitor.visit_associated_function_call(*struct_name, *function_name, args)
            },
            Expr::SliceAccess(object, slice_info) => {
                visitor.visit_slice_access(object, slice_info)
            },
            Expr::DictLiteral(entries) => visitor.visit_dict_literal(entries),
            Expr::BuiltinCall(func, args) => visitor.visit_builtin_call(func, args),
            Expr::TupleLiteral(elements) => visitor.visit_tuple_literal(elements),
            Expr::TupleAccess(tuple, index) => visitor.visit_tuple_access(tuple, *index),
            Expr::Cast(expr, target_type) => visitor.visit_cast(expr, target_type),
            Expr::With(allocator, body) => visitor.visit_with(allocator, body),
            Expr::Match(scrutinee, arms) => visitor.visit_match(scrutinee, arms),
            Expr::Range(start, end) => visitor.visit_range(start, end),
            Expr::Closure { params, return_type, body } => {
                visitor.visit_closure(params, return_type, body)
            }
        }
    }
}

impl Acceptable for Stmt {
    fn accept(&mut self, visitor: &mut dyn AstVisitor) -> Result<TypeDecl, TypeCheckError> {
        match self {
            Stmt::Expression(expr) => visitor.visit_expression_stmt(expr),
            Stmt::Var(name, type_decl, expr) => visitor.visit_var(*name, type_decl, expr),
            Stmt::Val(name, type_decl, expr) => visitor.visit_val(*name, type_decl, expr),
            Stmt::Return(expr) => visitor.visit_return(expr),
            Stmt::For(init, cond, step, body) => visitor.visit_for(*init, cond, step, body),
            Stmt::While(cond, body) => visitor.visit_while(cond, body),
            Stmt::Break => visitor.visit_break(),
            Stmt::Continue => visitor.visit_continue(),
            Stmt::StructDecl { name, generic_params, generic_bounds, fields, visibility } => visitor.visit_struct_decl(*name, generic_params, generic_bounds, fields, visibility),
            Stmt::ImplBlock { target_type, target_type_args, methods, trait_name } => visitor.visit_impl_block(*target_type, target_type_args, methods, *trait_name),
            Stmt::EnumDecl { name, generic_params, variants, visibility } => visitor.visit_enum_decl(*name, generic_params, variants, visibility),
            Stmt::TraitDecl { name, methods, visibility } => visitor.visit_trait_decl(*name, methods, visibility),
            // Aliases were already substituted during parsing; nothing
            // for the type checker to do here.
            Stmt::TypeAlias { .. } => Ok(TypeDecl::Unit),
        }
    }
}

impl<'a> ProgramVisitor for TypeCheckerVisitor<'a> {
    fn visit_program(&mut self, program: &Program) -> Result<(), TypeCheckError> {
        // Process package declaration if present
        if let Some(package_decl) = &program.package_decl {
            self.visit_package(package_decl)?;
        }

        // Process all import declarations
        for import_decl in &program.imports {
            self.visit_import(import_decl)?;
        }

        // Process all statements in the program (this includes StructDecl and ImplBlock)
        for index in 0..program.statement.len() {
            let stmt_ref = StmtRef(index as u32);
            self.visit_stmt(&stmt_ref)?;
        }

        // Process all functions in the program
        for function in &program.function {
            self.type_check(function.clone())?;
        }

        Ok(())
    }

    fn visit_package(&mut self, package_decl: &PackageDecl) -> Result<(), TypeCheckError> {
        // Phase 1: Basic package validation and context setting

        // Validate package name is not empty
        if package_decl.name.is_empty() {
            return Err(TypeCheckError::generic_error("Package name cannot be empty"));
        }

        // Check for reserved keywords in package name
        for &symbol in &package_decl.name {
            let name_str = self.core.string_interner.resolve(symbol)
                .ok_or_else(|| TypeCheckError::generic_error("Package name symbol not found in interner"))?;

            if super::module_access::is_reserved_keyword(name_str) {
                return Err(TypeCheckError::generic_error(&format!("Package name '{}' cannot use reserved keyword", name_str)));
            }
        }

        // Set current package context
        self.set_current_package(package_decl.name.clone());

        Ok(())
    }

    fn visit_import(&mut self, import_decl: &ImportDecl) -> Result<(), TypeCheckError> {
        // Phase 1: Basic import validation and registration

        // Validate import path is not empty
        if import_decl.module_path.is_empty() {
            return Err(TypeCheckError::generic_error("Import path cannot be empty"));
        }

        // Check for self-import
        if !self.is_valid_import(&import_decl.module_path) {
            return Err(TypeCheckError::generic_error("Cannot import current package (self-import)"));
        }

        // Validate each component of import path
        for &symbol in &import_decl.module_path {
            let name_str = self.core.string_interner.resolve(symbol)
                .ok_or_else(|| TypeCheckError::generic_error("Import path symbol not found in interner"))?;

            if super::module_access::is_reserved_keyword(name_str) {
                return Err(TypeCheckError::generic_error(&format!("Import path '{}' cannot use reserved keyword", name_str)));
            }
        }

        // Register the import for later name resolution
        self.register_import(import_decl.module_path.clone());

        Ok(())
    }
}

impl<'a> AstVisitor for TypeCheckerVisitor<'a> {
    // =========================================================================
    // Core Visitor Methods
    // =========================================================================

    fn visit_expr(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.visit_expr(expr)
    }

    fn visit_stmt(&mut self, stmt: &StmtRef) -> Result<TypeDecl, TypeCheckError> {
        self.visit_stmt(stmt)
    }

    // =========================================================================
    // Expression Type Checking
    // =========================================================================

    fn visit_unary(&mut self, op: &UnaryOp, operand: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.visit_unary(op, operand)
    }

    fn visit_binary(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.visit_binary(op, lhs, rhs)
    }

    fn visit_block(&mut self, statements: &Vec<StmtRef>) -> Result<TypeDecl, TypeCheckError> {
        self.visit_block(statements)
    }

    fn visit_if_elif_else(&mut self, _cond: &ExprRef, then_block: &ExprRef, elif_pairs: &Vec<(ExprRef, ExprRef)>, else_block: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.visit_if_elif_else(_cond, then_block, elif_pairs, else_block)
    }

    fn visit_assign(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.visit_assign(lhs, rhs)
    }

    fn visit_identifier(&mut self, name: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        self.visit_identifier(name)
    }

    // =========================================================================
    // Function and Method Type Checking
    // =========================================================================

    fn visit_call(&mut self, fn_name: DefaultSymbol, args_ref: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.visit_call(fn_name, args_ref)
    }

    // =========================================================================
    // Literal Type Checking
    // =========================================================================

    fn visit_int64_literal(&mut self, _value: &i64) -> Result<TypeDecl, TypeCheckError> {
        self.visit_int64_literal(_value)
    }

    fn visit_uint64_literal(&mut self, _value: &u64) -> Result<TypeDecl, TypeCheckError> {
        self.visit_uint64_literal(_value)
    }

    fn visit_float64_literal(&mut self, _value: &f64) -> Result<TypeDecl, TypeCheckError> {
        self.visit_float64_literal(_value)
    }

    fn visit_number_literal(&mut self, _value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        self.visit_number_literal(_value)
    }

    fn visit_string_literal(&mut self, _value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        self.visit_string_literal(_value)
    }

    fn visit_boolean_literal(&mut self, _value: &Expr) -> Result<TypeDecl, TypeCheckError> {
        self.visit_boolean_literal(_value)
    }

    fn visit_null_literal(&mut self) -> Result<TypeDecl, TypeCheckError> {
        self.visit_null_literal()
    }

    fn visit_expr_list(&mut self, _items: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        self.visit_expr_list(_items)
    }

    fn visit_array_literal(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        self.visit_array_literal(elements)
    }

    fn visit_slice_access(&mut self, object: &ExprRef, slice_info: &SliceInfo) -> Result<TypeDecl, TypeCheckError> {
        self.visit_slice_access_impl(object, slice_info)
    }

    fn visit_slice_assign(&mut self, object: &ExprRef, start: &Option<ExprRef>, end: &Option<ExprRef>, value: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.visit_slice_assign_impl(object, start, end, value)
    }

    fn visit_associated_function_call(&mut self, struct_name: DefaultSymbol, function_name: DefaultSymbol, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        self.visit_associated_function_call_impl(struct_name, function_name, args)
    }

    fn visit_dict_literal(&mut self, entries: &Vec<(ExprRef, ExprRef)>) -> Result<TypeDecl, TypeCheckError> {
        self.visit_dict_literal_impl(entries)
    }

    fn visit_tuple_literal(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        self.visit_tuple_literal_impl(elements)
    }

    fn visit_tuple_access(&mut self, tuple: &ExprRef, index: usize) -> Result<TypeDecl, TypeCheckError> {
        self.visit_tuple_access_impl(tuple, index)
    }

    fn visit_cast(&mut self, expr: &ExprRef, target_type: &TypeDecl) -> Result<TypeDecl, TypeCheckError> {
        self.visit_cast_impl(expr, target_type)
    }

    fn visit_range(&mut self, start: &ExprRef, end: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        // `start..end` requires both sides to be the same integer type. We
        // share the start's type as a hint while visiting end so untyped
        // numeric literals (`0..n`) pick up the matching concrete type.
        let saved_hint = self.type_inference.type_hint.clone();
        let start_ty = self.visit_expr(start)?;
        self.type_inference.type_hint = Some(start_ty.clone());
        let end_ty = self.visit_expr(end)?;
        self.type_inference.type_hint = saved_hint;

        let element_ty = match (&start_ty, &end_ty) {
            (TypeDecl::Int64, TypeDecl::Int64) => TypeDecl::Int64,
            (TypeDecl::UInt64, TypeDecl::UInt64) => TypeDecl::UInt64,
            (TypeDecl::Number, TypeDecl::Number) => TypeDecl::UInt64,
            (TypeDecl::Number, other) | (other, TypeDecl::Number)
                if matches!(other, TypeDecl::Int64 | TypeDecl::UInt64) => other.clone(),
            _ => {
                return Err(TypeCheckError::new(format!(
                    "range endpoints must be matching integer types, got {:?}..{:?}",
                    start_ty, end_ty
                )));
            }
        };
        Ok(TypeDecl::Range(Box::new(element_ty)))
    }

    fn visit_with(&mut self, allocator: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        // The RHS of `with allocator = ...` must evaluate to an Allocator handle.
        // Three accept paths:
        //   1. Bare `TypeDecl::Allocator` (the primitive runtime handle).
        //   2. Generic param bounded by `Allocator`
        //      (`fn f<A: Allocator>(a: A) { with allocator = a {...} }`).
        //   3. STDLIB-alloc-trait: a struct value that impls
        //      `core/std/allocator.t::Alloc` and carries exactly one
        //      `Allocator`-typed field. The `with` site auto-extracts
        //      that field at lowering time, so user code can write
        //      `with allocator = arena { ... }` for `arena: Arena`.
        let allocator_ty = self.visit_expr(allocator)?;
        // Resolve `Identifier(name)` to `Struct(name, [])` for the
        // is-it-a-struct check below — the parser emits `Identifier`
        // for bare type names because it can't distinguish struct from
        // alias at parse time. Same refinement the method-call site
        // does (commit `ea0c0cd`).
        let resolved_ty = match &allocator_ty {
            TypeDecl::Identifier(name)
                if self.context.struct_definitions.contains_key(name) =>
            {
                TypeDecl::Struct(*name, vec![])
            }
            other => other.clone(),
        };
        let is_allocator = match &resolved_ty {
            TypeDecl::Allocator => true,
            TypeDecl::Generic(sym) => matches!(
                self.context.current_fn_generic_bounds.get(sym),
                Some(TypeDecl::Allocator)
            ),
            TypeDecl::Struct(struct_name, _) => {
                // Look up the `Alloc` trait by interner symbol. If the
                // trait isn't registered (e.g. the program doesn't use
                // any stdlib that declares `Alloc`), this branch falls
                // through to the error path — matches the previous
                // behaviour for unregistered names.
                let alloc_trait = self.core.string_interner.get("Alloc");
                let conforms = alloc_trait
                    .map(|t| self.context.struct_implements_trait(*struct_name, t))
                    .unwrap_or(false);
                if !conforms {
                    false
                } else {
                    // STDLIB-alloc-trait: also require exactly one
                    // `Allocator`-typed field so the lowering pass
                    // has an unambiguous field to extract. Zero or
                    // multiple → reject.
                    let fields = self
                        .context
                        .get_struct_fields(*struct_name)
                        .cloned()
                        .unwrap_or_default();
                    let alloc_field_count = fields
                        .iter()
                        .filter(|f| matches!(f.type_decl, TypeDecl::Allocator))
                        .count();
                    alloc_field_count == 1
                }
            }
            _ => false,
        };
        if !is_allocator {
            return Err(TypeCheckError::new(format!(
                "`with allocator = ...` requires an Allocator value, but got {:?}",
                allocator_ty
            )));
        }
        self.visit_expr(body)
    }

    // =========================================================================
    // Statement Type Checking
    // =========================================================================

    fn visit_expression_stmt(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.visit_expression_stmt(expr)
    }

    fn visit_var(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        self.visit_var_impl(name, type_decl, expr)
    }

    fn visit_val(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.visit_val_impl(name, type_decl, expr)
    }

    fn visit_return(&mut self, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        self.visit_return(expr)
    }

    // =========================================================================
    // Control Flow Type Checking
    // =========================================================================

    fn visit_for(&mut self, init: DefaultSymbol, _cond: &ExprRef, range: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.visit_for_impl(init, _cond, range, body)
    }

    fn visit_while(&mut self, cond: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.visit_while_impl(cond, body)
    }

    fn visit_break(&mut self) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Unit)
    }

    fn visit_continue(&mut self) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Unit)
    }

    // =========================================================================
    // Struct Type Checking
    // =========================================================================

    fn visit_struct_decl(&mut self, name: DefaultSymbol, generic_params: &Vec<DefaultSymbol>, generic_bounds: &std::collections::HashMap<DefaultSymbol, TypeDecl>, fields: &Vec<StructField>, visibility: &Visibility) -> Result<TypeDecl, TypeCheckError> {
        self.visit_struct_decl_impl(name, generic_params, generic_bounds, fields, visibility)
    }

    fn visit_impl_block(&mut self, target_type: DefaultSymbol, target_type_args: &Vec<TypeDecl>, methods: &Vec<Rc<MethodFunction>>, trait_name: Option<DefaultSymbol>) -> Result<TypeDecl, TypeCheckError> {
        self.visit_impl_block_impl(target_type, target_type_args, methods, trait_name)
    }

    fn visit_trait_decl(&mut self, name: DefaultSymbol, methods: &Vec<TraitMethodSignature>, _visibility: &Visibility) -> Result<TypeDecl, TypeCheckError> {
        self.visit_trait_decl_impl(name, methods)
    }

    fn visit_enum_decl(&mut self, name: DefaultSymbol, generic_params: &Vec<DefaultSymbol>, variants: &Vec<EnumVariantDef>, _visibility: &Visibility) -> Result<TypeDecl, TypeCheckError> {
        // Reject duplicate enum names and duplicate variant names inside one enum.
        if self.context.enum_definitions.contains_key(&name) {
            let name_str = self.core.string_interner.resolve(name).unwrap_or("?").to_string();
            return Err(TypeCheckError::new(format!("enum '{}' is already defined", name_str)));
        }
        let mut seen = std::collections::HashSet::new();
        for v in variants {
            if !seen.insert(v.name) {
                let enum_str = self.core.string_interner.resolve(name).unwrap_or("?").to_string();
                let v_str = self.core.string_interner.resolve(v.name).unwrap_or("?").to_string();
                return Err(TypeCheckError::new(format!(
                    "duplicate variant '{}' in enum '{}'", v_str, enum_str
                )));
            }
        }
        self.context.enum_definitions.insert(name, variants.clone());
        if !generic_params.is_empty() {
            self.context.enum_generic_params.insert(name, generic_params.clone());
        }
        Ok(TypeDecl::Unit)
    }

    fn visit_match(&mut self, scrutinee: &ExprRef, arms: &Vec<MatchArm>) -> Result<TypeDecl, TypeCheckError> {
        self.visit_match_impl(scrutinee, arms)
    }

    fn visit_closure(
        &mut self,
        params: &crate::ast::ParameterList,
        return_type: &Option<TypeDecl>,
        body: &ExprRef,
    ) -> Result<TypeDecl, TypeCheckError> {
        self.visit_closure_impl(params, return_type, body)
    }

    fn visit_field_access(&mut self, obj: &ExprRef, field: &DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        self.visit_field_access_impl(obj, field)
    }

    fn visit_method_call(&mut self, obj: &ExprRef, method: &DefaultSymbol, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        self.visit_method_call_impl(obj, method, args)
    }

    fn visit_struct_literal(&mut self, struct_name: &DefaultSymbol, fields: &Vec<(DefaultSymbol, ExprRef)>) -> Result<TypeDecl, TypeCheckError> {
        self.visit_struct_literal_impl(struct_name, fields)
    }

    fn visit_qualified_identifier(&mut self, path: &Vec<DefaultSymbol>) -> Result<TypeDecl, TypeCheckError> {
        // Enum variant reference: `EnumName::VariantName`. Only unit variants
        // can be referenced this way; tuple variants must be constructed via
        // `Enum::Variant(args)` (handled in visit_associated_function_call).
        if path.len() == 2 {
            let enum_name = path[0];
            let variant_name = path[1];
            if let Some(variants) = self.context.enum_definitions.get(&enum_name).cloned() {
                if let Some(v) = variants.iter().find(|v| v.name == variant_name) {
                    if !v.payload_types.is_empty() {
                        let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                        let v_str = self.core.string_interner.resolve(variant_name).unwrap_or("?").to_string();
                        return Err(TypeCheckError::new(format!(
                            "variant '{}::{}' takes {} argument(s); call it as `{}::{}( ... )`",
                            enum_str, v_str, v.payload_types.len(), enum_str, v_str
                        )));
                    }
                    // For generic unit variants, take type arguments from the
                    // declared hint (e.g. `val x: Option<i64> = Option::None`).
                    // Without a hint, default to `Generic(T)` placeholders; the
                    // `Identifier ↔ Enum` equivalence rule makes this work at
                    // typical assignment sites, and the type inference engine
                    // may later resolve them.
                    let generic_params = self.context.enum_generic_params.get(&enum_name).cloned();
                    let expected_arity = generic_params.as_ref().map_or(0, |p| p.len());
                    let type_args = match &self.type_inference.type_hint {
                        Some(TypeDecl::Enum(hint_name, hint_args))
                            if *hint_name == enum_name && expected_arity == hint_args.len() =>
                        {
                            hint_args.clone()
                        }
                        // Parser emits `Struct(name, ...)` for `Name<T>`
                        // annotations before knowing it's an enum; accept it
                        // here as a valid hint shape.
                        Some(TypeDecl::Struct(hint_name, hint_args))
                            if *hint_name == enum_name && expected_arity == hint_args.len() =>
                        {
                            hint_args.clone()
                        }
                        _ => generic_params
                            .map(|ps| ps.iter().map(|p| TypeDecl::Generic(*p)).collect())
                            .unwrap_or_default(),
                    };
                    return Ok(TypeDecl::Enum(enum_name, type_args));
                }
                let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                let v_str = self.core.string_interner.resolve(variant_name).unwrap_or("?").to_string();
                return Err(TypeCheckError::new(format!(
                    "'{}' is not a variant of enum '{}'", v_str, enum_str
                )));
            }
        }
        // For now, treat qualified identifiers like regular identifiers using the last component
        if let Some(last_symbol) = path.last() {
            self.visit_identifier(*last_symbol)
        } else {
            Err(TypeCheckError::generic_error("empty qualified identifier path"))
        }
    }

    fn visit_builtin_method_call(&mut self, receiver: &ExprRef, method: &BuiltinMethod, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        <Self as method::MethodProcessing>::visit_builtin_method_call(self, receiver, method, args)
    }

    fn visit_builtin_call(&mut self, func: &BuiltinFunction, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // `ptr_read` originally always returned u64, but generic `List<T>`
        // code stores non-u64 values. When the caller supplies a primitive
        // type hint (e.g. `val v: i64 = __builtin_ptr_read(p, off)` or a
        // method with a `T` return type being visited under its hint),
        // surface the hint as the result so nested expressions pick up
        // the right element type. Fall back to u64 for backward compat.
        if matches!(func, BuiltinFunction::PtrRead) {
            if let Some(hint) = &self.type_inference.type_hint {
                if matches!(hint,
                    TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Bool
                    | TypeDecl::Float64
                    // NUM-W: narrow int hints so `val b: u8 =
                    // __builtin_ptr_read(p, i)` (Phase B-min
                    // `__builtin_str_to_ptr` byte-walk pattern) works.
                    | TypeDecl::Int8 | TypeDecl::UInt8
                    | TypeDecl::Int16 | TypeDecl::UInt16
                    | TypeDecl::Int32 | TypeDecl::UInt32
                    | TypeDecl::Ptr | TypeDecl::String | TypeDecl::Allocator
                    | TypeDecl::Struct(_, _) | TypeDecl::Enum(_, _)
                    | TypeDecl::Generic(_)
                ) {
                    return Ok(hint.clone());
                }
            }
            return Ok(TypeDecl::UInt64);
        }

        // Integer math builtins: dispatch on the actual argument type
        // so the same `min(a, b)` symbol works for both `i64` and `u64`.
        // `abs(x)` accepts `i64` only; the result type matches the
        // operand type. Mismatched / unsupported arg types produce a
        // targeted diagnostic instead of the generic "argument type
        // mismatch" the signature path would emit.
        // NOTE: f64 math arity / type-check arm (pow/sqrt/sin/cos/tan
        // /log/log2/exp/floor/ceil) lived here before Phase 4. Each
        // is now declared as `extern fn __extern_*_f64` in math.t,
        // so type checking falls through to the regular call path
        // (the extern fn signature is enough to validate arity +
        // arg types).

        if matches!(
            func,
            BuiltinFunction::Abs | BuiltinFunction::Min | BuiltinFunction::Max
        ) {
            let expected = match func {
                BuiltinFunction::Abs => 1usize,
                _ => 2,
            };
            if args.len() != expected {
                let name = match func {
                    BuiltinFunction::Abs => "abs",
                    BuiltinFunction::Min => "min",
                    BuiltinFunction::Max => "max",
                    _ => unreachable!(),
                };
                return Err(TypeCheckError::generic_error(&format!(
                    "{name} expects {expected} argument(s), got {}",
                    args.len()
                )));
            }
            let arg_types: Vec<TypeDecl> = args
                .iter()
                .map(|a| self.visit_expr(a))
                .collect::<Result<_, _>>()?;
            if matches!(func, BuiltinFunction::Abs) {
                // Polymorphic: `i64 -> i64` or `f64 -> f64`. Mirrors
                // C's overloaded `abs` / `fabs` distinction in a
                // single user-facing intrinsic.
                match arg_types[0] {
                    TypeDecl::Int64 => return Ok(TypeDecl::Int64),
                    TypeDecl::Float64 => return Ok(TypeDecl::Float64),
                    _ => {
                        return Err(TypeCheckError::generic_error(&format!(
                            "abs expects an i64 or f64 argument, got {:?}",
                            arg_types[0]
                        )));
                    }
                }
            }
            // Min / Max: both operands must be the same integer type.
            if !matches!(arg_types[0], TypeDecl::Int64 | TypeDecl::UInt64) {
                let name = if matches!(func, BuiltinFunction::Min) { "min" } else { "max" };
                return Err(TypeCheckError::generic_error(&format!(
                    "{name} expects integer arguments, got {:?}",
                    arg_types[0]
                )));
            }
            if arg_types[0] != arg_types[1] {
                let name = if matches!(func, BuiltinFunction::Min) { "min" } else { "max" };
                return Err(TypeCheckError::generic_error(&format!(
                    "{name} arguments must agree on type: got {:?} and {:?}",
                    arg_types[0], arg_types[1]
                )));
            }
            return Ok(arg_types[0].clone());
        }

        // Find matching function signature from pre-built table
        let signature = self.builtin_function_signatures.iter().find(|sig| sig.func == *func).cloned();

        if let Some(sig) = signature {
            Ok(sig.return_type.clone())
        } else {
            Ok(TypeDecl::Unknown)
        }
    }
}

// Core trait implementations
impl<'a> TypeCheckerCore<'a> for TypeCheckerVisitor<'a> {
    fn get_core_refs(&self) -> &CoreReferences<'a> {
        &self.core
    }

    fn get_core_refs_mut(&mut self) -> &mut CoreReferences<'a> {
        &mut self.core
    }

    fn get_context(&self) -> &TypeCheckContext {
        &self.context
    }

    fn get_context_mut(&mut self) -> &mut TypeCheckContext {
        &mut self.context
    }

    fn get_type_inference(&self) -> &TypeInferenceState {
        &self.type_inference
    }

    fn get_type_inference_mut(&mut self) -> &mut TypeInferenceState {
        &mut self.type_inference
    }
}

impl<'a> TypeInferenceManager for TypeCheckerVisitor<'a> {
    fn get_cached_type(&self, expr_ref: &ExprRef) -> Option<&TypeDecl> {
        self.optimization.type_cache.get(expr_ref)
    }

    fn cache_type(&mut self, expr_ref: &ExprRef, type_decl: TypeDecl) {
        self.optimization.type_cache.insert(*expr_ref, type_decl);
    }

    fn clear_type_cache(&mut self) {
        self.optimization.type_cache.clear();
    }

    fn setup_type_hint_for_val(&mut self, type_decl: &Option<TypeDecl>) -> Option<TypeDecl> {
        self.setup_type_hint_for_val(type_decl)
    }

    fn update_variable_expr_mapping(&mut self, name: DefaultSymbol, expr_ref: &ExprRef) {
        self.update_variable_expr_mapping(name, expr_ref)
    }

    fn apply_type_transformations(&mut self, name: DefaultSymbol, type_decl: &TypeDecl) -> Result<(), TypeCheckError> {
        self.apply_type_transformations(name, type_decl)
    }

    fn determine_final_type(&mut self, name: DefaultSymbol, inferred_type: TypeDecl, declared_type: &Option<TypeDecl>) -> Result<TypeDecl, TypeCheckError> {
        self.determine_final_type(name, inferred_type, declared_type)
    }
}
