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

pub trait Iterator<T> {
    fn next(&mut self) -> Option<T>
}
