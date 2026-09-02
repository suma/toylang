//! Consistency tests across the three execution paths.
//!
//! Each test source is run through:
//!
//! 1. **interpreter (lib API)** — `interpreter::execute_program` in
//!    process.
//! 2. **compiler** — `compile_file` produces an executable; we spawn
//!    it and observe its exit code.
//! 3. **JIT** — `interpreter::run_source` with `RunOptions::jit = true`,
//!    forcing the Cranelift JIT path **in-process**. The previous
//!    spawn-based design (`target/debug/interpreter` per test) was the
//!    largest hot spot in `samply` profiles — see
//!    `interpreter/src/output.rs` and `interpreter::jit::with_jit_override`
//!    for the per-call JIT/output overrides that make this safe under
//!    libtest's threaded execution.
//!
//! All three paths must agree on the value `main` would have returned,
//! with the standard POSIX truncation `& 0xff` applied uniformly so
//! programs need not keep their result under 256 to pass.
//!
//! These tests are slow because they invoke `cc`. Set `COMPILER_E2E=skip`
//! to opt out (mirrors `e2e.rs`).
//!
//! The tests are grouped into the modules below. They were one flat
//! 11k-line file in the order features landed; the grouping follows that
//! order rather than cutting across it, so a feature's tests stay
//! together and adjacent to what they were written beside.

mod harness;

mod basics;
mod contracts;
mod patterns_printing;
mod impls_refs;
mod allocators_drop;
mod types_strings;
mod compound_values;
mod dicts_closures;
mod interp_iterators;
mod traits_dyn;
mod match_scrutinee;
mod memory_profiling;
mod display_compound;
mod format_patterns;
mod const_eval;
mod float32;
mod simd;
mod soa;
mod diagnostics;
mod runtime_io;
mod parse_numbers;
mod generic_enum_payload;
mod primitive_receivers;
mod conv_span;
mod extern_buf;
mod net;
mod checked_narrow;
mod assignment_unit;
mod enum_arg_position;
mod char_literal_generic_arg;
mod compound_arg_call;
mod narrow_for_range;
mod enum_assoc_fn_producer;
