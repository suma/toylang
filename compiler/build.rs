// Pre-compile the toylang runtime once at compiler-build time and
// stash the resulting staticlib archive next to OUT_DIR. The link
// driver (`src/driver.rs`) loads those bytes via `include_bytes!` and
// writes them out as a sibling `.rt.a` for each AOT compile, so every
// `compile_file(... emit=Executable)` skips the runtime build step
// entirely. End-to-end test wall-clock stays at the old "pre-built
// object" level while the runtime itself is now Rust
// (`compiler/runtime/toylang_rt/`) rather than C.
//
// The runtime is `no_std` and dependency-free, so this is a bare
// `rustc --crate-type staticlib` invocation — no cargo, no target-dir
// plumbing, exactly the shape of the old `cc -c` call it replaces.
// `--cfg toylang_rt_standalone` turns on the pieces that only a final
// artifact needs (the panic handler, the malloc-backed global
// allocator, and the `rust_eh_personality` stub); the same source
// compiled as an rlib for the compiler's JIT leaves those to the host
// binary.
//
// rustc output is deterministic, but absolute paths embedded in the
// debug info would not be; `--remap-path-prefix` pins the runtime's
// path so the archive bytes (and therefore the linked binaries) are
// reproducible. Cargo invalidates this build script when the runtime
// source changes (the `rerun-if-changed` line below), so editing
// `lib.rs` still triggers a rebuild.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let runtime_src = "runtime/toylang_rt/src/lib.rs";
    println!("cargo:rerun-if-changed={runtime_src}");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR set by cargo"));
    let archive_path = out_dir.join("libtoylang_rt.a");

    // `RUSTC` is set by cargo for build scripts. The archive must
    // match the host platform, which is what `TARGET` names (this
    // toolchain has no cross-compilation support; the flags mirror
    // the codegen's assumptions).
    let rustc = env::var("RUSTC").expect("RUSTC set by cargo");
    let mut cmd = Command::new(&rustc);
    cmd.args([
        "--edition",
        "2024",
        "--crate-type",
        "staticlib",
        "--cfg",
        "toylang_rt_standalone",
        "-C",
        "opt-level=2",
        "-C",
        "panic=abort",
        "--remap-path-prefix",
    ]);
    let workspace_root = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set");
    cmd.arg(format!("{workspace_root}=<workspace>"));
    let target = env::var("TARGET").unwrap_or_default();
    if !target.is_empty() {
        cmd.arg("--target").arg(&target);
    }
    cmd.arg(runtime_src).arg("-o").arg(&archive_path);

    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn rustc for runtime build: {e}"));
    if !status.success() {
        panic!("rustc exited with {status} while building {runtime_src}");
    }
}
