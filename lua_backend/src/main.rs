use lua_backend::{LuaCodeGenerator, LuaTarget};
use compiler_core::CompilerSession;
use std::env;
use std::fs;
use std::process;

fn print_usage(program_name: &str) {
    eprintln!("Usage: {} [options] <source_file.t>", program_name);
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --luajit    Generate code for LuaJIT (uses bit module)");
    eprintln!("  --lua53     Generate code for Lua 5.3+ (default, uses native bitwise operators)");
    eprintln!("  --help      Show this help message");
}

fn main() {
    let args: Vec<String> = env::args().collect();
    
    if args.len() < 2 {
        print_usage(&args[0]);
        process::exit(1);
    }
    
    let mut target = LuaTarget::Lua53; // Default target
    let mut input_file = None;
    
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--luajit" => {
                target = LuaTarget::LuaJIT;
            }
            "--lua53" => {
                target = LuaTarget::Lua53;
            }
            "--help" | "-h" => {
                print_usage(&args[0]);
                process::exit(0);
            }
            arg if !arg.starts_with("--") => {
                if input_file.is_none() {
                    input_file = Some(arg.to_string());
                } else {
                    eprintln!("Error: Multiple input files specified");
                    print_usage(&args[0]);
                    process::exit(1);
                }
            }
            _ => {
                eprintln!("Error: Unknown option '{}'", args[i]);
                print_usage(&args[0]);
                process::exit(1);
            }
        }
        i += 1;
    }
    
    let input_file = match input_file {
        Some(file) => file,
        None => {
            eprintln!("Error: No input file specified");
            print_usage(&args[0]);
            process::exit(1);
        }
    };
    
    let input_file = &input_file;
    let source_code = match fs::read_to_string(input_file) {
        Ok(content) => content,
        Err(err) => {
            eprintln!("File read error: {}", err);
            process::exit(1);
        }
    };

    let mut session = CompilerSession::new();
    let program = match session.parse_program(&source_code) {
        Ok(program) => program,
        Err(err) => {
            eprintln!("Parse error: {}", err);
            process::exit(1);
        }
    };

    // Skip type checking for now and use basic code generation
    let mut generator = LuaCodeGenerator::new(&program, session.string_interner())
        .with_target(target);
    
    let lua_code = match generator.generate() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("Lua generation error: {}", err);
            process::exit(1);
        }
    };

    println!("{}", lua_code);
}