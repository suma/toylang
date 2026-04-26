//! Compares JIT and tree-walking interpreter throughput on workloads the
//! JIT supports today (numeric / boolean code with control flow). Each
//! workload is parsed and type-checked once outside the timed region, so
//! the measurements reflect the cost of `execute_program` only — which
//! still includes cranelift compilation in the JIT case, so plan for some
//! one-shot compile overhead in shorter benches.
//!
//! Run: `cargo bench --bench jit_bench` (jit feature is on by default).

use std::env;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use frontend::Parser;
use interpreter::{check_typing, execute_program};
use string_interner::DefaultStringInterner;

fn prepare(source: &str) -> (frontend::ast::Program, DefaultStringInterner) {
    let mut interner = DefaultStringInterner::with_capacity(256);
    let mut parser = Parser::new(source, &mut interner);
    let mut program = parser.parse_program().expect("parse");
    check_typing(&mut program, &mut interner, Some(source), Some("bench.t"))
        .expect("typecheck");
    (program, interner)
}

fn run_modes(c: &mut Criterion, group_name: &str, source: &str) {
    let (program, interner) = prepare(source);

    let mut group = c.benchmark_group(group_name);

    group.bench_function("interpreter", |b| {
        env::remove_var("INTERPRETER_JIT");
        b.iter(|| {
            execute_program(
                black_box(&program),
                black_box(&interner),
                None,
                None,
            )
        });
    });

    #[cfg(feature = "jit")]
    group.bench_function("jit", |b| {
        env::set_var("INTERPRETER_JIT", "1");
        b.iter(|| {
            execute_program(
                black_box(&program),
                black_box(&interner),
                None,
                None,
            )
        });
        env::remove_var("INTERPRETER_JIT");
    });

    group.finish();
}

fn fib_recursive(c: &mut Criterion) {
    // fib(20) is large enough that recursive call overhead dominates; the
    // JIT should win clearly here even after one-shot compile cost.
    let src = r#"
fn fib(n: u64) -> u64 {
    if n <= 1u64 {
        n
    } else {
        fib(n - 1u64) + fib(n - 2u64)
    }
}

fn main() -> u64 {
    fib(20u64)
}
"#;
    run_modes(c, "fib_recursive_20", src);
}

fn loop_sum(c: &mut Criterion) {
    // A plain numeric loop: 100_000 iterations with a single iadd. Highly
    // amenable to native codegen.
    let src = r#"
fn sum_to(n: u64) -> u64 {
    var acc: u64 = 0u64
    var i: u64 = 0u64
    while i < n {
        acc = acc + i
        i = i + 1u64
    }
    acc
}

fn main() -> u64 {
    sum_to(100000u64)
}
"#;
    run_modes(c, "loop_sum_100k", src);
}

fn fib_iterative(c: &mut Criterion) {
    // Iterative fibonacci — exercises a tight while loop with two `var`
    // updates per iteration. Larger n means the per-call cranelift compile
    // is amortized across many iterations.
    let src = r#"
fn fib_iter(n: u64) -> u64 {
    var a: u64 = 0u64
    var b: u64 = 1u64
    var i: u64 = 0u64
    while i < n {
        val tmp: u64 = a + b
        a = b
        b = tmp
        i = i + 1u64
    }
    a
}

fn main() -> u64 {
    fib_iter(50000u64)
}
"#;
    run_modes(c, "fib_iter_50k", src);
}

criterion_group!(benches, fib_recursive, loop_sum, fib_iterative);
criterion_main!(benches);
