#![feature(box_patterns)]

use std::io::{self, Write};
use frontend;
use bytecodeinterpreter::compiler::*;
use bytecodeinterpreter::processor::Processor;

fn main() {
    let mut compiler = Compiler::new();
    let mut interpreter = Processor::new();

    loop {
        println!("Input toylang expression:");
        print!(">>> ");
        io::stdout().flush().unwrap();
        let mut line = String::new();
        io::stdin().read_line(&mut line).expect("Failed to read line `read_line`");

        let mut parser = frontend::Parser::new(line.as_str());
        let expr = parser.parse_expr();
        if expr.is_err() {
            println!("parser_expr failed {}", expr.unwrap_err());
            return;
        }
        let expr = expr.unwrap();
        let codes: Vec<BCode> = compiler.compile(&expr).clone();
        interpreter.append(codes);
        interpreter.evaluate();
        println!("Evaluate expression: {:?}", interpreter);
    }
}