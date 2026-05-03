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
