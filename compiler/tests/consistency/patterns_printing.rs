//! Struct and tuple patterns, top-level `const`, and everything that
//! reaches stdout through `print` / `println`.

use super::harness::*;

/// PATTERN-COMPOUND-LOWER. Struct and tuple patterns used to reach
/// only the tree-walker: `match_lowering` handled wildcard, literal,
/// enum-variant and name patterns, and a compound scrutinee was
/// refused outright. Both lower now, so the three engines agree on
/// what an arm selects.
/// ENUM-TUPLE-SUBPATTERN-AOT. A tuple or struct pattern inside an
/// enum variant (`Some((a, b))`, `B(P { x, y: 0i64 })`) was refused by
/// the compiled lanes -- only the scrutinee's own top level took one.
/// The payload is a compound scrutinee of its own, so the same walk
/// checks and binds it, literal elements included.
#[test]
fn compound_patterns_inside_an_enum_variant_match_across_backends() {
    let src = r#"
        struct P { x: i64, y: i64 }
        enum E { A((i64, u64)), B(P), C }
        fn pair(n: u64) -> Option<(u64, u64)> {
            if n > 0u64 { Option::Some((n, n + 1u64)) } else { Option::None }
        }
        fn f(o: Option<(u64, u64)>) -> u64 {
            match o {
                Option::Some((a, b)) => a * 10u64 + b,
                Option::None => 0u64,
            }
        }
        fn g(e: E) -> i64 {
            match e {
                E::A((a, 3u64)) => a * 100i64,
                E::A((a, _)) => a,
                E::B(P { x, y: 0i64 }) => x + 1000i64,
                E::B(P { x, y }) => x * y,
                E::C => 7i64,
            }
        }
        fn main() -> u64 {
            val s: Option<(u64, u64)> = Option::Some((4u64, 2u64))
            val n: Option<(u64, u64)> = Option::None
            var acc: u64 = f(s) + f(n)
            val t: Option<(u64, u64)> = pair(3u64)
            match t {
                Option::Some((a, b)) => { acc = acc + a * b }
                Option::None => {}
            }
            val r = g(E::A((5i64, 3u64))) + g(E::A((5i64, 4u64)))
                + g(E::B(P { x: 2i64, y: 0i64 })) + g(E::B(P { x: 2i64, y: 3i64 })) + g(E::C)
            acc + r as u64
        }
    "#;
    // 42 + 12 + (500 + 5 + 1002 + 6 + 7)
    assert_eq!(interpreter_value(src), 1574);
    assert_consistent(src, "enum_compound_subpattern");
}

#[test]
fn struct_patterns_match_across_backends() {
    let src = r#"
        struct Point { x: i64, y: i64 }

        fn classify(p: Point) -> i64 {
            match p {
                Point { x: 0i64, y: 0i64 } => 0i64,
                Point { x: 0i64, y } => y * 100i64,
                Point { x, y: 0i64 } => x * 10i64,
                Point { x, y } => x + y,
            }
        }

        fn main() -> i64 {
            val origin = Point { x: 0i64, y: 0i64 }
            val on_y = Point { x: 0i64, y: 7i64 }
            val on_x = Point { x: 3i64, y: 0i64 }
            val other = Point { x: 3i64, y: 4i64 }
            println(classify(origin))
            println(classify(on_y))
            println(classify(on_x))
            classify(other)
        }
    "#;
    assert_consistent(src, "match_struct_pattern");
}

/// PATTERN-COMPOUND-LOWER. `..`, guards and nesting go through the
/// same dispatch, so they are pinned together.
#[test]
fn struct_pattern_rest_and_nesting_match_across_backends() {
    let src = r#"
        struct Config { host: str, port: i64, debug: bool }
        struct Inner { v: i64 }
        struct Outer { inner: Inner, tag: i64 }

        fn port_of(c: Config) -> i64 {
            match c {
                Config { port: 0i64, .. } => 0i64 - 1i64,
                Config { port, .. } if port > 100i64 => port * 2i64,
                Config { port, .. } => port,
            }
        }

        fn total(o: Outer) -> i64 {
            match o {
                Outer { inner: Inner { v: 0i64 }, tag } => tag,
                Outer { inner: Inner { v }, tag } => v * tag,
            }
        }

        fn main() -> i64 {
            val zero = Config { host: "a", port: 0i64, debug: false }
            val big = Config { host: "b", port: 500i64, debug: true }
            val small = Config { host: "c", port: 9i64, debug: false }
            val flat = Outer { inner: Inner { v: 0i64 }, tag: 7i64 }
            val nested = Outer { inner: Inner { v: 3i64 }, tag: 5i64 }
            println(port_of(zero))
            println(port_of(big))
            println(port_of(small))
            println(total(flat))
            total(nested)
        }
    "#;
    assert_consistent(src, "match_struct_rest");
}

