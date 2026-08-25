//! Numeric literal separators, string interpolation, the iterator
//! protocol and its adapters, f64 display, and labelled loops.

use interpreter::RunOptions;

use super::harness::*;

// Numeric literal separators (`1_000_000`, `0xDEAD_BEEFu64`,
// `3.141_592_f64`, …). The lexer accepts `_` between digits as
// a visual grouping aid; value conversion strips them before
// parsing. Backends never see the separator, so the same value
// reaches interpreter / JIT / AOT regardless of how the source
// was formatted.
#[test]
fn numeric_literal_separators_decimal_round_trip() {
    let src = r#"
        fn main() -> u64 {
            val a: u64 = 1_000_000u64
            val b: u64 = 1_2_3_4u64
            val c: u64 = 42_u64
            a + b + c
        }
    "#;
    assert_consistent(src, "numeric_literal_separators_decimal");
}

#[test]
fn numeric_literal_separators_hex_round_trip() {
    let src = r#"
        fn main() -> u64 {
            val a: u64 = 0xDEAD_BEEFu64
            val b: u32 = 0xFF_FFu32
            a + (b as u64)
        }
    "#;
    assert_consistent(src, "numeric_literal_separators_hex");
}

#[test]
fn numeric_literal_separators_signed_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val a: i64 = 1_234_567i64
            val b: i64 = -1_000_000i64
            a + b
        }
    "#;
    assert_consistent(src, "numeric_literal_separators_signed");
}

// STR-INTERP-AOT: 3-way consistency for string interpolation. The
// chain ends with `.len() as i64` so the program returns an exit
// code (the string itself is consumed by `println` for visual
// inspection on a manual run; we don't capture stdout in
// `assert_consistent`). Length matches across all three backends
// when the desugared `.concat() / __builtin_to_string()` chain
// produces the same bytes.

#[test]
fn string_interp_identifier_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val name = "world"
            val s = "hello {name}"
            s.len() as i64
        }
    "#;
    assert_consistent(src, "string_interp_identifier");
}

#[test]
fn string_interp_tuple_round_trip() {
    // STR-INTERP-COMPOUND-EXTEND tuple branch — `(elem0, elem1)` /
    // `(elem,)` formatting matches the interpreter's display.
    let src = r#"
        fn main() -> i64 {
            val t: (i64, u64) = (3i64, 5u64)
            val s = "t = {t}"
            s.len() as i64
        }
    "#;
    assert_consistent(src, "string_interp_tuple");
}

#[test]
fn string_interp_nested_struct_round_trip() {
    // STR-INTERP-COMPOUND-EXTEND nested-compound branch — struct
    // field that is itself a struct recurses through
    // `emit_struct_format`.
    let src = r#"
        struct Inner { x: i64, y: i64 }
        struct Outer { inner: Inner, n: u64 }
        fn main() -> i64 {
            val o: Outer = Outer { inner: Inner { x: 3i64, y: 5i64 }, n: 7u64 }
            val s = "{o}"
            s.len() as i64
        }
    "#;
    assert_consistent(src, "string_interp_nested_struct");
}

#[test]
fn string_interp_struct_round_trip() {
    // STR-INTERP-COMPOUND: interpolating a struct value at AOT
    // expands into per-field `toy_to_string_<ty>` +
    // `toy_str_concat` chain with format prefixes flowing through
    // `ConstStrBytes` (`.rodata` raw-bytes path). Result must
    // match the interpreter's `Object::to_display_string` output
    // byte-for-byte.
    let src = r#"
        struct Point { x: i64, y: i64 }
        fn main() -> i64 {
            val p: Point = Point { x: 3i64, y: 5i64 }
            val s = "p = {p}"
            s.len() as i64
        }
    "#;
    assert_consistent(src, "string_interp_struct");
}

