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
            Stmt::ImplBlock { target_type, methods } => visitor.visit_impl_block(*target_type, methods),
            Stmt::EnumDecl { name, variants, visibility } => visitor.visit_enum_decl(*name, variants, visibility),
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
        // A bare `TypeDecl::Allocator` is the obvious case, but a generic parameter
        // bounded by `Allocator` (e.g. `fn f<A: Allocator>(a: A) { with allocator = a {...} }`)
        // must also be accepted so the body can use the allocator it was handed.
        let allocator_ty = self.visit_expr(allocator)?;
        let is_allocator = match &allocator_ty {
            TypeDecl::Allocator => true,
            TypeDecl::Generic(sym) => matches!(
                self.context.current_fn_generic_bounds.get(sym),
                Some(TypeDecl::Allocator)
            ),
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

    fn visit_impl_block(&mut self, target_type: DefaultSymbol, methods: &Vec<Rc<MethodFunction>>) -> Result<TypeDecl, TypeCheckError> {
        self.visit_impl_block_impl(target_type, methods)
    }

    fn visit_enum_decl(&mut self, name: DefaultSymbol, variants: &Vec<EnumVariantDef>, _visibility: &Visibility) -> Result<TypeDecl, TypeCheckError> {
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
        Ok(TypeDecl::Unit)
    }

    fn visit_match(&mut self, scrutinee: &ExprRef, arms: &Vec<(Pattern, ExprRef)>) -> Result<TypeDecl, TypeCheckError> {
        if arms.is_empty() {
            return Err(TypeCheckError::new("match expression must have at least one arm".to_string()));
        }
        let scrutinee_ty = self.visit_expr(scrutinee)?;
        // Accept either a resolved Enum type or a bare Identifier naming a
        // registered enum (parser emits `TypeDecl::Identifier` for user-named
        // types and does not yet specialize to `Enum`).
        let enum_name = match &scrutinee_ty {
            TypeDecl::Enum(name) => *name,
            TypeDecl::Identifier(name) if self.context.enum_definitions.contains_key(name) => *name,
            _ => {
                return Err(TypeCheckError::new(format!(
                    "match scrutinee must be an enum, got {:?}", scrutinee_ty
                )));
            }
        };
        let variants = self.context.enum_definitions.get(&enum_name)
            .cloned()
            .ok_or_else(|| TypeCheckError::new("match on unknown enum".to_string()))?;

        // Validate each pattern and collect arm body types. Tuple-variant
        // bindings introduce fresh variables scoped to the arm body, so we
        // push a new variable scope around each body visit. While walking
        // the arms we also track coverage so the loop below can enforce
        // exhaustiveness.
        let mut arm_types: Vec<TypeDecl> = Vec::with_capacity(arms.len());
        let mut covered_variants: std::collections::HashSet<DefaultSymbol> = std::collections::HashSet::new();
        let mut has_wildcard = false;
        for (pat, body) in arms {
            let mut pushed_scope = false;
            match pat {
                Pattern::Wildcard => {
                    has_wildcard = true;
                }
                Pattern::EnumVariant(pat_enum, pat_variant, bindings) => {
                    if *pat_enum != enum_name {
                        let expected = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                        let got = self.core.string_interner.resolve(*pat_enum).unwrap_or("?").to_string();
                        return Err(TypeCheckError::new(format!(
                            "match pattern refers to enum '{}', but scrutinee is '{}'", got, expected
                        )));
                    }
                    let variant_def = variants.iter().find(|v| v.name == *pat_variant);
                    let variant_def = match variant_def {
                        Some(v) => v,
                        None => {
                            let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                            let v_str = self.core.string_interner.resolve(*pat_variant).unwrap_or("?").to_string();
                            return Err(TypeCheckError::new(format!(
                                "'{}' is not a variant of enum '{}'", v_str, enum_str
                            )));
                        }
                    };
                    if bindings.len() != variant_def.payload_types.len() {
                        let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                        let v_str = self.core.string_interner.resolve(*pat_variant).unwrap_or("?").to_string();
                        return Err(TypeCheckError::new(format!(
                            "variant '{}::{}' has {} payload field(s) but pattern bound {}",
                            enum_str, v_str, variant_def.payload_types.len(), bindings.len()
                        )));
                    }
                    if !bindings.is_empty() {
                        // Introduce each Name binding with the corresponding
                        // payload type for the arm body's scope.
                        self.context.vars.push(std::collections::HashMap::new());
                        pushed_scope = true;
                        for (binding, payload_ty) in bindings.iter().zip(variant_def.payload_types.iter()) {
                            if let crate::ast::PatternBinding::Name(sym) = binding {
                                self.context.set_var(*sym, payload_ty.clone());
                            }
                        }
                    }
                    covered_variants.insert(*pat_variant);
                }
            }
            let body_ty = self.visit_expr(body)?;
            if pushed_scope {
                self.context.vars.pop();
            }
            arm_types.push(body_ty);
        }

        // Exhaustiveness: without a wildcard arm, every variant of the enum
        // must appear at least once as an arm pattern. This catches the
        // `Option`-style mistake where a new variant is added to the enum
        // but an existing match is never updated.
        if !has_wildcard {
            let missing: Vec<DefaultSymbol> = variants.iter()
                .filter(|v| !covered_variants.contains(&v.name))
                .map(|v| v.name)
                .collect();
            if !missing.is_empty() {
                let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                let missing_strs: Vec<String> = missing.iter()
                    .map(|s| self.core.string_interner.resolve(*s).unwrap_or("?").to_string())
                    .collect();
                return Err(TypeCheckError::new(format!(
                    "non-exhaustive match on enum '{}': missing variant(s) {} — add an arm for each or a wildcard `_`",
                    enum_str,
                    missing_strs.join(", ")
                )));
            }
        }

        // All arms must share a common type.
        let first = arm_types[0].clone();
        for (i, t) in arm_types.iter().enumerate().skip(1) {
            if !first.is_equivalent(t) {
                return Err(TypeCheckError::new(format!(
                    "match arms have incompatible types: arm 0 is {:?}, arm {} is {:?}",
                    first, i, t
                )));
            }
        }
        Ok(first)
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
            if let Some(variants) = self.context.enum_definitions.get(&enum_name) {
                if let Some(v) = variants.iter().find(|v| v.name == variant_name) {
                    if !v.payload_types.is_empty() {
                        let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                        let v_str = self.core.string_interner.resolve(variant_name).unwrap_or("?").to_string();
                        return Err(TypeCheckError::new(format!(
                            "variant '{}::{}' takes {} argument(s); call it as `{}::{}( ... )`",
                            enum_str, v_str, v.payload_types.len(), enum_str, v_str
                        )));
                    }
                    return Ok(TypeDecl::Enum(enum_name));
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

    fn visit_builtin_call(&mut self, func: &BuiltinFunction, _args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
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
