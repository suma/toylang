use frontend::parser::core::Parser;

fn main() {
    let input = "val if";
    let mut parser = Parser::new(input);
    
    println!("Testing input: '{}'", input);
    
    // Peek at each token
    for i in 0..5 {
        if let Some(token) = parser.peek() {
            println!("Token {}: {:?}", i, token);
            parser.next();
        } else {
            println!("Token {}: None (EOF)", i);
            break;
        }
    }
}