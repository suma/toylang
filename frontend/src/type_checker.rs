use crate::ast::*;
use crate::type_decl::*;

// Builtin function signature definition
#[derive(Debug, Clone)]
pub struct BuiltinFunctionSignature {
    pub func: BuiltinFunction,
    pub arg_count: usize,
    pub arg_types: Vec<TypeDecl>,
    pub return_type: TypeDecl,
}

// Modular structure
pub mod core;
pub mod context;
pub mod error;
pub mod function;
pub mod generics;
pub mod inference;
pub mod optimization;

pub use core::CoreReferences;
pub use context::{is_wildcard_spec, TypeCheckContext, VarState};
pub use error::{SourceLocation, TypeCheckError, TypeCheckErrorKind};
pub use function::FunctionCheckingState;
pub use generics::GenericTypeChecking;
pub use inference::TypeInferenceState;
pub use optimization::PerformanceOptimization;

mod traits;
pub use traits::*;

mod literal_checker;
mod expression;
mod statement;
mod struct_literal;
mod impl_block;
mod trait_decl;
pub use trait_decl::expand_trait_defaults_in_pool;
mod collections;
mod builtin;
mod utility;
mod location;
mod error_helpers;
mod scope;
mod struct_registry;
mod method;
mod error_handling;
mod type_conversion;
mod tests;

mod recursive_type;
pub use recursive_type::check_recursive_types;

mod move_check;
pub use move_check::check_moves;

mod visitor;
mod visitor_impl;
mod module_access;
mod pattern_match;
mod method_call;

pub use visitor::TypeCheckerVisitor;
