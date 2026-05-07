# `String` — heap-allocated growable byte buffer. Defined as a
# `type` alias for `Vec<u8>`, not a wrapper struct: every method
# users want lives directly on `Vec<T>` (the generic ones — `push`,
# `pop`, `get`, `set`, `size`, `capacity`, `is_empty`, `as_ptr`,
# `clear`) or on the byte-specific `impl Vec<u8>` (`from_str`,
# `push_str`, `push_char`, `eq`, `extend_bytes`).
#
# Sibling to the language's static `str` type:
#
#   - `str` — pointer + length to a `.rodata` UTF-8 byte sequence
#     (or, in the interpreter, an immutable run of typed-slot u8s).
#     Cheap to pass around; not mutable; lifetime is the program /
#     literal scope.
#   - `String` — a `Vec<u8>` on the heap. Owned, growable, freed
#     via the active allocator's normal path. Use this when a
#     `str` needs to be copied onto the heap (e.g. read input,
#     concatenate, etc.).
#
# Construction: `Vec::from_str("text")` with a `String` (= `Vec<u8>`)
# annotation provides T=u8 inference, e.g.
#
#     val s: String = Vec::from_str("hello")
#
# `str` superset extension (the `impl` blocks below) — `Vec<u8>`
# also satisfies the `Length` / `AsPtr` traits from
# `core/std/str.t`, so `.len()` / `.as_ptr()` work on both
# `str` and `String` receivers with the same call shape.
# Internally each impl delegates to the existing inherent
# `Vec<u8>` API (`.size()` / `self.data`).
#
# `Concat<Other>` / `Contains<Needle>` are deferred — the AOT
# lower currently rejects trait methods that take a struct
# (or `&struct`) argument with "method argument produced no value".
# The traits live in `core/std/str_ops.t` for the impls to attach
# to once the AOT fix lands.
#
# Cross-module alias resolution (`frontend::resolve_type_aliases`)
# substitutes `String` with `Vec<u8>` everywhere in the integrated
# AST after parsing. User code can therefore write `val s: String`
# / `&String` / `-> String` and have it lower as `Vec<u8>` in
# every backend.
#
# Auto-loaded from `<core>/std/string.t -> ["std", "string"]`.
# Loads after `core/std/collections/vec.t` (segments-sort puts
# `["string"]` past `["collections", "vec"]`); the alias target's
# `Vec` reference is resolved at type-check time, so load order
# does not affect correctness.

type String = Vec<u8>

# `len()` — UTF-8 byte length. Identity wrapper around the
# existing `Vec<u8>::size()` so callers can use the same
# `.len()` name as `str`.
impl Length for Vec<u8> {
    fn len(self: Self) -> u64 {
        self.size()
    }
}

# `as_ptr()` — pointer to the first byte. The trait method on
# `str` returns a NUL-terminated pointer; `String` doesn't promise
# NUL termination (the buffer is sized exactly to `self.len`), so
# callers walking the bytes should pair `s.as_ptr()` with
# `s.len()` rather than scan for `'\0'`.
impl AsPtr for Vec<u8> {
    fn as_ptr(self: Self) -> ptr {
        self.data
    }
}

