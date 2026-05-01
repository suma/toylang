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
