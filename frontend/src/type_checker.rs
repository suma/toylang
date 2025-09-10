use std::collections::HashMap;
use std::rc::Rc;
use string_interner::{DefaultStringInterner, DefaultSymbol};
use crate::ast::*;
use crate::type_decl::*;
use crate::visitor::{AstVisitor, ProgramVisitor};
use crate::module_resolver::ModuleResolver;

// Builtin function signature definition
#[derive(Debug, Clone)]
pub struct BuiltinFunctionSignature {
    pub func: BuiltinFunction,
    pub arg_count: usize,
    pub arg_types: Vec<TypeDecl>,
    pub return_type: TypeDecl,
}

// Import new modular structure
pub mod core;
pub mod context;
pub mod error;
pub mod function;
pub mod generics;
pub mod inference;
pub mod optimization;

pub use core::CoreReferences;
pub use context::{TypeCheckContext, VarState};
pub use error::{SourceLocation, TypeCheckError, TypeCheckErrorKind};
pub use function::FunctionCheckingState;
pub use generics::GenericTypeChecking;
pub use inference::TypeInferenceState;
pub use optimization::PerformanceOptimization;

mod traits;
pub use traits::*;

mod literal_checker;
mod expression;
mod statement;
mod struct_literal;
mod collections;
mod builtin;
mod utility;
mod type_conversion;
mod tests;

// Struct definitions moved to separate modules

pub struct TypeCheckerVisitor<'a> {
    pub core: CoreReferences<'a>,
    pub context: TypeCheckContext,
    pub type_inference: TypeInferenceState,
    pub function_checking: FunctionCheckingState,
    pub optimization: PerformanceOptimization,
    pub errors: Vec<TypeCheckError>,
    pub source_code: Option<&'a str>,
    // Module system support
    pub current_package: Option<Vec<DefaultSymbol>>,
    pub imported_modules: HashMap<Vec<DefaultSymbol>, Vec<DefaultSymbol>>, // alias -> full_path
    // Track transformed expressions for Number -> concrete type conversions
    pub transformed_exprs: HashMap<ExprRef, Expr>,
    // Builtin method registry: (TypeDecl, method_name) -> BuiltinMethod
    pub builtin_methods: HashMap<(TypeDecl, String), BuiltinMethod>,
    // Builtin function signatures table
    pub builtin_function_signatures: Vec<BuiltinFunctionSignature>,
}




impl<'a> TypeCheckerVisitor<'a> {
    /// Create a TypeCheckerVisitor with program - processes package and imports automatically
    pub fn with_program(program: &'a mut Program, string_interner: &'a DefaultStringInterner) -> Self {
        // Clone package and imports to avoid borrowing conflicts
        let package_decl = program.package_decl.clone();
        let imports = program.imports.clone();
        
        let mut visitor = Self {
            core: CoreReferences::from_program(program, string_interner),
            context: TypeCheckContext::new(),
            type_inference: TypeInferenceState::new(),
            function_checking: FunctionCheckingState::new(),
            optimization: PerformanceOptimization::new(),
            errors: Vec::new(),
            source_code: None,
            current_package: None,
            imported_modules: HashMap::new(),
            builtin_methods: Self::create_builtin_method_registry(),
            builtin_function_signatures: TypeCheckerVisitor::create_builtin_function_signatures(),
            transformed_exprs: std::collections::HashMap::new(),
        };
        
        // Process package and imports immediately
        if let Some(ref package_decl) = package_decl {
            let _ = visitor.visit_package(package_decl);
        }
        
        for import_decl in &imports {
            let _ = visitor.visit_import(&import_decl);
        }
        
        visitor
    }

    // Keep the old API for backward compatibility
    pub fn new(stmt_pool: &'a StmtPool, expr_pool: &'a mut ExprPool, string_interner: &'a DefaultStringInterner, location_pool: &'a LocationPool) -> Self {
        Self {
            core: CoreReferences::new(stmt_pool, expr_pool, string_interner, location_pool),
            context: TypeCheckContext::new(),
            type_inference: TypeInferenceState::new(),
            function_checking: FunctionCheckingState::new(),
            optimization: PerformanceOptimization::new(),
            errors: Vec::new(),
            source_code: None,
            current_package: None,
            imported_modules: HashMap::new(),
            transformed_exprs: HashMap::new(),
            builtin_methods: Self::create_builtin_method_registry(),
            builtin_function_signatures: TypeCheckerVisitor::create_builtin_function_signatures(),
        }
    }
    
    fn create_builtin_function_signatures() -> Vec<BuiltinFunctionSignature> {
        Self::create_builtin_function_signatures_impl()
    }
    
    /// Create a TypeCheckerVisitor with module resolver for import handling
    pub fn with_module_resolver(
        stmt_pool: &'a StmtPool,
        expr_pool: &'a mut ExprPool,
        string_interner: &'a DefaultStringInterner,
        location_pool: &'a LocationPool,
        module_resolver: &'a mut ModuleResolver,
    ) -> Self {
        Self {
            core: CoreReferences::with_module_resolver(stmt_pool, expr_pool, string_interner, location_pool, module_resolver),
            context: TypeCheckContext::new(),
            type_inference: TypeInferenceState::new(),
            function_checking: FunctionCheckingState::new(),
            optimization: PerformanceOptimization::new(),
            errors: Vec::new(),
            source_code: None,
            current_package: None,
            imported_modules: HashMap::new(),
            builtin_methods: Self::create_builtin_method_registry(),
            builtin_function_signatures: TypeCheckerVisitor::create_builtin_function_signatures(),
            transformed_exprs: std::collections::HashMap::new(),
        }
    }
    
    pub fn with_source_code(mut self, source: &'a str) -> Self {
        self.source_code = Some(source);
        self
    }

    fn process_val_type(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        let expr_ty = match expr {
            Some(e) => {
                // Set type hint for proper type inference
                let old_hint = self.setup_type_hint_for_val(type_decl);
                let ty = self.visit_expr(e)?;
                
                // Apply type transformations and get final type
                self.apply_type_transformations_for_expr(type_decl, &ty, e)?;
                let final_ty = self.determine_final_type_for_expr(type_decl, &ty);
                
                // Restore previous hint
                self.type_inference.type_hint = old_hint;
                if final_ty == TypeDecl::Unit {
                    return Err(TypeCheckError::type_mismatch(TypeDecl::Unknown, final_ty.clone()));
                }
                Some(final_ty)
            }
            None => None,
        };

        match (type_decl, expr_ty.as_ref()) {
            (Some(TypeDecl::Unknown), Some(ty)) => {
                self.context.set_var(name, ty.clone());
            }
            (Some(decl), Some(ty)) => {
                if decl != ty {
                    return Err(TypeCheckError::type_mismatch(decl.clone(), ty.clone()));
                }
                self.context.set_var(name, ty.clone());
            }
            (None, Some(ty)) => {
                // No explicit type declaration - store the inferred type
                self.context.set_var(name, ty.clone());
            }
            (Some(decl), None) => {
                // Explicit type but no initial value - register with declared type
                self.context.set_var(name, decl.clone());
            }
            (None, None) => {
                // No type declaration and no initial value - use Unknown type
                self.context.set_var(name, TypeDecl::Unknown);
            }
        }

        Ok(TypeDecl::Unit)
    }

