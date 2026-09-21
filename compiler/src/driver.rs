//! Linker driver: turn a `.o` byte buffer into an executable on disk.
//!
//! We deliberately delegate to the system C compiler (`cc`) rather than
//! inventing our own linker logic. The `.o` we just emitted via
//! cranelift-object exports a C-ABI `main`, which `cc` will happily wire
//! up to the platform's start-up code (crt1 / crt0 / equivalents). That
//! covers macOS, Linux, and (with `cl.exe` / MSVC) Windows without us
//! having to enumerate sysroot paths or argv conventions per OS.
//!
//! ## Tiny Rust runtime
//!
//! Each compiled executable is linked against a small runtime
//! (`compiler/runtime/toylang_rt/`, a `no_std` Rust crate) that
//! provides type-specific `print` / `println` helpers plus the
//! allocator / profiler / io-extern machinery. The codegen pass emits
//! direct calls into these helpers; we build the runtime once at
//! compiler-build time (see `build.rs`) and link the staticlib archive
//! alongside the toylang object on every invocation. Doing it this way
//! side-steps the variadic ABI quirks (notably macOS aarch64) that
//! make calling `printf` directly from cranelift error-prone.

use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Pre-built runtime archive. `compiler/build.rs` compiles
/// `runtime/toylang_rt/src/lib.rs` to a staticlib with a bare rustc
/// invocation once when the compiler crate itself is built, so every
/// `link_executable` invocation can skip the runtime build step and
/// just hand `cc` two ready-to-link inputs. Massively cuts the
/// per-test cost of the compiler e2e suite (the runtime never changes
/// between tests in a single run).
const RUNTIME_ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/libtoylang_rt.a"));

/// Cache schema version — bump when the link inputs (cc flags,
/// runtime archive format, output convention) change in a way that
/// would invalidate previously-cached artefacts.
const LINK_CACHE_VERSION: u32 = 3;

/// Opt-in content-addressed cache for linked binaries. When the
/// `TOY_LINK_CACHE_DIR` env var is set, `link_executable` first
/// hashes its inputs (toylang object bytes + runtime object bytes
/// + cc selection + platform flag + cache version), looks up
///   `<dir>/<hash>.bin`, and — on hit — copies the cached binary
///   directly to `output` instead of invoking `cc`.
///
/// The dominant savings on macOS is the Mach-O ad-hoc code
/// signing pass that runs on every link (~150-300 ms per binary
/// on Apple Silicon). For repeat test runs (cargo nextest with
/// stable inputs) the hit rate approaches 100% after the first
/// run, turning the AOT-link bottleneck into a copy.
///
/// **This depends entirely on codegen being reproducible.** The
/// key is a hash of the object bytes, so a compiler whose output
/// drifts between runs turns the cache into a write-only
/// directory: it grew one entry per invocation and never hit once,
/// for as long as two passes iterated a `HashMap`. Measured after
/// the fix: an AOT build of a small program went from 67 ms to
/// 19 ms, and `cargo nextest run -p compiler` from 5.3 s to 2.8 s.
/// `compiler/tests/reproducible_build.rs` pins it — anything that
/// introduces an unordered iteration into lowering or codegen
/// silently costs the cache, so the guard has to be a test rather
/// than a comment.
///
/// Production users don't set the env var so behaviour stays
/// identical to the uncached path.
pub(crate) fn link_cache_dir(cli_override: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = cli_override {
        return Some(p.to_path_buf());
    }
    let raw = std::env::var_os("TOY_LINK_CACHE_DIR")?;
    if raw.is_empty() {
        return None;
    }
    Some(PathBuf::from(raw))
}

/// Stable hash of every input that affects the linked binary's
/// bytes. SipHasher13 with the fixed (0, 0) keys (i.e.
/// `DefaultHasher::new()`) is deterministic across processes —
/// caches built by one test run remain valid for the next.
fn compute_link_hash(object_bytes: &[u8], cc: &str, link_libs: &[String]) -> u64 {
    let mut h = DefaultHasher::new();
    h.write_u32(LINK_CACHE_VERSION);
    h.write_usize(object_bytes.len());
    h.write(object_bytes);
    h.write_usize(RUNTIME_ARCHIVE.len());
    h.write(RUNTIME_ARCHIVE);
    h.write(cc.as_bytes());
    // FFI_PLAN P1: the `-l` set and the `-L` search path both change
    // the linked binary. Forgetting them here would let two programs
    // that link different libraries share a cache entry — the
    // classic silent-mis-cache the hash exists to prevent.
    for lib in link_libs {
        h.write(lib.as_bytes());
        h.write_u8(0);
    }
    if let Some(paths) = std::env::var_os("TOYLANG_LINK_PATHS") {
        h.write(paths.as_encoded_bytes());
    }
    // Platform flag is encoded into the hash so a cache produced
    // on a Linux box can't collide with a macOS-flagged build.
    #[cfg(target_os = "macos")]
    h.write(b"macos:11.0");
    #[cfg(not(target_os = "macos"))]
    h.write(b"other");
    h.finish()
}

