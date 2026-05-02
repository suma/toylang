// Pre-compile the toylang C runtime once at compiler-build time and
// stash the resulting object next to OUT_DIR. The link driver
// (`src/driver.rs`) loads those bytes via `include_bytes!` and writes
// them out as a sibling `.rt.o` for each AOT compile, so every
// `compile_file(... emit=Executable)` skips the C compilation step
// entirely. End-to-end test wall-clock drops from ~5.7s per test
// (where `cc` rebuilds toylang_rt.c every time) to a quick two-object
// link.
//
// Cargo invalidates this build script when the runtime source
// changes (the `rerun-if-changed` line below), so editing
// toylang_rt.c still triggers a rebuild.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let runtime_src = "runtime/toylang_rt.c";
    println!("cargo:rerun-if-changed={runtime_src}");
    println!("cargo:rerun-if-changed=build.rs");
    // Allow callers to override the C compiler used at build time.
    println!("cargo:rerun-if-env-changed=CC");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR set by cargo"));
    let object_path = out_dir.join("toylang_rt.o");

    // Use the `cc` build helper so platform flags / sysroot matches
    // what cargo would otherwise pass. We could call `cc::Build` for
    // the heavy machinery; a single-file compile is straightforward
    // enough that invoking the resolved tool directly keeps the
    // build script trivial and avoids dragging cc's many feature
    // gates into the dep graph any further than necessary.
    let tool = cc::Build::new().get_compiler();
    let mut cmd = Command::new(tool.path());
    for (key, val) in tool.env() {
        cmd.env(key, val);
    }
    cmd.args(tool.args());
    // Object compilation flags. `-O2` matches what `cc` would do for
    // a release-style build; the runtime is small so the cost is
    // negligible at build time and the result is faster at run time.
    cmd.args(["-c", "-O2", "-fPIC", runtime_src, "-o"]).arg(&object_path);

    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn C compiler for runtime build: {e}"));
    if !status.success() {
        panic!(
            "C compiler exited with {status} while building {runtime_src}",
        );
    }
}
