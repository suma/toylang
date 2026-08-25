//! Format specs (STR-INTERP-FMT), extended patterns (PATTERN-EXTEND),
//! compound literal call arguments, and enum-typed struct fields.

use super::harness::*;

// STR-INTERP-FMT: format specs (`"{x:.2}"`). The rendered *text* is
// compared across backends by `example_consistency` running
// `interpreter/example/string_format_spec.t` (it diffs stdout); these
// cover the same lowering through the exit-code path, where a
// mis-decoded spec shows up as a different padded length.
//
// The spec constant is packed by `frontend::format_spec` and decoded
// twice — once in `frontend`/the IR VM, once in the `no_std`
// `toylang_rt` — so a divergence in the bit layout is exactly what
// these pin.

#[test]
fn format_spec_width_and_precision_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val pi: f64 = 3.14159265f64
            val n: u64 = 42u64
            val s = "[{pi:.3}][{n:6}][{n:<6}][{n:06}]"
            s.len() as i64
        }
    "#;
    assert_consistent(src, "format_spec_width_precision");
}

#[test]
fn format_spec_radix_round_trip() {
    // A negative value renders its two's-complement pattern at its
    // own width, so the i32 and i64 forms differ in length.
    let src = r#"
        fn main() -> i64 {
            val a: i32 = -1i32
            val b: i64 = -1i64
            val n: u64 = 255u64
            val s = "{a:x}|{b:x}|{n:b}|{n:o}|{n:X}"
            s.len() as i64
        }
    "#;
    assert_consistent(src, "format_spec_radix");
}

#[test]
fn format_spec_on_text_round_trip() {
    let src = r#"
        fn main() -> i64 {
            val t: str = "ok"
            val flag: bool = true
            val s = "[{t:6}][{t:>6}][{t:^7}][{flag:8}]"
            s.len() as i64
        }
    "#;
    assert_consistent(src, "format_spec_text");
}

// PATTERN-EXTEND: or-patterns, ranges, and `@` bindings across the
// three backends. `interpreter/example/match_pattern_extend.t` diffs
// the printed output; these pin the exit-code path, and in particular
// that each backend's handling of `Pattern::Binding` and
// `Pattern::Range` agrees with the tree-walker.

#[test]
fn or_pattern_alternatives_agree_across_backends() {
    let src = r#"
        enum Color { Red, Green, Blue }

        fn warm(c: Color) -> i64 {
            match c {
                Color::Red | Color::Green => 1i64,
                Color::Blue => 0i64,
            }
        }

        fn classify(n: i64) -> i64 {
            match n {
                0i64 | 1i64 | 2i64 => 10i64,
                _ => 20i64,
            }
        }

        fn main() -> i64 {
            val r: Color = Color::Red
            val b: Color = Color::Blue
            warm(r) + warm(b) + classify(1i64) + classify(9i64)
        }
    "#;
    assert_consistent(src, "or_pattern_alternatives");
}

#[test]
fn range_and_at_patterns_agree_across_backends() {
    let src = r#"
        fn describe(n: i64) -> i64 {
            match n {
                x @ 0i64 => x,
                y @ 1i64..10i64 => y * 10i64,
                10i64..100i64 => 5i64,
                _ => -1i64,
            }
        }

        fn main() -> i64 {
            describe(0i64) + describe(3i64) + describe(50i64) + describe(1000i64)
        }
    "#;
    assert_consistent(src, "range_and_at_patterns");
}

#[test]
fn a_synthesized_guard_ands_with_a_user_guard_across_backends() {
    let src = r#"
        fn gated(n: i64, allow: bool) -> i64 {
            match n {
                0i64..10i64 if allow => 1i64,
                0i64..10i64 => 2i64,
                _ => 3i64,
            }
        }

        fn main() -> i64 {
            gated(5i64, true) + gated(5i64, false) + gated(50i64, true)
        }
    "#;
    assert_consistent(src, "pattern_extend_guard_combination");
}


