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
    (codes::LEXICAL, E0012),
    (codes::RECURSIVE_TYPE, E0013),
    (codes::MOVED_VALUE, E0014),
    (codes::RESERVED_LITERAL, E0015),
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
  * `... generic parameter 'T' bound violation` -- the type argument
    inferred at this call site does not implement the trait the
    parameter is bounded by. It names the offending trait; write an
    `impl <Trait> for <Type>` block, or bound the caller's own type
    parameter with the same trait so the bound passes through. Methods
    inherit their impl block's bounds, so `v.sort()` reports this when
    the element type has no `Ord` impl.

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
return or field type it is a parse error — there is nothing there for
the checker to infer from.";

const E0012: &str = "\
E0012: a literal or character could not be read (lexical error)

The lexer could not make sense of some source text — most often a
literal whose content is invalid. The diagnostic points at the
offending literal, on its own line.

  * `\"\\q\"` — unknown escape sequence
  * `\"\\x80\"` — `\\xHH` can only encode ASCII bytes. `str` is UTF-8,
    and a lone byte >= 0x80 has no representation inside one: write
    `\\u{HEX}` (or the character itself) for non-ASCII code points.
  * `\"\\x\"` / `\"\\x4\"` / `\"\\xZZ\"` — `\\x` requires exactly two
    hex digits
  * `\"abc {x` — the `{...}` interpolation never closes
  * `'\\u{110000}'` — a code point outside the Unicode scalar range
    (max U+10FFFF, no surrogates)
  * `123abc` — digits followed by letters is not a number
  * `$` — a character no rule recognizes

Escapes are decoded at lex time, so a bad escape is a lexical error —
there is no way to write it that parses. `'\\xff'` as a *char* literal
is unaffected: it yields the `u32` value 255, not str bytes.

The `\\xHH` ASCII limit and the `\\u{HEX}` spelling of the fix are
documented in the language reference's string-literal section.";

const E0013: &str = "\
E0013: a type contains itself with no indirection

Every backend flattens a struct / enum / tuple down to its leaf
scalars, so a type that holds a value of itself has no finite size.

    enum List { Cons(i64, List), Nil }   # E0013: List::Cons.1: List
    struct Node { v: i64, next: Node }   # E0013: Node.next: Node

The diagnostic prints the member chain that closes the cycle, so a
cycle through two types names both hops:
`A.b: B -> B.a: A`.

A type argument is containment only when the type it is passed to
holds that parameter **by value**:

    struct Tree { v: i64, kids: Vec<Tree> }       # fine
    struct Wrapper<T> { v: T }
    struct Held { w: Wrapper<Held> }              # E0013

`Vec` keeps its elements behind a `ptr`, so `Tree`'s own layout does
not contain a `Tree`. `Wrapper` stores its `T` in a field, so `Held`
does.

Write the indirection by hand instead. Either store an index into a
side table:

    struct Node { v: i64, next: u64 }   # index into a `Vec<Node>`

or hold the value in a `Box<T>` — the stdlib struct for one
heap-allocated `T`, whose parameter appears in no field:

    enum List { Cons(i64, Box<List>), Nil }

or, at a lower level, hold a raw `ptr` and go through the heap
builtins:

    struct Node { v: i64, next: ptr, has_next: bool }

    val p: ptr = __builtin_heap_alloc(__builtin_sizeof(rest))
    __builtin_ptr_write(p, 0u64, rest)
    val rest: Node = __builtin_ptr_read(n.next, 0u64)

The annotation on the read is what gives it a shape -- it names the
type whose leaves come back out of the buffer, so it cannot be left
off. A struct, tuple or enum may be named there.

An enum in a buffer is a `u64` tag followed by every variant's payload,
so a `ptr` payload can carry the recursion:

    enum List { Cons(i64, ptr), Nil }

`ptr`, function types and `dyn Trait` are the positions that break a
cycle. `&T` is not: it is erased to `T` at lowering, so `next: &Node`
recurses exactly like `next: Node`.";

const E0014: &str = "\
E0014: a value that owns a resource was used after it was handed away

A type with an `impl Drop` owns something the runtime gives back — a
heap block, a buffer, an arena — and the scope that built it frees it
on the way out. Putting such a value somewhere that outlives the scope
therefore hands ownership over, and the old name no longer refers to
anything live.

    val c: Box<i64> = Box::new(7i64)
    store.push(c)              # ownership goes into the Vec
    val v: i64 = c.get()       # E0014: `c` was moved on the line above

Without the rule, `c` would be freed at the end of its scope while the
`Vec` still held the pointer: the interpreter would return a value and
the compiled binary would trap.

Fix it by reading the value through whatever now owns it
(`store.get(0u64)`), or by not handing it over — a parameter declared
`&T` / `&mut T` borrows, so passing to one of those leaves the caller
in charge.

Ownership is transitive: a `Vec<Box<i64>>`, an enum carrying a `Box`
payload, or a struct holding one by value owns resources too, so
handing *it* over moves the whole thing.

The positions that hand ownership over are: an argument in a by-value
parameter, a field of a struct / tuple / array being built, an enum
payload, and the right-hand side of an assignment. `val b = a` is
**not** one of them: compound bindings alias in this language, so `a`
and `b` name one value with one owner.

The same code also reports a hand-over this compiler will not model:

    if cond { store.push(c) }  # E0014: cannot be moved inside a branch

Whether `c` still owns its value at the end of the scope would depend
on `cond`, and deciding that needs a run-time flag the backends do not
have. Lift the transfer out of the branch, or build the value inside
it.";

const E0015: &str = "\
E0015: a reserved literal that no backend implements

`null` is a keyword the grammar still accepts, but nothing runs it.
It used to take on whatever type its position wanted and pass the
type check, then stop the program the moment it was evaluated:

    val name: str = null       # E0015
    var n: u64 = 0u64
    n = null                   # E0015

The language models absence with `Option<T>` instead, so that the
`match` is exhaustive and every backend agrees on the layout:

    val name: Option<str> = Option::None
    match name {
        Option::Some(s) => println(s),
        Option::None => println(\"(anonymous)\"),
    }

A raw pointer is a separate case, because address 0 is a real value
rather than an absent one:

    val p: ptr = __builtin_null_ptr()
    if __builtin_ptr_is_null(p) { ... }

The universal `is_null()` method is unsupported for the same reason
the literal is — it only made sense on a `null` that worked.";

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
