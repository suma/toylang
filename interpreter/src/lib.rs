pub mod environment;
pub mod object;
pub mod evaluation;
pub mod error;

use std::rc::Rc;
use std::collections::HashMap;
use frontend;
use frontend::ast::*;
use frontend::type_checker::*;
use crate::object::RcObject;
use crate::evaluation::EvaluationContext;
use crate::error::InterpreterError;

pub fn check_typing(program: &mut Program) -> Result<(), Vec<String>> {
    let mut errors: Vec<String> = vec![];
    let mut tc = TypeCheckerVisitor::new(&program.statement, &mut program.expression, &program.string_interner);

    // Register all defined functions
    program.function.iter().for_each(|f| { tc.add_function(f.clone()) });

    program.function.iter().for_each(|func| {
        let name = program.string_interner.resolve(func.name).unwrap_or("<NOT_FOUND>");
        println!("Checking function {}", name);
        let r = tc.type_check(func.clone());
        if r.is_err() {
            errors.push(format!("type_check failed in {}: {}", name, r.unwrap_err()));
        }
    });

    if errors.len() == 0 {
        Ok(())
    } else {
        Err(errors)
    }
}

pub fn execute_program(program: &Program) -> Result<RcObject, InterpreterError> {
    let mut main: Option<Rc<Function>> = None;
    let main_id = program.string_interner.get("main").unwrap();
    program.function.iter().for_each(|func| {
        if func.name == main_id && func.parameter.is_empty() {
            main = Some(func.clone());
        }
    });

    if main.is_some() {
        let mut func = HashMap::new();
        let mut string_interner = program.string_interner.clone();
        for f in &program.function {
            func.insert(f.name.clone(), f.clone());
        }

        let mut eval = EvaluationContext::new(&program.statement, &program.expression, &mut string_interner, func);
        let no_args = vec![];
        eval.evaluate_function(main.unwrap(), &no_args)
    } else {
        Err(InterpreterError::FunctionNotFound("main".to_string()))
    }
}