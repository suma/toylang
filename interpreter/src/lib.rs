pub mod environment;
pub mod object;
pub mod evaluation;
pub mod error;
pub mod error_formatter;
pub mod heap;

use std::rc::Rc;
use std::collections::HashMap;
use frontend::ast::*;
use frontend::type_checker::*;
use frontend::visitor::AstVisitor;
use string_interner::{DefaultSymbol, DefaultStringInterner};
use crate::object::RcObject;
use crate::evaluation::EvaluationContext;
use crate::error::InterpreterError;
use crate::error_formatter::ErrorFormatter;

/// Context for AST integration between modules and main program
struct AstIntegrationContext<'a> {
    main_program: &'a mut Program,
    module_program: &'a Program,
    main_string_interner: &'a mut DefaultStringInterner,
    module_string_interner: &'a DefaultStringInterner,
    expr_mapping: HashMap<u32, ExprRef>, // module ExprRef -> main ExprRef
    stmt_mapping: HashMap<u32, StmtRef>, // module StmtRef -> main StmtRef
}

impl<'a> AstIntegrationContext<'a> {
    fn new(
        main_program: &'a mut Program,
        module_program: &'a Program,
        main_string_interner: &'a mut DefaultStringInterner,
        module_string_interner: &'a DefaultStringInterner,
    ) -> Self {
        Self {
            main_program,
            module_program,
            main_string_interner,
            module_string_interner,
            expr_mapping: HashMap::new(),
            stmt_mapping: HashMap::new(),
        }
    }
    
    
    /// Remap expression with updated references to main program's AST pools
    fn remap_expression(&mut self, expr: &Expr) -> Result<Expr, String> {
        match expr {
            // Literals need no remapping
            Expr::True | Expr::False | Expr::Null => Ok(expr.clone()),
            Expr::Int64(v) => Ok(Expr::Int64(*v)),
            Expr::UInt64(v) => Ok(Expr::UInt64(*v)),
            Expr::Number(symbol) => {
                // Remap symbol to main program's string interner
                let symbol_str = self.module_string_interner.resolve(*symbol)
                    .ok_or("Cannot resolve Number symbol")?;
                let new_symbol = self.main_string_interner.get_or_intern(symbol_str);
                Ok(Expr::Number(new_symbol))
            }
            Expr::String(symbol) => {
                // Remap symbol to main program's string interner  
                let symbol_str = self.module_string_interner.resolve(*symbol)
                    .ok_or("Cannot resolve String symbol")?;
                let new_symbol = self.main_string_interner.get_or_intern(symbol_str);
                Ok(Expr::String(new_symbol))
            }
            Expr::Identifier(symbol) => {
                // Remap symbol to main program's string interner
                let symbol_str = self.module_string_interner.resolve(*symbol)
                    .ok_or("Cannot resolve Identifier symbol")?;
                let new_symbol = self.main_string_interner.get_or_intern(symbol_str);
                Ok(Expr::Identifier(new_symbol))
            }
            Expr::Binary(op, lhs, rhs) => {
                let new_lhs = self.expr_mapping.get(&lhs.0)
                    .ok_or("Cannot find LHS expression mapping")?.clone();
                let new_rhs = self.expr_mapping.get(&rhs.0)
                    .ok_or("Cannot find RHS expression mapping")?.clone();
                Ok(Expr::Binary(op.clone(), new_lhs, new_rhs))
            }
            Expr::Call(symbol, args) => {
                // Remap function name symbol
                let symbol_str = self.module_string_interner.resolve(*symbol)
                    .ok_or("Cannot resolve Call symbol")?;
                let new_symbol = self.main_string_interner.get_or_intern(symbol_str);
                
                // Remap arguments expression reference
                let new_args = self.expr_mapping.get(&args.0)
                    .ok_or("Cannot find Call args expression mapping")?.clone();
                Ok(Expr::Call(new_symbol, new_args))
            }
            Expr::ExprList(exprs) => {
                let mut new_exprs = Vec::new();
                for expr_ref in exprs {
                    let new_expr_ref = self.expr_mapping.get(&expr_ref.0)
                        .ok_or("Cannot find ExprList expression mapping")?.clone();
                    new_exprs.push(new_expr_ref);
                }
                Ok(Expr::ExprList(new_exprs))
            }
            Expr::Block(stmts) => {
                let mut new_stmts = Vec::new();
                for stmt_ref in stmts {
                    let new_stmt_ref = self.stmt_mapping.get(&stmt_ref.0)
                        .ok_or_else(|| {
                            eprintln!("DEBUG: Cannot find statement mapping for StmtRef({})", stmt_ref.0);
                            eprintln!("DEBUG: Available stmt_mappings: {:?}", self.stmt_mapping.keys().collect::<Vec<_>>());
                            eprintln!("DEBUG: Module has {} statements, main has {} statements", 
                                self.module_program.statement.len(),
                                self.main_program.statement.len());
                            format!("Cannot find Block statement mapping for StmtRef({})", stmt_ref.0)
                        })?.clone();
                    new_stmts.push(new_stmt_ref);
                }
                Ok(Expr::Block(new_stmts))
            }
            Expr::Assign(lhs, rhs) => {
                let new_lhs = self.expr_mapping.get(&lhs.0)
                    .ok_or("Cannot find Assign LHS expression mapping")?.clone();
                let new_rhs = self.expr_mapping.get(&rhs.0)
                    .ok_or("Cannot find Assign RHS expression mapping")?.clone();
                Ok(Expr::Assign(new_lhs, new_rhs))
            }
            Expr::IfElifElse(if_cond, if_block, elif_pairs, else_block) => {
                let new_if_cond = self.expr_mapping.get(&if_cond.0)
                    .ok_or("Cannot find IfElifElse condition expression mapping")?.clone();
                let new_if_block = self.expr_mapping.get(&if_block.0)
                    .ok_or("Cannot find IfElifElse if_block expression mapping")?.clone();
                
                let mut new_elif_pairs = Vec::new();
                for (elif_cond, elif_block) in elif_pairs {
                    let new_elif_cond = self.expr_mapping.get(&elif_cond.0)
                        .ok_or("Cannot find IfElifElse elif_cond expression mapping")?.clone();
                    let new_elif_block = self.expr_mapping.get(&elif_block.0)
                        .ok_or("Cannot find IfElifElse elif_block expression mapping")?.clone();
                    new_elif_pairs.push((new_elif_cond, new_elif_block));
                }
                
                let new_else_block = self.expr_mapping.get(&else_block.0)
                    .ok_or("Cannot find IfElifElse else_block expression mapping")?.clone();
                
                Ok(Expr::IfElifElse(new_if_cond, new_if_block, new_elif_pairs, new_else_block))
            }
            Expr::QualifiedIdentifier(path) => {
                // Remap all symbols in the qualified identifier path
                let mut new_path = Vec::new();
                for symbol in path {
                    let new_symbol = self.remap_symbol(*symbol)?;
                    new_path.push(new_symbol);
                }
                Ok(Expr::QualifiedIdentifier(new_path))
            }
            // Add other expression types as needed
            _ => Err(format!("Unsupported expression type for remapping: {:?}", expr))
        }
    }
    
