# String interpolation demo crafted for the JIT support test
# (`string_interpolation_jit_logs_compiled_main`). Avoids
# `__builtin_str_len` / `s.len()` style calls that the
# interpreter JIT still routes through the legacy
# extension-trait dispatch path — those would skip the function
# and defeat the "JIT compiled: main" assertion. The full demo
# in `string_interpolation.t` keeps `.len()` for richness; this
# variant only uses the runtime-helper-backed primitives:
#
#   - String literals materialise via `jit_string_literal`.
#   - `__builtin_to_string(<scalar>)` dispatches to
#     `jit_to_string_<ty>`.
#   - `s.concat(t)` lowers to `jit_str_concat`.
#   - `println(str_value)` routes through `jit_println_str`.

fn double(x: i64) -> i64 { x * 2i64 }

fn main() -> i64 {
    val name = "world"
    val n: i64 = 42i64
    val flag: bool = n > 0i64

    println("hello {name}")
    println("n={n}, n*2={n * 2i64}, double(n)={double(n)}")
    println("flag is {flag}")
    println("escaped braces: {{{n}}}")

    n
}
