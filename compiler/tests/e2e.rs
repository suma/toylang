//! End-to-end tests for the AOT compiler.
//!
//! Each test compiles a tiny toylang program to a real executable, runs
//! it, and asserts the exit code. The tests are skipped on platforms
//! where we don't have a system C compiler (the linker driver shells out
//! to `cc`); on Unix this is essentially always available.
//!
//! These tests are slow because they invoke `cc`. They are marked
//! `#[ignore]` only when explicitly requested via the `COMPILER_E2E=skip`
//! environment variable so CI can opt out for sandboxed runners.

use std::path::PathBuf;
use std::process::{Command, Output};

use compiler::{compile_file, CompilerOptions, EmitKind};

fn skip_e2e() -> bool {
    std::env::var("COMPILER_E2E").map(|v| v == "skip").unwrap_or(false)
}

fn unique_path(stem: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    p.push(format!("toy_compiler_test_{stem}_{pid}_{nanos}"));
    p
}

/// Compile `source` to a unique executable path, run it, and return the
/// exit code (or panic on link / spawn failure).
fn compile_and_run(source: &str, stem: &str) -> i32 {
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let exe_path = unique_path(stem);
    let options = CompilerOptions {
        input: src_path.clone(),
        output: Some(exe_path.clone()),
        emit: EmitKind::Executable,
        verbose: false,
        release: false,
    };
    compile_file(&options).expect("compile_file failed");
    let status = Command::new(&exe_path)
        .status()
        .expect("spawn compiled executable");
    let code = status.code().expect("process produced no exit code");
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);
    code
}

#[test]
fn returns_literal_exit_code() {
    if skip_e2e() {
        return;
    }
    let code = compile_and_run("fn main() -> u64 { 42u64 }\n", "literal");
    assert_eq!(code, 42);
}

#[test]
fn fibonacci_recursive() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn fib(n: u64) -> u64 {
            if n <= 1u64 { n } else { fib(n - 1u64) + fib(n - 2u64) }
        }
        fn main() -> u64 { fib(8u64) }
    "#;
    let code = compile_and_run(src, "fib");
    assert_eq!(code, 21);
}

#[test]
fn for_loop_sum() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            var sum = 0u64
            for i in 0u64..10u64 {
                sum = sum + i
            }
            sum
        }
    "#;
    let code = compile_and_run(src, "loop_sum");
    assert_eq!(code, 45);
}

#[test]
fn while_loop_with_break() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            var i = 0u64
            while i < 100u64 {
                if i == 7u64 { break }
                i = i + 1u64
            }
            i
        }
    "#;
    let code = compile_and_run(src, "while_break");
    assert_eq!(code, 7);
}

#[test]
fn if_elif_else_chain() {
    if skip_e2e() {
        return;
    }
    // POSIX shells truncate exit codes to the low 8 bits, so the values
    // we compare on are intentionally < 256.
    let src = r#"
        fn classify(n: u64) -> u64 {
            if n == 0u64 { 11u64 }
            elif n == 1u64 { 22u64 }
            elif n == 2u64 { 33u64 }
            else { 44u64 }
        }
        fn main() -> u64 { classify(2u64) }
    "#;
    let code = compile_and_run(src, "elif");
    assert_eq!(code, 33);
}

#[test]
fn signed_arithmetic() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> i64 {
            val a: i64 = -5i64
            val b: i64 = 12i64
            (a + b) * 3i64
        }
    "#;
    let code = compile_and_run(src, "signed");
    // (-5 + 12) * 3 = 21
    assert_eq!(code, 21);
}

#[test]
fn short_circuit_and_or() {
    if skip_e2e() {
        return;
    }
    // `&&` short-circuits: if `false`, the right operand mustn't run. We
    // pass it a divide-by-zero that would trap if it did. The compiled
    // code should evaluate to false and return 7.
    let src = r#"
        fn main() -> u64 {
            val ok: bool = false && (1u64 / 0u64 == 0u64)
            if ok { 0u64 } else { 7u64 }
        }
    "#;
    let code = compile_and_run(src, "short_and");
    assert_eq!(code, 7);
}

/// Compile, run, and capture both stdout and the exit status. Useful for
/// the panic / assert tests below where we care about the printed
/// message in addition to the exit code.
fn compile_and_capture(source: &str, stem: &str) -> Output {
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let exe_path = unique_path(stem);
    let options = CompilerOptions {
        input: src_path.clone(),
        output: Some(exe_path.clone()),
        emit: EmitKind::Executable,
        verbose: false,
        release: false,
    };
    compile_file(&options).expect("compile_file failed");
    let output = Command::new(&exe_path).output().expect("spawn binary");
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);
    output
}

#[test]
fn panic_prints_message_and_exits_one() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            panic("kaboom")
        }
    "#;
    let out = compile_and_capture(src, "panic_basic");
    assert_eq!(out.status.code(), Some(1), "panic should exit with status 1");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Compiler routes panic through libc `puts`, which writes to stdout
    // (the interpreter writes to stderr — documented divergence).
    assert!(
        stdout.contains("panic: kaboom"),
        "panic output should contain the message; got stdout={stdout:?}"
    );
}

#[test]
fn assert_passes_through_when_true() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            assert(1u64 + 1u64 == 2u64, "math broke")
            42u64
        }
    "#;
    let out = compile_and_capture(src, "assert_pass");
    assert_eq!(out.status.code(), Some(42));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("panic"),
        "assert(true) should produce no panic output; got stdout={stdout:?}"
    );
}

#[test]
fn assert_fires_on_false() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn divide(a: i64, b: i64) -> i64 {
            assert(b != 0i64, "divide: divisor must be non-zero")
            a / b
        }
        fn main() -> u64 {
            val r = divide(10i64, 0i64)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "assert_fail");
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("panic: divide: divisor must be non-zero"),
        "failed assert should print the message; got stdout={stdout:?}"
    );
}

#[test]
fn panic_in_else_branch_compiles() {
    if skip_e2e() {
        return;
    }
    // `panic` must be usable in an expression position thanks to its
    // bottom-type semantics on the front-end side; the compiler honours
    // that by terminating the panic block, so the if-expression still
    // type-checks even though one arm diverges.
    let src = r#"
        fn safe_divide(a: i64, b: i64) -> i64 {
            if b == 0i64 { panic("division by zero") } else { a / b }
        }
        fn main() -> u64 {
            val r = safe_divide(10i64, 2i64)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "panic_else");
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn println_string_literal() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            println("hello, world")
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_str");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hello, world\n");
}

#[test]
fn print_without_newline() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            print("foo")
            print("bar")
            println("!")
            0u64
        }
    "#;
    let out = compile_and_capture(src, "print_concat");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "foobar!\n");
}

#[test]
fn println_numeric_values() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            println(42u64)
            println(-13i64)
            println(0u64)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_nums");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "42\n-13\n0\n"
    );
}

#[test]
fn println_bool_values() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            println(true)
            println(false)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_bool");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "true\nfalse\n");
}

#[test]
fn print_inside_loop() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            for i in 0u64..3u64 {
                print("i=")
                println(i)
            }
            0u64
        }
    "#;
    let out = compile_and_capture(src, "print_loop");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "i=0\ni=1\ni=2\n"
    );
}

#[test]
fn struct_literal_and_field_read() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        struct Point { x: i64, y: i64 }

        fn main() -> u64 {
            val p = Point { x: 3i64, y: 4i64 }
            print("x=")
            println(p.x)
            print("y=")
            println(p.y)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "struct_read");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "x=3\ny=4\n");
}

#[test]
fn struct_field_write() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        struct Counter { n: u64, label: bool }

        fn main() -> u64 {
            var c = Counter { n: 0u64, label: false }
            c.n = c.n + 5u64
            c.label = true
            print("n=")
            println(c.n)
            print("label=")
            println(c.label)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "struct_write");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "n=5\nlabel=true\n"
    );
}

#[test]
fn struct_field_in_arithmetic() {
    if skip_e2e() {
        return;
    }
    // Treat struct fields as ordinary lvalues / rvalues in expressions.
    let src = r#"
        struct Pair { a: u64, b: u64 }

        fn main() -> u64 {
            val p = Pair { a: 7u64, b: 6u64 }
            val sum = p.a + p.b
            sum
        }
    "#;
    let out = compile_and_capture(src, "struct_arith");
    assert_eq!(out.status.code(), Some(13));
}

