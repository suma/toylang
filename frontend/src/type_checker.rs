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
mod impl_block;
mod collections;
mod builtin;
mod utility;
mod method;
mod error_handling;
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
        // Clone functions to avoid borrowing conflicts
        let functions = program.function.clone();
        
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
        
        // Register all functions from the program into the type checker context
        for func in &functions {
            visitor.add_function(func.clone());
        }
        
        // Register all structs from the program's statements into the type checker context
        let stmt_len = visitor.core.stmt_pool.len();
        for i in 0..stmt_len {
            let stmt_ref = StmtRef(i as u32);
            if let Some(stmt) = visitor.core.stmt_pool.get(&stmt_ref) {
                if let Stmt::StructDecl { name, generic_params: _, fields, visibility } = stmt {
                    visitor.context.register_struct(
                        name,
                        fields.clone(),
                        visibility
                    );
                }
            }
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
        vec![] // Empty for now - builtin functions are handled separately
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
        let original_hint = self.type_inference.type_hint.clone();
        if let Some(numeric_type) = self.scan_numeric_type_hint(&statements) {
            self.type_inference.type_hint = Some(numeric_type);
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
            Expr::Cast(expr, target_type) => visitor.visit_cast(expr, target_type),
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
        // Delegate to expression module implementation
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
        // Delegate to statement module implementation
        self.visit_for_impl(init, _cond, range, body)
    }

    fn visit_while(&mut self, cond: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        // Delegate to statement module implementation
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
        // Delegate to struct_literal module implementation
        self.visit_struct_decl_impl(name, generic_params, fields, visibility)
    }

    fn visit_impl_block(&mut self, target_type: DefaultSymbol, methods: &Vec<Rc<MethodFunction>>) -> Result<TypeDecl, TypeCheckError> {
        // Delegate to impl_block module implementation
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
        // Later this can be enhanced for proper module resolution
        if let Some(last_symbol) = path.last() {
            self.visit_identifier(*last_symbol)
        } else {
            Err(TypeCheckError::generic_error("empty qualified identifier path"))
        }
    }


    fn visit_builtin_method_call(&mut self, receiver: &ExprRef, method: &BuiltinMethod, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Use method.rs module for builtin method processing
        <Self as method::MethodProcessing>::visit_builtin_method_call(self, receiver, method, args)
    }

    fn visit_builtin_call(&mut self, func: &BuiltinFunction, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
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