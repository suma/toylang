# Stdlib `Add` trait (STDLIB-ITER-TERMINAL): the `+` an algorithm can
# ask a type parameter for.
#
# Auto-loaded from `<core>/std/ops.t -> ["std", "ops"]`. No `package`
# line -- same pattern as `core/std/cmp.t`. Named for the concept, as
# Rust's `std::ops` is.
#
# A generic body cannot write `a + b` on a `T`: the operator is only
# defined for the primitives and for a struct with an `add` method.
# `Add` names that method, so `<T: Add>` can call it -- that is what
# `SumIterator::sum` (`core/std/iter.t`) is written against.
#
# The method is `add` because that is the name the `+` overload
# dispatches to (`struct_cmp_method_name`), the way `Ord::lt` is `<`'s:
# a struct that already overloads `+` satisfies the trait by writing
# `impl Add for V` around the method it has. Both sides borrow, as in
# `Ord`.
#
# The primitive impls are the operator itself, so they share its
# semantics: integers wrap (RUNTIME-TRAP -- `+` never traps), floats
# are IEEE 754. Reach for `Checked::checked_add` when wrapping is not
# the answer.

pub trait Add {
    fn add(&self, other: &Self) -> Self
}

impl Add for u64 { fn add(&self, other: &Self) -> Self { self + other } }
impl Add for u32 { fn add(&self, other: &Self) -> Self { self + other } }
impl Add for u16 { fn add(&self, other: &Self) -> Self { self + other } }
impl Add for u8  { fn add(&self, other: &Self) -> Self { self + other } }
impl Add for i64 { fn add(&self, other: &Self) -> Self { self + other } }
impl Add for i32 { fn add(&self, other: &Self) -> Self { self + other } }
impl Add for i16 { fn add(&self, other: &Self) -> Self { self + other } }
impl Add for i8  { fn add(&self, other: &Self) -> Self { self + other } }
impl Add for f64 { fn add(&self, other: &Self) -> Self { self + other } }
impl Add for f32 { fn add(&self, other: &Self) -> Self { self + other } }