#[test]
fn struct_in_loop_accumulator() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        struct Acc { total: u64 }

        fn main() -> u64 {
            var a = Acc { total: 0u64 }
            for i in 1u64..5u64 {
                a.total = a.total + i
            }
            a.total
        }
    "#;
    let out = compile_and_capture(src, "struct_loop");
    // 1+2+3+4 = 10
    assert_eq!(out.status.code(), Some(10));
}

#[test]
fn cast_i64_to_u64_identity() {
    if skip_e2e() {
        return;
    }
    // i64↔u64 share the same bit pattern. Casting -1i64 to u64 should
    // surface the all-ones unsigned value modulo `& 0xff` truncation
    // applied by the OS to exit codes.
    let src = r#"
        fn main() -> u64 {
            val a: i64 = -1i64
            val b: u64 = a as u64
            b
        }
    "#;
    let out = compile_and_capture(src, "cast_neg1");
    // u64::MAX & 0xff == 0xff
    assert_eq!(out.status.code(), Some(0xff));
}

#[test]
fn cast_round_trip_through_f64() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            val n: u64 = 42u64
            val f: f64 = n as f64
            val back: u64 = f as u64
            back
        }
    "#;
    let out = compile_and_capture(src, "cast_round");
    assert_eq!(out.status.code(), Some(42));
}

#[test]
fn cast_float_to_int_truncates() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            val pi: f64 = 3.9f64
            val i: u64 = pi as u64
            i
        }
    "#;
    let out = compile_and_capture(src, "cast_trunc");
    // f→u uses cranelift's saturating truncation, matching Rust's `as`.
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn f64_arithmetic() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            val a: f64 = 1.5f64
            val b: f64 = 2.5f64
            val sum: f64 = a + b
            print("a+b = ")
            println(sum)
            val prod: f64 = a * b
            print("a*b = ")
            println(prod)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "f64_arith");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "a+b = 4.0\na*b = 3.75\n"
    );
}

#[test]
fn f64_unary_neg_and_compare() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            val x: f64 = 3.0f64
            val y: f64 = -x
            print("y = ")
            println(y)
            if y < 0.0f64 { 7u64 } else { 0u64 }
        }
    "#;
    let out = compile_and_capture(src, "f64_neg");
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "y = -3.0\n");
}

#[test]
fn f64_function_call() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn area(r: f64) -> f64 {
            r * r * 3.14159f64
        }
        fn main() -> u64 {
            val a: f64 = area(2.0f64)
            print("area(2.0) = ")
            println(a)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "f64_call");
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    // 2*2*3.14159 = 12.56636. printf %g typically renders as "12.5664"
    // but exact formatting varies; check the prefix.
    assert!(
        stdout.starts_with("area(2.0) = 12.566"),
        "unexpected stdout: {stdout:?}"
    );
}

#[test]
fn struct_returned_from_function() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        struct Point { x: i64, y: i64 }
        fn make(x: i64, y: i64) -> Point {
            Point { x: x, y: y }
        }
        fn main() -> u64 {
            val p = make(3i64, 4i64)
            print("p.x=")
            println(p.x)
            print("p.y=")
            println(p.y)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "struct_ret");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "p.x=3\np.y=4\n");
}

#[test]
fn struct_passed_to_function() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        struct Point { x: i64, y: i64 }
        fn dist_sq(p: Point) -> i64 { p.x * p.x + p.y * p.y }
        fn main() -> u64 {
            val p = Point { x: 3i64, y: 4i64 }
            val d = dist_sq(p)
            d as u64
        }
    "#;
    let out = compile_and_capture(src, "struct_param");
    assert_eq!(out.status.code(), Some(25));
}

#[test]
fn struct_boundary_round_trip() {
    if skip_e2e() {
        return;
    }
    // Struct flows in and out of functions; field arithmetic happens
    // across calls. This exercises the multi-arg-multi-result codegen
    // path end to end.
    let src = r#"
        struct Point { x: i64, y: i64 }
        fn add(a: Point, b: Point) -> Point {
            Point { x: a.x + b.x, y: a.y + b.y }
        }
        fn main() -> u64 {
            val p = Point { x: 10i64, y: 20i64 }
            val q = Point { x: 1i64, y: 2i64 }
            val sum = add(p, q)
            print("sum.x=")
            println(sum.x)
            print("sum.y=")
            println(sum.y)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "struct_round");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "sum.x=11\nsum.y=22\n");
}

#[test]
fn struct_explicit_return() {
    if skip_e2e() {
        return;
    }
    // `return p` where p is a struct binding should expand into a
    // multi-value return.
    let src = r#"
        struct Pair { a: u64, b: u64 }
        fn make(seed: u64) -> Pair {
            val p = Pair { a: seed, b: seed + 1u64 }
            return p
        }
        fn main() -> u64 {
            val p = make(7u64)
            p.a + p.b
        }
    "#;
    let out = compile_and_capture(src, "struct_explicit_ret");
    // 7 + 8 = 15
    assert_eq!(out.status.code(), Some(15));
}

#[test]
fn tuple_literal_and_access() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            val pair = (3u64, 4u64)
            print("a=")
            println(pair.0)
            print("b=")
            println(pair.1)
            pair.0 + pair.1
        }
    "#;
    let out = compile_and_capture(src, "tuple_basic");
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "a=3\nb=4\n");
}

#[test]
fn tuple_destructuring() {
    if skip_e2e() {
        return;
    }
    // The parser desugars `val (x, y) = (10, 20)` into
    // `val tmp = (10, 20); val x = tmp.0; val y = tmp.1`. The compiler
    // needs to handle the tmp binding (tuple literal rhs) and the
    // subsequent .0 / .1 accesses (tuple-access rhs of a scalar val).
    let src = r#"
        fn main() -> u64 {
            val (a, b) = (40u64, 2u64)
            a + b
        }
    "#;
    let out = compile_and_capture(src, "tuple_destruct");
    assert_eq!(out.status.code(), Some(42));
}

#[test]
fn tuple_element_assignment() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            var t = (0u64, 0u64, 0u64)
            t.0 = 1u64
            t.1 = 2u64
            t.2 = 3u64
            t.0 + t.1 + t.2
        }
    "#;
    let out = compile_and_capture(src, "tuple_assign");
    assert_eq!(out.status.code(), Some(6));
}

#[test]
fn tuple_mixed_types() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            val t = (10u64, true, -5i64)
            print("first=")
            println(t.0)
            print("flag=")
            println(t.1)
            print("signed=")
            println(t.2)
            t.0
        }
    "#;
    let out = compile_and_capture(src, "tuple_mixed");
    assert_eq!(out.status.code(), Some(10));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "first=10\nflag=true\nsigned=-5\n"
    );
}

#[test]
fn top_level_const_literal() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        const MAX: u64 = 100u64

        fn main() -> u64 {
            print("max=")
            println(MAX)
            MAX
        }
    "#;
    let out = compile_and_capture(src, "const_literal");
    // 100 & 0xff = 100
    assert_eq!(out.status.code(), Some(100));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "max=100\n");
}

#[test]
fn top_level_const_arithmetic_fold() {
    if skip_e2e() {
        return;
    }
    // `TWO_PI` references an earlier const and applies a binary op;
    // the compiler must fold both at compile time before the
    // function body sees the use site.
    let src = r#"
        const PI: f64 = 3.14f64
        const TWO_PI: f64 = PI + PI

        fn main() -> u64 {
            print("two_pi=")
            println(TWO_PI)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "const_fold");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "two_pi=6.28\n");
}

#[test]
fn dbc_requires_passes() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
            ensures result * b == a
        {
            a / b
        }

        fn main() -> u64 {
            val x: i64 = divide(10i64, 2i64)
            x as u64
        }
    "#;
    let out = compile_and_capture(src, "dbc_pass");
    assert_eq!(out.status.code(), Some(5));
}

