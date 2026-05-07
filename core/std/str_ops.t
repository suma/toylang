# Stdlib traits for byte-buffer / string-like operations.
#
# Currently empty: the planned `Concat<Other>` / `Contains<Needle>`
# traits depend on AOT lower being able to round-trip a trait
# method that takes a struct (or `&struct`) argument. Today the
# AOT compiler emits "method argument produced no value" for that
# shape (fine on interpreter / JIT). The traits will be added
# here once the AOT fix lands.
#
# Auto-loaded from `<core>/std/str_ops.t -> ["std", "str_ops"]`.
