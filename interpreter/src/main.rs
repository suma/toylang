use std::env;
use std::fs;
use interpreter::error_formatter::ErrorFormatter;
use compiler_core::CompilerSession;

/// Parse the source file using CompilerSession and handle parse errors
#[allow(dead_code)]
fn handle_parsing_from_source(source: &str, filename: &str) -> Result<frontend::ast::Program, ()> {
    let mut session = CompilerSession::new();
    let formatter = ErrorFormatter::new(source, filename);
    
    // Use CompilerSession's parse_program method which ensures consistent string interning
    match session.parse_program(source) {
        Ok(program) => Ok(program),
        Err(err) => {
            formatter.format_parse_error(&err);
            Err(())
        }
    }
}

/// Parse a file using CompilerSession's parse_module_file method
fn handle_parsing_from_file(file_path: &str) -> Result<frontend::ast::Program, ()> {
    let mut session = CompilerSession::new();
    
    // Use CompilerSession's parse_module_file method for consistent file handling
    match session.parse_module_file(file_path) {
        Ok(program) => Ok(program),
        Err(err) => {
            // Read source for error formatting
            if let Ok(source) = std::fs::read_to_string(file_path) {
                let formatter = ErrorFormatter::new(&source, file_path);
                formatter.format_parse_error(&err);
            } else {
                eprintln!("Parse error in {}: {}", file_path, err);
            }
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

    let filename = args[1].as_str();
    
    // Parse the source file using CompilerSession
    if verbose {
        println!("Parsing source file: {}", filename);
    }
    let mut program = match handle_parsing_from_file(filename) {
        Ok(prog) => prog,
        Err(()) => return,
    };
    
    // Read source for error formatting in subsequent steps
    let source = match fs::read_to_string(filename) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("Failed to read file {}: {}", filename, e);
            return;
        }
    };
    
    // Perform type checking
    if verbose {
        println!("Performing type checking");
    }
    if handle_type_checking(&mut program, &source, filename).is_err() {
        return;
    }
    
    // Execute the program
    if verbose {
        println!("Executing program");
    }
    let _ = handle_execution(&program, &source, filename);
}