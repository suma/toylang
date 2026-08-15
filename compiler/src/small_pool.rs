//! A deliberately small rayon pool for the compiler's internal
//! parallelism.
//!
//! Rayon's default pool takes one thread per core. That is the right
//! call for a process that compiles one large thing; it is the wrong
//! call here for two reasons:
//!
//! 1. **The pool is built per process.** `cargo nextest` runs one test
//!    per process, so every test that compiles a program pays to spawn
//!    the whole pool, use it for a few milliseconds, and tear it down.
//!    Measured on a 20-core machine, compiling a one-line program cost
//!    237 ms of CPU with the default pool and 199 ms with four threads
//!    — same wall time, 16% less work.
//! 2. **Toylang programs are small.** Across the whole example suite
//!    the largest program compiles in 120 ms single-threaded, 90 ms on
//!    four threads and 80 ms on twenty. Past four threads the curve is
//!    flat, so the extra workers only add scheduling.
//!
//! When many compiles run at once (the test suite, or a build server),
//! trading 10 ms of latency on the biggest program for 16% less CPU per
//! compile is the better end of the deal — the machine is saturated by
//! the concurrency, not by any single compile.
use std::sync::OnceLock;

/// Upper bound on worker threads. See the module docs for the
/// measurements behind the number.
const MAX_THREADS: usize = 4;

/// The process-wide pool. Built on first use; `install` on it runs a
/// closure with its workers instead of rayon's global pool.
pub(crate) fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(MAX_THREADS);
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("toy-codegen-{i}"))
            .build()
            // A pool that cannot be built is not worth aborting a
            // compile over: fall back to one that runs everything on
            // the calling thread.
            .unwrap_or_else(|_| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(1)
                    .build()
                    .expect("single-threaded rayon pool")
            })
    })
}
