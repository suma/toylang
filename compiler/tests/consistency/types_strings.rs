//! Type aliases, narrow integers, `String` / `str` methods, operator
//! overloads, and `Vec`.

use super::harness::*;

#[test]
fn char_literal_round_trip() {
    // char literal `'X'` lexes to `Kind::UInt32(<code point>)` so
    // the parser / type checker / IR / 3 backends all see a
    // standard `u32` value.  Test both plain ASCII chars and
    // the supported escapes (`\n` / `\t` / `\r` / `\0` / `\\`
    // / `\'` / `\"`).
    let src = r#"
        fn main() -> u64 {
            val a: u32 = 'A'
            val z: u32 = 'z'
            val zero: u32 = '0'
            val space: u32 = ' '
            if a != 65u32 { return 1u64 }
            if z != 122u32 { return 2u64 }
            if zero != 48u32 { return 3u64 }
            if space != 32u32 { return 4u64 }
            val nl: u32 = '\n'
            val tab: u32 = '\t'
            val cr: u32 = '\r'
            val nul: u32 = '\0'
            val bs: u32 = '\\'
            val sq: u32 = '\''
            val dq: u32 = '\"'
            if nl != 10u32 { return 5u64 }
            if tab != 9u32 { return 6u64 }
            if cr != 13u32 { return 7u64 }
            if nul != 0u32 { return 8u64 }
            if bs != 92u32 { return 9u64 }
            if sq != 39u32 { return 10u64 }
            if dq != 34u32 { return 11u64 }
            42u64
        }
    "#;
    assert_consistent(src, "char_literal_round_trip");
}