pub fn link_executable(
    object_bytes: &[u8],
    output: &Path,
    verbose: bool,
    cache_dir_override: Option<&Path>,
    link_libs: &[String],
) -> Result<(), String> {
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    // Opt-in cache hit: copy cached binary to `output`.
    if let Some(dir) = link_cache_dir(cache_dir_override) {
        let hash = compute_link_hash(object_bytes, &cc, link_libs);
        let cached = dir.join(format!("{hash:016x}.bin"));
        if cached.is_file() {
            // Copy preserves file mode on Unix (executable bit
            // included) so the resulting binary is runnable.
            std::fs::copy(&cached, output)
                .map_err(|e| format!("link cache copy {} -> {}: {}", cached.display(), output.display(), e))?;
            frontend::compile_profile::count("link.cache_hit", 1);
            record_exe_size(output);
            return Ok(());
        }
        frontend::compile_profile::count("link.cache_miss", 1);
        // Miss: link normally, then populate the cache atomically.
        link_executable_uncached(object_bytes, output, verbose, &cc, link_libs)?;
        if let Err(e) = populate_link_cache(&dir, hash, output) {
            // Cache population failure is non-fatal — we already
            // produced the requested binary. Surface a warning
            // rather than failing the user's build.
            eprintln!("link-cache: populate failed: {e}");
        }
        record_exe_size(output);
        return Ok(());
    }
    link_executable_uncached(object_bytes, output, verbose, &cc, link_libs)?;
    record_exe_size(output);
    Ok(())
}

/// COMPILE-PROFILE D: the size of what the link produced.
fn record_exe_size(output: &Path) {
    if frontend::compile_profile::is_enabled()
        && let Ok(meta) = std::fs::metadata(output)
    {
        frontend::compile_profile::count("link.exe_bytes", meta.len());
    }
}

/// Atomic cache write: copy `output` to a temp file in the cache
/// directory, then `rename` into place. Concurrent test workers
/// may race on the same hash; whichever rename wins, the result
/// is byte-identical so either outcome is correct.
fn populate_link_cache(dir: &Path, hash: u64, output: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create_dir_all {}: {}", dir.display(), e))?;
    // Embed the PID + nanos in the temp name so two concurrent
    // populators can't trip over each other's tmp file.
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(".{hash:016x}.{pid}.{nanos}.tmp"));
    std::fs::copy(output, &tmp)
        .map_err(|e| format!("link cache stage {}: {}", tmp.display(), e))?;
    let final_path = dir.join(format!("{hash:016x}.bin"));
    // `rename` is atomic on Unix and overwrites — the loser of a
    // race just overwrites with byte-identical content.
    std::fs::rename(&tmp, &final_path).map_err(|e| {
        // Cleanup the tmp on failure.
        let _ = std::fs::remove_file(&tmp);
        format!("link cache rename: {e}")
    })?;
    Ok(())
}

fn link_executable_uncached(
    object_bytes: &[u8],
    output: &Path,
    verbose: bool,
    cc: &str,
    link_libs: &[String],
) -> Result<(), String> {
    // Write the toylang object next to the desired output. Putting it
    // in the same directory keeps the artefact local and easy to clean
    // up; we don't bother with /tmp since the compiler may run in a
    // sandbox with limited /tmp visibility.
    let tmp_obj = sibling_temp_path(output, ".o");
    std::fs::write(&tmp_obj, object_bytes)
        .map_err(|e| format!("write {}: {}", tmp_obj.display(), e))?;

    // Materialise the pre-built runtime archive as a sibling file.
    // It's the same bytes for every link, so a follow-up could share
    // a single on-disk copy across compiles — but the cost of the
    // write itself is now well under the (already-eliminated) runtime
    // build cost, so leaving it per-compile keeps the cleanup logic
    // local and avoids races between concurrent test workers.
    let tmp_rt_archive = sibling_temp_path(output, ".rt.a");
    std::fs::write(&tmp_rt_archive, RUNTIME_ARCHIVE)
        .map_err(|e| format!("write {}: {}", tmp_rt_archive.display(), e))?;

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
    //
    // `-Wl,-dead_strip` removes the runtime functions the program
    // does not call. A rustc staticlib is a single archive member, so
    // the linker would otherwise pull in *all* `toy_*` helpers (the
    // old C object did too, but the Rust archive carries the f64
    // formatting machinery with it); stripping keeps the binaries at
    // the size of the pre-port era.
    let mut cmd = Command::new(cc);
    cmd.arg(&tmp_obj).arg(&tmp_rt_archive);
    #[cfg(target_os = "macos")]
    {
        cmd.arg("-mmacosx-version-min=11.0");
        cmd.arg("-Wl,-dead_strip");
    }
    // FFI_PLAN P1: `-l<lib>` per `extern fn ... from "lib"`
    // declaration, plus `-L` for every TOYLANG_LINK_PATHS entry.
    // The runtime crate is linked unconditionally as an archive, so
    // its lib name is a no-op here — the declaration exists so the
    // JIT and interpreter know which symbols the program means.
    for lib in link_libs {
        if lib == "toylang_rt" {
            continue;
        }
        cmd.arg(format!("-l{lib}"));
    }
    if let Some(paths) = std::env::var_os("TOYLANG_LINK_PATHS") {
        for path in std::env::split_paths(&paths) {
            cmd.arg(format!("-L{}", path.display()));
        }
    }
    cmd.arg("-o").arg(output);

    if verbose {
        eprintln!(
            "invoking: {} {} {}{} -o {}",
            cc,
            tmp_obj.display(),
            tmp_rt_archive.display(),
            if cfg!(target_os = "macos") {
                " -mmacosx-version-min=11.0 -Wl,-dead_strip"
            } else {
                ""
            },
            output.display()
        );
    }
    // Hand `cc` the object and the archive; it just links them into a
    // single executable, which is dramatically faster than rebuilding
    // the runtime every invocation.
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
    let _ = std::fs::remove_file(&tmp_rt_archive);
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
