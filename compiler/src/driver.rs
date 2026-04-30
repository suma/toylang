//! Linker driver: turn a `.o` byte buffer into an executable on disk.
//!
//! We deliberately delegate to the system C compiler (`cc`) rather than
//! inventing our own linker logic. The `.o` we just emitted via
//! cranelift-object exports a C-ABI `main`, which `cc` will happily wire
//! up to the platform's start-up code (crt1 / crt0 / equivalents). That
//! covers macOS, Linux, and (with `cl.exe` / MSVC) Windows without us
//! having to enumerate sysroot paths or argv conventions per OS.

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn link_executable(object_bytes: &[u8], output: &Path, verbose: bool) -> Result<(), String> {
    // Write the object to a temp path next to the desired output. Putting
    // it in the same directory keeps the artefact local and easy to clean
    // up; we don't bother with /tmp since the compiler may run in a
    // sandbox with limited /tmp visibility.
    let tmp_obj = sibling_temp_path(output, ".o");
    std::fs::write(&tmp_obj, object_bytes)
        .map_err(|e| format!("write {}: {}", tmp_obj.display(), e))?;

    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    if verbose {
        eprintln!("invoking: {} {} -o {}", cc, tmp_obj.display(), output.display());
    }
    let status = Command::new(&cc)
        .arg(&tmp_obj)
        .arg("-o")
        .arg(output)
        .status()
        .map_err(|e| format!("failed to spawn `{cc}`: {e}"))?;
    // Best-effort cleanup; if linking failed we still want to surface
    // that, not the rm error.
    let _ = std::fs::remove_file(&tmp_obj);
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
