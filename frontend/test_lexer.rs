use frontend::parser::core::Parser;

fn main() {
    let input = "fn main() -> i64 { val 123var = 1i64\n123var }";
    let mut parser = Parser::new(input);
    match parser.parse_program() {
        Ok(_) => println!("Parse succeeded (unexpected!)"),
        Err(e) => println!("Parse failed as expected: {:?}", e),
    }
}