// STR-INTERP-COMPOUND-EXTEND-ENUM: interpolating an *enum* value at
// AOT. The variant is not known at compile time, so lowering emits a
// runtime tag-dispatch chain — one block per variant, each building
// `EnumName::VariantName(p0, p1, ...)` via ConstStrBytes + StrConcat,
// converging on a result local. The output must match the
// interpreter's `Object::to_display_string` byte-for-byte, which is
// why these tests assert on stdout rather than just the length.

#[test]
fn string_interp_enum_round_trip() {
    let src = r#"
        enum Shape { Circle(i64), Rect(i64, i64), Point }

        fn main() -> u64 {
            val s = Shape::Circle(5i64)
            val r = Shape::Rect(3i64, 4i64)
            val p = Shape::Point
            println("{s} {r} {p}")
            0u64
        }
    "#;
    assert_stdout_consistent(src, "interp_enum");
    // The length route pins the same bytes through the `StrConcat`
    // chain's `.len()` half.
    let src = r#"
        enum Shape { Circle(i64), Rect(i64, i64), Point }

        fn main() -> i64 {
            val s = Shape::Circle(5i64)
            val r = Shape::Rect(3i64, 4i64)
            val p = Shape::Point
            val text = "{s} {r} {p}"
            text.len() as i64
        }
    "#;
    assert_consistent(src, "interp_enum_len");
}

#[test]
fn string_interp_enum_with_mixed_payload_types() {
    // The payloads route through different `toy_to_string_<ty>`
    // helpers (f64 / u64 / bool) — the dispatch chain must pass the
    // right value type per slot.
    let src = r#"
        enum R { Pair(f64, u64), Single(u64), Flag(bool) }

        fn main() -> u64 {
            val a = R::Pair(1.5f64, 7u64)
            val b = R::Single(9u64)
            val c = R::Flag(true)
            println("{a} | {b} | {c}")
            0u64
        }
    "#;
    assert_stdout_consistent(src, "interp_enum_payload_types");
}

#[test]
fn string_interp_generic_enum_round_trip() {
    // The header must carry the concrete type-arg list
    // (`Option<i64>::Some(5)`), matching the interpreter.
    let src = r#"
        fn main() -> u64 {
            val o: Option<i64> = Option::Some(5i64)
            val n: Option<i64> = Option::None
            println("{o} / {n}")
            0u64
        }
    "#;
    assert_stdout_consistent(src, "interp_enum_generic");
}

#[test]
fn string_interp_enum_returned_from_a_function() {
    // The binding arrives via `CallEnum` dests (function return), not
    // a literal — the storage shape is the same but the path into it
    // is not.
    let src = r#"
        enum M { F(f64), U(u64), B(bool) }

        fn pick(k: u64) -> M {
            if k == 0u64 { M::F(1.5f64) } elif k == 1u64 { M::U(7u64) } else { M::B(true) }
        }

        fn main() -> u64 {
            val a = pick(0u64)
            val b = pick(1u64)
            val c = pick(2u64)
            println("[{a}][{b}][{c}]")
            0u64
        }
    "#;
    assert_stdout_consistent(src, "interp_enum_from_function");
}

#[test]
fn string_interp_enum_inside_a_longer_literal() {
    // The enum's dispatch chain is one segment of a multi-part
    // interpolation; surrounding literal text must survive the
    // concat chain in order.
    let src = r#"
        enum S { A(i64), B(u64) }

        fn main() -> u64 {
            val s = S::B(42u64)
            println("value = {s} (end)")
            0u64
        }
    "#;
    assert_stdout_consistent(src, "interp_enum_in_literal");
}

#[test]
fn string_interp_arithmetic_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val a: i64 = 7i64
            val b: i64 = 35i64
            val s = "sum is {a + b}"
            s.len() as i64
        }
    "#;
    assert_consistent(src, "string_interp_arithmetic");
}

#[test]
fn string_interp_multi_segment_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val name = "Alice"
            val n: i64 = 30i64
            val u: u64 = 7u64
            val s = "name={name}, n={n}, u={u}"
            s.len() as i64
        }
    "#;
    assert_consistent(src, "string_interp_multi_segment");
}

