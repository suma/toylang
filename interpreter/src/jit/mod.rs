//! Optional JIT backend for the interpreter, gated behind the `jit` cargo
//! feature. Activated at runtime by setting `INTERPRETER_JIT=1`.
//!
//! Only a small numeric/bool subset of the language is currently handled;
//! anything outside the supported subset causes a silent fallback to the
//! tree-walking interpreter.

mod eligibility;
mod codegen;
mod runtime;

pub use runtime::try_execute_main;