    /// Remap statement with updated references to main program's AST pools
    fn remap_statement(&mut self, stmt: &Stmt) -> Result<Stmt, String> {
        match stmt {
            Stmt::Expression(expr_ref) => {
                let new_expr_ref = self.expr_mapping.get(&expr_ref.0)
                    .ok_or("Cannot find Expression statement mapping")?.clone();
                Ok(Stmt::Expression(new_expr_ref))
            }
            Stmt::Return(Some(expr_ref)) => {
                let new_expr_ref = self.expr_mapping.get(&expr_ref.0)
                    .ok_or("Cannot find Return expression mapping")?.clone();
                Ok(Stmt::Return(Some(new_expr_ref)))
            }
            Stmt::Return(None) => Ok(Stmt::Return(None)),
            Stmt::Break => Ok(Stmt::Break),
            Stmt::Continue => Ok(Stmt::Continue),
            Stmt::Var(name, typ, value) => {
                let new_name = self.remap_symbol(*name)?;
                let new_value = if let Some(expr_ref) = value {
                    let new_expr_ref = self.expr_mapping.get(&expr_ref.0)
                        .ok_or("Cannot find Var value expression mapping")?.clone();
                    Some(new_expr_ref)
                } else {
                    None
                };
                Ok(Stmt::Var(new_name, typ.clone(), new_value))
            }
            Stmt::Val(name, typ, value) => {
                let new_name = self.remap_symbol(*name)?;
                let new_value = self.expr_mapping.get(&value.0)
                    .ok_or("Cannot find Val value expression mapping")?.clone();
                Ok(Stmt::Val(new_name, typ.clone(), new_value))
            }
            Stmt::For(variable, start, end, body) => {
                let new_variable = self.remap_symbol(*variable)?;
                let new_start = self.expr_mapping.get(&start.0)
                    .ok_or("Cannot find For start expression mapping")?.clone();
                let new_end = self.expr_mapping.get(&end.0)
                    .ok_or("Cannot find For end expression mapping")?.clone();
                let new_body = self.expr_mapping.get(&body.0)
                    .ok_or("Cannot find For body expression mapping")?.clone();
                Ok(Stmt::For(new_variable, new_start, new_end, new_body))
            }
            Stmt::While(condition, body) => {
                let new_condition = self.expr_mapping.get(&condition.0)
                    .ok_or("Cannot find While condition expression mapping")?.clone();
                let new_body = self.expr_mapping.get(&body.0)
                    .ok_or("Cannot find While body expression mapping")?.clone();
                Ok(Stmt::While(new_condition, new_body))
            }
            // StructDecl and ImplBlock statements - preserve as string-based (no symbol remapping needed)
            Stmt::StructDecl { name, fields, visibility } => {
                Ok(Stmt::StructDecl {
                    name: name.clone(),
                    fields: fields.clone(),
                    visibility: visibility.clone()
                })
            }
            Stmt::ImplBlock { target_type, methods } => {
                // MethodFunction symbols need remapping
                let mut new_methods = Vec::new();
                for method in methods {
                    let new_method = self.remap_method_function(method)?;
                    new_methods.push(new_method);
                }
                Ok(Stmt::ImplBlock {
                    target_type: target_type.clone(),
                    methods: new_methods
                })
            }
        }
    }
    
