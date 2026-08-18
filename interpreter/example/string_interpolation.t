# String interpolation demo. `"...{expr}..."` lowers at parse
# time to a chain of `.concat()` calls with each `{expr}` lifted
# through `__builtin_to_string(...)`. The protocol is structural:
# any value whose `to_display_string` representation makes sense
# (every primitive and any struct / enum that has one) can appear
# inside `{...}`.
#
# `{{` / `}}` lex to literal `{` / `}` (Rust convention).
#
# Runs on every backend: the interpreter, the AOT compiler, and the
# cranelift JIT share the desugaring (the last two through the
# `toy_str_concat` / `toy_to_string_<ty>` runtime helpers). Format
# specs (`"{x:.2}"`) are in `string_format_spec.t`.

fn double(x: i64) -> i64 { x * 2i64 }

fn main() -> i64 {
    val name = "world"
    val n: i64 = 42i64
    val flag: bool = n > 0i64

    println("hello {name}")
    println("n={n}, n*2={n * 2i64}, double(n)={double(n)}")
    println("flag is {flag}, name length={name.len()}")
    println("escaped braces: {{{n}}}")

    n
}
