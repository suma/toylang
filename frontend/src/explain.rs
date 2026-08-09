//! Prose behind the diagnostic codes (LLM-LOOP P7).
//!
//! A code on its own is a label. What makes it worth having is that it
//! can be *asked about* — `interpreter --explain E0001` turns a
//! one-line diagnostic into the category it belongs to, a program that
//! triggers it, and the edit that resolves it, without a trip through
//! the language reference.
//!
//! Two rules for what goes in an entry:
//!
//! * **Name the toylang-specific trap, not the general concept.** "The
//!   types did not match" tells a reader nothing they did not already
//!   get from the diagnostic. "toylang has no implicit widening, so a
//!   `u64` reaching an `i64` slot needs `as i64`" is the part they
//!   cannot derive.
//! * **Every example compiles or fails exactly as claimed.** An
//!   explanation that misleads costs more than one that is missing —
//!   the reader spends a round trip on the wrong fix and trusts the
//!   next explanation less.

use crate::diagnostic::codes;

/// The explanation for a diagnostic code, or `None` if the code is not
/// one of ours. Matching is case-insensitive so `e0001` works.
pub fn explain(code: &str) -> Option<&'static str> {
    let normalized = code.to_ascii_uppercase();
    ENTRIES
        .iter()
        .find(|(c, _)| *c == normalized)
        .map(|(_, text)| *text)
}

/// Every code with its one-line summary, for listing.
pub fn summaries() -> Vec<(&'static str, &'static str)> {
    ENTRIES
        .iter()
        .map(|(code, text)| {
            // The first line of each entry is `E0001: <summary>`.
            let head = text.lines().next().unwrap_or("");
            let summary = head.split_once(": ").map(|(_, s)| s).unwrap_or(head);
            (*code, summary)
        })
        .collect()
}

type Entry = (&'static str, &'static str);

const ENTRIES: &[Entry] = &[
    (codes::TYPE_MISMATCH, E0001),
    (codes::TYPE_MISMATCH_OPERATION, E0002),
    (codes::NOT_FOUND, E0003),
    (codes::UNSUPPORTED_OPERATION, E0004),
    (codes::CONVERSION, E0005),
    (codes::ARRAY, E0006),
    (codes::METHOD, E0007),
    (codes::INVALID_LITERAL, E0008),
    (codes::ACCESS_DENIED, E0009),
    (codes::UNCATEGORISED, E0010),
    (codes::TYPE_HOLE, E0011),
];

const E0001: &str = "\
E0001: a value has a different type than the position it is used in

The initializer of a `val` / `var` does not fit its annotation.

    fn main() -> u64 {
        val x: bool = 1u64     # E0001: expected Bool, but got UInt64
        0u64
    }

Note what is *not* an error: a numeric annotation converts its
initializer where the conversion is exact, so `val x: i64 = 1u64` and
`val n: i64 = 42` are both fine. E0001 on a numeric pair therefore means
the value could not be represented, not merely that the names differ.

When both sides are numeric the diagnostic carries a machine-applicable
suggestion holding the exact replacement text -- `--diagnostics=json`
exposes it as `suggestions[].replacement`. When they are not (`u64`
reaching a `bool`, as above) no suggestion is offered, because no cast
would fix it: the value itself is wrong.

An argument that does not match its parameter reports the same code,
with the offending argument named in the context: `(in argument 2 of
function 'f')`. There *is* no implicit conversion at a call site, so
that form always needs an explicit `as`.

Two shapes that reach this code without looking numeric:

  * a float literal written without its suffix. `1.5` is not an `f64`
    literal in toylang -- write `1.5f64` (or `1f64`). The bare form is
    rejected so `outer.0.1` stays unambiguous as tuple access. Depending
    on where it sits, the bare form can also surface as E0010 with a
    complaint about tuple indexing.
  * a block whose last expression is the value. `if c { 1u64 } else { }`
    gives the `else` branch type `()`.";

const E0002: &str = "\
E0002: a binary operator got operands of incompatible types

Arithmetic, comparison and bitwise operators require both sides to have
the same type. There is no promotion between numeric types.

    fn main() -> u64 {
        val a: u64 = 1u64
        val b: i64 = 2i64
        val c = a + b          # E0002: u64 and i64
        0u64
    }

Fix: cast one side so both agree -- `a + (b as u64)`.