    /// Remap a symbol from module to main program's string interner
    fn remap_symbol(&mut self, symbol: DefaultSymbol) -> Result<DefaultSymbol, String> {
        let symbol_str = self.module_string_interner.resolve(symbol)
            .ok_or("Cannot resolve symbol")?;
        Ok(self.main_string_interner.get_or_intern(symbol_str))
    }
    
    /// Remap a function with all its symbols and AST references
    fn remap_function(&mut self, function: &Function) -> Result<Function, String> {
        let new_name = self.remap_symbol(function.name)?;
        
        // Remap parameters
        let mut new_parameters = Vec::new();
        for (param_symbol, param_type) in &function.parameter {
            let new_param_symbol = self.remap_symbol(*param_symbol)?;
            new_parameters.push((new_param_symbol, param_type.clone()));
        }
        
        // Remap function body statement reference
        let new_code = self.stmt_mapping.get(&function.code.0)
            .ok_or("Cannot find function code statement mapping")?.clone();
        
        Ok(Function {
            node: function.node.clone(),
            name: new_name,
            parameter: new_parameters,
            return_type: function.return_type.clone(),
            code: new_code,
            visibility: function.visibility.clone()
        })
    }
    
    /// Remap a method function with all its symbols and AST references
    fn remap_method_function(&mut self, method: &MethodFunction) -> Result<Rc<MethodFunction>, String> {
        let new_name = self.remap_symbol(method.name)?;
        
        // Remap parameters
        let mut new_parameters = Vec::new();
        for (param_symbol, param_type) in &method.parameter {
            let new_param_symbol = self.remap_symbol(*param_symbol)?;
            new_parameters.push((new_param_symbol, param_type.clone()));
        }
        
        // Remap method body statement reference
        let new_code = self.stmt_mapping.get(&method.code.0)
            .ok_or("Cannot find method code statement mapping")?.clone();
        
        Ok(Rc::new(MethodFunction {
            node: method.node.clone(),
            name: new_name,
            parameter: new_parameters,
            return_type: method.return_type.clone(),
            code: new_code,
            has_self_param: method.has_self_param,
            visibility: method.visibility.clone()
        }))
    }
    
