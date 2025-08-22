use frontend::parser::core::Parser;

fn main() {
    let input = "fn main() -> i64 { val if = 1i64\n0i64 }";
    println!("Testing input: {}", input);
    
    let mut parser = Parser::new(input);
    match parser.parse_program() {
        Ok(_) => println!("✓ Parsed successfully (unexpected!)"),
        Err(errors) => println!("✗ Parse failed as expected: {:?}", errors),
    }
}