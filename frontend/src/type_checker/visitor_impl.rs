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
            Stmt::StructDecl { name, generic_params, fields, visibility } => visitor.visit_struct_decl(*name, generic_params, fields, visibility),
            Stmt::ImplBlock { target_type, methods } => visitor.visit_impl_block(*target_type, methods),
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

    fn visit_with(&mut self, allocator: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        // Phase 1 minimum: type-check the allocator expression, then evaluate the body's type.
        // Allocator semantics (stack push/pop, binding resolution) land in a later phase.
        self.visit_expr(allocator)?;
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

    fn visit_struct_decl(&mut self, name: DefaultSymbol, generic_params: &Vec<DefaultSymbol>, fields: &Vec<StructField>, visibility: &Visibility) -> Result<TypeDecl, TypeCheckError> {
        self.visit_struct_decl_impl(name, generic_params, fields, visibility)
    }

    fn visit_impl_block(&mut self, target_type: DefaultSymbol, methods: &Vec<Rc<MethodFunction>>) -> Result<TypeDecl, TypeCheckError> {
        self.visit_impl_block_impl(target_type, methods)
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