#[test]
fn at_binding_over_patterns_agrees_across_backends() {
    // `@` wraps a pattern rather than desugaring to a guard, so each
    // backend has to peel it: bind the whole matched value, then let
    // the inner pattern decide. The three shapes below cover the
    // scrutinee kinds that bind differently — a scalar copies, an
    // enum copies its storage, a struct aliases the fields.
    let src = r#"
        enum Color { Red, Green, Blue }

        struct Point { x: i64, y: i64 }

        fn rank(c: Color) -> i64 {
            match c {
                Color::Red => 0i64,
                same @ Color::Green => rank_of(same),
                Color::Blue => 2i64,
            }
        }

        fn rank_of(c: Color) -> i64 { 1i64 }

        fn corner(p: Point) -> i64 {
            match p {
                whole @ Point { x: 0i64, y } => whole.y + y,
                Point { x, y } => x + y,
            }
        }

        fn scaled(n: i64) -> i64 {
            match n {
                x @ 7i64 => x * 2i64,
                _ => 0i64,
            }
        }

        fn main() -> i64 {
            val g: Color = Color::Green
            val b: Color = Color::Blue
            val origin: Point = Point { x: 0i64, y: 5i64 }
            val other: Point = Point { x: 2i64, y: 3i64 }
            rank(g) + rank(b) + corner(origin) + corner(other) + scaled(7i64) + scaled(1i64)
        }
    "#;
    assert_consistent(src, "at_binding_over_patterns");
}

#[test]
fn at_binding_inside_a_payload_agrees_across_backends() {
    // The payload position: `Just(n @ 3i64)` has to run the literal
    // check and the payload binding in the right order, and must not
    // swallow the values the next arm handles.
    let src = r#"
        enum Maybe { Just(i64), Nothing }

        fn f(m: Maybe) -> i64 {
            match m {
                Maybe::Just(n @ 3i64) => n * 10i64,
                Maybe::Just(n) => n,
                Maybe::Nothing => -1i64,
            }
        }

        fn main() -> i64 {
            val a: Maybe = Maybe::Just(3i64)
            val b: Maybe = Maybe::Just(8i64)
            val c: Maybe = Maybe::Nothing
            f(a) + f(b) + f(c)
        }
    "#;
    assert_consistent(src, "at_binding_inside_payload");
}


#[test]
fn ranges_that_span_the_type_agree_across_backends() {
    // A partition of `u64` with no wildcard: the type checker accepts
    // it, so every backend has to route the boundary values the same
    // way. The trailing fallthrough block would panic if one did not.
    let src = r#"
        fn size(n: u64) -> u64 {
            match n {
                0u64..10u64 => 0u64,
                10u64..100u64 => 1u64,
                100u64..18446744073709551615u64 => 2u64,
                18446744073709551615u64 => 3u64,
            }
        }

        fn main() -> u64 {
            size(0u64) + size(9u64) + size(10u64) + size(99u64)
                + size(100u64) + size(18446744073709551615u64)
        }
    "#;
    assert_consistent(src, "ranges_spanning_the_type");
}

#[test]
fn ranges_inside_a_payload_agree_across_backends() {
    // A range at a payload position, one of them under an `@`, so the
    // check and the binding have to happen in the right order.
    let src = r#"
        enum Maybe { Just(i64), Nothing }

        fn band(m: Maybe) -> i64 {
            match m {
                Maybe::Just(0i64..10i64) => 1i64,
                Maybe::Just(n @ 10i64..20i64) => n,
                Maybe::Just(_) => 0i64,
                Maybe::Nothing => -1i64,
            }
        }

        fn main() -> i64 {
            val a: Maybe = Maybe::Just(5i64)
            val b: Maybe = Maybe::Just(15i64)
            val c: Maybe = Maybe::Just(50i64)
            val d: Maybe = Maybe::Nothing
            band(a) + band(b) + band(c) + band(d)
        }
    "#;
    assert_consistent(src, "ranges_inside_a_payload");
}


