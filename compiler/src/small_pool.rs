//! Rayon pools for the compiler's internal parallelism, sized by the
//! work rather than by the machine.
//!
//! Rayon's default pool takes one thread per core. That is the wrong
//! call for most compiles here:
//!
//! 1. **The pool is built per process.** `cargo nextest` runs one test
//!    per process, so every test that compiles a program pays to spawn
//!    the whole pool, use it for a few milliseconds, and tear it down.
//!    Measured on a 20-core machine, compiling a one-line program cost
//!    237 ms of CPU with the default pool and 199 ms with four threads
//!    — same wall time, 16% less work.
//! 2. **Most toylang programs are small.** Every example lowers to at
//!    most ~5k IR instructions (~11 ms of codegen CPU); past four
//!    threads there is nothing left to split.
//!
//! A large program is the opposite case (CODEGEN-THREADS):
//! `poc/logsearch` is ~58k instructions and ~120 ms of codegen CPU, so
//! four threads left a 10-core machine mostly idle — `compile_functions`
//! measured 29.6 ms on 4 threads, 17.7 on 10, 12.7 on 20 (2026-10-05,
//! largest function first, see `codegen::build_object_module`). So there are
//! two pools: the small one every compile has always used, and one per
//! core that a compile reaches for only when its work is at least
//! [`LARGE_WORK_INSTS`].
//!
//! `TOYLANG_CODEGEN_THREADS=N` pins the thread count instead, for
//! measuring.
use std::sync::OnceLock;

/// Upper bound on worker threads for an ordinary compile. See the
/// module docs for the measurements behind the number.
const MAX_THREADS: usize = 4;

/// IR instructions from which a compile gets a thread per core. Codegen
/// costs ~2 µs per instruction, so this is about 3 ms of work for each
/// of the small pool's four threads: below it a wider pool only adds
/// threads to spawn.
pub(crate) const LARGE_WORK_INSTS: u64 = 6_000;

fn cores() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

fn build(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("toy-codegen-{i}"))
        .build()
        // A pool that cannot be built is not worth aborting a compile
        // over: fall back to one that runs everything on the calling
        // thread.
        .unwrap_or_else(|_| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .expect("single-threaded rayon pool")
        })
}

/// The pool for a compile whose functions add up to `work` IR
/// instructions. Each pool is built on first use and lives for the
/// process; `install` on it runs a closure with its workers instead of
/// rayon's global pool.
pub(crate) fn pool_for(work: u64) -> &'static rayon::ThreadPool {
    static PINNED: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    static SMALL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    static LARGE: OnceLock<rayon::ThreadPool> = OnceLock::new();
    let pinned = PINNED.get_or_init(|| {
        std::env::var("TOYLANG_CODEGEN_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n > 0)
            .map(build)
    });
    if let Some(pool) = pinned {
        return pool;
    }
    if work >= LARGE_WORK_INSTS && cores() > MAX_THREADS {
        LARGE.get_or_init(|| build(cores()))
    } else {
        SMALL.get_or_init(|| build(cores().min(MAX_THREADS)))
    }
}
