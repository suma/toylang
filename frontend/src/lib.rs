pub mod ast;
pub mod type_decl;
pub mod token;
pub mod type_checker;
pub mod parser;
pub mod visitor;

pub use parser::Parser;
pub use parser::error::{MultipleParserResult, ParserError};
pub use type_checker::error::{MultipleTypeCheckResult, TypeCheckError};

#[cfg(test)]
mod multiple_errors_test;

#[cfg(test)]
mod edge_case_tests;

// #[cfg(test)]
// mod property_tests;

#[cfg(test)]
mod error_handling_tests;

#[cfg(test)]
mod boundary_tests;

#[cfg(test)]
mod infinite_recursion_test;

