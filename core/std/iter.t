# Stdlib `trait Iterator<T>`. ITER-PROTOCOL-TRAIT lifted the
# generic-trait-declaration restriction so this is a real nominal
# trait, and since STDLIB-TRAIT-BASE B2 **every stdlib iterator names
# it** -- `VecIter` / `MapIter` / `FilterIter` / `EnumerateIter` /
# `ZipIter` / `DequeIter` / `SetIter` / `SoaVecIter` / `DictIter` /
# `DictMapIter` / `DictFilterIter` / `StringIter` / `CharsIter` /
# `StringMapIter` / `StringFilterIter` / `StringEnumerateIter`.
#
# That is what lets a function *take* an iterator:
#
#     fn total<I: Iterator<u64>>(it: I) -> u64 {
#         var sum: u64 = 0u64
#         for x in it { sum = sum + x }
#         sum
#     }
#
# Before, a function could take a `Vec<u64>` but had nowhere to
# receive the result of `v.iter().filter(...)`.
#
# The type argument stays explicit rather than becoming an associated
# `Item`: moving it would cost 16 impls and every use, and buy only
# the shorter `<I: Iterator>` -- while an explicit argument leaves
# room for one struct to name several (`Iterator<(K, V)>`).
#
# Any struct that exposes `fn next(&mut self) -> Option<T>` may
# implement it via `impl Iterator<T> for MyType { ... }`. The
# parser-level desugaring of `for x in EXPR { body }` (see
# `frontend/src/parser/stmt.rs::desugar_for_in_iterator`) only
# relies on the structural shape — the protocol works whether or
# not the impl is declared. Adding `impl Iterator<T> for ...`
# unlocks generic-bound consumers like
# `fn first<I: Iterator<i64>>(iter: I) -> Option<i64>`.
#
# Range (`0..10`) keeps its dedicated integer fast path through
# `Stmt::For` and does not flow through this trait.

# **Terminal operations** (STDLIB-ITER-TERMINAL) are default bodies, so
# every iterator above -- and any user iterator that writes `impl
# Iterator<T> for ...` -- has them without writing them:
#
#     val n = it.count()
#     val total = it.fold(0u64, fn(acc: u64, x: u64) -> u64 { acc + x })
#     val big = it.any(fn(x: u64) -> bool { x > 100u64 })
#
# Each one **consumes the iterator** (`self: Self`, like `collect`): the
# caller's binding still holds the position it had, so a second call
# starts over from there rather than from where the first one stopped.
#
# What an element-returning operation (`find` / `last` / `nth`, and
# `min` / `max` below) hands back is the element itself, the same
# shallow copy `next` gives -- an alias of what the container keeps,
# not a new owner. The move checker treats those method names like
# `next` (NEXT-ITEM-ALIAS), so binding the result does not free the
# element out from under the container.
#
# **How the bodies are written matters.** A default body is shared by
# every impl, and the impls name their element type differently
# (`impl<T, U> Iterator<U> for MapIter<T, U>`). The signatures are
# restated in each impl's terms when the default is installed, but the
# body is not rewritten, so **a default body here must not spell `T`**
# -- it takes types from `self.next()` and the signature instead.
# A method's own type parameter is spelled `Acc`, not a single letter,
# so it cannot meet an impl's own `A` / `B` (`ZipIter<A, B>`).
#
# In the compiled lanes a closure argument can only take and return
# scalars, so `fold` / `any` / `all` / `position` / `find` over an
# iterator of tuples or structs (`enumerate`, `zip`) are
# interpreter-only.

pub trait Iterator<T> {
    fn next(&mut self) -> Option<T>

    # How many elements are left.
    fn count(self: Self) -> u64 {
        var it = self
        var n: u64 = 0u64
        for x in it {
            n = n + 1u64
        }
        n
    }

    # The last element, or `None` when there are none.
    fn last(self: Self) -> Option<T> {
        var it = self
        var last = it.next()
        for x in it {
            last = Option::Some(x)
        }
        last
    }

