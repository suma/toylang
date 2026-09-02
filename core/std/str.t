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

# `str.to_string()` is **not** provided as a trait impl — the
# `ToString` trait was retired when `String` became a nominal
# struct (its non-`Self` return type tripped the frontend's
# trait-conformance canonicalisation). User code constructs an
# owned `String` from a `str` via `String::from_str(s)` instead;
# `String` carries an inherent `to_string()` for the
# String → String identity-clone case so `s.to_string()` still
# works on `String` receivers.

# ---------------------------------------------------------------------
# Searching a `str` (STDLIB-TEXT §3).
#
# `str` is a borrowed handle: it does not own a buffer, so it cannot
# answer a question whose answer is a *new* string. `substring` /
# `trim` / `to_ascii_upper` / `to_ascii_lower` / `split` all are, and
# they live on `String`, which owns one. What is left for `str` is the
# questions that only read -- and those are worth having here, because
# reaching them through `String::from_str(s)` would allocate a copy of
# the whole string to ask about part of it.
#
# All four reduce to one search, so there is one extern. It is an
# extern for the reason `Hash for str` and `Ord for str` are: the
# tree-walker's `as_ptr()` allocates, so a byte loop written in
# toylang would allocate once per call.
#
# Indices are **bytes**, like every other index into a `str`. UTF-8 is
# self-synchronising, so a match can only start at a character
# boundary and a returned offset is always one.
extern fn __extern_str_find(haystack: str, needle: str, from: u64) -> i64 from "toylang_rt" as "toy_str_find"

pub trait StrSearch {
    # Byte offset of the first occurrence of `needle` at or after
    # `from`, or `None`. An empty needle is found at `from`.
    fn find_from(self: Self, needle: str, from: u64) -> Option<u64>
    fn find(self: Self, needle: str) -> Option<u64>
    fn contains(self: Self, needle: str) -> bool
    fn starts_with(self: Self, prefix: str) -> bool
    fn ends_with(self: Self, suffix: str) -> bool
}

impl StrSearch for str {
    fn find_from(self: Self, needle: str, from: u64) -> Option<u64> {
        val at: i64 = __extern_str_find(self, needle, from)
        if at < 0i64 { Option::None } else { Option::Some(at as u64) }
    }

    fn find(self: Self, needle: str) -> Option<u64> {
        val at: i64 = __extern_str_find(self, needle, 0u64)
        if at < 0i64 { Option::None } else { Option::Some(at as u64) }
    }

    fn contains(self: Self, needle: str) -> bool {
        __extern_str_find(self, needle, 0u64) >= 0i64
    }

    # `find` reports the *first* occurrence, so a hit at 0 is exactly
    # "the needle is at the front".
    fn starts_with(self: Self, prefix: str) -> bool {
        __extern_str_find(self, prefix, 0u64) == 0i64
    }

    # Asked as "does it match at the one position that would reach the
    # end", which is what `from` exists for -- a backwards search would
    # be a second extern answering the same question.
    fn ends_with(self: Self, suffix: str) -> bool {
        val n: u64 = suffix.len()
        val h: u64 = self.len()
        if n > h { return false }
        val at: u64 = h - n
        __extern_str_find(self, suffix, at) == (at as i64)
    }
}
