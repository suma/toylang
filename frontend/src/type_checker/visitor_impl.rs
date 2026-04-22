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
            Stmt::EnumDecl { name, generic_params, variants, visibility } => visitor.visit_enum_decl(*name, generic_params, variants, visibility),
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

/// A pattern is irrefutable when it always matches any value of the expected
/// type. `Name` and `Wildcard` are irrefutable. Literals are refutable by
/// value. An `EnumVariant` pattern narrows to a single variant, so it is
/// refutable in any enum with more than one variant — we conservatively
/// treat it as refutable, since the check only affects whether an already-
/// seen top-level variant triggers an "unreachable arm" error when the
/// same variant reappears with different sub-patterns.
fn is_irrefutable_pattern(pat: &Pattern) -> bool {
    match pat {
        Pattern::Wildcard | Pattern::Name(_) => true,
        Pattern::Literal(_) | Pattern::EnumVariant(_, _, _) => false,
    }
}

impl<'a> TypeCheckerVisitor<'a> {
    /// Recursively type-check a sub-pattern against the expected payload type.
    /// Introduces any `Name` bindings into the *current* variable scope, which
    /// callers are responsible for pushing/popping around the arm body.
    fn check_sub_pattern(&mut self, pat: &Pattern, expected_ty: &TypeDecl) -> Result<(), TypeCheckError> {
        match pat {
            Pattern::Wildcard => Ok(()),
            Pattern::Name(sym) => {
                self.context.set_var(*sym, expected_ty.clone());
                Ok(())
            }
            Pattern::Literal(lit_expr) => {
                if !matches!(expected_ty, TypeDecl::Bool | TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::String) {
                    return Err(TypeCheckError::new(format!(
                        "literal pattern is only valid where a primitive value is expected, got {:?}",
                        expected_ty
                    )));
                }
                let saved_hint = self.type_inference.type_hint.clone();
                self.type_inference.type_hint = Some(expected_ty.clone());
                let lit_ty = self.visit_expr(lit_expr)?;
                self.type_inference.type_hint = saved_hint;
                if !lit_ty.is_equivalent(expected_ty) {
                    return Err(TypeCheckError::new(format!(
                        "literal pattern type {:?} does not match expected {:?}",
                        lit_ty, expected_ty
                    )));
                }
                Ok(())
            }
            Pattern::EnumVariant(pat_enum, pat_variant, sub_patterns) => {
                // Extract the enum name + type args from the expected payload
                // type. Accept Enum, Struct (parser can emit this), or
                // Identifier forms, the same way the top-level match logic
                // does.
                let (enum_name, enum_type_args) = match expected_ty {
                    TypeDecl::Enum(name, args) => (*name, args.clone()),
                    TypeDecl::Struct(name, args)
                        if self.context.enum_definitions.contains_key(name) => (*name, args.clone()),
                    TypeDecl::Identifier(name)
                        if self.context.enum_definitions.contains_key(name) => (*name, Vec::new()),
                    _ => {
                        return Err(TypeCheckError::new(format!(
                            "enum-variant sub-pattern expects an enum payload, got {:?}",
                            expected_ty
                        )));
                    }
                };
                if *pat_enum != enum_name {
                    let expected = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                    let got = self.core.string_interner.resolve(*pat_enum).unwrap_or("?").to_string();
                    return Err(TypeCheckError::new(format!(
                        "nested pattern refers to enum '{}', but payload type is enum '{}'", got, expected
                    )));
                }
                let variants = self.context.enum_definitions.get(&enum_name).cloned()
                    .ok_or_else(|| TypeCheckError::new("nested match on unknown enum".to_string()))?;
                let variant_def = variants.iter().find(|v| v.name == *pat_variant)
                    .cloned()
                    .ok_or_else(|| {
                        let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                        let v_str = self.core.string_interner.resolve(*pat_variant).unwrap_or("?").to_string();
                        TypeCheckError::new(format!("'{}' is not a variant of enum '{}'", v_str, enum_str))
                    })?;
                if sub_patterns.len() != variant_def.payload_types.len() {
                    let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                    let v_str = self.core.string_interner.resolve(*pat_variant).unwrap_or("?").to_string();
                    return Err(TypeCheckError::new(format!(
                        "variant '{}::{}' has {} payload field(s) but pattern bound {}",
                        enum_str, v_str, variant_def.payload_types.len(), sub_patterns.len()
                    )));
                }
                let generic_params = self.context.enum_generic_params.get(&enum_name).cloned().unwrap_or_default();
                let mut substitutions: std::collections::HashMap<DefaultSymbol, TypeDecl> = std::collections::HashMap::new();
                for (param, arg) in generic_params.iter().zip(enum_type_args.iter()) {
                    substitutions.insert(*param, arg.clone());
                }
                for (sub, payload_ty) in sub_patterns.iter().zip(variant_def.payload_types.iter()) {
                    let resolved = payload_ty.substitute_generics(&substitutions);
                    self.check_sub_pattern(sub, &resolved)?;
                }
                Ok(())
            }
        }
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

    fn visit_match(&mut self, scrutinee: &ExprRef, arms: &Vec<(Pattern, ExprRef)>) -> Result<TypeDecl, TypeCheckError> {
        if arms.is_empty() {
            return Err(TypeCheckError::new("match expression must have at least one arm".to_string()));
        }
        let scrutinee_ty = self.visit_expr(scrutinee)?;

        // Classify the scrutinee. Enum matches and primitive matches accept
        // different pattern shapes, so we dispatch on this up-front.
        enum ScrutineeKind {
            Enum {
                name: DefaultSymbol,
                type_args: Vec<TypeDecl>,
                variants: Vec<crate::ast::EnumVariantDef>,
            },
            Primitive(TypeDecl),
        }
        let kind = match &scrutinee_ty {
            TypeDecl::Enum(name, args) => {
                let variants = self.context.enum_definitions.get(name)
                    .cloned()
                    .ok_or_else(|| TypeCheckError::new("match on unknown enum".to_string()))?;
                ScrutineeKind::Enum { name: *name, type_args: args.clone(), variants }
            }
            TypeDecl::Identifier(name) if self.context.enum_definitions.contains_key(name) => {
                let variants = self.context.enum_definitions.get(name).cloned().unwrap();
                ScrutineeKind::Enum { name: *name, type_args: Vec::new(), variants }
            }
            TypeDecl::Struct(name, args) if self.context.enum_definitions.contains_key(name) => {
                let variants = self.context.enum_definitions.get(name).cloned().unwrap();
                ScrutineeKind::Enum { name: *name, type_args: args.clone(), variants }
            }
            TypeDecl::Bool | TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::String => ScrutineeKind::Primitive(scrutinee_ty.clone()),
            _ => {
                return Err(TypeCheckError::new(format!(
                    "match scrutinee must be an enum or a primitive (bool / i64 / u64 / str), got {:?}",
                    scrutinee_ty
                )));
            }
        };

        // Track coverage to enforce exhaustiveness and reject unreachable arms.
        let mut arm_types: Vec<TypeDecl> = Vec::with_capacity(arms.len());
        // Two sets because of nested patterns:
        //  - `fully_covered_variants` gates the unreachable-arm check and only
        //    includes variants whose sub-patterns were all irrefutable.
        //  - `seen_variants` gates exhaustiveness; any arm for a variant
        //    counts, since exhaustiveness across arbitrary nested patterns is
        //    undecidable in our simple analysis.
        let mut fully_covered_variants: std::collections::HashSet<DefaultSymbol> = std::collections::HashSet::new();
        let mut seen_variants: std::collections::HashSet<DefaultSymbol> = std::collections::HashSet::new();
        let mut covered_int64: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let mut covered_uint64: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let mut covered_bool: std::collections::HashSet<bool> = std::collections::HashSet::new();
        let mut covered_strings: std::collections::HashSet<DefaultSymbol> = std::collections::HashSet::new();
        let mut has_wildcard = false;
        for (arm_index, (pat, body)) in arms.iter().enumerate() {
            if has_wildcard {
                return Err(TypeCheckError::new(format!(
                    "unreachable match arm at position {}: a wildcard `_` arm already covers every value",
                    arm_index
                )));
            }
            let mut pushed_scope = false;
            match pat {
                Pattern::Wildcard => {
                    has_wildcard = true;
                }
                Pattern::Name(sym) => {
                    // Bare name at top level binds the whole scrutinee; it is
                    // irrefutable and therefore acts like a wildcard for
                    // exhaustiveness.
                    self.context.vars.push(std::collections::HashMap::new());
                    pushed_scope = true;
                    self.context.set_var(*sym, scrutinee_ty.clone());
                    has_wildcard = true;
                }
                Pattern::Literal(literal_expr) => {
                    let prim_ty = match &kind {
                        ScrutineeKind::Primitive(t) => t.clone(),
                        ScrutineeKind::Enum { .. } => {
                            return Err(TypeCheckError::new(
                                "literal pattern cannot be used in a match on an enum".to_string()
                            ));
                        }
                    };
                    // Literal expression must have the same primitive type as
                    // the scrutinee. We visit it with the scrutinee type as a
                    // hint so bare numeric literals pick up i64 / u64.
                    let saved_hint = self.type_inference.type_hint.clone();
                    self.type_inference.type_hint = Some(prim_ty.clone());
                    let lit_ty = self.visit_expr(literal_expr)?;
                    self.type_inference.type_hint = saved_hint;
                    if !lit_ty.is_equivalent(&prim_ty) {
                        return Err(TypeCheckError::new(format!(
                            "literal pattern type {:?} does not match scrutinee type {:?}",
                            lit_ty, prim_ty
                        )));
                    }
                    // Record the concrete literal value for duplicate / exhaustiveness checks.
                    if let Some(lit_expr) = self.core.expr_pool.get(literal_expr) {
                        match lit_expr {
                            Expr::Int64(v) => {
                                if !covered_int64.insert(v) {
                                    return Err(TypeCheckError::new(format!(
                                        "unreachable match arm: literal {} already handled by an earlier arm", v
                                    )));
                                }
                            }
                            Expr::UInt64(v) => {
                                if !covered_uint64.insert(v) {
                                    return Err(TypeCheckError::new(format!(
                                        "unreachable match arm: literal {} already handled by an earlier arm", v
                                    )));
                                }
                            }
                            Expr::True => {
                                if !covered_bool.insert(true) {
                                    return Err(TypeCheckError::new(
                                        "unreachable match arm: literal `true` already handled by an earlier arm".to_string()
                                    ));
                                }
                            }
                            Expr::False => {
                                if !covered_bool.insert(false) {
                                    return Err(TypeCheckError::new(
                                        "unreachable match arm: literal `false` already handled by an earlier arm".to_string()
                                    ));
                                }
                            }
                            Expr::String(sym) => {
                                if !covered_strings.insert(sym) {
                                    let s = self.core.string_interner.resolve(sym).unwrap_or("?").to_string();
                                    return Err(TypeCheckError::new(format!(
                                        "unreachable match arm: literal {:?} already handled by an earlier arm",
                                        s
                                    )));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Pattern::EnumVariant(pat_enum, pat_variant, bindings) => {
                    let (enum_name, enum_type_args, variants) = match &kind {
                        ScrutineeKind::Enum { name, type_args, variants } => (*name, type_args.clone(), variants.clone()),
                        ScrutineeKind::Primitive(t) => {
                            return Err(TypeCheckError::new(format!(
                                "enum-variant pattern cannot be used in a match on {:?}", t
                            )));
                        }
                    };
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
                    // `Option::Some(Some(x))` and `Option::Some(None)` share
                    // the top variant `Some` but aren't redundant — they
                    // cover disjoint sub-patterns. So we only treat a
                    // variant as redundant when an earlier arm's sub-patterns
                    // are all irrefutable (Name / Wildcard at every slot).
                    if fully_covered_variants.contains(pat_variant) {
                        let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                        let v_str = self.core.string_interner.resolve(*pat_variant).unwrap_or("?").to_string();
                        return Err(TypeCheckError::new(format!(
                            "unreachable match arm: variant '{}::{}' already fully covered by an earlier arm",
                            enum_str, v_str
                        )));
                    }
                    if bindings.len() != variant_def.payload_types.len() {
                        let enum_str = self.core.string_interner.resolve(enum_name).unwrap_or("?").to_string();
                        let v_str = self.core.string_interner.resolve(*pat_variant).unwrap_or("?").to_string();
                        return Err(TypeCheckError::new(format!(
                            "variant '{}::{}' has {} payload field(s) but pattern bound {}",
                            enum_str, v_str, variant_def.payload_types.len(), bindings.len()
                        )));
                    }
                    if !bindings.is_empty() {
                        self.context.vars.push(std::collections::HashMap::new());
                        pushed_scope = true;
                        let generic_params = self.context.enum_generic_params.get(&enum_name).cloned().unwrap_or_default();
                        let mut substitutions: std::collections::HashMap<DefaultSymbol, TypeDecl> = std::collections::HashMap::new();
                        for (param, arg) in generic_params.iter().zip(enum_type_args.iter()) {
                            substitutions.insert(*param, arg.clone());
                        }
                        for (sub_pat, payload_ty) in bindings.iter().zip(variant_def.payload_types.iter()) {
                            let resolved = payload_ty.substitute_generics(&substitutions);
                            self.check_sub_pattern(sub_pat, &resolved)?;
                        }
                    }
                    // Only mark the variant as fully covered if every
                    // sub-pattern is irrefutable. Refutable sub-patterns
                    // (literals, nested enum variants) leave part of the
                    // variant's value space unmatched, so another arm
                    // targeting the same variant can still be useful.
                    if bindings.iter().all(is_irrefutable_pattern) {
                        fully_covered_variants.insert(*pat_variant);
                    }
                    seen_variants.insert(*pat_variant);
                }
            }
            let body_ty = self.visit_expr(body)?;
            if pushed_scope {
                self.context.vars.pop();
            }
            arm_types.push(body_ty);
        }

        // Exhaustiveness. Enums must cover every variant. `bool` must cover
        // both `true` and `false`. Other primitives have an unbounded value
        // space, so a wildcard is mandatory.
        if !has_wildcard {
            match &kind {
                ScrutineeKind::Enum { name, variants, .. } => {
                    let missing: Vec<DefaultSymbol> = variants.iter()
                        .filter(|v| !seen_variants.contains(&v.name))
                        .map(|v| v.name)
                        .collect();
                    if !missing.is_empty() {
                        let enum_str = self.core.string_interner.resolve(*name).unwrap_or("?").to_string();
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
                ScrutineeKind::Primitive(TypeDecl::Bool) => {
                    if !covered_bool.contains(&true) || !covered_bool.contains(&false) {
                        return Err(TypeCheckError::new(
                            "non-exhaustive match on bool: cover both `true` and `false` or add a wildcard `_`".to_string()
                        ));
                    }
                }
                ScrutineeKind::Primitive(t) => {
                    let t_name = match t {
                        TypeDecl::Int64 => "i64".to_string(),
                        TypeDecl::UInt64 => "u64".to_string(),
                        TypeDecl::String => "str".to_string(),
                        other => format!("{:?}", other),
                    };
                    return Err(TypeCheckError::new(format!(
                        "non-exhaustive match on {}: primitive value space is unbounded, add a wildcard `_` arm",
                        t_name
                    )));
                }
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