#[test]
fn string_interp_bool_and_f64_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val b: bool = true
            val f: f64 = 3.5f64
            val s = "b={b}, f={f}"
            s.len() as i64
        }
    "#;
    assert_consistent(src, "string_interp_bool_and_f64");
}

#[test]
fn string_interp_double_brace_escape_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val n: i64 = 7i64
            val s = "value {{is {n}}}"
            s.len() as i64
        }
    "#;
    assert_consistent(src, "string_interp_double_brace_escape");
}

// ITER-PROTOCOL-AOT: 3-way consistency for the iterator protocol.
// `for x in iter { ... }` desugars to a while + match on
// `iter.next()` (an `&mut self` enum-returning method call) at the
// parser level. AOT support requires (a) `&mut self` writeback in
// let-rhs / match-scrutinee positions, (b) skipping the synthetic
// temporary when EXPR is already a bare identifier so the desugared
// `var __iter = iter` doesn't try to alias-copy a struct binding.


#[test]
fn iter_protocol_basic_round_trip() {
    let src = format!(
        r#"
        {ITER_COUNTER_PRELUDE}
        fn main() -> i64 {{
            var sum = 0i64
            var iter = Counter::new(5i64)
            for x in iter {{ sum = sum + x }}
            sum
        }}
        "#,
    );
    assert_consistent(&src, "iter_protocol_basic");
}

#[test]
fn iter_protocol_break_round_trip() {
    let src = format!(
        r#"
        {ITER_COUNTER_PRELUDE}
        fn main() -> i64 {{
            var sum = 0i64
            var iter = Counter::new(100i64)
            for x in iter {{
                if x >= 5i64 {{ break }}
                sum = sum + x
            }}
            sum
        }}
        "#,
    );
    assert_consistent(&src, "iter_protocol_break");
}

#[test]
fn iter_protocol_continue_round_trip() {
    let src = format!(
        r#"
        {ITER_COUNTER_PRELUDE}
        fn main() -> i64 {{
            var sum = 0i64
            var iter = Counter::new(10i64)
            for x in iter {{
                if x % 2i64 == 1i64 {{ continue }}
                sum = sum + x
            }}
            sum
        }}
        "#,
    );
    assert_consistent(&src, "iter_protocol_continue");
}

#[test]
fn iter_protocol_zero_iterations_round_trip() {
    let src = format!(
        r#"
        {ITER_COUNTER_PRELUDE}
        fn main() -> i64 {{
            var sum = 0i64
            var iter = Counter::new(0i64)
            for x in iter {{ sum = sum + x + 1i64 }}
            sum
        }}
        "#,
    );
    assert_consistent(&src, "iter_protocol_zero_iterations");
}

// STDLIB-ITER-ADAPT: `VecIter<T>` gains `map` / `filter` /
// `enumerate` / `zip` / `collect` (implemented in
// `core/std/collections/vec.t` as ordinary `next(&mut self) ->
// Option<T>` structs). These pin the 3-way consistency of the
// adapters, including the frontend's method-only generic param
// inference on generic-struct receivers (`it.map(f)` binds `U` from
// the closure signature) and the AOT field-closure-call dispatch
// (`self.f(v)` on a struct field of fn type).

#[test]
fn iter_adapt_map_round_trip() {
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(1u64)
            v.push(2u64)
            v.push(3u64)
            var it = v.iter()
            var m = it.map(fn(x: u64) -> u64 { x * 2u64 })
            var total: u64 = 0u64
            for x in m { total = total + x }
            total
        }
    "#;
    assert_consistent(src, "iter_adapt_map");
}

#[test]
fn iter_adapt_filter_round_trip() {
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(1u64)
            v.push(2u64)
            v.push(3u64)
            v.push(4u64)
            var it = v.iter()
            var f = it.filter(fn(x: u64) -> bool { x % 2u64 == 0u64 })
            var total: u64 = 0u64
            for x in f { total = total + x }
            total
        }
    "#;
    assert_consistent(src, "iter_adapt_filter");
}

