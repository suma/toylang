use frontend::parser::core::Parser;

fn main() {
    let input = "fn main() -> i64 { val if = 1i64\n0i64 }";
    let mut parser = Parser::new(input);
    
    match parser.parse_program() {
        Ok(_) => println!("Parse succeeded - this is the problem"),
        Err(error) => {
            println!("Parse failed as expected: {}", error);
            std::process::exit(0);
        }
    }
}