    # The element `n` places on (0 is the next one), or `None` when the
    # stream ends first.
    fn nth(self: Self, n: u64) -> Option<T> {
        var it = self
        var i: u64 = 0u64
        for x in it {
            if i == n {
                return Option::Some(x)
            }
            i = i + 1u64
        }
        Option::None
    }

    # `f(...f(f(init, e0), e1)..., en)` -- the general reduction every
    # other operation here is a special case of.
    fn fold<Acc>(self: Self, init: Acc, f: fn (Acc, T) -> Acc) -> Acc {
        var it = self
        var acc = init
        for x in it {
            acc = f(acc, x)
        }
        acc
    }

    # Whether `p` holds for some element. Stops at the first that does;
    # `false` for an empty stream.
    fn any(self: Self, p: fn (T) -> bool) -> bool {
        var it = self
        for x in it {
            if p(x) {
                return true
            }
        }
        false
    }

    # Whether `p` holds for every element. Stops at the first that does
    # not; `true` for an empty stream.
    fn all(self: Self, p: fn (T) -> bool) -> bool {
        var it = self
        for x in it {
            if !p(x) {
                return false
            }
        }
        true
    }

    # The index (counted from where the iterator stands) of the first
    # element `p` holds for. `Vec::index_of` is the by-value form.
    fn position(self: Self, p: fn (T) -> bool) -> Option<u64> {
        var it = self
        var i: u64 = 0u64
        for x in it {
            if p(x) {
                return Option::Some(i)
            }
            i = i + 1u64
        }
        Option::None
    }

    # The first element `p` holds for.
    fn find(self: Self, p: fn (T) -> bool) -> Option<T> {
        var it = self
        for x in it {
            if p(x) {
                return Option::Some(x)
            }
        }
        Option::None
    }
}

# `min` / `max` need `T: Ord` and `sum` needs `T: Add + Default`, which
# a default body on `Iterator<T>` cannot ask for: it is checked against
# every impl, including `VecIter<T>` for a `T` with no order. So each
# lives on a trait of its own whose default body is checked under the
# **impl's** bounds, and an iterator opts in with an empty impl:
#
#     impl<T: Ord> OrdIterator<T> for VecIter<T> {}
#     impl<T: Add + Default> SumIterator<T> for VecIter<T> {}
#
# Calling `min()` on a `VecIter<P>` whose `P` has no `Ord` is then the
# ordinary bound violation at the call site. The stdlib iterators that
# yield a single value opt in next to their `impl Iterator`; the ones
# that yield tuples (`enumerate`, `zip`, `Dict::iter`) do not, since a
# tuple has no `Ord`.
#
# The traits do not list `next` -- toylang has no supertraits yet (A3)
# -- so an impl on a type without one fails inside the inherited body.
pub trait OrdIterator<T> {
    # The smallest element; the first of several equal ones. `None` for
    # an empty stream.
    fn min(self: Self) -> Option<T> {
        var it = self
        var best = it.next()
        for x in it {
            match best {
                Option::Some(b) => {
                    if x.lt(b) {
                        best = Option::Some(x)
                    }
                }
                Option::None => {}
            }
        }
        best
    }

    # The largest element; the last of several equal ones (Rust's
    # rule, so `min` and `max` of a two-element stream are its two
    # elements). `None` for an empty stream.
    fn max(self: Self) -> Option<T> {
        var it = self
        var best = it.next()
        for x in it {
            match best {
                Option::Some(b) => {
                    if !x.lt(b) {
                        best = Option::Some(x)
                    }
                }
                Option::None => {}
            }
        }
        best
    }
}

pub trait SumIterator<T> {
    # The elements added up with `Add::add` (integers wrap, as `+`
    # does); `T`'s zero for an empty stream. Starts from the first
    # element rather than from zero so the body never has to name `T`
    # (see the note on `Iterator`): `unwrap_or_default` supplies the
    # zero only when there is no first element.
    fn sum(self: Self) -> T {
        var it = self
        val first = it.next()
        var acc = first.unwrap_or_default()
        for x in it {
            acc = acc.add(x)
        }
        acc
    }
}
