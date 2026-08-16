# Stdlib `Drop` trait — RAII-style scope-exit cleanup.
#
# `Drop` declares a single `drop(&mut self)` method that backends
# invoke when the value's lifetime ends. The auto-call is general:
# a binding whose type (transitively) contains a `Drop`-impl type
# is glued at scope exit — the backend frees everything the value
# owns (for `Box<T>` / `Vec<T>` the heap slots themselves, for
# structs the fields, for enums the active payload) and then runs
# this method, which for the containers frees the storage the
# contents lived in. See "Ownership" in docs/language.md and the
# DROP-GLUE entry in design-docs/todo.md.
#
# Explicit `value.drop()` calls are still fine: the stdlib
# implementations are written to be idempotent, and `free` itself
# is idempotent on every backend.
pub trait Drop {
    fn drop(&mut self)
}