#[test]
fn iter_adapt_collect_round_trip() {
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(1u64)
            v.push(2u64)
            v.push(3u64)
            var it = v.iter()
            var m = it.map(fn(x: u64) -> u64 { x + 10u64 })
            var c = m.collect()
            var total: u64 = 0u64
            var i: u64 = 0u64
            while i < c.size() {
                total = total + c.get(i)
                i = i + 1u64
            }
            total
        }
    "#;
    assert_consistent(src, "iter_adapt_collect");
}

#[test]
fn iter_adapt_enumerate_round_trip() {
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(3u64)
            v.push(7u64)
            v.push(11u64)
            var it = v.iter()
            var e = it.enumerate()
            var total: u64 = 0u64
            for kv in e { total = total + kv.0 * kv.1 }
            total
        }
    "#;
    assert_consistent(src, "iter_adapt_enumerate");
}

#[test]
fn iter_adapt_zip_round_trip() {
    let src = r#"
        fn main() -> u64 {
            var a: Vec<u64> = Vec::new()
            a.push(1u64)
            a.push(2u64)
            a.push(3u64)
            var b: Vec<u64> = Vec::new()
            b.push(10u64)
            b.push(20u64)
            b.push(30u64)
            b.push(40u64)
            var ia = a.iter()
            var ib = b.iter()
            var z = ia.zip(ib)
            var total: u64 = 0u64
            for p in z { total = total + p.0 + p.1 }
            total
        }
    "#;
    assert_consistent(src, "iter_adapt_zip");
}

// STDLIB-ITER-ADAPT on `DictIter<K, V>` / `StringIter` (dict.t /
// string.t): the Dict adapters take `f: fn (K, V) -> U` (key and
// value as separate scalar args — an AOT closure cannot receive a
// tuple parameter) and keep the iterator state flat with `count`
// packed into `index`'s high 32 bits to stay within the 8-return
// register budget.

#[test]
fn iter_adapt_dict_map_round_trip() {
    let src = r#"
        fn main() -> u64 {
            var d: Dict<u64, u64> = Dict::new()
            d.insert(10u64, 100u64)
            d.insert(20u64, 200u64)
            d.insert(30u64, 300u64)
            var it = d.iter()
            var m = it.map(fn(k: u64, v: u64) -> u64 { k + v })
            var total: u64 = 0u64
            for x in m { total = total + x }
            total
        }
    "#;
    assert_consistent(src, "iter_adapt_dict_map");
}

#[test]
fn iter_adapt_dict_filter_round_trip() {
    let src = r#"
        fn main() -> u64 {
            var d: Dict<u64, u64> = Dict::new()
            d.insert(10u64, 100u64)
            d.insert(20u64, 200u64)
            d.insert(30u64, 300u64)
            var it = d.iter()
            var f = it.filter(fn(k: u64, v: u64) -> bool { v > 150u64 })
            var total: u64 = 0u64
            for kv in f { total = total + kv.1 }
            total
        }
    "#;
    assert_consistent(src, "iter_adapt_dict_filter");
}

#[test]
fn iter_adapt_string_map_round_trip() {
    let src = r#"
        fn main() -> u64 {
            val s = String::from_str("abc")
            var it = s.iter()
            var m = it.map(fn(b: u8) -> u64 { (b as u64) - 96u64 })
            var total: u64 = 0u64
            for x in m { total = total + x }
            total
        }
    "#;
    assert_consistent(src, "iter_adapt_string_map");
}

#[test]
fn iter_adapt_string_filter_round_trip() {
    let src = r#"
        fn main() -> u64 {
            val s = String::from_str("hello")
            var it = s.iter()
            var f = it.filter(fn(b: u8) -> bool { b == 108u8 })
            var total: u64 = 0u64
            for b in f { total = total + (b as u64) }
            total
        }
    "#;
    assert_consistent(src, "iter_adapt_string_filter");
}

