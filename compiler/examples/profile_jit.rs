//! Standalone profiler for the JIT compile pipeline. Measures the
//! per-test cost paid by `compiler/tests/e2e_batched.rs` and
//! breaks it into:
//!   - parse (`session.parse_program`)
//!   - check_typing_with_core_modules (auto-load + type check)
//!   - rest of compile (lower + JIT codegen + finalize)
//!   - the same total without `core_modules_dir` (fast-path
//!     baseline used by the lazy-core trick in
//!     `e2e_batched.rs::compile_to_jit_lazy_core`)
//!
//! Run with:
//!
//!   cargo run -p compiler --example profile_jit
//!
//! Use `--release` to estimate cranelift-codegen production cost
//! (debug builds run cranelift unoptimized too, so the numbers
//! are dominated by Rust's debug overhead).

use std::path::PathBuf;
use std::time::{Duration, Instant};

fn core_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("core")
}

fn main() {
    let core = core_dir();
    let source = "fn main() -> u64 { 42u64 }";

    let opts_full = compiler::CompilerOptions {
        input: PathBuf::from("<jit>"),
        output: None,
        emit: compiler::EmitKind::Executable,
        verbose: false,
        release: false,
        core_modules_dir: Some(core.clone()),
    };
    let opts_no_core = compiler::CompilerOptions {
        core_modules_dir: None,
        ..opts_full.clone()
    };

    // Warm up file-system caches and lazy statics.
    for _ in 0..3 {
        let _ = compiler::compile_to_jit_main_with_options(source, &opts_full);
    }

    const N: u32 = 30;

    let mut t_no_core = Duration::ZERO;
    for _ in 0..N {
        let t = Instant::now();
        let _ = compiler::compile_to_jit_main_with_options(source, &opts_no_core).expect("compile");
        t_no_core += t.elapsed();
    }

    let mut t_parse = Duration::ZERO;
    let mut t_check = Duration::ZERO;
    let mut t_total = Duration::ZERO;
    for _ in 0..N {
        let t0 = Instant::now();
        let mut session = compiler_core::CompilerSession::new();

        let t = Instant::now();
        let mut program = session.parse_program(source).expect("parse");
        t_parse += t.elapsed();

        let t = Instant::now();
        interpreter::check_typing_with_core_modules(
            &mut program,
            session.string_interner_mut(),
            Some(source),
            None,
            Some(&core),
        )
        .expect("type check");
        t_check += t.elapsed();

        // Approximate the rest of the pipeline by running the
        // public entry; subtract parse + check above to get the
        // codegen contribution.
        let _ = compiler::compile_to_jit_main_with_options(source, &opts_full).expect("compile");
        t_total += t0.elapsed();
    }

    let parse = t_parse / N;
    let check = t_check / N;
    let total = t_total / N;
    let codegen = total.saturating_sub(parse + check);
    let no_core = t_no_core / N;
    println!("avg per phase (N={N}):");
    println!("  parse              : {:>10?}", parse);
    println!("  check_typing+core  : {:>10?}", check);
    println!("  rest (lower+JIT)   : {:>10?}", codegen);
    println!("  total (with core)  : {:>10?}", total);
    println!("  total (no core)    : {:>10?}  <- lazy-core fast path baseline", no_core);
}
