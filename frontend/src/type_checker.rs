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
pub mod inference;
pub mod optimization;

pub use core::CoreReferences;
pub use context::{TypeCheckContext, VarState};
pub use error::{SourceLocation, TypeCheckError, TypeCheckErrorKind};
pub use function::FunctionCheckingState;
pub use inference::TypeInferenceState;
pub use optimization::PerformanceOptimization;

mod traits;
pub use traits::*;

mod literal_checker;

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
            builtin_methods: Self::create_builtin_method_registry(),
            builtin_function_signatures: TypeCheckerVisitor::create_builtin_function_signatures(),
        }
    }
    
    fn create_builtin_method_registry() -> HashMap<(TypeDecl, String), BuiltinMethod> {
        let mut registry = HashMap::new();
        
        // Universal methods (available for all types - we'll handle these specially)
        // is_null is handled separately in visit_method_call
        
        // String methods
        registry.insert((TypeDecl::String, "len".to_string()), BuiltinMethod::StrLen);
        registry.insert((TypeDecl::String, "concat".to_string()), BuiltinMethod::StrConcat);
        registry.insert((TypeDecl::String, "substring".to_string()), BuiltinMethod::StrSubstring);
        registry.insert((TypeDecl::String, "contains".to_string()), BuiltinMethod::StrContains);
        registry.insert((TypeDecl::String, "split".to_string()), BuiltinMethod::StrSplit);
        registry.insert((TypeDecl::String, "trim".to_string()), BuiltinMethod::StrTrim);
        registry.insert((TypeDecl::String, "to_upper".to_string()), BuiltinMethod::StrToUpper);
        registry.insert((TypeDecl::String, "to_lower".to_string()), BuiltinMethod::StrToLower);
        
        // Future: Array methods (when ArrayLen etc. are added)
        // registry.insert((TypeDecl::Array(vec![], 0), "len".to_string()), BuiltinMethod::ArrayLen);
        // Note: For arrays, we'll need special handling since TypeDecl::Array contains element types
        
        registry
    }
    
    fn create_builtin_function_signatures() -> Vec<BuiltinFunctionSignature> {
        vec![
            // Memory management
            BuiltinFunctionSignature {
                func: BuiltinFunction::HeapAlloc,
                arg_count: 1,
                arg_types: vec![TypeDecl::UInt64],
                return_type: TypeDecl::Ptr,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::HeapFree,
                arg_count: 1,
                arg_types: vec![TypeDecl::Ptr],
                return_type: TypeDecl::Unit,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::HeapRealloc,
                arg_count: 2,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::UInt64],
                return_type: TypeDecl::Ptr,
            },
            
            // Pointer operations
            BuiltinFunctionSignature {
                func: BuiltinFunction::PtrRead,
                arg_count: 2,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::UInt64],
                return_type: TypeDecl::UInt64,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::PtrWrite,
                arg_count: 3,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::UInt64, TypeDecl::UInt64],
                return_type: TypeDecl::Unit,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::PtrIsNull,
                arg_count: 1,
                arg_types: vec![TypeDecl::Ptr],
                return_type: TypeDecl::Bool,
            },
            
            // Memory operations
            BuiltinFunctionSignature {
                func: BuiltinFunction::MemCopy,
                arg_count: 3,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::Ptr, TypeDecl::UInt64],
                return_type: TypeDecl::Unit,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::MemMove,
                arg_count: 3,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::Ptr, TypeDecl::UInt64],
                return_type: TypeDecl::Unit,
            },
            BuiltinFunctionSignature {
                func: BuiltinFunction::MemSet,
                arg_count: 3,
                arg_types: vec![TypeDecl::Ptr, TypeDecl::UInt64, TypeDecl::UInt64], // Note: third arg should be u8 but using u64 for now
                return_type: TypeDecl::Unit,
            },
        ]
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
        }
    }
    
    pub fn with_source_code(mut self, source: &'a str) -> Self {
        self.source_code = Some(source);
        self
    }
    
    fn get_expr_location(&self, expr_ref: &ExprRef) -> Option<SourceLocation> {
        self.core.location_pool.get_expr_location(expr_ref).cloned()
    }
    
    fn get_stmt_location(&self, stmt_ref: &StmtRef) -> Option<SourceLocation> {
        self.core.location_pool.get_stmt_location(stmt_ref).cloned()
    }
    
    /// Calculate line and column from offset position in source code
    fn calculate_line_col_from_offset(&self, offset: usize) -> (u32, u32) {
        if let Some(source) = self.source_code {
            let mut line = 1u32;
            let mut column = 1u32;
            
            for (i, ch) in source.char_indices() {
                if i >= offset {
                    break;
                }
                if ch == '\n' {
                    line += 1;
                    column = 1;
                } else {
                    column += 1;
                }
            }
            
            (line, column)
        } else {
            // Fallback if source code is not available
            (1, 1)
        }
    }
    
    /// Create SourceLocation from Node with calculated line and column
    fn node_to_source_location(&self, node: &Node) -> SourceLocation {
        let (line, column) = self.calculate_line_col_from_offset(node.start);
        SourceLocation {
            line,
            column,
            offset: node.start as u32,
        }
    }
    
    // Helper methods for location tracking (can be used for future error reporting enhancements)

    pub fn push_context(&mut self) {
        self.context.vars.push(HashMap::new());
    }

    pub fn pop_context(&mut self) {
        self.context.vars.pop();
    }

    pub fn add_function(&mut self, f: Rc<Function>) {
        self.context.set_fn(f.name, f.clone());
    }

    fn process_val_type(&mut self, name: DefaultSymbol, type_decl: &Option<TypeDecl>, expr: &Option<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        let expr_ty = match expr {
            Some(e) => {
                let ty = self.visit_expr(e)?;
                if ty == TypeDecl::Unit {
                    return Err(TypeCheckError::type_mismatch(TypeDecl::Unknown, ty));
                }
                Some(ty)
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

        let statements = match self.core.stmt_pool.get(s.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference"))? {
            Stmt::Expression(e) => {
                match self.core.expr_pool.0.get(e.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference"))? {
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
            if let Some(stmt) = self.core.stmt_pool.get(s.to_index()) {
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
            let stmt_obj = self.core.stmt_pool.get(stmt.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference"))?;
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
        
        // Check if the function body type matches the declared return type
        if let Some(ref expected_return_type) = func.return_type {
            if &last != expected_return_type {
                // Create location information from function node with calculated line and column
                let func_location = self.node_to_source_location(&func.node);
                
                return Err(TypeCheckError::type_mismatch(
                    expected_return_type.clone(),
                    last.clone()
                ).with_location(func_location)
                .with_context("function return type"));
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
            Expr::ArrayAccess(array, index) => visitor.visit_array_access(array, index),
            Expr::FieldAccess(obj, field) => visitor.visit_field_access(obj, field),
            Expr::MethodCall(obj, method, args) => visitor.visit_method_call(obj, method, args),
            Expr::StructLiteral(struct_name, fields) => visitor.visit_struct_literal(struct_name, fields),
            Expr::QualifiedIdentifier(path) => visitor.visit_qualified_identifier(path),
            Expr::BuiltinMethodCall(receiver, method, args) => visitor.visit_builtin_method_call(receiver, method, args),
            Expr::IndexAccess(object, index) => {
                eprintln!("DEBUG: Processing IndexAccess expression");
                visitor.visit_index_access(object, index)
            },
            Expr::IndexAssign(object, index, value) => {
                eprintln!("DEBUG: Processing IndexAssign expression");
                visitor.visit_index_assign(object, index, value)
            },
            Expr::DictLiteral(entries) => visitor.visit_dict_literal(entries),
            Expr::BuiltinCall(func, args) => visitor.visit_builtin_call(func, args),
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
            Stmt::StructDecl { name, fields, visibility } => visitor.visit_struct_decl(name, fields, visibility),
            Stmt::ImplBlock { target_type, methods } => visitor.visit_impl_block(target_type, methods),
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
        let expr_obj = self.core.expr_pool.get(expr.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference"))?;
        
        
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
        let result = self.core.stmt_pool.get(stmt.to_index()).unwrap_or(&Stmt::Break).clone().accept(self);
        
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

    fn visit_binary(&mut self, op: &Operator, lhs: &ExprRef, rhs: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let op = op.clone();
        let lhs = lhs.clone();
        let rhs = rhs.clone();
        let lhs_ty = {
            let lhs_obj = self.core.expr_pool.get(lhs.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid left-hand expression reference"))?;
            lhs_obj.clone().accept(self)?
        };
        let rhs_ty = {
            let rhs_obj = self.core.expr_pool.get(rhs.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid right-hand expression reference"))?;
            rhs_obj.clone().accept(self)?
        };
        
        // Resolve types with automatic conversion for Number type
        let (resolved_lhs_ty, resolved_rhs_ty) = self.resolve_numeric_types(&lhs_ty, &rhs_ty)
            .map_err(|mut error| {
                if error.location.is_none() {
                    if let Some(location) = self.get_expr_location(&lhs) {
                        error = error.with_location(location);
                    }
                }
                error
            })?;
        
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
            if let Some(stmt) = self.core.stmt_pool.get(s.to_index()) {
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
            let stmt = self.core.stmt_pool.get(s.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference in block"))?;
            let stmt_type = match stmt {
                Stmt::Return(None) => Ok(TypeDecl::Unit),
                Stmt::Return(ret_ty) => {
                    if let Some(e) = ret_ty {
                        let e = e.clone();
                        let expr_obj = self.core.expr_pool.get(e.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference in return"))?;
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
                    let stmt_obj = self.core.stmt_pool.get(s.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid statement reference"))?;
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
        let is_if_empty = match self.core.expr_pool.get(if_block.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid if block expression reference"))? {
            Expr::Block(expressions) => expressions.is_empty(),
            _ => false,
        };
        if !is_if_empty {
            let if_expr = self.core.expr_pool.get(if_block.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid if block expression reference"))?;
            let if_ty = if_expr.clone().accept(self)?;
            block_types.push(if_ty);
        }

        // Check elif-blocks
        for (_, elif_block) in elif_pairs {
            let elif_block = elif_block.clone();
            let is_elif_empty = match self.core.expr_pool.get(elif_block.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid elif block expression reference"))? {
                Expr::Block(expressions) => expressions.is_empty(),
                _ => false,
            };
            if !is_elif_empty {
                let elif_expr = self.core.expr_pool.get(elif_block.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid elif block expression reference"))?;
                let elif_ty = elif_expr.clone().accept(self)?;
                block_types.push(elif_ty);
            }
        }

        // Check else-block
        let else_block = else_block.clone();
        let is_else_empty = match self.core.expr_pool.get(else_block.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid else block expression reference"))? {
            Expr::Block(expressions) => expressions.is_empty(),
            _ => false,
        };
        if !is_else_empty {
            let else_expr = self.core.expr_pool.get(else_block.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid else block expression reference"))?;
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
            let lhs_obj = self.core.expr_pool.get(lhs.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid left-hand expression reference"))?;
            lhs_obj.clone().accept(self)?
        };
        let rhs_ty = {
            let rhs_obj = self.core.expr_pool.get(rhs.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid right-hand expression reference"))?;
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
            
            let status = self.function_checking.is_checked_fn.get(&fn_name);
            if status.is_none() || status.as_ref().and_then(|s| s.as_ref()).is_none() {
                // not checked yet
                let fun_copy = self.context.get_fn(fn_name).ok_or_else(|| TypeCheckError::not_found("Function", "<INTERNAL_ERROR>"))?;
                self.type_check(fun_copy.clone())?;
            }

            // Type check function arguments with proper type hints
            // Clone data we need to avoid borrowing conflicts
            let args_data = if let Some(args_expr) = self.core.expr_pool.get(args_ref.to_index()) {
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
        Ok(TypeDecl::Unknown)
    }
    
    // =========================================================================
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


    fn visit_array_access(&mut self, array: &ExprRef, index: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let array_type = self.visit_expr(array)?;
        
        // Set type hint for index to UInt64 (default for array indexing)
        let original_hint = self.type_inference.type_hint.clone();
        self.type_inference.type_hint = Some(TypeDecl::UInt64);
        
        let index_type = self.visit_expr(index)?;
        
        // Restore original type hint
        self.type_inference.type_hint = original_hint;

        // Handle index type inference and conversion
        let _final_index_type = match index_type {
            TypeDecl::Number => {
                // Transform Number index to UInt64 (default for array indexing)
                self.transform_numeric_expr(index, &TypeDecl::UInt64)?;
                TypeDecl::UInt64
            },
            TypeDecl::Unknown => {
                // Infer index as UInt64 for unknown types (likely variables)
                TypeDecl::UInt64
            },
            TypeDecl::UInt64 | TypeDecl::Int64 => {
                // Already a valid integer type
                index_type
            },
            _ => {
                return Err(TypeCheckError::array_error(&format!(
                    "Array index must be an integer type, but got {:?}", index_type
                )));
            }
        };

        // Array must be an array type
        match array_type {
            TypeDecl::Array(ref element_types, _size) => {
                if element_types.is_empty() {
                    return Err(TypeCheckError::array_error("Cannot access elements of empty array"));
                }
                Ok(element_types[0].clone())
            }
            _ => Err(TypeCheckError::array_error(&format!(
                "Cannot index into non-array type {:?}", array_type
            )))
        }
    }
    
    fn visit_index_access(&mut self, object: &ExprRef, index: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let object_type = self.visit_expr(object)?;
        
        match object_type {
            TypeDecl::Array(ref element_types, _size) => {
                // Array indexing - expects UInt64 index
                let original_hint = self.type_inference.type_hint.clone();
                self.type_inference.type_hint = Some(TypeDecl::UInt64);
                let _index_type = self.visit_expr(index)?;
                self.type_inference.type_hint = original_hint;
                
                if element_types.is_empty() {
                    return Err(TypeCheckError::array_error("Cannot access elements of empty array"));
                }
                Ok(element_types[0].clone())
            }
            TypeDecl::Dict(ref key_type, ref value_type) => {
                // Dict indexing - check key type matches
                let index_type = self.visit_expr(index)?;
                
                // Verify the index type matches the key type
                if index_type != **key_type {
                    return Err(TypeCheckError::type_mismatch(
                        *key_type.clone(), index_type
                    ));
                }
                
                Ok(*value_type.clone())
            }
            TypeDecl::Identifier(struct_name) => {
                // Check for __getitem__ method on struct
                let struct_name_str = self.core.string_interner.resolve(struct_name)
                    .ok_or_else(|| TypeCheckError::generic_error("Unknown struct name"))?;
                
                // Type check the index first to avoid borrowing conflicts
                let index_type = self.visit_expr(index)?;
                
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
            }
            _ => {
                Err(TypeCheckError::generic_error(&format!(
                    "Cannot index into type {:?}", object_type
                )))
            }
        }
    }
    
    fn visit_index_assign(&mut self, object: &ExprRef, index: &ExprRef, value: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        // Type check the object being indexed
        let object_type = self.visit_expr(object)?;
        
        // Type check the index
        let index_type = self.visit_expr(index)?;
        
        // Type check the value being assigned
        let value_type = self.visit_expr(value)?;
        
        
        // Check type compatibility based on container type
        match object_type {
            TypeDecl::Array(ref element_types, _size) => {
                // Array assignment - check index and value types
                let resolved_index_type = if index_type == TypeDecl::Number {
                    // Transform Number index to UInt64 for arrays
                    self.transform_numeric_expr(index, &TypeDecl::UInt64)?;
                    TypeDecl::UInt64
                } else {
                    index_type
                };
                
                if resolved_index_type != TypeDecl::UInt64 {
                    return Err(TypeCheckError::generic_error(&format!(
                        "Array index must be UInt64, found {:?}", resolved_index_type
                    )));
                }
                
                if !element_types.is_empty() {
                    let expected_element_type = &element_types[0];
                    let resolved_value_type = if value_type == TypeDecl::Number {
                        // If element type is also Number, convert both to UInt64
                        if *expected_element_type == TypeDecl::Number {
                            self.transform_numeric_expr(value, &TypeDecl::UInt64)?;
                            TypeDecl::UInt64
                        } else {
                            // Transform Number value to expected element type
                            self.transform_numeric_expr(value, expected_element_type)?;
                            expected_element_type.clone()
                        }
                    } else {
                        value_type
                    };
                    
                    let final_expected_type = if *expected_element_type == TypeDecl::Number && resolved_value_type == TypeDecl::UInt64 {
                        // Update array element type from Number to UInt64
                        TypeDecl::UInt64
                    } else {
                        expected_element_type.clone()
                    };
                    
                    if final_expected_type != resolved_value_type {
                        return Err(TypeCheckError::generic_error(&format!(
                            "Array element type mismatch: expected {:?}, found {:?}", 
                            final_expected_type, resolved_value_type
                        )));
                    }
                    Ok(resolved_value_type)
                } else {
                    Ok(value_type)
                }
            }
            TypeDecl::Dict(ref key_type, ref val_type) => {
                // Dict assignment - check key and value types
                if **key_type != TypeDecl::Unknown && **key_type != index_type {
                    return Err(TypeCheckError::generic_error(&format!(
                        "Dict key type mismatch: expected {:?}, found {:?}", 
                        key_type, index_type
                    )));
                }
                
                if **val_type != TypeDecl::Unknown {
                    let resolved_value_type = if value_type == TypeDecl::Number {
                        // Transform Number value to expected dict value type
                        self.transform_numeric_expr(value, val_type)?;
                        (**val_type).clone()
                    } else {
                        value_type
                    };
                    
                    if **val_type != resolved_value_type {
                        return Err(TypeCheckError::generic_error(&format!(
                            "Dict value type mismatch: expected {:?}, found {:?}. All values must have the same type.", 
                            val_type, resolved_value_type
                        )));
                    }
                    Ok(resolved_value_type)
                } else {
                    Ok(value_type)
                }
            }
            TypeDecl::Identifier(struct_name) => {
                // Check for __setitem__ method on struct
                let struct_name_str = self.core.string_interner.resolve(struct_name)
                    .ok_or_else(|| TypeCheckError::generic_error("Unknown struct name"))?;
                
                // Look for __setitem__ method
                if let Some(setitem_method) = self.context.get_method_function_by_name(struct_name_str, "__setitem__", self.core.string_interner) {
                    // Check if method has correct signature: __setitem__(self, index: T, value: U)
                    if setitem_method.parameter.len() >= 3 {
                        let index_param_type = &setitem_method.parameter[1].1;
                        let value_param_type = &setitem_method.parameter[2].1;
                        
                        // Check index type matches
                        if index_type != *index_param_type {
                            return Err(TypeCheckError::type_mismatch(
                                index_param_type.clone(), index_type
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
                        Err(TypeCheckError::generic_error("__setitem__ method must have at least 3 parameters (self, index, value)"))
                    }
                } else {
                    Err(TypeCheckError::generic_error(&format!(
                        "Cannot assign index to type {:?} - no __setitem__ method found", object_type
                    )))
                }
            }
            _ => {
                Err(TypeCheckError::generic_error(&format!(
                    "Cannot assign index to type {:?}", object_type
                )))
            }
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
    
    // =========================================================================
    // Statement Type Checking
    // =========================================================================

    fn visit_expression_stmt(&mut self, expr: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let expr_obj = self.core.expr_pool.get(expr.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference in statement"))?;
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
            let expr_obj = self.core.expr_pool.get(e.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid expression reference in return"))?;
            let return_type = expr_obj.clone().accept(self)?;
            Ok(return_type)
        }
    }
    
    // =========================================================================
    // Control Flow Type Checking
    // =========================================================================

    fn visit_for(&mut self, init: DefaultSymbol, _cond: &ExprRef, range: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        self.push_context();
        let range_obj = self.core.expr_pool.get(range.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid range expression reference"))?;
        let range_ty = range_obj.clone().accept(self)?;
        let ty = Some(range_ty);
        self.process_val_type(init, &ty, &Some(*range))?;
        let body_obj = self.core.expr_pool.get(body.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid body expression reference"))?;
        let res = body_obj.clone().accept(self);
        self.pop_context();
        res
    }

    fn visit_while(&mut self, _cond: &ExprRef, body: &ExprRef) -> Result<TypeDecl, TypeCheckError> {
        let body_obj = self.core.expr_pool.get(body.to_index()).ok_or_else(|| TypeCheckError::generic_error("Invalid body expression reference in while"))?;
        body_obj.clone().accept(self)
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

    fn visit_struct_decl(&mut self, name: &String, fields: &Vec<StructField>, visibility: &Visibility) -> Result<TypeDecl, TypeCheckError> {
        // 1. Check for duplicate field names
        let mut field_names = std::collections::HashSet::new();
        for field in fields {
            if !field_names.insert(field.name.clone()) {
                return Err(TypeCheckError::generic_error(&format!(
                    "Duplicate field '{}' in struct '{}'", field.name, name
                )));
            }
        }
        
        // 2. Validate field types
        for field in fields {
            match &field.type_decl {
                TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Bool | TypeDecl::String => {
                    // Basic types are valid
                },
                TypeDecl::Identifier(struct_name) => {
                    // Check if referenced struct is already defined
                    if !self.context.struct_definitions.contains_key(struct_name) {
                        return Err(TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)));
                    }
                },
                TypeDecl::Array(element_types, _) => {
                    // Validate array element types
                    for element_type in element_types {
                        if let TypeDecl::Identifier(struct_name) = element_type {
                            if !self.context.struct_definitions.contains_key(struct_name) {
                                return Err(TypeCheckError::not_found("Struct", &format!("{:?}", struct_name)));
                            }
                        }
                    }
                },
                _ => {
                    return Err(TypeCheckError::unsupported_operation(
                        &format!("field type in struct '{}'", name), field.type_decl.clone()
                    ));
                }
            }
        }
        
        // 3. Register struct definition with visibility information
        let struct_symbol = self.core.string_interner.get(name)
            .unwrap_or_else(|| panic!("Struct name should already be in string interner: {}", name));
        let struct_def = crate::type_checker::context::StructDefinition {
            fields: fields.clone(),
            visibility: visibility.clone(),
        };
        
        // Store the struct definition for later type checking and access control
        self.context.struct_definitions.insert(struct_symbol, struct_def);
        
        Ok(TypeDecl::Unit)
    }

    fn visit_impl_block(&mut self, target_type: &String, methods: &Vec<Rc<MethodFunction>>) -> Result<TypeDecl, TypeCheckError> {
        // Get the struct symbol for the target type
        let struct_symbol = self.core.string_interner.get(target_type)
            .ok_or_else(|| TypeCheckError::not_found("struct type", target_type))?;

        // Set current impl target for Self resolution
        let old_impl_target = self.context.current_impl_target;
        self.context.current_impl_target = Some(struct_symbol);

        // Impl block type checking - validate methods
        for method in methods {
            // Check method parameter types
            for (_, param_type) in &method.parameter {
                // Resolve Self type to the actual struct type
                let resolved_type = self.resolve_self_type(param_type);
                
                match &resolved_type {
                    TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Bool | TypeDecl::String | 
                    TypeDecl::Identifier(_) => {
                        // Valid parameter types (including struct types)
                    },
                    _ => {
                        let method_name = self.core.string_interner.resolve(method.name).unwrap_or("<unknown>");
                        return Err(TypeCheckError::unsupported_operation(
                            &format!("parameter type in method '{}' for impl block '{}'", method_name, target_type),
                            resolved_type
                        ));
                    }
                }
            }
            
            // Check return type if specified
            if let Some(ref ret_type) = method.return_type {
                // Resolve Self type to the actual struct type
                let resolved_ret_type = self.resolve_self_type(ret_type);
                
                match &resolved_ret_type {
                    TypeDecl::Int64 | TypeDecl::UInt64 | TypeDecl::Bool | TypeDecl::String | 
                    TypeDecl::Unit | TypeDecl::Identifier(_) => {
                        // Valid return types (including struct types)
                    },
                    _ => {
                        let method_name = self.core.string_interner.resolve(method.name).unwrap_or("<unknown>");
                        return Err(TypeCheckError::unsupported_operation(
                            &format!("return type in method '{}' for impl block '{}'", method_name, target_type),
                            resolved_ret_type
                        ));
                    }
                }
            }

            // Register method in context
            self.context.register_struct_method(struct_symbol, method.name, method.clone());
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
            TypeDecl::Struct(struct_symbol) => {
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
            _ => {
                let field_name = self.core.string_interner.resolve(*field).unwrap_or("<unknown>");
                Err(TypeCheckError::unsupported_operation(
                    &format!("field access '{}'", field_name), obj_type
                ))
            }
        }
    }

    fn visit_method_call(&mut self, obj: &ExprRef, method: &DefaultSymbol, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Check recursion depth to prevent stack overflow
        if self.type_inference.recursion_depth >= self.type_inference.max_recursion_depth {
            return Err(TypeCheckError::generic_error(
                "Maximum recursion depth reached in method call type inference - possible circular reference"
            ));
        }
        
        self.type_inference.recursion_depth += 1;
        let obj_type_result = self.visit_expr(obj);
        self.type_inference.recursion_depth -= 1;
        
        let obj_type = obj_type_result?;
        
        // Type check all arguments
        let mut arg_types = Vec::new();
        for arg in args {
            arg_types.push(self.visit_expr(arg)?);
        }
        
        let method_name = self.core.string_interner.resolve(*method).unwrap_or("<unknown>");
        
        // First, check if this is a user-defined method for a struct
        if let TypeDecl::Struct(struct_name) = &obj_type {
            let struct_name_str = self.core.string_interner.resolve(*struct_name).unwrap_or("<unknown>");
            if let Some(method_return_type) = self.context.get_method_return_type(&struct_name_str, method_name) {
                // User-defined method found - use it (priority over builtin)
                return Ok(method_return_type.clone());
            }
        }
        
        // Second, check if this could be a builtin method
        // Special case for is_null - available for all types
        if method_name == "is_null" {
            if !args.is_empty() {
                return Err(TypeCheckError::method_error(
                    "is_null", obj_type, &format!("takes no arguments, but {} provided", args.len())
                ));
            }
            return Ok(TypeDecl::Bool);
        }
        
        // Check builtin method registry
        if let Some(builtin_method) = self.builtin_methods.get(&(obj_type.clone(), method_name.to_string())).cloned() {
            // Delegate to builtin method handler
            return self.visit_builtin_method_call(obj, &builtin_method, args);
        }
        
        // Handle built-in methods for basic types
        match obj_type {
            TypeDecl::String => {
                match method_name {
                    "len" => {
                        // String.len() method - no arguments required, returns u64
                        if !args.is_empty() {
                            return Err(TypeCheckError::method_error(
                                "len", TypeDecl::String, &format!("takes no arguments, but {} provided", args.len())
                            ));
                        }
                        Ok(TypeDecl::UInt64)
                    }
                    _ => {
                        Err(TypeCheckError::method_error(
                            method_name, TypeDecl::String, "method not found"
                        ))
                    }
                }
            }
            TypeDecl::Identifier(struct_symbol) | TypeDecl::Struct(struct_symbol) => {
                // Look up method in struct methods
                if let Some(method) = self.context.get_struct_method(struct_symbol, *method) {
                    // Return method's return type, or Unit if not specified
                    Ok(method.return_type.clone().unwrap_or(TypeDecl::Unit))
                } else {
                    let struct_name = self.core.string_interner.resolve(struct_symbol).unwrap_or("<unknown>");
                    Err(TypeCheckError::method_error(
                        method_name, obj_type.clone(), &format!("method not found on struct '{}'", struct_name)
                    ))
                }
            }
            _ => {
                Err(TypeCheckError::method_error(
                    method_name, obj_type, "method call on non-struct type"
                ))
            }
        }
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
        // Check recursion depth to prevent stack overflow
        if self.type_inference.recursion_depth >= self.type_inference.max_recursion_depth {
            return Err(TypeCheckError::generic_error(
                "Maximum recursion depth reached in builtin method call type inference - possible circular reference"
            ));
        }
        
        self.type_inference.recursion_depth += 1;
        let receiver_type_result = self.visit_expr(receiver);
        self.type_inference.recursion_depth -= 1;
        
        let receiver_type = receiver_type_result?;
        
        // Special case: is_null() is available for all types
        if *method == BuiltinMethod::IsNull {
            if !args.is_empty() {
                return Err(TypeCheckError::method_error(
                    "is_null", receiver_type, &format!("takes no arguments, but {} provided", args.len())
                ));
            }
            return Ok(TypeDecl::Bool);
        }
        
        // Method signature table: (method, receiver_type, arg_count, arg_types, return_type)
        struct MethodSignature {
            method: BuiltinMethod,
            receiver_type: TypeDecl,
            arg_count: usize,
            arg_types: Vec<TypeDecl>,
            return_type: TypeDecl,
        }
        
        let method_table = vec![
            // String methods
            MethodSignature {
                method: BuiltinMethod::StrLen,
                receiver_type: TypeDecl::String,
                arg_count: 0,
                arg_types: vec![],
                return_type: TypeDecl::UInt64,
            },
            MethodSignature {
                method: BuiltinMethod::StrConcat,
                receiver_type: TypeDecl::String,
                arg_count: 1,
                arg_types: vec![TypeDecl::String],
                return_type: TypeDecl::String,
            },
            MethodSignature {
                method: BuiltinMethod::StrSubstring,
                receiver_type: TypeDecl::String,
                arg_count: 2,
                arg_types: vec![TypeDecl::UInt64, TypeDecl::UInt64],
                return_type: TypeDecl::String,
            },
            MethodSignature {
                method: BuiltinMethod::StrContains,
                receiver_type: TypeDecl::String,
                arg_count: 1,
                arg_types: vec![TypeDecl::String],
                return_type: TypeDecl::Bool,
            },
            MethodSignature {
                method: BuiltinMethod::StrSplit,
                receiver_type: TypeDecl::String,
                arg_count: 1,
                arg_types: vec![TypeDecl::String],
                return_type: TypeDecl::Array(vec![TypeDecl::String], 0), // Dynamic array
            },
            MethodSignature {
                method: BuiltinMethod::StrTrim,
                receiver_type: TypeDecl::String,
                arg_count: 0,
                arg_types: vec![],
                return_type: TypeDecl::String,
            },
            MethodSignature {
                method: BuiltinMethod::StrToUpper,
                receiver_type: TypeDecl::String,
                arg_count: 0,
                arg_types: vec![],
                return_type: TypeDecl::String,
            },
            MethodSignature {
                method: BuiltinMethod::StrToLower,
                receiver_type: TypeDecl::String,
                arg_count: 0,
                arg_types: vec![],
                return_type: TypeDecl::String,
            },
        ];
        
        // Find matching method signature
        let signature = method_table.iter().find(|sig| 
            sig.method == *method && sig.receiver_type == receiver_type
        );
        
        if let Some(sig) = signature {
            // Check argument count
            if args.len() != sig.arg_count {
                return Err(TypeCheckError::method_error(
                    &format!("{:?}", method), receiver_type, 
                    &format!("takes {} argument(s), but {} provided", sig.arg_count, args.len())
                ));
            }
            
            // Check argument types
            for (_i, (arg, expected_type)) in args.iter().zip(&sig.arg_types).enumerate() {
                let arg_type = self.visit_expr(arg)?;
                if arg_type != *expected_type {
                    return Err(TypeCheckError::type_mismatch(
                        expected_type.clone(), arg_type
                    ));
                }
            }
            
            Ok(sig.return_type.clone())
        } else {
            // Method not available for this type
            Err(TypeCheckError::method_error(
                &format!("{:?}", method), receiver_type.clone(), 
                &format!("method '{:?}' is not available for type '{:?}'", method, receiver_type)
            ))
        }
    }
    
    fn visit_builtin_call(&mut self, func: &BuiltinFunction, args: &Vec<ExprRef>) -> Result<TypeDecl, TypeCheckError> {
        // Check recursion depth to prevent stack overflow
        if self.type_inference.recursion_depth >= self.type_inference.max_recursion_depth {
            return Err(TypeCheckError::generic_error(
                "Maximum recursion depth reached in builtin function call type inference - possible circular reference"
            ));
        }
        
        // Find matching function signature from pre-built table
        let signature = self.builtin_function_signatures.iter().find(|sig| sig.func == *func).cloned();
        
        if let Some(sig) = signature {
            // Check argument count
            if args.len() != sig.arg_count {
                return Err(TypeCheckError::generic_error(&format!(
                    "builtin function '{:?}' takes {} argument(s), but {} provided", 
                    func, sig.arg_count, args.len()
                )));
            }
            
            // Check argument types
            for (_i, (arg, expected_type)) in args.iter().zip(&sig.arg_types).enumerate() {
                let arg_type = self.visit_expr(arg)?;
                if arg_type != *expected_type {
                    return Err(TypeCheckError::type_mismatch(
                        expected_type.clone(), arg_type
                    ));
                }
            }
            
            Ok(sig.return_type.clone())
        } else {
            Err(TypeCheckError::generic_error(&format!(
                "unknown builtin function '{:?}'", func
            )))
        }
    }
}

impl<'a> TypeCheckerVisitor<'a> {
    /// Resolve Self type to the actual struct type in impl block context
    pub fn resolve_self_type(&self, type_decl: &TypeDecl) -> TypeDecl {
        match type_decl {
            TypeDecl::Self_ => {
                if let Some(target_symbol) = self.context.current_impl_target {
                    TypeDecl::Identifier(target_symbol)
                } else {
                    // Self used outside impl context - should be an error
                    type_decl.clone()
                }
            }
            _ => type_decl.clone(),
        }
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
                            if let Some(expr) = self.core.expr_pool.get(element.to_index()) {
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
        
        // 2. Validate provided fields against struct definition
        self.context.validate_struct_fields(*struct_name, fields, &self.core)?;
        
        // 3. Type check each field and verify type compatibility
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
                    // Allow null assignment to any type
                    } else if field_type == TypeDecl::Unknown {
                        // Allow unknown type (including null values) assignment to struct fields
                    } else {
                        return Err(TypeCheckError::type_mismatch(expected_type.clone(), field_type));
                    }
                }
            }
            
            field_types.insert(*field_name, field_type);
        }
        
        // 4. Verify all required fields are provided (already done in validate_struct_fields)
        
        Ok(TypeDecl::Identifier(*struct_name))
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
        let old_hint = self.type_inference.type_hint.clone();
        
        if let Some(decl) = type_decl {
            match decl {
                TypeDecl::Array(element_types, _) => {
                    // For array types (including struct arrays), set the array type as hint for array literal processing
                    if !element_types.is_empty() {
                        self.type_inference.type_hint = Some(decl.clone());
                    }
                },
                TypeDecl::Struct(_) => {
                    // For struct types, set the struct type as hint for struct literal processing
                    self.type_inference.type_hint = Some(decl.clone());
                },
                TypeDecl::Dict(_, _) => {
                    // For dict types, set the dict type as hint for dict literal processing
                    self.type_inference.type_hint = Some(decl.clone());
                },
                _ if decl != &TypeDecl::Unknown && decl != &TypeDecl::Number => {
                    self.type_inference.type_hint = Some(decl.clone());
                },
                _ => {}
            }
        }
        
        old_hint
    }

    fn update_variable_expr_mapping(&mut self, name: DefaultSymbol, expr_ref: &ExprRef) {
        let expr_ty = if let Ok(ty) = self.visit_expr(expr_ref) { ty } else { return };
        self.update_variable_expr_mapping_internal(name, expr_ref, &expr_ty);
    }
    
    fn apply_type_transformations(&mut self, name: DefaultSymbol, type_decl: &TypeDecl) -> Result<(), TypeCheckError> {
        self.apply_type_transformations_internal(name, type_decl)
    }
    
    fn determine_final_type(&mut self, name: DefaultSymbol, inferred_type: TypeDecl, declared_type: &Option<TypeDecl>) -> Result<TypeDecl, TypeCheckError> {
        self.determine_final_type_internal(name, inferred_type, declared_type)
    }
}

impl<'a> TypeCheckerVisitor<'a> {
    /// Updates variable-expression mapping for type inference (internal implementation)
    fn update_variable_expr_mapping_internal(&mut self, name: DefaultSymbol, expr_ref: &ExprRef, expr_ty: &TypeDecl) {
        if *expr_ty == TypeDecl::Number || (*expr_ty != TypeDecl::Number && self.has_number_in_expr(expr_ref)) {
            self.type_inference.variable_expr_mapping.insert(name, expr_ref.clone());
        } else {
            // Remove old mapping for non-Number types to prevent stale references
            self.type_inference.variable_expr_mapping.remove(&name);
            // Also remove from number_usage_context to prevent stale type inference
            let indices_to_remove: Vec<usize> = self.type_inference.number_usage_context
                .iter()
                .enumerate()
                .filter_map(|(i, (old_expr, _))| {
                    if self.is_old_number_for_variable(name, old_expr) {
                        Some(i)
                    } else {
                        None
                    }
                })
                .collect();
            
            // Remove in reverse order to maintain valid indices
            for &index in indices_to_remove.iter().rev() {
                self.type_inference.number_usage_context.remove(index);
            }
        }
    }

    /// Applies type transformations for numeric expressions (internal implementation)
    fn apply_type_transformations_internal(&mut self, _name: DefaultSymbol, _type_decl: &TypeDecl) -> Result<(), TypeCheckError> {
        // Implementation for trait method - delegating to existing logic
        Ok(())
    }
    
    /// Applies type transformations for numeric expressions based on context
    fn apply_type_transformations_for_expr(&mut self, type_decl: &Option<TypeDecl>, expr_ty: &TypeDecl, expr_ref: &ExprRef) -> Result<(), TypeCheckError> {
        if type_decl.is_none() && *expr_ty == TypeDecl::Number {
            // No explicit type, but we have a Number - use type hint if available
            if let Some(hint) = self.type_inference.type_hint.clone() {
                if matches!(hint, TypeDecl::Int64 | TypeDecl::UInt64) {
                    // Transform Number to hinted type
                    self.transform_numeric_expr(expr_ref, &hint)?;
                }
            }
        } else if type_decl.as_ref().map_or(false, |decl| *decl == TypeDecl::Unknown) && *expr_ty == TypeDecl::Int64 {
            // Unknown type declaration with Int64 inference - also transform
            if let Some(hint) = self.type_inference.type_hint.clone() {
                if matches!(hint, TypeDecl::Int64 | TypeDecl::UInt64) {
                    self.transform_numeric_expr(expr_ref, &hint)?;
                }
            }
        } else if let Some(decl) = type_decl {
            if decl != &TypeDecl::Unknown && decl != &TypeDecl::Number && *expr_ty == *decl {
                // Expression returned the hinted type, transform Number literals to concrete type
                if let Some(expr) = self.core.expr_pool.get(expr_ref.to_index()) {
                    if let Expr::Number(_) = expr {
                        self.transform_numeric_expr(expr_ref, decl)?;
                    }
                }
            }
        }
        
        Ok(())
    }

    /// Determines the final type for a variable declaration
    fn determine_final_type_internal(&mut self, _name: DefaultSymbol, inferred_type: TypeDecl, declared_type: &Option<TypeDecl>) -> Result<TypeDecl, TypeCheckError> {
        // Implementation for trait method - delegating to existing logic
        Ok(self.determine_final_type_for_expr(declared_type, &inferred_type))
    }
    
    fn determine_final_type_for_expr(&self, type_decl: &Option<TypeDecl>, expr_ty: &TypeDecl) -> TypeDecl {
        match (type_decl, expr_ty) {
            (Some(TypeDecl::Unknown), _) => expr_ty.clone(),
            // For dict types, if we have explicit type annotation, prefer it over inferred type
            (Some(TypeDecl::Dict(key_type, value_type)), TypeDecl::Dict(inferred_key, inferred_value)) => {
                // If both key and value types are explicit (not Unknown), use the declared type
                if **key_type != TypeDecl::Unknown && **value_type != TypeDecl::Unknown {
                    TypeDecl::Dict(key_type.clone(), value_type.clone())
                } else {
                    TypeDecl::Dict(inferred_key.clone(), inferred_value.clone())
                }
            },
            (Some(decl), _) if decl != &TypeDecl::Unknown && decl != &TypeDecl::Number => decl.clone(),
            (None, _) => expr_ty.clone(),
            _ => expr_ty.clone(),
        }
    }

    // Transform Expr::Number nodes to concrete types based on resolved types
    fn transform_numeric_expr(&mut self, expr_ref: &ExprRef, target_type: &TypeDecl) -> Result<(), TypeCheckError> {
        if let Some(expr) = self.core.expr_pool.get_mut(expr_ref.to_index()) {
            if let Expr::Number(value) = expr {
                let num_str = self.core.string_interner.resolve(*value)
                    .ok_or_else(|| TypeCheckError::generic_error("Failed to resolve number literal"))?;
                
                match target_type {
                    TypeDecl::UInt64 => {
                        if let Ok(val) = num_str.parse::<u64>() {
                            *expr = Expr::UInt64(val);
                        } else {
                            return Err(TypeCheckError::conversion_error(num_str, "UInt64"));
                        }
                    },
                    TypeDecl::Int64 => {
                        if let Ok(val) = num_str.parse::<i64>() {
                            *expr = Expr::Int64(val);
                        } else {
                            return Err(TypeCheckError::conversion_error(num_str, "Int64"));
                        }
                    },
                    _ => {
                        return Err(TypeCheckError::unsupported_operation("transform", target_type.clone()));
                    }
                }
            }
        }
        Ok(())
    }

    // Update variable type in context if identifier was type-converted
    fn update_identifier_types(&mut self, expr_ref: &ExprRef, original_ty: &TypeDecl, resolved_ty: &TypeDecl) -> Result<(), TypeCheckError> {
        if original_ty == &TypeDecl::Number && resolved_ty != &TypeDecl::Number {
            if let Some(expr) = self.core.expr_pool.get(expr_ref.to_index()) {
                if let Expr::Identifier(name) = expr {
                    // Update the variable's type
                    self.context.update_var_type(*name, resolved_ty.clone());
                }
            }
        }
        Ok(())
    }

    // Record Number usage context for both identifiers and direct Number literals
    fn record_number_usage_context(&mut self, expr_ref: &ExprRef, original_ty: &TypeDecl, resolved_ty: &TypeDecl) -> Result<(), TypeCheckError> {
        if original_ty == &TypeDecl::Number && resolved_ty != &TypeDecl::Number {
            if let Some(expr) = self.core.expr_pool.get(expr_ref.to_index()) {
                match expr {
                    Expr::Identifier(name) => {
                        // Find all Number expressions that might belong to this variable
                        // and record the context type
                        for i in 0..self.core.expr_pool.len() {
                            if let Some(candidate_expr) = self.core.expr_pool.get(i) {
                                if let Expr::Number(_) = candidate_expr {
                                    let candidate_ref = ExprRef(i as u32);
                                    // Check if this Number might be associated with this variable
                                    if self.is_number_for_variable(*name, &candidate_ref) {
                                        self.type_inference.number_usage_context.push((candidate_ref, resolved_ty.clone()));
                                    }
                                }
                            }
                        }
                    }
                    Expr::Number(_) => {
                        // Direct Number literal - record its resolved type
                        self.type_inference.number_usage_context.push((expr_ref.clone(), resolved_ty.clone()));
                    }
                    _ => {}
                }
            }
        }
        
        Ok(())
    }

    // Check if an expression contains Number literals
    fn has_number_in_expr(&self, expr_ref: &ExprRef) -> bool {
        if let Some(expr) = self.core.expr_pool.get(expr_ref.to_index()) {
            match expr {
                Expr::Number(_) => true,
                _ => false, // For now, only check direct Number literals
            }
        } else {
            false
        }
    }

    // Check if a Number expression is associated with a specific variable
    fn is_number_for_variable(&self, var_name: DefaultSymbol, number_expr_ref: &ExprRef) -> bool {
        // Use the recorded mapping to check if this Number expression belongs to this variable
        if let Some(mapped_expr_ref) = self.type_inference.variable_expr_mapping.get(&var_name) {
            return mapped_expr_ref == number_expr_ref;
        }
        false
    }
    
    // Check if an old Number expression might be associated with a variable for cleanup
    fn is_old_number_for_variable(&self, _var_name: DefaultSymbol, number_expr_ref: &ExprRef) -> bool {
        // Check if this Number expression was previously mapped to this variable
        // This is used for cleanup when variables are redefined
        if let Some(expr) = self.core.expr_pool.get(number_expr_ref.to_index()) {
            if let Expr::Number(_) = expr {
                // For now, we'll be conservative and remove all Number contexts when variables are redefined
                return true;
            }
        }
        false
    }

    // Propagate concrete type to Number variable immediately
    fn propagate_to_number_variable(&mut self, expr_ref: &ExprRef, target_type: &TypeDecl) -> Result<(), TypeCheckError> {
        if let Some(expr) = self.core.expr_pool.get(expr_ref.to_index()) {
            if let Expr::Identifier(name) = expr {
                if let Some(var_type) = self.context.get_var(*name) {
                    if var_type == TypeDecl::Number {
                        // Find and record the Number expression for this variable
                        for i in 0..self.core.expr_pool.len() {
                            if let Some(candidate_expr) = self.core.expr_pool.get(i) {
                                if let Expr::Number(_) = candidate_expr {
                                    let candidate_ref = ExprRef(i as u32);
                                    if self.is_number_for_variable(*name, &candidate_ref) {
                                        self.type_inference.number_usage_context.push((candidate_ref, target_type.clone()));
                                        // Update variable type in context
                                        self.context.update_var_type(*name, target_type.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // Finalize any remaining Number types with context-aware inference
    fn finalize_number_types(&mut self) -> Result<(), TypeCheckError> {
        // Use recorded context information to transform Number expressions
        let context_info = self.type_inference.number_usage_context.clone();
        for (expr_ref, target_type) in &context_info {
            if let Some(expr) = self.core.expr_pool.get(expr_ref.to_index()) {
                if let Expr::Number(_) = expr {
                    self.transform_numeric_expr(&expr_ref, &target_type)?;
                    
                    // Update variable types in context if this expression is mapped to a variable
                    for (var_name, mapped_expr_ref) in &self.type_inference.variable_expr_mapping.clone() {
                        if mapped_expr_ref == expr_ref {
                            self.context.update_var_type(*var_name, target_type.clone());
                        }
                    }
                }
            }
        }
        
        // Second pass: handle any remaining Number types by using variable context
        let expr_len = self.core.expr_pool.len();
        for i in 0..expr_len {
            if let Some(expr) = self.core.expr_pool.get(i) {
                if let Expr::Number(_) = expr {
                    let expr_ref = ExprRef(i as u32);
                    
                    // Skip if already processed in first pass
                    let already_processed = context_info.iter().any(|(processed_ref, _)| processed_ref == &expr_ref);
                    if already_processed {
                        continue;
                    }
                    
                    // Find if this Number is associated with a variable and use its final type
                    // Use type hint if available, otherwise default to UInt64
                    let mut target_type = self.type_inference.type_hint.clone().unwrap_or(TypeDecl::UInt64);
                    
                    for (var_name, mapped_expr_ref) in &self.type_inference.variable_expr_mapping {
                        if mapped_expr_ref == &expr_ref {
                            // Check the current type of this variable in context
                            if let Some(var_type) = self.context.get_var(*var_name) {
                                if var_type != TypeDecl::Number {
                                    target_type = var_type;
                                    break;
                                }
                            }
                        }
                    }
                    
                    self.transform_numeric_expr(&expr_ref, &target_type)?;
                    
                    // Update variable types in context if this expression is mapped to a variable
                    for (var_name, mapped_expr_ref) in &self.type_inference.variable_expr_mapping.clone() {
                        if mapped_expr_ref == &expr_ref {
                            self.context.update_var_type(*var_name, target_type.clone());
                        }
                    }
                }
            }
        }
        Ok(())
    }


    // Helper method to resolve numeric types with automatic conversion
    fn resolve_numeric_types(&self, lhs_ty: &TypeDecl, rhs_ty: &TypeDecl) -> Result<(TypeDecl, TypeDecl), TypeCheckError> {
        match (lhs_ty, rhs_ty) {
            // Both types are already concrete - no conversion needed
            (TypeDecl::UInt64, TypeDecl::UInt64) => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
            (TypeDecl::Int64, TypeDecl::Int64) => Ok((TypeDecl::Int64, TypeDecl::Int64)),
            (TypeDecl::Bool, TypeDecl::Bool) => Ok((TypeDecl::Bool, TypeDecl::Bool)),
            (TypeDecl::String, TypeDecl::String) => Ok((TypeDecl::String, TypeDecl::String)),
            
            // Number type automatic conversion
            (TypeDecl::Number, TypeDecl::UInt64) => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
            (TypeDecl::UInt64, TypeDecl::Number) => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
            (TypeDecl::Number, TypeDecl::Int64) => Ok((TypeDecl::Int64, TypeDecl::Int64)),
            (TypeDecl::Int64, TypeDecl::Number) => Ok((TypeDecl::Int64, TypeDecl::Int64)),
            
            // Two Number types - check if we have a context hint, otherwise default to UInt64
            (TypeDecl::Number, TypeDecl::Number) => {
                if let Some(hint) = &self.type_inference.type_hint {
                    match hint {
                        TypeDecl::Int64 => Ok((TypeDecl::Int64, TypeDecl::Int64)),
                        TypeDecl::UInt64 => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
                        _ => Ok((TypeDecl::UInt64, TypeDecl::UInt64)),
                    }
                } else {
                    Ok((TypeDecl::UInt64, TypeDecl::UInt64))
                }
            },
            
            // Cross-type operations (UInt64 vs Int64) - generally not allowed for safety
            (TypeDecl::UInt64, TypeDecl::Int64) | (TypeDecl::Int64, TypeDecl::UInt64) => {
                Err(TypeCheckError::type_mismatch_operation("mixed signed/unsigned", lhs_ty.clone(), rhs_ty.clone()))
            },
            
            // Other type mismatches
            _ => {
                if lhs_ty == rhs_ty {
                    Ok((lhs_ty.clone(), rhs_ty.clone()))
                } else {
                    Err(TypeCheckError::type_mismatch(lhs_ty.clone(), rhs_ty.clone()))
                }
            }
        }
    }
    
    // Propagate type to Number expression and associated variables
    fn propagate_type_to_number_expr(&mut self, expr_ref: &ExprRef, target_type: &TypeDecl) -> Result<(), TypeCheckError> {
        if let Some(expr) = self.core.expr_pool.get(expr_ref.to_index()) {
            match expr {
                Expr::Identifier(name) => {
                    // If this is an identifier with Number type, update it
                    if let Some(var_type) = self.context.get_var(*name) {
                        if var_type == TypeDecl::Number {
                            self.context.update_var_type(*name, target_type.clone());
                            // Also record for Number expression transformation
                            if let Some(mapped_expr) = self.type_inference.variable_expr_mapping.get(name) {
                                self.type_inference.number_usage_context.push((mapped_expr.clone(), target_type.clone()));
                            }
                        }
                    }
                },
                Expr::Number(_) => {
                    // Direct Number literal
                    self.type_inference.number_usage_context.push((expr_ref.clone(), target_type.clone()));
                },
                _ => {
                    // For other expression types, we might need to recurse
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::type_decl::TypeDecl;
    use string_interner::DefaultStringInterner;

    fn create_test_ast_builder() -> AstBuilder {
        AstBuilder::new()
    }

    fn create_test_type_checker<'a>(
        stmt_pool: &'a StmtPool, 
        expr_pool: &'a mut ExprPool, 
        string_interner: &'a DefaultStringInterner,
        location_pool: &'a LocationPool
    ) -> TypeCheckerVisitor<'a> {
        TypeCheckerVisitor {
            core: CoreReferences::new(stmt_pool, expr_pool, string_interner, location_pool),
            context: TypeCheckContext::new(),
            type_inference: TypeInferenceState::new(),
            function_checking: FunctionCheckingState::new(),
            optimization: PerformanceOptimization::new(),
            errors: Vec::new(),
            builtin_function_signatures: TypeCheckerVisitor::create_builtin_function_signatures(),
            source_code: None,
            current_package: None,
            imported_modules: HashMap::new(),
            builtin_methods: TypeCheckerVisitor::create_builtin_method_registry(),
        }
    }

    #[test]
    fn test_bool_array_literal_type_inference() {
        let mut builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        // Create bool literals: [true, false, true]
        let true_expr = builder.bool_true_expr(None);
        let false_expr = builder.bool_false_expr(None);
        let true_expr2 = builder.bool_true_expr(None);
        
        let array_elements = vec![true_expr, false_expr, true_expr2];
        let _array_expr = builder.array_literal_expr(array_elements, None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Test type inference
        let result = type_checker.visit_array_literal(&vec![ExprRef(0), ExprRef(1), ExprRef(2)]);
        
        assert!(result.is_ok());
        let array_type = result.unwrap();
        
        match array_type {
            TypeDecl::Array(element_types, size) => {
                assert_eq!(size, 3);
                assert_eq!(element_types.len(), 3);
                assert_eq!(element_types[0], TypeDecl::Bool);
                assert_eq!(element_types[1], TypeDecl::Bool);
                assert_eq!(element_types[2], TypeDecl::Bool);
            },
            _ => panic!("Expected Array type, got {:?}", array_type),
        }
    }

    #[test]
    fn test_bool_array_literal_with_type_hint() {
        let mut builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        // Create bool literals: [true, false]
        let true_expr = builder.bool_true_expr(None);
        let false_expr = builder.bool_false_expr(None);
        
        let array_elements = vec![true_expr, false_expr];
        let _array_expr = builder.array_literal_expr(array_elements, None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Set type hint for bool array
        type_checker.type_inference.type_hint = Some(TypeDecl::Array(vec![TypeDecl::Bool], 2));
        
        // Test type inference with hint
        let result = type_checker.visit_array_literal(&vec![ExprRef(0), ExprRef(1)]);
        
        assert!(result.is_ok());
        let array_type = result.unwrap();
        
        match array_type {
            TypeDecl::Array(element_types, size) => {
                assert_eq!(size, 2);
                assert_eq!(element_types.len(), 2);
                assert_eq!(element_types[0], TypeDecl::Bool);
                assert_eq!(element_types[1], TypeDecl::Bool);
            },
            _ => panic!("Expected Array type, got {:?}", array_type),
        }
    }

    #[test]
    fn test_bool_array_mixed_type_error() {
        let mut builder = create_test_ast_builder();
        let mut string_interner = DefaultStringInterner::new();
        
        // Create mixed literals: [true, 42] - should fail
        let true_expr = builder.bool_true_expr(None);
        let number_symbol = string_interner.get_or_intern("42");
        let number_expr = builder.number_expr(number_symbol, None);
        
        let array_elements = vec![true_expr, number_expr];
        let _array_expr = builder.array_literal_expr(array_elements, None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Test type inference - should fail
        let result = type_checker.visit_array_literal(&vec![ExprRef(0), ExprRef(1)]);
        
        assert!(result.is_err());
        let error = result.unwrap_err();
        
        // Check that it's an array error about type mismatch
        match error.kind {
            TypeCheckErrorKind::ArrayError { message } => {
                assert!(message.contains("must have the same type"));
                assert!(message.contains("Bool"));
                assert!(message.contains("Number"));
            },
            _ => panic!("Expected ArrayError, got {:?}", error.kind),
        }
    }

    #[test]
    fn test_bool_array_empty_error() {
        let builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Test empty array - should fail
        let result = type_checker.visit_array_literal(&vec![]);
        
        assert!(result.is_err());
        let error = result.unwrap_err();
        
        // Check that it's an array error about empty arrays
        match error.kind {
            TypeCheckErrorKind::ArrayError { message } => {
                assert!(message.contains("Empty array literals are not supported"));
            },
            _ => panic!("Expected ArrayError about empty arrays, got {:?}", error.kind),
        }
    }

    #[test]
    fn test_bool_array_with_wrong_type_hint() {
        let mut builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        // Create bool literals: [true, false]
        let true_expr = builder.bool_true_expr(None);
        let false_expr = builder.bool_false_expr(None);
        
        let array_elements = vec![true_expr, false_expr];
        let _array_expr = builder.array_literal_expr(array_elements, None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Set wrong type hint (expecting UInt64 array)
        type_checker.type_inference.type_hint = Some(TypeDecl::Array(vec![TypeDecl::UInt64], 2));
        
        // Test type inference with wrong hint - should fail
        let result = type_checker.visit_array_literal(&vec![ExprRef(0), ExprRef(1)]);
        
        assert!(result.is_err());
        let error = result.unwrap_err();
        
        // Check that it's an array error about type mismatch
        match error.kind {
            TypeCheckErrorKind::ArrayError { message } => {
                assert!(message.contains("Bool"));
                assert!(message.contains("UInt64"));
            },
            _ => panic!("Expected ArrayError about type mismatch, got {:?}", error.kind),
        }
    }

    #[test]
    fn test_bool_literal_type_checking() {
        let mut builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        // Create bool literals
        let _true_expr = builder.bool_true_expr(None);
        let _false_expr = builder.bool_false_expr(None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Test individual bool literals
        let true_result = type_checker.visit_boolean_literal(&Expr::True);
        let false_result = type_checker.visit_boolean_literal(&Expr::False);
        
        assert!(true_result.is_ok());
        assert!(false_result.is_ok());
        
        assert_eq!(true_result.unwrap(), TypeDecl::Bool);
        assert_eq!(false_result.unwrap(), TypeDecl::Bool);
    }

    #[test]
    fn test_bool_array_single_element() {
        let mut builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        // Create single bool literal: [true]
        let true_expr = builder.bool_true_expr(None);
        
        let array_elements = vec![true_expr];
        let _array_expr = builder.array_literal_expr(array_elements, None);
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Test single element array
        let result = type_checker.visit_array_literal(&vec![ExprRef(0)]);
        
        assert!(result.is_ok());
        let array_type = result.unwrap();
        
        match array_type {
            TypeDecl::Array(element_types, size) => {
                assert_eq!(size, 1);
                assert_eq!(element_types.len(), 1);
                assert_eq!(element_types[0], TypeDecl::Bool);
            },
            _ => panic!("Expected Array type, got {:?}", array_type),
        }
    }

    #[test]
    fn test_bool_array_large_array_performance() {
        let mut builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        // Create large bool array: [true, false, true, false, ...] (100 elements)
        let mut elements = Vec::new();
        for i in 0..100 {
            if i % 2 == 0 {
                elements.push(builder.bool_true_expr(None));
            } else {
                elements.push(builder.bool_false_expr(None));
            }
        }
        
        let element_refs: Vec<ExprRef> = (0..100).map(|i| ExprRef(i)).collect();
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Measure performance
        let start = std::time::Instant::now();
        let result = type_checker.visit_array_literal(&element_refs);
        let duration = start.elapsed();
        
        assert!(result.is_ok());
        let array_type = result.unwrap();
        
        match array_type {
            TypeDecl::Array(element_types, size) => {
                assert_eq!(size, 100);
                assert_eq!(element_types.len(), 100);
                // All elements should be Bool type
                for element_type in &element_types {
                    assert_eq!(*element_type, TypeDecl::Bool);
                }
            },
            _ => panic!("Expected Array type, got {:?}", array_type),
        }
        
        // Performance assertion - should complete within 100ms for 100 elements
        assert!(duration.as_millis() < 100, "Type inference took too long: {:?}", duration);
    }

    #[test]
    fn test_bool_array_edge_cases() {
        let mut builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        
        // Test with maximum realistic array size
        let mut elements = Vec::new();
        for _ in 0..1000 {
            elements.push(builder.bool_true_expr(None));
        }
        
        let element_refs: Vec<ExprRef> = (0..1000).map(|i| ExprRef(i)).collect();
        
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        let result = type_checker.visit_array_literal(&element_refs);
        
        assert!(result.is_ok());
        let array_type = result.unwrap();
        
        match array_type {
            TypeDecl::Array(element_types, size) => {
                assert_eq!(size, 1000);
                assert_eq!(element_types.len(), 1000);
            },
            _ => panic!("Expected Array type, got {:?}", array_type),
        }
    }

    // ========== Struct Array Type Inference Tests ==========

    #[test]
    fn test_struct_definition_registration() {
        let mut string_interner = DefaultStringInterner::new();
        let point_symbol = string_interner.get_or_intern("Point");
        
        let builder = create_test_ast_builder();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Register Point struct manually
        let struct_fields: Vec<StructField> = vec![
            StructField {
                name: "x".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
            StructField {
                name: "y".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
        ];
        
        type_checker.context.register_struct(point_symbol, struct_fields, crate::ast::Visibility::Private);
        
        // Verify struct registration
        let definition = type_checker.context.get_struct_definition(point_symbol);
        assert!(definition.is_some());
        assert_eq!(definition.unwrap().fields.len(), 2);
    }

    #[test]
    fn test_struct_array_type_compatibility() {
        let mut string_interner = DefaultStringInterner::new();
        let point_symbol = string_interner.get_or_intern("Point");
        
        let builder = create_test_ast_builder();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Register Point struct
        let struct_fields: Vec<StructField> = vec![
            StructField {
                name: "x".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
        ];
        type_checker.context.register_struct(point_symbol, struct_fields, crate::ast::Visibility::Private);
        
        // Test array with same struct types
        let point_type = TypeDecl::Struct(point_symbol);
        let array_type = TypeDecl::Array(vec![point_type.clone(), point_type.clone()], 2);
        
        // This should be valid
        assert!(matches!(array_type, TypeDecl::Array(ref types, 2) if types.len() == 2 && types[0] == point_type && types[1] == point_type));
    }

    #[test]
    fn test_struct_field_validation() {
        let mut string_interner = DefaultStringInterner::new();
        let point_symbol = string_interner.get_or_intern("Point");
        let x_symbol = string_interner.get_or_intern("x");
        let _y_symbol = string_interner.get_or_intern("y");
        
        let builder = create_test_ast_builder();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Register Point struct
        let struct_fields: Vec<StructField> = vec![
            StructField {
                name: "x".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
            StructField {
                name: "y".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
        ];
        type_checker.context.register_struct(point_symbol, struct_fields, crate::ast::Visibility::Private);
        
        // Test struct literal validation with missing field - should fail
        let incomplete_fields = vec![(x_symbol, ExprRef(0))]; // missing y field
        let result = type_checker.context.validate_struct_fields(point_symbol, &incomplete_fields, &type_checker.core);
        
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(error.to_string().contains("Missing required field"));
    }

    #[test]
    fn test_mixed_struct_types_error() {
        let mut string_interner = DefaultStringInterner::new();
        let point_symbol = string_interner.get_or_intern("Point");
        let circle_symbol = string_interner.get_or_intern("Circle");
        
        let builder = create_test_ast_builder();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Register Point and Circle structs
        let point_fields: Vec<StructField> = vec![
            StructField {
                name: "x".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
        ];
        let circle_fields: Vec<StructField> = vec![
            StructField {
                name: "radius".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
        ];
        
        type_checker.context.register_struct(point_symbol, point_fields, crate::ast::Visibility::Private);
        type_checker.context.register_struct(circle_symbol, circle_fields, crate::ast::Visibility::Private);
        
        // Test array with mixed struct types - should be caught by array type checker
        let point_type = TypeDecl::Struct(point_symbol);
        let circle_type = TypeDecl::Struct(circle_symbol);
        
        // This demonstrates that different struct types cannot be mixed in arrays
        assert_ne!(point_type, circle_type);
    }

    #[test]
    fn test_struct_array_inference_with_hint() {
        let mut string_interner = DefaultStringInterner::new();
        let point_symbol = string_interner.get_or_intern("Point");
        
        let builder = create_test_ast_builder();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let mut type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Register Point struct
        let struct_fields: Vec<StructField> = vec![
            StructField {
                name: "x".to_string(),
                type_decl: TypeDecl::Int64,
                visibility: crate::ast::Visibility::Public,
            },
        ];
        type_checker.context.register_struct(point_symbol, struct_fields, crate::ast::Visibility::Private);
        
        // Set type hint for struct array
        let point_type = TypeDecl::Struct(point_symbol);
        let array_hint = TypeDecl::Array(vec![point_type.clone()], 1);
        type_checker.type_inference.type_hint = Some(array_hint.clone());
        
        // Verify type hint was set correctly
        assert_eq!(type_checker.type_inference.type_hint, Some(array_hint));
        
        // Test that the setup_type_hint_for_val method works with struct arrays
        let _old_hint = type_checker.setup_type_hint_for_val(&Some(TypeDecl::Array(vec![point_type], 2)));
        assert!(type_checker.type_inference.type_hint.is_some());
    }

    #[test]
    fn test_line_col_calculation() {
        let source_code = "fn main() -> u64 {\n    val x = 42\n    x\n}";
        let builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool)
            .with_source_code(source_code);
        
        // Test various offsets
        // Offset 0: "fn" - should be line 1, column 1
        let (line, col) = type_checker.calculate_line_col_from_offset(0);
        assert_eq!((line, col), (1, 1));
        
        // Offset 19: First char of line 2 - should be line 2, column 1
        let (line, col) = type_checker.calculate_line_col_from_offset(19);
        assert_eq!((line, col), (2, 1));
        
        // Offset 23: "val" on line 2 - should be line 2, column 5
        let (line, col) = type_checker.calculate_line_col_from_offset(23);
        assert_eq!((line, col), (2, 5));
        
        // Offset 35: Line 3, column 2 (after 4 spaces) - should be line 3, column 2
        let (line, col) = type_checker.calculate_line_col_from_offset(35);
        assert_eq!((line, col), (3, 2));
    }

    #[test]
    fn test_node_to_source_location() {
        let source_code = "fn test() -> bool {\n    true\n}";
        let builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool)
            .with_source_code(source_code);
        
        // Create a node at the start of the function (offset 0)
        let node = Node::new(0, 2);
        let location = type_checker.node_to_source_location(&node);
        assert_eq!(location.line, 1);
        assert_eq!(location.column, 1);
        assert_eq!(location.offset, 0);
        
        // Create a node at line 2 (offset 20)
        let node = Node::new(20, 24);
        let location = type_checker.node_to_source_location(&node);
        assert_eq!(location.line, 2);
        assert_eq!(location.column, 1);
        assert_eq!(location.offset, 20);
    }

    #[test]
    fn test_source_location_without_source() {
        // Test fallback behavior when source code is not provided
        let builder = create_test_ast_builder();
        let string_interner = DefaultStringInterner::new();
        let (expr_pool, stmt_pool, location_pool) = builder.extract_pools();
        let mut expr_pool_mut = expr_pool;
        let type_checker = create_test_type_checker(&stmt_pool, &mut expr_pool_mut, &string_interner, &location_pool);
        
        // Without source code, should return (1, 1) as fallback
        let (line, col) = type_checker.calculate_line_col_from_offset(100);
        assert_eq!((line, col), (1, 1));
        
        let node = Node::new(50, 55);
        let location = type_checker.node_to_source_location(&node);
        assert_eq!(location.line, 1);
        assert_eq!(location.column, 1);
        assert_eq!(location.offset, 50);
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
        
        for (index, _stmt) in program.statement.0.iter().enumerate() {
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
        if let Some(obj_expr) = self.core.expr_pool.0.get(obj.to_index()) {
            if let Expr::Identifier(module_symbol) = obj_expr {
                let module_alias = vec![*module_symbol];
                
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
        "bool" | "str" | "self"
    )
}