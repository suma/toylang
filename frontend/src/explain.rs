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
    (codes::NEVER_ALLOCATES, E0016),
    (codes::CONST_FN, E0017),
    (codes::CONTRACT_PURITY, E0018),
    (codes::RUNTIME_PANIC, E0019),
    (codes::CONTRACT_VIOLATION, E0020),
    (codes::CAPTURED_ASSIGN, E0021),
    (codes::REGION_ESCAPE, E0022),
    (codes::IMPL_PRECONDITION, E0023),
    (codes::UNSAFE_REQUIRED, E0024),
    (codes::UNUSED_RESULT, E0025),
    (codes::WINDOW_ESCAPE, E0026),
    (codes::BORROW_COPY_OUT, E0027),
    (codes::OWNING_ELEMENT_COPY, E0028),
    (codes::PARALLEL_BODY, E0029),
    (codes::UNKNOWN_MODULE_PATH, E0030),
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
function 'f')`, or `(in argument 1 of method 'fill')` for a method
call. There *is* no implicit conversion at a call site, so that form
always needs an explicit `as`. A `&mut T` parameter is the one case
where no cast is the answer: pass the borrow the parameter asks for
(`h.fill(&mut out)`), since a bare value would be copied and the
callee's writes lost.

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
  * an operator the type does not define. Operator overloading is a
    struct feature and needs the method written out, so `a == b` on a
    struct wants `fn eq(&self, other: &P) -> bool` in `impl P`. Enums
    do not overload operators at all — match on the variants:

        val same: bool = match a {
            Color::Red => match b { Color::Red => true, Color::Green => false },
            Color::Green => match b { Color::Green => true, Color::Red => false },
        }

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
  * `... generic parameter 'T' compares its values with `==`, but ...`
    -- the body being called compares two values of that type
    parameter, and the type argument at this call site has no answer
    for `==`. Write `fn eq(&self, other: &T) -> bool` in an `impl T`
    block. An enum cannot: comparison overloading is a struct feature,
    so match on the variants instead (or carry a scalar tag). No bound
    is involved -- the requirement comes from the body, not from the
    signature, which is why it is reported at the call rather than at
    the declaration.

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
    val rest: Node = __builtin_ptr_read::<Node>(n.next, 0u64)

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

const E0016: &str = "\
E0016: a function declared `never_allocates` can reach the allocator

`never_allocates` is the compile-time counterpart to
`ensures allocates(0u64)`. That clause measures one call and reports
what it cost; this one says the call cannot allocate at all, and the
compiler checks it by following every path out of the function.

    never_allocates fn triangle(n: u64) -> u64 {
        var total: u64 = 0u64
        var i: u64 = 1u64
        while i <= n { total = total + i  i = i + 1u64 }
        total
    }                                    # fine: no path reaches the allocator

    never_allocates fn build(n: u64) -> u64 {
        var v: Vec<u64> = Vec::new()     # E0016: build -> new ->
        v.push(n)                        #        __builtin_heap_alloc
        n
    }

The message names the path, because the allocation is usually not in
the function you wrote — `Vec::new` is rejected for reaching
`__builtin_heap_alloc`, not for being on a list.

What counts as allocating is what the allocation counters count:
`__builtin_heap_alloc` and `__builtin_heap_realloc`, plus everything
built on them. Memory the runtime spends holding a `str` is not the
program\'s allocation, so `println(\"{x}\")` is allowed.

The other form of this error is a call the check cannot follow:

    never_allocates fn run(f: fn () -> u64) -> u64 { f() }   # E0016

A closure value, a `dyn Trait` receiver, or an `extern fn` lands
somewhere with no body to walk. Assuming such a call is
allocation-free would make the whole guarantee worthless, so it is
refused instead. For `extern`, the author can take responsibility with
`never_allocates extern fn getchar() -> i32 from \"c\"`, which is a
declaration rather than a proof.

To fix: remove the allocating call, take a buffer as a parameter
instead of building one, or drop the `never_allocates` and state the
weaker runtime bound with `ensures allocates(0u64)`.";

const E0017: &str = "\
E0017: a function declared `const fn` cannot be evaluated at compile time

`const fn` says the function *may* be run while compiling — so that
`const N: u64 = double(21u64)` becomes `42` before any backend sees
it. The compiler checks the promise by following every path out of the
function, the same walk `never_allocates` uses, and refuses anything
it could not actually run.

    const fn double(n: u64) -> u64 { n * 2u64 }   # fine
    const D: u64 = double(21u64)                  # folded to 42u64

    const fn noisy(n: u64) -> u64 {
        println(\"hello\")                          # E0017: noisy -> println
        n
    }

What is refused, and why:

  - the allocator and raw pointers — a folded result has to fit in a
    scalar constant, so nothing reachable may build a heap value
  - `print` / `println` — they would write to the *compiler's* stdout,
    and whether they ran at all would depend on whether the fold
    happened
  - the allocation counters and `__builtin_current_allocator` — they
    answer questions about a run, and there is no run yet
  - `extern fn`, closure calls, and `dyn Trait` receivers — there is no
    body to walk and no way to call the implementation while compiling.
    Unlike `never_allocates`, `extern` gets no escape hatch here: the
    author's word that a C function is pure still does not let the
    compiler run it

Calling an *unannotated* function is fine. This is a reachability
check, not an attribute that has to be propagated, so a `const fn` may
call any function whose own reachable set is clean.

`panic` and `assert` are allowed on purpose: reaching one during a
fold is reported as a compile error, which is better than the same
call certainly aborting at run time.

To fix: move the printing or the allocation to the caller, or drop the
`const fn` and let the call happen at run time.";

const E0018: &str = "\
E0018: a contract that does more than answer a question

Reported as a **warning** today. `COMPILE_TIME_EVAL.md` C4 gives it one
release at that severity before it becomes an error, because programs
were written against a compiler that never objected.

Two shapes share the code.

**An impure predicate.** A contract is a statement *about* the
program, so switching the checks off with `INTERPRETER_CONTRACTS` or
`--release` must not change what the program does:

    fn noisy(n: u64) -> bool { println(\"checking\")  n > 0u64 }
    fn f(n: u64) -> u64 requires noisy(n) { n }    # E0018

That program printed `checking` with the contracts switched off, since
the switch is read by the tree-walker while the default engine has the
checks lowered into the IR. The check follows every path out of the
clause and refuses one that allocates, frees, writes through a
pointer, or prints. Reads are fine, the allocation counters included —
`ensures __builtin_live_bytes() == old(__builtin_live_bytes())` is
what ALLOC-CONTRACT is for. A call it cannot follow (an `extern fn`, a
closure value, a `dyn Trait` receiver) is reported too: an
implementation outside the language cannot be shown to do nothing.

**A constant call that breaks its own precondition.** When every
argument is a constant, the compiler can run the check itself:

    const fn half(n: u64) -> u64 requires n % 2u64 == 0u64 { n / 2u64 }
    val b: u64 = half(3u64)                        # E0018

This is a warning rather than an error even after the migration,
because nothing here knows whether the call is reached — the same
reason `if false { 1u64 / 0u64 }` stays legal. In a position that
*forces* a value, like a `const` initialiser, the identical failure is
an error instead (`E0017`).

To fix: move the effect out of the predicate — compute it in the body
and compare against a parameter — or pass an argument the contract
accepts.";

const E0019: &str = "\
E0019: the program stopped while it was running

Not a defect the compiler found — a failure the program reached. Three
things carry this code:

- `panic(\"...\")`, which the program asked for;
- a failed `assert(cond, \"...\")`;
- a RUNTIME-TRAP guard: `u64` subtraction that would wrap, integer
  division by zero, signed `MIN / -1`, an array index past the end.

All four execution engines report it the same way — the position, the
source line with a caret under the failing expression, and a backtrace
innermost-first:

    Runtime error occurred:
    Error at demo.t:2:20:
       |
     2 |     if n == 0u64 { panic(\"bottom\") }
       |                    ^^^^^ panic: bottom
       |
       = backtrace (innermost first):
           f (x7, called at line 3)
           main

`--diagnostics=json` gives the same failure with `span` and
`backtrace` as data. A `--release` build keeps the position and drops
the backtrace, which is the only part with a run-time cost.

To fix: the trap cases have checked forms in `core/std/checked.t`
(`checked_sub` / `checked_div` return `Option<T>`); the `panic` cases
are the program's own decision, so the fix is wherever the value that
reached it came from — which is what the backtrace is for.";

const E0020: &str = "\
E0020: a contract was false at run time

A `requires` clause was false on entry, or an `ensures` clause was
false on exit. Unlike E0018, which is about the *shape* of a contract,
this is the contract doing its job.

    fn half(n: u64) -> u64
        requires n % 2u64 == 0u64
    {
        n / 2u64
    }
    fn main() -> u64 { half(3u64) }   # E0020

The report names the clause, the function, and the values the
predicate saw:

    Contract violation: `requires` clause #1 of function `half`
    evaluated to false (with n = 3)

An `ensures allocates(N)` / `retains(N)` clause reports the measured
amount against the budget instead of a bare false.

A `requires` violation is the *caller\'s* defect: the backtrace names
the call that passed the argument. An `ensures` violation is the
function\'s own.

To fix: check the precondition before calling, or widen the contract if
the value was legal after all. `INTERPRETER_CONTRACTS=off` and
`--release` switch the checks off, which hides the report without
making the program correct.";

const E0021: &str = "\
E0021: a closure assigns to a binding it captured by copy

A closure that is only ever called where it is defined *shares* the
bindings it captures: reads see the current value and writes reach the
outer binding.

    var count: u64 = 0u64
    val bump = fn() -> u64 { count = count + 1u64  count }
    bump()                      # 1
    bump()                      # 2
    count                       # 2

A closure that can outlive those bindings cannot share them — the
frame that owns them may be gone by the time it runs — so it captures
a copy, and writing to a copy reaches nothing. That is this error:

    fn make() -> fn () -> u64 {
        var count: u64 = 0u64
        fn() -> u64 { count = count + 1u64  count }     # E0021
    }

A closure escapes by being used as a value rather than called:
returned, passed to a function, stored in a struct, or called from
inside another closure. The judgement is syntactic and deliberately
blunt — being wrong this way costs a diagnostic, being wrong the other
way reads a dead frame.

Writing *through* a capture (`p.x = ...`, `a[i] = ...`) follows the
same rule as writing to it. Before the rule existed the shape of the
value decided the meaning: a captured compound kept its cell, so the
write reached the outer binding on the three interpreter engines,
while the two compiled engines could not build the program at all.

To fix an escaping closure: return the value and assign it at the call
site, or keep the closure where its captures live.

    var count: u64 = 0u64
    val bump = fn(n: u64) -> u64 { n + 1u64 }
    count = bump(count)

The rule is about *where the binding is*, not how it was declared. A
`val` capture is refused for the same reason a `var` one is, so \"use
`var`\" is not the fix.";

const E0022: &str = "\
E0022: memory from a scoped allocator outlives the allocator

`with allocator = arena { ... }` routes every allocation in the body
through `arena`, and the arena hands all of it back at once when it
goes out of scope. A pointer that leaves with a longer life than the
arena is already dead when it is used:

    fn leak() -> ptr {
        val arena = Arena::new()
        with allocator = arena { __builtin_heap_alloc(8u64) }   # E0022
    }                                     # arena frees it here

The value may not be returned, and it may not be bound or assigned to
a name declared outside the arena\'s own scope. Staying inside that
scope is fine — the arena is still alive there:

    val arena = Arena::new()
    val p = with allocator = arena { __builtin_heap_alloc(8u64) }
    __builtin_ptr_write(p, 0u64, 7u64)          # fine

This is about where the memory came from, not about the type. A `u64`
read back out of arena memory is a copy and escapes nothing, so
`with allocator = arena { list.get(0u64) }` is legal while
`with allocator = arena { list }` is not.

Only allocators whose life this can see are checked: one bound by a
`val` / `var` in the same function, or one built inline. A parameter
or a field belongs to the caller, so allocating from it and handing
the result back — what `Arena::alloc` itself does — is correct and
unchecked.

To fix it: copy what you need out of the region before leaving it,
allocate from an allocator that lives long enough (`with allocator =
__builtin_default_allocator() { ... }`), or move the arena out to the
scope the value has to reach.";

const E0023: &str = "\
E0023: an implementation demands more than its trait promised

A `requires` clause is what callers are told to satisfy. Code that
reaches a method through the trait — `&dyn Trait`, or a `<T: Trait>`
bound — can read the trait\'s clauses and nothing else, so an
implementation that adds one of its own breaks calls that were
written correctly:

    trait Shrink {
        fn shrink(&self, by: u64) -> u64
            requires by > 0u64
    }

    impl Shrink for B {
        fn shrink(&self, by: u64) -> u64
            requires by < 100u64          # E0023
        { self.n - by }
    }

    fn use_it(s: &dyn Shrink) -> u64 {
        s.shrink(200u64)                  # honours `by > 0u64`, still fails
    }

An implementation may inherit a precondition; it may not strengthen
one. The same applies when the trait declares no precondition at all —
then callers may pass anything the types allow, and any clause the
impl adds is stronger than that.

Postconditions are the other way round and stay free: an
implementation that promises *more* than its trait breaks nobody, so
`ensures` on an impl is added to the trait\'s and both are checked.

Three ways out:

* **Move the clause to the trait**, when every implementation should
  demand it. That is the usual answer — the obligation belongs where
  callers can read it.
* **Handle the case in the body**, returning `Option` / `Result` or
  panicking with a message, when only this implementation cares.
* **Widen the type**, when the clause is really saying the parameter
  should not have been able to hold that value.

Deliberate *weakening* — an implementation that accepts more than its
trait requires — is sound but has no syntax yet (Eiffel spells it
`require else`). Leave the clause off: the trait\'s precondition still
applies, and accepting more than you promised to accept breaks no
caller.";

const E0024: &str = "\
E0024: raw memory access needs an `unsafe fn` declaration

The body calls a builtin that reads or writes raw memory
(`__builtin_ptr_read`, `__builtin_ptr_write`, the `mem_*` family,
`__simd_load` / `__simd_store`, ...) but is not declared `unsafe fn`.
The declaration is what makes a raw-pointer dialect visible at the
signature level — `--effects` answers the same question as
`raw_read` / `raw_write`:

    fn get(&self, i: u64) -> T {          # E0024: __builtin_ptr_read ...
        val v: T = __builtin_ptr_read::<T>(self.addr, i * 8u64)
        v
    }

    unsafe fn get(&self, i: u64) -> T {   # fine
        val v: T = __builtin_ptr_read::<T>(self.addr, i * 8u64)
        v
    }

The check is **direct**: a function is asked only about its own body,
not about what its callees do — calling an `unsafe fn` (or anything
the stdlib hides behind `Ptr<T>` / `Span<T>`) keeps the caller safe,
which is the point of those types. Ordering with the other prefix
modifiers is free (`never_allocates unsafe fn`, `unsafe const fn`).

Three ways out:

* **Declare the function `unsafe fn`**, when the raw access is the
  function's job.
* **Go through `Ptr<T>` / `Span<T>`** (`core/std/ptr.t` /
  `core/std/span.t`), which concentrate the raw builtins in the
  stdlib — the caller stays safe.
* **Delete the access**, when a plain binding or a `Vec<T>` says the
  same thing without leaving the language.";

const E0025: &str = "\
E0025: a `Result` was produced and discarded

The statement evaluates to a `Result` and nothing reads it, so a
failure it reports goes nowhere:

    fn main() -> u64 {
        io::write_file(\"out.txt\", body)   # E0025: the disk could be full
        0u64
    }

That program answers success whatever happened. The language has no
exceptions by design — a failure travels in the return value or not at
all — so a discarded `Result` is the one shape where an error can go
missing without anyone deciding to ignore it.

Three ways out, and the third is the point:

    match write_file(path, body) {        # handle it
        Result::Ok(n) => { ... }
        Result::Err(e) => { println(e) }
    }

    write_file(path, body)?               # propagate it

    val _ignored = write_file(path, body) # ignore it, on the record

No new syntax for the last one: binding the value is what says the
result was considered and dropped on purpose, which a best-effort
write on a shutdown path really is.

The rule is narrow on purpose. Only a statement that is **not** the
last one in its block counts — a block's last statement is its value,
and whether that value is wanted is a question the block cannot
answer. So every report is a place the value provably goes nowhere.

`Option` is not covered. An ignored `Option` is usually a lookup whose
absence is the answer; an ignored `Result` is an unreported failure.

Reported as a warning: programs were written this way before the check
existed, and ignoring a failure can be deliberate.";

const E0030: &str = "\
E0030: a call names a module path that does not exist

    zzz::math::min_i64(3i64, 7i64)   # E0030: no module is called `zzz::math`

A qualifier is checked **from the end**: `math::min_i64` finds
`std.math.min_i64` because the path ends in `math`, and the leading
segments are optional. Optional is not the same as ignored — until
this check, everything before the last segment was dropped, so a
wrong path resolved exactly like a right one and ran.

Write as many trailing segments as it takes to name one module, and
no more: `math::` where the name is unique, `std::math::` if
something else grows a `math`.

**The extra segments do not yet pick between two candidates.** When
a bare name is ambiguous, `[E0010]` lists the competing paths;
writing more of one of them is the fix it suggests, and that half is
still to come (`design-docs/MODULE_SYSTEM.md` P3).";

const E0029: &str = "\
E0029: a parallel loop body depends on the order of its iterations

`parallel for i in 0u64..n { .. }` says the iterations may run in any
order, and now they do. Four things in a body would make that
visible, so none of them is allowed in one:

    parallel for i in 0u64..n {
        println(i)                    # E0029: interleaved output
    }

    parallel for i in 0u64..n {
        with allocator = arena { .. }  # E0029: a scoped allocator
    }

    var acc = 0u64
    parallel for i in 0u64..n {
        acc = acc + i                 # E0029: a name from outside
    }

    parallel for i in 0u64..n {
        if found(i) { break }         # E0029: break (and return)
    }

Output because interleaved lines are not the same output, and the
lanes that run the loop sequentially would stop agreeing with the
ones that do not. Collect what each iteration produces — into a slot
of its own, indexed by `i` — and print after the loop.

A scoped allocator because the region check reasons about one control
flow; several iterations sharing one arena is outside what it can
say. The body runs on the default allocator.

A write to a name from outside because the next iteration would read
what the last one wrote, which is the one thing `any order` cannot
survive. Give each iteration a place of its own — a slot indexed by
`i`, written through a window (`Span<T>`, whose `set` reaches the
buffer it views) — and combine them after the loop. A `var` declared
*inside* the body is one per iteration and may be assigned freely.

`break` and `return` because both mean `stop the rest`, and the rest
may already have run or be running. `continue` is fine: ending one
iteration is a thing an iteration can decide on its own.

A body that *calls* something which writes to an outer binding
(`v.push(x)`) is refused too, by the compiler rather than here —
knowing that `push` writes to `v` takes the callee's signature.

**What is not checked is whether the iterations are independent.**
Writing to `out[i]` is yours to get right, and `requires` is where to
say it. That is the same decision `Span` makes about aliasing: the
language has no aliasing rules, and inventing one for this construct
alone would leave two rule systems to keep straight.

A `parallel for` takes a range. An iterator has an order of its own,
and splitting one is a different question (`design-docs/CONCURRENCY.md`).";

const E0028: &str = "\
E0028: an owning element taken out of a container by value

`get` answers with the element itself, and for a type that owns
something — `String`, `Vec<T>`, `Box<T>`, `TcpStream`, anything holding
one — what comes back is a shallow copy: the same pointer, the same
descriptor. The container still holds it, and now so does the binding.
Both free it.

    var conns: Vec<TcpStream> = Vec::new()
    conns.push(cl)
    val s: TcpStream = conns.get(0u64)   # E0028
    # `s` dies at the brace and closes the descriptor the table still
    # lists; the next write reports EBADF

Say what you mean instead:

    val s: &TcpStream = conns.borrow(0u64)   # name it, do not claim it
    val s: TcpStream = conns.get(0u64).clone()  # a second one, if that
                                                # is really what you want

A scalar element is unaffected — `val n: u64 = v.get(i)` copies a
number, which owns nothing.

**Handing the value straight on is allowed.** If the binding gives the
value away before it dies — into another container, into a by-value
parameter, back into the same slot with `set` — there is only ever one
owner, and the check says nothing. It is the binding that *keeps* the
value that is reported.

The rule reads a name: `get`. That is the convention this language
already dispatches on (`eq` for `==`, `to_str` for printing, `next` for
`for`), and the callee cannot be asked instead — a function that reads
an element out of raw memory looks exactly like `pop`, which is
*supposed* to hand ownership over.

Every place this check found in the stdlib was a live bug: `Vec::clone`
freed the elements it was copying, `String::join` freed the parts it
joined, and the JSON reader freed a node's text each time it read the
node. None of them failed a test, because the bump heap never reuses an
address — the second free is invisible until the resource is something
the OS hands back once.";

const E0027: &str = "\
E0027: an owning value copied out of a borrow

A borrow names something somebody else owns. Taking a *value* out of
one copies the handle, not the resource, so the copy and the original
would both free it —

    val e = v.borrow(0u64)       # `e` names the element
    val s: String = e            # E0027: `s` would own it too

For a scalar this is harmless and allowed: a `u64` owns nothing, so
`val n: u64 = e` is a plain read. It is types that free something —
`String`, `Vec<T>`, `Box<T>`, anything holding one — that are refused.

Two ways forward. Read through the borrow, which is what it is for
(`e.len()`, `e.field`, comparisons, printing). Or take a copy of your
own, and say so:

    val s = e.clone()            # a second String, with its own buffer

The rule exists because the alternative is silent: on a heap that never
reuses an address the second free is invisible, and the damage shows up
only for something the OS hands back once, like a descriptor.";

const E0026: &str = "\
E0026: a window outlives the buffer it views

`Span<T>` and `Column<T>` are views, not owners: they hold an address
into somebody else's memory. When that memory belongs to a binding in
the same frame, the window cannot leave the frame with it —

    fn dangling() -> Option<Span<u8>> {
        var v: Vec<u8> = Vec::new()
        v.push(1u8)
        v.as_span()                 # E0026: `v` dies at the brace below
    }

The returned window points at freed memory. Nothing else catches this:
the move check follows values whose *type* owns a resource, and a
`Span` owns nothing — the `Vec` does.

The rule is the one REGION (`E0022`) uses for a scoped allocator, over
a different owner: a value derived from something whose end this pass
can see must not reach a place that outlives it. So a window may not
be returned, and may not be bound or assigned outside the buffer's own
scope. Staying inside it is fine — that is what a window is for.

**A window on a parameter is not this shape.** The buffer belongs to
the caller, so handing the window back out is correct, and it is
exactly how the stdlib is written:

    fn as_span(&self) -> Option<Span<T>>   # `self` is a parameter: fine

Two ways out when the check fires:

* **Return the owner** and let the caller take the window. The buffer
  then lives as long as whoever asked for it.
* **Copy what you need out** — a scalar read out of the window is a
  copy and escapes nothing.

The check does not follow a window through a closure, and it does not
know about reallocation: a `push` that grows a `Vec` invalidates every
window on it, which is a separate hazard the type system does not
cover.";

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
