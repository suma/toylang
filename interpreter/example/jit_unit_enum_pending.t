# Phase JE-1a infrastructure check. The enum is JIT-eligible
# (non-generic, unit-variant-only) so its layout sits in the JIT's
# `enum_layouts` thread-local, but constructor + match codegen is
# deferred to JE-1b. The interpreter handles the program; the JIT
# falls back with a precise "infra ready, codegen pending"
# message. Expected exit: 1 (Color::Red branch).

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
