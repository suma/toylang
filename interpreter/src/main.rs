use std::env;
use std::fs;
use interpreter::error_formatter::ErrorFormatter;

/// Parse the source file and handle parse errors
fn handle_parsing(source: &str, filename: &str) -> Result<frontend::ast::Program, ()> {
    let mut parser = frontend::Parser::new(source);
    let program = parser.parse_program();
    let formatter = ErrorFormatter::new(source, filename);
    
    // Handle parse errors using unified error display
    if !parser.errors.is_empty() {
        formatter.display_parse_errors(&parser.errors);
        return Err(());
    }
    
    match program {
        Ok(prog) => Ok(prog),
        Err(err) => {
            formatter.format_parse_error(&err);
            Err(())
        }
    }
}

/// Perform type checking and handle type check errors
fn handle_type_checking(program: &mut frontend::ast::Program, source: &str, filename: &str) -> Result<(), ()> {
    let formatter = ErrorFormatter::new(source, filename);
    
    match interpreter::check_typing(program, Some(source), Some(filename)) {
        Ok(()) => Ok(()),
        Err(errors) => {
            formatter.display_type_check_errors(&errors);
            Err(())
        }
    }
}

/// Execute the program and handle runtime errors
fn handle_execution(program: &frontend::ast::Program, source: &str, filename: &str) -> Result<(), ()> {
    let formatter = ErrorFormatter::new(source, filename);
    
    match interpreter::execute_program(program, Some(source), Some(filename)) {
        Ok(result) => {
            println!("Result: {result:?}");
            Ok(())
        }
        Err(error) => {
            formatter.display_runtime_error(&error);
            Err(())
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let verbose = env::args().any(|arg| arg == "-v");
    if args.len() != 2 && !verbose {
        println!("Usage:");
        println!("  {} <file>", args[0]);
        println!("  {} <file> -v", args[0]);
        return;
    }

    if verbose {
        println!("Reading file {}", args[1]);
    }
    let file = fs::read_to_string(&args[1]).expect("Failed to read file");
    let source = file.as_str();
    let filename = args[1].as_str();
    
    // Parse the source file
    if verbose {
        println!("Parsing source file");
    }
    let mut program = match handle_parsing(source, filename) {
        Ok(prog) => prog,
        Err(()) => return,
    };
    
    // Perform type checking
    if verbose {
        println!("Performing type checking");
    }
    if handle_type_checking(&mut program, source, filename).is_err() {
        return;
    }
    
    // Execute the program
    if verbose {
        println!("Executing program");
    }
    let _ = handle_execution(&program, source, filename);
}