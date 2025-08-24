use lua_backend::LuaCodeGenerator;
use compiler_core::CompilerSession;
use std::env;
use std::fs;
use std::process;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <source_file.t>", args[0]);
        process::exit(1);
    }

    let input_file = &args[1];
    let source_code = match fs::read_to_string(input_file) {
        Ok(content) => content,
        Err(err) => {
            eprintln!("File read error: {}", err);
            process::exit(1);
        }
    };

    let mut session = CompilerSession::new();
    let program = match session.parse_program(&source_code) {
        Ok(program) => {
            // Debug: print program structure
            println!("Debug: Program structure:");
            println!("  Functions: {}", program.function.len());
            for (i, func) in program.function.iter().enumerate() {
                let func_name = session.string_interner().resolve(func.name).unwrap_or("<unknown>");
                println!("    Function {}: {}", i, func_name);
            }
            println!("  Statements: {}", program.statement.len());
            for (i, stmt) in program.statement.0.iter().enumerate() {
                match stmt {
                    frontend::ast::Stmt::StructDecl { name, .. } => {
                        println!("    Statement {}: StructDecl({})", i, name);
                    }
                    frontend::ast::Stmt::ImplBlock { target_type, methods } => {
                        println!("    Statement {}: ImplBlock({}, {} methods)", i, target_type, methods.len());
                        for method in methods {
                            let method_name = session.string_interner().resolve(method.name).unwrap_or("<unknown>");
                            println!("      Method: {}", method_name);
                        }
                    }
                    _ => {
                        println!("    Statement {}: {:?}", i, std::mem::discriminant(stmt));
                    }
                }
            }
            println!("  Expressions: {}", program.expression.len());
            program
        }
        Err(err) => {
            eprintln!("Parse error: {}", err);
            process::exit(1);
        }
    };

    // Skip type checking for now and use basic code generation
    let mut generator = LuaCodeGenerator::new(&program, session.string_interner());
    
    let lua_code = match generator.generate() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("Lua generation error: {}", err);
            process::exit(1);
        }
    };

    println!("{}", lua_code);
}