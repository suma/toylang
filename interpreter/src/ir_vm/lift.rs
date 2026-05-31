//! AST → IR lifting bridge.
//!
//! The actual lowering lives in `compiler::lower` and is invoked from
//! the top-level interpreter entry point (`execute_program` in `lib.rs`).
//! This module is a placeholder so the VM can receive an already-lowered
//! `compiler_ir::Module`.
