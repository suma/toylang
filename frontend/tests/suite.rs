//! Every integration test in this crate, in one binary.
//!
//! Cargo compiles and links each `tests/*.rs` as its own test binary.
//! Linking, not compiling, is what this workspace's build spends its
//! time on — the executables are ~27MB apiece because each one
//! statically embeds the frontend and cranelift, and a rebuild is
//! dominated by system time in the linker. `autotests = false` in
//! Cargo.toml turns the per-file default off so this is the crate's
//! single test target.
//!
//! Each former test binary is a module below, so test names gain their
//! file as a prefix. Filters keep working.


mod common;

#[path = "closure_parser_tests.rs"]
mod closure_parser_tests;

#[path = "closure_type_checking_tests.rs"]
mod closure_type_checking_tests;

#[path = "collections_tests.rs"]
mod collections_tests;

#[path = "collections_type_checking_tests.rs"]
mod collections_type_checking_tests;

#[path = "diagnostic_spelling_tests.rs"]
mod diagnostic_spelling_tests;

#[path = "dict_index_tests.rs"]
mod dict_index_tests;

#[path = "edge_case_boundary_tests.rs"]
#[allow(clippy::module_inception)]
mod edge_case_boundary_tests;

#[path = "error_handling_tests.rs"]
#[allow(clippy::module_inception)]
mod error_handling_tests;

#[path = "full_ast_cache_tests.rs"]
mod full_ast_cache_tests;

#[path = "generics_tests.rs"]
mod generics_tests;

#[path = "generics_unification_tests.rs"]
mod generics_unification_tests;

#[path = "infinite_recursion_tests.rs"]
#[allow(clippy::module_inception)]
mod infinite_recursion_tests;

#[path = "method_resolution_tests.rs"]
mod method_resolution_tests;

#[path = "module_interface_tests.rs"]
mod module_interface_tests;

#[path = "module_resolver_tests.rs"]
#[allow(clippy::module_inception)]
mod module_resolver_tests;

#[path = "module_system_tests.rs"]
mod module_system_tests;

#[path = "parser_integration_tests.rs"]
mod parser_integration_tests;

#[path = "property_based_integration_tests.rs"]
mod property_based_integration_tests;

#[path = "struct_literal_tests.rs"]
mod struct_literal_tests;

#[path = "type_conversion_tests.rs"]
mod type_conversion_tests;

#[path = "type_system_tests.rs"]
mod type_system_tests;
