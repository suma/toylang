# Stdlib `Drop` trait — RAII-style scope-exit cleanup.
#
# `Drop` declares a single `drop(&mut self)` method that backends
# may invoke when the value's lifetime ends. Today the auto-call
# wiring is restricted to the two stdlib allocator wrappers
# (`Arena` / `FixedBuffer`) used in the `with allocator =
# StructName::new(args) { ... }` temporary form — the lower /
# interpreter detects the constructor by name and emits the
# corresponding `__builtin_*_drop` builtin at scope exit. Other
# user-defined `impl Drop for SomeStruct { ... }` impls are
# accepted by the type checker (the trait conformance check
# matches the signature) but the auto-call is not yet generalised
# to user types — those still need an explicit `value.drop()` at
# the appropriate point.
#
# Future phase: extend the auto-call to any temporary whose type
# implements `Drop`, with reverse-construction order at scope
# exit and panic-safety guarantees.
pub trait Drop {
    fn drop(&mut self)
}