#[test]
fn generic_struct_value_passing_round_trip() {
    // Pre-existing limitation now fixed: passing a generic-struct
    // value across a function boundary used to fail in the
    // interpreter with `Struct(name, []) != Struct(name, [Int64])`
    // because runtime values don't carry type args. Static type
    // checking already passes; this is the runtime defence-in-depth
    // check that was too strict. `is_equivalent` for `Struct/Struct`
    // now accepts an empty params side, mirroring the existing
    // `Enum/Enum` and `Identifier/Struct` relaxations.
    //
    // Pin the fix across all 3 backends.
    let src = r#"
        struct Box<T> { v: T }

        fn unwrap_int(p: Box<i64>) -> i64 {
            p.v
        }

        fn double_pair(p: Box<u64>) -> u64 {
            p.v + p.v
        }

        fn main() -> u64 {
            val a: Box<i64> = Box { v: 21i64 }
            val b: Box<u64> = Box { v: 7u64 }
            val ai: i64 = unwrap_int(a)
            val bi: u64 = double_pair(b)
            if ai != 21i64 { return 1u64 }
            if bi != 14u64 { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "generic_struct_value_passing_round_trip");
}

#[test]
fn generic_type_alias_round_trip() {
    // `type Pair<T> = Box<T>` — a parameterised alias. Use sites
    // `Pair<i64>` and `Pair<u64>` substitute the type arg into the
    // alias target at parse time, so downstream sees the
    // fully-monomorphised struct type. Two different concrete
    // instantiations co-exist in the same program.
    //
    // Also covers:
    //   - non-generic alias of a generic alias (`type IntPair =
    //     Pair<i64>`) — chains a substituted form into another
    //     alias name
    //
    // The earlier `Struct(name, []) vs Struct(name, [...])` runtime
    // limitation was lifted in the same series, so we can now also
    // pass alias-typed struct values across function boundaries.
    let src = r#"
        struct Box<T> { v: T }
        type Pair<T> = Box<T>
        type IntPair = Pair<i64>

        fn unwrap_int(p: IntPair) -> i64 {
            p.v
        }

        fn make_u64_pair(n: u64) -> Pair<u64> {
            Box { v: n }
        }

        fn main() -> u64 {
            val a: IntPair = Box { v: 21i64 }
            val b: Pair<u64> = make_u64_pair(7u64)
            val ai: i64 = unwrap_int(a)
            if ai != 21i64 { return 1u64 }
            if b.v != 7u64 { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "generic_type_alias_round_trip");
}

#[test]
fn narrow_arithmetic_compares_at_its_own_width() {
    // NUM-W. The IR VM did every operation at 64 bits and left the
    // result there, so `200u8 * 3u8` sat in the slot as 600. Printing
    // masked it and casting masked it, so the only way to see the
    // difference was to compare: this program returned 1 on the IR VM
    // and 0 on the tree-walker, the JIT and the AOT binary, all three
    // of which work at the narrow width natively.
    let src = r#"
        fn main() -> u64 {
            var a: u8 = 200u8
            var b: u8 = 3u8
            var c: i8 = 100i8
            if a * b != 88u8 { return 1u64 }
            if a + b != 203u8 { return 2u64 }
            if c + c != -56i8 { return 3u64 }
            if ~a != 55u8 { return 4u64 }
            0u64
        }
    "#;
    assert_consistent(src, "narrow_arithmetic_compares_at_its_own_width");
}

#[test]
fn narrow_int_jit_phase_c_cast_sizeof_round_trip() {
    // NUM-W-JIT Phase C: integer-width casts (`u8 as u16`,
    // `i32 as u32`, `u8 as u64`) lower to cranelift `sextend` /
    // `uextend` / `ireduce`, and `__builtin_sizeof` over a narrow
    // value returns the byte count (1 / 2 / 4). This shape used
    // to silently fall back to the interpreter; now it
    // JIT-compiles end-to-end.
    let src = r#"
        fn main() -> u64 {
            val a: u8 = 200u8 + 50u8
            val b: u16 = a as u16 - 100u16
            val c: i32 = -1i32
            val d: u32 = c as u32
            val sized: u64 = __builtin_sizeof(a)
                + __builtin_sizeof(b)
                + __builtin_sizeof(c)
                + __builtin_sizeof(d)
            if a != 250u8 { return 1u64 }
            if b != 150u16 { return 2u64 }
            if d != 4294967295u32 { return 3u64 }
            if sized != 11u64 { return 4u64 }
            42u64
        }
    "#;
    assert_consistent(src, "narrow_int_jit_phase_c_cast_sizeof_round_trip");
}

#[test]
fn stdout_narrow_int_jit_print_match() {
    // NUM-W-JIT Phase B: `print(narrow_val)` /
    // `println(narrow_val)` go through per-width helper symbols
    // (`jit_print_u8`, `jit_println_i32`, ...) registered with the
    // JIT runtime. Each helper formats with the native Rust width's
    // `Display` impl, which matches the AOT pipeline (libc printf
    // via `toy_print_u8` / etc.) and the tree-walking interpreter
    // (`Object::to_display_string`). Stdout-equality across all 3
    // backends pins that no width drops a sign bit or zero-extends
    // wrong on its way to the helper.
    let src = r#"
        fn main() -> u64 {
            val a: u8 = 100u8
            val b: i8 = -42i8
            val c: u16 = 1000u16
            val d: i16 = -1234i16
            val e: u32 = 4294967290u32
            val f: i32 = -7i32
            println(a)
            println(b)
            println(c)
            println(d)
            println(e)
            println(f)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "stdout_narrow_int_jit_print");
}

#[test]
fn narrow_int_jit_phase_a_round_trip() {
    // NUM-W-JIT Phase A: u8 / u16 / u32 / i8 / i16 / i32 are now
    // recognised at the JIT eligibility + literal-codegen layer,
    // and `iadd` / `isub` etc. are width-polymorphic in cranelift
    // so arithmetic between two same-width narrow operands
    // compiles end-to-end. Cross-function narrow calls also work
    // — `add_u8(a, b)` flows args through cranelift's calling
    // convention with the appropriate `I8` / `I16` / `I32` ABI
    // type.
    //
    // Cast-to-wider (`r as u64`) and `__builtin_sizeof` of a
    // narrow value still fall back; later phases add them.
    let src = r#"
        fn add_u8(a: u8, b: u8) -> u8 {
            a + b
        }

        fn double_u16(x: u16) -> u16 {
            x + x
        }

        fn neg_i32(x: i32) -> i32 {
            0i32 - x
        }

        fn main() -> u64 {
            val r1: u8 = add_u8(100u8, 50u8)
            val r2: u16 = double_u16(1000u16)
            val r3: i32 = neg_i32(-7i32)
            if r1 != 150u8 { return 1u64 }
            if r2 != 2000u16 { return 2u64 }
            if r3 != 7i32 { return 3u64 }
            42u64
        }
    "#;
    assert_consistent(src, "narrow_int_jit_phase_a_round_trip");
}

#[test]
fn vec_from_str_empty_string_round_trip() {
    // `String::from_str("")` previously hit a "Invalid memory access
    // in mem_copy" interpreter error: `core/std/collections/vec.t::from_str`
    // calls `__builtin_heap_alloc(0u64)` then
    // `__builtin_heap_realloc(p, 0u64)` followed by
    // `__builtin_mem_copy(s.as_ptr(), data, 0u64)`. The mem_copy
    // happily accepts size==0 in the AOT path (libc memcpy(3) is
    // a no-op for n=0) but the interpreter's `HeapManager::copy_memory`
    // returned `false` on size==0 (no slices / typed_slots matched
    // the empty range), and the builtin treated `false` as a hard
    // error. Fixed by adding an early-return for size==0 in
    // copy_memory / move_memory / set_memory so all three are
    // consistent with their libc counterparts.
    let src = r#"
        fn main() -> u64 {
            val s: String = String::from_str("")
            if s.size() != 0u64 { return 1u64 }
            if !s.is_empty() { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "vec_from_str_empty_string_round_trip");
}

#[test]
fn type_alias_forward_reference_round_trip() {
    // Forward references to type aliases. The cross-module
    // alias resolution pass (`frontend::resolve_type_aliases`,
    // `c6a6d20`) runs after the entire AST is built, so it
    // doesn't matter where in the file an alias is declared
    // relative to its uses. Per-file parser-time substitution
    // still requires "before-use" ordering, but anything the
    // parser couldn't resolve falls through to the post-pass
    // and gets fixed up there.
    //
    // Covers:
    //   - non-generic alias used before declaration (`Foo`)
    //   - alias chain (`B -> A -> u64`) where `B` precedes `A`
    //   - generic alias used before declaration (`Pair<T>`)
    let src = r#"
        fn main() -> u64 {
            val a: A = 42u64
            val b: B = 7u64
            val p: Pair<u64> = Box { v: 5u64 }
            if a != 42u64 { return 1u64 }
            if b != 7u64 { return 2u64 }
            if p.v != 5u64 { return 3u64 }
            42u64
        }

        struct Box<T> { v: T }
        type Pair<T> = Box<T>
        type B = A
        type A = u64
    "#;
    assert_consistent(src, "type_alias_forward_reference_round_trip");
}

#[test]
fn cross_module_char_alias_round_trip() {
    // `core/std/char.t::type char = u32` is resolved by the
    // cross-module alias pass — annotation positions in user
    // code (`val a: char`, `c: char` parameter, `-> char` return)
    // all substitute to `u32`. Used in conjunction with the
    // `Vec<u8>::push_char(&mut self, c: char)` declaration in
    // `core/std/collections/vec.t`, which UTF-8 encodes the
    // codepoint into 1-4 bytes.
    let src = r#"
        fn id_char(c: char) -> char {
            c
        }

        fn main() -> u64 {
            val a: char = 65u32
            val b: char = id_char(a)
            if b != 65u32 { return 1u64 }
            var s: String = String::from_str("x")
            s.push_char(a)
            if s.size() != 2u64 { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "cross_module_char_alias_round_trip");
}

#[test]
fn cross_module_type_alias_round_trip() {
    // `type String = Vec<u8>` lives in `core/std/string.t` and is
    // resolved by `frontend::resolve_type_aliases` after module
    // integration. This test confirms the alias propagates from the
    // stdlib file into user code: type annotations (`val s: String`,
    // `&String` parameters) and the `Vec<u8>` method dispatch
    // (`.size()`, `.eq(other)`, `.push_str(other)`) all work
    // through the alias.
    //
    // Pinned across all 3 backends — the resolution pass runs in
    // both `interpreter::check_typing_with_core_modules` (which
    // the AOT compiler also delegates to via `compiler::compile_file`)
    // and the JIT pipeline (silent fallback).
    let src = r#"
        fn len_of(s: &String) -> u64 {
            s.size()
        }

        fn main() -> u64 {
            val a: String = String::from_str("hello")
            val b: String = String::from_str("hello")
            val c: String = String::from_str("world")
            if len_of(a) != 5u64 { return 1u64 }
            if !a.eq(b) { return 2u64 }
            if a.eq(c) { return 3u64 }
            42u64
        }
    "#;
    assert_consistent(src, "cross_module_type_alias_round_trip");
}

#[test]
fn type_alias_round_trip() {
    // `type Name = TargetType` aliases are eagerly substituted by the
    // parser, so the type checker / IR / 3 backends see only the
    // expanded target. The test pins:
    //   - primitive alias (Byte = u32) used in a val annotation,
    //     a function return type, and as a parameter type
    //   - alias chain (Word = Byte) — both names must resolve to u32
    //   - struct alias inside a generic — `Pair = Box<Byte>` so the
    //     parser substitutes `Byte` *inside* the type-arg list
    //   - nested usage of one alias inside another's target type
    let src = r#"
        type Byte = u32
        type Word = Byte
        type Score = i64

        struct Box<T> { v: T }
        type ByteBox = Box<Byte>

        fn id_byte(b: Byte) -> Byte {
            b
        }

        fn double(s: Score) -> Score {
            s + s
        }

        fn make_byte_box(b: Byte) -> ByteBox {
            Box { v: b }
        }

        fn main() -> u64 {
            val w: Word = 7u32
            val b: Byte = id_byte(w)
            if b != 7u32 { return 1u64 }
            val s: Score = double(21i64)
            if s != 42i64 { return 2u64 }
            val bb: ByteBox = make_byte_box(99u32)
            if bb.v != 99u32 { return 3u64 }
            42u64
        }
    "#;
    assert_consistent(src, "type_alias_round_trip");
}

#[test]
fn string_eq_clear_push_char_round_trip() {
    // `String::eq` / `String::clear` / `String::push_char` —
    // the byte-comparison + reset + 1-byte-append trio. Exercises:
    //   - `eq(&self, other: &String) -> bool` with both
    //     length-mismatch (early-return false) and length-equal
    //     full-loop paths
    //   - `clear(&mut self)` followed by `is_empty()` / `len()`
    //   - `push_char(&mut self, c: char)` (char = u32) UTF-8
    //     encoding ASCII codepoints into a single byte
    // Auto-borrow at the call sites: `s.eq(a)` passes `a:
    // String` into the `&String` param thanks to REF-Stage-2-min.
    // 3-way pin across interpreter / JIT silent fallback / AOT.
    let src = r#"
        fn main() -> u64 {
            var s: String = String::from_str("hi")
            s.push_char(33u32)

            val a: String = String::from_str("hi!")
            val b: String = String::from_str("hi?")

            if !s.eq(a) { return 1u64 }
            if s.eq(b) { return 2u64 }

            s.clear()
            if !s.is_empty() { return 3u64 }
            if s.size() != 0u64 { return 4u64 }

            s.push_char(120u32)
            val x: String = String::from_str("x")
            if !s.eq(x) { return 5u64 }
            if s.size() != 1u64 { return 6u64 }

            42u64
        }
    "#;
    assert_consistent(src, "string_eq_clear_push_char_round_trip");
}

#[test]
fn push_char_two_byte_utf8_round_trip() {
    // `Vec<u8>::push_char` UTF-8 encoding for codepoint 0xE9 ('é').
    // Expected bytes: [0xC3, 0xA9]. Pinned across interpreter, JIT
    // (silent fallback for generic Vec<T>), and AOT.
    let src = r#"
        fn main() -> u64 {
            var s: Vec<u8> = Vec::new()
            s.push_char(0xE9u32)
            if s.size() != 2u64 { return 1u64 }
            if s.get(0u64) != 0xC3u8 { return 2u64 }
            if s.get(1u64) != 0xA9u8 { return 3u64 }
            42u64
        }
    "#;
    assert_consistent(src, "push_char_two_byte_utf8_round_trip");
}

#[test]
fn string_len_via_length_trait_round_trip() {
    // `core/std/string.t::impl Length for Vec<u8>` exposes `.len()`
    // on `String` as a thin wrapper around `.size()`. Pinning this
    // confirms trait-based dispatch into stdlib `Vec<u8>` works
    // across all 3 backends (interpreter, JIT silent fallback, AOT).
    let src = r#"
        fn main() -> u64 {
            val s: String = String::from_str("hello")
            if s.len() != 5u64 { return 1u64 }
            if s.len() != s.size() { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "string_len_via_length_trait_round_trip");
}

#[test]
fn string_as_ptr_via_trait_round_trip() {
    // `impl AsPtr for Vec<u8>` exposes `.as_ptr()` on `String`.
    // Round-trip through `__builtin_ptr_read` confirms the
    // returned pointer addresses the buffer's first byte in every
    // backend.
    let src = r#"
        unsafe fn main() -> u64 {
            val s: String = String::from_str("Z")
            val p: ptr = s.as_ptr()
            val b: u8 = __builtin_ptr_read::<u8>(p, 0u64)
            if b != 0x5Au8 { return 1u64 }
            42u64
        }
    "#;
    assert_consistent(src, "string_as_ptr_via_trait_round_trip");
}

#[test]
fn string_substring_round_trip() {
    // `core/std/string.t::impl Substring for Vec<u8>` exposes
    // `.substring(start, end)` returning a fresh `Vec<u8>`. Pinning
    // confirms the half-open byte slice + the let-rhs path for
    // compound-returning instance methods works in all 3 backends.
    let src = r#"
        fn main() -> u64 {
            val s: String = String::from_str("hello world")
            val sub: String = s.substring(6u64, 11u64)
            val expected: String = String::from_str("world")
            if !sub.eq(expected) { return 1u64 }
            42u64
        }
    "#;
    assert_consistent(src, "string_substring_round_trip");
}

#[test]
fn string_trim_round_trip() {
    let src = r#"
        fn main() -> u64 {
            val s: String = String::from_str("  trim me  ")
            val t: String = s.trim()
            val expected: String = String::from_str("trim me")
            if !t.eq(expected) { return 1u64 }
            42u64
        }
    "#;
    assert_consistent(src, "string_trim_round_trip");
}

#[test]
fn string_to_ascii_upper_round_trip() {
    let src = r#"
        fn main() -> u64 {
            val s: String = String::from_str("Hello World")
            val u: String = s.to_ascii_upper()
            val expected: String = String::from_str("HELLO WORLD")
            if !u.eq(expected) { return 1u64 }
            42u64
        }
    "#;
    assert_consistent(src, "string_to_upper_round_trip");
}

#[test]
fn string_to_ascii_lower_round_trip() {
    let src = r#"
        fn main() -> u64 {
            val s: String = String::from_str("Hello World")
            val l: String = s.to_ascii_lower()
            val expected: String = String::from_str("hello world")
            if !l.eq(expected) { return 1u64 }
            42u64
        }
    "#;
    assert_consistent(src, "string_to_lower_round_trip");
}

#[test]
fn builtin_sizeof_struct_value_round_trip() {
    // Pre-fix the AOT lower silently rejected
    // `__builtin_sizeof(s)` whenever `s` was a struct binding —
    // first because `value_scalar` returned None for
    // `Binding::Struct`, then because `compute_byte_size` only
    // handled scalar types. Now the recursion sums field sizes
    // (matching `interpreter/src/evaluation/builtin.rs::object_byte_size`)
    // so a struct probe round-trips on all 3 backends.
    let src = r#"
        struct Point { x: i64, y: i64 }
        struct Frame { p: Point, q: Point }
        fn main() -> u64 {
            val pt: Point = Point { x: 1i64, y: 2i64 }
            if __builtin_sizeof(pt) != 16u64 { return 1u64 }
            val fr: Frame = Frame {
                p: Point { x: 3i64, y: 4i64 },
                q: Point { x: 5i64, y: 6i64 },
            }
            # 16 (Point) + 16 (Point) = 32
            if __builtin_sizeof(fr) != 32u64 { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "builtin_sizeof_struct_value_round_trip");
}

#[test]
fn sizeof_type_arg_round_trip() {
    // POINTER P1: `__builtin_sizeof::<T>()` — the type-argument form
    // must answer the same widths on all 3 backends. Covers the four
    // places a generic parameter can be resolved from: a concrete
    // written type, a generic free function's call arguments, a
    // generic method's receiver, and a `Self`-returning associated
    // call whose `T` exists only in the `val` annotation.
    let src = r#"
        struct Slice2<T> { data: ptr, len: u64 }

        impl<T> Slice2<T> {
            unsafe fn alloc(len: u64, proto: T) -> Self {
                val p: ptr = __builtin_heap_alloc(__builtin_sizeof::<T>() * len)
                __builtin_ptr_write(p, 0u64, proto)
                Slice2 { data: p, len: len }
            }
            unsafe fn get(&self, i: u64) -> T {
                val v: T = __builtin_ptr_read::<T>(self.data, i * __builtin_sizeof::<T>())
                v
            }
        }

        fn elem_size<T>(probe: T) -> u64 {
            __builtin_sizeof::<T>()
        }

        unsafe fn main() -> u64 {
            # Concrete written types.
            if __builtin_sizeof::<u64>() != 8u64 { return 1u64 }
            if __builtin_sizeof::<u8>() != 1u64 { return 2u64 }
            if __builtin_sizeof::<(i64, bool)>() != 9u64 { return 3u64 }
            # SIMD-F32 / SIMD: the tree-walker's declared-type walk
            # used to miss these two widths (the value form answered
            # them via the runtime Object, so only the type form
            # diverged).
            if __builtin_sizeof::<f32>() != 4u64 { return 8u64 }
            if __builtin_sizeof::<f64x2>() != 16u64 { return 9u64 }
            # Generic free function: T from the call arguments.
            if elem_size(0u64) != 8u64 { return 4u64 }
            if elem_size(1i8) != 1u64 { return 5u64 }
            # Generic method: T from the receiver; associated call: T
            # from the val annotation (no T-bearing argument exists).
            val s: Slice2<u64> = Slice2::alloc(2u64, 7u64)
            if s.get(0u64) != 7u64 { return 6u64 }
            if __builtin_sizeof::<Slice2<u64>>() != 16u64 { return 7u64 }
            42u64
        }
    "#;
    assert_consistent(src, "sizeof_type_arg_round_trip");
}

#[test]
fn vec_of_vec_round_trip() {
    // AOT-COMPOUND-PTR-RW: `Vec<T>` for compound `T` (struct
    // here) round-trips through the heap buffer because
    // `__builtin_ptr_write/read` now expand into per-leaf
    // scalar reads / writes. Pre-fix the AOT lower bailed at
    // "ptr_write value produced no value" the first time
    // `Vec<u8>::push` tried to store a compound element.
    let src = r#"
        fn main() -> u64 {
            var outer: Vec<String> = Vec::new()
            val a: String = String::from_str("hi")
            val b: String = String::from_str("world")
            outer.push(a)
            outer.push(b)
            if outer.size() != 2u64 { return 1u64 }
            val first: &String = outer.borrow(0u64)
            if first.size() != 2u64 { return 2u64 }
            val second: &String = outer.borrow(1u64)
            if second.size() != 5u64 { return 3u64 }
            42u64
        }
    "#;
    assert_consistent(src, "vec_of_vec_round_trip");
}

#[test]
fn string_split_round_trip() {
    // `Split<Vec<u8>, Vec<String>>` for String, riding on the
    // AOT-COMPOUND-PTR-RW landing. Three-way pin: AOT did not
    // type-check this trait return shape before the lower fix.
    let src = r#"
        fn main() -> u64 {
            val s: String = String::from_str("a,b,c")
            val sep: String = String::from_str(",")
            val parts: Vec<String> = s.split(sep)
            if parts.size() != 3u64 { return 1u64 }
            val a: &String = parts.borrow(0u64)
            val b: &String = parts.borrow(1u64)
            val c: &String = parts.borrow(2u64)
            val ea: String = String::from_str("a")
            val eb: String = String::from_str("b")
            val ec: String = String::from_str("c")
            if !a.eq(ea) { return 2u64 }
            if !b.eq(eb) { return 3u64 }
            if !c.eq(ec) { return 4u64 }
            42u64
        }
    "#;
    assert_consistent(src, "string_split_round_trip");
}

#[test]
fn struct_ord_cmp_operator_overload_round_trip() {
    // OP-OVERLOAD-EXTEND Phase 2: `<` `<=` `>` `>=` dispatch to
    // `lt` / `le` / `gt` / `ge` methods (each `(&self, &Self) -> bool`).
    let src = r#"
        struct N { v: i64 }
        impl N {
            fn lt(&self, other: &N) -> bool { self.v < other.v }
            fn le(&self, other: &N) -> bool { self.v <= other.v }
            fn gt(&self, other: &N) -> bool { self.v > other.v }
            fn ge(&self, other: &N) -> bool { self.v >= other.v }
            fn eq(&self, other: &N) -> bool { self.v == other.v }
        }
        fn main() -> u64 {
            val a: N = N { v: 1i64 }
            val b: N = N { v: 2i64 }
            if !(a < b) { return 1u64 }
            if a > b { return 2u64 }
            if !(a <= b) { return 3u64 }
            if a >= b { return 4u64 }
            42u64
        }
    "#;
    assert_consistent(src, "struct_ord_cmp_operator_overload_round_trip");
}

#[test]
fn struct_bitwise_operator_overload_round_trip() {
    // OP-OVERLOAD-EXTEND Phase 3: `&` `|` `^` `<<` `>>` dispatch
    // to `bitand` / `bitor` / `bitxor` / `shl` / `shr`. Self
    // return → AOT routes through `let_lowering.rs::Binary` arm
    // (CallStruct into fresh binding).
    let src = r#"
        struct Bits { v: u64 }
        impl Bits {
            fn bitand(&self, other: &Bits) -> Bits { Bits { v: self.v & other.v } }
            fn bitor(&self, other: &Bits) -> Bits { Bits { v: self.v | other.v } }
            fn bitxor(&self, other: &Bits) -> Bits { Bits { v: self.v ^ other.v } }
            fn shl(&self, other: &Bits) -> Bits { Bits { v: self.v << other.v } }
            fn shr(&self, other: &Bits) -> Bits { Bits { v: self.v >> other.v } }
            fn eq(&self, other: &Bits) -> bool { self.v == other.v }
        }
        fn main() -> u64 {
            val a: Bits = Bits { v: 0xF0u64 }
            val b: Bits = Bits { v: 0x0Fu64 }
            val and_result: Bits = a & b
            val expect_and: Bits = Bits { v: 0u64 }
            if !(and_result == expect_and) { return 1u64 }
            val or_result: Bits = a | b
            val expect_or: Bits = Bits { v: 0xFFu64 }
            if !(or_result == expect_or) { return 2u64 }
            val sh: Bits = Bits { v: 4u64 }
            val one: Bits = Bits { v: 1u64 }
            val sl: Bits = one << sh
            val expect_sl: Bits = Bits { v: 16u64 }
            if !(sl == expect_sl) { return 3u64 }
            42u64
        }
    "#;
    assert_consistent(src, "struct_bitwise_operator_overload_round_trip");
}

#[test]
fn struct_unary_operator_overload_round_trip() {
    // OP-OVERLOAD-EXTEND Phase 4: `-` `~` `!` dispatch to `neg`
    // / `bitnot` / `not` (each `fn (&self) -> Self`). New
    // single-arg path in `let_lowering.rs` for the `Self` return.
    let src = r#"
        struct Sign { v: i64 }
        impl Sign {
            fn neg(&self) -> Sign { Sign { v: 0i64 - self.v } }
            fn bitnot(&self) -> Sign { Sign { v: ~self.v } }
            fn eq(&self, other: &Sign) -> bool { self.v == other.v }
        }
        fn main() -> u64 {
            val a: Sign = Sign { v: 5i64 }
            val n: Sign = -a
            val expect_n: Sign = Sign { v: 0i64 - 5i64 }
            if !(n == expect_n) { return 1u64 }
            val bn: Sign = ~a
            val expect_bn: Sign = Sign { v: ~5i64 }
            if !(bn == expect_bn) { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "struct_unary_operator_overload_round_trip");
}

#[test]
fn struct_compound_assign_operator_overload_round_trip() {
    // OP-OVERLOAD-EXTEND Phase 1: `a += b` desugars to
    // `a = a + b`, which routes through `assign.rs::lower_assign`.
    // The new struct-binding compound-assign arm there detects
    // the desugared shape and emits `CallStruct` into the
    // existing leaf locals (instead of bailing with the "compiler
    // MVP cannot reassign a struct binding whole" diagnostic).
    let src = r#"
        struct Vec3 { x: i64, y: i64, z: i64 }

        impl Vec3 {
            fn add(&self, other: &Vec3) -> Vec3 {
                Vec3 { x: self.x + other.x, y: self.y + other.y, z: self.z + other.z }
            }
            fn sub(&self, other: &Vec3) -> Vec3 {
                Vec3 { x: self.x - other.x, y: self.y - other.y, z: self.z - other.z }
            }
            fn eq(&self, other: &Vec3) -> bool {
                self.x == other.x && self.y == other.y && self.z == other.z
            }
        }

        fn main() -> u64 {
            var a: Vec3 = Vec3 { x: 1i64, y: 2i64, z: 3i64 }
            val b: Vec3 = Vec3 { x: 10i64, y: 20i64, z: 30i64 }
            a += b
            val expect_after_add: Vec3 = Vec3 { x: 11i64, y: 22i64, z: 33i64 }
            if !(a == expect_after_add) { return 1u64 }
            a -= b
            val expect_after_sub: Vec3 = Vec3 { x: 1i64, y: 2i64, z: 3i64 }
            if !(a == expect_after_sub) { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "struct_compound_assign_operator_overload_round_trip");
}

#[test]
fn struct_bitwise_compound_assign_operator_overload_round_trip() {
    // COMPOUND-ASSIGN-BITWISE: `&= |= ^= <<= >>=` desugar the same way
    // the arithmetic five do, so they inherit the operator overload
    // dispatch (`bitand` / `bitor` / `bitxor` / `shl` / `shr`) without
    // the type checker or any backend learning a new shape. Pinned
    // 3-way because that inheritance is the whole claim.
    let src = r#"
        struct Flags { bits: u64 }

        impl Flags {
            fn bitand(&self, other: &Flags) -> Flags { Flags { bits: self.bits & other.bits } }
            fn bitor(&self, other: &Flags) -> Flags { Flags { bits: self.bits | other.bits } }
            fn bitxor(&self, other: &Flags) -> Flags { Flags { bits: self.bits ^ other.bits } }
            fn shl(&self, other: &Flags) -> Flags { Flags { bits: self.bits << other.bits } }
            fn shr(&self, other: &Flags) -> Flags { Flags { bits: self.bits >> other.bits } }
            fn eq(&self, other: &Flags) -> bool { self.bits == other.bits }
        }

        fn main() -> u64 {
            var f: Flags = Flags { bits: 0x0Cu64 }
            val mask: Flags = Flags { bits: 0x0Au64 }
            val one: Flags = Flags { bits: 0x01u64 }
            val low: Flags = Flags { bits: 0x0Fu64 }
            val three: Flags = Flags { bits: 3u64 }
            val two: Flags = Flags { bits: 2u64 }
            val after_and: Flags = Flags { bits: 0x08u64 }
            val after_or: Flags = Flags { bits: 0x09u64 }
            val after_xor: Flags = Flags { bits: 0x06u64 }
            val after_shl: Flags = Flags { bits: 0x30u64 }
            val after_shr: Flags = Flags { bits: 0x0Cu64 }
            f &= mask
            if !(f == after_and) { return 1u64 }
            f |= one
            if !(f == after_or) { return 2u64 }
            f ^= low
            if !(f == after_xor) { return 3u64 }
            f <<= three
            if !(f == after_shl) { return 4u64 }
            f >>= two
            if !(f == after_shr) { return 5u64 }
            42u64
        }
    "#;
    assert_consistent(src, "struct_bitwise_compound_assign_operator_overload_round_trip");
}

#[test]
fn operator_overload_written_with_self_round_trip() {
    // `docs/language.md` spells every overload as
    // `fn op(&self, other: &Self) -> Self`, but the compiled lanes
    // used to reject exactly that: the `Self` substitution in
    // `compiler_lower` was a top-level match, so `&Self` arrived at
    // `lower_param_or_return_type` as `Ref { inner: Self_ }` and
    // failed with "cannot lower method parameter". Only the
    // `other: &Flags` spelling worked. Pinned 3-way so the
    // documented form stays runnable everywhere.
    let src = r#"
        struct Flags { bits: u64 }

        impl Flags {
            fn bitand(&self, other: &Self) -> Self { Flags { bits: self.bits & other.bits } }
            fn bitor(&self, other: &Self) -> Self { Flags { bits: self.bits | other.bits } }
            fn add(&self, other: &Self) -> Self { Flags { bits: self.bits + other.bits } }
            fn eq(&self, other: &Self) -> bool { self.bits == other.bits }
        }

        fn main() -> u64 {
            var f: Flags = Flags { bits: 0x0Cu64 }
            val mask: Flags = Flags { bits: 0x0Au64 }
            val one: Flags = Flags { bits: 0x01u64 }
            val expect: Flags = Flags { bits: 0x09u64 }
            f &= mask
            f |= one
            if !(f == expect) { return 1u64 }
            val sum: Flags = f + one
            val expect_sum: Flags = Flags { bits: 0x0Au64 }
            if !(sum == expect_sum) { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "operator_overload_written_with_self_round_trip");
}

#[test]
fn self_inside_a_by_value_and_mutable_reference_position_round_trip() {
    // The same substitution now recurses, so `&mut Self` and a
    // by-value `Self` argument lower too — including the trait-method
    // form, where `Self` is the only way to name the type.
    let src = r#"
        struct P { v: u64 }

        trait Doubler {
            fn twice(self: Self) -> Self
        }

        impl Doubler for P {
            fn twice(self: Self) -> Self { P { v: self.v * 2u64 } }
        }

        impl P {
            fn combine(&self, other: Self) -> Self { P { v: self.v + other.v } }
            fn bump(&mut self, other: &mut Self) { self.v = self.v + other.v }
        }

        fn main() -> u64 {
            val a: P = P { v: 3u64 }
            val b: P = P { v: 4u64 }
            val c: P = a.combine(b)
            val d: P = c.twice()
            var e: P = P { v: 1u64 }
            var g: P = P { v: 2u64 }
            e.bump(&mut g)
            d.v + e.v
        }
    "#;
    assert_consistent(src, "self_inside_a_by_value_and_mutable_reference_position_round_trip");
}

#[test]
fn struct_arith_operator_overload_round_trip() {
    // Operator overload (Phase B continuation): `+` / `-` / `*` /
    // `/` / `%` between matching struct values dispatch to the
    // user-defined `add` / `sub` / `mul` / `div` / `rem` method.
    // Pinned 3-way to lock in: frontend type checker accepts the
    // overload before `resolve_numeric_types` would reject it,
    // interpreter routes through `evaluate_binary`'s extended
    // `overload_method_name` table, AOT routes through
    // `let_lowering.rs`'s arithmetic struct arm (CallStruct into
    // a fresh binding for the compound `Self` return).
    let src = r#"
        struct Vec3 { x: i64, y: i64, z: i64 }

        impl Vec3 {
            fn add(&self, other: &Vec3) -> Vec3 {
                Vec3 { x: self.x + other.x, y: self.y + other.y, z: self.z + other.z }
            }
            fn sub(&self, other: &Vec3) -> Vec3 {
                Vec3 { x: self.x - other.x, y: self.y - other.y, z: self.z - other.z }
            }
            fn eq(&self, other: &Vec3) -> bool {
                self.x == other.x && self.y == other.y && self.z == other.z
            }
        }

        fn main() -> u64 {
            val a: Vec3 = Vec3 { x: 1i64, y: 2i64, z: 3i64 }
            val b: Vec3 = Vec3 { x: 10i64, y: 20i64, z: 30i64 }
            val sum: Vec3 = a + b
            val expect_sum: Vec3 = Vec3 { x: 11i64, y: 22i64, z: 33i64 }
            if !(sum == expect_sum) { return 1u64 }
            val diff: Vec3 = b - a
            val expect_diff: Vec3 = Vec3 { x: 9i64, y: 18i64, z: 27i64 }
            if !(diff == expect_diff) { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "struct_arith_operator_overload_round_trip");
}

#[test]
fn string_eq_operator_round_trip() {
    // Phase B operator overload — `s == t` / `s != t` between
    // two String values dispatch to the user-defined `eq` method.
    // Pinned 3-way: interpreter does it via
    // `evaluate_binary` early dispatch; AOT does it via
    // `lower_binary::try_lower_struct_eq` (Call to the resolved
    // `eq` FuncId with leaf locals as args); JIT silently falls
    // back to the interpreter for struct-typed binaries.
    let src = r#"
        fn main() -> u64 {
            val a: String = String::from_str("hello")
            val b: String = String::from_str("hello")
            val c: String = String::from_str("world")
            if !(a == b) { return 1u64 }
            if a == c { return 2u64 }
            if a != b { return 3u64 }
            if !(a != c) { return 4u64 }
            42u64
        }
    "#;
    assert_consistent(src, "string_eq_operator_round_trip");
}

#[test]
fn string_from_str_via_alias_round_trip() {
    // `String::from_str("...")` — the alias `type String = Vec<u8>`
    // is now rewritten in expression position too, so the qualifier
    // resolves to `Vec` and dispatch picks up the existing
    // `impl Vec<u8>::from_str` from the val annotation. Pinned
    // 3-way to confirm the alias rewrite happens in every backend.
    let src = r#"
        fn main() -> u64 {
            val s: String = String::from_str("hello")
            val expected: String = String::from_str("hello")
            if s.len() != 5u64 { return 1u64 }
            if !s.eq(expected) { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "string_from_str_via_alias_round_trip");
}

#[test]
fn string_concat_round_trip() {
    // `Concat` trait + per-byte push body. Pinning confirms the
    // let-rhs path's identifier-flatten extension (which lets a
    // `Vec<u8>` argument decompose into leaf locals across the
    // cranelift call ABI) works for compound-returning trait
    // methods.
    let src = r#"
        fn main() -> u64 {
            val a: String = String::from_str("hello")
            val b: String = String::from_str(" world")
            val c: String = a.concat(b)
            val expected: String = String::from_str("hello world")
            if !c.eq(expected) { return 1u64 }
            42u64
        }
    "#;
    assert_consistent(src, "string_concat_round_trip");
}

#[test]
fn string_contains_round_trip() {
    let src = r#"
        fn main() -> u64 {
            val s: String = String::from_str("hello world")
            val present: String = String::from_str("o w")
            val absent: String = String::from_str("xyz")
            if !s.contains(present) { return 1u64 }
            if s.contains(absent) { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "string_contains_round_trip");
}

#[test]
fn str_to_string_round_trip() {
    // STRING-NOMINAL: `str.to_string()` impl was retired when the
    // `ToString` trait left stdlib. User code uses
    // `String::from_str(s)` instead.
    let src = r#"
        fn main() -> u64 {
            val s = "literal"
            val owned: String = String::from_str(s)
            val expected: String = String::from_str("literal")
            if !owned.eq(expected) { return 1u64 }
            if owned.len() != 7u64 { return 2u64 }
            42u64
        }
    "#;
    assert_consistent(src, "str_to_string_round_trip");
}

#[test]
fn push_char_three_byte_utf8_round_trip() {
    // `Vec<u8>::push_char` UTF-8 encoding for codepoint 0x3042 ('あ').
    // Expected bytes: [0xE3, 0x81, 0x82].
    let src = r#"
        fn main() -> u64 {
            var s: Vec<u8> = Vec::new()
            s.push_char(0x3042u32)
            if s.size() != 3u64 { return 1u64 }
            if s.get(0u64) != 0xE3u8 { return 2u64 }
            if s.get(1u64) != 0x81u8 { return 3u64 }
            if s.get(2u64) != 0x82u8 { return 4u64 }
            42u64
        }
    "#;
    assert_consistent(src, "push_char_three_byte_utf8_round_trip");
}

#[test]
fn vec_user_space_round_trip() {
    // `core/std/collections/vec.t::Vec<T>` is the user-space
    // dynamic array sibling to `core/std/dict.t::Dict<K, V>`.
    // Built entirely on `__builtin_heap_alloc` /
    // `__builtin_heap_realloc` / `__builtin_ptr_read` /
    // `__builtin_ptr_write` / `__builtin_sizeof` — no
    // special-casing in the parser, type checker, or any
    // backend. Mutating methods (`push`, `pop`, `set`) use
    // `&mut self` so the AOT Self-out-parameter writeback
    // (Stage 1 of `&` references) propagates `self.cap` /
    // `self.data` / `self.len` updates back to the caller's
    // binding.
    //
    // Coverage:
    //   - `Vec::new()` (associated function on a generic struct
    //     — DICT-AOT-NEW Phase B)
    //   - `push` past the initial capacity, exercising the
    //     geometric grow path (`heap_realloc`)
    //   - `get` random read
    //   - `set` random write
    //   - `pop` and the resulting `size()` decrement
    //   - `is_empty()` true / false transitions
    //
    // Exit 42 means every step matched. Any digit 1..7 names the
    // step that failed first.
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            var i: u64 = 0u64
            while i < 10u64 {
                v.push(i * 11u64)
                i = i + 1u64
            }
            val a: u64 = v.get(0u64)
            val b: u64 = v.get(5u64)
            val c: u64 = v.get(9u64)
            v.set(5u64, 999u64)
            val d: u64 = v.get(5u64)
            val sz_before: u64 = v.size()
            val popped: u64 = v.pop()
            val sz_after: u64 = v.size()
            val empty_before: bool = v.is_empty()
            if a != 0u64 { 1u64 }
            elif b != 55u64 { 2u64 }
            elif c != 99u64 { 3u64 }
            elif d != 999u64 { 4u64 }
            elif sz_before != 10u64 { 5u64 }
            elif popped != 99u64 { 6u64 }
            elif sz_after != 9u64 { 7u64 }
            elif empty_before { 8u64 }
            else { 42u64 }
        }
    "#;
    assert_consistent(src, "vec_user_space_round_trip");
}

#[test]
fn str_len_extension_method_round_trip() {
    // `core/std/str.t::Length::len(self) -> u64` returns the byte
    // count of the string. AOT lowers to a libc `strlen` call;
    // the `.rodata` per-literal layout
    // (`[bytes][NUL][u64 len]` per `declare_print_string`) keeps
    // the trailing NUL precisely so the strlen walk terminates
    // at the right position. Interpreter / JIT (silent fallback)
    // return `s.bytes().len()` directly.
    //
    // Mixes empty, ASCII, and short literals to confirm the
    // length matches across all three backends (interpreter,
    // JIT silent fallback, AOT).
    let src = r#"
        fn main() -> u64 {
            val empty = ""
            val short = "hi"
            val mid = "hello"
            empty.len() + short.len() + mid.len()
        }
    "#;
    assert_consistent(src, "str_len_extension_method_round_trip");
}

#[test]
fn str_as_ptr_extension_method_round_trip() {
    // `core/std/str.t::AsPtr::as_ptr(self) -> ptr` is the user-
    // facing entry point for the byte-pointer view of a string;
    // the body delegates to the underlying
    // `__builtin_str_to_ptr` primitive. This test confirms the
    // extension-trait dispatch reaches the same backend path
    // across all three backends — interpreter / JIT (silent
    // fallback through interpreter, since str scalar isn't
    // modelled in the JIT IR) / AOT.
    //
    // Walks "hi" byte-by-byte through the method form. Exit 42
    // means each byte ('h'=104, 'i'=105, NUL=0) matched.
    let src = r#"
        unsafe fn main() -> u64 {
            val s = "hi"
            val p: ptr = s.as_ptr()
            val a: u8 = __builtin_ptr_read::<u8>(p, 0u64)
            val b: u8 = __builtin_ptr_read::<u8>(p, 1u64)
            val nul: u8 = __builtin_ptr_read::<u8>(p, 2u64)
            if a == 104u8 {
                if b == 105u8 {
                    if nul == 0u8 { 42u64 } else { 3u64 }
                } else { 2u64 }
            } else { 1u64 }
        }
    "#;
    assert_consistent(src, "str_as_ptr_extension_method_round_trip");
}

#[test]
fn str_to_ptr_byte_walk_round_trip() {
    // `__builtin_str_to_ptr(s: str) -> ptr` returns a pointer to
    // the string's UTF-8 bytes (NUL-terminated). 3-way
    // `assert_consistent`:
    //   - AOT: identity — `Type::Str` already lowers to a
    //     pointer-sized handle into `.rodata`, so the cast is a
    //     no-op at the cranelift level.
    //   - JIT: silent fallback (eligibility rejects, interpreter
    //     handles it).
    //   - Interpreter: heap-allocates `len + 1` bytes via the
    //     active allocator, stores each byte as `Object::U8` in
    //     typed_slots so `__builtin_ptr_read(p, i)` with a
    //     `val: u8 = ...` annotation returns the byte at offset i,
    //     plus the NUL terminator at offset `len`.
    //
    // Walks "hi" byte-by-byte, checking 'h'=104, 'i'=105, NUL=0.
    // Exit 42 means every byte matched.
    let src = r#"
        unsafe fn main() -> u64 {
            val s = "hi"
            val p: ptr = __builtin_str_to_ptr(s)
            val a: u8 = __builtin_ptr_read::<u8>(p, 0u64)
            val b: u8 = __builtin_ptr_read::<u8>(p, 1u64)
            val nul: u8 = __builtin_ptr_read::<u8>(p, 2u64)
            if a == 104u8 {
                if b == 105u8 {
                    if nul == 0u8 { 42u64 } else { 3u64 }
                } else { 2u64 }
            } else { 1u64 }
        }
    "#;
    assert_consistent(src, "str_to_ptr_byte_walk_round_trip");
}

// --- `__builtin_str_from_bytes` ------------------------------------
//
// The inverse of `str_to_ptr`, and the only way to build a `str` from
// bytes computed at runtime. Asserted on stdout because printing is
// what users do with the result, and because the text is the thing
// that would change.

#[test]
fn str_from_bytes_round_trips_through_a_buffer() {
    let src = r#"
        unsafe fn main() -> u64 {
            val src_str = "hi"
            val p: ptr = __builtin_str_to_ptr(src_str)
            val back: str = __builtin_str_from_bytes(p, 2u64)
            println(back)
            println("[{back}]")
            println(__builtin_str_len(back))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "str_from_bytes_round_trip");
}

#[test]
fn str_from_bytes_reads_bytes_the_engines_store_differently() {
    // A `__builtin_ptr_write` of a narrow integer lands only in the
    // interpreter's typed-slot map — the raw byte buffer is stamped
    // for 64-bit writes alone, on both the tree-walker and the IR VM.
    // Reading only the raw buffer made this print five NUL bytes on
    // the interpreter and `hello` on the compiled backends, so
    // `HeapManager::read_byte_at` is the one place that knows where a
    // byte actually lives.
    let src = r#"
        unsafe fn main() -> u64 {
            val p: ptr = __builtin_heap_alloc(5u64)
            __builtin_ptr_write(p, 0u64, 104u8)
            __builtin_ptr_write(p, 1u64, 101u8)
            __builtin_ptr_write(p, 2u64, 108u8)
            __builtin_ptr_write(p, 3u64, 108u8)
            __builtin_ptr_write(p, 4u64, 111u8)
            val s: str = __builtin_str_from_bytes(p, 5u64)
            __builtin_heap_free(p)
            println(s)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "str_from_bytes_typed_slots");
}

#[test]
fn a_str_built_from_bytes_survives_the_buffer_changing() {
    // `str_from_bytes` copies. A str that aliased the buffer would
    // change under the program's feet, and differently per backend:
    // the interpreter stores an owned String, the compiled backends
    // malloc a fresh block.
    let src = r#"
        unsafe fn main() -> u64 {
            val p: ptr = __builtin_heap_alloc(2u64)
            __builtin_ptr_write(p, 0u64, 104u8)
            __builtin_ptr_write(p, 1u64, 105u8)
            val s: str = __builtin_str_from_bytes(p, 2u64)
            __builtin_ptr_write(p, 0u64, 88u8)
            println(s)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "str_from_bytes_copies");
}

#[test]
fn narrow_int_unary_operators_agree_across_backends() {
    // NUM-W: `docs/language.md` says the narrow widths "work identically
    // to `u64` / `i64` ... the type checker / interpreter / JIT / AOT
    // compiler all carry the width through end-to-end". Unary `-` and `~`
    // did not: the type checker's unary arm tested `== TypeDecl::Int64`
    // and `== TypeDecl::UInt64` rather than asking whether the type was an
    // integer, so `-a` for `a: i32` and `~c` for `c: u8` were type errors.
    //
    // The width matters to the answer, not just to the annotation: `~` has
    // to complement at the operand's own width, so `~3u8` is `252` and not
    // a widened `18446744073709551612`.
    let src = r#"
        fn main() -> u64 {
            val a: i8 = 5i8
            val b: i16 = 300i16
            val c: i32 = 70000i32
            val d: u8 = 3u8
            val e: u16 = 300u16
            val f: u32 = 70000u32
            println(-a)
            println(-b)
            println(-c)
            println(~d)
            println(~e)
            println(~f)
            println(~a)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "narrow_int_unary");
}

#[test]
fn negating_an_unsigned_value_is_still_rejected() {
    // The widening above is to the *signed* widths only. `-x` on an
    // unsigned type has no value to take, so it stays a type error at
    // every width rather than wrapping.
    for src in [
        "fn main() -> u64 { val a: u8 = 3u8
 val b: u8 = -a
 0u64 }",
        "fn main() -> u64 { val a: u32 = 3u32
 val b: u32 = -a
 0u64 }",
        "fn main() -> u64 { val a: u64 = 3u64
 val b: u64 = -a
 0u64 }",
    ] {
        let errors = type_check_errors(src);
        assert!(
            errors.iter().any(|e| e.contains("unary minus")),
            "expected a unary-minus rejection for:\n{src}\ngot: {errors:?}"
        );
    }
}

// CHAR-LITERAL-NUM: a char literal is a `u32`, and a position naming
// another integer width may take it when the code point fits. The
// three backends have to agree on both halves — the width the value
// ends up at, and the arithmetic done on it.

#[test]
fn char_literals_take_the_width_the_position_asks_for() {
    let src = r#"
        fn as_byte(b: u8) -> u64 { b as u64 }

        fn main() -> u64 {
            val kept: u32 = 'a'
            val narrowed: u8 = '0'
            val widened: i64 = '\n'
            val inferred = 'z'
            as_byte('A')
                + (kept as u64)
                + (narrowed as u64)
                + (widened as u64)
                + (inferred as u64)
                + (__builtin_sizeof(inferred))
        }
    "#;
    // 65 + 97 + 48 + 10 + 122 + 4 (the inferred literal is a u32).
    assert_consistent(src, "char_literal_widths");
}

#[test]
fn a_strings_bytes_compare_against_characters() {
    let src = r#"
        fn main() -> u64 {
            val s = String::from_str("hello 42")
            var ells: u64 = 0u64
            for b in s.iter() {
                if b == 'l' { ells = ells + 1u64 }
            }
            var digits: u64 = 0u64
            var i: u64 = 0u64
            while i < s.size() {
                val c: u8 = s.get(i)
                if c >= '0' && c <= '9' {
                    digits = digits * 10u64 + ((c - '0') as u64)
                }
                i = i + 1u64
            }
            ells * 100u64 + digits
        }
    "#;
    // Two `l`s, and the digits read back as 42.
    assert_consistent(src, "char_literal_bytes");
}

// STDLIB-TEXT T0 / §2: `str` holds valid UTF-8, and the byte -> str
// door is where that is established.
//
// This used to be the clearest disagreement in the language: the same
// program answered 6 on the tree-walker and 2 on the compiled lanes,
// because only the tree-walker substituted U+FFFD for bytes it could
// not decode (three bytes each, and `len()` counted them). Neither
// answer was better than the other; what was missing was a decision
// about what `str` holds.

#[test]
fn bytes_that_are_not_text_do_not_become_a_str() {
    let src = r#"
        fn main() -> u64 {
            var s = String::new()
            s.push(255u8)
            s.push(254u8)
            val t: str = s.to_str()
            t.len()
        }
    "#;
    // Every lane refuses, so there is no length to disagree about.
    let out = compiled_run_streams(src, "str_from_bytes_invalid_utf8");
    if let Some((code, _stdout, stderr)) = out {
        assert_ne!(code, 0, "an invalid str was accepted");
        assert!(
            stderr.contains("not valid UTF-8"),
            "the refusal does not say what was wrong:\n{stderr}"
        );
    }
}

#[test]
fn a_string_can_be_asked_whether_it_is_text_first() {
    // The discipline the rest of the stdlib uses for failures that
    // would otherwise need a `Result` on every call: ask once, before
    // crossing. Covers each way RFC 3629 says a sequence is invalid.
    let src = r#"
        fn main() -> u64 {
            val good = String::from_str("héllo, 世界 🌏")
            var bad = String::new()
            bad.push(255u8)
            var lone = String::new()
            lone.push(226u8)          # a 3-byte lead with no continuations
            var overlong = String::new()
            overlong.push(192u8)      # 2-byte lead below the shortest form
            overlong.push(175u8)
            var surrogate = String::new()
            surrogate.push(237u8)     # U+D800, which is not a scalar value
            surrogate.push(160u8)
            surrogate.push(128u8)
            println(good.is_utf8())
            println(bad.is_utf8())
            println(lone.is_utf8())
            println(overlong.is_utf8())
            println(surrogate.is_utf8())
            0u64
        }
    "#;
    assert_stdout_consistent(src, "string_is_utf8");
}

#[test]
fn strs_sort_by_bytes_which_is_codepoint_order() {
    // STDLIB-TEXT §5. `Vec<str>::sort()` was a bound violation before
    // there was an `Ord for str` -- the last primitive without one.
    //
    // The order is bytes, so uppercase sorts before lowercase and a
    // prefix sorts before what extends it. That is not a collation and
    // is not meant to be one.
    let src = r#"
        fn main() -> u64 {
            var v: Vec<str> = Vec::new()
            v.push("pear")
            v.push("apple")
            v.push("Banana")
            v.push("apple pie")
            v.push("Ω")
            v.push("é")
            v.sort()
            var i: u64 = 0u64
            while i < v.size() {
                println(v.get(i))
                i = i + 1u64
            }
            0u64
        }
    "#;
    assert_stdout_consistent(src, "str_ord_sort");
}

#[test]
fn strs_compare_with_the_operators() {
    // `impl Ord for str` gave `str` an ordering that only `a.lt(b)`
    // could reach, because operator overloading dispatches on a struct
    // receiver. The comparison is rewritten into that call instead, so
    // the four operators work and mean what `Vec<str>::sort()` means.
    //
    // `Ord` declares only `lt`, so the other three are spelled with
    // it. Equal operands are the case that catches a wrong spelling:
    // every one of the four has a different answer there.
    let src = r#"
        fn main() -> u64 {
            val a: str = "abc"
            val b: str = "abd"
            val c: str = "abc"
            println(a < b)
            println(b < a)
            println(a < c)
            println(a <= c)
            println(a >= c)
            println(a > c)
            println(b > a)
            println(a >= b)
            # A prefix sorts before what extends it, and bytes put
            # uppercase first -- the same order `sort` uses.
            val p: str = "ab"
            val u: str = "Abc"
            println(p < a)
            println(u < a)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "str_compare_operators");
}

#[test]
fn str_comparisons_work_wherever_they_are_written() {
    // The rewrite is a post-pass over the pool rather than an
    // interception, because an operand of another operator, a
    // condition and a tail expression each reach the checker by a
    // different route and only some carry the node's own reference.
    let src = r#"
        fn take(b: bool) -> u64 { if b { 1u64 } else { 0u64 } }
        fn cmp(a: str, b: str) -> bool { a < b }

        fn main() -> u64 {
            val a: str = "abc"
            val b: str = "abd"
            var n: u64 = 0u64
            n = n + take(a < b)              # argument
            var i: u64 = 0u64
            while a < b && i < 1u64 { i = i + 1u64 }   # condition, and an operand of `&&`
            n = n + i
            if cmp(a, b) { n = n + 1u64 }    # a function's tail
            val negated: bool = !(a < b)     # operand of a unary
            if negated { n = n + 10u64 }
            val r = a < b                    # a plain binding
            val m = match r { true => 1u64, false => 0u64 }
            n + m
        }
    "#;
    assert_consistent(src, "str_compare_positions");
}

// STDLIB-TEXT T3: ASCII classification on `u8` and `u32`.
//
// Both widths, because `String::get` hands back a `u8` while
// `push_char` takes a `u32` — one impl would make every call spell a
// cast that carries no information. Everything outside ASCII answers
// false and converts to itself, which is what the names promise.

#[test]
fn ascii_classification_answers_for_both_widths() {
    let src = r#"
        fn main() -> u64 {
            val b: u8 = '7'
            val c: u32 = 'Q'
            println(b.is_ascii_digit())
            println(b.is_ascii_alpha())
            println(b.is_ascii_alnum())
            println(c.is_ascii_alpha())
            println(c.is_ascii_upper())
            println(c.to_ascii_lower())
            println(b.to_ascii_upper())
            val sp: u8 = ' '
            println(sp.is_ascii_space())
            # Past ASCII: false, and unchanged by the conversions.
            val hi: u8 = 200u8
            println(hi.is_ascii())
            println(hi.is_ascii_alpha())
            println(hi.to_ascii_upper())
            0u64
        }
    "#;
    assert_stdout_consistent(src, "ascii_class");
}

#[test]
fn digit_value_reads_a_character_in_any_radix() {
    let src = r#"
        fn show(d: Option<u32>) -> u64 {
            match d {
                Option::Some(v) => v as u64,
                Option::None => 99u64,
            }
        }

        fn main() -> u64 {
            val f: u8 = 'f'
            val nine: u8 = '9'
            val z: u8 = 'Z'
            # Bound first: the compiled lanes refuse a compound return
            # in expression position.
            val hex = f.digit_value(16u32)
            val dec = f.digit_value(10u32)      # not a decimal digit
            val d9 = nine.digit_value(10u32)
            val b36 = z.digit_value(36u32)
            val bin = nine.digit_value(2u32)    # out of range for binary
            println(show(hex))
            println(show(dec))
            println(show(d9))
            println(show(b36))
            println(show(bin))
            0u64
        }
    "#;
    assert_stdout_consistent(src, "digit_value");
}

// STDLIB-TEXT T4: bytes -> codepoints.

#[test]
fn chars_walks_codepoints_not_bytes() {
    let src = r#"
        fn main() -> u64 {
            val s = String::from_str("aé漢🌏")
            var n: u64 = 0u64
            for c in s.chars() {
                println(c)
                n = n + 1u64
            }
            println(n)
            println(s.len())
            0u64
        }
    "#;
    // Four characters, ten bytes: the two numbers differing is the
    // whole point of having both iterators.
    assert_stdout_consistent(src, "chars_iter");
}

#[test]
fn a_broken_sequence_yields_one_replacement_character_and_keeps_going() {
    // `None` already means "the end", so a decode failure cannot be
    // reported there without making every loop unable to tell a bad
    // byte from a finished string. U+FFFD and one byte forward is the
    // `from_utf8_lossy` convention, and it terminates.
    let src = r#"
        fn main() -> u64 {
            var bad = String::new()
            bad.push(97u8)
            bad.push(255u8)     # not a lead byte
            bad.push(226u8)     # a 3-byte lead with nothing after it
            bad.push(98u8)
            for c in bad.chars() { println(c) }
            println(bad.is_utf8())
            0u64
        }
    "#;
    assert_stdout_consistent(src, "chars_lossy");
}

// STDLIB-TEXT T5: the rest of the `String` surface.

#[test]
fn string_searches_and_builds() {
    let src = r#"
        fn at(o: Option<u64>) -> u64 {
            match o {
                Option::Some(i) => i,
                Option::None => 99u64,
            }
        }

        fn main() -> u64 {
            val s = String::from_str("one two  three")
            val two = String::from_str("two")
            val o = String::from_str("o")
            val one = String::from_str("one")
            val three = String::from_str("three")
            val dash = String::from_str("-")
            val f1 = s.find(two)
            val f2 = s.rfind(o)
            val f3 = s.find_from(o, 7u64)
            println(at(f1))
            println(at(f2))
            println(at(f3))
            println(s.starts_with(one))
            println(s.ends_with(three))
            println(s.eq_str("one two  three"))
            println(s.eq_str("nope"))
            val rep = s.replace(two, dash)
            println(rep)
            val bar = dash.repeat(4u64)
            println(bar)
            val ws = s.split_whitespace()
            println(ws.size())
            0u64
        }
    "#;
    assert_stdout_consistent(src, "string_search_build");
}

#[test]
fn lines_drops_the_carriage_return_and_the_trailing_newline() {
    // A CRLF file has to read the same as an LF one, and a trailing
    // newline must not invent a final empty line — every
    // line-oriented tool agrees on both.
    let src = r#"
        fn main() -> u64 {
            val text = String::from_str("a\r\nb\nc\n")
            val ls = text.lines()
            println(ls.size())
            var i: u64 = 0u64
            while i < ls.size() {
                val line: &String = ls.borrow(i)
                println(line)
                i = i + 1u64
            }
            val no_trailing = String::from_str("x\ny")
            val l2 = no_trailing.lines()
            println(l2.size())
            0u64
        }
    "#;
    assert_stdout_consistent(src, "string_lines");
}

#[test]
fn join_is_the_inverse_of_split() {
    // On `String` rather than `Vec`, because a container generic over
    // anything should not grow a method that exists for one element
    // type.
    let src = r#"
        fn main() -> u64 {
            val s = String::from_str("a,b,c")
            val comma = String::from_str(",")
            val parts = s.split(comma)
            val back = String::join(parts, comma)
            println(back)
            println(back.eq_str("a,b,c"))
            val dash = String::from_str(" - ")
            val spaced = String::join(parts, dash)
            println(spaced)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "string_join");
}

#[test]
fn an_associated_function_takes_a_borrowed_container() {
    // ASSOC-FN-REF-ARG: `String::join(&parts, &sep)` — the explicit
    // borrow, which is what a reader writes once the parameter is
    // spelled `&Vec<String>`. It did not lower at all: the three
    // associated-call shapes lowered their arguments one at a time
    // with no callee to ask, so nothing knew a `&`-compound parameter
    // wants an address, and the argument produced no value. The same
    // signature as a *free* function has always worked, so the
    // difference was the call spelling.
    let src = r#"
        fn main() -> u64 {
            val s = String::from_str("a,b,c")
            val comma = String::from_str(",")
            val parts = s.split(comma)
            val dash = String::from_str("-")
            val joined: String = String::join(&parts, &dash)
            println(joined)
            # The parts are still there: `join` borrows them, and
            # borrows the separator too.
            val first: &String = parts.borrow(0u64)
            println(first)
            println(parts.size())
            0u64
        }
    "#;
    assert_stdout_consistent(src, "string_join_explicit_borrow");
}

#[test]
fn push_str_takes_the_literal_everyone_writes_first() {
    // `s.push_str("literal")` used to type-check and then die at run
    // time with `Cannot access field on non-struct object:
    // ConstString`, because the name meant "append a String". The
    // names now say which type they take.
    let src = r#"
        fn main() -> u64 {
            var s = String::new()
            s.push_str("hello")
            s.push_str(", ")
            val w = String::from_str("world")
            s.push_string(w)
            println(s)
            s.len()
        }
    "#;
    assert_consistent(src, "push_str_literal");
}
