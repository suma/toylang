use frontend::parser::Parser;
use frontend::parser::expr::parse_block;

fn main() {
    // Test parsing a block with closing paren
    let input = "{ ) }";
    
    println!("Parsing input: '{}'", input);
    let mut parser = Parser::new(input);
    
    // Skip opening brace
    parser.next();
    
    // Try to parse block content
    match parse_block(&mut parser) {
        Ok(_expr) => println!("Parsed block successfully"),
        Err(e) => println!("Parse error: {:?}", e),
    }
}