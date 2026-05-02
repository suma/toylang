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

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

    // Platform-specific link flags. On macOS, cranelift's `ObjectModule`
    // doesn't emit an `LC_BUILD_VERSION` load command in the Mach-O
    // it produces; recent `ld` versions warn about this with
    // "no platform load command found in '...'" for every input
    // object. Passing `-mmacosx-version-min=...` tells the driver
    // (and downstream linker) which platform to assume, which
    // suppresses the warning. The version we pick is conservative —
    // any reasonable supported macOS works; the value doesn't affect
    // codegen (cranelift already generated the object), only the
    // linker's deployment-target metadata. No-op on Linux / Windows
    // where cranelift emits ELF / COFF and the warning doesn't apply.
    let mut cmd = Command::new(&cc);
    cmd.arg(&tmp_obj).arg(&tmp_rt_src);
    #[cfg(target_os = "macos")]
    {
        cmd.arg("-mmacosx-version-min=11.0");
    }
    cmd.arg("-o").arg(output);

    if verbose {
        eprintln!(
            "invoking: {} {} {}{} -o {}",
            cc,
            tmp_obj.display(),
            tmp_rt_src.display(),
            if cfg!(target_os = "macos") {
                " -mmacosx-version-min=11.0"
            } else {
                ""
            },
            output.display()
        );
    }
    // Hand `cc` both the toylang `.o` and the runtime `.c`; it compiles
    // the latter and links them into a single executable in one call.
    //
    // On macOS we capture stderr and filter out the per-object
    // "ld: warning: no platform load command found in '...'"
    // diagnostic. cranelift-object doesn't emit `LC_BUILD_VERSION`,
    // so the linker prints this warning for every input object even
    // though `-mmacosx-version-min` already gives it the deployment
    // target. The forwarded stderr keeps every other diagnostic
    // intact (real link errors, missing symbols, etc.).
    cmd.stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn `{cc}`: {e}"))?;
    let stderr_handle = child.stderr.take();
    let output = child
        .wait_with_output()
        .map_err(|e| format!("waiting for `{cc}`: {e}"))?;
    drop(stderr_handle);
    forward_filtered_stderr(&output.stderr);
    let status = output.status;
    // Best-effort cleanup; if linking failed we still want to surface
    // that, not the rm error.
    let _ = std::fs::remove_file(&tmp_obj);
    let _ = std::fs::remove_file(&tmp_rt_src);
    if !status.success() {
        return Err(format!("`{cc}` exited with status {}", status));
    }
    Ok(())
}

/// Forward `cc`'s stderr to ours, dropping the macOS-specific
/// "no platform load command found" warning that cranelift-object's
/// Mach-O output triggers for every input object. Other diagnostics
/// pass through unchanged.
fn forward_filtered_stderr(bytes: &[u8]) {
    let stderr_text = String::from_utf8_lossy(bytes);
    let mut out = std::io::stderr().lock();
    for line in stderr_text.split_inclusive('\n') {
        if line.contains("ld: warning: no platform load command found") {
            continue;
        }
        let _ = out.write_all(line.as_bytes());
    }
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
