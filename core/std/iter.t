# Iterator protocol — desugaring contract for `for x in EXPR { body }`.
#
# This file is intentionally **comment-only**: a `trait Iterator<T>`
# declaration would require generic-trait support (`trait Foo<T>`),
# which is on the deferred list (see CLAUDE.md → OOP keywords).
# Until that lands, the iterator protocol is **structural** rather
# than nominal — any struct that exposes a method matching the
# expected shape participates in `for x in EXPR { body }`.
#
# Required shape:
#
#     fn next(&mut self) -> Option<T>
#
# Returning `Option::Some(value)` yields `value` to the next loop
# iteration; returning `Option::None` ends the loop.
#
# The parser desugars `for x in EXPR { body }` (where EXPR is **not**
# a literal range — `0..N` and `0 to N` keep their dedicated
# `Stmt::For` integer fast path) into:
#
#     {
#         var __iter_for_<n> = EXPR
#         while true {
#             match __iter_for_<n>.next() {
#                 Option::Some(x) => { body; continue },
#                 Option::None    => { break },
#             }
#         }
#     }
#
# The `continue` after `body` exists purely to unify the match arm
# types at `Unit` so the user's body can end in any expression
# (e.g. an assignment `sum = sum + x`, which would otherwise return
# its rhs type and clash with the `None` arm's `break`).
