# Stdlib `String` — heap-allocated growable byte buffer wrapping
# `Vec<u8>`. Sibling to the language's static `str` type:
#
#   - `str` — pointer + length to a `.rodata` UTF-8 byte sequence
#     (or, in the interpreter, an immutable run of typed-slot u8s).
#     Cheap to pass around; not mutable; lifetime is the program /
#     literal scope.
#   - `String` — a `Vec<u8>` on the heap. Owned, growable, and
#     freed via the active allocator's normal path. Use this when
#     a `str` needs to be copied onto the heap (e.g. read input,
#     concatenate, etc.).
#
# Auto-loaded from `<core>/std/string.t -> ["std", "string"]`.
#
# API:
#   - `String::from_str(s: str) -> Self` — copy `s`'s bytes onto
#     the heap (NUL terminator NOT copied; size matches `s.len()`)
#   - `String::len(&self) -> u64` — byte count
#   - `String::is_empty(&self) -> bool`
#   - `String::as_ptr(&self) -> ptr` — pointer to the underlying
#     byte buffer (matches `Vec<u8>.data`)
#   - `String::eq(&self, other: &String) -> bool` — byte-wise
#     equality (length-then-loop)
#   - `String::clear(&mut self)` — reset to empty (cap preserved)
#   - `String::push_char(&mut self, c: u8)` — append one byte
#   - `String::push_str(&mut self, other: &String)` — append
#     all bytes of `other`

pub struct String {
    vec: Vec<u8>,
}

impl String {
    # `String` has no public empty constructor — `from_str("")` is
    # the required entry point. Keeping a single constructor
    # (`from_str`) means callers never accidentally observe an
    # uninitialised / zero-capacity `String`, and the struct's
    # private `vec` field stays inaccessible from the outside.

    # Copy the UTF-8 bytes of a `str` into a fresh, heap-allocated
    # `String`. The trailing NUL terminator is intentionally NOT
    # copied — `len()` matches `s.len()` exactly. Bulk allocate +
    # memcpy:
    #   - AOT: `s.as_ptr()` is the byte_start of the `.rodata`
    #     `[bytes][NUL][u64 len]` layout; `__builtin_mem_copy`
    #     lowers to libc memcpy(3).
    #   - Interpreter: `s.as_ptr()` populates typed-slot `u8`
    #     entries; `HeapManager::copy_memory` is typed-slots-aware
    #     and propagates them to the destination buffer.
    fn from_str(s: str) -> Self {
        val n: u64 = s.len()
        # Bulk allocate + memcpy lowers to a single `malloc` +
        # `memcpy` at AOT. The `heap_alloc(0) + heap_realloc(p, n)`
        # pair handles `n == 0` gracefully (realloc(p, 0) returns
        # a freed/null-equivalent pointer; mem_copy with size 0
        # is a no-op).
        val raw: ptr = __builtin_heap_alloc(0u64)
        val data: ptr = __builtin_heap_realloc(raw, n)
        __builtin_mem_copy(s.as_ptr(), data, n)
        # Compiler MVP: the `vec` field must be initialised by a
        # struct literal, and the `String` return value must come
        # from a bare identifier — hence the inline `Vec { ... }`
        # plus the trailing `val result` binding.
        val result: String = String {
            vec: Vec {
                data: data,
                len: n,
                cap: n,
                elem_size: 1u64,
            }
        }
        result
    }

    # Delegate to `Vec`'s public method API instead of reading
    # `self.vec`'s fields directly. The `vec` field is private to
    # `String`; from outside the impl it must be opaque, and from
    # inside it should still be treated as such so the `Vec` /
    # `String` boundary stays clean. All three are read-only and
    # take `&self` so callers don't pay the cost of cloning the
    # underlying `Vec<u8>` handle just to query it.
    fn len(&self) -> u64 {
        self.vec.size()
    }

    fn is_empty(&self) -> bool {
        self.vec.is_empty()
    }

    fn as_ptr(&self) -> ptr {
        self.vec.as_ptr()
    }

    # Append the bytes of `other` to `self` in-place. `other` is
    # taken by reference (`&String`) — REF-Stage-2 minimum subset:
    # caller-side auto-borrow lets `s.push_str(b)` work with `b:
    # String`, and at runtime / IR the reference is currently
    # erased to a value (no semantic difference until the IR
    # learns true pointer passing). Internally we delegate to
    # `Vec<u8>::extend_bytes` so the geometric grow logic is
    # shared.
    fn push_str(&mut self, other: &String) {
        self.vec.extend_bytes(other.as_ptr(), other.len())
    }

    # Byte-wise equality. Two `String`s are equal iff they have
    # the same length and every byte matches. Length check first
    # so different-sized strings short-circuit without walking
    # the buffer. Both receivers are immutable references —
    # callers may pass either `String` or `&String` thanks to
    # auto-borrow.
    fn eq(&self, other: &String) -> bool {
        val n: u64 = self.len()
        if n != other.len() {
            return false
        }
        val pa: ptr = self.as_ptr()
        val pb: ptr = other.as_ptr()
        var i: u64 = 0u64
        while i < n {
            val a: u8 = __builtin_ptr_read(pa, i)
            val b: u8 = __builtin_ptr_read(pb, i)
            if a != b {
                return false
            }
            i = i + 1u64
        }
        true
    }

    # Reset to an empty `String` while keeping the underlying
    # buffer. After `clear()`, `len() == 0` and `is_empty() ==
    # true`; further `push_char` / `push_str` calls reuse the
    # existing capacity until the geometric-grow threshold is
    # hit again.
    fn clear(&mut self) {
        self.vec.clear()
    }

    # Append a single byte. Equivalent to `push_str` of a
    # one-byte `String`, but skips the temporary `String`
    # construction. The `c: u8` argument is by value — narrow
    # integers are scalars and don't benefit from `&u8`.
    fn push_char(&mut self, c: u8) {
        self.vec.push(c)
    }
}
