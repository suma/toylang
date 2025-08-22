use frontend::parser::core::Parser;
use frontend::token::tokenize;
use string_interner::DefaultStringInterner;

fn main() {
    let test_cases = vec!["1u64+", "*2u64", "(1u64+2u64"];
    
    for case in test_cases {
        println!("\nTesting: '{}'", case);
        let mut interner = DefaultStringInterner::new();
        let tokens = tokenize(case, &mut interner);
        println!("Tokens: {:?}", tokens);
        
        if let Ok(tokens) = tokens {
            let mut parser = Parser::new(tokens, &mut interner, case);
            match parser.parse_expr_impl() {
                Ok(expr) => println!("Successfully parsed: {:?}", expr),
                Err(e) => println!("Parse error: {:?}", e),
            }
            println!("Parser errors: {:?}", parser.errors);
        } else {
            println!("Tokenization failed: {:?}", tokens);
        }
    }
}