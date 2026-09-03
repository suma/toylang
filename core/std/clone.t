# `Clone` — an independent copy (STDLIB-TRAIT-BASE §6).
#
# **This is not a convenience.** `val b = a` on a compound is an
# *alias*, not a copy, and putting `a` into a container moves it
# (`[E0014]`) -- after which the name cannot be read. `a.clone()` is
# the way to keep both, which makes `Clone` part of the ownership
# model rather than an extra.
#
# It does not make a type stop moving. Whether a value moves is
# decided by whether it has a `Drop`, and a `Clone` impl does not
# change that; what it gives you is something else to hand over.
#
# For a type that owns memory (`Vec` / `String` / `Box` / `Dict` /
# `Set` / `Deque`) the copy owns **its own** allocation and is freed
# independently -- that is the whole difference from an alias.
#
# There is deliberately no `Copy`: "this type does not move" is
# already answered by whether it has a `Drop`, and a second way to
# say it could only disagree with the first.

pub trait Clone {
    fn clone(&self) -> Self
}

#     fn dup<T: Clone>(v: &T) -> T {
#         val c: T = v.clone()
#         c
#     }

# Primitives: the value is the copy. Impl'd for every width, the way
# `Hash` and `Ord` are, so a `<T: Clone>` bound accepts them.
impl Clone for u64 { fn clone(&self) -> Self { self } }
impl Clone for u32 { fn clone(&self) -> Self { self } }
impl Clone for u16 { fn clone(&self) -> Self { self } }
impl Clone for u8  { fn clone(&self) -> Self { self } }
impl Clone for i64 { fn clone(&self) -> Self { self } }
impl Clone for i32 { fn clone(&self) -> Self { self } }
impl Clone for i16 { fn clone(&self) -> Self { self } }
impl Clone for i8  { fn clone(&self) -> Self { self } }
impl Clone for f64 { fn clone(&self) -> Self { self } }
impl Clone for f32 { fn clone(&self) -> Self { self } }
impl Clone for bool { fn clone(&self) -> Self { self } }
impl Clone for usize { fn clone(&self) -> Self { self } }

# `str` borrows its bytes and copying the handle copies nothing --
# which is exactly right, since there is nothing to own.
impl Clone for str { fn clone(&self) -> Self { self } }