#[test]
fn or_patterns_in_sub_positions_agree_across_backends() {
    // A `|` in a payload / field / element position expands the whole
    // pattern into one arm per combination, so what the backends see
    // is ordinary arms — but the expansion has to produce the same
    // set, and in the same order, for all three.
    let src = r#"
        enum Shape { Circle(i64), Rect(i64, i64), Dot }

        struct Point { x: i64, y: i64 }

        fn small(s: Shape) -> i64 {
            match s {
                Shape::Circle(1i64 | 2i64) => 1i64,
                Shape::Circle(_) => 0i64,
                Shape::Rect(1i64 | 2i64, 3i64 | 4i64) => 2i64,
                Shape::Rect(_, _) => 0i64,
                Shape::Dot => -1i64,
            }
        }

        fn axis(p: Point) -> i64 {
            match p {
                Point { x: 0i64 | 1i64, y } => y,
                Point { x, y } => x + y,
            }
        }

        fn first(s: Shape) -> i64 {
            match s {
                Shape::Circle(n) | Shape::Rect(n, _) => n,
                Shape::Dot => 0i64,
            }
        }

        fn main() -> i64 {
            val c2: Shape = Shape::Circle(2i64)
            val c9: Shape = Shape::Circle(9i64)
            val r24: Shape = Shape::Rect(2i64, 4i64)
            val r94: Shape = Shape::Rect(9i64, 4i64)
            val on: Point = Point { x: 1i64, y: 7i64 }
            val off: Point = Point { x: 5i64, y: 7i64 }
            small(c2) + small(c9) + small(r24) + small(r94)
                + axis(on) + axis(off) + first(c2) + first(r24)
        }
    "#;
    assert_consistent(src, "or_patterns_in_sub_positions");
}

// CALL-ARG-COMPOUND-LITERAL: a struct / tuple literal written straight
// into an argument. A compound never flows through SSA as one value —
// it lives in one local per leaf — so the literal has to be
// materialised into leaf locals at the call site, which is what
// binding it to a `val` first used to do by hand.

#[test]
fn struct_literal_call_arguments_agree_across_backends() {
    let src = r#"
        struct Point { x: i64, y: i64 }
        struct Box2 { lo: Point, hi: Point }

        fn sum(p: Point) -> i64 { p.x + p.y }
        fn span(b: Box2) -> i64 { b.hi.x - b.lo.x + b.hi.y - b.lo.y }
        fn both(p: Point, q: Point) -> i64 { sum(p) + sum(q) }
        fn mixed(n: i64, p: Point, flag: bool) -> i64 {
            if flag { n + sum(p) } else { n }
        }

        fn main() -> i64 {
            sum(Point { x: 1i64, y: 2i64 })
                + span(Box2 { lo: Point { x: 0i64, y: 0i64 }, hi: Point { x: 3i64, y: 4i64 } })
                + both(Point { x: 1i64, y: 1i64 }, Point { x: 2i64, y: 2i64 })
                + mixed(10i64, Point { x: 1i64, y: 2i64 }, true)
        }
    "#;
    assert_consistent(src, "struct_literal_call_arguments");
}

#[test]
fn tuple_literal_call_arguments_agree_across_backends() {
    let src = r#"
        struct Point { x: i64, y: i64 }

        fn pair(t: (i64, i64)) -> i64 { t.0 * t.1 }
        fn sum(p: Point) -> i64 { p.x + p.y }
        fn nested(t: (i64, i64), p: Point) -> i64 { pair(t) + sum(p) }

        fn main() -> i64 {
            pair((3i64, 4i64)) + nested((2i64, 5i64), Point { x: 1i64, y: 1i64 })
        }
    "#;
    assert_consistent(src, "tuple_literal_call_arguments");
}

#[test]
fn generic_struct_literal_argument_follows_the_parameter_slot() {
    // The literal's own name cannot pick between `Cell<i64>` and
    // `Cell<bool>`; the callee's declared parameter type does. Both
    // monomorphisations in one program is what pins that.
    let src = r#"
        struct Cell<T> { value: T }

        fn take_i64(c: Cell<i64>) -> i64 { c.value }
        fn take_bool(c: Cell<bool>) -> bool { c.value }

        fn main() -> i64 {
            val flag: bool = take_bool(Cell { value: true })
            if flag { take_i64(Cell { value: 7i64 }) } else { 0i64 }
        }
    "#;
    assert_consistent(src, "generic_struct_literal_argument");
}

