# PATTERN-EXTEND: or-patterns, ranges, and `@` bindings.
#
#   a | b        one arm, several alternatives
#   lo..hi       a half-open range, like the `..` expression form
#   name @ pat   bind the matched value while still testing it
#
# An alternative list expands into one arm per alternative, sharing
# the body. `@` binds the whole matched value and leaves the decision
# to the pattern it wraps, so it takes an enum variant or a struct
# pattern as readily as a literal — and because a binding never
# rejects anything, it is invisible to the exhaustiveness check.
#
# A range covers a span of the value space, and the checker tracks
# literals and ranges as one interval set: an arm inside an earlier
# span is unreachable, and adjacent spans merge, so arms that
# partition the type need no `_` at all (see `size` below).

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

# Ranges that partition the type leave nothing over, so this compiles
# without a wildcard. The last arm is the literal `u64` maximum, which
# the half-open range above it cannot reach.
fn size(n: u64) -> str {
    match n {
        0u64..10u64 => "tiny",
        10u64..100u64 => "small",
        100u64..18446744073709551615u64 => "big",
        18446744073709551615u64 => "max",
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

# The inner pattern can be anything. Here it is a variant — the arm
# still runs only for `Green`, and `same` names the value that got it
# there, so it can be passed on without rebuilding it.
fn rank(c: Color) -> i64 {
    match c {
        Color::Red => 0i64,
        same @ Color::Green => rank_of(same),
        Color::Blue => 2i64,
    }
}

fn rank_of(c: Color) -> i64 {
    1i64
}

# Inside a payload, and over a struct pattern: `n @ 3i64` reads the
# payload and tests it at once, and `whole @ Point { .. }` names the
# struct while its fields still decide.
struct Point {
    x: i64,
    y: i64,
}

fn corner(p: Point) -> i64 {
    match p {
        whole @ Point { x: 0i64, y } => whole.y + y,
        Point { x, y } => x + y,
    }
}

enum Maybe {
    Just(i64),
    Nothing,
}

fn triple(m: Maybe) -> i64 {
    match m {
        Maybe::Just(n @ 3i64) => n * 10i64,
        Maybe::Just(n) => n,
        Maybe::Nothing => -1i64,
    }
}

# A range works at a payload position too, `@` included.
fn band(m: Maybe) -> i64 {
    match m {
        Maybe::Just(0i64..10i64) => 1i64,
        Maybe::Just(n @ 10i64..20i64) => n,
        Maybe::Just(_) => 0i64,
        Maybe::Nothing => -1i64,
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

    println(size(5u64))
    println(size(50u64))
    println(size(500u64))
    println(size(18446744073709551615u64))

    println(describe(0i64))
    println(describe(3i64))
    println(describe(50i64))
    println(describe(1000i64))

    println(gated(5i64, true))
    println(gated(5i64, false))
    println(gated(50i64, true))

    println(rank(r))
    println(rank(g))
    println(rank(b))

    val origin: Point = Point { x: 0i64, y: 5i64 }
    val other: Point = Point { x: 2i64, y: 3i64 }
    println(corner(origin))
    println(corner(other))

    val three: Maybe = Maybe::Just(3i64)
    val eight: Maybe = Maybe::Just(8i64)
    val none: Maybe = Maybe::Nothing
    println(triple(three))
    println(triple(eight))
    println(triple(none))

    val fifteen: Maybe = Maybe::Just(15i64)
    println(band(three))
    println(band(fifteen))
    println(band(none))

    describe(3i64) + describe(50i64)
}