    /// Copy struct declarations from module to main program
    fn copy_struct_declarations(&mut self) -> Result<(), String> {
        for i in 0..self.module_program.statement.len() {
            let stmt_ref = StmtRef(i as u32);
            if let Some(stmt) = self.module_program.statement.get(&stmt_ref) {
                if let Stmt::StructDecl { name, fields, visibility } = stmt {
                    // StructDecl uses String names, no symbol remapping needed
                    let new_struct_stmt = Stmt::StructDecl {
                        name: name.clone(),
                        fields: fields.clone(),
                        visibility: visibility.clone()
                    };
                    self.main_program.statement.add(new_struct_stmt);
                }
            }
        }
        Ok(())
    }
    
    /// Copy functions from module to main program with proper AST integration
    fn copy_functions(&mut self) -> Result<Vec<Rc<Function>>, String> {
        let mut integrated_functions = Vec::new();
        
        for function in &self.module_program.function {
            let new_function = self.remap_function(function)?;
            integrated_functions.push(Rc::new(new_function));
        }
        
        Ok(integrated_functions)
    }
    
    /// Complete AST integration process using three-phase approach to handle circular dependencies
    fn integrate(&mut self) -> Result<Vec<Rc<Function>>, String> {
        eprintln!("AST Integration: Starting three-phase integration...");
        
        // Phase 1: Create placeholder mappings for all AST nodes
        self.create_placeholder_mappings()?;
        eprintln!("AST Integration: Created placeholders ({} expressions, {} statements)", 
            self.expr_mapping.len(), self.stmt_mapping.len());
        
        // Phase 2: Replace placeholders with actual remapped content
        self.update_with_remapped_content()?;
        eprintln!("AST Integration: Updated with remapped content");
        
        // Phase 3: Copy struct declarations and functions
        self.copy_struct_declarations()?;
        let integrated_functions = self.copy_functions()?;
        eprintln!("AST Integration: Integrated {} functions", integrated_functions.len());
        
        Ok(integrated_functions)
    }
    
    /// Phase 1: Create placeholder mappings for all expressions and statements
    fn create_placeholder_mappings(&mut self) -> Result<(), String> {
        // Create placeholder mappings for all expressions
        for index in 0..self.module_program.expression.len() {
            let placeholder_expr = Expr::Null;
            let main_expr_ref = self.main_program.expression.add(placeholder_expr);
            self.expr_mapping.insert(index as u32, main_expr_ref);
        }
        
        // Create placeholder mappings for all statements
        for index in 0..self.module_program.statement.len() {
            let placeholder_stmt = Stmt::Break;
            let main_stmt_ref = self.main_program.statement.add(placeholder_stmt);
            self.stmt_mapping.insert(index as u32, main_stmt_ref);
        }
        
        Ok(())
    }
    
    /// Phase 2: Replace placeholders with actual remapped content
    fn update_with_remapped_content(&mut self) -> Result<(), String> {
        // Pool structures don't support direct element replacement
        // We need a different approach - rebuild with correct mappings
        // For now, create new Pool structures with remapped content
        let _new_expr_pool = ExprPool::new();
        let _new_stmt_pool = StmtPool::new();
        
        // Copy existing content from main program first
        // This is a placeholder implementation - needs proper solution
        // Update all expressions with correct content
        for index in 0..self.module_program.expression.len() {
            let expr_ref = ExprRef(index as u32);
            if let Some(expr) = self.module_program.expression.get(&expr_ref) {
                let _remapped_expr = self.remap_expression(&expr)?;
                let _main_expr_ref = self.expr_mapping.get(&(index as u32)).unwrap().clone();
                // TODO: Need to implement proper Pool update mechanism
            }
        }
        
        // Update all statements with correct content
        for index in 0..self.module_program.statement.len() {
            let stmt_ref = StmtRef(index as u32);
            if let Some(stmt) = self.module_program.statement.get(&stmt_ref) {
                let _remapped_stmt = self.remap_statement(&stmt)?;
                let _main_stmt_ref = self.stmt_mapping.get(&(index as u32)).unwrap().clone();
                // TODO: Need to implement proper Pool update mechanism
            }
        }
        
        Ok(())
    }
}

