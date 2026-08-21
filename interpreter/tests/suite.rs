//! Every integration test in this crate, in one binary.
//!
//! Cargo compiles and links each `tests/*.rs` as its own test binary.
//! At 36 files that meant 36 links of a ~27MB executable — each one
//! statically embedding the interpreter, the frontend, and cranelift —
//! and linking, not compiling, is what the build spends its time on
//! (touching one test file relinks in ~9s; touching a library rebuilds
//! for ~2min, of which the great majority is system time in the
//! linker). `autotests = false` in Cargo.toml turns the auto-discovery
//! off so this file is the crate's single test target.
//!
//! Each former test binary is included below as a module, so test
//! names gain their file as a prefix: `language_core_tests::foo`
//! rather than a bare `foo`. Filters keep working.
//!
//! nextest runs one process per test, so merging the binaries does not
//! merge the tests' address spaces. Plain `cargo test` does run them as
//! threads in one process — tests that mutate process-wide state
//! (environment variables, the destruction log) rely on `#[serial]`
//! for that, which now actually serializes them where before they
//! happened to be in different binaries.

mod common;

#[path = "advanced_syntax_tests.rs"]
mod advanced_syntax_tests;

#[path = "auxiliary_query_tests.rs"]
mod auxiliary_query_tests;

#[path = "builtin_test_and_check_tests.rs"]
mod builtin_test_and_check_tests;

#[path = "closure_tests.rs"]
mod closure_tests;

#[path = "collections_array_slice_tests.rs"]
mod collections_array_slice_tests;

#[path = "collections_dict_tests.rs"]
mod collections_dict_tests;

#[path = "collections_tuple_struct_tests.rs"]
mod collections_tuple_struct_tests;

#[path = "contract_mode_tests.rs"]
mod contract_mode_tests;

#[path = "debug_builtins_tests.rs"]
mod debug_builtins_tests;

#[path = "diagnostics_json_tests.rs"]
mod diagnostics_json_tests;

#[path = "diagnostics_location_tests.rs"]
mod diagnostics_location_tests;

#[path = "diagnostics_recovery_tests.rs"]
mod diagnostics_recovery_tests;

#[path = "generics_tests.rs"]
mod generics_tests;

#[path = "if_val_tests.rs"]
mod if_val_tests;

#[path = "incremental_compilation_tests.rs"]
mod incremental_compilation_tests;

#[path = "io_tests.rs"]
mod io_tests;

#[path = "ir_vm_engine_parity.rs"]
mod ir_vm_engine_parity;

#[path = "iterator_protocol_tests.rs"]
mod iterator_protocol_tests;

#[path = "jit_integration.rs"]
mod jit_integration;

#[path = "labelled_loop_tests.rs"]
mod labelled_loop_tests;

#[path = "language_core_tests.rs"]
mod language_core_tests;

#[path = "lex_error_diagnostics_tests.rs"]
mod lex_error_diagnostics_tests;

#[path = "memory_tests.rs"]
mod memory_tests;

#[path = "module_property_tests.rs"]
mod module_property_tests;

#[path = "move_check_tests.rs"]
mod move_check_tests;

#[path = "oop_tests.rs"]
mod oop_tests;

#[path = "operator_overload_tests.rs"]
mod operator_overload_tests;

#[path = "checked_arith_tests.rs"]
mod checked_arith_tests;

#[path = "contract_alloc_sugar_tests.rs"]
mod contract_alloc_sugar_tests;

#[path = "contract_old_tests.rs"]
mod contract_old_tests;

#[path = "contract_trait_tests.rs"]
mod contract_trait_tests;

#[path = "ord_tests.rs"]
mod ord_tests;

#[path = "recursive_type_tests.rs"]
mod recursive_type_tests;

#[path = "regression_tests.rs"]
mod regression_tests;

#[path = "runtime_observability_tests.rs"]
mod runtime_observability_tests;

#[path = "string_interpolation_tests.rs"]
mod string_interpolation_tests;

#[path = "string_stdlib_tests.rs"]
mod string_stdlib_tests;

#[path = "trait_tests.rs"]
mod trait_tests;

#[path = "try_op_tests.rs"]
mod try_op_tests;
