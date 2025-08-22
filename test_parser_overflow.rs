use frontend::parser::Parser;

fn main() {
    let input = "fn main() -> u64 {
    )
}";
    
    println!("Parsing input...");
    println!("Input:\n{}", input);
    let mut parser = Parser::new(input);
    
    // Set recursion limit
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024) // 8MB stack
        .spawn(move || {
            let result = parser.parse_program_multiple_errors();
    
    if result.has_errors() {
        println!("Errors found:");
        for err in &result.errors {
            println!("  - {}", err);
        }
    }
    
    if let Some(program) = result.result {
        println!("Program parsed successfully!");
        println!("Functions: {}", program.function.len());
    }
}