#[test]
fn iter_adapt_string_enumerate_round_trip() {
    let src = r#"
        fn main() -> u64 {
            val s = String::from_str("xyz")
            var it = s.iter()
            var e = it.enumerate()
            var total: u64 = 0u64
            for kv in e { total = total + kv.0 + (kv.1 as u64) }
            total
        }
    "#;
    assert_consistent(src, "iter_adapt_string_enumerate");
}

#[test]
fn iter_adapt_string_collect_round_trip() {
    let src = r#"
        fn main() -> u64 {
            val s = String::from_str("hello")
            var it = s.iter()
            var m = it.map(fn(b: u8) -> u8 { b })
            var c = m.collect()
            var total: u64 = 0u64
            var i: u64 = 0u64
            while i < c.size() {
                total = total + (c.get(i) as u64)
                i = i + 1u64
            }
            total
        }
    "#;
    assert_consistent(src, "iter_adapt_string_collect");
}

#[test]
fn iter_protocol_nested_round_trip() {
    let src = format!(
        r#"
        {ITER_COUNTER_PRELUDE}
        fn main() -> i64 {{
            var total = 0i64
            var outer = Counter::new(3i64)
            for i in outer {{
                var inner = Counter::new(3i64)
                for j in inner {{ total = total + i * j }}
            }}
            total
        }}
        "#,
    );
    assert_consistent(&src, "iter_protocol_nested");
}

#[test]
fn string_interp_chain_with_println_round_trip() {
    // Exercises the full chain through `println` — interpolation
    // result flows directly into the print builtin (no `val s =`
    // intermediate). The exit code carries `n` so the test pins
    // both the print path and the function-return path together.
    let src = r#"
        fn main() -> i64 {
            val name = "world"
            val n: i64 = 42i64
            println("hello {name}, n={n}")
            n
        }
    "#;
    assert_consistent(src, "string_interp_chain_with_println");
}

// RUNTIME_PORT R1: the f64 display that used to diverge. The C
// runtime formatted with `%g` (6 significant digits) while the
// interpreter and JIT used Rust's `Display`, so the three agreed
// only for short decimals — the expressions below are the ones
// measured in RUNTIME_PORT.md 実測1, plus the integral/decimal
// boundary. Since R1 they all run the same `toylang_rt` formatting
// code, so agreement is by construction; this test keeps a drift
// from sneaking back in through a golden path.

#[test]
fn f64_display_agrees_across_backends() {
    let src = r#"
        fn main() -> u64 {
            println(0.1f64 + 0.2f64)
            println(1234567.75f64)
            println(123456789.0f64 * 10000000000000.0f64)
            println(1.0f64)
            println(-2.5f64)
            println(0.000001f64)
            println(3.14159f64)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "f64_display_agrees");
}

// RUNTIME-PORT R4, first step ("interpreter とバイト一致を先にテストで
// 固定してから"): a comprehensive f64 display sweep. The values are the
// ones the shortest-round-trip canonical (Rust `Display`, 論点4) has to
// get right: huge / tiny magnitudes where the positional form runs into
// hundreds of digits, the integral `.0` rule, signed zero, infinity and
// NaN (reached through arithmetic — toylang has no scientific-notation
// literals). Every backend must print byte-identical text.

#[test]
fn f64_display_comprehensive_set_agrees_across_backends() {
    let src = r#"
        fn main() -> u64 {
            println(0.1f64 + 0.2f64)
            println(1.0f64)
            println(-0.0f64)
            println(3.141592653589793f64)
            println(1234567.75f64)
            println(0.1f64)
            println(100000000.00000001f64)
            println(123456789012345678901.0f64)
            println(1.0f64 / 0.0f64)
            println(0.0f64 / 0.0f64)
            println(math::pow(10.0f64, 23.0f64))
            println(math::pow(10.0f64, 300.0f64))
            println(math::pow(10.0f64, -300.0f64))
            println(math::pow(2.0f64, 1023.0f64))
            println(-math::pow(2.0f64, 1023.0f64))
            println(2.5f64)
            println(-2.5f64)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "f64_display_comprehensive");
}

