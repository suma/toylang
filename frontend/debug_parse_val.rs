use frontend::parser::core::Parser;

fn main() {
    let input = "fn main() -> i64 { val if = 1i64; 0i64 }";
    let mut parser = Parser::new(input);
    
    match parser.parse_program() {
        Ok(_) => println!("✓ Parse succeeded"),
        Err(error) => {
            println!("✗ Parse failed: {}", error);
        }
    }
}