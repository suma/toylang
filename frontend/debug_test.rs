use crate::parser::core::Parser;

fn main() {
    let mut parser = Parser::new("(1u64+2u64");
    let result = parser.parse_expr_impl();
    println\!("Result: {:?}", result);
    println\!("Parser errors: {:?}", parser.errors);
}
EOF < /dev/null