#[test]
fn dbc_requires_violation_panics() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
        {
            a / b
        }

        fn main() -> u64 {
            val x: i64 = divide(10i64, 0i64)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "dbc_requires_fail");
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("requires violation"),
        "expected 'requires violation' in stdout, got: {stdout:?}"
    );
}

#[test]
fn dbc_ensures_violation_panics() {
    if skip_e2e() {
        return;
    }
    // `ensures result > 0i64` is intentionally violated by returning
    // a non-positive value. The check fires after the body computes
    // the return value, so we should observe the panic immediately
    // after `divide` would have returned.
    let src = r#"
        fn always_negative() -> i64
            ensures result > 0i64
        {
            -1i64
        }

        fn main() -> u64 {
            val x: i64 = always_negative()
            0u64
        }
    "#;
    let out = compile_and_capture(src, "dbc_ensures_fail");
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ensures violation"),
        "expected 'ensures violation' in stdout, got: {stdout:?}"
    );
}

#[test]
fn release_flag_skips_requires_check() {
    if skip_e2e() {
        return;
    }
    // Without `--release` the requires check fires on the violation
    // path. With `--release` it is dropped, so the body executes
    // — we use a reachable side effect (printing) as evidence.
    let src = r#"
        fn check(x: i64) -> i64
            requires x > 0i64
        {
            print("ran with x=")
            println(x)
            x
        }

        fn main() -> u64 {
            val r: i64 = check(-1i64)
            r as u64
        }
    "#;
    // 1) checked build: panic, exit 1
    let src_path = unique_path("rel_chk.t");
    std::fs::write(&src_path, src).unwrap();
    let exe_chk = unique_path("rel_chk");
    let opts_chk = CompilerOptions {
        input: src_path.clone(),
        output: Some(exe_chk.clone()),
        emit: EmitKind::Executable,
        verbose: false,
        release: false,
    };
    compile_file(&opts_chk).expect("compile checked");
    let out_chk = Command::new(&exe_chk).output().expect("spawn checked");
    assert_eq!(out_chk.status.code(), Some(1));
    let _ = std::fs::remove_file(&exe_chk);
    // 2) release build: predicate gone, body runs and returns -1 (cast
    //    to u64 → 0xff... ; & 0xff = 0xff = 255).
    let exe_rel = unique_path("rel_rel");
    let opts_rel = CompilerOptions {
        input: src_path.clone(),
        output: Some(exe_rel.clone()),
        emit: EmitKind::Executable,
        verbose: false,
        release: true,
    };
    compile_file(&opts_rel).expect("compile release");
    let out_rel = Command::new(&exe_rel).output().expect("spawn release");
    assert_eq!(out_rel.status.code(), Some(0xff));
    assert!(
        String::from_utf8_lossy(&out_rel.stdout).contains("ran with x=-1"),
        "expected the checked-out body to actually execute under --release"
    );
    let _ = std::fs::remove_file(&exe_rel);
    let _ = std::fs::remove_file(&src_path);
}

#[test]
fn nested_struct_field_read_and_write() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        struct Inner { x: i64, y: i64 }
        struct Outer { inner: Inner, label: u64 }

        fn main() -> u64 {
            val o = Outer { inner: Inner { x: 3i64, y: 4i64 }, label: 42u64 }
            print("o.inner.x=")
            println(o.inner.x)
            print("o.inner.y=")
            println(o.inner.y)
            print("o.label=")
            println(o.label)
            var p = Outer { inner: Inner { x: 0i64, y: 0i64 }, label: 0u64 }
            p.inner.x = 7i64
            p.inner.y = 8i64
            p.label = 99u64
            print("p.inner.x+y=")
            println(p.inner.x + p.inner.y)
            p.label
        }
    "#;
    let out = compile_and_capture(src, "nested_field");
    assert_eq!(out.status.code(), Some(99));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "o.inner.x=3\no.inner.y=4\no.label=42\np.inner.x+y=15\n"
    );
}

#[test]
fn nested_struct_passed_through_function() {
    if skip_e2e() {
        return;
    }
    // Outer contains Inner; passing Outer through a function should
    // expand into 3 cranelift params (Inner.x, Inner.y, Outer.tag)
    // at the boundary, with field access still working on the
    // receiving side.
    let src = r#"
        struct Inner { x: i64, y: i64 }
        struct Outer { inner: Inner, tag: u64 }

        fn dist_sq(o: Outer) -> i64 {
            o.inner.x * o.inner.x + o.inner.y * o.inner.y
        }

        fn main() -> u64 {
            val o = Outer { inner: Inner { x: 3i64, y: 4i64 }, tag: 0u64 }
            val d = dist_sq(o)
            d as u64
        }
    "#;
    let out = compile_and_capture(src, "nested_param");
    assert_eq!(out.status.code(), Some(25));
}

#[test]
fn tuple_returned_from_function() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn make_pair(a: u64, b: u64) -> (u64, u64) {
            (a, b)
        }
        fn main() -> u64 {
            val pair = make_pair(10u64, 20u64)
            print("pair.0=")
            println(pair.0)
            print("pair.1=")
            println(pair.1)
            pair.0 + pair.1
        }
    "#;
    let out = compile_and_capture(src, "tuple_ret");
    assert_eq!(out.status.code(), Some(30));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "pair.0=10\npair.1=20\n");
}

#[test]
fn tuple_passed_to_function() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn sum(p: (u64, u64)) -> u64 { p.0 + p.1 }
        fn main() -> u64 {
            val pair = (4u64, 5u64)
            sum(pair)
        }
    "#;
    let out = compile_and_capture(src, "tuple_param");
    assert_eq!(out.status.code(), Some(9));
}

#[test]
fn tuple_round_trip_through_function() {
    if skip_e2e() {
        return;
    }
    // Tuple flows in and out of the same function — exercises both
    // multi-arg and multi-result codegen paths in one shot.
    let src = r#"
        fn swap(p: (u64, u64)) -> (u64, u64) {
            (p.1, p.0)
        }
        fn main() -> u64 {
            val orig = (3u64, 8u64)
            val swapped = swap(orig)
            print("0=")
            println(swapped.0)
            print("1=")
            println(swapped.1)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "tuple_round");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "0=8\n1=3\n");
}

#[test]
fn tuple_returning_call_into_destructure() {
    if skip_e2e() {
        return;
    }
    // The parser desugars `val (a, b) = f()` into
    // `val tmp = f(); val a = tmp.0; val b = tmp.1`. The compiler's
    // tuple-returning-call path produces the tmp binding, then the
    // existing tuple-element accesses pick up `a` and `b`.
    let src = r#"
        fn make() -> (i64, u64) {
            (-7i64, 42u64)
        }
        fn main() -> u64 {
            val (a, b) = make()
            print("a=")
            println(a)
            print("b=")
            println(b)
            b
        }
    "#;
    let out = compile_and_capture(src, "tuple_destruct_call");
    assert_eq!(out.status.code(), Some(42));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "a=-7\nb=42\n");
}

#[test]
fn emit_object_writes_o_file() {
    if skip_e2e() {
        return;
    }
    let src_path = unique_path("emit_obj.t");
    std::fs::write(&src_path, "fn main() -> u64 { 1u64 }\n").unwrap();
    let obj_path = unique_path("emit_obj.o");
    let options = CompilerOptions {
        input: src_path.clone(),
        output: Some(obj_path.clone()),
        emit: EmitKind::Object,
        verbose: false,
        release: false,
    };
    compile_file(&options).expect("compile_file failed");
    let metadata = std::fs::metadata(&obj_path).expect("object file exists");
    assert!(metadata.len() > 0, "object file should be non-empty");
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&obj_path);
}

#[test]
fn emit_ir_writes_compiler_ir() {
    // `--emit=ir` produces the compiler's mid-level IR (not Cranelift IR).
    // Sanity-check that the textual format mentions both the function
    // declaration form and the per-block label we use.
    if skip_e2e() {
        return;
    }
    let src_path = unique_path("emit_ir.t");
    std::fs::write(&src_path, "fn main() -> u64 { 99u64 }\n").unwrap();
    let ir_path = unique_path("emit_ir.ir");
    let options = CompilerOptions {
        input: src_path.clone(),
        output: Some(ir_path.clone()),
        emit: EmitKind::Ir,
        verbose: false,
        release: false,
    };
    compile_file(&options).expect("compile_file failed");
    let text = std::fs::read_to_string(&ir_path).expect("ir file exists");
    assert!(text.contains("export function main()"), "ir text should declare `main`: {text}");
    assert!(text.contains("bb0:"), "ir text should label the entry block: {text}");
    assert!(text.contains("ret %"), "ir text should end with a return: {text}");
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&ir_path);
}