/// PATTERN-COMPOUND-LOWER. Tuple patterns were in the same position
/// and are fixed by the same change.
#[test]
fn tuple_patterns_match_across_backends() {
    let src = r#"
        fn classify(t: (i64, i64)) -> i64 {
            match t {
                (0i64, y) => y * 100i64,
                (x, 0i64) => x * 10i64,
                (x, y) => x + y,
            }
        }

        fn main() -> i64 {
            val a = (0i64, 7i64)
            val b = (3i64, 0i64)
            val c = (3i64, 4i64)
            println(classify(a))
            println(classify(b))
            classify(c)
        }
    "#;
    assert_consistent(src, "match_tuple_pattern");
}

#[test]
fn top_level_const_match() {
    let src = r#"
        const BASE: u64 = 10u64
        const TIMES: u64 = 7u64
        const TOTAL: u64 = BASE * TIMES
        fn main() -> u64 { TOTAL }
    "#;
    assert_consistent(src, "const_total");
}

#[test]
fn dbc_passing_match() {
    // `requires` / `ensures` succeed on this input, so all three
    // backends should produce the same exit code (no panic).
    let src = r#"
        fn divide(a: i64, b: i64) -> i64
            requires b != 0i64
            ensures result * b == a
        {
            a / b
        }
        fn main() -> u64 {
            val q: i64 = divide(20i64, 4i64)
            q as u64
        }
    "#;
    assert_consistent(src, "dbc_pass");
}

#[test]
fn boolean_returns_match() {
    // `bool` returns: interpreter yields Bool(b), compiler returns 0 or 1
    // via the cranelift-generated function.
    let src = r#"
        fn is_even(n: u64) -> bool { n % 2u64 == 0u64 }
        fn main() -> u64 {
            if is_even(10u64) { 1u64 } else { 0u64 }
        }
    "#;
    assert_consistent(src, "bool_return");
}


// =====================================================================
// Stdout consistency tests (Phase V).
//
// `assert_stdout_consistent` runs each source through all three
// backends (in-process interpreter binary, JIT-flagged interpreter
// binary, and the AOT compiler-built executable) and asserts the
// captured stdout bytes are byte-identical. Catches print-formatting
// drift that the exit-code-only `assert_consistent` cannot.
// =====================================================================

