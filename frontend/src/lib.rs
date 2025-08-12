pub mod ast;
pub mod type_decl;
pub mod token;
pub mod type_checker;
pub mod parser;
pub mod visitor;

pub use parser::Parser;
pub use parser::error::{MultipleParserResult, ParserError};
pub use type_checker::error::{MultipleTypeCheckResult, TypeCheckError};


