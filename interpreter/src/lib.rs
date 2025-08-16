pub mod environment;
pub mod object;
pub mod evaluation;
pub mod error;
pub mod error_formatter;

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

/// Common setup for TypeCheckerVisitor with struct and impl registration
fn setup_type_checker(program: &mut Program) -> TypeCheckerVisitor {
    // First, collect and register struct definitions in the program's string_interner
    let mut struct_definitions = Vec::new();
    for stmt_ref in &program.statement.0 {
        if let frontend::ast::Stmt::StructDecl { name, fields, .. } = stmt_ref {
            struct_definitions.push((name.clone(), fields.clone()));
        }
    }
    
    // Register struct names in string_interner and collect symbols
    let mut struct_symbols_and_fields = Vec::new();
    for (name, fields) in struct_definitions {
        let struct_symbol = program.string_interner.get_or_intern(name);
        struct_symbols_and_fields.push((struct_symbol, fields));
    }

    // Now create the type checker
    let mut tc = TypeCheckerVisitor::new(&program.statement, &mut program.expression, &program.string_interner, &program.location_pool);

    // Register all defined functions
    program.function.iter().for_each(|f| { tc.add_function(f.clone()) });
    
    // Register struct definitions with their symbols
    for (struct_symbol, fields) in struct_symbols_and_fields {
        tc.context.register_struct(struct_symbol, fields);
    }

    tc
}

/// Setup TypeCheckerVisitor with module resolution support
fn setup_type_checker_with_modules(program: &mut Program) -> Result<TypeCheckerVisitor, Vec<String>> {
    let mut errors: Vec<String> = Vec::new();
    
    // Clone imports before creating TypeChecker to avoid borrowing conflicts
    let imports = program.imports.clone();
    
    // Check if program has imports that need resolution
    if !imports.is_empty() {
        // FIRST: Load and integrate all modules into the main program before creating TypeChecker
        for import in &imports {
            if let Err(err) = load_and_integrate_module(program, import) {
                errors.push(format!("Module integration error for {:?}: {}", import, err));
            }
        }
        
        if !errors.is_empty() {
            return Err(errors);
        }
        
        // NOW: Create TypeCheckerVisitor with integrated program using standard setup
        Ok(setup_type_checker(program))
    } else {
        // No imports, use standard setup
        Ok(setup_type_checker(program))
    }
}

/// Load and integrate a module directly into the main program before TypeChecker creation
fn load_and_integrate_module(program: &mut Program, import: &ImportDecl) -> Result<(), String> {
    // Simple module resolution: look for module files in modules/ directory
    let module_name = import.module_path.first()
        .and_then(|&symbol| program.string_interner.resolve(symbol))
        .ok_or("Invalid module path")?;
    
    // Construct module file path
    let module_file = format!("modules/{}/{}.t", module_name, module_name);
    eprintln!("Attempting to load module: {}", module_file);
    
    // Try to read and parse the module file
    match std::fs::read_to_string(&module_file) {
        Ok(source) => {
            eprintln!("Successfully read module file");
            
            // Parse module and integrate into main program
            integrate_module_into_program(&source, program)?;
            
            Ok(())
        }
        Err(err) => Err(format!("Failed to read module file {}: {}", module_file, err))
    }
}

/// Integrate a module's AST directly into the main program with symbol remapping
fn integrate_module_into_program(source: &str, main_program: &mut Program) -> Result<(), String> {
    // Parse the module with its own interner first
    let mut parser = frontend::ParserWithInterner::new(source);
    let module_program = parser.parse_program()
        .map_err(|e| format!("Parse error in module: {}", e))?;
    
    eprintln!("Successfully parsed module, {} functions found", module_program.function.len());
    
    // Integrate functions with symbol remapping
    for function in &module_program.function {
        // Get function name from module's interner
        if let Some(func_name_str) = module_program.string_interner.resolve(function.name) {
            // Create new symbol in main program's interner
            let new_function_symbol = main_program.string_interner.get_or_intern(func_name_str);
            
            // Create a new function with remapped symbols
            let mut new_function = function.as_ref().clone();
            new_function.name = new_function_symbol;
            
            // Remap parameter symbols
            for param in &mut new_function.parameter {
                if let Some(param_name_str) = module_program.string_interner.resolve(param.0) {
                    let new_param_symbol = main_program.string_interner.get_or_intern(param_name_str);
                    param.0 = new_param_symbol;
                }
            }
            
            // Add the remapped function to main program
            main_program.function.push(Rc::new(new_function));
            eprintln!("Integrated function: {} -> {:?}", func_name_str, new_function_symbol);
        }
    }
    
    // Integrate struct declarations (both name and field.name are Strings, no symbol remapping needed)
    for stmt_ref in &module_program.statement.0 {
        if let frontend::ast::Stmt::StructDecl { name, fields, visibility } = stmt_ref {
            // Create new struct declaration (no symbol remapping needed as all names are Strings)
            let new_struct_stmt = frontend::ast::Stmt::StructDecl {
                name: name.clone(),
                fields: fields.clone(),
                visibility: visibility.clone(),
            };
            
            // Add to main program's statements
            main_program.statement.0.push(new_struct_stmt);
            eprintln!("Integrated struct: {}", name);
        }
    }
    
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

pub fn check_typing(program: &mut Program, source_code: Option<&str>, filename: Option<&str>) -> Result<(), Vec<String>> {
    let mut errors: Vec<String> = vec![];
    
    // Extract data before setting up TypeChecker to avoid borrowing conflicts
    let functions = program.function.clone();
    let string_interner = program.string_interner.clone();
    let mut impl_blocks = Vec::new();
    for stmt_ref in &program.statement.0 {
        if let frontend::ast::Stmt::ImplBlock { target_type, methods } = stmt_ref {
            impl_blocks.push((target_type.clone(), methods.clone()));
        }
    }
    
    // Setup TypeChecker with module resolution support
    let mut tc = match setup_type_checker_with_modules(program) {
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
        let name = string_interner.resolve(func.name).unwrap_or("<NOT_FOUND>");
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

fn find_main_function(program: &Program) -> Result<Rc<Function>, InterpreterError> {
    let main_id = program.string_interner.get("main")
        .ok_or_else(|| InterpreterError::FunctionNotFound("main function symbol not found".to_string()))?;
    
    for func in &program.function {
        if func.name == main_id && func.parameter.is_empty() {
            return Ok(func.clone());
        }
    }
    
    Err(InterpreterError::FunctionNotFound("main".to_string()))
}

fn build_function_map(program: &Program) -> HashMap<DefaultSymbol, Rc<Function>> {
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
    
    for stmt_ref in &program.statement.0 {
        if let frontend::ast::Stmt::ImplBlock { target_type, methods } = stmt_ref {
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

pub fn execute_program(program: &Program, source_code: Option<&str>, filename: Option<&str>) -> Result<RcObject, String> {
    let main_function = match find_main_function(program) {
        Ok(func) => func,
        Err(e) => return Err(format!("Runtime Error: {e}")),
    };
    
    let func_map = build_function_map(program);
    let mut string_interner = program.string_interner.clone();
    let method_registry = build_method_registry(program, &mut string_interner);
    
    let mut eval = EvaluationContext::new(
        &program.statement, 
        &program.expression, 
        &mut string_interner, 
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