#[test]
fn compound_literal_arguments_to_methods_agree_across_backends() {
    // The method path indexes the callee's parameters past the
    // receiver, so a wrong offset here would build the wrong shape.
    let src = r#"
        struct Point { x: i64, y: i64 }
        struct Grid { origin: Point }

        impl Grid {
            fn make(o: Point) -> Self { Grid { origin: o } }
            fn shifted(self: Self, by: Point) -> i64 {
                self.origin.x + by.x + self.origin.y + by.y
            }
        }

        fn main() -> i64 {
            val g: Grid = Grid::make(Point { x: 1i64, y: 2i64 })
            g.shifted(Point { x: 10i64, y: 20i64 })
        }
    "#;
    assert_consistent(src, "compound_literal_method_arguments");
}

#[test]
fn a_struct_field_may_be_an_enum() {
    // JIT-enum-1: `FieldShape` gained an `Enum` form, so a field can
    // hold a tag plus a payload slot per variant instead of being
    // forced into one local. Before this the whole program was
    // refused at the struct declaration.
    //
    // Covers the four things the field has to do: be built by a
    // literal, be read back, be matched on, and cross a function
    // boundary in both directions.
    let src = r#"
        enum Color { Red, Green, Blue }

        struct Painted {
            color: Color,
            size: i64,
        }

        fn describe(p: Painted) -> i64 {
            match p.color {
                Color::Red => 10i64,
                Color::Green => 20i64,
                Color::Blue => 30i64,
            }
        }

        fn repaint(size: i64) -> Painted {
            Painted { color: Color::Blue, size: size }
        }

        fn main() -> i64 {
            val p: Painted = Painted { color: Color::Green, size: 7i64 }
            val q: Painted = repaint(3i64)
            describe(p) + describe(q) + p.size + q.size
        }
    "#;
    // 20 + 30 + 7 + 3
    assert_consistent(src, "struct_field_enum");
}

#[test]
fn an_enum_struct_field_is_assigned_whole() {
    // The field has no single local to store into, so assignment goes
    // through the enum storage (tag + the variant's payload slots).
    // A copy between two struct bindings has to carry the same tree.
    let src = r#"
        enum Color { Red, Green, Blue }

        struct Painted {
            color: Color,
            size: i64,
        }

        fn code(p: Painted) -> i64 {
            match p.color {
                Color::Red => 1i64,
                Color::Green => 2i64,
                Color::Blue => 3i64,
            }
        }

        fn main() -> i64 {
            var p: Painted = Painted { color: Color::Green, size: 1i64 }
            val before: i64 = code(p)
            val blue: Color = Color::Blue
            p.color = blue
            val copy: Painted = p
            before * 100i64 + code(p) * 10i64 + code(copy)
        }
    "#;
    // 2*100 + 3*10 + 3
    assert_consistent(src, "struct_field_enum_assign");
}

#[test]
fn a_payload_bearing_enum_fits_in_a_struct_field() {
    // A unit-only enum needs a tag and nothing else; a variant with a
    // payload proves the per-variant slots are allocated and written
    // through the field as well.
    let src = r#"
        struct Boxed {
            value: Option<i64>,
            tag: i64,
        }

        fn main() -> i64 {
            val b: Boxed = Boxed { value: Option::Some(5i64), tag: 2i64 }
            val e: Boxed = Boxed { value: Option::None, tag: 3i64 }
            val got: i64 = match b.value {
                Option::Some(n) => n,
                Option::None => 0i64,
            }
            val none: i64 = match e.value {
                Option::Some(n) => n,
                Option::None => 100i64,
            }
            got + none + b.tag + e.tag
        }
    "#;
    // 5 + 100 + 2 + 3
    assert_consistent(src, "struct_field_enum_payload");
}

