//! toylang AOT compiler.
//!
//! Pipeline: source → frontend (parse + type-check via `compiler_core`) →
//! codegen (Cranelift Object emits a `.o`) → driver (system `cc` links into
//! an executable). The CLI lives in `main.rs`; the public API exposed here
//! lets tests drive the pipeline programmatically.
//!
//! ## Scope (initial MVP)
//!
//! What works:
//! - Numeric `fn main() -> u64 | i64` returning a value that becomes the
//!   process exit code.
//! - Scalar primitives: `i64`, `u64`, `bool`. (`f64` lowering is wired but
//!   currently only flows through arithmetic and comparison.)
//! - Literals, arithmetic (`+ - * /`), comparison (`== != < <= > >=`),
//!   logical AND/OR (short-circuit), unary minus, val/var bindings, plain
//!   assignment, `if/elif/else`, `while`, `for ... in start..end`,
//!   `break` / `continue`, `return`, calls to other compiled functions.
//!
//! What does NOT work yet (silently rejected with a clear error):
//! - Strings, structs, tuples, arrays, dicts, enums, traits, allocator
//!   features, contracts, generics, panic/assert, `print` / `println`,
//!   pointer / heap builtins, casts other than identity i64↔u64.
//!
//! These limitations exist because the Cranelift codegen here does not yet
//! have a runtime to back any of them. They will land in subsequent phases
//! (see `design-docs/todo.md` #183).

pub mod all_backends;
pub mod cache;
pub mod codegen;
pub mod driver;
pub use compiler_ir as ir;
pub mod jit;
/// The AST → IR lowering pass now lives in the `compiler_lower` crate so
/// the interpreter can drive it too (it cannot depend on `compiler`, which
/// depends on it). Re-exported as `compiler::lower` for source compat.
pub use compiler_lower as lower;
pub use compiler_lower::ContractMessages;
pub mod options;
mod small_pool;

pub use jit::{compile_to_jit_main, compile_to_jit_main_with_options, JitMainFn, JitProgram};
pub use options::{CompilerOptions, EmitKind};

use frontend::ast::File;
use std::path::{Path, PathBuf};
use string_interner::DefaultStringInterner;

/// Read the program, from stdin when `path` is `-` (D6).
///
/// The AOT backend compiles from a file, so a stdin program is spilled
/// to a temp file for that step. The caller gets the path to use and is
/// responsible for the returned guard's lifetime; dropping it removes
/// the spill.
pub fn read_input(path: &Path) -> std::io::Result<(String, PathBuf, Option<SpillGuard>)> {
    if path != Path::new("-") {
        let source = std::fs::read_to_string(path)?;
        return Ok((source, path.to_path_buf(), None));
    }
    use std::io::Read;
    let mut source = String::new();
    std::io::stdin().read_to_string(&mut source)?;
    let spill = all_backends::temp_path("toy_stdin").with_extension("t");
    std::fs::write(&spill, &source)?;
    Ok((source, spill.clone(), Some(SpillGuard(spill))))
}

/// Removes the stdin spill file when dropped.
pub struct SpillGuard(PathBuf);

impl Drop for SpillGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Top-level entry point used by both the CLI and the integration tests.
/// Returns `Ok(())` after writing whichever artefact `options.emit`
/// requested. Errors are stringified for display.
pub fn compile_file(options: &CompilerOptions) -> Result<(), String> {
    let source = std::fs::read_to_string(&options.input).map_err(|e| {
        format!("failed to read {}: {}", options.input.display(), e)
    })?;

    // Parse + type-check via the existing CompilerSession so this binary
    // shares interner state with the interpreter and stays consistent with
    // every other consumer of the frontend.
    let mut session = compiler_core::CompilerSession::new();
    let mut program = session
        .parse_program(&source)
        // DIAG-SYMBOL-NAME-LOWER: `ParserError` has a `Display` that
        // says what went wrong and where; `{:?}` handed the reader the
        // struct instead (`ParserError { kind: UnexpectedToken { .. },
        // location: SourceLocation { file: FileId(0), .. } }`).
        .map_err(|e| format!("parse error: {e}"))?;

    // Reuse the interpreter's check_typing so trait conformance, allocator
    // bounds, and contract validation all run before codegen sees the AST.
    // Forwards the optional core-modules directory so the AOT build path
    // sees the same auto-loaded modules the interpreter does (resolution
    // priority: `options.core_modules_dir` > `TOYLANG_CORE_MODULES` env
    // var > exe-relative search; see `resolve_core_modules_dir`).
    let core_modules_dir = resolve_core_modules_dir(options.core_modules_dir.clone());
    if options.verbose {
        if let Some(d) = &core_modules_dir {
            eprintln!("core modules: {}", d.display());
        } else {
            eprintln!("core modules: <none> (auto-load disabled)");
        }
    }
    let diagnostics_json = options.diagnostics_json;
    let warnings = interpreter::check_typing_diagnostics(
        &mut program,
        session.string_interner_mut(),
        Some(&source),
        Some(options.input.to_string_lossy().as_ref()),
        core_modules_dir.as_deref(),
    )
    .map_err(|diagnostics| {
        if diagnostics_json {
            interpreter::emit_diagnostics_json(&diagnostics);
            return format!("{} type-check error(s)", diagnostics.len());
        }
        let input_name = options.input.to_string_lossy();
        let formatter =
            interpreter::error_formatter::ErrorFormatter::new(&source, input_name.as_ref());
        let rendered: Vec<String> = diagnostics
            .iter()
            .map(|d| formatter.format_diagnostic(d))
            .collect();
        format!("type-check failed:\n  {}", rendered.join("\n  "))
    })?;

    // COMPILE-TIME-EVAL C4: warnings do not stop the compile, but the
    // driver is the only place a user would see them.
    if !warnings.is_empty() {
        if diagnostics_json {
            interpreter::emit_diagnostics_json(&warnings);
        } else {
            let input_name = options.input.to_string_lossy();
            let formatter =
                interpreter::error_formatter::ErrorFormatter::new(&source, input_name.as_ref());
            let rendered: Vec<String> = warnings
                .iter()
                .map(|d| formatter.format_diagnostic(d))
                .collect();
            formatter.display_warnings(&rendered);
        }
    }

    // Intern the canonical contract-violation messages now while the
    // session's interner is still mutable. The lowering pass uses
    // these symbols to attach a clause-specific panic message to
    // requires/ensures checks without needing `&mut` access of its
    // own.
    let contract_msgs = ContractMessages::intern(session.string_interner_mut());

    compile_checked_program(&program, session.string_interner(), &contract_msgs, options)
}

