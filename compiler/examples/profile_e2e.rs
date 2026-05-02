//! Standalone profiler for the AOT compile-and-run pipeline.
//! Splits each iteration into three phases — object emit, link,
//! and binary spawn — and prints per-phase wall clock so the
//! dominant cost is obvious at a glance.
//!
//! Run with:
//!   cargo run --release -p compiler --example profile_e2e
//!
//! Typical Apple Silicon output (release build):
//!   trivial: object  ~3 ms, link ~45 ms, spawn ~250 ms
//!       fib: object  ~3 ms, link ~45 ms, spawn ~200 ms
//!  struct_ret: object  ~3 ms, link ~45 ms, spawn ~270 ms
//!
//! `spawn` is dominated by the macOS first-execve cost (Gatekeeper
//! / xprotect / dyld initial scan) which ad-hoc-signed *and*
//! signature-stripped fresh binaries both pay (~200 - 300 ms each).
//! Subsequent runs of the same binary hit the kernel cache and
//! drop to <5 ms. The compiler test suite (`compiler/tests/e2e.rs`)
//! produces 193 unique binaries, so each first-run cost stacks
//! up to ~60 s wall clock; see
//! `compiler/tests/e2e_batched.rs` for the consolidation
//! prototype that amortises the spawn step across many sub-tests.

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use compiler::{compile_file, CompilerOptions, EmitKind};

fn main() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let core = workspace.join("core");

    let sources: &[(&str, &str)] = &[
        ("trivial", "fn main() -> u64 { 42u64 }\n"),
        (
            "fib",
            "fn fib(n: u64) -> u64 { if n <= 1u64 { n } else { fib(n - 1u64) + fib(n - 2u64) } }\nfn main() -> u64 { fib(8u64) }\n",
        ),
        (
            "struct_ret",
            "struct P { x: u64, y: u64 }\nfn make() -> P { P { x: 3u64, y: 4u64 } }\nfn main() -> u64 { val p = make()\n p.x + p.y }\n",
        ),
    ];

    let n_iters = 5;
    for (name, src) in sources {
        let mut total_obj = std::time::Duration::ZERO;
        let mut total_link = std::time::Duration::ZERO;
        let mut total_run = std::time::Duration::ZERO;
        for i in 0..n_iters {
            let src_path = std::env::temp_dir().join(format!("prof_{name}_{i}.t"));
            let obj_path = std::env::temp_dir().join(format!("prof_{name}_{i}.o"));
            let exe_path = std::env::temp_dir().join(format!("prof_{name}_{i}"));
            std::fs::write(&src_path, src).unwrap();

            // Phase 1: compile to object only (no linking).
            let obj_opts = CompilerOptions {
                input: src_path.clone(),
                output: Some(obj_path.clone()),
                emit: EmitKind::Object,
                verbose: false,
                release: false,
                core_modules_dir: Some(core.clone()),
            };
            let t_obj0 = Instant::now();
            compile_file(&obj_opts).expect("compile object");
            let obj_dur = t_obj0.elapsed();
            total_obj += obj_dur;

            // Phase 2: full executable (compile + link). The
            // difference compile_file_exec - compile_file_obj
            // is the link cost (cc invocation, code-signing, etc.).
            let exe_opts = CompilerOptions {
                input: src_path.clone(),
                output: Some(exe_path.clone()),
                emit: EmitKind::Executable,
                verbose: false,
                release: false,
                core_modules_dir: Some(core.clone()),
            };
            let t_exe0 = Instant::now();
            compile_file(&exe_opts).expect("compile exec");
            let exe_dur = t_exe0.elapsed();
            total_link += exe_dur.saturating_sub(obj_dur);

            // Phase 3: process spawn + execution.
            let t2 = Instant::now();
            let _ = Command::new(&exe_path).status().unwrap();
            total_run += t2.elapsed();

            let _ = std::fs::remove_file(&src_path);
            let _ = std::fs::remove_file(&obj_path);
            let _ = std::fs::remove_file(&exe_path);
        }
        let avg_obj = total_obj / n_iters;
        let avg_link = total_link / n_iters;
        let avg_run = total_run / n_iters;
        println!(
            "{name:>12}: object {avg_obj:>8.2?}, link {avg_link:>8.2?}, spawn {avg_run:>8.2?}",
        );
    }
}
