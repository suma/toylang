//! Linker driver: turn a `.o` byte buffer into an executable on disk.
//!
//! We deliberately delegate to the system C compiler (`cc`) rather than
//! inventing our own linker logic. The `.o` we just emitted via
//! cranelift-object exports a C-ABI `main`, which `cc` will happily wire
//! up to the platform's start-up code (crt1 / crt0 / equivalents). That
//! covers macOS, Linux, and (with `cl.exe` / MSVC) Windows without us
//! having to enumerate sysroot paths or argv conventions per OS.
//!
//! ## Tiny C runtime
//!
//! Each compiled executable is linked against a small C runtime
//! (`compiler/runtime/toylang_rt.c`) that provides type-specific
//! `print` / `println` helpers. The codegen pass emits direct calls
//! into these helpers; we compile the runtime alongside the toylang
//! object on every invocation. Doing it this way side-steps the
//! variadic ABI quirks (notably macOS aarch64) that make calling
//! `printf` directly from cranelift error-prone.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Source for the runtime, embedded into the compiler binary at build
/// time. We write it out to a temp file each compile so `cc` can see it.
const RUNTIME_C_SOURCE: &str = include_str!("../runtime/toylang_rt.c");

pub fn link_executable(object_bytes: &[u8], output: &Path, verbose: bool) -> Result<(), String> {
    // Write the toylang object next to the desired output. Putting it
    // in the same directory keeps the artefact local and easy to clean
    // up; we don't bother with /tmp since the compiler may run in a
    // sandbox with limited /tmp visibility.
    let tmp_obj = sibling_temp_path(output, ".o");
    std::fs::write(&tmp_obj, object_bytes)
        .map_err(|e| format!("write {}: {}", tmp_obj.display(), e))?;

    // Materialise the C runtime as a sibling source file. The compiler
    // built this binary with the source baked in via include_str!, so
    // the user doesn't need to ship the .c file themselves.
    let tmp_rt_src = sibling_temp_path(output, ".rt.c");
    std::fs::write(&tmp_rt_src, RUNTIME_C_SOURCE)
        .map_err(|e| format!("write {}: {}", tmp_rt_src.display(), e))?;

    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    if verbose {
        eprintln!(
            "invoking: {} {} {} -o {}",
            cc,
            tmp_obj.display(),
            tmp_rt_src.display(),
            output.display()
        );
    }
    // Hand `cc` both the toylang `.o` and the runtime `.c`; it compiles
    // the latter and links them into a single executable in one call.
    let status = Command::new(&cc)
        .arg(&tmp_obj)
        .arg(&tmp_rt_src)
        .arg("-o")
        .arg(output)
        .status()
        .map_err(|e| format!("failed to spawn `{cc}`: {e}"))?;
    // Best-effort cleanup; if linking failed we still want to surface
    // that, not the rm error.
    let _ = std::fs::remove_file(&tmp_obj);
    let _ = std::fs::remove_file(&tmp_rt_src);
    if !status.success() {
        return Err(format!("`{cc}` exited with status {}", status));
    }
    Ok(())
}

fn sibling_temp_path(output: &Path, extension_with_dot: &str) -> PathBuf {
    let mut p = output.to_path_buf();
    let stem = output
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| "a.out".to_string());
    p.set_file_name(format!(".toy_compile_{}{}", stem, extension_with_dot));
    p
}