#[test]
fn emit_clif_writes_cranelift_ir() {
    // `--emit=clif` produces Cranelift's textual IR, post-IR-lowering.
    if skip_e2e() {
        return;
    }
    let src_path = unique_path("emit_clif.t");
    std::fs::write(&src_path, "fn main() -> u64 { 7u64 }\n").unwrap();
    let clif_path = unique_path("emit_clif.clif");
    let options = CompilerOptions {
        input: src_path.clone(),
        output: Some(clif_path.clone()),
        emit: EmitKind::Clif,
        verbose: false,
        release: false,
    };
    compile_file(&options).expect("compile_file failed");
    let text = std::fs::read_to_string(&clif_path).expect("clif file exists");
    // Cranelift IR uses `function` keyword followed by the signature,
    // e.g. `function u0:0(...) ...` or `function %main(...)` depending
    // on naming. Either way the keyword is present.
    assert!(text.contains("function"), "clif text should mention `function`: {text}");
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&clif_path);
}

#[test]
fn println_struct_value() {
    if skip_e2e() {
        return;
    }
    // Field display order matches the interpreter: alphabetical by
    // name. We declare `x` then `y`, which already happens to be
    // alphabetical, so the output is `Point { x: 3, y: 4 }`.
    let src = r#"
        struct Point { x: i64, y: i64 }
        fn main() -> u64 {
            val p = Point { x: 3i64, y: 4i64 }
            println(p)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_struct");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Point { x: 3, y: 4 }\n",
    );
}

#[test]
fn println_struct_field_order_alphabetical() {
    if skip_e2e() {
        return;
    }
    // Declaration order is `b, a, c`; the print should reorder to `a, b, c`.
    let src = r#"
        struct Triple { b: u64, a: u64, c: u64 }
        fn main() -> u64 {
            val t = Triple { b: 2u64, a: 1u64, c: 3u64 }
            println(t)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_struct_alpha");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Triple { a: 1, b: 2, c: 3 }\n",
    );
}

#[test]
fn print_struct_no_trailing_newline() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        struct Pair { x: u64, y: u64 }
        fn main() -> u64 {
            val p = Pair { x: 7u64, y: 9u64 }
            print(p)
            print("!")
            0u64
        }
    "#;
    let out = compile_and_capture(src, "print_struct_no_nl");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Pair { x: 7, y: 9 }!",
    );
}

#[test]
fn println_nested_struct_value() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        struct Inner { v: u64 }
        struct Outer { inner: Inner, k: u64 }
        fn main() -> u64 {
            val o = Outer { inner: Inner { v: 42u64 }, k: 7u64 }
            println(o)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_nested");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Outer { inner: Inner { v: 42 }, k: 7 }\n",
    );
}

#[test]
fn println_struct_with_bool_field() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        struct Flag { ok: bool, n: u64 }
        fn main() -> u64 {
            val f = Flag { ok: true, n: 5u64 }
            println(f)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_struct_bool");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Flag { n: 5, ok: true }\n",
    );
}

#[test]
fn println_tuple_value() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            val t = (3u64, 4u64, 5u64)
            println(t)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_tuple");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "(3, 4, 5)\n",
    );
}

#[test]
fn println_tuple_pair_mixed_types() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            val t = (-7i64, true)
            println(t)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_tuple_pair");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "(-7, true)\n",
    );
}

#[test]
fn enum_unit_variant_match() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Color {
            Red,
            Green,
            Blue,
        }
        fn main() -> u64 {
            val c = Color::Green
            match c {
                Color::Red => 11u64,
                Color::Green => 22u64,
                Color::Blue => 33u64,
            }
        }
    "#;
    assert_eq!(compile_and_run(src, "enum_unit"), 22);
}

#[test]
fn enum_tuple_variant_with_one_payload() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Shape {
            Circle(i64),
            Point,
        }
        fn main() -> u64 {
            val s = Shape::Circle(7i64)
            val a: i64 = match s {
                Shape::Circle(r) => r * 2i64,
                Shape::Point => 0i64,
            }
            a as u64
        }
    "#;
    assert_eq!(compile_and_run(src, "enum_one_payload"), 14);
}

#[test]
fn enum_tuple_variant_with_multi_payload() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Shape {
            Circle(i64),
            Rect(i64, i64),
            Point,
        }
        fn main() -> u64 {
            val s = Shape::Rect(3i64, 7i64)
            val a: i64 = match s {
                Shape::Circle(r) => r,
                Shape::Rect(w, h) => w * h,
                Shape::Point => 0i64,
            }
            a as u64
        }
    "#;
    assert_eq!(compile_and_run(src, "enum_multi_payload"), 21);
}

#[test]
fn enum_match_with_wildcard() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Shape {
            Circle(i64),
            Rect(i64, i64),
            Point,
        }
        fn main() -> u64 {
            val s = Shape::Point
            val a: i64 = match s {
                Shape::Circle(r) => r,
                _ => 99i64,
            }
            a as u64
        }
    "#;
    assert_eq!(compile_and_run(src, "enum_wildcard"), 99);
}

#[test]
fn enum_match_discards_payload_with_underscore() {
    if skip_e2e() {
        return;
    }
    // Sub-pattern `_` discards the payload at that position. Useful when
    // we care which variant we have but not what's inside.
    let src = r#"
        enum Pair {
            One(u64),
            Two(u64, u64),
        }
        fn main() -> u64 {
            val p = Pair::Two(5u64, 6u64)
            match p {
                Pair::One(_) => 0u64,
                Pair::Two(_, b) => b,
            }
        }
    "#;
    assert_eq!(compile_and_run(src, "enum_discard_underscore"), 6);
}

#[test]
fn enum_with_bool_payload() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Maybe {
            Yes(bool),
            No,
        }
        fn main() -> u64 {
            val m = Maybe::Yes(true)
            match m {
                Maybe::Yes(b) => if b { 1u64 } else { 2u64 },
                Maybe::No => 99u64,
            }
        }
    "#;
    assert_eq!(compile_and_run(src, "enum_bool_payload"), 1);
}

#[test]
fn enum_match_used_inside_loop() {
    if skip_e2e() {
        return;
    }
    // Verify match works inside a loop body. We build the enum value
    // ahead of the match each iteration (the compiler MVP only allows
    // enum construction as the immediate rhs of `val` / `var`, so we
    // can't put the construction inside an `if` branch yet).
    let src = r#"
        enum Pick {
            Even,
            Odd,
        }
        fn payoff(n: u64) -> u64 {
            if n % 2u64 == 0u64 {
                val p = Pick::Even
                match p {
                    Pick::Even => 10u64,
                    Pick::Odd => 1u64,
                }
            } else {
                val p = Pick::Odd
                match p {
                    Pick::Even => 10u64,
                    Pick::Odd => 1u64,
                }
            }
        }
        fn main() -> u64 {
            var sum = 0u64
            for i in 0u64..5u64 {
                sum = sum + payoff(i)
            }
            sum
        }
    "#;
    // 0->Even=10, 1->Odd=1, 2->Even=10, 3->Odd=1, 4->Even=10 => 32
    assert_eq!(compile_and_run(src, "enum_in_loop"), 32);
}

#[test]
fn match_scalar_u64_with_literal_arms() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn classify(n: u64) -> u64 {
            match n {
                0u64 => 100u64,
                1u64 => 200u64,
                2u64 => 300u64,
                _ => 999u64,
            }
        }
        fn main() -> u64 {
            classify(2u64)
        }
    "#;
    assert_eq!(compile_and_run(src, "match_scalar_u64"), 300 & 0xff);
}

