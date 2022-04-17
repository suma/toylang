#![feature(box_patterns)]

mod processor;

use std::io;
use frontend;
use frontend::ast::*;
use processor::*;

fn main() {
    let mut p = Processor::new();
    loop {
        println!("Input toylang expression:");
        let mut line = String::new();
        io::stdin().read_line(&mut line).expect("Failed to read line `read_line`");

        let mut parser = frontend::Parser::new(line.as_str());
        let expr = parser.parse_expr();
        if expr.is_err() {
            println!("parser_expr failed {}", expr.unwrap_err());
            return;
        }
        println!("print AST: {:?}", expr.as_ref().unwrap());
        let expr = expr.unwrap();
        println!("Evaluate expression: {:?}", p.evaluate(&expr));
    }
}