/// The canonical output is Rust's `Display` — spelled out here so a
/// drift on *every* backend (which `assert_stdout_consistent` cannot
/// catch) is still pinned. The values are the short ones from the
/// comprehensive sweep; the giant positional forms are covered by the
/// sweep's equality check instead of being transcribed.
#[test]
fn f64_display_canonical_is_rust_display() {
    let src = r#"
        fn main() -> u64 {
            println(0.1f64 + 0.2f64)
            println(1.0f64)
            println(-0.0f64)
            println(3.141592653589793f64)
            println(1234567.75f64)
            println(0.1f64)
            println(1.0f64 / 0.0f64)
            println(0.0f64 / 0.0f64)
            println(2.5f64)
            println(-2.5f64)
            0u64
        }
    "#;
    let expected = "0.30000000000000004\n1.0\n-0.0\n3.141592653589793\n1234567.75\n0.1\ninf\nNaN\n2.5\n-2.5\n";
    let options = RunOptions::default();
    let (result, captured) = interpreter::output::with_capture(|| {
        interpreter::run_source(src, "test.t", &options)
    });
    result.expect("interpreter run");
    assert_eq!(
        captured, expected,
        "the interpreter's f64 display drifted from the Rust-Display canonical"
    );
    // And the compiled backends print the same text (the sweep covers
    // them; this runs it under the same source shape for locality).
    assert_stdout_consistent(src, "f64_display_canonical");
}

#[test]
fn f64_interpolation_agrees_with_the_interpreter_jit() {
    // The sweeps above go through `println(v)`, which every backend
    // renders with its `print_f64` helper. String interpolation takes a
    // different road — `__builtin_to_string(v)` — and the interpreter
    // JIT is the one column that used to implement that road's f64 rule
    // by hand (`v == (v as i64) as f64`) instead of delegating to
    // `Object::to_display_string`. `as i64` saturates, so beyond i64's
    // range the hand-written test went false and the value printed with
    // shortest-round-trip digits and no trailing `.0`, disagreeing with
    // the interpreter, the AOT binary — and with `println(v)` in the
    // very same run.
    //
    // `assert_stdout_consistent` cannot pin this: its lite path returns
    // as soon as the tree-walker, the AOT binary and the *compiler*-side
    // JIT agree, and those three were always right. So drive the
    // interpreter's own JIT column directly, the way
    // `the_interpreter_jit_compares_str_content_too` does.
    //
    // 10^30 is built by multiplication because toylang has no
    // scientific-notation literals; it is far outside i64.
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            var big: f64 = 1f64
            for i in 0u64 to 30u64 {
                big = big * 10f64
            }
            println("{big}")
            println(big)
            0u64
        }
    "#;
    let core = core_modules_dir();
    let mut parser = frontend::ParserWithInterner::new(src);
    let checked = checked_program(src, &mut parser, Some(core.as_path()))
        .expect("interpreter type-check (with core)");
    let interp = checked_interpreter_stdout(&checked, src);
    let jit = checked_jit_stdout(&checked, src);

    // Both lines of the tree-walker's own output must already match
    // each other — interpolation and `println` are the two roads.
    let mut lines = interp.lines();
    let (a, b) = (lines.next().unwrap(), lines.next().unwrap());
    assert_eq!(a, b, "interpolation and println disagree in the tree-walker");

    assert_eq!(
        interp, jit,
        "the interpreter JIT's `__builtin_to_string(f64)` drifted from \
         `Object::to_display_string`",
    );
    // And the compiled backends agree on the same source.
    assert_stdout_consistent(src, "f64_interpolation_jit");
}

// LABEL: 3-way pin for `@label: while/for` + `break @label` /
// `continue @label`. Both round-trips exercise nested loops where
// the label resolves through multiple loop_stack frames.

