# PATTERN-EXTEND: or-patterns, ranges, and `@` bindings.
#
#   a | b        one arm, several alternatives
#   lo..hi       a half-open range, like the `..` expression form
#   name @ pat   bind the matched value while still testing it
#
# All three lower onto pattern forms that already existed. An
# alternative list expands into one arm per alternative, sharing the
# body; a range or `@` becomes an irrefutable name binding plus a
# comparison guard. A guarded arm never counts as exhaustive, so an
# integer match still needs its `_` arm exactly where it did before.

enum Color {
    Red,
    Green,
    Blue,
}

# Two variants in one arm, the third in another: the enum is covered,
# so no wildcard is required.
fn is_warm(c: Color) -> bool {
    match c {
        Color::Red | Color::Green => true,
        Color::Blue => false,
    }
}

# `0i64..5i64` covers 0 through 4 — half-open, the same convention
# `for i in 0..5` uses.
fn bucket(n: i64) -> str {
    match n {
        0i64..5i64 => "low",
        5i64..10i64 => "mid",
        _ => "high",
    }
}

# `@` keeps the value around after testing it. Combining it with a
# range gives "match this span, and let me use the number".
fn describe(n: i64) -> i64 {
    match n {
        x @ 0i64 => x,
        y @ 1i64..10i64 => y * 10i64,
        z @ 10i64..100i64 => z * 100i64,
        _ => -1i64,
    }
}

# The synthesized guard ANDs with a user-written one.
fn gated(n: i64, allow: bool) -> str {
    match n {
        0i64..10i64 if allow => "allowed",
        0i64..10i64 => "blocked",
        _ => "out of range",
    }
}

fn main() -> i64 {
    val r: Color = Color::Red
    val g: Color = Color::Green
    val b: Color = Color::Blue
    println(is_warm(r))
    println(is_warm(g))
    println(is_warm(b))

    println(bucket(0i64))
    println(bucket(7i64))
    println(bucket(42i64))

    println(describe(0i64))
    println(describe(3i64))
    println(describe(50i64))
    println(describe(1000i64))

    println(gated(5i64, true))
    println(gated(5i64, false))
    println(gated(50i64, true))

    describe(3i64) + describe(50i64)
}