/// Common setup for TypeCheckerVisitor with struct and impl registration
fn setup_type_checker<'a>(program: &'a mut Program, string_interner: &'a mut DefaultStringInterner) -> TypeCheckerVisitor<'a> {
    // First, collect and register struct definitions
    let mut struct_definitions = Vec::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(stmt) = program.statement.get(&stmt_ref) {
            if let frontend::ast::Stmt::StructDecl { name, fields, visibility } = &stmt {
                struct_definitions.push((name.clone(), fields.clone(), visibility.clone()));
            }
        }
    }
    
    // Register struct names in string_interner and collect symbols
    let mut struct_symbols_and_fields = Vec::new();
    for (name, fields, visibility) in struct_definitions {
        let struct_symbol = string_interner.get_or_intern(name);
        struct_symbols_and_fields.push((struct_symbol, fields, visibility));
    }

    // Register all defined functions before creating the type checker
    let functions_to_register: Vec<_> = program.function.iter().cloned().collect();

    // Now create the type checker
    let mut tc = TypeCheckerVisitor::with_program(program, string_interner);

    // Register all defined functions
    functions_to_register.iter().for_each(|f| { tc.add_function(f.clone()) });
    
    // Register struct definitions with their symbols
    for (struct_symbol, fields, visibility) in struct_symbols_and_fields {
        tc.context.register_struct(struct_symbol, fields, visibility);
    }

    tc
}

/// Setup TypeCheckerVisitor with module resolution support
fn setup_type_checker_with_modules<'a>(program: &'a mut Program, string_interner: &'a mut DefaultStringInterner) -> Result<TypeCheckerVisitor<'a>, Vec<String>> {
    let mut errors: Vec<String> = Vec::new();
    
    // Clone imports before creating TypeChecker to avoid borrowing conflicts
    let imports = program.imports.clone();
    
    // Check if program has imports that need resolution
    if !imports.is_empty() {
        eprintln!("Setting up TypeChecker with module resolution for {} imports", imports.len());
        
        // Load and integrate each imported module
        for import in &imports {
            if let Err(err) = load_and_integrate_module(program, import, string_interner) {
                errors.push(format!("Module integration error: {}", err));
            }
        }
        
        // If there were module loading errors, return them
        if !errors.is_empty() {
            return Err(errors);
        }
        
        // Create TypeChecker with integrated modules
        Ok(setup_type_checker(program, string_interner))
    } else {
        // No imports, use standard setup
        Ok(setup_type_checker(program, string_interner))
    }
}

/// Load and integrate a module directly into the main program before TypeChecker creation
fn load_and_integrate_module(program: &mut Program, import: &ImportDecl, string_interner: &mut DefaultStringInterner) -> Result<(), String> {
    // Simple module resolution: look for module files in modules/ directory
    let module_name = import.module_path.first()
        .and_then(|&symbol| string_interner.resolve(symbol))
        .ok_or("Invalid module path")?;
    
    // Construct module file path
    let module_file = format!("modules/{}/{}.t", module_name, module_name);
    eprintln!("Attempting to load module: {}", module_file);
    
    // Try to read and parse the module file
    match std::fs::read_to_string(&module_file) {
        Ok(source) => {
            eprintln!("Successfully read module file");
            
            // Parse module and integrate into main program
            integrate_module_into_program(&source, program, string_interner)?;
            
            Ok(())
        }
        Err(err) => Err(format!("Failed to read module file {}: {}", module_file, err))
    }
}