#[test]
fn a_struct_pattern_can_name_an_enum_variant_in_a_field() {
    // Two things at once: the field pattern dispatches on the field's
    // own tag, and `val x = match <struct> { ... }` infers its type
    // from a name a *compound* pattern bound — which the inference
    // could not do before, so this shape was rejected with "could not
    // infer scalar type for val/var rhs" even without an enum in it.
    let src = r#"
        enum Color { Red, Green, Blue }

        struct Painted {
            color: Color,
            size: i64,
        }

        fn main() -> i64 {
            val red: Painted = Painted { color: Color::Red, size: 4i64 }
            val green: Painted = Painted { color: Color::Green, size: 4i64 }
            val a: i64 = match red {
                Painted { color: Color::Red, size } => size,
                Painted { color: _, size } => size * 10i64,
            }
            val b: i64 = match green {
                Painted { color: Color::Red, size } => size,
                Painted { color: _, size } => size * 10i64,
            }
            a + b
        }
    "#;
    // 4 + 40
    assert_consistent(src, "struct_pattern_enum_field");
}

#[test]
fn a_generic_struct_may_name_another_type_in_a_field() {
    // TYPE-NAME-SPELLING: the field's declared type arrives as
    // `Identifier(Point)` / `Identifier(Color)` while the literal
    // initialising it types as `Struct(Point, [])` / `Enum(Color, [])`.
    // The generic-inference unifier read those as different types, so
    // the whole declaration failed to type-check — on every backend,
    // since this is a frontend pass.
    let src = r#"
        struct Point { x: i64 }
        enum Color { Red, Green, Blue }

        struct Cell<T> {
            value: T,
            p: Point,
            color: Color,
        }

        fn main() -> i64 {
            val g: Cell<i64> = Cell { value: 8i64, p: Point { x: 1i64 }, color: Color::Green }
            val n: i64 = match g.color {
                Color::Red => 100i64,
                Color::Green => 200i64,
                Color::Blue => 300i64,
            }
            g.value + g.p.x + n
        }
    "#;
    // 8 + 1 + 200
    assert_consistent(src, "generic_struct_named_field");
}

// AOT-GENERIC-THROUGH-STRUCT: the AOT / compiler-JIT lowering infers a
// generic function's type arguments through a struct parameter's type
// args (`fn peek<T>(c: Cell<T>) -> T`). The interpreter-side JIT
// learned this with #159; the compiled path was still rejecting the
// call with "cannot infer type arguments for generic function". Each
// call site's concrete instance supplies the zip, so two monomorphs of
// the same function may coexist (`peek<u64>` / `peek<i64>`).

#[test]
fn generic_function_infers_type_args_through_struct_param() {
    let src = r#"
        struct Cell<T> { value: T }

        fn peek<T>(c: Cell<T>) -> T {
            c.value
        }

        fn main() -> u64 {
            val a: Cell<u64> = Cell { value: 9u64 }
            val b: Cell<i64> = Cell { value: 6i64 }
            peek(a) + peek(b) as u64
        }
    "#;
    assert_consistent(src, "generic_through_struct");
}

#[test]
fn generic_function_infers_two_params_through_struct() {
    // Two type params in one struct (`Pair<A, B>`) must each bind to
    // the corresponding concrete arg slot.
    let src = r#"
        struct Pair<A, B> { first: A, second: B }

        fn pick_first<A, B>(p: Pair<A, B>) -> A {
            p.first
        }

        fn pick_second<A, B>(p: Pair<A, B>) -> B {
            p.second
        }

        fn main() -> u64 {
            val p: Pair<u64, i64> = Pair { first: 11u64, second: 4i64 }
            pick_first(p) + pick_second(p) as u64
        }
    "#;
    assert_consistent(src, "generic_through_struct_two_params");
}

#[test]
fn generic_function_infers_through_enum_param() {
    // The parser spells `Option<T>` as `Struct(Option, [T])` until the
    // type checker refines it to `Enum`; the lowering accepts either
    // spelling as long as the concrete instance is an enum.
    let src = r#"
        fn unwrap_default<T>(o: Option<T>) -> T {
            match o {
                Option::Some(v) => v,
                Option::None => panic("none"),
            }
        }

        fn main() -> u64 {
            val o: Option<u64> = Option::Some(21u64)
            unwrap_default(o)
        }
    "#;
    assert_consistent(src, "generic_through_enum");
}

