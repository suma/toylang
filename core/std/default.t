# `Default` — the value a type starts at (STDLIB-TRAIT-BASE §7).
#
# The point is not the value; it is being able to *ask for one without
# having one*. `Vec::resize` has to fill new slots before any element
# exists, and `Dict::get_or_default` has to answer for a key that is
# not there. Neither has a receiver to call a method on, so the type
# alone has to answer:
#
#     fn make<T: Default>() -> T { T::default() }
#
# That shape -- a type argument named by nothing but the position the
# result lands in -- is what B5 taught the checker, the monomorphiser
# and the tree-walker to read.
#
# **Zero, not "empty-ish".** Numbers are 0, `bool` is false, and the
# owning types are empty. There is deliberately no default for a type
# whose zero would be a lie: a `Ptr<T>` has no null it is willing to
# admit to (POINTER P5), and an enum has no variant the language can
# pick for it.

pub trait Default {
    fn default() -> Self
}

impl Default for u64 { fn default() -> Self { 0u64 } }
impl Default for u32 { fn default() -> Self { 0u32 } }
impl Default for u16 { fn default() -> Self { 0u16 } }
impl Default for u8  { fn default() -> Self { 0u8 } }
impl Default for i64 { fn default() -> Self { 0i64 } }
impl Default for i32 { fn default() -> Self { 0i32 } }
impl Default for i16 { fn default() -> Self { 0i16 } }
impl Default for i8  { fn default() -> Self { 0i8 } }
impl Default for f64 { fn default() -> Self { 0f64 } }
impl Default for f32 { fn default() -> Self { 0f32 } }
impl Default for bool { fn default() -> Self { false } }
# `usize` is absent for the same reason it is absent from `Ord` and
# `Hash`: it has no literal of its own, so there is nothing to write
# on the right-hand side.

# No `impl Default for str`. `str` is a reserved keyword, so
# `str::default()` cannot be written -- and the substituted form a
# generic body lowers to is the same unwritable name, which means the
# compiled lanes could not reach the impl even though the tree-walker
# could. An impl only one lane can see is worse than none. The owning
# side is served by `String::new()`.