/// Lower, codegen and (optionally) link an already parsed **and
/// type-checked** program. Skips the frontend pass — source reading,
/// parsing, core-module integration and type-checking — which the
/// caller has already performed.
///
/// This is the entry the consistency suite uses to run every backend
/// off one checked program: the AOT lane used to re-parse and
/// re-type-check through [`compile_file`], duplicating the frontend
/// work the tree-walker / IR VM / JIT lanes already shared.
///
/// `contract_msgs` must be produced with the same interner the caller
/// passed to the type-checker (see [`ContractMessages::intern`]) — the
/// symbols it holds are resolved by address, so a different interner
/// would silently mis-key them.
pub fn compile_checked_program(
    program: &File,
    string_interner: &DefaultStringInterner,
    contract_msgs: &ContractMessages,
    options: &CompilerOptions,
) -> Result<(), String> {
    let (object_bytes, link_libs) =
        codegen::emit_object(program, string_interner, contract_msgs, options)?;

    match options.emit {
        EmitKind::Object => {
            let out = options.output.clone().unwrap_or_else(|| default_object_path(&options.input));
            std::fs::write(&out, &object_bytes)
                .map_err(|e| format!("failed to write {}: {}", out.display(), e))?;
            if options.verbose {
                eprintln!("wrote {}", out.display());
            }
        }
        EmitKind::Executable => {
            let out = options.output.clone().unwrap_or_else(|| default_exe_path(&options.input));
            driver::link_executable(
                &object_bytes,
                &out,
                options.verbose,
                options.link_cache_dir.as_deref(),
                &link_libs,
            )?;
            if options.verbose {
                eprintln!("wrote {}", out.display());
            }
        }
        EmitKind::Ir => {
            // Emit our own mid-level IR — the layer between AST and
            // Cranelift. Useful for inspecting how the front-end maps
            // onto the compiler's internal representation.
            let ir_text = codegen::emit_ir_text(program, string_interner, contract_msgs, options)?;
            let out = options.output.clone().unwrap_or_else(|| {
                let mut p = options.input.clone();
                p.set_extension("ir");
                p
            });
            std::fs::write(&out, ir_text)
                .map_err(|e| format!("failed to write {}: {}", out.display(), e))?;
            if options.verbose {
                eprintln!("wrote {}", out.display());
            }
        }
        EmitKind::Clif => {
            // Cranelift IR text — for backend debugging.
            let clif_text = codegen::emit_clif_text(program, string_interner, contract_msgs, options)?;
            let out = options.output.clone().unwrap_or_else(|| {
                let mut p = options.input.clone();
                p.set_extension("clif");
                p
            });
            std::fs::write(&out, clif_text)
                .map_err(|e| format!("failed to write {}: {}", out.display(), e))?;
            if options.verbose {
                eprintln!("wrote {}", out.display());
            }
        }
    }
    Ok(())
}

/// Resolve the core-modules directory using the same priority chain
/// the interpreter binary does. Mirrors
/// `interpreter::main::resolve_core_modules_dir` so a single build of
/// the source repo behaves identically across the AOT compiler and
/// the interpreter:
///
/// 1. CLI / API caller override (`options.core_modules_dir`).
/// 2. `TOYLANG_CORE_MODULES` env var. Empty value opts out.
/// 3. Executable-relative probe — `<exe>/modules/`,
///    `<exe>/../share/toylang/modules/`, `<exe>/../../interpreter/modules/`
///    (the last entry is the dev-tree fallback so
///    `target/debug/compiler` finds `<repo>/interpreter/modules/`).
///
/// Returns `None` when nothing resolves; auto-loading then becomes a
/// no-op.
pub fn resolve_core_modules_dir(
    cli_override: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    if let Some(p) = cli_override {
        return Some(p);
    }
    if let Some(env_val) = std::env::var_os("TOYLANG_CORE_MODULES") {
        if env_val.is_empty() {
            return None;
        }
        return Some(std::path::PathBuf::from(env_val));
    }
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    // Default search candidates. The third entry is the dev-tree
    // fallback: when the binary is `target/debug/compiler`,
    // `exe_dir/../../core` resolves to `<repo>/core/`. The first two
    // cover a co-located distribution and a Unix install layout.
    let candidates: [std::path::PathBuf; 3] = [
        exe_dir.join("core"),
        exe_dir.join("../share/toylang/core"),
        exe_dir.join("../../core"),
    ];
    for cand in candidates {
        if cand.is_dir() {
            return Some(cand);
        }
    }
    None
}

fn default_object_path(input: &Path) -> std::path::PathBuf {
    let mut p = input.to_path_buf();
    p.set_extension("o");
    p
}

fn default_exe_path(input: &Path) -> std::path::PathBuf {
    let mut p = input.to_path_buf();
    p.set_extension("");
    if p.as_os_str().is_empty() {
        p = std::path::PathBuf::from("a.out");
    }
    p
}