#[test]
fn labelled_break_round_trip() {
    let src = r#"
        fn main() -> i64 {
            var found: i64 = -1i64
            @outer: for i in 0i64 to 5i64 {
                for j in 0i64 to 5i64 {
                    if i == 2i64 && j == 3i64 {
                        found = i * 10i64 + j
                        break @outer
                    }
                }
            }
            found
        }
    "#;
    assert_consistent(src, "labelled_break");
}

// IF-VAL: 3-way pin for `if val` / `while val` desugar. The construct
// is purely parser-level (frontend desugars to `match` / `while true +
// match`), so every backend should already see plain match/while —
// these tests guard against accidental regressions in the desugar.

#[test]
fn if_val_some_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val opt: Option<i64> = Option::Some(42i64)
            if val Option::Some(x) = opt {
                x
            } else {
                -1i64
            }
        }
    "#;
    assert_consistent(src, "if_val_some");
}

#[test]
fn if_val_none_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val opt: Option<i64> = Option::None
            if val Option::Some(x) = opt {
                x
            } else {
                -1i64
            }
        }
    "#;
    assert_consistent(src, "if_val_none");
}

#[test]
fn while_val_drain_round_trip() {
    // Counter struct with `&mut self` next() — same shape the iterator
    // protocol uses, so AOT's method-call enum-scrutinee path applies.
    // The free-function form is covered by the
    // `match_scrutinee_is_a_*` tests below.
    let src = r#"
        struct Counter { v: i64 }

        impl Counter {
            fn new(start: i64) -> Counter {
                Counter { v: start }
            }
            fn next(&mut self) -> Option<i64> {
                if self.v > 0i64 {
                    val cur: i64 = self.v
                    self.v = self.v - 1i64
                    Option::Some(cur)
                } else {
                    Option::None
                }
            }
        }

        fn main() -> i64 {
            var sum: i64 = 0i64
            var c: Counter = Counter::new(4i64)
            while val Option::Some(x) = c.next() {
                sum = sum + x
            }
            sum
        }
    "#;
    assert_consistent(src, "while_val_drain");
}

#[test]
fn labelled_continue_round_trip() {
    let src = r#"
        fn main() -> i64 {
            var hits: i64 = 0i64
            @outer: for i in 0i64 to 4i64 {
                for j in 0i64 to 4i64 {
                    if j == 2i64 { continue @outer }
                    hits = hits + 1i64
                }
                hits = hits + 100i64
            }
            hits
        }
    "#;
    assert_consistent(src, "labelled_continue");
}

#[test]
fn aot_arena_bytes_used_and_reset() {
    // stdlib introspection: `Arena::bytes_used()` reflects the
    // cumulative size of allocations made through the wrapper;
    // `arena.reset()` clears the counter and the runtime
    // tracking, returning the wrapper to its zero state for
    // re-use. Pin the contract across all 3 backends.
    let src = r#"
        fn main() -> u64 {
            val arena = Arena::new()
            val p1: ptr = arena.alloc(64u64)
            val p2: ptr = arena.alloc(32u64)
            if arena.bytes_used() != 96u64 { return 1u64 }
            arena.reset()
            if arena.bytes_used() != 0u64 { return 2u64 }
            val p3: ptr = arena.alloc(8u64)
            if arena.bytes_used() != 8u64 { return 3u64 }
            42u64
        }
    "#;
    assert_consistent(src, "aot_arena_bytes_used_and_reset");
}

