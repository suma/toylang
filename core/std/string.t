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
