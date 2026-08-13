//! IR VM ⇄ tree-walker parity.
//!
//! Phase 4 (crate-reorg + fallback wiring): the interpreter can now drive
//! the shared IR VM (`compiler_lower` → `ir_vm`) as an alternative to the
//! tree-walker. This test pins that, for a self-contained corpus of
//! IR-VM-eligible programs, the IR VM produces the same `main` result the
//! tree-walker does — the correctness precondition for eventually retiring
//! the tree-walker (Phase 4 deletion).
//!
//! Programs here avoid the stdlib so the test needs no core-modules dir.

use frontend::ParserWithInterner;

/// Parse + type-check `src`, then assert the IR VM and tree-walker agree on
/// `main`'s result. Returns whether the IR VM lane actually ran (eligible).
fn assert_engine_parity(src: &str) -> bool {
    let mut parser = ParserWithInterner::new(src);
    let mut program = parser.parse_program().expect("parse");
    let interner = parser.get_string_interner();
    interpreter::check_typing_with_core_modules(
        &mut program,
        interner,
        Some(src),
        Some("parity.t"),
        None,
    )
    .expect("type-check");

    // Tree-walker oracle (TOY_IR_VM / INTERPRETER_JIT unset in the test env).
    let tw = interpreter::execute_program(&program, interner, Some(src), Some("parity.t"))
        .expect("tree-walker run");

    // IR VM path (env-independent entry).
    match interpreter::ir_vm::lift::run_main_via_ir_vm(&program, interner) {
        Some(vm) => {
            assert_eq!(
                *tw.borrow(),
                *vm.borrow(),
                "IR VM / tree-walker disagree for:\n{src}"
            );
            true
        }
        None => false,
    }
}

#[test]
fn parity_arithmetic_and_recursion() {
    assert!(assert_engine_parity(
        r#"
        fn fib(n: u64) -> u64 {
            if n <= 1u64 { n } else { fib(n - 1u64) + fib(n - 2u64) }
        }
        fn main() -> u64 { fib(10u64) }
    "#
    ));
}

#[test]
fn parity_loop_accumulate() {
    assert!(assert_engine_parity(
        r#"
        fn main() -> u64 {
            var acc = 0u64
            for i in 0u64 to 10u64 { acc = acc + i }
            acc
        }
    "#
    ));
}

#[test]
fn parity_struct_field_sum() {
    assert!(assert_engine_parity(
        r#"
        struct Point { x: u64, y: u64 }
        fn make() -> Point { Point { x: 30u64, y: 12u64 } }
        fn main() -> u64 {
            val p = make()
            p.x + p.y
        }
    "#
    ));
}

#[test]
fn parity_closure_capture() {
    assert!(assert_engine_parity(
        r#"
        fn main() -> i64 {
            val n = 10i64
            val add_n = fn(x: i64) -> i64 { x + n }
            add_n(32i64)
        }
    "#
    ));
}

#[test]
fn parity_enum_match() {
    assert!(assert_engine_parity(
        r#"
        enum Shape { Circle(i64), Rect(i64, i64), Point }
        fn main() -> i64 {
            val s = Shape::Rect(6i64, 7i64)
            match s {
                Shape::Circle(r) => r * r * 3i64,
                Shape::Rect(w, h) => w * h,
                Shape::Point => 0i64,
            }
        }
    "#
    ));
}

#[test]
fn parity_dyn_trait_dispatch() {
    assert!(assert_engine_parity(
        r#"
        trait Animal { fn sound(self: Self) -> i64 }
        struct Dog {}
        struct Cat {}
        impl Animal for Dog { fn sound(self: Self) -> i64 { 1i64 } }
        impl Animal for Cat { fn sound(self: Self) -> i64 { 2i64 } }
        fn describe(a: &dyn Animal) -> i64 { a.sound() }
        fn main() -> i64 {
            val d = Dog {}
            val c = Cat {}
            describe(d) + describe(c) * 10i64
        }
    "#
    ));
}

#[test]
fn parity_contract_passing() {
    assert!(assert_engine_parity(
        r#"
        fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
            ensures result * b == a
        {
            a / b
        }
        fn main() -> i64 { divide(84i64, 2i64) }
    "#
    ));
}

#[test]
fn parity_negative_array_index() {
    // Python-style negative indexing: a[-1] = last, a[-2] = second-last.
    assert!(assert_engine_parity(
        r#"
        fn main() -> u64 {
            val a: [u64; 5] = [10, 20, 30, 40, 50]
            a[-1i64] + a[-2i64]
        }
    "#
    ));
}

#[test]
fn parity_str_returning_main() {
    // `main` returning a `str` — the IR VM captures the heap bytes before
    // teardown and reconstructs an Object::String matching the tree-walker.
    assert!(assert_engine_parity(
        r#"
        fn greet(name: str) -> str { "hello ".concat(name) }
        fn main() -> str { greet("world") }
    "#
    ));
}

#[test]
fn parity_string_literal_match() {
    // str equality in `match` compares content, not the handle pointer.
    assert!(assert_engine_parity(
        r#"
        fn classify(s: str) -> i64 {
            match s {
                "zero" => 0i64,
                "one" => 1i64,
                "two" => 2i64,
                _ => -1i64,
            }
        }
        fn main() -> i64 {
            classify("one") + classify("two") + classify("unknown")
        }
    "#
    ));
}

#[test]
fn parity_f64_arithmetic_and_compare() {
    // Pins the f64 BinOp / UnaryOp dispatch (add/sub/mul/div, compare, neg).
    assert!(assert_engine_parity(
        r#"
        fn main() -> u64 {
            val a: f64 = 7.5f64
            val b: f64 = 2.0f64
            val s = a + b
            val d = a - b
            val m = a * b
            val q = a / b
            var acc: u64 = 0u64
            if s > 9.0f64 { acc = acc + 1u64 }
            if d < 6.0f64 { acc = acc + 2u64 }
            if -a < 0.0f64 { acc = acc + 4u64 }
            acc + (m as u64) + (q as u64)
        }
    "#
    ));
}

#[test]
fn parity_bool_logic() {
    // Pins RawSlot::from_bool zero-extension (no garbage upper bytes).
    assert!(assert_engine_parity(
        r#"
        fn main() -> bool {
            val t = true
            val f = false
            !t || (f == false)
        }
    "#
    ));
}

#[test]
fn parity_narrow_int_cast() {
    assert!(assert_engine_parity(
        r#"
        fn main() -> u32 {
            val c: i32 = -1i32
            c as u32
        }
    "#
    ));
}

#[test]
fn fallback_does_not_duplicate_stdout() {
    // A program the IR VM can run that prints and *then* diverges: the VM
    // emits partial output and hands the run back to the tree-walker, which
    // re-runs the program. The fallback must discard the VM's partial output
    // rather than replay it — otherwise `hello` appears twice.
    let src = "fn main() -> u64 {\n    println(\"hello\")\n    panic(\"boom\")\n}\n";
    let (result, stdout) = interpreter::output::with_capture(|| {
        interpreter::run_source(src, "dup.t", &interpreter::RunOptions::default())
    });
    assert!(result.is_err(), "the program should diverge");
    assert_eq!(stdout, "hello\n", "output must not be duplicated on fallback");
}
