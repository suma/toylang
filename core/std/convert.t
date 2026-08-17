# `From` / `Into` — value conversion traits.
#
#     trait From<T> { fn from(value: T) -> Self }     # `Target::from(source)`
#     trait Into<T> { fn into(self: Self) -> T }      # `source.into()`
#
# Rust-style blanket impl (`U: From<T>` implies `T: Into<U>`) is
# *not* written out here — toylang has no `where` clauses, so the
# type checker supplies it syntactically: a call `expr.into()` is
# rewritten to `Target::from(expr)` when the expected type `Target`
# implements `From<source>`. Users only write the `From` side; the
# `Into` side is derived at the call site.
#
# `?` also consults these impls (cross-error conversion): when the
# enclosed expression yields `Result<T, E1>` inside a function
# returning `Result<T, E2>`, an `E2: From<E1>` impl converts the
# error before re-returning it.
#
# Conversions are provided in the modules that own the target type
# (e.g. `impl From<str> for String` lives in `core/std/string.t`).

trait From<T> {
    fn from(value: T) -> Self
}

trait Into<T> {
    fn into(self: Self) -> T
}