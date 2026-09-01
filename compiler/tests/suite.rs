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
//!
//! nextest runs one process per test, so this does not merge the
//! tests' address spaces. Under plain `cargo test` they become threads
//! in one process, which matters for the tests here that set
//! environment variables (`consistency.rs`, `ffi_tests.rs`).

#[path = "all_backends_cli.rs"]
mod all_backends_cli;

#[path = "consistency/mod.rs"]
mod consistency;

#[path = "e2e.rs"]
mod e2e;

#[path = "e2e_batched.rs"]
mod e2e_batched;

#[path = "example_consistency.rs"]
mod example_consistency;

#[path = "ffi_tests.rs"]
mod ffi_tests;

#[path = "net_abi_tests.rs"]
mod net_abi_tests;

#[path = "jit_smoke.rs"]
mod jit_smoke;

#[path = "lower_diagnostic_spelling.rs"]
mod lower_diagnostic_spelling;

#[path = "reproducible_build.rs"]
mod reproducible_build;