/// Integrate module into main program using comprehensive AST deep-copy
pub fn integrate_module_into_program(
    source: &str, 
    main_program: &mut Program, 
    main_string_interner: &mut DefaultStringInterner
) -> Result<(), String> {
    eprintln!("Starting AST-based module integration...");
    
    // Parse the module with its own interner
    let mut parser = frontend::ParserWithInterner::new(source);
    let module_program = parser.parse_program()
        .map_err(|e| format!("Parse error in module: {}", e))?;
    
    // Get the module's string interner
    let module_string_interner = parser.get_string_interner();
    
    eprintln!("Successfully parsed module: {} functions, {} expressions, {} statements", 
        module_program.function.len(),
        module_program.expression.len(),
        module_program.statement.len()
    );
    
    // Create AST integration context with both string interners
    let mut integration_context = AstIntegrationContext::new(
        main_program, 
        &module_program,
        main_string_interner,
        module_string_interner
    );
    
    // Perform complete AST integration
    let integrated_functions = integration_context.integrate()?;
    
    // Add integrated functions to main program
    for function in integrated_functions {
        let func_name = main_string_interner.resolve(function.name).unwrap_or("<unknown>");
        eprintln!("Successfully integrated function: {}", func_name);
        main_program.function.push(function);
    }
    
    eprintln!("AST-based module integration completed successfully");
    Ok(())
}

/// Process impl blocks and collect errors (extracted data version to avoid borrowing conflicts)
fn process_impl_blocks_extracted(
    tc: &mut TypeCheckerVisitor,
    impl_blocks: &[(String, Vec<std::rc::Rc<MethodFunction>>)],
    formatter: &Option<ErrorFormatter>
) -> Vec<String> {
    let mut errors = Vec::new();

    for (target_type, methods) in impl_blocks {
        if let Err(err) = tc.visit_impl_block(target_type, methods) {
            let formatted_error = if let Some(ref fmt) = formatter {
                fmt.format_type_check_error(&err)
            } else {
                format!("Impl block error for {target_type}: {err}")
            };
            errors.push(formatted_error);
        }
    }

    errors
}

pub fn check_typing(
    program: &mut Program, 
    string_interner: &mut DefaultStringInterner,
    source_code: Option<&str>, 
    filename: Option<&str>
) -> Result<(), Vec<String>> {
    let mut errors: Vec<String> = vec![];
    
    // Clone string_interner for later use
    let string_interner_for_names = string_interner.clone();
    
    // Extract data before setting up TypeChecker to avoid borrowing conflicts
    let functions = program.function.clone();
    let mut impl_blocks = Vec::new();
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(stmt) = program.statement.get(&stmt_ref) {
            if let frontend::ast::Stmt::ImplBlock { target_type, methods } = &stmt {
                impl_blocks.push((target_type.clone(), methods.clone()));
            }
        }
    }
    
    // Setup TypeChecker with module resolution support
    let mut tc = match setup_type_checker_with_modules(program, string_interner) {
        Ok(tc) => tc,
        Err(module_errors) => {
            errors.extend(module_errors);
            return Err(errors);
        }
    };

    // Create error formatter if we have source code and filename
    let formatter = if let (Some(source), Some(file)) = (source_code, filename) {
        Some(ErrorFormatter::new(source, file))
    } else {
        None
    };

    // Process impl blocks and collect errors
    errors.extend(process_impl_blocks_extracted(&mut tc, &impl_blocks, &formatter));

    // Process functions
    functions.iter().for_each(|func| {
        let name = string_interner_for_names.resolve(func.name).unwrap_or("<NOT_FOUND>");
        // Commented out for performance benchmarking
        // println!("Checking function {}", name);
        let r = tc.type_check(func.clone());
        if let Err(mut error) = r {
            
            // Add source location information if available
            if let (Some(source), Some(location)) = (source_code, error.location.as_ref()) {
                // Calculate line and column from source
                let (line, column) = calculate_line_col_from_offset(source, location.offset as usize);
                error.location = Some(frontend::type_checker::SourceLocation {
                    line,
                    column,
                    offset: location.offset,
                });
            }
            
            // Use formatter if available, otherwise fallback to simple format
            let formatted_error = if let Some(ref fmt) = formatter {
                fmt.format_type_check_error(&error)
            } else {
                format!("type_check failed in {name}: {error}")
            };
            
            errors.push(formatted_error);
        }
    });

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}


