# Phase JE-1b end-to-end: a non-generic, unit-variant-only enum
# now compiles via the JIT (`Color::Red` lowers to `iconst U64`
# of the variant tag, `match c {...}` lowers to a brif chain
# across per-variant blocks terminating in a shared cont block).
# Both interpreter and JIT must return exit 1 (Color::Red branch).

enum Color { Red, Green, Blue }

fn pick() -> u64 {
    val c: Color = Color::Red
    match c {
        Color::Red => 1u64,
        Color::Green => 2u64,
        Color::Blue => 3u64,
    }
}

fn main() -> u64 {
    pick()
}
