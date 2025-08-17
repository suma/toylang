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

/// Perform type checking and handle type check errors
fn handle_type_checking(program: &mut frontend::ast::Program, string_interner: &mut string_interner::DefaultStringInterner, source: &str, filename: &str) -> Result<(), ()> {
    let formatter = ErrorFormatter::new(source, filename);
    
    match interpreter::check_typing(program, string_interner, Some(source), Some(filename)) {
        Ok(()) => Ok(()),
        Err(errors) => {
            formatter.display_type_check_errors(&errors);
            Err(())
        }
    }
}

/// Execute the program and handle runtime errors
fn handle_execution(program: &frontend::ast::Program, string_interner: &string_interner::DefaultStringInterner, source: &str, filename: &str) -> Result<(), ()> {
    let formatter = ErrorFormatter::new(source, filename);
    
    match interpreter::execute_program(program, string_interner, Some(source), Some(filename)) {
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
    
    // Create a compiler session as the central compilation context
    let mut session = CompilerSession::new();
    
    // Read source first for error formatting
    let source = match fs::read_to_string(filename) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("Failed to read file {}: {}", filename, e);
            return;
        }
    };
    
    // Parse the source file within the compiler session context
    if verbose {
        println!("Parsing source file: {}", filename);
    }
    let mut program = match session.parse_program(&source) {
        Ok(prog) => prog,
        Err(err) => {
            let formatter = ErrorFormatter::new(&source, filename);
            formatter.format_parse_error(&err);
            return;
        }
    };
    
    // Perform type checking with session's shared resources
    if verbose {
        println!("Performing type checking");
    }
    if handle_type_checking(&mut program, session.string_interner_mut(), &source, filename).is_err() {
        return;
    }
    
    // Execute the program using session's context
    if verbose {
        println!("Executing program");
    }
    let _ = handle_execution(&program, session.string_interner(), &source, filename);
}