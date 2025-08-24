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
        Ok(program) => program,
        Err(err) => {
            eprintln!("Parse error: {:?}", err);
            process::exit(1);
        }
    };

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