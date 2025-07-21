pub mod environment;
pub mod object;
pub mod evaluation;
pub mod error;

use std::rc::Rc;
use std::collections::HashMap;
use frontend;
use frontend::ast::*;
use frontend::type_checker::*;
use string_interner::{DefaultSymbol, DefaultStringInterner};
use crate::object::RcObject;
use crate::evaluation::EvaluationContext;
use crate::error::InterpreterError;

pub fn check_typing(program: &mut Program, source_code: Option<&str>) -> Result<(), Vec<String>> {
    let mut errors: Vec<String> = vec![];
    let mut tc = TypeCheckerVisitor::new(&program.statement, &mut program.expression, &program.string_interner, &program.location_pool);

    // Register all defined functions
    program.function.iter().for_each(|f| { tc.add_function(f.clone()) });

    program.function.iter().for_each(|func| {
        let name = program.string_interner.resolve(func.name).unwrap_or("<NOT_FOUND>");
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
            
            errors.push(format!("type_check failed in {}: {}", name, error));
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
    let main_id = program.string_interner.get("main").unwrap();
    
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

pub fn execute_program(program: &Program) -> Result<RcObject, InterpreterError> {
    let main_function = find_main_function(program)?;
    
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
    eval.evaluate_function(main_function, &no_args)
}