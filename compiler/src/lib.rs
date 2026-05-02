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
//! (see `todo.md` #183).

pub mod codegen;
pub mod driver;
pub mod ir;
pub mod lower;
pub mod options;

pub use options::{CompilerOptions, EmitKind};

use std::path::Path;

/// Pre-interned panic messages used by the lowering pass to attach
/// clause-specific text to contract-violation panics. We intern these
/// once up front so the lowering code can pass `DefaultSymbol`s
/// straight through to `Terminator::Panic` without ever needing
/// mutable access to the interner itself.
pub struct ContractMessages {
    pub requires_violation: string_interner::DefaultSymbol,
    pub ensures_violation: string_interner::DefaultSymbol,
}

impl ContractMessages {
    pub fn intern(interner: &mut string_interner::DefaultStringInterner) -> Self {
        Self {
            requires_violation: interner.get_or_intern("requires violation"),
            ensures_violation: interner.get_or_intern("ensures violation"),
        }
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
        .map_err(|e| format!("parse error: {e:?}"))?;

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
    interpreter::check_typing_with_core_modules(
        &mut program,
        session.string_interner_mut(),
        Some(&source),
        Some(options.input.to_string_lossy().as_ref()),
        core_modules_dir.as_deref(),
    )
    .map_err(|errors| format!("type-check failed:\n  {}", errors.join("\n  ")))?;

    // Intern the canonical contract-violation messages now while the
    // session's interner is still mutable. The lowering pass uses
    // these symbols to attach a clause-specific panic message to
    // requires/ensures checks without needing `&mut` access of its
    // own.
    let contract_msgs = ContractMessages::intern(session.string_interner_mut());

    let object_bytes = codegen::emit_object(&program, session.string_interner(), &contract_msgs, options)?;

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
            driver::link_executable(&object_bytes, &out, options.verbose)?;
            if options.verbose {
                eprintln!("wrote {}", out.display());
            }
        }
        EmitKind::Ir => {
            // Emit our own mid-level IR — the layer between AST and
            // Cranelift. Useful for inspecting how the front-end maps
            // onto the compiler's internal representation.
            let ir_text = codegen::emit_ir_text(&program, session.string_interner(), &contract_msgs, options)?;
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
            let clif_text = codegen::emit_clif_text(&program, session.string_interner(), &contract_msgs, options)?;
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
