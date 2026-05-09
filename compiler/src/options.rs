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
    /// Skip Design-by-Contract runtime checks (`requires` / `ensures`).
    /// Equivalent to the interpreter's `INTERPRETER_CONTRACTS=off`. Use
    /// when the contract overhead matters and the predicates have been
    /// validated in a checked build.
    pub release: bool,
    /// Override for the core-modules directory. When `None`, the
    /// driver consults `TOYLANG_CORE_MODULES` and then falls back to
    /// an executable-relative search (see
    /// `compiler::resolve_core_modules_dir`). Set explicitly via the
    /// `--core-modules <DIR>` CLI flag or by direct API consumers.
    pub core_modules_dir: Option<PathBuf>,
    /// Content-addressed link cache directory. When `Some`, the linker
    /// driver looks up `<dir>/<hash>.bin` keyed on the toylang object
    /// bytes + cc + platform; cache hits skip the `cc` invocation and
    /// just copy the cached binary to `output`. `None` falls back to
    /// the `TOY_LINK_CACHE_DIR` env var, then to no cache. The
    /// integration tests pin a stable per-suite cache dir here so
    /// repeat runs of `cargo nextest` reuse linked binaries instead
    /// of re-invoking `cc` on every test.
    pub link_cache_dir: Option<PathBuf>,
}

impl CompilerOptions {
    pub fn new(input: PathBuf) -> Self {
        Self {
            input,
            output: None,
            emit: EmitKind::Executable,
            verbose: false,
            release: false,
            core_modules_dir: None,
            link_cache_dir: None,
        }
    }
}
