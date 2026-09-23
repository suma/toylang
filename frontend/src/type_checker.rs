use crate::ast::*;
use crate::type_decl::*;

/// One builtin's declared shape.
///
/// Only `func` and `return_type` are consulted: the lookup in
/// `visit_builtin_call_impl` finds the row by `func` and answers with
/// `return_type` **without visiting the arguments**. `arg_types`
/// therefore documents the intended prototype rather than enforcing
/// it — the builtins whose arguments really are checked go through
/// `check_memory_builtin_args`. There used to be an `arg_count`
/// beside it, restated by hand in every row and read by nothing.
#[derive(Debug, Clone)]
pub struct BuiltinFunctionSignature {
    pub func: BuiltinFunction,
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
pub use context::{is_wildcard_spec, path_ends_with, FnLookup, ModuleFunction, TypeCheckContext, VarState};
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
mod trait_overload;
pub use trait_overload::{
    base_method_name, find_duplicate_impl_method, mangle_overloaded_trait_impls,
    overload_candidates, overload_name,
};

pub mod effects;
pub use effects::{Effect, EffectSet, EffectTable};
mod alloc_check;
mod parallel_check;
mod module_path_check;
pub use alloc_check::check_never_allocates;
mod const_fn_check;
mod unsafe_check;
pub use const_fn_check::check_const_fn;
pub use unsafe_check::check_unsafe_declarations;
mod contract_purity;
pub use contract_purity::check_contract_purity;
mod unused_result;
pub use unused_result::check_unused_results;
mod collections;
mod builtin;
mod utility;
mod location;
mod error_helpers;
mod scope;
mod struct_registry;
mod method;
mod type_conversion;
mod tests;

mod region_check;
pub use region_check::check_regions;

mod recursive_type;
pub use recursive_type::check_recursive_types;

mod move_check;
pub use move_check::check_moves;
pub use parallel_check::check_parallel_loops;
pub use module_path_check::check_module_paths;

mod contains_drop;
pub use contains_drop::{type_contains_drop, DropAnalysis};

mod closure_escape;
pub use closure_escape::mark_by_ref_closures;

mod visitor;
mod visitor_impl;
mod module_access;
mod eq_requirement;
mod pattern_match;
mod enum_cast;
mod enum_struct_variant;
mod method_call;
mod simd;

pub use visitor::TypeCheckerVisitor;
