use frontend::parser::core::ParserWithInterner;

fn main() {
    let test_cases = ["1u64+", "*2u64", "(1u64+2u64"];
    
    for input in &test_cases {
        println!("Testing input: '{}'", input);
        let mut parser = ParserWithInterner::new(input);
        
        // Check what tokens we get
        println!("  Tokens:");
        let mut temp_parser = frontend::parser::core::ParserWithInterner::new(input);
        let mut token_count = 0;
        while let Some(token) = temp_parser.peek() {
            println!("    {:?}", token);
            temp_parser.next();
            token_count += 1;
            if token_count > 10 { break; } // Prevent infinite loop
        }
        
        let result = parser.parse_expr_impl();
        
        println!("  parse_expr_impl result: {:?}", result);
        println!("  errors.len(): {}", parser.errors.len());
        println!("  errors: {:?}", parser.errors);
        
        // Check remaining tokens after parsing
        println!("  remaining token: {:?}", parser.peek());
        
        println!("  is_err: {}", result.is_err());
        println!("  should fail: {}", result.is_err() || parser.errors.len() > 0);
        println!();
    }
}