//! Command-line / API options for a single compiler invocation. Lives in
//! its own module so both `main.rs` and the integration tests can build a
//! `CompilerOptions` value directly without re-parsing CLI flags.

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmitKind {
    /// Default. Emit an executable by linking the object via the system C
    /// compiler. The output path defaults to the stem of the input file.
    Executable,
    /// Emit an unlinked `.o` object file. Useful for inspecting symbols
    /// or linking by hand.
    Object,
    /// Emit the compiler's mid-level IR (the `ir` module). Useful for
    /// reviewing how the front-end was lowered before the Cranelift step.
    Ir,
    /// Emit Cranelift IR (`.clif`) text. Useful for debugging the
    /// backend codegen (post-IR).
    Clif,
}

#[derive(Debug, Clone)]
pub struct CompilerOptions {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub emit: EmitKind,
    pub verbose: bool,
}

impl CompilerOptions {
    pub fn new(input: PathBuf) -> Self {
        Self {
            input,
            output: None,
            emit: EmitKind::Executable,
            verbose: false,
        }
    }
}