fn calculate_line_col_from_offset(source: &str, offset: usize) -> (u32, u32) {
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
}

fn find_main_function(program: &Program, string_interner: &DefaultStringInterner) -> Result<Rc<Function>, InterpreterError> {
    let main_id = string_interner.get("main")
        .ok_or_else(|| InterpreterError::FunctionNotFound("main function symbol not found".to_string()))?;
    
    for func in &program.function {
        if func.name == main_id && func.parameter.is_empty() {
            return Ok(func.clone());
        }
    }
    
    Err(InterpreterError::FunctionNotFound("main".to_string()))
}

fn build_function_map(program: &Program, _string_interner: &DefaultStringInterner) -> HashMap<DefaultSymbol, Rc<Function>> {
    let mut func_map = HashMap::new();
    for f in &program.function {
        func_map.insert(f.name, f.clone());
    }
    func_map
}

/// Initialize module environment based on package and import declarations
fn initialize_module_environment(eval: &mut EvaluationContext, program: &Program) {
    // Set current module from package declaration
    if let Some(package_decl) = &program.package_decl {
        eval.environment.set_current_module(Some(package_decl.name.clone()));
        eval.environment.register_module(package_decl.name.clone());
    }
    
    // Register imported modules
    for import_decl in &program.imports {
        eval.environment.register_module(import_decl.module_path.clone());
    }
    
    // Note: Actual module loading and variable population would happen here
    // For now, we just register the module namespaces
}

fn build_method_registry(
    program: &Program, 
    string_interner: &mut DefaultStringInterner
) -> HashMap<DefaultSymbol, HashMap<DefaultSymbol, Rc<MethodFunction>>> {
    let mut method_registry = HashMap::new();
    
    for i in 0..program.statement.len() {
        let stmt_ref = StmtRef(i as u32);
        if let Some(stmt) = program.statement.get(&stmt_ref) {
            if let frontend::ast::Stmt::ImplBlock { target_type, methods } = &stmt {
                let struct_name_symbol = string_interner.get_or_intern(target_type.clone());
                for method in methods {
                    let method_name_symbol = method.name;
                    method_registry
                        .entry(struct_name_symbol)
                        .or_insert_with(HashMap::new)
                        .insert(method_name_symbol, method.clone());
                }
            }
        }
    }
    
    method_registry
}

fn register_methods(
    eval: &mut EvaluationContext,
    method_registry: HashMap<DefaultSymbol, HashMap<DefaultSymbol, Rc<MethodFunction>>>
) {
    for (struct_symbol, methods) in method_registry {
        for (method_symbol, method_func) in methods {
            eval.register_method(struct_symbol, method_symbol, method_func);
        }
    }
}

pub fn execute_program(program: &Program, string_interner: &DefaultStringInterner, source_code: Option<&str>, filename: Option<&str>) -> Result<RcObject, String> {
    let main_function = match find_main_function(program, string_interner) {
        Ok(func) => func,
        Err(e) => return Err(format!("Runtime Error: {e}")),
    };
    
    let func_map = build_function_map(program, string_interner);
    let mut string_interner_mut = string_interner.clone();
    let method_registry = build_method_registry(program, &mut string_interner_mut);
    
    let mut eval = EvaluationContext::new(
        &program.statement, 
        &program.expression, 
        &mut string_interner_mut, 
        func_map
    );
    
    // Initialize module system
    initialize_module_environment(&mut eval, program);
    
    register_methods(&mut eval, method_registry);
    
    let no_args = vec![];
    match eval.evaluate_function(main_function, &no_args) {
        Ok(result) => Ok(result),
        Err(runtime_error) => {
            // Format runtime error with source location if available
            let formatted_error = if let (Some(source), Some(file)) = (source_code, filename) {
                let formatter = ErrorFormatter::new(source, file);
                // Try to extract location from runtime error if possible
                formatter.format_runtime_error(&runtime_error.to_string(), None)
            } else {
                format!("Runtime Error: {runtime_error}")
            };
            Err(formatted_error)
        }
    }
}