# Stdlib `trait Iterator<T>`. ITER-PROTOCOL-TRAIT lifted the
# generic-trait-declaration restriction so this can now be a real
# nominal trait instead of pure documentation.
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
