pub mod environment;
pub mod object;
pub mod evaluation;
pub mod error;
pub mod error_formatter;

use std::rc::Rc;
use std::collections::HashMap;
use frontend;
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
        match stmt_ref {
            frontend::ast::Stmt::StructDecl { name, fields } => {
                struct_definitions.push((name.clone(), fields.clone()));
            }
            _ => {}
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
                format!("Impl block error for {}: {}", target_type, err)
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
    
    // Setup TypeChecker with struct and function registration
    let mut tc = setup_type_checker(program);

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
        if r.is_err() {
            let mut error = r.unwrap_err();
            
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
                format!("type_check failed in {}: {}", name, error)
            };
            
            errors.push(formatted_error);
        }
    });

    if errors.len() == 0 {
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
        Err(e) => return Err(format!("Runtime Error: {}", e)),
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
                format!("Runtime Error: {}", runtime_error)
            };
            Err(formatted_error)
        }
    }
}