    pub fn type_check(&mut self, func: Rc<Function>) -> Result<TypeDecl, TypeCheckError> {
        let mut last = TypeDecl::Unit;
        let s = func.code.clone();

        // Is already checked
        match self.function_checking.is_checked_fn.get(&func.name) {
            Some(Some(result_ty)) => return Ok(result_ty.clone()),  // already checked
            Some(None) => return Ok(TypeDecl::Unknown), // now checking
            None => (),
        }

        // Now checking...
        self.function_checking.is_checked_fn.insert(func.name, None);

        // Clear type cache at the start of each function to limit cache scope
        self.optimization.type_cache.clear();

        self.function_checking.call_depth += 1;

        let statements = match self.core.stmt_pool.get(&s).ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference"))? {
            Stmt::Expression(e) => {
                match self.core.expr_pool.get(&e).ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference"))? {
                    Expr::Block(statements) => {
                        statements.clone()  // Clone required: statements is used in multiple loops and we need mutable access to self
                    }
                    _ => {
                        return Err(TypeCheckError::generic_error("type_check: expected block expression"));
                    }
                }
            }
            _ => return Err(TypeCheckError::generic_error("type_check: expected block statement")),
        };

        self.push_context();
        // Define variable of argument for this `func`
        func.parameter.iter().for_each(|(name, type_decl)| {
            self.context.set_var(*name, type_decl.clone());
        });

        // Pre-scan for explicit type declarations and establish global type context
        let mut global_numeric_type: Option<TypeDecl> = None;
        for s in statements.iter() {
            if let Some(stmt) = self.core.stmt_pool.get(&s) {
                match stmt {
                    Stmt::Val(_, Some(type_decl), _) | Stmt::Var(_, Some(type_decl), _) => {
                        if matches!(type_decl, TypeDecl::Int64 | TypeDecl::UInt64) {
                            global_numeric_type = Some(type_decl.clone());
                            break; // Use the first explicit numeric type found
                        }
                    }
                    _ => {}
                }
            }
        }
        
        // Set global type hint if found
        let original_hint = self.type_inference.type_hint.clone();
        if let Some(ref global_type) = global_numeric_type {
            self.type_inference.type_hint = Some(global_type.clone());
        } else if let Some(ref return_type) = func.return_type {
            // Use function return type as type hint for Number literals
            self.type_inference.type_hint = Some(return_type.clone());
        }

        for stmt in statements.iter() {
            let stmt_obj = self.core.stmt_pool.get(&stmt).ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference"))?;
            let res = stmt_obj.clone().accept(self);
            if res.is_err() {
                return res;
            } else {
                last = res?;
            }
        }
        self.pop_context();
        self.function_checking.call_depth -= 1;

        // Restore original type hint
        self.type_inference.type_hint = original_hint;

        // Final pass: convert any remaining Number literals to default type (UInt64)
        self.finalize_number_types()?;
        
        // Apply all accumulated expression transformations
        self.apply_expr_transformations();
        
        // Check if the function body type matches the declared return type
        if let Some(ref expected_return_type) = func.return_type {
            let types_match = match (&last, expected_return_type) {
                // Special case for arrays: if actual type has size 0 (dynamic), check if element types are compatible
                (TypeDecl::Array(actual_elements, 0), TypeDecl::Array(expected_elements, _)) => {
                    // For dynamic arrays (slice results), check if element types are compatible
                    if expected_elements.is_empty() {
                        // Empty array expected - this is always compatible with dynamic slice
                        true
                    } else if actual_elements.len() == 1 && !expected_elements.is_empty() {
                        // All expected elements should be the same type as the single actual element
                        expected_elements.iter().all(|expected_elem| expected_elem == &actual_elements[0])
                    } else {
                        actual_elements == expected_elements
                    }
                }
                // Regular type comparison
                _ => &last == expected_return_type
            };
            
            if !types_match {
                // Create location information from function node with calculated line and column
                let func_location = self.node_to_source_location(&func.node);
                
                // Add detailed information about the type mismatch
                let func_name_str = self.core.string_interner.resolve(func.name).unwrap_or("<unknown>");
                
                // Debug: If this is Generic type, show more details
                let additional_info = if let TypeDecl::Generic(sym) = &last {
                    let sym_str = self.core.string_interner.resolve(*sym).unwrap_or("<unknown>");
                    format!(" [Generic symbol: '{}']", sym_str)
                } else {
                    String::new()
                };
                
                let detailed_context = format!(
                    "function return type (function: {}, expected: {:?}, got: {:?}{})",
                    func_name_str, expected_return_type, last, additional_info
                );
                
                return Err(TypeCheckError::type_mismatch(
                    expected_return_type.clone(),
                    last.clone()
                ).with_location(func_location)
                .with_context(&detailed_context));
            }
        }
        
        self.function_checking.is_checked_fn.insert(func.name, Some(last.clone()));
        Ok(last)
    }
}
pub trait Acceptable {
    fn accept(&mut self, visitor: &mut dyn AstVisitor) -> Result<TypeDecl, TypeCheckError>;
}

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
            
            // Check if any part is a reserved keyword
            if is_reserved_keyword(name_str) {
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
            
            // Check if any part is a reserved keyword  
            if is_reserved_keyword(name_str) {
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
        // Check cache first
        if let Some(cached_type) = self.get_cached_type(expr) {
            return Ok(cached_type.clone());
        }
        
        // Set up context hint for nested expressions
        let original_hint = self.type_inference.type_hint.clone();
        let expr_obj = self.core.expr_pool.get(&expr).ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference"))?;
        
        
        let result = expr_obj.clone().accept(self);
        
        // If an error occurred, try to add location information if not already present
        let result = match result {
            Err(mut error) if error.location.is_none() => {
                error.location = self.get_expr_location(expr);
                Err(error)
            }
            other => other,
        };
        
        // Cache the result if successful
        if let Ok(ref result_type) = result {
            self.cache_type(&expr, result_type.clone());
            // Record the type in the comprehensive expr_types mapping
            self.type_inference.set_expr_type(*expr, result_type.clone());
            
            // Record expression type for code generation
            
            // Context propagation: if this expression resolved to a concrete numeric type,
            // and we don't have a current hint, set it for sibling expressions
            if original_hint.is_none() && (result_type == &TypeDecl::Int64 || result_type == &TypeDecl::UInt64) {
                if self.type_inference.type_hint.is_none() {
                    self.type_inference.type_hint = Some(result_type.clone());
                }
            }
        }
        
        result
    }

    fn visit_stmt(&mut self, stmt: &StmtRef) -> Result<TypeDecl, TypeCheckError> {
        let mut stmt_val = self.core.stmt_pool.get(&stmt).unwrap_or(Stmt::Break).clone();
        
        // Debug output for statement type
        
        let result = stmt_val.accept(self);
        
        // If an error occurred, try to add location information if not already present
        match result {
            Err(mut error) if error.location.is_none() => {
                error.location = self.get_stmt_location(stmt);
                Err(error)
            }
            other => other,
        }
    }
    
    // =========================================================================
    // Expression Type Checking
    // =========================================================================

    fn visit_unary(&mut self, op: &UnaryOp, operand: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let op = op.clone();
        let operand = operand.clone();
        let operand_ty = {
            let operand_obj = self.core.expr_pool.get(&operand).ok_or_else(|| TypeCheckError::generic_error("Invalid operand expression reference"))?;
            operand_obj.clone().accept(self)?
        };
        
        // Resolve type with automatic conversion for Number type
        let resolved_ty = if operand_ty == TypeDecl::Number {
            // For bitwise NOT, prefer UInt64 for Number type
            if op == UnaryOp::BitwiseNot {
                TypeDecl::UInt64
            } else {
                TypeDecl::Bool
            }
        } else {
            operand_ty.clone()
        };
        
        // Transform AST node if type conversion occurred
        if operand_ty == TypeDecl::Number && resolved_ty != TypeDecl::Number {
            self.transform_numeric_expr(&operand, &resolved_ty)?;
        }
        
        let result_type = match op {
            UnaryOp::BitwiseNot => {
                if resolved_ty == TypeDecl::UInt64 {
                    TypeDecl::UInt64
                } else if resolved_ty == TypeDecl::Int64 {
                    TypeDecl::Int64
                } else {
                    let mut error = TypeCheckError::type_mismatch_operation("bitwise NOT", resolved_ty.clone(), TypeDecl::Unit);
                    if let Some(location) = self.get_expr_location(&operand) {
                        error = error.with_location(location);
                    }
                    return Err(error);
                }
            }
            UnaryOp::LogicalNot => {
                if resolved_ty == TypeDecl::Bool {
                    TypeDecl::Bool
                } else {
                    let mut error = TypeCheckError::type_mismatch_operation("logical NOT", resolved_ty.clone(), TypeDecl::Unit);
                    if let Some(location) = self.get_expr_location(&operand) {
                        error = error.with_location(location);
                    }
                    return Err(error);
                }
            }
        };
        
        Ok(result_type)
    }

    fn visit_binary(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let op = op.clone();
        let lhs = lhs.clone();
        let rhs = rhs.clone();
        let lhs_ty = {
            let lhs_obj = self.core.expr_pool.get(&lhs).ok_or_else(|| TypeCheckError::generic_error("Invalid left-hand expression reference"))?;
            lhs_obj.clone().accept(self)?
        };
        let rhs_ty = {
            let rhs_obj = self.core.expr_pool.get(&rhs).ok_or_else(|| TypeCheckError::generic_error("Invalid right-hand expression reference"))?;
            rhs_obj.clone().accept(self)?
        };
        
        // Special handling for shift operations where right operand must be UInt64
        let (resolved_lhs_ty, resolved_rhs_ty) = if matches!(op, Operator::LeftShift | Operator::RightShift) {
            self.resolve_shift_operand_types(&lhs_ty, &rhs_ty)
        } else {
            // For other operations, use normal resolution
            self.resolve_numeric_types(&lhs_ty, &rhs_ty)
                .map_err(|mut error| {
                    if error.location.is_none() {
                        if let Some(location) = self.get_expr_location(&lhs) {
                            error = error.with_location(location);
                        }
                    }
                    error
                })?
        };
        
        // Context propagation: if we have a type hint, propagate it to Number expressions
        if let Some(hint) = self.type_inference.type_hint.clone() {
            if lhs_ty == TypeDecl::Number && (hint == TypeDecl::Int64 || hint == TypeDecl::UInt64) {
                self.propagate_type_to_number_expr(&lhs, &hint)?;
            }
            if rhs_ty == TypeDecl::Number && (hint == TypeDecl::Int64 || hint == TypeDecl::UInt64) {
                self.propagate_type_to_number_expr(&rhs, &hint)?;
            }
        }
        
        // Record Number usage context for later finalization
        self.record_number_usage_context(&lhs, &lhs_ty, &resolved_lhs_ty)?;
        self.record_number_usage_context(&rhs, &rhs_ty, &resolved_rhs_ty)?;
        
        // Immediate propagation: if one side has concrete type, propagate to Number variables
        if resolved_lhs_ty != TypeDecl::Number && rhs_ty == TypeDecl::Number {
            self.propagate_to_number_variable(&rhs, &resolved_lhs_ty)?;
        }
        if resolved_rhs_ty != TypeDecl::Number && lhs_ty == TypeDecl::Number {
            self.propagate_to_number_variable(&lhs, &resolved_rhs_ty)?;
        }
        
        // Transform AST nodes if type conversion occurred
        if lhs_ty == TypeDecl::Number && resolved_lhs_ty != TypeDecl::Number {
            self.transform_numeric_expr(&lhs, &resolved_lhs_ty)?;
        }
        if rhs_ty == TypeDecl::Number && resolved_rhs_ty != TypeDecl::Number {
            self.transform_numeric_expr(&rhs, &resolved_rhs_ty)?;
        }
        
        // Update variable types if identifiers were involved in type conversion
        self.update_identifier_types(&lhs, &lhs_ty, &resolved_lhs_ty)?;
        self.update_identifier_types(&rhs, &rhs_ty, &resolved_rhs_ty)?;
        
        let result_type = match op {
            Operator::IAdd if resolved_lhs_ty == TypeDecl::String && resolved_rhs_ty == TypeDecl::String => {
                TypeDecl::String
            }
            Operator::IAdd | Operator::ISub | Operator::IDiv | Operator::IMul => {
                if resolved_lhs_ty == TypeDecl::UInt64 && resolved_rhs_ty == TypeDecl::UInt64 {
                    TypeDecl::UInt64
                } else if resolved_lhs_ty == TypeDecl::Int64 && resolved_rhs_ty == TypeDecl::Int64 {
                    TypeDecl::Int64
                } else {
                    let mut error = TypeCheckError::type_mismatch_operation("arithmetic", resolved_lhs_ty.clone(), resolved_rhs_ty.clone());
                    if let Some(location) = self.get_expr_location(&lhs) {
                        error = error.with_location(location);
                    }
                    return Err(error);
                }
            }
            Operator::LE | Operator::LT | Operator::GE | Operator::GT | Operator::EQ | Operator::NE => {
                if (resolved_lhs_ty == TypeDecl::UInt64 || resolved_lhs_ty == TypeDecl::Int64) && 
                   (resolved_rhs_ty == TypeDecl::UInt64 || resolved_rhs_ty == TypeDecl::Int64) {
                    TypeDecl::Bool
                } else if resolved_lhs_ty == TypeDecl::Bool && resolved_rhs_ty == TypeDecl::Bool {
                    TypeDecl::Bool
                } else {
                    let mut error = TypeCheckError::type_mismatch_operation("comparison", resolved_lhs_ty.clone(), resolved_rhs_ty.clone());
                    if let Some(location) = self.get_expr_location(&lhs) {
                        error = error.with_location(location);
                    }
                    return Err(error);
                }
            }
            Operator::LogicalAnd | Operator::LogicalOr => {
                if resolved_lhs_ty == TypeDecl::Bool && resolved_rhs_ty == TypeDecl::Bool {
                    TypeDecl::Bool
                } else {
                    let mut error = TypeCheckError::type_mismatch_operation("logical", resolved_lhs_ty.clone(), resolved_rhs_ty.clone());
                    if let Some(location) = self.get_expr_location(&lhs) {
                        error = error.with_location(location);
                    }
                    return Err(error);
                }
            }
            Operator::BitwiseAnd | Operator::BitwiseOr | Operator::BitwiseXor => {
                if resolved_lhs_ty == TypeDecl::UInt64 && resolved_rhs_ty == TypeDecl::UInt64 {
                    TypeDecl::UInt64
                } else if resolved_lhs_ty == TypeDecl::Int64 && resolved_rhs_ty == TypeDecl::Int64 {
                    TypeDecl::Int64
                } else {
                    let mut error = TypeCheckError::type_mismatch_operation("bitwise", resolved_lhs_ty.clone(), resolved_rhs_ty.clone());
                    if let Some(location) = self.get_expr_location(&lhs) {
                        error = error.with_location(location);
                    }
                    return Err(error);
                }
            }
            Operator::LeftShift | Operator::RightShift => {
                // For shift operations, right operand must be UInt64
                if resolved_rhs_ty != TypeDecl::UInt64 {
                    let mut error = TypeCheckError::type_mismatch_operation("shift", TypeDecl::UInt64, resolved_rhs_ty.clone());
                    if let Some(location) = self.get_expr_location(&rhs) {
                        error = error.with_location(location);
                    }
                    return Err(error);
                }
                // Left operand can be either UInt64 or Int64
                if resolved_lhs_ty == TypeDecl::UInt64 {
                    TypeDecl::UInt64
                } else if resolved_lhs_ty == TypeDecl::Int64 {
                    TypeDecl::Int64
                } else {
                    let mut error = TypeCheckError::type_mismatch_operation("shift", resolved_lhs_ty.clone(), TypeDecl::UInt64);
                    if let Some(location) = self.get_expr_location(&lhs) {
                        error = error.with_location(location);
                    }
                    return Err(error);
                }
            }
        };
        
        Ok(result_type)
    }

    fn visit_block(&mut self, statements: &Vec<StmtRef>) -> Result<TypeDecl, TypeCheckError> {
        let mut last_empty = true;
        let mut last: Option<TypeDecl> = None;
        
        // Clear type cache at the start of each block to limit cache scope to current block
        self.optimization.type_cache.clear();
        
        // Pre-scan for explicit type declarations and establish global type context
        let mut global_numeric_type: Option<TypeDecl> = None;
        for s in statements.iter() {
            if let Some(stmt) = self.core.stmt_pool.get(&s) {
                match stmt {
                    Stmt::Val(_, Some(type_decl), _) | Stmt::Var(_, Some(type_decl), _) => {
                        if matches!(type_decl, TypeDecl::Int64 | TypeDecl::UInt64) {
                            global_numeric_type = Some(type_decl.clone());
                            break; // Use the first explicit numeric type found
                        }
                    }
                    _ => {}
                }
            }
        }
        
        // Set global type hint if found
        let original_hint = self.type_inference.type_hint.clone();
        if let Some(ref global_type) = global_numeric_type {
            self.type_inference.type_hint = Some(global_type.clone());
        }
        
        // This code assumes Block(expression) don't make nested function
        // so `return` expression always return for this context.
        for s in statements.iter() {
            let stmt = self.core.stmt_pool.get(&s).ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference in block"))?;
            let stmt_type = match stmt {
                Stmt::Return(None) => Ok(TypeDecl::Unit),
                Stmt::Return(ret_ty) => {
                    if let Some(e) = ret_ty {
                        let e = e.clone();
                        let expr_obj = self.core.expr_pool.get(&e).ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference in return"))?;
                        let ty = expr_obj.clone().accept(self)?;
                        if last_empty {
                            last_empty = false;
                            Ok(ty)
                        } else if let Some(last_ty) = last.clone() {
                            if last_ty == ty {
                                Ok(ty)
                            } else {
                                return Err(TypeCheckError::type_mismatch(last_ty, ty).with_context("return statement"));
                            }
                        } else {
                            Ok(ty)
                        }
                    } else {
                        Ok(TypeDecl::Unit)
                    }
                }
                _ => {
                    let stmt_obj = self.core.stmt_pool.get(&s).ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference"))?;
                    stmt_obj.clone().accept(self)
                }
            };

            match stmt_type {
                Ok(def_ty) => last = Some(def_ty),
                Err(e) => return Err(e),
            }
        }
        
        // Restore original type hint
        self.type_inference.type_hint = original_hint;

        if let Some(last_type) = last {
            Ok(last_type)
        } else {
            Err(TypeCheckError::generic_error("Empty block - no return value"))
        }
    }


    fn visit_if_elif_else(&mut self, _cond: &ExprRef, then_block: &ExprRef, elif_pairs: &Vec<(ExprRef, ExprRef)>, else_block: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        // Collect all block types
        let mut block_types = Vec::new();

        // Check if-block
        let if_block = then_block.clone();
        let is_if_empty = match self.core.expr_pool.get(&if_block).ok_or_else(|| TypeCheckError::generic_error("Invalid if block expression reference"))? {
            Expr::Block(expressions) => expressions.is_empty(),
            _ => false,
        };
        if !is_if_empty {
            let if_expr = self.core.expr_pool.get(&if_block).ok_or_else(|| TypeCheckError::generic_error("Invalid if block expression reference"))?;
            let if_ty = if_expr.clone().accept(self)?;
            block_types.push(if_ty);
        }

        // Check elif-blocks
        for (_, elif_block) in elif_pairs {
            let elif_block = elif_block.clone();
            let is_elif_empty = match self.core.expr_pool.get(&elif_block).ok_or_else(|| TypeCheckError::generic_error("Invalid elif block expression reference"))? {
                Expr::Block(expressions) => expressions.is_empty(),
                _ => false,
            };
            if !is_elif_empty {
                let elif_expr = self.core.expr_pool.get(&elif_block).ok_or_else(|| TypeCheckError::generic_error("Invalid elif block expression reference"))?;
                let elif_ty = elif_expr.clone().accept(self)?;
                block_types.push(elif_ty);
            }
        }

        // Check else-block
        let else_block = else_block.clone();
        let is_else_empty = match self.core.expr_pool.get(&else_block).ok_or_else(|| TypeCheckError::generic_error("Invalid else block expression reference"))? {
            Expr::Block(expressions) => expressions.is_empty(),
            _ => false,
        };
        if !is_else_empty {
            let else_expr = self.core.expr_pool.get(&else_block).ok_or_else(|| TypeCheckError::generic_error("Invalid else block expression reference"))?;
            let else_ty = else_expr.clone().accept(self)?;
            block_types.push(else_ty);
        }

        // If no blocks have values or all blocks are empty, return Unit
        if block_types.is_empty() {
            return Ok(TypeDecl::Unit);
        }

        // Check if all blocks have the same type
        let first_type = &block_types[0];
        for block_type in &block_types[1..] {
            if block_type != first_type {
                return Ok(TypeDecl::Unit); // Different types, return Unit
            }
        }

        Ok(first_type.clone())
    }

    fn visit_assign(&mut self, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let lhs = lhs.clone();
        let rhs = rhs.clone();
        let lhs_ty = {
            let lhs_obj = self.core.expr_pool.get(&lhs).ok_or_else(|| TypeCheckError::generic_error("Invalid left-hand expression reference"))?;
            lhs_obj.clone().accept(self)?
        };
        let rhs_ty = {
            let rhs_obj = self.core.expr_pool.get(&rhs).ok_or_else(|| TypeCheckError::generic_error("Invalid right-hand expression reference"))?;
            rhs_obj.clone().accept(self)?
        };
        // Allow assignment compatibility
        if lhs_ty != rhs_ty {
            match (&lhs_ty, &rhs_ty) {
                // Allow unknown type (null values) assignment to any concrete type
                (_, TypeDecl::Unknown) => {
                    // Allow assignment of unknown/null to any type
                }
                // Allow assignment when types are equivalent (for type inference)
                (TypeDecl::Unknown, _) => {
                    // Allow assignment from any type to unknown (type inference)
                }
                _ => {
                    return Err(TypeCheckError::type_mismatch(lhs_ty, rhs_ty).with_context("assignment"));
                }
            }
        }
        Ok(lhs_ty)
    }

    fn visit_identifier(&mut self, name: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        if let Some(val_type) = self.context.get_var(name) {
            // Return the stored type, which may be Number for type inference
            Ok(val_type.clone())
        } else if let Some(fun) = self.context.get_fn(name) {
            Ok(fun.return_type.clone().unwrap_or(TypeDecl::Unknown))
        } else if let Some(generic_type) = self.type_inference.lookup_generic_type(name) {
            // Check if this is a generic type parameter
            Ok(generic_type.clone())
        } else if let Some(_struct_def) = self.context.get_struct_definition(name) {
            // Check if this is a struct type
            Ok(TypeDecl::Struct(name, vec![]))
        } else {
            let name_str = self.core.string_interner.resolve(name).unwrap_or("<NOT_FOUND>");
            // Note: Location information will be added by visit_expr
            return Err(TypeCheckError::not_found("Identifier", name_str));
        }
    }
    
    // =========================================================================
    // Function and Method Type Checking
    // =========================================================================

    fn visit_call(&mut self, fn_name: DefaultSymbol, args_ref: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.push_context();
        if let Some(fun) = self.context.get_fn(fn_name) {
            // Check visibility access control
            if let Err(err) = self.check_function_access(&fun) {
                self.pop_context();
                return Err(err);
            }
            
            // Handle generic function calls
            if !fun.generic_params.is_empty() {
                return self.visit_generic_call(fn_name, args_ref, &fun);
            }
            
            let status = self.function_checking.is_checked_fn.get(&fn_name);
            if status.is_none() || status.as_ref().and_then(|s| s.as_ref()).is_none() {
                // not checked yet
                let fun_copy = self.context.get_fn(fn_name).ok_or_else(|| TypeCheckError::not_found("Function", "<INTERNAL_ERROR>"))?;
                self.type_check(fun_copy.clone())?;
            }

            // Type check function arguments with proper type hints
            // Clone data we need to avoid borrowing conflicts
            let args_data = if let Some(args_expr) = self.core.expr_pool.get(&args_ref) {
                if let Expr::ExprList(args) = args_expr {
                    Some(args.clone())
                } else {
                    None
                }
            } else {
                self.pop_context();
                return Err(TypeCheckError::generic_error("Invalid arguments reference"));
            };
            
            if let Some(args) = args_data {
                let param_types: Vec<_> = fun.parameter.iter().map(|(_, ty)| ty.clone()).collect();
                
                // Check argument count
                if args.len() != param_types.len() {
                    self.pop_context();
                    let fn_name_str = self.core.string_interner.resolve(fn_name).unwrap_or("<NOT_FOUND>");
                    return Err(TypeCheckError::generic_error(&format!(
                        "Function '{}' argument count mismatch: expected {}, found {}",
                        fn_name_str, param_types.len(), args.len()
                    )));
                }
                
                // Type check each argument with expected type as hint
                let original_hint = self.type_inference.type_hint.clone();
                for (arg_index, (arg, expected_type)) in args.iter().zip(&param_types).enumerate() {
                    // Set type hint for this argument
                    self.type_inference.type_hint = Some(expected_type.clone());
                    let arg_type = self.visit_expr(arg)?;
                    
                    // Check type compatibility
                    if arg_type != *expected_type && arg_type != TypeDecl::Unknown {
                        // Restore hint before returning error
                        self.type_inference.type_hint = original_hint;
                        self.pop_context();
                        let fn_name_str = self.core.string_interner.resolve(fn_name).unwrap_or("<NOT_FOUND>");
                        return Err(TypeCheckError::generic_error(&format!(
                            "Type error: expected {:?}, found {:?}. Function '{}' argument {} type mismatch",
                            expected_type, arg_type, fn_name_str, arg_index + 1
                        )));
                    }
                }
                // Restore original hint
                self.type_inference.type_hint = original_hint;
            }
            
            self.pop_context();
            Ok(fun.return_type.clone().unwrap_or(TypeDecl::Unknown))
        } else {
            self.pop_context();
            let fn_name_str = self.core.string_interner.resolve(fn_name).unwrap_or("<NOT_FOUND>");
            Err(TypeCheckError::not_found("Function", fn_name_str))
        }
    }
    
    // =========================================================================
    // Literal Type Checking
    // =========================================================================

    fn visit_int64_literal(&mut self, _value: &i64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Int64)
    }

    fn visit_uint64_literal(&mut self, _value: &u64) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::UInt64)
    }

    fn visit_number_literal(&mut self, _value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Number)
    }

    fn visit_string_literal(&mut self, _value: DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::String)
    }

    fn visit_boolean_literal(&mut self, _value: &Expr) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Bool)
    }

    fn visit_null_literal(&mut self) -> Result<TypeDecl, TypeCheckError> {
        // Null value type is determined by context
        // If we have a type hint, use that; otherwise return Unknown
        if let Some(hint) = self.type_inference.get_type_hint() {
            Ok(hint)
        } else {
            Ok(TypeDecl::Unknown)
        }
    }
    

    // Array and Collection Type Checking
    // =========================================================================

    fn visit_expr_list(&mut self, _items: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        Ok(TypeDecl::Unit)
    }

    fn visit_array_literal(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        if elements.is_empty() {
            return Err(TypeCheckError::array_error("Empty array literals are not supported"));
        }

        // Check recursion depth to prevent stack overflow
        if self.type_inference.recursion_depth >= self.type_inference.max_recursion_depth {
            return Err(TypeCheckError::generic_error(
                "Maximum recursion depth reached in array literal type inference - possible circular reference"
            ));
        }
        
        self.type_inference.recursion_depth += 1;
        
        // Execute the main logic and capture result
        let result = self.visit_array_literal_impl(elements);
        
        // Always decrement recursion depth before returning
        self.type_inference.recursion_depth -= 1;
        
        result
    }


    
    
    fn visit_slice_access(&mut self, object: &ExprRef, slice_info: &SliceInfo) -> Result<TypeDecl, TypeCheckError> {
        let object_type = self.visit_expr(object)?;
        
        match object_type {
            TypeDecl::Array(ref element_types, _size) => {
                // Simplified type checking for slice indices
                match slice_info.slice_type {
                    SliceType::SingleElement => {
                        // For single element access, be more strict with type checking
                        if let Some(start_expr) = &slice_info.start {
                            let original_hint = self.type_inference.type_hint.clone();
                            self.type_inference.type_hint = Some(TypeDecl::Int64); // Allow negative indices
                            let start_type = self.visit_expr(start_expr)?;
                            self.type_inference.type_hint = original_hint;

                            if start_type == TypeDecl::UInt64 {
                                self.transform_numeric_expr(start_expr, &TypeDecl::Int64)?;
                            }
                            
                            // Allow UInt64, Int64, or transform Number
                            match start_type {
                                TypeDecl::UInt64 | TypeDecl::Int64 | TypeDecl::Unknown => {
                                    // Valid types
                                }
                                TypeDecl::Number => {
                                    // Transform Number to Int64 (could be negative)
                                    self.transform_numeric_expr(start_expr, &TypeDecl::Int64)?;
                                }
                                _ => {
                                    return Err(TypeCheckError::array_error(&format!(
                                        "Array index must be an integer type, but got {:?}", start_type
                                    )));
                                }
                            }
                        }
                    }
                    SliceType::RangeSlice => {
                        // For range slices, set Int64 hint for potential negative indices
                        let original_hint = self.type_inference.type_hint.clone();
                        self.type_inference.type_hint = Some(TypeDecl::Int64);
                        
                        // Visit start expression if present
                        if let Some(start_expr) = &slice_info.start {
                            let _ = self.visit_expr(start_expr)?;
                        }
                        
                        // Visit end expression if present
                        if let Some(end_expr) = &slice_info.end {
                            let _ = self.visit_expr(end_expr)?;
                        }
                        
                        // Restore original hint
                        self.type_inference.type_hint = original_hint;
                    }
                }
                
                if element_types.is_empty() {
                    return Err(TypeCheckError::array_error("Cannot slice empty array"));
                }
                
                // Use SliceInfo to distinguish single element access vs range slice
                match slice_info.slice_type {
                    SliceType::SingleElement => {
                        // Single element access: arr[i] returns element type
                        Ok(element_types[0].clone())
                    }
                    SliceType::RangeSlice => {
                        // Range slice: arr[start..end] returns array type
                        // Create array type with single element type and size 0 (dynamic)
                        let single_element_type = element_types[0].clone();
                        Ok(TypeDecl::Array(vec![single_element_type], 0))
                    }
                }
            }
            TypeDecl::Dict(ref key_type, ref value_type) => {
                // Dictionary access: dict[key] (only single element access, not slicing)
                if slice_info.is_valid_for_dict() {
                    // Single element access: dict[key]
                    if let Some(index_expr) = &slice_info.start {
                        let index_type = self.visit_expr(index_expr)?;
                        
                        // Verify the index type matches the key type
                        if index_type != **key_type {
                            return Err(TypeCheckError::type_mismatch(
                                *key_type.clone(), index_type
                            ));
                        }
                        
                        Ok(*value_type.clone())
                    } else {
                        Err(TypeCheckError::generic_error("Dictionary access requires key index"))
                    }
                } else {
                    // Range slicing is not supported for dictionaries
                    Err(TypeCheckError::generic_error("Dictionary slicing is not supported - use single key access dict[key]"))
                }
            }
            TypeDecl::Identifier(struct_name) => {
                // Struct access: check for __getitem__ method (only single element access)
                if slice_info.is_valid_for_dict() {
                    // Single element access: struct[key]
                    if let Some(index_expr) = &slice_info.start {
                        let struct_name_str = self.core.string_interner.resolve(struct_name)
                            .ok_or_else(|| TypeCheckError::generic_error("Unknown struct name"))?;
                        
                        // Type check the index first to avoid borrowing conflicts
                        let index_type = self.visit_expr(index_expr)?;
                        
                        // Look for __getitem__ method
                        if let Some(getitem_method) = self.context.get_method_function_by_name(struct_name_str, "__getitem__", self.core.string_interner) {
                            // Check if method has correct signature: __getitem__(self, index: T) -> U
                            if getitem_method.parameter.len() >= 2 {
                                let index_param_type = &getitem_method.parameter[1].1;
                                if index_type != *index_param_type {
                                    return Err(TypeCheckError::type_mismatch(
                                        index_param_type.clone(), index_type
                                    ));
                                }
                                
                                // Return the method's return type
                                if let Some(return_type) = &getitem_method.return_type {
                                    Ok(return_type.clone())
                                } else {
                                    Err(TypeCheckError::generic_error("__getitem__ method must have return type"))
                                }
                            } else {
                                Err(TypeCheckError::generic_error("__getitem__ method must have at least 2 parameters (self, index)"))
                            }
                        } else {
                            Err(TypeCheckError::generic_error(&format!(
                                "Cannot index into type {:?} - no __getitem__ method found", object_type
                            )))
                        }
                    } else {
                        Err(TypeCheckError::generic_error("Struct access requires index"))
                    }
                } else {
                    // Range slicing is not supported for structs
                    Err(TypeCheckError::generic_error("Struct slicing is not supported - use single index access struct[key]"))
                }
            }
            _ => {
                Err(TypeCheckError::generic_error(&format!(
                    "Cannot access type {:?} - only arrays, dictionaries, and structs with __getitem__ are supported", object_type
                )))
            }
        }
    }
    
    fn visit_slice_assign(&mut self, object: &ExprRef, start: &Option<ExprRef>, end: &Option<ExprRef>, value: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let object_type = self.visit_expr(object)?;
        let value_type = self.visit_expr(value)?;
        
        match object_type {
            TypeDecl::Array(ref element_types, _size) => {
                return self.handle_array_slice_assign(element_types, start, end, &value_type);
            }
            TypeDecl::Dict(ref key_type, ref dict_value_type) => {
                // Dictionary assignment: dict[key] = value (only single element assignment)
                if start.is_some() && end.is_none() {
                    // Single element assignment: dict[key] = value
                    if let Some(key_expr) = start {
                        let key_type_result = self.visit_expr(key_expr)?;
                        
                        // Verify the key type matches the dictionary key type
                        if key_type_result != **key_type {
                            return Err(TypeCheckError::type_mismatch(
                                *key_type.clone(), key_type_result
                            ));
                        }
                        
                        // Check value type compatibility with dictionary value type
                        let expected_dict_value_type = &**dict_value_type;
                        if *expected_dict_value_type != TypeDecl::Unknown {
                            let resolved_value_type = if value_type == TypeDecl::Number {
                                // Transform Number value to expected dict value type
                                self.transform_numeric_expr(value, expected_dict_value_type)?;
                                expected_dict_value_type.clone()
                            } else {
                                value_type.clone()
                            };
                            
                            if *expected_dict_value_type != resolved_value_type {
                                return Err(TypeCheckError::generic_error(&format!(
                                    "Dict value type mismatch: expected {:?}, found {:?}", 
                                    expected_dict_value_type, resolved_value_type
                                )));
                            }
                            Ok(resolved_value_type)
                        } else {
                            Ok(value_type.clone())
                        }
                    } else {
                        Err(TypeCheckError::generic_error("Dictionary assignment requires key index"))
                    }
                } else {
                    // Range slice assignment not supported for dictionaries
                    Err(TypeCheckError::generic_error("Dictionary slice assignment not supported - use single key assignment dict[key] = value"))
                }
            }
            TypeDecl::Identifier(struct_name) => {
                // Struct assignment: check for __setitem__ method (only single element assignment)
                if start.is_some() && end.is_none() {
                    // Single element assignment: struct[key] = value
                    if let Some(key_expr) = start {
                        let struct_name_str = self.core.string_interner.resolve(struct_name)
                            .ok_or_else(|| TypeCheckError::generic_error("Unknown struct name"))?;
                        
                        // Type check the key and value
                        let key_type_result = self.visit_expr(key_expr)?;
                        
                        // Look for __setitem__ method
                        if let Some(setitem_method) = self.context.get_method_function_by_name(struct_name_str, "__setitem__", self.core.string_interner) {
                            // Check if method has correct signature: __setitem__(self, key: T, value: U)
                            if setitem_method.parameter.len() >= 3 {
                                let key_param_type = &setitem_method.parameter[1].1;
                                let value_param_type = &setitem_method.parameter[2].1;
                                
                                // Check key type matches
                                if key_type_result != *key_param_type {
                                    return Err(TypeCheckError::type_mismatch(
                                        key_param_type.clone(), key_type_result
                                    ));
                                }
                                
                                // Check value type matches
                                if value_type != *value_param_type {
                                    return Err(TypeCheckError::type_mismatch(
                                        value_param_type.clone(), value_type
                                    ));
                                }
                                
                                // Assignment returns the value type
                                Ok(value_type)
                            } else {
                                Err(TypeCheckError::generic_error("__setitem__ method must have at least 3 parameters (self, key, value)"))
                            }
                        } else {
                            Err(TypeCheckError::generic_error(&format!(
                                "Cannot assign to struct type {:?} - no __setitem__ method found", object_type
                            )))
                        }
                    } else {
                        Err(TypeCheckError::generic_error("Struct assignment requires key index"))
                    }
                } else {
                    // Range slice assignment not supported for structs
                    Err(TypeCheckError::generic_error("Struct slice assignment not supported - use single key assignment struct[key] = value"))
                }
            }
            _ => {
                Err(TypeCheckError::generic_error(&format!(
                    "Cannot assign to type {:?} - only arrays, dictionaries, and structs with __setitem__ are supported", object_type
                )))
            }
        }
    }
    
    fn visit_associated_function_call(&mut self, struct_name: DefaultSymbol, function_name: DefaultSymbol, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Handle Container::function_name(args) type calls for any associated function
        
        // Check if this is a known struct
        if !self.context.is_generic_struct(struct_name) {
            return Err(TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)));
        }
        
        // Look for the associated function in the struct's impl block
        let struct_name_str = self.core.string_interner.resolve(struct_name).unwrap_or("<unknown>");
        let function_name_str = self.core.string_interner.resolve(function_name).unwrap_or("<unknown>");
        
        
        if let Some(method) = self.context.get_struct_method(struct_name, function_name) {
            // Clone the method to avoid borrowing issues
            let method_clone = method.clone();
            // Handle generic associated function call with type inference
            self.handle_generic_associated_function_call(struct_name, function_name, args, &method_clone)
        } else {
            
            Err(TypeCheckError::generic_error(&format!(
                "Associated function '{}' not found for struct '{:?}'", 
                function_name_str, struct_name
            )))
        }
    }
    
    fn visit_dict_literal(&mut self, entries: &Vec<(ExprRef, ExprRef)>) -> Result<TypeDecl, TypeCheckError> {
        if entries.is_empty() {
            // Empty dict - type will be inferred from usage or type hint
            if let Some(TypeDecl::Dict(key_type, value_type)) = &self.type_inference.type_hint {
                return Ok(TypeDecl::Dict(key_type.clone(), value_type.clone()));
            }
            return Ok(TypeDecl::Dict(Box::new(TypeDecl::Unknown), Box::new(TypeDecl::Unknown)));
        }
        
        // Save the original type hint to restore later
        let original_hint = self.type_inference.type_hint.clone();
        
        // Extract expected types from type hint if available (clone to avoid borrow issues)
        let (expected_key_type, expected_value_type) = if let Some(TypeDecl::Dict(key_type, value_type)) = &self.type_inference.type_hint {
            (Some(key_type.as_ref().clone()), Some(value_type.as_ref().clone()))
        } else {
            (None, None)
        };
        
        // Check first entry to determine key and value types
        let (first_key, first_value) = &entries[0];
        
        // Set type hints for key and value if we have them
        if let Some(expected_key) = &expected_key_type {
            self.type_inference.type_hint = Some(expected_key.clone());
        }
        let key_type = self.visit_expr(first_key)?;
        
        if let Some(expected_value) = &expected_value_type {
            self.type_inference.type_hint = Some(expected_value.clone());
        }
        let value_type = self.visit_expr(first_value)?;
        
        // Debug: print inferred types for troubleshooting
        // eprintln!("DEBUG dict_literal: key_type={:?}, value_type={:?}, expected_key={:?}, expected_value={:?}", 
        //           key_type, value_type, expected_key_type, expected_value_type);
        
        // Restore original hint
        self.type_inference.type_hint = original_hint.clone();
        
        // If we have type hints and the inferred types are Unknown, use the hint types
        let final_key_type = if key_type == TypeDecl::Unknown && expected_key_type.is_some() {
            expected_key_type.clone().unwrap()
        } else {
            // Convert Number to concrete type
            if key_type == TypeDecl::Number {
                TypeDecl::UInt64  // Default numeric type for keys
            } else {
                key_type
            }
        };
        
        let final_value_type = if value_type == TypeDecl::Unknown && expected_value_type.is_some() {
            expected_value_type.clone().unwrap()
        } else {
            // Convert Number to concrete type  
            if value_type == TypeDecl::Number {
                TypeDecl::UInt64  // Default numeric type for values
            } else {
                value_type
            }
        };
        
        // Verify all entries have consistent types - static typing requirement
        for (entry_index, (key_ref, value_ref)) in entries.iter().skip(1).enumerate() {
            // Set type hints for consistency checking
            if let Some(expected_key) = &expected_key_type {
                self.type_inference.type_hint = Some(expected_key.clone());
            }
            let k_type = self.visit_expr(key_ref)?;
            
            if let Some(expected_value) = &expected_value_type {
                self.type_inference.type_hint = Some(expected_value.clone());
            }
            let v_type = self.visit_expr(value_ref)?;
            
            // Restore original hint
            self.type_inference.type_hint = original_hint.clone();
            
            // Use final types for consistency checking
            let check_key_type = if k_type == TypeDecl::Unknown && expected_key_type.is_some() {
                expected_key_type.clone().unwrap()
            } else {
                // Convert Number to concrete type
                if k_type == TypeDecl::Number {
                    TypeDecl::UInt64
                } else {
                    k_type
                }
            };
            
            let check_value_type = if v_type == TypeDecl::Unknown && expected_value_type.is_some() {
                expected_value_type.clone().unwrap()
            } else {
                // Convert Number to concrete type
                if v_type == TypeDecl::Number {
                    TypeDecl::UInt64
                } else {
                    v_type
                }
            };
            
            if check_key_type != final_key_type {
                return Err(TypeCheckError::generic_error(&format!(
                    "Dict key type mismatch at entry {}: expected {:?}, found {:?}. All keys must have the same type.",
                    entry_index + 1, final_key_type, check_key_type
                )));
            }
            if check_value_type != final_value_type {
                return Err(TypeCheckError::generic_error(&format!(
                    "Dict value type mismatch at entry {}: expected {:?}, found {:?}. All values must have the same type.",
                    entry_index + 1, final_value_type, check_value_type
                )));
            }
        }
        
        Ok(TypeDecl::Dict(Box::new(final_key_type), Box::new(final_value_type)))
    }
    
    fn visit_tuple_literal(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        if elements.is_empty() {
            // Empty tuple is allowed and has type Tuple(vec![])
            return Ok(TypeDecl::Tuple(vec![]));
        }
        
        // Save the original type hint to restore later
        let original_hint = self.type_inference.type_hint.clone();
        
        // Extract expected types from type hint if available
        let expected_types = if let Some(TypeDecl::Tuple(types)) = &self.type_inference.type_hint {
            Some(types.clone())
        } else {
            None
        };
        
        // Check each element and collect their types
        let mut element_types = Vec::new();
        for (index, elem_ref) in elements.iter().enumerate() {
            // Set type hint for this element if available
            if let Some(ref expected) = expected_types {
                if index < expected.len() {
                    self.type_inference.type_hint = Some(expected[index].clone());
                }
            }
            
            let elem_type = self.visit_expr(elem_ref)?;
            
            // Convert Number to concrete type (default to UInt64)
            let final_elem_type = if elem_type == TypeDecl::Number {
                // If we have a hint, use it; otherwise default to UInt64
                if let Some(ref expected) = expected_types {
                    if index < expected.len() && expected[index] != TypeDecl::Unknown {
                        expected[index].clone()
                    } else {
                        TypeDecl::UInt64
                    }
                } else {
                    TypeDecl::UInt64
                }
            } else {
                elem_type
            };
            
            element_types.push(final_elem_type);
        }
        
        // Restore original hint
        self.type_inference.type_hint = original_hint;
        
        Ok(TypeDecl::Tuple(element_types))
    }
    
    fn visit_tuple_access(&mut self, tuple: &ExprRef, index: usize) -> Result<TypeDecl, TypeCheckError> {
        let tuple_type = self.visit_expr(tuple)?;
        
        match tuple_type {
            TypeDecl::Tuple(ref types) => {
                if index >= types.len() {
                    return Err(TypeCheckError::generic_error(&format!(
                        "Tuple index {} out of bounds for tuple with {} elements",
                        index, types.len()
                    )));
                }
                Ok(types[index].clone())
            }
            _ => {
                Err(TypeCheckError::generic_error(&format!(
                    "Cannot access index {} on non-tuple type {:?}",
                    index, tuple_type
                )))
            }
        }
    }
    
    // =========================================================================
    // Statement Type Checking
    // =========================================================================

    fn visit_expression_stmt(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let expr_obj = self.core.expr_pool.get(&expr).ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference in statement"))?;
        expr_obj.clone().accept(self)
    }

    fn visit_var(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        let type_decl = type_decl.clone();
        let expr = expr.clone();
        self.process_val_type(name, &type_decl, &expr)?;
        Ok(TypeDecl::Unit)
    }

    fn visit_val(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let expr_ref = expr.clone();
        let type_decl = type_decl.clone();
        
        // Set type hint and evaluate expression
        let old_hint = self.setup_type_hint_for_val(&type_decl);
        let expr_ty = self.visit_expr(&expr_ref)?;
        
        // Manage variable-expression mapping
        self.update_variable_expr_mapping_internal(name, &expr_ref, &expr_ty);
        
        // Apply type transformations
        self.apply_type_transformations_for_expr(&type_decl, &expr_ty, &expr_ref)?;
        
        // Determine final type and store variable
        let final_type = self.determine_final_type_for_expr(&type_decl, &expr_ty);
        
        self.context.set_var(name, final_type);
        
        // Restore previous type hint
        self.type_inference.type_hint = old_hint;
        
        Ok(TypeDecl::Unit)
    }

    fn visit_return(&mut self, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        if expr.is_none() {
            Ok(TypeDecl::Unit)
        } else {
            let e = expr.as_ref().ok_or_else(|| TypeCheckError::generic_error("Expected expression in return"))?;
            let expr_obj = self.core.expr_pool.get(&e).ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference in return"))?;
            let return_type = expr_obj.clone().accept(self)?;
            Ok(return_type)
        }
    }
    
    // =========================================================================
    // Control Flow Type Checking
    // =========================================================================

    fn visit_for(&mut self, init: DefaultSymbol, _cond: &ExprRef, range: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.push_context();
        let range_obj = self.core.expr_pool.get(&range).ok_or_else(|| TypeCheckError::generic_error("Invalid range expression reference"))?;
        let range_ty = range_obj.clone().accept(self)?;
        let ty = Some(range_ty);
        self.process_val_type(init, &ty, &Some(*range))?;
        let body_obj = self.core.expr_pool.get(&body).ok_or_else(|| TypeCheckError::generic_error("Invalid body expression reference"))?;
        let res = body_obj.clone().accept(self);
        self.pop_context();
        res
    }

    fn visit_while(&mut self, cond: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        // Evaluate condition type first
        let cond_obj = self.core.expr_pool.get(&cond).ok_or_else(|| TypeCheckError::generic_error("Invalid condition expression reference in while"))?;
        let cond_type = cond_obj.clone().accept(self)?;
        
        // Verify condition is boolean
        if cond_type != TypeDecl::Bool {
            return Err(TypeCheckError::type_mismatch(TypeDecl::Bool, cond_type));
        }
        
        // Create new scope for while body
        self.push_context();
        let body_obj = self.core.expr_pool.get(&body).ok_or_else(|| TypeCheckError::generic_error("Invalid body expression reference in while"))?;
        let res = body_obj.clone().accept(self);
        self.pop_context();
        res
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
        
        // Push generic parameters into scope for field type checking
        if !generic_params.is_empty() {
            let generic_substitutions: std::collections::HashMap<DefaultSymbol, TypeDecl> = 
                generic_params.iter().map(|param| (*param, TypeDecl::Generic(*param))).collect();
            self.type_inference.push_generic_scope(generic_substitutions);
        }
        
        // 1. Check for duplicate field names
        let mut field_names = std::collections::HashSet::new();
        for field in fields {
            if !field_names.insert(field.name.clone()) {
                if !generic_params.is_empty() {
                    self.type_inference.pop_generic_scope();
                }
                return Err(TypeCheckError::generic_error(&format!(
                    "Duplicate field '{}' in struct '{:?}'", field.name, name
                )));
            }
        }
        
        // 2. Validate field types
        for field in fields {
            match &field.type_decl {
                TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Bool | TypeDecl::String => {
                    // Basic types are valid
                },
                TypeDecl::Generic(_) => {
                    // Generic types are valid if they're in scope
                },
                TypeDecl::Identifier(struct_name) => {
                    // Check if referenced struct is already defined
                    if !self.context.struct_definitions.contains_key(struct_name) {
                        if !generic_params.is_empty() {
                            self.type_inference.pop_generic_scope();
                        }
                        return Err(TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)));
                    }
                },
                TypeDecl::Array(element_types, _) => {
                    // Validate array element types
                    for element_type in element_types {
                        match element_type {
                            TypeDecl::Identifier(struct_name) => {
                                if !self.context.struct_definitions.contains_key(struct_name) {
                                    if !generic_params.is_empty() {
                                        self.type_inference.pop_generic_scope();
                                    }
                                    return Err(TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)));
                                }
                            },
                            TypeDecl::Generic(_) => {
                                // Generic array elements are valid
                            },
                            _ => {}
                        }
                    }
                },
                _ => {
                    if !generic_params.is_empty() {
                        self.type_inference.pop_generic_scope();
                    }
                    return Err(TypeCheckError::unsupported_operation(
                        &format!("field type in struct '{:?}'", name), field.type_decl.clone()
                    ));
                }
            }
        }
        
        // 3. Register struct definition with visibility information
        let struct_symbol = name;
        let struct_def = crate::type_checker::context::StructDefinition {
            fields: fields.clone(),
            visibility: visibility.clone(),
        };
        
        // Store the struct definition for later type checking and access control
        self.context.struct_definitions.insert(struct_symbol, struct_def);
        
        // Register generic parameters if any
        if !generic_params.is_empty() {
            self.context.set_struct_generic_params(name, generic_params.clone());
        }
        
        // Pop generic scope after processing
        if !generic_params.is_empty() {
            self.type_inference.pop_generic_scope();
        }
        
        Ok(TypeDecl::Unit)
    }

    fn visit_impl_block(&mut self, target_type: DefaultSymbol, methods: &Vec<Rc<MethodFunction>>) -> Result<TypeDecl, TypeCheckError> {
        // target_type is already a symbol
        let struct_symbol = target_type;

        // Set current impl target for Self resolution
        let old_impl_target = self.context.current_impl_target;
        self.context.current_impl_target = Some(struct_symbol);

        // Check if this is a generic struct and set up generic scope
        let generic_params = self.context.get_struct_generic_params(struct_symbol).cloned();
        let has_generics = generic_params.is_some() && !generic_params.as_ref().unwrap().is_empty();
        
        if has_generics {
            // Push generic parameters into scope for method type checking
            let generic_substitutions: std::collections::HashMap<DefaultSymbol, TypeDecl> = 
                generic_params.as_ref().unwrap().iter().map(|param| (*param, TypeDecl::Generic(*param))).collect();
            self.type_inference.push_generic_scope(generic_substitutions);
        }

        // Impl block type checking - validate methods
        for method in methods {
            // Check method parameter types
            for (_, param_type) in &method.parameter {
                // Resolve Self type to the actual struct type
                let resolved_type = self.resolve_self_type(param_type);
                
                match &resolved_type {
                    TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Bool | TypeDecl::String | 
                    TypeDecl::Identifier(_) | TypeDecl::Generic(_) | TypeDecl::Struct(_, _) => {
                        // Valid parameter types (including struct types and generic types)
                    },
                    _ => {
                        if has_generics {
                            self.type_inference.pop_generic_scope();
                        }
                        let method_name = self.core.string_interner.resolve(method.name).unwrap_or("<unknown>");
                        return Err(TypeCheckError::unsupported_operation(
                            &format!("parameter type in method '{}' for impl block '{:?}'", method_name, target_type),
                            resolved_type
                        ));
                    }
                }
            }
            
            // Check return type if specified - now with proper generic support
            if let Some(ref ret_type) = method.return_type {
                // Try to resolve return type in generic context
                let resolved_ret_type = self.resolve_self_type(ret_type);
                
                // For generic types, we need to validate they can be resolved
                // but don't enforce strict type checking here since generics will be resolved later
                match &resolved_ret_type {
                    TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Bool | TypeDecl::String | 
                    TypeDecl::Unit | TypeDecl::Identifier(_) | TypeDecl::Generic(_) | TypeDecl::Struct(_, _) => {
                        // Valid return types (including generic types and struct types)
                    },
                    _ => {
                        if has_generics {
                            self.type_inference.pop_generic_scope();
                        }
                        let method_name = self.core.string_interner.resolve(method.name).unwrap_or("<unknown>");
                        return Err(TypeCheckError::unsupported_operation(
                            &format!("return type in method '{}' for impl block", method_name),
                            resolved_ret_type
                        ));
                    }
                }
            }

            // Type check method body
            // Set up parameter context for method
            self.context.push_scope();
            for (param_name, param_type) in &method.parameter {
                let resolved_param_type = self.resolve_self_type(param_type);
                self.context.set_var(*param_name, resolved_param_type);
            }
            
            // Type check method body
            let body_result = self.visit_stmt(&method.code);
            
            // Restore parameter context
            self.context.pop_scope();
            
            // Check if body type matches return type
            if let Some(ref expected_return_type) = method.return_type {
                let resolved_expected_type = self.resolve_self_type(expected_return_type);
                match body_result {
                    Ok(actual_return_type) => {
                        // For generic methods, use more sophisticated type checking
                        if has_generics {
                            // Try to apply generic substitutions for better matching
                            // Skip strict checking for generic return types - they'll be resolved during instantiation
                            match (&resolved_expected_type, &actual_return_type) {
                                (TypeDecl::Generic(_), _) | (_, TypeDecl::Generic(_)) => {
                                    // Allow generic types to match - will be resolved later
                                }
                                _ => {
                                    // Use normal type compatibility checking
                                    if !self.are_types_compatible(&actual_return_type, &resolved_expected_type) {
                                        self.type_inference.pop_generic_scope();
                                        return Err(TypeCheckError::type_mismatch(
                                            resolved_expected_type,
                                            actual_return_type
                                        ));
                                    }
                                }
                            }
                        } else {
                            // Non-generic method - use normal type compatibility checking
                            if !self.are_types_compatible(&actual_return_type, &resolved_expected_type) {
                                return Err(TypeCheckError::type_mismatch(
                                    resolved_expected_type,
                                    actual_return_type
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        if has_generics {
                            self.type_inference.pop_generic_scope();
                        }
                        return Err(e);
                    }
                }
            }

            // Register method in context
            self.context.register_struct_method(struct_symbol, method.name, method.clone());
        }
        
        // Pop generic scope if it was pushed
        if has_generics {
            self.type_inference.pop_generic_scope();
        }
        
        // Restore previous impl target context
        self.context.current_impl_target = old_impl_target;
        
        // Impl block declaration returns Unit
        Ok(TypeDecl::Unit)
    }
    

    fn visit_field_access(&mut self, obj: &ExprRef, field: &DefaultSymbol) -> Result<TypeDecl, TypeCheckError> {
        // Check recursion depth to prevent stack overflow
        if self.type_inference.recursion_depth >= self.type_inference.max_recursion_depth {
            return Err(TypeCheckError::generic_error(
                "Maximum recursion depth reached in field access type inference - possible circular reference"
            ));
        }
        
        // Phase 4: Check if this might be a module qualified name (math.add)
        if let Some(module_function_type) = self.try_resolve_module_qualified_name(obj, field)? {
            return Ok(module_function_type);
        }
        
        self.type_inference.recursion_depth += 1;
        let obj_type_result = self.visit_expr(obj);
        self.type_inference.recursion_depth -= 1;
        
        let obj_type = obj_type_result?;
        
        match obj_type {
            TypeDecl::Identifier(struct_name) => {
                // Look up the struct definition and get the field type
                if let Some(struct_fields) = self.context.get_struct_fields(struct_name) {
                    let field_name = self.core.string_interner.resolve(*field).unwrap_or("<unknown>");
                    for struct_field in struct_fields {
                        if struct_field.name == field_name {
                            return Ok(struct_field.type_decl.clone());
                        }
                    }
                    Err(TypeCheckError::not_found("field", field_name))
                } else {
                    let struct_name_str = self.core.string_interner.resolve(struct_name).unwrap_or("<unknown>");
                    Err(TypeCheckError::not_found("struct", struct_name_str))
                }
            }
            TypeDecl::Struct(struct_symbol, _) => {
                // Handle symbol-based struct type  
                if let Some(struct_fields) = self.context.get_struct_fields(struct_symbol) {
                    let field_name = self.core.string_interner.resolve(*field).unwrap_or("<unknown>");
                    for struct_field in struct_fields {
                        if struct_field.name == field_name {
                            return Ok(struct_field.type_decl.clone());
                        }
                    }
                    Err(TypeCheckError::not_found("field", field_name))
                } else {
                    let struct_name_str = self.core.string_interner.resolve(struct_symbol).unwrap_or("<unknown>");
                    Err(TypeCheckError::not_found("struct", struct_name_str))
                }
            }
            TypeDecl::Self_ => {
                // Resolve Self type in current context
                let resolved_type = self.resolve_self_type(&obj_type);
                match resolved_type {
                    TypeDecl::Self_ => {
                        // Self could not be resolved - not in impl context
                        let field_name = self.core.string_interner.resolve(*field).unwrap_or("<unknown>");
                        Err(TypeCheckError::generic_error(&format!(
                            "Cannot resolve Self type for field access '{}' - not in impl context", field_name
                        )))
                    }
                    TypeDecl::Identifier(struct_symbol) => {
                        if let Some(struct_fields) = self.context.get_struct_fields(struct_symbol) {
                            let field_name = self.core.string_interner.resolve(*field).unwrap_or("<unknown>");
                            for struct_field in struct_fields {
                                if struct_field.name == field_name {
                                    return Ok(struct_field.type_decl.clone());
                                }
                            }
                            Err(TypeCheckError::not_found("field", field_name))
                        } else {
                            let struct_name_str = self.core.string_interner.resolve(struct_symbol).unwrap_or("<unknown>");
                            Err(TypeCheckError::not_found("struct", struct_name_str))
                        }
                    }
                    TypeDecl::Struct(struct_symbol, _) => {
                        if let Some(struct_fields) = self.context.get_struct_fields(struct_symbol) {
                            let field_name = self.core.string_interner.resolve(*field).unwrap_or("<unknown>");
                            for struct_field in struct_fields {
                                if struct_field.name == field_name {
                                    return Ok(struct_field.type_decl.clone());
                                }
                            }
                            Err(TypeCheckError::not_found("field", field_name))
                        } else {
                            let struct_name_str = self.core.string_interner.resolve(struct_symbol).unwrap_or("<unknown>");
                            Err(TypeCheckError::not_found("struct", struct_name_str))
                        }
                    }
                    _ => {
                        let field_name = self.core.string_interner.resolve(*field).unwrap_or("<unknown>");
                        Err(TypeCheckError::unsupported_operation(
                            &format!("field access '{}' on resolved Self type", field_name), resolved_type
                        ))
                    }
                }
            }
            _ => {
                let field_name = self.core.string_interner.resolve(*field).unwrap_or("<unknown>");
                Err(TypeCheckError::unsupported_operation(
                    &format!("field access '{}'", field_name), obj_type
                ))
            }
        }
    }

    fn visit_method_call(&mut self, obj: &ExprRef, method: &DefaultSymbol, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Delegate to the implementation in expression.rs
        self.visit_method_call_impl(obj, method, args)
    }

    fn visit_struct_literal(&mut self, struct_name: &DefaultSymbol, fields: &Vec<(DefaultSymbol, ExprRef)>) -> Result<TypeDecl, TypeCheckError> {
        // Check recursion depth to prevent stack overflow
        if self.type_inference.recursion_depth >= self.type_inference.max_recursion_depth {
            return Err(TypeCheckError::generic_error(
                "Maximum recursion depth reached in struct type inference - possible circular reference"
            ));
        }
        
        
        self.type_inference.recursion_depth += 1;
        
        // Execute the main logic and capture result
        let result = self.visit_struct_literal_impl(struct_name, fields);
        
        // Always decrement recursion depth before returning
        self.type_inference.recursion_depth -= 1;
        
        result
    }

    fn visit_qualified_identifier(&mut self, path: &Vec<DefaultSymbol>) -> Result<TypeDecl, TypeCheckError> {
        // For now, treat qualified identifiers like regular identifiers using the last component
        // Later this can be enhanced for proper module resolution
        if let Some(last_symbol) = path.last() {
            self.visit_identifier(*last_symbol)
        } else {
            Err(TypeCheckError::generic_error("empty qualified identifier path"))
        }
    }

    fn visit_builtin_method_call(&mut self, receiver: &ExprRef, method: &BuiltinMethod, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Simplified implementation for now
        let _ = receiver; // unused parameter
        let _ = args; // unused parameter
        match method {
            BuiltinMethod::StrLen => Ok(TypeDecl::UInt64),
            BuiltinMethod::IsNull => Ok(TypeDecl::Bool),
            _ => Ok(TypeDecl::Unknown),
        }
    }

    fn visit_builtin_call(&mut self, func: &BuiltinFunction, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Find matching function signature from pre-built table
        let _ = args; // unused parameter
        let signature = self.builtin_function_signatures.iter().find(|sig| sig.func == *func).cloned();
        
        if let Some(sig) = signature {
            Ok(sig.return_type.clone())
        } else {
            Ok(TypeDecl::Unknown)
        }
    }
    
}

impl<'a> TypeCheckerVisitor<'a> {
    // =========================================================================
    // Constant Expression Evaluation
    // =========================================================================
    

    /// Resolve Self type to the actual struct type in impl block context
    pub fn resolve_self_type(&self, type_decl: &TypeDecl) -> TypeDecl {
        match type_decl {
            TypeDecl::Self_ => {
                if let Some(target_symbol) = self.context.current_impl_target {
                    TypeDecl::Struct(target_symbol, vec![])
                } else {
                    // Self used outside impl context - should be an error
                    type_decl.clone()
                }
            }
            _ => type_decl.clone(),
        }
    }

    /// Check method arguments against parameter types, handling Self type specially
    fn check_method_arguments(&self, obj_type: &TypeDecl, method: &Rc<MethodFunction>, 
                             _args: &Vec<ExprRef>, arg_types: &Vec<TypeDecl>, method_name: &str) -> Result<(), TypeCheckError> {
        // Check argument count
        if arg_types.len() + 1 != method.parameter.len() {
            return Err(TypeCheckError::method_error(
                method_name, 
                obj_type.clone(),
                &format!("expected {} arguments, found {}", method.parameter.len() - 1, arg_types.len())
            ));
        }

        // Check the first parameter (self parameter)
        if !method.parameter.is_empty() {
            let (_, first_param_type) = &method.parameter[0];
            
            // For Self type, we need to match it with the actual struct type
            let expected_self_type = match first_param_type {
                TypeDecl::Self_ => obj_type.clone(), // Self should match the object type
                _ => first_param_type.clone()
            };
            
            // Check if obj_type is compatible with the first parameter type
            if !self.are_types_compatible(&expected_self_type, obj_type) {
                return Err(TypeCheckError::method_error(
                    method_name,
                    obj_type.clone(),
                    &format!("self parameter type mismatch: expected {:?}, found {:?}", expected_self_type, obj_type)
                ));
            }
        }

        // Check remaining arguments (starting from index 1 since index 0 is self)
        for (i, arg_type) in arg_types.iter().enumerate() {
            if i + 1 < method.parameter.len() {
                let (_, param_type) = &method.parameter[i + 1];
                
                // For Self type in method parameters, resolve to object type
                let resolved_param_type = match param_type {
                    TypeDecl::Self_ => obj_type.clone(),
                    _ => param_type.clone()
                };
                
                if !self.are_types_compatible(&resolved_param_type, arg_type) {
                    return Err(TypeCheckError::method_error(
                        method_name,
                        obj_type.clone(),
                        &format!("argument {} type mismatch: expected {:?}, found {:?}", i + 1, resolved_param_type, arg_type)
                    ));
                }
            }
        }

        Ok(())
    }

    fn visit_array_literal_impl(&mut self, elements: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Save the original type hint to restore later
        let original_hint = self.type_inference.type_hint.clone();
        
        // If we have a type hint for the array element type, use it for element type inference
        let element_type_hint = if let Some(TypeDecl::Array(element_types, _)) = &self.type_inference.type_hint {
            if !element_types.is_empty() {
                Some(element_types[0].clone())
            } else {
                None
            }
        } else {
            None
        };

        // Type check all elements with proper type hint for each element
        let mut element_types = Vec::new();
        for element in elements {
            // Set the element type hint for each element individually
            if let Some(ref hint) = element_type_hint {
                self.type_inference.type_hint = Some(hint.clone());
            }
            
            let element_type = self.visit_expr(element)?;
            element_types.push(element_type);
            
            // Restore original hint after processing each element
            self.type_inference.type_hint = original_hint.clone();
        }

        // If we have array type hint, handle type inference for all elements
        if let Some(TypeDecl::Array(ref expected_element_types, _)) = original_hint {
            if !expected_element_types.is_empty() {
                let expected_element_type = &expected_element_types[0];
                
                // Handle type inference for each element
                for (i, element) in elements.iter().enumerate() {
                    match &element_types[i] {
                        TypeDecl::Number => {
                            // Transform Number literals to the expected type
                            self.transform_numeric_expr(element, expected_element_type)?;
                            element_types[i] = expected_element_type.clone();
                        },
                        TypeDecl::Bool => {
                            // Bool literals - check type compatibility
                            if expected_element_type != &TypeDecl::Bool {
                                return Err(TypeCheckError::array_error(&format!(
                                    "Array element {} has type Bool but expected {:?}",
                                    i, expected_element_type
                                )));
                            }
                            // Type is correct, no transformation needed
                        },
                        TypeDecl::Identifier(actual_struct) => {
                            // Struct literals - check type compatibility
                            if let TypeDecl::Identifier(expected_struct) = expected_element_type {
                                if actual_struct != expected_struct {
                                    return Err(TypeCheckError::array_error(&format!(
                                        "Array element {} has struct type {:?} but expected {:?}",
                                        i, actual_struct, expected_struct
                                    )));
                                }
                                // Same struct type, no transformation needed
                            } else {
                                return Err(TypeCheckError::array_error(&format!(
                                    "Array element {} has struct type {:?} but expected {:?}",
                                    i, actual_struct, expected_element_type
                                )));
                            }
                        },
                        actual_type if actual_type == expected_element_type => {
                            // Element already has the expected type, but may need AST transformation
                            // Check if this is a number literal that needs transformation
                            if let Some(expr) = self.core.expr_pool.get(&element) {
                                if matches!(expr, Expr::Number(_)) {
                                    self.transform_numeric_expr(element, expected_element_type)?;
                                }
                            }
                        },
                        TypeDecl::Unknown => {
                            // For variables with unknown type, try to infer from context
                            element_types[i] = expected_element_type.clone();
                        },
                        actual_type if actual_type != expected_element_type => {
                            // Check if type conversion is possible
                            match (actual_type, expected_element_type) {
                                (TypeDecl::Int64, TypeDecl::UInt64) | 
                                (TypeDecl::UInt64, TypeDecl::Int64) => {
                                    return Err(TypeCheckError::array_error(&format!(
                                        "Cannot mix signed and unsigned integers in array. Element {} has type {:?} but expected {:?}",
                                        i, actual_type, expected_element_type
                                    )));
                                },
                                (TypeDecl::Bool, _other_type) | (_other_type, TypeDecl::Bool) => {
                                    return Err(TypeCheckError::array_error(&format!(
                                        "Cannot mix Bool with other types in array. Element {} has type {:?} but expected {:?}",
                                        i, actual_type, expected_element_type
                                    )));
                                },
                                (TypeDecl::Identifier(struct1), TypeDecl::Identifier(struct2)) => {
                                    if struct1 != struct2 {
                                        return Err(TypeCheckError::array_error(&format!(
                                            "Array element {} has struct type {:?} but expected {:?}",
                                            i, struct1, struct2
                                        )));
                                    }
                                },
                                (TypeDecl::Identifier(struct_name), other_type) | (other_type, TypeDecl::Identifier(struct_name)) => {
                                    return Err(TypeCheckError::array_error(&format!(
                                        "Cannot mix struct type {:?} with {:?} in array. Element {} has incompatible type",
                                        struct_name, other_type, i
                                    )));
                                },
                                _ => {
                                    // Accept the actual type if it matches expectations
                                    if actual_type == expected_element_type {
                                        // Already matches, no change needed
                                    } else {
                                        return Err(TypeCheckError::array_error(&format!(
                                            "Array element {} has type {:?} but expected {:?}",
                                            i, actual_type, expected_element_type
                                        )));
                                    }
                                }
                            }
                        },
                        _ => {
                            // Type already matches expected type
                        }
                    }
                }
            }
        }

        // Handle Number types when no type hint was provided
        if original_hint.is_none() {
            for (i, element) in elements.iter().enumerate() {
                if element_types[i] == TypeDecl::Number {
                    // Transform Number to default UInt64 when no hint is available
                    self.transform_numeric_expr(element, &TypeDecl::UInt64)?;
                    element_types[i] = TypeDecl::UInt64;
                }
            }
        }

        // Restore the original type hint
        self.type_inference.type_hint = original_hint;

        let first_type = &element_types[0];
        for (i, element_type) in element_types.iter().enumerate() {
            if element_type != first_type {
                return Err(TypeCheckError::array_error(&format!(
                    "Array elements must have the same type, but element {} has type {:?} while first element has type {:?}",
                    i, element_type, first_type
                )));
            }
        }

        Ok(TypeDecl::Array(element_types, elements.len()))
    }

    fn visit_struct_literal_impl(&mut self, struct_name: &DefaultSymbol, fields: &Vec<(DefaultSymbol, ExprRef)>) -> Result<TypeDecl, TypeCheckError> {
        // 1. Check if struct definition exists and clone it
        let struct_definition = self.context.get_struct_definition(*struct_name)
            .ok_or_else(|| TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)))?
            .clone();
        
        // 2. Check if this is a generic struct and handle type inference
        let generic_params = self.context.get_struct_generic_params(*struct_name).cloned();
        let is_generic = generic_params.is_some() && !generic_params.as_ref().unwrap().is_empty();
        
        
        if is_generic {
            return self.visit_generic_struct_literal(struct_name, fields, &struct_definition, &generic_params.unwrap());
        }
        
        // 3. Handle non-generic struct (existing logic)
        // Validate provided fields against struct definition
        self.context.validate_struct_fields(*struct_name, fields, &self.core)?;
        
        // Type check each field and verify type compatibility
        let mut field_types = std::collections::HashMap::new();
        for (field_name, field_expr) in fields {
            // Find expected field type from struct definition
            let field_name_str = self.core.string_interner.resolve(*field_name).unwrap_or("<unknown>");
            let expected_field_type = struct_definition.fields.iter()
                .find(|def| def.name == field_name_str)
                .map(|def| &def.type_decl);
            
            // Set type hint for field expression
            let original_hint = self.type_inference.type_hint.clone();
            if let Some(expected_type) = expected_field_type {
                self.type_inference.type_hint = Some(expected_type.clone());
            }
            
            // Type check the field expression
            let field_type = self.visit_expr(field_expr)?;
            self.type_inference.type_hint = original_hint;
            
            // Verify type compatibility
            if let Some(expected_type) = expected_field_type {
                if &field_type != expected_type {
                    // Check for Number type auto-conversion
                    if field_type == TypeDecl::Number && (expected_type == &TypeDecl::Int64 || expected_type == &TypeDecl::UInt64) {
                        self.transform_numeric_expr(field_expr, expected_type)?;
                    // Use comprehensive type compatibility checking
                    } else if !self.are_types_compatible(expected_type, &field_type) {
                        return Err(TypeCheckError::type_mismatch(expected_type.clone(), field_type));
                    }
                }
            }
            
            field_types.insert(*field_name, field_type);
        }
        
        Ok(TypeDecl::Identifier(*struct_name))
    }
    
    /// Handle generic struct literal type inference
    fn visit_generic_struct_literal(&mut self, struct_name: &DefaultSymbol, fields: &Vec<(DefaultSymbol, ExprRef)>, 
                                   struct_definition: &crate::type_checker::context::StructDefinition, 
                                   generic_params: &Vec<DefaultSymbol>) -> Result<TypeDecl, TypeCheckError> {
        // Clear previous constraints for this inference
        self.type_inference.clear_constraints();
        
        // Validate provided fields against struct definition
        self.context.validate_struct_fields(*struct_name, fields, &self.core)?;
        
        // Push generic parameters onto the scope for proper resolution
        let mut generic_scope = std::collections::HashMap::new();
        for param in generic_params {
            generic_scope.insert(*param, TypeDecl::Generic(*param));
        }
        self.type_inference.push_generic_scope(generic_scope);
        
        // Collect field types and create constraints for type parameter inference
        let mut field_types = std::collections::HashMap::new();
        
        for (field_name, field_expr) in fields {
            // Find expected field type from struct definition
            let field_name_str = self.core.string_interner.resolve(*field_name).unwrap_or("<unknown>");
            let expected_field_type = struct_definition.fields.iter()
                .find(|def| def.name == field_name_str)
                .map(|def| &def.type_decl);
            
            if let Some(expected_type) = expected_field_type {
                // Type check the field expression without hint first
                let field_type = self.visit_expr(field_expr)?;
                
                
                // Add constraint for generic type unification
                self.type_inference.add_constraint(
                    expected_type.clone(),
                    field_type.clone(),
                    crate::type_checker::inference::ConstraintContext::FieldAccess {
                        struct_name: *struct_name,
                        field_name: *field_name,
                    }
                );
                
                field_types.insert(*field_name, field_type);
            }
        }
        
        // Solve constraints to get type substitutions
        let substitutions = match self.type_inference.solve_constraints() {
            Ok(solution) => {
                solution
            },
            Err(e) => {
                self.type_inference.pop_generic_scope();
                let struct_name_str = self.core.string_interner.resolve(*struct_name).unwrap_or("<unknown>");
                return Err(TypeCheckError::generic_error(&format!(
                    "Type inference failed for generic struct '{}': {}",
                    struct_name_str, e
                )));
            }
        };
        
        // Now verify field types with the resolved substitutions
        for (field_name, field_expr) in fields {
            let field_name_str = self.core.string_interner.resolve(*field_name).unwrap_or("<unknown>");
            let expected_field_type = struct_definition.fields.iter()
                .find(|def| def.name == field_name_str)
                .map(|def| &def.type_decl);
            
            if let Some(expected_type) = expected_field_type {
                // Apply substitutions to the expected type
                let substituted_expected = expected_type.substitute_generics(&substitutions);
                let actual_type = field_types.get(field_name).unwrap();
                
                // Check type compatibility with substitution
                if !self.are_types_compatible(&substituted_expected, actual_type) {
                    // Check for Number type auto-conversion
                    if *actual_type == TypeDecl::Number && 
                       (substituted_expected == TypeDecl::Int64 || substituted_expected == TypeDecl::UInt64) {
                        self.transform_numeric_expr(field_expr, &substituted_expected)?;
                    } else {
                        self.type_inference.pop_generic_scope();
                        return Err(TypeCheckError::type_mismatch(substituted_expected, actual_type.clone()));
                    }
                }
            }
        }
        
        // Ensure all generic parameters have been inferred
        for generic_param in generic_params {
            if !substitutions.contains_key(generic_param) {
                self.type_inference.pop_generic_scope();
                let param_name = self.core.string_interner.resolve(*generic_param).unwrap_or("<unknown>");
                return Err(TypeCheckError::generic_error(&format!(
                    "Cannot infer generic type parameter '{}' for struct '{}'",
                    param_name,
                    self.core.string_interner.resolve(*struct_name).unwrap_or("<unknown>")
                )));
            }
        }
        
        // Record the type substitutions for later use in method calls
        // This allows method calls on this struct instance to use the inferred types
        // Implementation delegated to type inference engine
        
        // Pop the generic scope
        self.type_inference.pop_generic_scope();
        
        // Generate instantiated struct name and record instantiation
        let _instantiated_name_str = self.generate_instantiated_struct_name(*struct_name, &substitutions);
        
        // Create and record the instantiation for potential code generation (postponed)
        // Note: We store the string for now and will convert to Symbol later to avoid borrowing issues
        // This is a temporary solution - a better approach would be to refactor the borrowing
        
        // For now, we'll record the need for instantiation without the symbol conversion
        // TODO: Implement proper instantiation recording with symbol management
        
        // Return the concrete struct type
        Ok(TypeDecl::Struct(*struct_name, vec![]))
    }
    
    /// Generate a unique name for instantiated generic struct
    fn generate_instantiated_struct_name(&self, struct_name: DefaultSymbol, substitutions: &std::collections::HashMap<DefaultSymbol, TypeDecl>) -> String {
        let base_name = self.core.string_interner.resolve(struct_name).unwrap_or("<unknown>");
        
        // Sort substitutions for consistent naming
        let mut sorted_subs: Vec<_> = substitutions.iter().collect();
        sorted_subs.sort_by_key(|(k, _)| *k);
        
        let mut name_parts = vec![base_name.to_string()];
        for (param, concrete_type) in sorted_subs {
            let param_name = self.core.string_interner.resolve(*param).unwrap_or("<unknown>");
            let type_name = match concrete_type {
                TypeDecl::UInt64 => "u64",
                TypeDecl::Int64 => "i64",
                TypeDecl::Bool => "bool",
                TypeDecl::String => "str",
                _ => "unknown"
            };
            name_parts.push(format!("{}_{}", param_name, type_name));
        }
        
        name_parts.join("_")
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
        self.optimization.type_cache.insert(expr_ref.clone(), type_decl);
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

impl<'a> TypeCheckerVisitor<'a> {
    /// Add error to collection without returning immediately
    pub fn collect_error(&mut self, error: TypeCheckError) {
        self.errors.push(error);
    }

    /// Type check program with multiple error collection
    pub fn check_program_multiple_errors(&mut self, program: &Program) -> error::MultipleTypeCheckResult<()> {
        self.errors.clear();
        
        // Collect errors during type checking instead of returning immediately
        for func in &program.function {
            if let Err(e) = self.type_check(func.clone()) {
                self.errors.push(e);
            }
        }
        
        for index in 0..program.statement.len() {
            let stmt_ref = StmtRef(index as u32);
            if let Err(e) = self.visit_stmt(&stmt_ref) {
                self.errors.push(e);
            }
        }
        
        if self.errors.is_empty() {
            error::MultipleTypeCheckResult::success(())
        } else {
            error::MultipleTypeCheckResult::with_errors((), self.errors.clone())
        }
    }
    
    /// Clear collected errors
    pub fn clear_errors(&mut self) {
        self.errors.clear();
    }
    
    // Module management methods (Phase 1: Basic namespace management)
    
    /// Set the current package context
    pub fn set_current_package(&mut self, package_path: Vec<DefaultSymbol>) {
        self.current_package = Some(package_path);
    }
    
    /// Get the current package path
    pub fn get_current_package(&self) -> Option<&Vec<DefaultSymbol>> {
        self.current_package.as_ref()
    }
    
    /// Register an imported module (simple alias -> full_path mapping)
    pub fn register_import(&mut self, module_path: Vec<DefaultSymbol>) {
        // Use the last component as alias (e.g., math.utils -> utils)
        let alias = if let Some(&last) = module_path.last() {
            vec![last]
        } else {
            module_path.clone()
        };
        self.imported_modules.insert(alias, module_path);
    }
    
    /// Check if a module path is valid for import (not self-referencing)
    pub fn is_valid_import(&self, module_path: &[DefaultSymbol]) -> bool {
        if let Some(current_pkg) = &self.current_package {
            // Prevent self-import
            current_pkg != module_path
        } else {
            true
        }
    }
    
    /// Try to resolve a module qualified name (e.g., math.add)
    /// Returns Some(TypeDecl) if it's a valid module qualified name, None if it's a regular field access
    pub fn try_resolve_module_qualified_name(&mut self, obj: &ExprRef, field: &DefaultSymbol) -> Result<Option<TypeDecl>, TypeCheckError> {
        // Check if obj is an identifier that matches an imported module
        if let Some(obj_expr) = self.core.expr_pool.get(&obj) {
            if let Expr::Identifier(module_symbol) = obj_expr {
                let module_alias = vec![module_symbol];
                
                // Check if this identifier matches an imported module
                if let Some(full_module_path) = self.imported_modules.get(&module_alias) {
                    // Clone the module path to avoid borrowing issues
                    let module_path_clone = full_module_path.clone();
                    // This is a module qualified name: module.identifier
                    return self.resolve_module_member_type(&module_path_clone, field);
                }
            }
        }
        
        Ok(None)
    }
    
    /// Resolve the type of a member in a specific module
    fn resolve_module_member_type(&mut self, module_path: &[DefaultSymbol], member_name: &DefaultSymbol) -> Result<Option<TypeDecl>, TypeCheckError> {
        // Phase 4: Basic implementation - assume functions return their declared type
        // For now, we'll look for functions in the current scope that might belong to the module
        
        // Convert member name to string for lookup
        let member_str = self.core.string_interner.resolve(*member_name)
            .ok_or_else(|| TypeCheckError::generic_error("Member name not found in string interner"))?;
        
        // Simple heuristic: if it's a known function pattern, return a generic function type
        // In a full implementation, this would query the module resolver for actual module contents
        if self.is_likely_function_name(member_str) {
            // Return a placeholder function type - in practice this would be looked up from module metadata
            // For now, we'll use TypeDecl::Unknown to represent a module function
            Ok(Some(TypeDecl::Unknown))
        } else {
            // Could be a variable, constant, or type - for now return error
            Err(TypeCheckError::generic_error(&format!(
                "Member '{}' not found in module '{:?}'", 
                member_str, 
                self.resolve_module_path_names(module_path)
            )))
        }
    }
    
    /// Helper to check if a name looks like a function (simple heuristic)
    fn is_likely_function_name(&self, name: &str) -> bool {
        // Common function name patterns
        name.chars().all(|c| c.is_alphanumeric() || c == '_') && 
        !name.chars().next().unwrap_or('0').is_uppercase() // Not a type name
    }
    
    /// Helper to convert module path symbols to readable names
    fn resolve_module_path_names(&self, module_path: &[DefaultSymbol]) -> Vec<String> {
        module_path.iter()
            .map(|&symbol| self.core.string_interner.resolve(symbol).unwrap_or("<unknown>").to_string())
            .collect()
    }
    
    // =========================================================================
    // Phase 3: Access Control and Visibility Enforcement
    // =========================================================================
    
    /// Check if a function can be accessed based on visibility and module context
    fn check_function_access(&self, function: &Function) -> Result<(), TypeCheckError> {
        // If function is public, it's accessible from anywhere
        if function.visibility == Visibility::Public {
            return Ok(());
        }
        
        // If function is private, check if we're in the same module
        if function.visibility == Visibility::Private {
            // For now, assume same-module access is allowed
            // TODO: Implement proper module boundary checking
            if self.is_same_module_access() {
                return Ok(());
            } else {
                let fn_name = self.core.string_interner
                    .resolve(function.name)
                    .unwrap_or("<unknown>");
                return Err(TypeCheckError::access_denied(
                    &format!("Private function '{}' cannot be accessed from different module", fn_name)
                ));
            }
        }
        
        Ok(())
    }

    /// Check if current access is within the same module
    fn is_same_module_access(&self) -> bool {
        // For Phase 3 initial implementation, assume same module access
        // TODO: Implement proper module context tracking
        // This should compare current_package with the function/struct's defining module
        true
    }
}

/// Check if a string is a reserved keyword
fn is_reserved_keyword(name: &str) -> bool {
    matches!(name, 
        "fn" | "val" | "var" | "if" | "else" | "for" | "in" | "to" | 
        "while" | "break" | "continue" | "return" | "struct" | "impl" | 
        "package" | "import" | "pub" | "true" | "false" | "u64" | "i64" | 
        "bool" | "str" | "self" | "Self"
    )
}