For a `struct` on both sides this is not a type error at all: `+` / `-`
/ `*` / `/` / `%` dispatch to `add` / `sub` / `mul` / `div` / `rem`
methods when the struct has them, and `==` / `!=` to `eq`. Reaching
E0002 with two values of the same struct type means the method is
missing (or has the wrong signature: arithmetic wants
`(&self, &Self) -> Self`, comparison `(&self, &Self) -> bool`).";

const E0003: &str = "\
E0003: a name could not be resolved

A function, variable, field, type or enum variant is not in scope at the
point it is used.

    fn main() -> u64 {
        val x = calculate_totl(3u64)   # E0003
        0u64
    }

When exactly one name in scope is a close enough spelling, the
diagnostic carries a machine-applicable `did you mean` suggestion. When
two are equally close it deliberately offers nothing -- guessing between
`print` and `println` for `printn` would be wrong half the time.

Causes that are not typos:

  * the name is defined in a module that was never imported. `import`
    the module, or qualify the call.
  * the name is a method, and the call was written bare. Methods are
    reached through a receiver: `v.len()`, not `len(v)`.
  * an identifier that was meant to be a number. A leading `_` makes
    `_42` an identifier, not a literal with a digit separator.";

const E0004: &str = "\
E0004: the construct is not defined for this type

The syntax is valid but means nothing for the type it was applied to.
Most often a field access on something that has no fields.

    fn main() -> u64 {
        val n: u64 = 1u64
        val x = n.field         # E0004: field access 'field' for type UInt64
        0u64
    }

Other sources:

  * a `struct` field, or a method parameter / return type, declared with
    a type the checker does not accept in that position
  * field access through `Self` in an `impl` block where `Self` resolved
    to something without that field

Two nearby failures that are *not* this code:

  * a `u64` subtraction that would go below zero type checks and traps
    at run time (`u64 subtraction underflowed`). Compute in `i64` when
    the result can be negative.
  * `%` on `f64` passes the type checker. It fails later, in the
    backend, because the operation is deliberately unsupported there.";

const E0005: &str = "\
E0005: a numeric literal does not fit the type it is being read as

The literal's text is a number, but not one the target type can hold.

    fn main() -> u64 {
        val x: i64 = 99999999999999999999
        0u64
    }
    # E0005: Cannot convert '99999999999999999999' to UInt64

Fix: use a value inside the type's range, or a type that can hold it.
`i64` spans -9223372036854775808..=9223372036854775807 and `u64`
0..=18446744073709551615; nothing wider exists.

Digit separators (`1_000_000u64`, `0xDEAD_BEEFu64`) are allowed between
digits and do not affect the value. A leading `_` makes the token an
identifier rather than a number, which reports E0003 instead.

An `as` cast between types that have no conversion (`true as u64`) is
*not* this code -- it reports E0010 with `Cannot cast Bool to UInt64`.";

const E0006: &str = "\
E0006: an array operation is invalid

Raised for array literals whose elements disagree in type, an empty
array literal, an index that is not an integer, or a slice of something
that cannot be sliced.

    fn main() -> u64 {
        val xs = [1u64, true]
        0u64
    }
    # E0006: Array elements must have the same type, but element 1 has
    #        type Bool while first element has type UInt64

`val xs = []` is also this code: an empty literal gives the checker
nothing to infer the element type from. Annotate the binding, or start
it with an element.

Array types are written `[T; N]` for a fixed size and `[T]` when the
size is not part of the type.";

const E0007: &str = "\
E0007: a method call could not be resolved or did not fit

The receiver's type has no such method, or it has one whose signature
does not accept the call.

    struct Counter { n: i64 }
    fn main() -> u64 {
        val c = Counter { n: 0i64 }
        val x = c.increment()   # E0007: no method `increment` on Counter
        0u64
    }

Where methods come from:

  * an inherent `impl Counter { ... }` block
  * `impl <Trait> for Counter { ... }` -- trait methods are registered as
    inherent methods too, so `c.trait_method()` works without importing
    the trait
  * a trait `default` body, inherited by any impl that omits the method
  * extension traits in the stdlib (`Substring`, `Trim`, `Concat`, ...)
    which is how `s.trim()` works on a plain `str`

If the method exists by name, the mismatch is in its shape -- argument
count, argument types, or a receiver the call does not supply. Note that
the receiver form (`self: Self` / `&self` / `&mut self`) is *not*
checked against the mutability of the binding, so a `&mut self` method
called through a `val` resolves; that is not the cause.

