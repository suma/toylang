#![allow(clippy::slow_vector_initialization)]
#![allow(clippy::upper_case_acronyms)]

pub mod api;
pub mod ast;
#[cfg(feature = "serde")]
pub mod cache;
pub mod compile_profile;
pub mod type_decl;
pub mod token;
pub mod format_spec;
pub mod type_checker;
pub mod diagnostic;
pub mod source_map;
pub mod explain;
pub mod parser;
pub mod visitor;
pub mod module_resolver;
pub mod alias_resolution;

#[cfg(test)]
mod tuple_tests;

pub use alias_resolution::resolve_type_aliases;
pub use parser::{Parser, ParserWithInterner};
pub use parser::error::{MultipleParserResult, ParserError};
pub use type_checker::error::{MultipleTypeCheckResult, TypeCheckError};
pub use module_resolver::{ModuleResolver, ResolvedModule};