#[test]
fn generic_function_infers_through_tuple_param() {
    // A `(T, u64)` parameter: the tuple binding carries per-element
    // shapes (no interned tuple id), so the zip walks the shapes
    // directly.
    let src = r#"
        fn first_of_pair<T>(p: (T, u64)) -> T {
            p.0
        }

        fn main() -> u64 {
            val p: (u64, u64) = (1u64, 2u64)
            first_of_pair(p) + 1u64
        }
    "#;
    assert_consistent(src, "generic_through_tuple");
}

#[test]
fn generic_function_infers_through_nested_struct_param() {
    // `Wrapper<Cell<T>>` — the zip recurses through two levels of
    // type args.
    let src = r#"
        struct Cell<T> { value: T }
        struct Wrapper<T> { inner: T }

        fn peek<T>(c: Cell<T>) -> T {
            c.value
        }

        fn unwrap_wrap<T>(w: Wrapper<Cell<T>>) -> T {
            w.inner.value
        }

        fn main() -> u64 {
            val a: Cell<u64> = Cell { value: 9u64 }
            val inner: Cell<u64> = Cell { value: 3u64 }
            val w: Wrapper<Cell<u64>> = Wrapper { inner: inner }
            peek(a) + unwrap_wrap(w)
        }
    "#;
    assert_consistent(src, "generic_through_nested_struct");
}

// ---------------------------------------------------------------
// NEWTYPE: tuple structs (`struct Meters(i64)`).
//
// The declaration is desugared by the parser to a struct whose fields
// are named by position; the type checker rewrites the two sugared
// uses (`Meters(v)` construction, `m.0` access) to `StructLiteral` /
// `FieldAccess` before lowering. These pin that all three backends
// therefore behave exactly as they do for a struct with named fields.
// ---------------------------------------------------------------

#[test]
fn tuple_struct_construction_and_access_match_across_backends() {
    let src = r#"
        struct Meters(i64)
        struct Sample(i64, i64)

        fn total(a: Meters, b: Meters) -> Meters {
            Meters(a.0 + b.0)
        }

        fn main() -> i64 {
            val m = Meters(42i64)
            val t = total(m, Meters(8i64))
            val s = Sample(3i64, 4i64)
            t.0 + s.0 + s.1
        }
    "#;
    assert_consistent(src, "tuple_struct_basic");
}

#[test]
fn tuple_struct_methods_match_across_backends() {
    // `&self` receiver and a `Self`-typed return, both reached through
    // the positional field.
    let src = r#"
        struct Meters(i64)

        impl Meters {
            fn scale(&self, k: i64) -> Meters { Meters(self.0 * k) }
            fn raw(&self) -> i64 { self.0 }
        }

        fn main() -> i64 {
            val m = Meters(6i64)
            val doubled = m.scale(2i64)
            doubled.raw() + m.0
        }
    "#;
    assert_consistent(src, "tuple_struct_methods");
}

#[test]
fn generic_tuple_struct_matches_across_backends() {
    let src = r#"
        struct Wrap<T>(T)

        fn main() -> i64 {
            val a: Wrap<i64> = Wrap(9i64)
            val b: Wrap<i64> = Wrap(4i64)
            a.0 - b.0
        }
    "#;
    assert_consistent(src, "tuple_struct_generic");
}

#[test]
fn tuple_struct_prints_in_the_form_it_was_written() {
    // `Meters { 0: 3 }` would be syntax the reader cannot type back in,
    // so a positional struct renders as `Meters(3)` -- in all three
    // backends, which is the part worth pinning.
    let src = r#"
        struct Meters(i64)
        struct Sample(i64, str)
        struct Point { x: i64, y: i64 }

        fn main() -> i64 {
            println(Meters(3i64))
            println(Sample(7i64, "seven"))
            println(Point { x: 1i64, y: 2i64 })
            0i64
        }
    "#;
    assert_stdout_consistent(src, "tuple_struct_print");
}

