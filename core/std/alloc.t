# Stdlib allocation failure (ERROR_MODEL D5). Auto-loaded from
# `<core>/std/alloc.t`.
#
# **Running out of memory is a panic by default.** The containers do
# not return a `Result` from `push` / `push_str` / `push_char`: a
# per-element answer would put a branch and a `?` in every loop of
# every correct program, and the price is paid by the programs that
# never run out. What they do instead is *notice* -- an allocation
# that fails now stops the program with the number of bytes it asked
# for, rather than writing through the null pointer it got back.
#
# A caller that wants to recover **asks before it allocates**:
#
#     with allocator = fb {
#         var v: Vec<u64> = Vec::new()
#         v.try_reserve(items.size())?   # the only place that can fail
#         for x in items { v.push(x) }   # within capacity: no regrow
#     }
#
# The promise is capacity-shaped: "the next `n` elements will not
# reallocate". A push past that reallocates again and can panic again.
# `try_with_capacity` is the same question asked before the value
# exists, when there is nothing to call `try_reserve` on.
#
# There is deliberately no `try_push`. It is `try_reserve`'s weaker
# form -- it asks once per element instead of once per loop -- and it
# puts back exactly the branch this shape exists to remove. It can be
# added later without changing a signature, so it waits for a program
# that `try_reserve` cannot serve.

# Why an allocation could not be made. Two variants, split by what the
# caller does about it (ERROR_MODEL D3).
pub enum AllocError {
    # The allocator has no room left -- raise the budget, or hold less.
    OutOfMemory,
    # The requested byte count does not fit in `u64`, so no allocator
    # could serve it. A different budget will not help; the request
    # itself is impossible.
    SizeOverflow,
}

impl Display for AllocError {
    fn to_str(&self) -> str {
        match self {
            AllocError::OutOfMemory => "out of memory",
            AllocError::SizeOverflow => "allocation size overflows u64",
        }
    }
}