#[test]
fn aot_fixed_buffer_introspection() {
    // stdlib introspection: `FixedBuffer` exposes `capacity()`
    // (constant), `used()` / `remaining()` (current quota),
    // `is_empty()` (used == 0), and `reset()` (return quota
    // to 0). Pin the invariants used + remaining == capacity.
    let src = r#"
        fn main() -> u64 {
            val fb = FixedBuffer::new(128u64)
            if !fb.is_empty() { return 1u64 }
            if fb.capacity() != 128u64 { return 2u64 }
            if fb.remaining() != 128u64 { return 3u64 }

            val q1: ptr = fb.alloc(40u64)
            if fb.used() != 40u64 { return 4u64 }
            if fb.remaining() != 88u64 { return 5u64 }
            if fb.is_empty() { return 6u64 }
            if fb.used() + fb.remaining() != fb.capacity() { return 7u64 }

            # Quota exhaustion returns null without disturbing accounting.
            val q_oob: ptr = fb.alloc(200u64)
            if !__builtin_ptr_is_null(q_oob) { return 8u64 }
            if fb.used() != 40u64 { return 9u64 }

            fb.free(q1)
            if !fb.is_empty() { return 10u64 }

            fb.reset()
            if !fb.is_empty() { return 11u64 }

            42u64
        }
    "#;
    assert_consistent(src, "aot_fixed_buffer_introspection");
}


#[test]
fn loop_basic_round_trip() {
    let src = r#"
        fn main() -> u64 {
            var i = 0u64
            var sum = 0u64
            loop {
                if i >= 5u64 {
                    break
                }
                sum = sum + i
                i = i + 1u64
            }
            sum
        }
    "#;
    assert_consistent(src, "loop_basic");
}

#[test]
fn loop_labelled_break_round_trip() {
    let src = r#"
        fn main() -> u64 {
            var i = 0u64
            var sum = 0u64
            @outer: loop {
                if i >= 3u64 {
                    break @outer
                }
                sum = sum + i
                i = i + 1u64
            }
            sum
        }
    "#;
    assert_consistent(src, "loop_labelled_break");
}

#[test]
fn comparison_chain_lt_lt_round_trip() {
    let src = r#"
        fn main() -> u64 {
            val x = 5u64
            if 0u64 < x < 10u64 {
                1u64
            } else {
                0u64
            }
        }
    "#;
    assert_consistent(src, "comparison_chain_lt_lt");
}

#[test]
fn comparison_chain_three_ops_round_trip() {
    let src = r#"
        fn main() -> u64 {
            val x = 7u64
            if 5u64 < x <= 7u64 < 10u64 {
                1u64
            } else {
                0u64
            }
        }
    "#;
    assert_consistent(src, "comparison_chain_three_ops");
}

// ---------------------------------------------------------------------
// `?` operator (Try-op early-return) round-trip
//
// The parser emits `Expr::Try { inner, .. }`; the type checker
// rewrites it to a `match` over `Result<T, E>` or `Option<T>` whose
// error arm `return`s the propagating value. Backends see only the
// desugared form. These tests pin interpreter / JIT / AOT agreement
// for the four canonical cases.
// ---------------------------------------------------------------------

#[test]
fn a_str_containing_a_nul_prints_all_of_itself() {
    // `toy_print_str` used to take the byte_start of the str layout and
    // scan forward for the terminator, so a str with a NUL in it stopped
    // there: `"ab\u{0}cd"` printed as `ab` on the AOT binary and both
    // JITs, while the tree-walker printed all five bytes. The length is
    // part of the layout -- nothing needed scanning for.
    //
    // `\u{0}` is the reachable way to write one; the escape is decoded
    // at lex time, so the NUL is in the literal's bytes.
    let src = r#"
        fn main() -> u64 {
            val a = "ab\u{0}cd"
            println(a)
            println("{a}")
            println("x\u{0}y".concat("z\u{0}w"))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "str_with_nul");
}

#[test]
fn a_printed_literal_containing_a_nul_survives_the_rodata_path() {
    // A bare string-literal argument takes a different road: the lower
    // emits `PrintStr`, which reads a `.rodata` blob rather than a
    // runtime str value. That blob had no length field at all for the
    // codegen-synthesised fragments, so it gained one when the helper
    // stopped scanning.
    let src = r#"
        fn main() -> u64 {
            println("lit\u{0}eral")
            print("two\u{0}part")
            println("")
            0u64
        }
    "#;
    assert_stdout_consistent(src, "literal_with_nul");
}