#[test]
fn tuple_struct_patterns_match_across_backends() {
    // `Meters(v)` lowers to the same `Pattern::Struct` that
    // `Point { x }` produces, so exhaustiveness, `..`, literal
    // sub-patterns and every backend's matching code are shared.
    let src = r#"
        struct Meters(i64)
        struct Sample(i64, i64)

        fn main() -> i64 {
            val m = Meters(7i64)
            val s = Sample(5i64, 3i64)
            val a = match m {
                Meters(0i64) => 100i64,
                Meters(_) => 1i64,
            }
            val b = match s { Sample(n, _) => n }
            val c = match s { Sample(n, ..) => n }
            a + b + c
        }
    "#;
    assert_consistent(src, "tuple_struct_patterns");
}

#[test]
fn unsuffixed_literals_take_the_expected_type_across_backends() {
    // NUMBER-HINT: an unsuffixed integer literal resolves from the
    // position it lands in — a parameter type, a declared return
    // type, an explicit `return`. The signedness that comes out is
    // observable (`u64` subtraction traps where `i64` wraps), so all
    // three backends must agree on which type was chosen.
    let src = r#"
        fn twice(x: i64) -> i64 { x * 2i64 }
        fn pick(n: i64) -> i64 {
            if n > 0i64 {
                return 1
            }
            0
        }

        fn main() -> i64 {
            val a = twice(21)
            val b = pick(5i64)
            val c: i64 = 10
            val d = c - 40
            a + b + d
        }
    "#;
    assert_consistent(src, "unsuffixed_literal_positions");
}

#[test]
fn unsuffixed_literals_reach_narrow_parameters_across_backends() {
    // NUM-W: the same coercion for the narrow widths, so `f(3)`
    // needs no `3i8`.
    let src = r#"
        fn widen8(x: i8) -> i64 { x as i64 }
        fn widen32(x: u32) -> i64 { x as i64 }

        fn main() -> i64 {
            widen8(3) + widen32(70000)
        }
    "#;
    assert_consistent(src, "unsuffixed_literal_narrow");
}

#[test]
fn an_annotated_sibling_does_not_retype_neighbouring_literals() {
    // NUMBER-HINT: `total` is unannotated, so its literals default to
    // `u64` — a neighbouring `val step: i64` must not make them
    // signed. The difference is runtime-observable (`u64` subtraction
    // traps where `i64` wraps), so all three backends must agree on
    // which type each binding got.
    let src = r#"
        fn main() -> i64 {
            val step: i64 = 3i64
            val total = 10
            val doubled = total * 2
            (doubled as i64) + step
        }
    "#;
    assert_consistent(src, "unsuffixed_literal_sibling_annotation");
}

#[test]
fn literal_positions_resolve_identically_across_backends() {
    // NUMBER-HINT: assignment, closure parameter and body, enum
    // payload, associated-function argument, array siblings, and the
    // branch tails of `if` / `match`. The type each literal lands on
    // is runtime-observable, and the failure mode of getting it wrong
    // is a raw `Expr::Number` reaching lowering, so all three backends
    // have to run this.
    let src = r#"
        struct S { n: i64 }
        enum E { V(i64) }
        impl S {
            fn make(n: i64) -> S { S { n: n } }
        }

        fn branch(n: i64) -> i64 { if n > 5i64 { 1 } elif n > 2i64 { 2 } else { 3 } }
        fn arm(n: i64) -> i64 { match n { 0i64 => 10, _ => 20 } }

        fn main() -> i64 {
            var m: i64 = 0i64
            m = 5
            val sib = [1i64, 2, 3]
            val e = E::V(5)
            val payload = match e { E::V(v) => v }
            val s = S::make(5)
            val c = fn(x: i64) -> i64 { if x > 0i64 { 100 } else { 200 } }
            val t = fn() -> i64 { 5 }
            m + sib[1] + payload + s.n + c(1i64) + t() + branch(3i64) + arm(0i64)
        }
    "#;
    assert_consistent(src, "unsuffixed_literal_positions_all");
}