#[test]
fn match_scalar_u64_falls_through_to_wildcard() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn classify(n: u64) -> u64 {
            match n {
                0u64 => 100u64,
                1u64 => 200u64,
                _ => 7u64,
            }
        }
        fn main() -> u64 {
            classify(99u64)
        }
    "#;
    assert_eq!(compile_and_run(src, "match_scalar_default"), 7);
}

#[test]
fn match_scalar_bool_arms() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            val b = true
            match b {
                true => 11u64,
                false => 22u64,
            }
        }
    "#;
    assert_eq!(compile_and_run(src, "match_scalar_bool"), 11);
}

#[test]
fn match_scalar_i64_with_negative_literal() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn sign(n: i64) -> u64 {
            match n {
                -1i64 => 1u64,
                0i64 => 2u64,
                _ => 3u64,
            }
        }
        fn main() -> u64 {
            sign(-1i64)
        }
    "#;
    assert_eq!(compile_and_run(src, "match_scalar_i64_neg"), 1);
}

#[test]
fn match_variant_with_literal_subpattern() {
    if skip_e2e() {
        return;
    }
    // The first arm only matches `Circle(0i64)`; a non-zero radius
    // should fall through to the second arm that binds `r`.
    let src = r#"
        enum Shape {
            Circle(i64),
            Other,
        }
        fn main() -> u64 {
            val s = Shape::Circle(5i64)
            val a: i64 = match s {
                Shape::Circle(0i64) => 0i64,
                Shape::Circle(r) => r * 10i64,
                Shape::Other => -1i64,
            }
            a as u64
        }
    "#;
    assert_eq!(compile_and_run(src, "match_variant_lit_sub"), 50);
}

#[test]
fn match_variant_literal_subpattern_matches() {
    if skip_e2e() {
        return;
    }
    // Same enum, but this time the literal sub-pattern *does* match.
    let src = r#"
        enum Shape {
            Circle(i64),
            Other,
        }
        fn main() -> u64 {
            val s = Shape::Circle(0i64)
            val a: i64 = match s {
                Shape::Circle(0i64) => 7i64,
                Shape::Circle(r) => r,
                Shape::Other => -1i64,
            }
            a as u64
        }
    "#;
    assert_eq!(compile_and_run(src, "match_variant_lit_sub_hit"), 7);
}

#[test]
fn match_arm_guard_on_variant() {
    if skip_e2e() {
        return;
    }
    // Guard runs after the binding is in scope; a falsy guard skips
    // the arm and we fall through to the next.
    let src = r#"
        enum Pick {
            Some(i64),
            None,
        }
        fn main() -> u64 {
            val p = Pick::Some(7i64)
            val a: i64 = match p {
                Pick::Some(x) if x < 0i64 => 1i64,
                Pick::Some(x) if x > 5i64 => x * 2i64,
                Pick::Some(x) => x,
                Pick::None => 0i64,
            }
            a as u64
        }
    "#;
    // 7 > 5, so second guard fires: 7 * 2 = 14
    assert_eq!(compile_and_run(src, "match_guard_variant"), 14);
}

#[test]
fn match_arm_guard_on_scalar() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn classify(n: u64) -> u64 {
            match n {
                _ if n == 0u64 => 1u64,
                _ if n < 10u64 => 2u64,
                _ => 3u64,
            }
        }
        fn main() -> u64 {
            classify(5u64)
        }
    "#;
    assert_eq!(compile_and_run(src, "match_guard_scalar"), 2);
}

#[test]
fn println_enum_unit_variant() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Color {
            Red,
            Green,
            Blue,
        }
        fn main() -> u64 {
            val c = Color::Green
            println(c)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_enum_unit");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Color::Green\n");
}

#[test]
fn println_enum_tuple_variant_one_payload() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Shape {
            Circle(i64),
            Point,
        }
        fn main() -> u64 {
            val s = Shape::Circle(5i64)
            println(s)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_enum_one_payload");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Shape::Circle(5)\n");
}

#[test]
fn println_enum_tuple_variant_multi_payload() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Shape {
            Circle(i64),
            Rect(i64, i64),
            Point,
        }
        fn main() -> u64 {
            val s = Shape::Rect(3i64, 7i64)
            println(s)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_enum_multi_payload");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Shape::Rect(3, 7)\n");
}

#[test]
fn println_enum_with_bool_payload() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Maybe {
            Yes(bool),
            No,
        }
        fn main() -> u64 {
            val m = Maybe::Yes(true)
            println(m)
            val n = Maybe::No
            println(n)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_enum_bool");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Maybe::Yes(true)\nMaybe::No\n",
    );
}

#[test]
fn println_enum_dispatches_at_runtime() {
    if skip_e2e() {
        return;
    }
    // Multiple variants reached through different runtime paths
    // exercise the per-variant body blocks. Output order should be
    // Circle, Rect, Point — matching the construction order, not
    // declaration order.
    let src = r#"
        enum Shape {
            Circle(i64),
            Rect(i64, i64),
            Point,
        }
        fn main() -> u64 {
            for i in 0u64..3u64 {
                if i == 0u64 {
                    val s = Shape::Circle(7i64)
                    println(s)
                } elif i == 1u64 {
                    val s = Shape::Rect(2i64, 4i64)
                    println(s)
                } else {
                    val s = Shape::Point
                    println(s)
                }
            }
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_enum_runtime_dispatch");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Shape::Circle(7)\nShape::Rect(2, 4)\nShape::Point\n",
    );
}

#[test]
fn print_enum_no_trailing_newline() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Tag {
            A,
            B(u64),
        }
        fn main() -> u64 {
            val a = Tag::A
            print(a)
            print(" / ")
            val b = Tag::B(99u64)
            println(b)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "print_enum_no_nl");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Tag::A / Tag::B(99)\n",
    );
}

#[test]
fn println_enum_from_function_parameter() {
    if skip_e2e() {
        return;
    }
    // Print an enum that arrived as a parameter — verifies the
    // boundary's per-variant payload locals can drive the same
    // runtime tag-dispatch print as a locally-bound enum.
    let src = r#"
        enum Shape {
            Circle(i64),
            Rect(i64, i64),
            Point,
        }
        fn show(s: Shape) {
            println(s)
        }
        fn main() -> u64 {
            val s = Shape::Rect(11i64, 22i64)
            show(s)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_enum_from_param");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Shape::Rect(11, 22)\n");
}

#[test]
fn enum_passed_to_function() {
    if skip_e2e() {
        return;
    }
    // Call site builds an enum, callee receives it as a Type::Enum
    // parameter and matches on it. Caller's per-variant payload
    // locals expand into one cranelift block param per slot in
    // canonical declaration order, and the callee's allocated locals
    // mirror that order so the boundary is consistent.
    let src = r#"
        enum Shape {
            Circle(i64),
            Rect(i64, i64),
            Point,
        }
        fn area(s: Shape) -> i64 {
            match s {
                Shape::Circle(r) => r * r * 3i64,
                Shape::Rect(w, h) => w * h,
                Shape::Point => 0i64,
            }
        }
        fn main() -> u64 {
            val s = Shape::Rect(3i64, 7i64)
            val a: i64 = area(s)
            a as u64
        }
    "#;
    assert_eq!(compile_and_run(src, "enum_arg_rect"), 21);
}

#[test]
fn enum_unit_variant_passed_to_function() {
    if skip_e2e() {
        return;
    }
    // Same boundary path but with a unit variant — payload locals
    // for non-chosen variants stay uninit, only the tag matters.
    let src = r#"
        enum Shape {
            Circle(i64),
            Rect(i64, i64),
            Point,
        }
        fn label(s: Shape) -> u64 {
            match s {
                Shape::Circle(_) => 1u64,
                Shape::Rect(_, _) => 2u64,
                Shape::Point => 3u64,
            }
        }
        fn main() -> u64 {
            val s = Shape::Point
            label(s)
        }
    "#;
    assert_eq!(compile_and_run(src, "enum_arg_unit"), 3);
}