#[test]
fn stdout_scalar_println_match() {
    let src = r#"
        fn main() -> u64 {
            println(42i64)
            println(true)
            println(false)
            println(7u64)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_scalar");
}

#[test]
fn stdout_string_literal_and_var_match() {
    let src = r#"
        fn main() -> u64 {
            println("hello world")
            val s = "from var"
            println(s)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_str");
}

#[test]
fn unicode_escape_round_trip() {
    // `\u{HEX}` Unicode escape — char literal lexes the code point
    // as `u32`, and string literal encodes it as 1-4 UTF-8 bytes.
    // This test pins the char-literal half across all 3 backends;
    // the string-literal half is exercised by the stdout test
    // below (interpreter / JIT / AOT must produce identical bytes,
    // including for multi-byte UTF-8 sequences).
    let src = r#"
        fn main() -> u64 {
            val ascii: u32 = '\u{41}'
            val bmp: u32 = '\u{3042}'
            val astral: u32 = '\u{1F600}'
            if ascii != 65u32 { return 1u64 }
            if bmp != 12354u32 { return 2u64 }
            if astral != 128512u32 { return 3u64 }
            42u64
        }
    "#;
    assert_consistent(src, "unicode_escape_round_trip");
}

#[test]
fn stdout_string_unicode_escape_match() {
    // String-literal `\u{HEX}` encodes the code point into UTF-8
    // bytes once at lex time. The 3 backends each just emit the
    // bytes verbatim through `println`. Stdout-equality across
    // interpreter / JIT / AOT pins that the encoding is done
    // exactly once and isn't double-handled downstream.
    let src = r#"
        fn main() -> u64 {
            println("ascii \u{41}")
            println("bmp \u{3042}")
            println("astral \u{1F600}")
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_unicode_escape");
}

#[test]
fn hex_escape_round_trip() {
    // `\xHH` 2-digit hex escape in both char and string literals.
    // The lexer decodes it once (handler in `lexer.l`) and downstream
    // sees a plain `u32` value (char) / decoded byte (string).
    let src = r#"
        fn main() -> u64 {
            val a: u32 = '\x41'
            val z: u32 = '\x7A'
            val nul: u32 = '\x00'
            val high: u32 = '\xff'
            if a != 65u32 { return 1u64 }
            if z != 122u32 { return 2u64 }
            if nul != 0u32 { return 3u64 }
            if high != 255u32 { return 4u64 }
            42u64
        }
    "#;
    assert_consistent(src, "hex_escape_round_trip");
}

#[test]
fn stdout_string_literal_escape_sequences_match() {
    // String escape sequences are processed by the lexer once and then
    // travel as raw bytes through the type checker, IR, and all 3
    // backends. Stdout-equality across interpreter / JIT / AOT pins
    // that nobody re-escapes or double-decodes the body.
    //
    // Covered escapes: `\n` (LF=10) / `\t` (HT=9) / `\\` (literal
    // backslash) / `\'` (literal single quote). `\r` and `\0` are
    // not exercised here — `\0` would terminate downstream C printf
    // helpers, and `\r` makes diffs harder to read.
    let src = r#"
        fn main() -> u64 {
            println("line1\nline2")
            println("tab\there")
            println("backslash\\done")
            println("quote\'done")
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_string_escapes");
}

#[test]
fn stdout_struct_println_match() {
    let src = r#"
        struct Point { x: i64, y: i64 }
        fn main() -> u64 {
            val p = Point { x: 3i64, y: 4i64 }
            println(p)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_struct");
}

#[test]
fn stdout_enum_println_match() {
    let src = r#"
        enum Shape { Circle(i64), Rect(i64, i64), Empty }
        fn main() -> u64 {
            val a = Shape::Circle(5i64)
            val b = Shape::Rect(3i64, 7i64)
            val c = Shape::Empty
            println(a)
            println(b)
            println(c)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_enum");
}

#[test]
fn stdout_generic_struct_println_match() {
    // Catches the divergence the interpreter generic-print phase
    // tracked down: compiler emits `Cell<i64> { value: 7 }`; the
    // interpreter used to drop the type args and emit
    // `Cell { value: 7 }` instead. With the fix in place all three
    // backends agree on the type-argument-bearing output.
    let src = r#"
        struct Cell<T> { value: T }
        fn main() -> u64 {
            val c: Cell<i64> = Cell { value: 7i64 }
            println(c)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_generic");
}

#[test]
fn stdout_generic_enum_println_match() {
    // Uses `Maybe` rather than `Option` so the test's inline enum
    // declaration doesn't collide with `core/std/option.t`'s
    // `enum Option<T>` (now auto-loaded by every program).
    let src = r#"
        enum Maybe<T> { Nothing, Just(T) }
        fn main() -> u64 {
            val s: Maybe<i64> = Maybe::Just(5i64)
            val n: Maybe<i64> = Maybe::Nothing
            println(s)
            println(n)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_generic_enum");
}

#[test]
fn stdout_loop_with_print_match() {
    let src = r#"
        fn main() -> u64 {
            for i in 0u64..4u64 {
                println(i)
            }
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_loop");
}

#[test]
fn stdout_narrow_int_dedicated_helpers() {
    // NUM-W-AOT-pack Phase 2: AOT now calls
    // `toy_print_{i,u}{8,16,32}` directly instead of routing
    // through the wide helpers via sextend / uextend. Output must
    // stay byte-identical across interpreter / JIT (silent
    // fallback) / AOT for every narrow width, including the
    // signed-edge cases where the previous wide path's
    // sign-extension shaped the printed digits.
    //
    // Mixes positive + negative + max values across all six
    // widths so a regression in the new per-width helper
    // (wrong format string, missing `cast` in the JIT capture
    // path, ABI width mismatch) would surface as a divergent
    // line in the captured stdout.
    let src = r#"
        fn main() -> u64 {
            println(7i8)
            println(-5i8)
            println(127i8)
            println(255u8)
            println(-1000i16)
            println(50000u16)
            println(-1000000i32)
            println(4000000000u32)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_narrow_int_dedicated");
}

#[test]
fn a_match_arm_may_be_a_block_that_ends_in_a_name() {
    // AOT-MATCH-STR-ARM-BLOCK, which named the type it was first met
    // with rather than the shape: a tail `match` whose arms are
    // blocks ending in a name the block itself bound produced no
    // value, and the function was rejected with "falls through
    // without producing a value of the declared return type" — for a
    // body the interpreter ran.
    //
    // The result local is sized by peeking at the arms before they
    // are lowered, and the peek could not see through `val a = ..`
    // to what `a` is. It reads the binding now.
    //
    // Both `str` (where it was found) and `u64` (where it was also
    // broken, which is what showed the title was wrong).
    let src = r#"
        fn spell(o: Option<u64>) -> str {
            match o {
                Option::Some(v) => { val a: String = String::from_str("one")
                                     val s: str = a.to_str()
                                     s }
                Option::None => { val b: String = String::from_str("none")
                                  val t: str = b.to_str()
                                  t }
            }
        }

        fn count(o: Option<u64>) -> u64 {
            match o {
                Option::Some(v) => { val a = v + 1u64
                                     a }
                Option::None => { val b = 9u64
                                  b }
            }
        }

        fn main() -> u64 {
            println(spell(Option::Some(1u64)))
            println(spell(Option::None))
            println(count(Option::Some(1u64)))
            println(count(Option::None))
            # And bound rather than returned, which failed with a
            # different message ("val/var rhs produced no value").
            val five: Option<u64> = Option::Some(5u64)
            val r = match five {
                Option::Some(v) => { val a = v * 2u64
                                     a }
                Option::None => { val b = 0u64
                                  b }
            }
            println(r)
            0u64
        }
    "#;
    assert_renders(src, "match_arm_block_tail_name", "one\nnone\n2\n9\n10\n");
}