Use `--api <module>` to list the methods a module actually provides
rather than reading its source.";

const E0008: &str = "\
E0008: a numeric literal's text is not a number at all

Reserved for a literal token the checker cannot parse as either `i64` or
`u64` and that no type hint claimed first.

In practice this is nearly unreachable today: an out-of-range literal
hits the E0005 path before it gets here, because a numeric annotation is
almost always in scope by then. If you *do* see E0008, the token is
malformed rather than merely too large -- check the digits and the
suffix.

Related codes: a value that does not fit its target is E0005; a value of
the wrong kind entirely is E0001.";

const E0009: &str = "\
E0009: an item is not visible from here

The name resolved, but it is private to the module that declares it.

    # modules/helper.t
    fn secret() -> u64 { 1u64 }      # not pub

    # main.t
    import helper
    fn main() -> u64 { helper::secret() }

Fix: mark the declaration `pub` in the module that owns it.

**Not enforced yet.** The check exists but its \"are we in the same
module?\" test currently answers yes for every caller, so private items
are reachable across modules and this code is not emitted. Write `pub`
on what you mean to export anyway -- when the check is completed, code
that relied on the gap stops compiling.

It is deliberately a separate code from E0003. \"Not found\" and \"found
but private\" call for opposite edits -- one changes the call site, the
other changes the declaration -- so conflating them would send the
reader to the wrong file.";

const E0010: &str = "\
E0010: an error that has no more specific code yet

The catch-all. The message text is the whole of the diagnostic; there is
no category behind this code to look up. It is a large and common
bucket, not a rare one.

Frequent members, with what each actually means:

  * `Cannot cast X to Y` -- `as` is defined between numeric types only.
    For a bool, write `if b { 1u64 } else { 0u64 }`.
  * `Cannot access index N on non-tuple type ...` -- usually a float
    literal written without its suffix. `1.5` lexes as `1` `.5`, which
    looks like tuple indexing. Write `1.5f64`.
  * the reference-escape rule: a `val` / `var` binding cannot hold `&T`.
  * a `requires` / `ensures` clause that is not `bool`.

Syntax errors do not reach this code, or any code -- `else if` (write
`elif`), a stray `;`, and other parse failures are reported separately,
before type checking runs.";

const E0011: &str = "\
E0011: the answer to a type hole -- not a defect

Writing `_` in place of a `val` / `var` type annotation asks the checker
what the initializer's type is. It reports the answer and then fails the
program, so the question cannot be left in code that runs.

    fn main() -> u64 {
        val total: _ = 1i64 + 2i64
        0u64
    }
    # E0011: type hole: `total` has type `i64`

Fix: replace the `_` with the reported type -- or delete the annotation
entirely, since the binding would infer the same type anyway.

Every hole in a file is answered in a single run, and the binding keeps
its inferred type, so later uses of the variable are checked normally
instead of collapsing into follow-on errors.

`_` is only accepted in a `val` / `var` annotation. In a parameter,
return or field type it is a parse error -- there is nothing there for
the checker to infer from.";

#[cfg(test)]
mod tests {
    use super::*;

    /// A code with no explanation is worse than no code: `--explain`
    /// answers "unknown code" for something the compiler just printed.
    #[test]
    fn every_code_has_an_explanation() {
        let missing: Vec<&str> = codes::ALL
            .iter()
            .copied()
            .filter(|c| explain(c).is_none())
            .collect();
        assert!(missing.is_empty(), "codes without an explanation: {missing:?}");
    }

    /// Each entry's first line has to be `E00NN: <summary>`, because
    /// that is what the listing and the `--explain` header read.
    #[test]
    fn entries_start_with_their_own_code() {
        for (code, text) in ENTRIES {
            let head = text.lines().next().unwrap_or("");
            assert!(
                head.starts_with(&format!("{code}: ")),
                "entry for {code} starts with {head:?}"
            );
        }
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert_eq!(explain("e0001"), explain("E0001"));
        assert!(explain("E9999").is_none());
    }

    #[test]
    fn summaries_cover_every_code() {
        assert_eq!(summaries().len(), codes::ALL.len());
        assert!(summaries().iter().all(|(_, s)| !s.is_empty()));
    }
}
