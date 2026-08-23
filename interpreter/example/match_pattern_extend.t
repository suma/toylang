# PATTERN-EXTEND: or-patterns, ranges, and `@` bindings.
#
#   a | b        one arm, several alternatives
#   lo..hi       a half-open range, like the `..` expression form
#   name @ pat   bind the matched value while still testing it
#
# An alternative list expands into one arm per alternative, sharing
# the body. A range becomes an irrefutable name binding plus a
# comparison guard, and a guarded arm never counts as exhaustive, so
# an integer match still needs its `_` arm exactly where it did
# before. `@` is a pattern of its own: it binds the whole matched
# value and leaves the decision to the pattern it wraps, so it wraps
# an enum variant or a struct pattern as readily as a literal — and
# because a binding never rejects anything, it is invisible to the
# exhaustiveness check.

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

    describe(3i64) + describe(50i64)
}