#[test]
fn enum_passed_through_two_functions() {
    if skip_e2e() {
        return;
    }
    // Two-hop pass: caller -> outer -> inner. Verifies the boundary
    // expansion / re-pack works when a function both receives and
    // forwards an enum value.
    let src = r#"
        enum Pick {
            A(i64),
            B,
        }
        fn inner(p: Pick) -> i64 {
            match p {
                Pick::A(n) => n,
                Pick::B => 0i64,
            }
        }
        fn outer(p: Pick) -> i64 {
            inner(p)
        }
        fn main() -> u64 {
            val p = Pick::A(42i64)
            val r: i64 = outer(p)
            r as u64
        }
    "#;
    assert_eq!(compile_and_run(src, "enum_arg_two_hops"), 42);
}

#[test]
fn enum_construction_in_if_branches() {
    if skip_e2e() {
        return;
    }
    // The if-chain itself produces an enum value: each branch ends
    // in a different variant of the same enum, and the resulting
    // binding holds whichever branch ran.
    let src = r#"
        enum Pick {
            Zero,
            One,
            Many(u64),
        }
        fn weight(p: Pick) -> u64 {
            match p {
                Pick::Zero => 0u64,
                Pick::One => 1u64,
                Pick::Many(n) => n,
            }
        }
        fn classify(n: u64) -> u64 {
            val p = if n == 0u64 {
                Pick::Zero
            } elif n == 1u64 {
                Pick::One
            } else {
                Pick::Many(n)
            }
            weight(p)
        }
        fn main() -> u64 {
            classify(0u64) + classify(1u64) + classify(7u64)
        }
    "#;
    // 0 + 1 + 7 = 8
    assert_eq!(compile_and_run(src, "enum_in_if_branches"), 8);
}

#[test]
fn enum_construction_in_match_arms() {
    if skip_e2e() {
        return;
    }
    // Symmetric: a match expression whose arms return enum values
    // also flows through the composite-target lowering.
    let src = r#"
        enum Pick {
            Zero,
            Big(u64),
        }
        fn weight(p: Pick) -> u64 {
            match p {
                Pick::Zero => 0u64,
                Pick::Big(n) => n,
            }
        }
        fn main() -> u64 {
            val n = 5u64
            val p = match n {
                0u64 => Pick::Zero,
                _ => Pick::Big(n),
            }
            weight(p)
        }
    "#;
    assert_eq!(compile_and_run(src, "enum_in_match_arms"), 5);
}

#[test]
fn enum_construction_in_nested_if() {
    if skip_e2e() {
        return;
    }
    // Nested if-chain: outer branches contain inner branches, all
    // of which still end in enum constructors of the same enum.
    let src = r#"
        enum Pick {
            Tiny(u64),
            Big(u64),
        }
        fn weight(p: Pick) -> u64 {
            match p {
                Pick::Tiny(n) => n,
                Pick::Big(n) => n * 100u64,
            }
        }
        fn main() -> u64 {
            val n = 7u64
            val p = if n < 10u64 {
                if n < 5u64 {
                    Pick::Tiny(n)
                } else {
                    Pick::Tiny(n + 1u64)
                }
            } else {
                Pick::Big(n)
            }
            weight(p)
        }
    "#;
    // n=7, < 10 yes, < 5 no → Tiny(8) → 8
    assert_eq!(compile_and_run(src, "enum_in_nested_if"), 8);
}

#[test]
fn enum_branch_with_existing_binding() {
    if skip_e2e() {
        return;
    }
    // One branch produces a brand-new variant via construction; the
    // other forwards an existing enum binding. Both should land in
    // the same shared target.
    let src = r#"
        enum Pick {
            Default,
            Custom(u64),
        }
        fn weight(p: Pick) -> u64 {
            match p {
                Pick::Default => 42u64,
                Pick::Custom(n) => n,
            }
        }
        fn pick_for(n: u64) -> u64 {
            val fallback = Pick::Default
            val p = if n == 0u64 {
                fallback
            } else {
                Pick::Custom(n)
            }
            weight(p)
        }
        fn main() -> u64 {
            pick_for(0u64) + pick_for(11u64)
        }
    "#;
    // 42 + 11 = 53
    assert_eq!(compile_and_run(src, "enum_branch_existing_binding"), 53);
}

#[test]
fn enum_returned_from_function_simple() {
    if skip_e2e() {
        return;
    }
    // Tail-position enum binding flows through the existing
    // `pending_enum_value` channel; the body just `val`s and
    // returns the binding.
    let src = r#"
        enum Shape {
            Circle(i64),
            Rect(i64, i64),
            Point,
        }
        fn make_rect(w: i64, h: i64) -> Shape {
            val s = Shape::Rect(w, h)
            s
        }
        fn main() -> u64 {
            val r = make_rect(4i64, 5i64)
            val a: i64 = match r {
                Shape::Circle(_) => -1i64,
                Shape::Rect(w, h) => w * h,
                Shape::Point => 0i64,
            }
            a as u64
        }
    "#;
    assert_eq!(compile_and_run(src, "enum_return_simple"), 20);
}

#[test]
fn enum_returned_from_if_chain_function() {
    if skip_e2e() {
        return;
    }
    // Body's tail is an if-chain whose branches each construct an
    // enum value. `lower_body` allocates the return target and
    // routes through `lower_into_enum_target` so each branch writes
    // into the shared locals.
    let src = r#"
        enum Pick {
            Zero,
            One,
            Many(u64),
        }
        fn pick(n: u64) -> Pick {
            if n == 0u64 {
                Pick::Zero
            } elif n == 1u64 {
                Pick::One
            } else {
                Pick::Many(n)
            }
        }
        fn weight(p: Pick) -> u64 {
            match p {
                Pick::Zero => 0u64,
                Pick::One => 1u64,
                Pick::Many(n) => n,
            }
        }
        fn main() -> u64 {
            val a = pick(0u64)
            val b = pick(1u64)
            val c = pick(7u64)
            weight(a) + weight(b) + weight(c)
        }
    "#;
    // 0 + 1 + 7 = 8
    assert_eq!(compile_and_run(src, "enum_return_if_chain"), 8);
}

#[test]
fn enum_returned_from_match_function() {
    if skip_e2e() {
        return;
    }
    // Symmetric: the body's tail is a match producing enum values.
    let src = r#"
        enum Tag {
            Lo,
            Hi(u64),
        }
        fn classify(n: u64) -> Tag {
            match n {
                0u64 => Tag::Lo,
                _ => Tag::Hi(n),
            }
        }
        fn read(t: Tag) -> u64 {
            match t {
                Tag::Lo => 1u64,
                Tag::Hi(n) => n + 10u64,
            }
        }
        fn main() -> u64 {
            val a = classify(0u64)
            val b = classify(5u64)
            read(a) + read(b)
        }
    "#;
    // 1 + 15 = 16
    assert_eq!(compile_and_run(src, "enum_return_match"), 16);
}

#[test]
fn enum_returned_via_tail_constructor() {
    if skip_e2e() {
        return;
    }
    // Function body is just a single `Enum::Variant(args)` literal;
    // no intermediate `val` binding required.
    let src = r#"
        enum Box {
            Item(u64),
        }
        fn make(n: u64) -> Box {
            Box::Item(n + 1u64)
        }
        fn unwrap(b: Box) -> u64 {
            match b {
                Box::Item(v) => v,
            }
        }
        fn main() -> u64 {
            val b = make(41u64)
            unwrap(b)
        }
    "#;
    assert_eq!(compile_and_run(src, "enum_return_constructor"), 42);
}

#[test]
fn generic_enum_option_with_explicit_annotation() {
    if skip_e2e() {
        return;
    }
    // Annotation `val a: Option<i64> = ...` drives the
    // monomorphisation; both branches go through the same
    // Option<i64> instance.
    let src = r#"
        enum Option<T> {
            None,
            Some(T),
        }
        fn unwrap_or(o: Option<i64>, default: i64) -> i64 {
            match o {
                Option::Some(v) => v,
                Option::None => default,
            }
        }
        fn main() -> u64 {
            val a: Option<i64> = Option::Some(100i64)
            val b: Option<i64> = Option::None
            val r: i64 = unwrap_or(a, 1i64) + unwrap_or(b, 2i64)
            r as u64
        }
    "#;
    // 100 + 2 = 102
    assert_eq!(compile_and_run(src, "generic_enum_option"), 102);
}

