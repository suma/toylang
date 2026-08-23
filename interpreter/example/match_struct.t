# PATTERN-STRUCT: destructuring a struct in `match` and `if val`.
#
# Enum, tuple, literal, range, `@`, or-patterns and guards were all
# there before this; a struct had to be taken apart with field
# accesses inside the arm body instead.
#
#   cargo run -q -p interpreter -- interpreter/example/match_struct.t
#
# Runs on every backend: the lowering handles struct and tuple
# patterns alike (PATTERN-COMPOUND-LOWER).

struct Point { x: i64, y: i64 }

# A literal in a field position makes the arm refutable; the last arm,
# whose field patterns are plain names, covers everything.
fn quadrant(p: Point) -> str {
    match p {
        Point { x: 0i64, y: 0i64 } => "origin",
        Point { x: 0i64, y } => "on the y axis",
        Point { x, y: 0i64 } => "on the x axis",
        Point { x, y } => "somewhere else",
    }
}

struct Config { host: str, port: i64, debug: bool }

# `..` ignores the fields the pattern does not name. Without it every
# field must be listed, so a field added later cannot slip past a
# pattern that looks complete.
fn port_of(c: Config) -> i64 {
    match c {
        Config { port: 0i64, .. } => 0i64 - 1i64,
        Config { debug: true, port, .. } => port * 2i64,
        Config { port, .. } => port,
    }
}

# Field patterns are patterns, so they nest.
struct Inner { v: i64 }
struct Outer { inner: Inner, tag: i64 }

fn total(o: Outer) -> i64 {
    match o {
        Outer { inner: Inner { v: 0i64 }, tag } => tag,
        Outer { inner: Inner { v }, tag } => v * tag,
    }
}

fn main() -> u64 {
    # The values are bound first because passing a struct literal
    # straight into a call is a separate compiler-MVP gap, unrelated
    # to patterns.
    val origin = Point { x: 0i64, y: 0i64 }
    val on_x = Point { x: 3i64, y: 0i64 }
    val elsewhere = Point { x: 3i64, y: 4i64 }
    println(quadrant(origin))
    println(quadrant(on_x))
    println(quadrant(elsewhere))

    val cfg = Config { host: "a", port: 8080i64, debug: true }
    val nested = Outer { inner: Inner { v: 3i64 }, tag: 5i64 }
    println(port_of(cfg))
    println(total(nested))

    # `if val` takes the same patterns.
    val p = Point { x: 0i64, y: 9i64 }
    if val Point { x: 0i64, y } = p {
        println(y)
    } else {
        println("not on the axis")
    }
    0u64
}
