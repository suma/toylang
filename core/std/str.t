# NOTE: no `package` line. The file's package path would be
# `std.str`, but `str` is a reserved primitive-type keyword the
# parser refuses to accept as a package segment. The auto-load
# integration derives the module path from the file system
# location (`core/std/str.t -> ["std", "str"]`) independently of
# any in-file `package` declaration, so dropping it costs nothing.
#
# Stdlib extension trait for `str`. Auto-loaded so user code can
# write `s.as_ptr()` without an `import` line — same shape as
# `core/std/i64.t::Abs` and `core/std/hash.t::Hash for str`.
#
# `as_ptr()` returns a pointer to the string's UTF-8 bytes
# (NUL-terminated). The pointer is valid for the lifetime of the
# input string. Backend semantics differ on what the pointee
# representation is:
#   - AOT / JIT: identity — `str` values are already pointer-sized
#     handles into a `.rodata` blob (or a heap-allocated copy),
#     so `s.as_ptr()` returns the same address.
#   - Interpreter: heap-allocates `len + 1` bytes via the active
#     allocator and writes each byte as a typed-slot u8 entry, so
#     `__builtin_ptr_read(p, i)` with a `val: u8 = ...` annotation
#     returns the byte at offset i. The NUL terminator lives at
#     index `len`.
#
# Use case: low-level FFI / interop where the caller needs to walk
# the bytes of a string with `__builtin_ptr_read` (and `mem_copy`
# / `mem_set` for buffers built from `__builtin_heap_alloc`).
# The `__builtin_str_to_ptr` primitive remains the underlying
# operation; this trait is the user-facing entry point.

trait AsPtr {
    fn as_ptr(self: Self) -> ptr
}

impl AsPtr for str {
    fn as_ptr(self: Self) -> ptr {
        __builtin_str_to_ptr(self)
    }
}

# `str` byte length. Returns the number of UTF-8 bytes (NOT the
# character count for multi-byte sequences).
#
# Backend semantics:
#   - AOT: calls libc `strlen` on the str's byte pointer. The
#     per-literal `.rodata` layout (`[bytes][NUL][u64 len]`)
#     keeps the trailing NUL precisely so this walk terminates
#     at the right position.
#   - Interpreter: returns `s.bytes().len()` directly.
#
# `Length` rather than `Len` to avoid conflicting with any future
# user `trait Len { fn len() }` they may want for collections.
trait Length {
    fn len(self: Self) -> u64
}

impl Length for str {
    fn len(self: Self) -> u64 {
        __builtin_str_len(self)
    }
}

# `str.to_string()` — copy the str's UTF-8 bytes onto the heap
# as a `Vec<u8>` (= `String`) through the active allocator.
# Mirrors `Vec::from_str(s)` so user code can pick whichever shape
# reads better at the call site (`s.to_string()` vs
# `Vec::from_str(s)`). The `ToString` trait is declared in
# `core/std/str_ops.t` (alongside `ToString for Vec<u8>`).
impl ToString for str {
    fn to_string(self: Self) -> Vec<u8> {
        val r: Vec<u8> = Vec::from_str(self)
        r
    }
}