#[test]
fn generic_enum_option_with_u64_payload() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Option<T> {
            None,
            Some(T),
        }
        fn unwrap_or(o: Option<u64>, default: u64) -> u64 {
            match o {
                Option::Some(v) => v,
                Option::None => default,
            }
        }
        fn main() -> u64 {
            val a: Option<u64> = Option::Some(7u64)
            unwrap_or(a, 99u64)
        }
    "#;
    assert_eq!(compile_and_run(src, "generic_enum_option_u64"), 7);
}

#[test]
fn generic_enum_inferred_from_argument() {
    if skip_e2e() {
        return;
    }
    // No explicit annotation on the val — the compiler infers `T`
    // from the constructor argument type.
    let src = r#"
        enum Box<T> {
            Put(T),
        }
        fn main() -> u64 {
            val b = Box::Put(42u64)
            match b {
                Box::Put(v) => v,
            }
        }
    "#;
    assert_eq!(compile_and_run(src, "generic_enum_infer"), 42);
}

#[test]
fn generic_enum_returned_from_function() {
    if skip_e2e() {
        return;
    }
    // Function returns Option<u64>; caller monomorphises through
    // the function's return type.
    let src = r#"
        enum Option<T> {
            None,
            Some(T),
        }
        fn divide(a: u64, b: u64) -> Option<u64> {
            if b == 0u64 {
                Option::None
            } else {
                Option::Some(a / b)
            }
        }
        fn unwrap_or(o: Option<u64>, default: u64) -> u64 {
            match o {
                Option::Some(v) => v,
                Option::None => default,
            }
        }
        fn main() -> u64 {
            val a = divide(20u64, 4u64)
            val b = divide(20u64, 0u64)
            unwrap_or(a, 99u64) + unwrap_or(b, 7u64)
        }
    "#;
    // 5 + 7 = 12
    assert_eq!(compile_and_run(src, "generic_enum_return"), 12);
}

#[test]
fn generic_enum_two_instantiations_dont_collide() {
    if skip_e2e() {
        return;
    }
    // Same template `Option<T>` instantiated twice with different
    // type args; the monomorphiser interns each (base, args) once
    // and keeps them distinct.
    let src = r#"
        enum Option<T> {
            None,
            Some(T),
        }
        fn unwrap_i64(o: Option<i64>) -> i64 {
            match o {
                Option::Some(v) => v,
                Option::None => 0i64,
            }
        }
        fn unwrap_u64(o: Option<u64>) -> u64 {
            match o {
                Option::Some(v) => v,
                Option::None => 0u64,
            }
        }
        fn main() -> u64 {
            val a: Option<i64> = Option::Some(7i64)
            val b: Option<u64> = Option::Some(11u64)
            (unwrap_i64(a) as u64) + unwrap_u64(b)
        }
    "#;
    // 7 + 11 = 18
    assert_eq!(compile_and_run(src, "generic_enum_two_inst"), 18);
}

#[test]
fn nested_enum_payload_construction_and_match() {
    if skip_e2e() {
        return;
    }
    // `Option<Option<i64>>`: the outer Some's payload is itself an
    // enum value. Storage tree nests; nested `Some(Some(v))`
    // sub-pattern threads through both tag dispatches.
    let src = r#"
        enum Option<T> {
            None,
            Some(T),
        }
        fn main() -> u64 {
            val x: Option<Option<i64>> = Option::Some(Option::Some(42i64))
            val r: i64 = match x {
                Option::Some(Option::Some(v)) => v,
                Option::Some(Option::None) => -1i64,
                Option::None => -2i64,
            }
            r as u64
        }
    "#;
    assert_eq!(compile_and_run(src, "nested_enum_match"), 42);
}

#[test]
fn nested_enum_payload_inner_none_branch() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Option<T> {
            None,
            Some(T),
        }
        fn main() -> u64 {
            val x: Option<Option<i64>> = Option::Some(Option::None)
            val r: i64 = match x {
                Option::Some(Option::Some(v)) => v,
                Option::Some(Option::None) => 7i64,
                Option::None => -2i64,
            }
            r as u64
        }
    "#;
    assert_eq!(compile_and_run(src, "nested_enum_inner_none"), 7);
}

#[test]
fn nested_enum_outer_none_branch() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Option<T> {
            None,
            Some(T),
        }
        fn main() -> u64 {
            val x: Option<Option<i64>> = Option::None
            val r: i64 = match x {
                Option::Some(Option::Some(v)) => v,
                Option::Some(Option::None) => -1i64,
                Option::None => 11i64,
            }
            r as u64
        }
    "#;
    assert_eq!(compile_and_run(src, "nested_enum_outer_none"), 11);
}

#[test]
fn nested_enum_println_recurses() {
    if skip_e2e() {
        return;
    }
    // print/println recurses through nested enum payload, matching
    // the interpreter's `Object::to_display_string`.
    let src = r#"
        enum Option<T> {
            None,
            Some(T),
        }
        fn main() -> u64 {
            val x: Option<Option<i64>> = Option::Some(Option::Some(7i64))
            println(x)
            val y: Option<Option<i64>> = Option::Some(Option::None)
            println(y)
            val z: Option<Option<i64>> = Option::None
            println(z)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "nested_enum_println");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Option<Option<i64>>::Some(Option<i64>::Some(7))\nOption<Option<i64>>::Some(Option<i64>::None)\nOption<Option<i64>>::None\n",
    );
}

#[test]
fn enum_var_reassignment() {
    if skip_e2e() {
        return;
    }
    // `var p` is an enum binding; subsequent assignments overwrite
    // its tag + payload locals in place. `lower_assign` routes the
    // rhs through `lower_into_enum_storage` so the existing storage
    // is reused (cranelift def_var handles the re-binding).
    let src = r#"
        enum Pick {
            A(u64),
            B,
            C(u64),
        }
        fn weight(p: Pick) -> u64 {
            match p {
                Pick::A(n) => n,
                Pick::B => 100u64,
                Pick::C(n) => n + 1000u64,
            }
        }
        fn main() -> u64 {
            var p = Pick::A(5u64)
            val a = weight(p)
            p = Pick::B
            val b = weight(p)
            p = Pick::C(7u64)
            val c = weight(p)
            a + b + c
        }
    "#;
    // 5 + 100 + 1007 = 1112; 1112 & 0xff = 88
    assert_eq!(compile_and_run(src, "enum_reassign"), 1112 & 0xff);
}

#[test]
fn generic_struct_simple() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        struct Cell<T> {
            data: T,
        }
        fn unwrap(c: Cell<u64>) -> u64 {
            c.data
        }
        fn main() -> u64 {
            val c: Cell<u64> = Cell { data: 42u64 }
            unwrap(c)
        }
    "#;
    assert_eq!(compile_and_run(src, "generic_struct_simple"), 42);
}

#[test]
fn generic_struct_two_instantiations() {
    if skip_e2e() {
        return;
    }
    // `Cell<u64>` and `Cell<i64>` get distinct StructIds and don't
    // collide; field-access lowering picks the right monomorphisation
    // through the binding's `struct_id`.
    let src = r#"
        struct Cell<T> {
            data: T,
        }
        fn unwrap_u(c: Cell<u64>) -> u64 { c.data }
        fn unwrap_i(c: Cell<i64>) -> i64 { c.data }
        fn main() -> u64 {
            val a: Cell<u64> = Cell { data: 7u64 }
            val b: Cell<i64> = Cell { data: 5i64 }
            unwrap_u(a) + (unwrap_i(b) as u64)
        }
    "#;
    assert_eq!(compile_and_run(src, "generic_struct_two_inst"), 12);
}

#[test]
fn generic_struct_returned_from_function() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        struct Cell<T> { data: T }
        fn make() -> Cell<u64> {
            val c: Cell<u64> = Cell { data: 7u64 }
            c
        }
        fn main() -> u64 {
            val r = make()
            r.data
        }
    "#;
    assert_eq!(compile_and_run(src, "generic_struct_return"), 7);
}

#[test]
fn println_generic_struct_includes_type_args() {
    if skip_e2e() {
        return;
    }
    // Generic instantiations show their type args in the print
    // header so the user can tell `Y<i64>` apart from `Y<u64>`.
    let src = r#"
        struct Y<T> { b: T }
        fn main() -> u64 {
            val y: Y<i64> = Y { b: 2i64 }
            println(y)
            val z: Y<u64> = Y { b: 7u64 }
            println(z)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_generic_struct");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Y<i64> { b: 2 }\nY<u64> { b: 7 }\n",
    );
}

#[test]
fn println_generic_enum_includes_type_args() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Option<T> { None, Some(T) }
        fn main() -> u64 {
            val a: Option<i64> = Option::Some(5i64)
            println(a)
            val b: Option<u64> = Option::None
            println(b)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_generic_enum");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Option<i64>::Some(5)\nOption<u64>::None\n",
    );
}

#[test]
fn generic_struct_two_type_params() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        struct Pair<A, B> {
            first: A,
            second: B,
        }
        fn main() -> u64 {
            val p: Pair<u64, bool> = Pair { first: 99u64, second: true }
            if p.second { p.first } else { 0u64 }
        }
    "#;
    assert_eq!(compile_and_run(src, "generic_struct_two_params"), 99);
}

#[test]
fn generic_function_identity_two_instantiations() {
    if skip_e2e() {
        return;
    }
    // Two call sites of `fn id<T>(x: T) -> T` produce two
    // monomorphisations: one for u64, one for i64.
    let src = r#"
        fn id<T>(x: T) -> T {
            x
        }
        fn main() -> u64 {
            val a: u64 = id(7u64)
            val b: i64 = id(-3i64)
            a + (b as u64)
        }
    "#;
    // 7 + wrap(-3 as u64) = 7 + (u64::MAX - 2) wraps to 4
    assert_eq!(compile_and_run(src, "generic_fn_id"), 4);
}

#[test]
fn generic_function_with_two_typed_params() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn pair_sum<T>(a: T, b: T) -> T {
            a + b
        }
        fn main() -> u64 {
            val u: u64 = pair_sum(10u64, 20u64)
            val i: i64 = pair_sum(3i64, 4i64)
            u + (i as u64)
        }
    "#;
    assert_eq!(compile_and_run(src, "generic_fn_pair_sum"), 37);
}

#[test]
fn generic_function_taking_generic_enum() {
    if skip_e2e() {
        return;
    }
    // `unwrap_or<T>` accepts an `Option<T>` plus a default. The
    // call infers T from the binding's enum_id (`Option<u64>` here),
    // and the resulting monomorphisation uses the concrete `Option<u64>`
    // throughout its body — `match` arms bind `v` as `u64`.
    let src = r#"
        enum Option<T> { None, Some(T) }
        fn unwrap_or<T>(o: Option<T>, default: T) -> T {
            match o {
                Option::Some(v) => v,
                Option::None => default,
            }
        }
        fn main() -> u64 {
            val a: Option<u64> = Option::Some(42u64)
            val b: Option<u64> = Option::None
            unwrap_or(a, 1u64) + unwrap_or(b, 2u64)
        }
    "#;
    // 42 + 2 = 44
    assert_eq!(compile_and_run(src, "generic_fn_unwrap_or"), 44);
}

#[test]
fn generic_function_called_from_generic_function() {
    if skip_e2e() {
        return;
    }
    // Outer generic `apply` calls inner generic `id`. Each
    // instantiation of `apply<T>` enqueues a fresh `id<T>`
    // instantiation; the work-queue drains both.
    let src = r#"
        fn id<T>(x: T) -> T { x }
        fn apply<T>(x: T) -> T { id(x) }
        fn main() -> u64 {
            val a: u64 = apply(11u64)
            val b: i64 = apply(5i64)
            a + (b as u64)
        }
    "#;
    assert_eq!(compile_and_run(src, "generic_fn_chained"), 16);
}

#[test]
fn enum_with_f64_payload_construct_and_match() {
    if skip_e2e() {
        return;
    }
    // f64 in enum payload: storage is just a Type::F64 local, the
    // boundary flatten passes it as cranelift F64. No bitcast
    // needed since cranelift handles f64 natively.
    let src = r#"
        enum Shape {
            Circle(f64),
            Rect(f64, f64),
            Point,
        }
        fn area(s: Shape) -> f64 {
            match s {
                Shape::Circle(r) => r * r * 3.14f64,
                Shape::Rect(w, h) => w * h,
                Shape::Point => 0.0f64,
            }
        }
        fn main() -> u64 {
            val c = Shape::Circle(2.0f64)
            val a: f64 = area(c)
            a as u64
        }
    "#;
    // 2.0 * 2.0 * 3.14 = 12.56 → as u64 = 12
    assert_eq!(compile_and_run(src, "enum_f64_match"), 12);
}

#[test]
fn enum_with_f64_payload_println() {
    if skip_e2e() {
        return;
    }
    let src = r#"
        enum Shape {
            Circle(f64),
            Rect(f64, f64),
            Point,
        }
        fn main() -> u64 {
            val c = Shape::Circle(2.0f64)
            val r = Shape::Rect(3.0f64, 4.0f64)
            val p = Shape::Point
            println(c)
            println(r)
            println(p)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "enum_f64_println");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Shape::Circle(2.0)\nShape::Rect(3.0, 4.0)\nShape::Point\n",
    );
}

#[test]
fn generic_enum_with_f64_payload() {
    if skip_e2e() {
        return;
    }
    // `Option<f64>` runs the same monomorphisation path as
    // `Option<i64>`; payload type just lowers to F64 via the
    // updated `is_supported_enum_payload` check.
    let src = r#"
        enum Option<T> { None, Some(T) }
        fn unwrap_or(o: Option<f64>, default: f64) -> f64 {
            match o {
                Option::Some(v) => v,
                Option::None => default,
            }
        }
        fn main() -> u64 {
            val a: Option<f64> = Option::Some(3.5f64)
            val b: Option<f64> = Option::None
            val r: f64 = unwrap_or(a, 0.0f64) + unwrap_or(b, 100.0f64)
            r as u64
        }
    "#;
    // 3.5 + 100.0 = 103.5 → as u64 = 103
    assert_eq!(compile_and_run(src, "generic_enum_f64"), 103);
}

#[test]
fn enum_passed_after_construction_in_each_branch() {
    if skip_e2e() {
        return;
    }
    // The compiler MVP doesn't yet allow enum construction at
    // expression positions other than the immediate rhs of `val` /
    // `var`, so we have to construct + pass on each branch instead
    // of doing `val p = if ... { Pick::Zero } else { ... }`.
    let src = r#"
        enum Pick {
            Zero,
            One,
            Many(u64),
        }
        fn weight(p: Pick) -> u64 {
            match p {
                Pick::Zero => 0u64,
                Pick::One => 1u64,
                Pick::Many(n) => n,
            }
        }
        fn classify(n: u64) -> u64 {
            if n == 0u64 {
                val p = Pick::Zero
                weight(p)
            } elif n == 1u64 {
                val p = Pick::One
                weight(p)
            } else {
                val p = Pick::Many(n)
                weight(p)
            }
        }
        fn main() -> u64 {
            classify(0u64) + classify(1u64) + classify(7u64)
        }
    "#;
    // 0 + 1 + 7 = 8
    assert_eq!(compile_and_run(src, "enum_via_branch_pass"), 8);
}

#[test]
fn match_arm_guard_falls_to_next_when_false() {
    if skip_e2e() {
        return;
    }
    // First arm matches the variant but its guard is false, so we
    // fall to the catch-all.
    let src = r#"
        enum Pick {
            Some(i64),
            None,
        }
        fn main() -> u64 {
            val p = Pick::Some(3i64)
            match p {
                Pick::Some(x) if x > 100i64 => 99u64,
                _ => 1u64,
            }
        }
    "#;
    assert_eq!(compile_and_run(src, "match_guard_false"), 1);
}

#[test]
fn println_tuple_singleton() {
    if skip_e2e() {
        return;
    }
    // Single-element tuples render with a trailing comma to disambiguate
    // from a parenthesised expression — matches the interpreter.
    let src = r#"
        fn main() -> u64 {
            val t = (42u64,)
            println(t)
            0u64
        }
    "#;
    let out = compile_and_capture(src, "println_tuple_one");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "(42,)\n",
    );
}
