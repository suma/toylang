# Language Reference

A consolidated reference for the toy language implemented in this repo.
Authoritative for syntax and semantics of `.t` source files. For
implementation-side details, see the companion documents:

- [`JIT.md`](../design-docs/JIT.md) — cranelift JIT supported subset, diagnostics, performance
- [`ALLOCATOR_PLAN.md`](../design-docs/ALLOCATOR_PLAN.md) — allocator-system design and roadmap
- [`BUILTIN_ARCHITECTURE.md`](../design-docs/BUILTIN_ARCHITECTURE.md) — three-layer builtin design
- [`TEST_PLAN.md`](../design-docs/TEST_PLAN.md) — testing strategy
- [`interpreter/README.md`](../interpreter/README.md) — interpreter CLI and env vars

## Table of Contents

- [Hello, world](#hello-world)
- [Lexical structure](#lexical-structure)
- [Types](#types) (incl. syntax + reference types)
- [Type checker](#type-checker)
- [Literals](#literals)
- [Expressions](#expressions)
- [SIMD vectors](#simd-vectors)
- [Statements](#statements)
- [Functions](#functions)
- [Closures](#closures)
- [Structs and methods](#structs-and-methods)
- [Traits](#traits)
- [Enums and pattern matching](#enums-and-pattern-matching)
- [Generics and bounds](#generics-and-bounds)
- [Modules](#modules) (incl. core auto-load + extern fn)
- [Allocators](#allocators)
- [Built-in functions and methods](#built-in-functions-and-methods) (numeric methods + Option / Result + math + string are stdlib `core/std/*.t`)
- [Test blocks](#test-blocks)
- [Design by Contract](#design-by-contract)
- [Runtime model](#runtime-model)
- [Known limitations](#known-limitations)

---

## Hello, world

```rust
fn main() -> u64 {
    println("hello")
    0u64
}
```

Every program needs a `main` function. The integer it returns becomes the
process exit code.

---

## Lexical structure

### Comments

```rust
# single-line comment, terminates at newline

/* block comment, multi-line.
   Block comments do NOT nest. */
```

Both forms can appear inline or on their own line.

### Identifiers

`[A-Za-z_][A-Za-z0-9_]*`. Reserved keywords cannot be used as identifiers
(the parser rejects `val val = 0` etc.).

### Keywords

Reserved words, none of which can be used as identifiers:

```
fn  val  var  const  type  return  break  continue
if  elif  else  for  in  to  while  loop
class  struct  trait  impl  dyn  enum  match  Self
true  false  null
pub  extern  package  import  as  mut
with  ambient
requires  ensures
u8  u16  u32  u64  i8  i16  i32  i64  f64
bool  str  ptr  usize  dict
```

`else if` is **not** valid; use `elif`.

`test` is **contextual**, not reserved: `test "name" { ... }` at the
top level declares a test block (see [Test blocks](#test-blocks)),
while `fn test(...)` and `val test = ...` keep working as ordinary
identifiers.

`unsafe` and `never_allocates` are **contextual** the same way: they
are modifiers only immediately before `fn` (in either order, and
either side of `const`), so `val unsafe = 3u64` and
`fn never_allocates()` keep their ordinary meaning. See
[`unsafe fn`](#unsafe-fn--raw-memory-access).

`@` is reserved as the [labelled-loop](#control-flow) prefix
(`@outer:`, `break @outer`, `continue @outer`) and cannot appear
elsewhere — there are no decorators / attributes that share the
sigil.

### Statement separators

Statements are separated by newlines. Semicolons are not used and not
accepted as statement separators.

### Whitespace and newlines

Whitespace within a statement is insignificant. A `-` at the start of a
new source line is parsed as unary negation of a fresh expression, not as
a continuation of the previous statement:

```rust
val a: i64 = 10i64
-a              # this is its own statement, not `10 - a`
```

---

## Types

### Syntax of Type

Type annotations appear after a `:` in `val` / `var` / parameter / field
declarations and after `->` in function / method return positions. The
parser recognises the following grammar (see
`frontend/src/parser/types.rs`); each form maps to a `TypeDecl` variant
in `frontend/src/type_decl.rs`:

```
Type ::= '&' ('mut')? Type             # reference (immutable / mutable)
       | 'soa' '[' Type (';' INT)? ']' # SoA-layout array (see "Array layout: soa")
       | 'soa' 'Vec' '<' Type '>'      # sugar for SoaVec<T> (see "Vec layout: soa Vec<T>")
       | '[' Type (';' INT)? ']'       # array: [T; N] or dynamic [T]
       | 'dict' '[' Type ',' Type ']'  # dict[K, V]
       | '(' Type (',' Type)* ')'      # tuple
       | '(' ')'                       # unit
       | PrimitiveKeyword              # bool / u8..u64 / i8..i64 / f32 / f64 / str / ptr
       | VectorKeyword                 # f64x2 / f32x4 / i32x4 / i64x2 / u8x16
       | 'Allocator'                   # opaque allocator handle
       | 'Self'                        # enclosing impl target
       | Identifier ('<' Type (',' Type)* '>')?
                                        # struct / enum / type alias, optionally generic
```

Primitive / built-in types:

| Type | Description |
|---|---|
| `bool` | `true` / `false` |
| `u8` / `u16` / `u32` / `u64` | unsigned integers (8/16/32/64-bit) |
| `i8` / `i16` / `i32` / `i64` | signed integers (8/16/32/64-bit) |
| `f64` | IEEE 754 double-precision float |
| `f32` | IEEE 754 single-precision float (SIMD-F32; added for `f32x4` — see [SIMD](../design-docs/SIMD.md)) |
| `f64x2` / `f32x4` / `i32x4` / `i64x2` / `u8x16` | 128-bit SIMD vectors. See [SIMD vectors](#simd-vectors) |
| `str` | UTF-8 string handle (interned literal or `.rodata` reference) |
| `ptr` | Raw heap pointer (0 = null) |
| `usize` | Reserved keyword, used in some builtin signatures |
| `()` | Unit (no value); function with no return type produces this |
| `dict[K, V]` | Built-in dictionary literal type, **interpreter-only** — the compiled lanes refuse it. The portable map is the stdlib `Dict<K, V>`; see [`Dict<K, V>`](#dictk-v-stdlib) |
| `[T; N]` | Fixed-size array of `T` with length `N` |
| `soa [T; N]` | The same type as `[T; N]` with SoA storage — see [Array layout: `soa`](#array-layout-soa) |
| `soa Vec<T>` | Sugar for the stdlib `SoaVec<T>` — a *distinct* type from `Vec<T>`; see [Vec layout: `soa Vec<T>`](#vec-layout-soa-vect) |
| `Column<T>` | A window onto one field of every element (`ps.mass`); see [Column windows](#column-windows-psfield) |
| `[T]` | Dynamic-array slice (returned by slicing) |
| `(T1, T2, ...)` | Tuple — heterogeneous, fixed-arity |
| `Self` | The enclosing struct/enum type within an `impl` block |
| `null` | Reserved. The literal is refused by the type checker (`E0015`); the internal `Object::Null(T)` remains only as a runtime backstop |
| `Allocator` | Opaque allocator handle (see [Allocators](#allocators)) |

Composite / user-defined types:

| Type | Description |
|---|---|
| `Name` | User-defined struct or enum (the parser emits `Identifier(name)`; the type checker resolves it to `Struct(name, [])` or `Enum(name, [])`) |
| `Name<T1, ...>` | Generic struct or enum instantiation |
| `&T` / `&mut T` | Immutable / mutable reference to `T`. Used in **parameter** positions (`fn push_str(&mut self, other: &String)`) and in explicit-borrow expressions (`f(&mut s)`). See [Reference types](#reference-types) below |

### Reference types

A `&T` (immutable) / `&mut T` (mutable) annotation marks a parameter as
**call-by-reference**: the caller does not pay for a value copy. The
current implementation (REF-Stage-2 (a)+(d)+(e)+(f)) covers the
type-system surface; the borrow checker is **escape-only** — there are
no lifetimes, no aliasing rules, no field-level borrow, no iterator
invalidation.

Type system:

- `&T` and `&mut T` are parsed as `TypeDecl::Ref { is_mut, inner }` and
  are **not equivalent** to `T`. The two forms are also distinct from
  each other.
- Argument compatibility (`is_arg_compatible`):
  - `T` → `&T` is allowed via **auto-borrow** at the call site
    (`s.push_str(b)` for `b: String`).
  - `T` → `&mut T` is **not** auto-borrowed; the caller must write
    `&mut <var>` so the mutability is visible at the call site
    (Rust-style discipline).
  - `&mut T` → `&T` is allowed (a mutable reference satisfies an
    immutable expectation).
  - The reverse direction (`&T` / `&mut T` flowing into `T`) is
    rejected; there is no auto-deref.
- Method dispatch on a `&T` / `&mut T` receiver auto-derefs to `T` for
  impl-table lookup (`(&s).len()` and `s.len()` resolve identically).

Explicit borrow expressions (`UnaryOp::Borrow` / `UnaryOp::BorrowMut`):

- Prefix `& expr` produces `&T`; prefix `&mut expr` produces `&mut T`.
- `&mut <name>` is the **only** accepted shape for a mutable borrow:
  - The operand must be a bare identifier (no `&mut s.field`,
    `&mut arr[i]`, etc.; field-level borrow is intentionally out of
    scope).
  - The named binding must be `var`-declared. Borrowing a `val` (or
    a top-level `const`) mutably is rejected with a precise diagnostic
    naming the binding.
- Binary `&` (bitwise AND) remains unambiguous since it is reached only
  after a primary; prefix `&` lives at the start of an expression.

Escape rule (REF-Stage-2 (e)) — references can only flow into a
function via parameters / method receivers and cannot **escape** the
referent's frame:

- Returning a reference is rejected: `fn f(x: &u64) -> &u64` is a
  compile-time error.
- Storing a reference in a `val` / `var` binding is rejected — both an
  explicit annotation (`val r: &u64 = &a`) and an inferred type
  (`val r = &a`) trip the rule.
- Storing a reference in a struct field is rejected
  (`struct S { r: &u64 }`).
- Compound types containing a reference (e.g. `(u64, &u64)`,
  `[&u64; 4]`) are caught the same way via `TypeDecl::contains_ref()`.

Without lifetimes, this conservative escape rule is the only protection
against dangling references. Once IR-level pointer passing lands the
escape rule will still hold; the borrow check has no "promotion" path.

Lowering — references are currently **erased**: both the interpreter
and the AOT backend pass references identically to values (struct
receivers still leaf-flatten). The frontend type checker is the one
enforcing call-by-reference semantics; a future phase will introduce
`Type::Ref` + `AddressOf` so `&mut T` argument types can propagate true
mutation back to the caller (today only `&mut self` mutates the
caller's binding, via the Self-out-parameter convention from REF Stage
1).

Out of scope (deferred):

- IR-level pointer passing (`Type::Ref`, `AddressOf`); true mutation
  propagation through `&mut T` non-self parameters.
- Aliasing / borrow exclusion rules (multiple `&mut` to the same
  binding remain allowed at runtime).
- Lifetimes / scope inference.
- `&mut s.field` / `&mut arr[i]` and similar non-identifier lvalues.
- Iterator invalidation checks.

The `&self` / `&mut self` receiver forms are part of the same family
but predate `&T` (REF Stage 1) and live in their own parser path.
Stdlib read-only methods (`String::len`, `Vec::size`, …) take `&self`;
mutating ones (`Vec::push`, `String::push_str`) take `&mut self`.

### Type aliases

`type Name = TargetType` declares a synonym for an existing type at
the top level. Aliases are pure rewrites — they do not introduce a
new nominal type — and they carry zero runtime cost: every backend
sees the substituted target type after parsing.

```rust
type Byte = u32
type Word = Byte                 # alias chain — collapses to u32
type Pair<T> = Box<T>            # generic alias

struct Box<T> { v: T }
type IntPair = Pair<i64>         # generic alias used as a target

fn id(b: Byte) -> Byte { b }
val p: IntPair = Box { v: 21i64 }
```

Resolution happens in two layers:

- **Per-file (parser-time)** — within the file that declares the
  alias, the parser eagerly substitutes any subsequent `Byte` /
  `Pair<u8>` / `IntPair` mention with the target type.
- **Cross-module (post-integration)** — after every auto-loaded /
  imported module has been merged into the program AST, a global
  pass (`frontend::resolve_type_aliases`) substitutes alias
  references that survived parsing in other modules. This is what
  lets a stdlib alias (`core/std/string.t::type String = Vec<u8>`)
  reach user code transparently.

Properties:

- **Forward references work.** `val x: Foo = ...` followed later
  by `type Foo = i64` resolves cleanly: the per-file pass leaves
  the unknown identifier alone, and the post-integration pass
  substitutes it after the alias is collected.
- **Alias chains collapse.** `type A = u8; type B = A; type C = B`
  end up with `C` resolving directly to `u8` — the substitution
  recurses on the resolved target.
- **Generic aliases are arity-checked at the use site.** Bare uses
  of a generic alias (`Pair` without `<...>`) and arity mismatches
  are rejected by the type checker with proper source-location
  context.
- **`pub type` is parsed but not yet enforced** at module
  boundaries — alias visibility currently flows through the
  global cross-module pass for any auto-loaded module.

Stdlib types:

- `core/std/char.t::type char = u32` — Unicode codepoint alias.
  Char literals (`'a'` / `'\n'` / `'\u{1F600}'`) lex straight to
  `Kind::UInt32`, so a `c: char` parameter receives any Unicode
  scalar value without truncation. `String::push_char(c: char)`
  UTF-8 encodes the codepoint into 1-4 bytes (RFC 3629); surrogate
  codepoints (U+D800..U+DFFF) and codepoints >= U+110000 panic.
- `core/std/string.t::struct String { data, len, cap, elem_size }`
  — **nominal struct** (no longer a `type` alias for `Vec<u8>`).
  Memory layout matches `Vec<u8>` exactly so the
  `__builtin_heap_*` / `ptr_read` / `ptr_write` family operates
  on the underlying byte buffer with no per-type special-casing,
  but the type system treats `String` as its own identity (errors
  say "expected String", `String` and `Vec<u8>` are not
  interchangeable). Construction:

  ```rust
  val s: String = String::from_str("hello")
  var t: String = String::new()
  ```

  Inherent methods on `impl String`: `new` / `from_str` / `push` /
  `pop` / `get` / `set` / `size` / `len` / `as_ptr` / `capacity` /
  `is_empty` / `clear` / `extend_bytes` / `push_str` / `push_char`
  / `eq` / `to_string`. Extension trait impls (in
  `core/std/str_ops.t`): `Substring` / `Trim` / `CaseConvert` /
  `Concat<String>` / `Contains<String>` / `Split<String,
  Vec<String>>` — so `.substring(s, e)` / `.trim()` /
  `.to_upper()` / `.to_lower()` / `.concat(other)` /
  `.contains(needle)` / `.split(sep)` work on both `str` and
  `String` with the same call shape (`str` routes through
  builtins, `String` through trait impls). `==` / `!=` between
  Strings dispatches to `eq` via the operator-overload path.

  `Vec<u8>` (the generic byte vector) remains available as a
  distinct type for byte-level work that doesn't need String
  semantics — the alias is gone, so byte-vector and string
  values no longer collapse. Convert across the boundary with
  explicit constructors / `as_ptr()` chains.

### Type inference

Local variable annotations are optional when the rhs is unambiguous:

```rust
val a = 42        # u64 by default
val b: i64 = 42   # context infers i64; literal is silently widened/converted
val c = 3.14f64   # f64
```

Without an annotation, integer literals default to `u64`. With an
annotation that conflicts (`val x: bool = 42`), the type checker errors.

For generic struct construction the val annotation also drives the
generic argument when no field directly references the type parameter:

```rust
struct Container<T> { value: u64 }
val c: Container<u8> = Container { value: 0u64 }   # T = u8 from annotation
```

The runtime / IR pick this up so concrete-args dispatch
(`impl Trait for Container<u8>` vs `impl Trait for Container<i64>`)
sees the right `[u8]` type-arg vector at the receiver.

#### Type holes (`_`)

Writing `_` where a `val` / `var` annotation goes asks the type checker
what the initializer produces. It reports the answer and fails the
program — a hole is a question, not code meant to survive.

```rust
fn main() -> u64 {
    val total: _ = 1i64 + 2i64
    0u64
}
```

```
 2 |     val total: _ = 1i64 + 2i64
   |                    ^^^^^^^^^^^ [E0011] type hole: `total` has type `i64`
```

Every hole in a file is answered in one run, and each binding keeps its
inferred type, so later uses are checked normally rather than collapsing
into follow-on errors. The reported type is spelled the way source
spells it, so it can be pasted over the `_`.

`_` is accepted only in this position. In a parameter, return, field or
element type there is no initializer to infer from, and the parser
rejects it.

---

## Type checker

The frontend type checker runs once per program after parsing and
module integration, before any execution or AOT lowering. It walks the
AST via `frontend/src/type_checker/visitor.rs` and assigns a
`TypeDecl` to every expression, validates declarations, and rejects
programs that won't run safely.

### Position in the pipeline

```
source ─► lexer ─► parser ─► AST + ExprPool/StmtPool
                      │
                      └──► auto-load / module integration
                                    │
                                    ▼
                            ┌───────────────┐
                            │ Type checker  │  ◄── this section
                            └───────────────┘
                                    │
                            ┌───────┴───────┐
                            ▼               ▼
                       interpreter     AOT compiler
                                       (Cranelift IR)
```

A type-check failure is fatal — the program is reported and not
executed / not lowered. Errors carry source locations so the formatter
can underline the offending token.

### What it checks

1. **Declarations** — every `struct` / `enum` / `trait` / `impl` block
   is visited; field / variant / method types are validated. Trait
   conformance is checked structurally: an `impl Trait for Type` must
   provide every method the trait declared, with matching parameter
   and return types (modulo `Self` / generic substitution).
2. **Function bodies** — each `fn` is type-checked top-down. Parameter
   types annotate the symbol table; the body's tail expression must
   match the return type (or `()` if none).
3. **Expressions** — arithmetic / comparison / logical / bitwise /
   shift / cast / call / method-call / field / index / range — each
   produces a typed value or fails. Branches of `if` / `match` /
   blocks must agree on type.
4. **Statements** — `val` / `var` annotation matching, `return` /
   `break` / `continue` flow validity, `for` / `while` body returning
   `()` outside its tail.
5. **Patterns** — `match` arms must cover the scrutinee
   exhaustively; literal / variant / nested patterns are checked
   for shape and reachability (see
   [Exhaustiveness and reachability](#exhaustiveness-and-reachability)).

### Type equivalence and assignment

Two distinct relations live on `TypeDecl`:

- **`is_equivalent(a, b)`** — used for assignment, return-type
  matching, trait conformance, and pattern equality. Treats
  `Identifier(name)` as a stand-in for the registered `Struct(name, _)`
  / `Enum(name, _)` so user-named types unify with their resolved
  form. Generic-typed values (`Generic(T)`) and `Unknown` accept
  anything during inference. **`&T` and `T` are NOT equivalent**, and
  `&T` ≠ `&mut T` — the reference distinctions are real.
- **`is_arg_compatible(actual, expected)`** — used at every
  argument-passing site (call / method-call / associated function /
  module function). Falls back to `is_equivalent`, plus two narrow
  relaxations:
  - `T` → `&T` auto-borrow (the immutable case only).
  - `&mut T` → `&T` downgrade (a mutable reference satisfies an
    immutable expectation).
  Importantly, `T` → `&mut T` auto-borrow is **rejected** — the caller
  must write `&mut <name>` explicitly so the mutability is visible
  at the call site.

Assignment is also rejected for any reference type: `val r: &T = ...`
is a [REF-Stage-2 (e)](#reference-types) escape error — references
cannot be stored in `val` / `var` bindings at all, regardless of the
rhs's type.

### Method dispatch

When the type checker sees `obj.method(args)`:

1. Compute `obj_type` via `visit_expr`. Auto-deref `&T → T` so the
   inner type drives the lookup (`(&s).len()` resolves the same as
   `s.len()`).
2. Refine `Identifier(name)` → `Struct(name, [])` / `Enum(name, [])`
   for any registered top-level type, since the parser cannot
   distinguish bare names from aliases at parse time.
3. Look the method up in `context.struct_methods` /
   `enum_definitions` / primitive extension-trait registry.
4. For generic structs / enums, build a substitution map from the
   receiver's concrete type-args, then resolve `Self` and any
   `Generic(P)` in the method's parameter / return types.
5. Validate each actual argument with `is_arg_compatible`.

Receiver kinds are summarised in the following table:

| Receiver in `fn ...(<receiver>, ...)` | Mutability | Writeback (AOT) |
|---|---|---|
| `self: Self` | by-value | n/a (mutations are local) |
| `&self` | by-reference, immutable | n/a |
| `&mut self` | by-reference, mutable | yes — Self-out-parameter writeback (REF Stage 1) |

### Concrete-args impl dispatch

A `(struct, method)` pair may have **multiple impls** with distinct
`target_type_args` (e.g. `impl Trait for Vec<u8>` and
`impl Trait for Vec<i64>` coexist). Lookup picks the matching spec by
the receiver's concrete type-args via a 3-tier fallback:

1. exact match on `target_type_args`;
2. generic-parameterised impl with empty `target_type_args` (matches
   anything);
3. lone-spec fallback (single spec wins regardless of args; preserved
   for static-dispatch sites that don't yet thread an annotation
   hint, e.g. `Vec::from_str(s)`).

### `Self` resolution

Inside an `impl` block, `Self` resolves to the impl's target type:

| Impl form | `Self` resolves to |
|---|---|
| `impl Foo` | `Identifier(Foo)` (later refined to `Struct(Foo, [])` / `Enum(...)`) |
| `impl<T> Foo<T>` | `Struct(Foo, [Generic(T)])` (T stays generic in the body) |
| `impl Trait for Vec<u8>` | `Struct(Vec, [u8])` (concrete args propagated through Self) |
| `impl Foo for i64` | `Int64` (primitive impl target) |

### Trait conformance

For `impl Trait for Type`, the type checker iterates the trait's
declared method signatures and matches them against the impl's:

- Method name must exist in the impl.
- Receiver kind (`self: Self` / `&self` / `&mut self`) must match
  exactly. (`self_is_mut` flag on `MethodFunction`.)
- Parameter and return types must be `is_equivalent`, with `Self`
  substituted to the impl target.
- Generic params on the impl block (`impl<T> Trait for Foo<T>`) are
  substituted before comparison.

A mismatch produces a precise diagnostic naming the missing or
non-conforming method.

### Generics

Generic functions, structs, enums, and methods are validated in
template form (with `Generic(T)` placeholders). Concrete
instantiations are produced lazily:

- The interpreter uses runtime values to bind type parameters.
- The AOT compiler monomorphises on demand: each call site infers
  the substitution from the actual argument types, looks up the
  cached `(name, type_args) → FuncId`, and lowers a fresh body if
  needed.

`<T: Trait>` bounds are validated when the type checker sees a
generic call: the actual type must implement the trait. Bound
inheritance through impl / struct / method levels is respected.

### Built-in / extension dispatch

Primitive receivers (`i64.abs()`, `u8.hash()`) dispatch through the
same `struct_methods` registry, keyed by the canonical name symbol
(`"i64"`, `"u8"`, …). Extension traits over primitives
(`impl Hash for u8`) register their methods under that symbol; the
type checker prefers user impls over the legacy hardcoded
`BuiltinMethod` arms.

### Pattern matching

`match` expressions are checked for:

- **Type agreement** — every pattern's binding shape must match the
  scrutinee type.
- **Exhaustiveness** — without a wildcard arm, every variant /
  literal value must be covered. Nested enum payloads are
  recursively checked.
- **Reachability** — duplicate variants, literals, or arms after a
  wildcard are rejected as unreachable.

### Errors

A type error is represented by `TypeCheckError`
(`frontend/src/type_checker/error.rs`). The visitor accumulates
errors with source locations rather than aborting on the first one
when feasible, so the user sees multiple problems per run. The
front-end driver (`interpreter::check_typing*` /
`compile_file`) routes errors through `ErrorFormatter` for the
caret-pointer formatting visible in test output.

Every diagnostic carries a stable code (`E0001`…`E0021`). These are
toylang's own numbering, not Rust's — identical-looking identifiers with
different meanings would be worse than none. `interpreter --explain
<CODE>` prints the category, a program that triggers it, and the fix;
`interpreter --explain` with no argument lists them all.

Failures that happen *while the program runs* carry codes too: `E0019`
for a panic, a failed `assert` or a runtime trap, and `E0020` for a
violated `requires` / `ensures`.

`--diagnostics=json` emits the same diagnostics on stderr in machine
form, including spans and any machine-applicable fix. A runtime
failure adds a `backtrace` array — `function` plus the `line` it was
called from, innermost first, with no `line` on the entry frame:

```json
{
  "severity": "error",
  "code": "E0019",
  "message": "panic: boom",
  "file": "core/std/option.t",
  "span": { "line": 57, "column": 29, "offset": 1893, "end_offset": 1898 },
  "backtrace": [
    { "function": "Option::unwrap", "line": 4 },
    { "function": "main" }
  ]
}
```

`file` is the *failure's* file, which is not necessarily the one being
run: a panic inside the stdlib names the stdlib.

---

## Literals

### Integer literals

```
42u64       # u64
42i64       # i64
42u8        # u8       (also: u16 / u32 / i8 / i16 / i32)
42i32       # i32
0xFFu64     # hex u64
0xFFi64     # hex i64
0xFFu8      # hex narrow int (range-checked at lex time)
0xFF        # suffix-less; type resolved by context (default u64)
42          # suffix-less; type resolved by context
-3i64       # i64 with leading minus inside the lexer
```

The narrow widths (`u8` / `u16` / `u32` / `i8` / `i16` / `i32`) work
identically to `u64` / `i64`: the lexer validates the literal fits,
the parser stores the value at its native width, and the type
checker / interpreter / JIT / AOT compiler all carry the width
through end-to-end.

#### How a suffix-less literal gets its type

A literal written without a suffix is not `u64` yet. The parser
records it as an unresolved placeholder, and the first position
that states an expected integer type claims it:

| Position | Example | Literal becomes |
|---|---|---|
| Binding annotation | `val c: i64 = 10` | `i64` |
| Assignment target | `m = 5` where `m: i64` | `i64` |
| Function / method / associated-function argument | `f(21)` for `fn f(x: i64)` | `i64` |
| Closure parameter | `c(5)` for `fn(x: i64) -> i64` | `i64` |
| Declared return type (tail expression) | `fn main() -> u64 { 0 }` | `u64` |
| Explicit `return` | `return 1` in `fn f() -> i64` | `i64` |
| Closure body | `fn() -> i64 { 5 }` | `i64` |
| Struct field | `P { x: 5 }` for `x: i64` | `i64` |
| Enum payload | `E::V(5)` for `V(i64)` | `i64` |
| Array / tuple / dict element | `val a: [i64; 3] = [1, 2, 3]` | `i64` |
| A typed sibling element | `[1i64, 2, 3]` | `i64` |
| Arithmetic with a typed operand | `c - 40` where `c: i64` | `i64` |
| Unary minus | `-5` | `i64` |

The branch tails of an `if` / `elif` / `match` count as the position
their enclosing expression is in, so `fn f() -> i64 { if c { 1 } else
{ 2 } }` needs no suffixes.

A generic position is not one of these — there is no concrete type to
take, and the literal is what decides the parameter:

```
fn id<T>(x: T) -> T { x }
val v: i64 = id(5)      # `5` resolves through the annotation, not `T`
val b = B { v: 5 }      # struct B<T> { v: T } — B<u64>, by the default
val b: B<i64> = B { v: 5 }   # annotated: B<i64>
```

All of these accept the narrow widths too, and the value is
range-checked against the target: `f(300)` for `fn f(x: u8)` is a
conversion error, not a silent wrap.

A literal that reaches none of these positions falls back to
**`u64`**. So does one that reaches a position expecting a
non-integer type — that is a type error either way, and reporting
`expected bool, but got u64` gives the reader a type they can act on.

The fallback is **`u64`**, and it is load-bearing: `u64` subtraction
traps on underflow (see *Runtime traps*), so `val a = 5  val b = 10
a - b` panics rather than producing `-5`. Annotate when the value can
go negative.

The resolution is per function and per literal. A literal in one
function is never decided by an annotation in another, and an
annotated binding never retypes its unannotated neighbours:

```
val b: i64 = 10     # b is i64
val a = 42          # a is still u64 — b's annotation is not contagious
```

A type hole reports the type the literal actually ends up with, not
the fallback: in `val a = 42  val h: _ = a  f(a)` where `f` takes an
`i64`, the hole answers `i64`.

#### Numeric separators

`_` is allowed between digits as a visual grouping aid for both
decimal and hexadecimal literals. The first character must be a
digit (so `_42` parses as an identifier, not a number); after
that any number of `_` may appear between digits or before the
type suffix.

```
1_000_000u64        # one million
1_2_3_4i64          # 1234 (separators may be irregular)
42_u64              # underscore before the suffix is allowed
0xDEAD_BEEFu64      # hex literal with separators
0xFF_FFu32          # 65535
3_141.592_653_f64   # floats too — both integer and fraction parts
1_000_000           # suffix-less; type resolved by context
```

The separators carry no semantic weight — `1_000` and `1000`
produce the same value at the same type. Lexer-level only; the
AST / IR / runtime never see the underscore.

### Float literals

Float literals always require an explicit `f64` or `f32` suffix to
disambiguate them from tuple-access syntax (`outer.0.1`):

```
3.14f64
42f64       # = 42.0f64
-2.5f64
3.14f32     # single precision (SIMD-F32)
42f32       # = 42.0f32
```

The decimal text is parsed as f64 then narrowed to f32 by rounding;
a value outside f32's range becomes ±inf, matching Rust's `1e40f32`.

A bare `1.5` is **not** a valid token in this language. Exponent
notation is not part of the grammar either — `1.0e300f64` is a lex
error (`E0012`), so write the digits out or compute the value.

To convert an integer to a float, use `as`:

```rust
val i: i64 = 5i64
val f: f64 = i as f64
```

### Boolean and null literals

```
true
false
null
```

`null` still lexes and parses — so that writing it produces a
diagnostic rather than an "undefined identifier" — but **the type
checker refuses it**:

```
[E0015] `null` is reserved and has no runtime meaning: model an absent
        value with `Option<T>`, or a raw null pointer with
        `__builtin_null_ptr()`
```

It used to type-check instead (a `null` in a typed position took that
position's type) and then stop the program the moment it was
evaluated, which meant the type system accepted programs no backend
could run. Model absence with `Option<T>`; the universal `is_null()`
method is refused for the same reason — see [`is_null`](#is_null).
`__builtin_null_ptr()` is the separate, working, raw-pointer case.

### Char literals

A single-quoted single character is a `u32` value carrying the
character's Unicode code point. `val c = 'a'` infers `u32` and
`__builtin_sizeof(c)` is 4 — a code point is held in 32 bits, which
is what the `char` alias names.

```rust
'A'           # 65u32
'z'           # 122u32
' '           # 32u32 (space)
'\n'          # 10u32  (line feed)
'\t'          # 9u32
'\r'          # 13u32
'\0'          # 0u32   (NUL)
'\\'          # 92u32  (literal backslash)
'\''          # 39u32  (literal single quote)
'\"'          # 34u32  (literal double quote)
'\xHH'        # 0xHH as u32 (exactly 2 hex digits)
'\u{HEX}'     # Unicode code point (1-6 hex digits, max 0x10FFFF)
'\u{1F600}'   # 😀 = 128512u32
```

**A position naming another integer type gets it** (CHAR-LITERAL-NUM).
This is the one exception to the [NUM-W](#integer-literals) rule that
integer types never convert implicitly:

```rust
val b: u8 = '0'          # 48u8
val i: i64 = '\n'        # 10i64
val big: u8 = '\u{1F600}' # error: 128512 does not fit a u8
val f: u8 = 42u32        # error: a suffixed literal is not a character
```

The exception exists because a string's bytes are `u8` while a
character is a code point: `s.get(i) == 'h'`, `c >= '0' && c <= '9'`
and `c - '0'` are the shape byte-scanning code takes, and requiring
`48u8` with the character in a comment is how that code looked
before. Only a literal *written as a character* moves — a suffixed
literal already named its type, so nothing is left to decide — and
only when the code point fits, so nothing is truncated silently.

A generic parameter counts as naming a type once the receiver has
decided it. `Vec<u8>::push(v: T)` is declared with a `T`, but a
`Vec<u8>` has already fixed `T` to `u8`, so `v.push('B')` and
`s.set(0u64, 'A')` on a `Span<u8>` narrow the same way an explicitly
`u8` parameter would. A parameter the *method* introduced
(`fn pick<U>(other: U)`) names nothing the receiver knows, so there
the argument still decides.

The two levels stay distinct in the standard library: **byte-wise
access and iteration yield `u8`** (`String::get(i)`, `String::iter()`,
`Vec<u8>`), while the **character-level API takes the `u32` code
point** (`push_char(c: char)`, which UTF-8 encodes it into 1-4
bytes). A char literal meets either without a cast.

Multi-byte UTF-8 between bare quotes (`'あ'`) is **not** accepted at
the lexer surface today — use `'\u{3042}'` instead. (The constraint
comes from the rflex regex backend not supporting `\xNN` byte
ranges in character classes; the lexer comment block at the rule
site has the details.)

The companion alias `core/std/char.t::type char = u32` names the
code point type. Use it at signature sites that want
to document "this is a codepoint" rather than a raw integer.
`Vec<u8>::push_char(c: char)` UTF-8 encodes the codepoint into 1-4
bytes following RFC 3629; surrogate codepoints (U+D800..U+DFFF) and
values >= U+110000 panic.

### String literals

```rust
"hello"           # ConstString — interned, immutable
"line1\nline2"    # \n decoded to LF in the lexer
"hex \x41 here"   # \x41 decoded to 'A' (ASCII only, see below)
"unicode \u{3042} here"   # \u{3042} encoded as 3-byte UTF-8 'あ'
"日本語 ♠ 😀"      # non-ASCII source characters pass through verbatim
```

The lexer decodes the same escape ladder as the char literal rule
(`\n` / `\t` / `\r` / `\0` / `\\` / `\'` / `\xHH` / `\u{HEX}`)
once at lex time and stores the resulting bytes in the
`Kind::String(...)` token. Downstream layers see only the decoded
byte sequence.

Non-ASCII characters written **directly** in the literal are kept as
their source UTF-8 bytes, so `"♠"` is 3 bytes (`__builtin_str_len`
counts bytes) and equals `"\u{2660}"`. Unlike the char literal rule,
the string rule has no ASCII-only restriction.

`\xHH` is limited to `HH <= 0x7f`. A lone byte >= 0x80 is not valid
UTF-8 on its own and therefore has no representation inside a `str`;
`"\x80"` is a lex error. Use `\u{HEX}` (or the character itself) for
non-ASCII code points. The char literal `'\xff'` is unaffected — it
yields the `u32` value `255`, not str bytes.

Lexical failures are reported as `E0012` diagnostics that point at the
offending literal (`--explain E0012`): a bad escape, an unterminated
string or interpolation, a number followed by letters (`123abc`), or a
character no rule recognizes (`$`). A lex error fails the parse, so it
never leaks into type checking as a mismatch elsewhere in the file.

`\"` inside a `"..."` literal is **not** yet decodable — the
closing-quote regex still wins. Use `'\"'` (char) or
`"\u{22}"` (Unicode escape) when you need a literal `"` in a
string for now.

Multi-line string literals are not yet supported.

#### String interpolation

A string literal that contains one or more `{expr}` segments is
an **interpolated** string. The lexer emits
`Kind::InterpolatedString(parts)` and the parser desugars it at
parse time into a chain of `.concat()` calls, lifting each
`{expr}` through `__builtin_to_string(...)`:

```rust
val name = "world"
val n: i64 = 42i64

"hello {name}"
# ⇒ "hello ".concat(__builtin_to_string(name))

"n={n}, n*2={n * 2i64}"
# ⇒ "n=".concat(__builtin_to_string(n))
#       .concat(", n*2=")
#       .concat(__builtin_to_string(n * 2i64))
```

`__builtin_to_string(value)` produces the same display string
`print` / `println` would emit (powered by
`Object::to_display_string` in the interpreter), so every
primitive (`i64` / `u64` / `f64` / `bool` / `str` / narrow ints)
and user-defined struct / tuple participate. AOT side uses
`toy_to_string_<ty>` runtime helpers for primitives, and the
new `InstKind::ConstStrBytes` (raw `.rodata` bytes, no interner
roundtrip) + `lower_struct_to_string` / `lower_tuple_to_string`
for compound values — fields/elements are walked recursively so
nested compounds (`Outer { inner: Inner { x: 3, y: 5 }, n: 7 }`,
`((a, b), c)`) format correctly. Field order matches the
interpreter (alphabetical for structs, declaration order for
tuples).

Compound interpolation examples:

```rust
struct Point { x: i64, y: i64 }
val p: Point = Point { x: 3i64, y: 5i64 }
println("p = {p}")        # p = Point { x: 3, y: 5 }

val t: (i64, u64) = (3i64, 5u64)
println("t = {t}")        # t = (3, 5)

val single: (i64,) = (42i64,)
println("{single}")       # (42,)
```

Enum values interpolate too, on every backend — the AOT / JIT
side dispatches on the variant tag through a branch chain and
concatenates that variant's payload:

```rust
enum Shape { Circle(i64), Point }
val s: Shape = Shape::Circle(3i64)
println("{s}")                       # Shape::Circle(3)

val o: Option<i64> = Option::Some(7i64)
println("{o}")                       # Option<i64>::Some(7)
```

Empty literal segments are filtered, so `"{a}{b}"` lowers
directly to `__builtin_to_string(a).concat(__builtin_to_string(b))`
without an empty-string head.

Escapes:

- `{{` lexes to a literal `{`
- `}}` lexes to a literal `}`
- All other escape sequences (`\n` / `\xHH` / `\u{HEX}` …) work
  inside literal segments unchanged

Brace nesting inside `{expr}` is tracked so struct literals
participate (`"point at {Point { x: 1, y: 2 }}"`). Nested string
literals inside `{expr}` are not yet supported (the inner `"`
terminates the outer regex).

##### Format specs

A segment may carry a **format spec** after a `:` —
`"{value:spec}"` — choosing width, alignment, padding, precision,
and radix:

```text
spec  := [align] ['0'] [width] ['.' precision] [type]
align := '<' | '>' | '^'
type  := 'x' | 'X' | 'b' | 'o'
```

```rust
val pi: f64 = 3.14159265f64
println("{pi:.2}")            # 3.14
println("[{pi:8.3}]")         # [   3.142]

val n: u64 = 42u64
println("[{n:6}] [{n:<6}] [{n:^6}]")   # [    42] [42    ] [  42  ]
println("[{n:06}]")                    # [000042]
println("{n:x} {n:X} {n:b} {n:o}")     # 2a 2A 101010 52

val label: str = "ok"
println("[{label:6}] [{label:>6}]")    # [ok    ] [    ok]
```

Details, and where this differs from Rust:

- **Precision is the reason the feature exists.** Without it there
  is no way to choose how many decimals an `f64` shows — the
  default rendering is fixed (integral values get one decimal
  place, everything else the shortest round-trippable form).
  Precision applies to `f64`; it does not truncate strings.
- **Default alignment** follows the value: numbers pad on the
  left, text and `bool` on the right. An explicit `<` / `>` / `^`
  overrides that.
- **`0` pads with zeros after the sign**, so `"{-42i64:06}"` is
  `-00042` rather than `00-42`. It applies to numbers and is
  ignored once an explicit alignment is given.
- **A non-decimal radix on a negative value** renders the
  two's-complement pattern **at the value's own width**:
  `"{-1i32:x}"` is `ffffffff`, not sixteen digits.
- **Specs apply to primitives only** — integers of every width,
  `f64`, `bool`, and `str`. A struct, tuple, or enum with a spec
  is a type error, because a single width or radix has no defined
  meaning for a value that renders through a recursive field walk.
  Give the type a [`Display`](#display) `to_str` method and
  interpolate that instead.
- **The spec is part of the literal, never a runtime value.** It
  is parsed and packed into a constant at parse time, so a
  malformed spec (`"{x:.q}"`) is a parse error rather than a
  runtime surprise, and the backends see one extra scalar
  argument rather than a string to interpret.
- Not supported (and not planned unless a program needs them): a
  fill character other than `0`, `+`, `#`, and `$`-parameterised
  width.
- An empty spec (`"{x:}"`) is exactly the default rendering — it
  lowers to the same call a spec-less segment does.

Only a depth-0 `:` starts a spec, so a struct literal's field
colon (`"{Point { x: 1i64 }}"`) and a path's `::`
(`"{Color::Red}"`) stay part of the expression.

**Backend coverage**: interpreter, AOT compiler, and the
cranelift JIT all run the desugaring end-to-end. AOT and JIT
share the runtime helpers `toy_str_concat`, the
`toy_to_string_<ty>` family (one per scalar primitive), and the
`toy_format_*` family behind format specs; both
emit identical heap-str layout
(`[bytes][NUL][u64 len LE]`, returned pointer points at the
`u64 len` field) so the result is pointer-uniform with `.rodata`
strs and flows through `print` / `println` / `__builtin_str_len`
unchanged. The interpreter JIT (`interpreter/src/jit/`, a separate
codebase) compiles plain interpolation through its own
`jit_string_literal` / `jit_to_string_<ty>` / `jit_str_concat`
helpers — pinned by
`interpreter/example/string_interpolation_jit.t`. **Format specs
are the exception**: `__builtin_format` has no interpreter-JIT
lowering, so a segment carrying a spec makes the enclosing
function fall back to the tree-walking interpreter silently.

### Array, tuple, and dict literals

```rust
val arr: [i64; 3] = [1i64, 2i64, 3i64]
val tup           = (1u64, true, 3.5f64)
val dict          = dict{"a": 1u64, "b": 2u64}
```

The `dict{...}` literal builds the built-in `dict[K, V]`, which **only
the interpreter can run** — the JIT and the AOT compiler reject it with
`compiler MVP cannot lower a dict literal yet`. Code that has to run on
every backend uses the stdlib [`Dict<K, V>`](#dictk-v-stdlib) instead.

### Array layout: `soa`

A fixed-size array annotation may carry the prefix modifier `soa`:

```rust
struct Particle { x: f64, y: f64, mass: f64 }

val ps: soa [Particle; 1024]    # x[0..1024] | y[0..1024] | mass[0..1024]
val qs: [Particle; 1024]        # element-major (AoS), the default
```

`soa` chooses the **placement** of the elements, not their meaning:

- **It is not part of type identity.** `soa [T; N]` and `[T; N]` are
  the same type everywhere types are compared — the two spellings
  assign to each other, and no API splits. The modifier exists so a
  program can be measured with and without it by editing annotations
  only.
- `soa` is a contextual keyword: it is a modifier only when the next
  token opens an array type, so `val soa = 5u64` keeps parsing.
- Reads and writes are spelled exactly as the AoS form:
  `ps[i].mass` (one leaf), `ps[i].mass = 2.0f64` (one leaf),
  `val p = ps[i]` (whole element), `ps[1..3]` (range slice — with no
  annotation the slice keeps the source's layout; an explicit
  annotation re-layouts in either direction).
- A scalar element type (`soa [u64; N]`) has one leaf, one column —
  the modifier is accepted and means nothing extra.
- **An enum element** flattens to its tag followed by every variant's
  payload (`__builtin_sizeof`'s rule), so `soa [Shape; N]` gives the
  tag a column of its own. An enum element is also the one compound
  element that can be **written whole** (`ss[i] = Shape::Point`): a
  variant has no field names to assign through the way a struct
  element's `ps[i].x = v` does. A generic enum takes its
  instantiation from the annotation (`soa [Option<i64>; 3]`), since
  `Option::Some(1i64)` names the enum but not its type argument.
- **Columns pack tightly.** Each column strides by its leaf's own
  width, where the interleaved layout pays a uniform 8 bytes per leaf:
  `soa [Mixed; 3]` for `struct Mixed { flag: bool, byte: u8, single:
  f32, wide: u64 }` occupies 42 bytes against the AoS spelling's 96,
  and the difference is entirely padding. This is the one way the two
  layouts differ in cost rather than only in placement.
- Bounds checks behave identically to the AoS form, including
  negative constant indices (`ps[-1i64].x`).

Writing a whole compound element (`ps[i] = p`) is not supported —
write individual leaves (`ps[i].x = ...`) instead.

The lowering gives each leaf scalar its own slot (`ArraySlotId`), so
the columns are ordinary scalar arrays to the IR, codegen, and the
IR VM; the tree-walking interpreter does not observe the layout at
all, which is what makes "same program, `soa` on and off, same
answer" the pinned contract across all backends. Design notes:
[`DATA_ORIENTED.md`](../design-docs/DATA_ORIENTED.md).

### Column windows: `ps.field`

A field name on an *array* (or on a `soa Vec<T>`) is the column
window: every element's copy of that field, as a `Column<T>`
(`core/std/column.t`).

```rust
struct Particle { x: f64, y: f64, mass: f64 }

fn total_mass(ms: Column<f64>) -> f64 {
    var total: f64 = 0.0f64
    var i: u64 = 0u64
    while i < ms.len() {
        total = total + ms.get(i)
        i = i + 1u64
    }
    total
}

val ps: soa [Particle; 1024] = ...
val ms = ps.mass            # Column<f64>, 1024 long
total_mass(ms)
```

This is what makes a column *passable*: `ps[i].mass` already reads one
field cheaply, but before windows there was no way to hand "the masses"
to a function.

- The window is `get` / `set` / `len` / `is_empty`. `set` writes
  through — it is a **view**, and the array behind it changes.
- It carries a stride, so the **same type describes either layout**:
  under `soa` the values are contiguous, interleaved they are one
  element apart. A function taking a column keeps compiling while the
  modifier is added and removed, which is the measurement `soa` exists
  for. The stride itself is not readable (see below).
- A `soa Vec<T>`'s column windows its **live elements** — `len`, not
  capacity.
- The field must be a scalar. A compound field occupies as many
  columns as it has leaves, and a window reads one value per stride.
- Columns are numbered in leaves: in `struct Body { pos: Point, mass:
  f64 }`, `mass` is the third column, because `pos` is two of them.
- A window is a view and owns nothing; like `Span<T>` it must not
  outlive what it points at (unchecked — see
  [`REGIONS.md`](../design-docs/REGIONS.md) for the checked case).
- There is deliberately no `as_ptr` / `as_raw` / `stride`. A column is
  addressable on the compiled lanes and not on the tree-walking
  interpreter, which holds arrays as values rather than as memory;
  keeping the address in is what lets one type mean the same thing on
  every engine.
- **Compiled-lane limit**: the window must be bound before it is
  passed (`val ms = ps.mass` then `total_mass(ms)`), the same rule
  every other compound-producing expression follows. The interpreter
  accepts `total_mass(ps.mass)` directly.

### Vec layout: `soa Vec<T>`

The heap counterpart. `soa Vec<T>` is **sugar for the stdlib
`SoaVec<T>`** (`core/std/collections/soa_vec.t`), rewritten in the
parser, so nothing downstream sees the `soa` spelling:

```rust
var ps: soa Vec<Particle> = SoaVec::new()   # column-major
var qs: Vec<Particle>     = Vec::new()      # element-major
```

`SoaVec<T>`'s call surface is `Vec<T>`'s — `push` / `pop` / `get` /
`set` / `size` / `capacity` / `is_empty` / `clear` / `iter` — so the
loops around it do not change when the layout does. One allocation is
divided into one column per leaf scalar of `T`; leaf `j` of element
`i` lives at `prefix_j * cap + i * stride_j`, where `stride_j` is the
leaf's own width and `prefix_j` the sum of the widths before it.

Unlike the stack form, **this is a distinct type, not a layout flag**:

- `soa Vec<T>` and `Vec<T>` do not assign to each other, and a
  function declared to take one refuses the other. A heap buffer's
  layout is observable — through what a grow has to move, through
  what a raw pointer into it would address — so a single type would
  need a runtime tag and a branch in every accessor.
- Switching a program between them is therefore two edits (the
  annotation and the constructor), not one.
- `as_ptr` is deliberately absent from `SoaVec<T>`: the bytes mean
  something different here.
- Growing cannot resize in place (every column but the first moves),
  so a grow allocates a fresh buffer and re-places the elements. The
  allocation *totals* still match `Vec<T>`: columns stride by their
  leaf's real width, so `cap` elements cost `cap * sizeof(T)` bytes
  in both layouts.
- A scalar `T` (`soa Vec<u64>`) has one column, whose addressing is
  byte-for-byte the AoS one.
- Elements that own memory are released with the vec: the drop glue
  walks the columns the way it walks `Vec`'s interleaved buffer, so a
  `soa Vec<Box<i64>>` frees every box.

The column arithmetic lives in two builtins, `__builtin_soa_read(base,
index, cap)` and `__builtin_soa_write(base, index, cap, value)`, which
expand to one ordinary pointer read / write per column — the IR,
codegen and the IR VM learn nothing about SoA. Both are `unsafe`
builtins (raw memory), which is why `SoaVec`'s accessors are
`unsafe fn` and its callers are not. `__builtin_soa_read` takes its
element type from the annotation, exactly as `__builtin_ptr_read`
does. The interpreter's own JIT falls back silently on both.

Design notes: [`DATA_ORIENTED.md`](../design-docs/DATA_ORIENTED.md).

---

## Expressions

### Operators

Listed lowest precedence first:

| Operator | Notes |
|---|---|
| `\|\|` | Logical OR (short-circuit) |
| `&&` | Logical AND (short-circuit) |
| `==` `!=` `<` `<=` `>` `>=` | Comparison; result is `bool`. `str` compares **content**, not identity — `"h".concat("i") == "hi"` is true |
| `??` | Null-coalesce; see [`??` operator](#-operator-null-coalesce) |
| `\|` `^` `&` | Bitwise (integer) |
| `<<` `>>` | Shift; rhs must be `u64` |
| `..` | Range expression `start..end` (half-open) |
| `+` `-` | Add / subtract. **Not** string concatenation: `str + str` is a type error (E0002) — neither `str` nor `String` provides an `add` overload; use `a.concat(b)` |
| `*` `/` `%` | Multiply / divide / remainder |
| Unary `-` | Negation (`i64`, `f64` only) |
| Unary `!` | Logical not (`bool`) |
| Unary `~` | Bitwise not (`u64`, `i64`) |
| Unary `&` / `&mut` | Borrow expression — produces `&T` / `&mut T`. `&mut` requires the operand to be a bare `var`-declared identifier; see [Reference types](#reference-types) |
| `as` | Type cast (any numeric primitive ↔ any other: i64 ↔ u64, i64/u64 ↔ f64/f32, f64 ↔ f32) |
| Postfix `?` | Early-return on `Result::Err` / `Option::None`; see [`?` operator](#-operator-early-return) |
| `.field` `.0` `.method(...)` | Field / tuple-index / method access |
| `[...]` | Indexing / slicing (arrays, dicts, structs with `__getitem__`) |

Compound assignment desugars at parse time: `x += 1` is rewritten to
`x = x + 1`. Ten forms exist — the arithmetic five (`+=`, `-=`, `*=`,
`/=`, `%=`) and the bitwise five (`&=`, `|=`, `^=`, `<<=`, `>>=`).
The lhs may be an identifier, a field access (`p.x += 1i64`), a tuple
index (`t.0 += 1u64`) or an index (`a[i] *= 2u64`).

Because `>>=` is one token, a nested generic closed immediately before
an `=` (`val v: Option<Option<u64>>= ..`) is split back into `>`, `>`,
`=` by the type-argument parser — the same treatment `>>` already got.
A single `>` in that position (`Vec<u64>= ..`) is still a parse error;
write a space before the `=`.

There are no bitwise-logical (`&&=`, `||=`) forms: short-circuiting
makes them a different operation, not a compound assignment.

### Comparison chain

Relational operators may be chained. `a < b < c` is equivalent to
`a < b && b < c`, but the intermediate expression `b` is evaluated
exactly once (it is stored in a synthetic temporary at parse time).
Any sequence of `<`, `<=`, `>`, `>=` may be chained:

```rust
if 0u64 < x <= 10u64 < 100u64 { ... }   # all three must hold
```

Equality operators (`==`, `!=`) may not be chained — they already
bind looser than `&&` and mixing them with ordering comparisons
would be ambiguous.

### `?` operator (early return)

`expr?` is a postfix operator that unwraps a `Result<T, E>` or
`Option<T>` into `T` on success and short-circuits the enclosing
function with the propagating value on failure. The two forms
desugar at type-check time into:

```rust
# expr : Result<T, E>
val x = compute()?
# behaves like (when the enclosing fn returns Result<T, E>):
val x = {
    val __try_t = compute()
    match __try_t {
        Result::Ok(__try_v) => __try_v as T,
        Result::Err(__try_e) => {
            return __try_t
        },
    }
}

# If the enclosing fn's success type differs (`Result<T2, E>` around
# a `?` on `Result<T1, E>`), the error arm reconstructs against the
# declared return type instead of re-returning the scrutinee:
#       val __try_err: Result<T2, E> = Result::Err(__try_e)
#       return __try_err

# expr : Option<T>
val x = lookup()?
# behaves like:
val x = {
    val __try_t = lookup()
    match __try_t {
        Option::Some(__try_v) => __try_v as T,
        Option::None => {
            return __try_t
        },
    }
}
```

The desugar runs inside the type checker (not the parser) because
the variant names (`Ok`/`Err` vs `Some`/`None`) depend on the
inner expression's type. The synthetic `__try_t_<n>` /
`__try_v_<n>` / `__try_e_<n>` binding names are pre-interned by
the parser (each `?` instance gets a fresh `<n>` from the
parser's synthetic counter, so nested `?` calls don't collide).

Once the rewrite lands, backends only see the resulting `Block`
+ `Match` — no `Try` node survives type-checking.

```rust
fn divide(a: i64, b: i64) -> Result<i64, str> {
    if b == 0i64 {
        Result::Err("division by zero")
    } else {
        Result::Ok(a / b)
    }
}

fn pipeline(a: i64, b: i64, c: i64) -> Result<i64, str> {
    val x = divide(a, b)?     # propagates Err out of `pipeline`
    val y = divide(x, c)?     # second `?` only runs if the first succeeded
    Result::Ok(y + 1i64)
}
```

**Constraints and gotchas:**

- The inner expression must be `Result<T, E>` or `Option<T>`. Any
  other type is a type-check error.
- The enclosing function's declared return type must match what
  the `?` propagates (`Result<_, E>` for a Result `?`, `Option<_>`
  for an Option `?`). A success-type change (`read_file(p)?` of
  `Result<str, IoError>` inside a fn declared
  `-> Result<u64, IoError>`) works — the desugar reconstructs the
  error variant against the declared type. An *error*-type change
  requires `E2: From<E1>` (below). `return` itself is checked
  against the declared return type on every path, so `?` inside a
  fn returning nothing is a type error rather than a silent lie.
- `?` always introduces its own binding internally, so the
  scrutinee inside the desugar is always an identifier — the
  surrounding `match`'s scrutinee rules never come into play.
- **Cross-error conversion**: when the inner error type `E1`
  differs from the enclosing fn's `E2` and `E2: From<E1>` is
  implemented (see [From / Into](#from--into-via-the-auto-loaded-convert-module)),
  the error arm converts through `E2::from(e)`
  before re-returning; without a `From` impl the program is a
  type error.
- **Out of scope** (initial implementation): user-defined `Try`
  trait.

Backends: all three (interpreter / cranelift JIT / AOT) execute
the rewritten `match` directly.

### `??` operator (null-coalesce)

`a ?? b` yields `a`'s contained value when `a` is `Option::Some` /
`Result::Ok`, and evaluates and yields `b` otherwise. It works on
both `Option<T>` and `Result<T, E>`; the right operand must have the
success type `T`.

```rust
val port: Option<u64> = config.get("port")
val p = port ?? 8080u64

val r: Result<u64, str> = read_count()
val n = r ?? 0u64
```

The operator is **right-associative** (`a ?? b ?? c` groups as
`a ?? (b ?? c)` — the only chaining that types) and binds tighter
than the comparison operators but looser than the shift / arithmetic
operators: `a ?? b == c` groups as `(a ?? b) == c`.

Like `?`, the desugar happens in the type checker — the parser emits
`Expr::NullCoalesce`, which is rewritten into:

```text
a ?? b
# behaves like:
{
    val __coalesce_t = a
    match __coalesce_t {
        Option::Some(__coalesce_v) => __coalesce_v as T,
        Option::None => b,
    }
}
```

(analogously `Ok(v) => v` / `Err(_) => b` for `Result`). Because the
rewrite is a `match`, the default operand `b` is **lazy** — it only
evaluates on the `None` / `Err` path. This is `unwrap_or`'s result
with `unwrap_or_else`'s evaluation discipline; when the default is a
plain value the two are equivalent.

**Constraints:**

- The left operand must be `Option<T>` or `Result<T, E>`. Anything
  else (including a bare value) is a type error.
- Both arms must have the same type: `opt ?? "str"` where
  `opt: Option<u64>` is a type error. An unresolved success type
  (the lhs is a bare `Option::None`) is decided by the default
  operand.
- The synthetic binding names are `__coalesce_t_<n>` /
  `__coalesce_v_<n>` / `__coalesce_e_<n>`, pre-interned per instance
  like `?`'s.

Backends: all three (interpreter / cranelift JIT / AOT) execute the
rewritten `match` directly; the example
`interpreter/example/null_coalesce.t` is swept across all of them.

### Operator overload (struct receivers)

Same-shape struct values can overload most binary and unary
operators by implementing the matching method on the struct.
The frontend short-circuits the overload before its primitive
type-check rule, so overloaded operators don't conflict with
the primitive paths (`i64 + i64` continues to lower as a
direct `BinOp::Add`).

**Structs only.** A struct with no matching method does not get the
operator, and the type checker says which method is missing. Enums do
not overload operators at all — including `==` — so two enum values
are compared by matching on their variants; writing an `eq` in
`impl SomeEnum` does not change that, and the checker says so rather
than accepting a comparison nothing dispatches.

| Operator | Method signature | Result |
|---|---|---|
| `==` `!=` | `fn eq(&self, other: &Self) -> bool` | `bool` (`!=` negates) |
| `<` `<=` `>` `>=` | `fn lt/le/gt/ge(&self, other: &Self) -> bool` | `bool` |
| `+` `-` `*` `/` `%` | `fn add/sub/mul/div/rem(&self, other: &Self) -> Self` | `Self` |
| `+=` `-=` `*=` `/=` `%=` | (uses `add`/`sub`/`mul`/`div`/`rem` via desugar) | (mutates lhs) |
| `&=` `\|=` `^=` `<<=` `>>=` | (uses `bitand`/`bitor`/`bitxor`/`shl`/`shr` via desugar) | (mutates lhs) |
| `&` `\|` `^` `<<` `>>` | `fn bitand/bitor/bitxor/shl/shr(&self, other: &Self) -> Self` | `Self` |
| `-` (unary) `~` `!` | `fn neg/bitnot/not(&self) -> Self` | `Self` |

```rust
struct Vec3 { x: i64, y: i64, z: i64 }
impl Vec3 {
    fn add(&self, other: &Vec3) -> Vec3 {
        Vec3 { x: self.x + other.x, y: self.y + other.y, z: self.z + other.z }
    }
    fn eq(&self, other: &Vec3) -> bool { ... }
    fn neg(&self) -> Vec3 { Vec3 { x: 0i64 - self.x, y: 0i64 - self.y, z: 0i64 - self.z } }
}

var a: Vec3 = Vec3 { x: 1i64, y: 2i64, z: 3i64 }
val b: Vec3 = Vec3 { x: 10i64, y: 20i64, z: 30i64 }
a += b              # uses add (compound assign)
val c: Vec3 = a + b # uses add
val n: Vec3 = -a    # uses neg
if a == b { ... }   # uses eq
```

An overload's result is an ordinary value: it chains (`a + b + c`),
takes literal operands (`a & Bits { v: 1 }`), and stands wherever the
struct itself could — a field root (`(a + b).x`), an argument
(`take(a + b)`), a condition (`if (a + b) == c`).

**Out of scope** (deliberate):
- `&&` / `||` — short-circuit semantics make method dispatch
  unsound (the rhs would always evaluate). Both operators stay
  primitive-only.
- Enum receivers. No engine dispatches an operator method on an
  enum, so the checker rejects the comparison rather than letting
  it fail at run time. Match on the variants instead — note that a
  tuple scrutinee (`match (a, b)`) is itself outside the AOT
  subset, so nest the matches or compare a scalar tag:

  ```rust
  val same: bool = match a {
      Color::Red => match b { Color::Red => true, Color::Green => false },
      Color::Green => match b { Color::Green => true, Color::Red => false },
  }
  ```

### `Ord` and `Vec::sort` (STDLIB-ORD)

`core/std/ord.t` declares `trait Ord { fn lt(self: Self, other: Self) -> bool }`
with impls for every primitive width, `f64`, `bool`, and `String`
(byte-wise, in `core/std/string.t`). The method is named `lt` — the
same name the `<` operator overload dispatches to — so a type that
implements `impl Ord` also gets the `<` operator for free, and a
type with a hand-written `lt` already satisfies the shape. The
receiver is `self: Self` (by value) because primitives cannot be
dereferenced; the alias-based compound semantics keep the caller's
binding usable, so sort can call `lt` repeatedly.

`Vec<T>::sort()` (`core/std/collections/vec.t`) is a stable in-place
insertion sort over the bound `impl<T: Ord> Vec<T>`:

```rust
var v: Vec<u64> = Vec::new()
v.push(3u64); v.push(1u64); v.push(2u64)
v.sort()                    # [1, 2, 3]
```

Sorting works for primitives, `String`, and any user struct with
`impl Ord`. `f64` compares with the native `<`, so NaN (less than
nothing, including itself) stays put rather than ordering. Calling
`sort` on a `Vec<T>` whose `T` does not implement `Ord` is rejected
at the call site, like any other unsatisfied bound (see
[Generics and bounds](#generics-and-bounds)):

```
[E0010] Method 'sort' generic parameter 'T' bound violation:
        expected Ord, got P (struct `P` does not implement trait `Ord`)
```

### `Dict<K, V>` (stdlib)

`core/std/dict.t` provides `Dict<K, V>`, written in toylang on top of
the heap builtins the same way `Vec<T>` is — no parser, type checker,
or backend special-casing. It runs on all three backends.

```rust
var d: Dict<str, u64> = Dict::new()
d.insert("a", 1u64)
d.insert("b", 2u64)

match d.get("a") {
    Option::Some(v) => println(v),
    Option::None => println("missing"),
}

println(d.get_or("c", 0u64))      # 0 — the fallback, no match needed
println(d.contains_key("b"))      # true
println(d.remove("b"))            # true (false when the key was absent)
println(d.size())                 # 1
```

A key type needs two things: `impl Hash` (see
[`Hash` and `hash_mix`](#hash-and-hash_mix-stdlib)) and an answer for
`==`. Every integer width, `bool`, `str` and `String` have both. A
struct key needs both written by hand — there is no derive:

```rust
struct Point { x: i64, y: i64 }

impl Hash for Point {
    fn hash(self: Self) -> u64 { (self.x as u64) ^ ((self.y as u64) << 1u64) }
}

impl Point {
    fn eq(&self, other: &Point) -> bool { self.x == other.x && self.y == other.y }
}
```

A missing `Hash` is the ordinary bound violation at the call site; a
missing `eq` is reported the same way (see
[`==` on a type parameter](#-on-a-type-parameter)).

Two properties are worth knowing before building on it:

- **Lookup is a hash probe, removal is a shift.** `insert`, `get`,
  `get_or` and `contains_key` are O(1) expected: a power-of-two table
  of indices sits beside the entries and is what gets probed.
  `remove` is O(n) — it shifts the later entries down to keep the
  iteration order below intact, and rebuilds the table because the
  indices it holds have moved.
- **Iteration is in insertion order, and stays that way.** `d.iter()`
  walks the entries, not the table. An update through `insert` leaves
  the entry where it is; a removal keeps the survivors in order; a key
  re-inserted after a removal goes to the end.

Modifying a dict while iterating it is undefined: the iterator holds a
copy of the key and value buffer pointers, which a growing `insert`
can move.

### `==` on a type parameter

A generic body may compare two values of its own type parameter, with
no bound to declare:

```rust
impl<T> Bag<T> {
    fn contains(&self, needle: T) -> bool {
        val e: T = self.v.get(0u64)
        e == needle
    }
}
```

The comparison means whatever `==` means for the type argument: the
primitive comparison for a primitive, byte equality for `str`, and the
type's own `eq` method for a struct that has one (see
[Operator overloading](#operator-overloading)). There is no `Eq` trait
to implement and no `<T: Eq>` to write — the requirement comes from the
body rather than from the signature.

It is still a requirement, and it is checked where the type argument is
known — the call site:

```
[E0010] Method 'contains' generic parameter 'T' compares its values
        with `==`, but `P` has no `eq` (define
        `fn eq(&self, other: &P) -> bool` in `impl P`)
```

An enum type argument is rejected outright: comparison overloading is a
struct feature, so an `eq` written in `impl SomeEnum` would type-check
and then fail to dispatch. Match on the variants instead.

### `Hash` and `hash_mix` (stdlib)

`core/std/hash.t` declares `trait Hash { fn hash(self: Self) -> u64 }`
with impls for every integer width, `bool`, `str`, and `String` (in
`core/std/string.t`, the module that owns the type). `hash` promises
one thing: equal values hash equally. It does *not* promise a spread —
the integer impls are the identity, or a same-width cast for the
signed widths so that `-5i8` hashes as the byte it is rather than as a
sign-extended `u64`.

Spreading is a separate step, `hash_mix`:

```rust
val slot: u64 = hash_mix(key.hash()) & (cap - 1u64)
```

`hash_mix` is splitmix64's finalizer — three xor-shift-multiply rounds,
which spread every input bit across the whole word. A table applies it
before taking the low bits as a slot index, so that a hash written by
user code gets the same treatment as the built-in ones.

`str` and `String` hash with FNV-1a over their UTF-8 bytes, so the two
spellings of the same text agree:

```rust
val s: String = String::from_str("k")
s.hash() == "k".hash()          # true
```

The value is fixed, not merely consistent within a run: the
interpreter, the JIT, and a compiled binary all produce the same u64
for the same bytes, and there is no per-process seed (that would make
a hash table's iteration order differ run to run, which the
determinism rules rule out). The `str` impl runs in the runtime
(`toylang_rt::toy_str_hash`) rather than as a toylang byte walk,
because `str::as_ptr()` copies the bytes on every call in the
interpreter and a hash runs once per lookup.

### Numeric semantics

- **Integer arithmetic**: standard two's-complement. `+`, `*`, and
  signed `-` **wrap** on overflow, on every backend and in every build
  profile — the compiled binary produces the value the interpreter
  produces. This is deliberate: one arithmetic semantics, not one per
  build. Use `checked_*` / `saturating_*` (see *Overflow-aware
  arithmetic* below) where wrapping is the wrong answer.
- **Integer division and `%`**: truncated; `(-7) % 3 == -1`.
- **Arithmetic that traps rather than wrapping**: three cases stop the
  program instead of producing a value, because the value they would
  produce is a plausible-looking number that surfaces far from the
  mistake. See *Runtime traps*.
- **SIMD lanes never trap**: the arithmetic on a vector's lanes wraps
  in every case, and integer `/` and `%` on a vector are rejected at
  type-check time rather than given a per-lane guard. The asymmetry
  with the scalar rules above is deliberate — see
  [SIMD vectors](#simd-vectors).
- **Float arithmetic**: standard IEEE 754. NaN compares false against
  everything (matching Rust's `PartialOrd`). `f32` (SIMD-F32) has the
  same semantics at single precision — including `%` (`0.1f32 % 0.3f32`
  is an IEEE remainder, no trap); there are no float traps in any
  width. Mixing `f32` with `f64` (or with an integer) is a type error;
  cross-width moves go through `as`.
- **`as` casts**:
  - `i64 ↔ u64`: bit-preserving reinterpretation.
  - `f64 → i64/u64`: truncate toward zero, saturate on out-of-range,
    NaN becomes 0 (matching Rust's `as` since 1.45).
  - `i64/u64 → f64`: nearest-rounding conversion.
  - `f32 → int`: same saturating rule as `f64`, at single precision.
  - `f64 → f32`: demote (nearest-rounding); `f32 → f64`: promote
    (exact — every f32 is representable in f64).

### Control flow as expression

`if` / `match` / blocks are expressions and yield a value:

```rust
val grade: str = if score >= 80u64 {
    "A"
} elif score >= 60u64 {
    "B"
} else {
    "C"
}
```

Every branch must produce the same type (or no branch may produce a
value, in which case the expression has type `()` aka Unit).

### Range expressions

```rust
for i in 0u64..n { ... }     # half-open
val r = 0u64..10u64           # range as a value
for i in 0u64 to n { ... }    # legacy `to` form, still accepted
```

### `with` blocks

Lexically scoped allocator binding:

```rust
var arena = Arena::new()
with allocator = arena {
    # `__builtin_heap_alloc` in here reads the handle `arena` carries
}
```

See [Allocators](#allocators).

---

## SIMD vectors

toylang has five 128-bit vector types. Ordinary operators work on them
**lane-wise**, so the only intrinsics are the operations a type and an
operator cannot express.

| Type | Lanes | Lane type |
|---|---|---|
| `f64x2` | 2 | `f64` |
| `f32x4` | 4 | `f32` |
| `i32x4` | 4 | `i32` |
| `i64x2` | 2 | `i64` |
| `u8x16` | 16 | `u8` |

The width stops at 128 bits deliberately: SSE2 on x86-64 and NEON on
aarch64 both have it unconditionally, so a program that uses vectors
runs the same everywhere and an AOT binary stays portable. There is no
way to ask how wide the host's registers are — the width is always
written in the source.

```rust
fn dot2(xs: ptr, ys: ptr) -> f64 {
    val a: f64x2 = __simd_load(xs, 0u64)
    val b: f64x2 = __simd_load(ys, 0u64)
    __simd_reduce_add(a * b)
}
```

A vector is a single value, not a container: unlike a struct it is not
decomposed at a function boundary, so it can be passed and returned
directly. `__builtin_sizeof` reports 16 for every vector type.

### Lane-wise operators

| Operator | Lanes | Result |
|---|---|---|
| `+` `-` `*` | any | same vector type |
| `/` | float lanes only | same vector type |
| `%` | — | rejected |
| `&` `\|` `^` | integer lanes only | same vector type |
| `<<` `>>` | integer lanes only, **`u64` amount on the right** | same vector type |
| `==` `!=` `<` `<=` `>` `>=` | any | **mask** (see below) |
| unary `-` | signed / float lanes | same vector type |
| unary `~` | integer lanes | same vector type |
| unary `!` | — | rejected |
| `&&` `\|\|` | — | rejected (they short-circuit, which has no lane-wise meaning) |

Both sides of a lane-wise operator must be the same vector type; a
scalar is broadcast explicitly with `__simd_splat`, never implicitly.
The one exception is a shift, whose right operand is a single `u64`
applied to every lane (this is the shape SIMD hardware offers), taken
modulo the lane width.

**Integer lanes wrap and never trap.** A scalar `u64 -` panics on
underflow and a scalar `/` panics on zero (see
[Numeric semantics](#numeric-semantics)); checking sixteen lanes would
cost more than the vectorisation saves, so integer `/` and `%` are
rejected outright rather than being given a quieter failure mode. Use
a reciprocal computed in float lanes, or a scalar loop. Float `/` is
offered because IEEE division does not trap.

### Masks

A comparison answers once per lane, so it produces a **mask**: an
integer vector of the same lane width, all-ones for true and all-zeros
for false.

| Vector | Mask |
|---|---|
| `f64x2`, `i64x2` | `i64x2` |
| `f32x4`, `i32x4` | `i32x4` |
| `u8x16` | `u8x16` |

`__simd_all(a == b)` is the whole-vector equality question;
`__simd_any` is its existential twin. A mask is an ordinary vector, so
`~`, `&`, and `|` combine masks.

### Intrinsics

| Intrinsic | Signature |
|---|---|
| `__simd_splat(x)` | `E -> V` — the scalar in every lane |
| `__simd_load(p, i)` | `(ptr, u64) -> V` |
| `__simd_store(p, i, v)` | `(ptr, u64, V) -> ()` |
| `__simd_extract(v, k)` | `(V, u64) -> E` — `k` literal, in range |
| `__simd_insert(v, k, x)` | `(V, u64, E) -> V` |
| `__simd_select(mask, a, b)` | `(M, V, V) -> V` — lane-wise, branch-free |
| `__simd_reduce_add(v)` | `V -> E` |
| `__simd_reduce_min(v)` / `__simd_reduce_max(v)` | `V -> E` |
| `__simd_reduce_and(v)` / `__simd_reduce_or(v)` | `V -> E`, integer lanes only |
| `__simd_any(mask)` / `__simd_all(mask)` | `M -> bool` |

Every `__simd_*` intrinsic is pure except `__simd_load` / `__simd_store`,
which carry the same effects `__builtin_ptr_read` / `__builtin_ptr_write`
do. The pure ones are therefore callable from a `const fn`, from a
`never_allocates` body, and from a `requires` / `ensures` predicate.

**`__simd_reduce_*` folds lane 0 through lane n, in order.** That is
part of the language, not an implementation detail: a pairwise tree
would give a different `f64` sum on one backend than on another, and
a reduction appears once at the end of a loop where the sequential
form costs nothing.

**`__simd_load(p, i)` addresses by *element*.** Lane `k` reads the
bytes at `(i + k) * lane_bytes`. Note the contrast with
`__builtin_ptr_read(p, off)`, whose `off` is a byte count — so the
`Vec<T>` idiom `__builtin_ptr_read(self.data, i * self.elem_size)`
becomes `__simd_load(self.data, i)` when `T` is the lane type.

A lane index (`__simd_extract` / `__simd_insert`) must be a literal in
range. The lane is part of the instruction, not a value it reads; to
select a lane computed at run time, store the vector and read the
element back.

### Where the vector type comes from

`__simd_splat` and `__simd_load` have no lane-type suffix, so the call
alone does not say what it produces. The type is taken from context —
an annotation, or the vector on the other side of an operator:

```rust
val a: f64x2 = __simd_splat(1.5f64)   # from the annotation
val masked = v & __simd_splat(15u8)   # from `v`
```

Where no context names a vector type the program is rejected with an
error saying so; bind the call to an annotated `val` first.

### Printing

A vector prints as its type name applied to its lanes, with each lane
spelled exactly as that scalar would be on its own — including the
`.0` an integral float keeps:

```
f64x2(1.5, 9.0)
u8x16(8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8)
```

String interpolation (`"{v}"`) produces the same text. A format spec
(`"{v:.2}"`) is rejected: width, precision, and radix have no single
meaning across lanes.

### Backend coverage

Interpreter (tree-walker), IR VM, AOT, and the compiler's JIT all
support the full surface; the interpreter's own JIT takes its usual
silent fallback (its scalar type set has no vector). Example:
`interpreter/example/simd.t`.

---

## Statements

### Variable declarations

```rust
val a: i64 = 7i64       # immutable; required initializer
val b      = 7i64       # type inferred from rhs
var c: u64 = 0u64       # mutable
```

`val` produces a binding that cannot be reassigned. `var` permits later
`=` assignment.

Both forms require an initializer: a bare `var d` (no `=`) is a parse
error. There is no declare-now-assign-later shape, and no implicit
null to stand in until the first assignment.

**An assignment produces no value: its type is `()`.** This holds for
every form — `a = v`, `p.f = v`, `a[i] = v`, `d[k] = v`, a
`__setitem__` write, and the compound operators, which desugar to
`a = a OP b` at parse time. So a block ending in an assignment is a
`()` block:

```rust
match x {
    Option::Some(v) => { acc = acc + v }   # both arms are ()
    Option::None    => {}
}

fn f() -> u64 { var a = 0u64  a = 5u64 }  # error: expected u64, got ()
```

Assignment is not usable in an expression position either: `val x = (a = b)`
is a parse error and `a = b = c` does not run. `()` is what makes the
three agree.

### Top-level `const` declarations

A `const` is an immutable binding declared at file scope (alongside
`fn`, `struct`, `enum`, etc.) and visible from every function body:

```rust
const PI: f64 = 3.14159f64
const MAX_RETRIES: u64 = 3u64
const GREETING: str = "hello"

fn area(r: f64) -> f64 { PI * r * r }
```

- The type annotation is **mandatory** (no inference).
- The initializer is an arbitrary expression — including references to
  *earlier-declared* consts. Forward references are not allowed.
- Each const is evaluated **once** at program startup, before `main`,
  and the result is bound as an immutable global.
- Visibility (`pub const ...`) follows the same rules as `pub fn`.

Today the JIT silently falls back to the tree-walking interpreter for
any function that references a `const` — see [`JIT.md`](../design-docs/JIT.md).

### `const fn` — evaluation while compiling

`const fn` marks a function the compiler may run *during compilation*:

```rust
const fn double(n: u64) -> u64 { n * 2u64 }

const D: u64 = double(21u64)          # folded to 42u64 before lowering
fn main() -> u64 { double(3u64) + D } # folded to 6u64 + 42u64
```

The spelling is C++'s `constexpr`, not `consteval`: it says the
function **can** be folded, never that it must be. What forces a fold
is the position the call is in.

- **Forced positions** — a `const NAME: T = ...` initialiser whose
  expression calls anything. The value has to exist before the program
  starts, so a failure is a compile error (`E0017`): a trap, a
  `panic`, a broken `requires`, a spent step budget, or a callee that
  is not a `const fn`.
- **Everywhere else** the fold is an optimisation. A call whose
  arguments are all literals is folded when it succeeds and left alone
  when it does not, so `if false { boom(1u64) }` stays legal and keeps
  its run-time behaviour.

A fold produces a **number or a bool** — nothing else has a literal to
become. A `const fn` returning `str`, a struct, or a `Vec` type-checks
and runs, but is never folded.

The declaration itself is checked (`E0017`) by walking every path out
of the function, the same way `never_allocates` is. Refused: the
allocator and raw pointers, `print` / `println`, the allocation
counters and the allocator context, and any call that cannot be
followed — a closure value, a `dyn Trait` receiver, or an `extern fn`.
Unlike `never_allocates`, `extern` gets **no escape hatch**: an
author's word that a C function is pure still leaves the compiler with
no way to call it.

- Callees need no annotation. A `const fn` may call any function whose
  own reachable set is clean, so the stdlib needs no `const fn` pass.
- `panic` and `assert` are allowed on purpose. Reaching one in a
  forced fold is reported while compiling, which beats the same call
  certainly aborting at run time.
- Free functions only for now; methods are not yet declarable
  `const fn`.
- Contextual with the declaration form: `const` followed by `fn` is
  the modifier, `const` followed by a name is a binding. It combines
  with `never_allocates` in either order.
- The evaluator is the IR VM (`compiler_vm`) — the same engine the
  run-time fast path executes — so a folded value is by construction
  the value the program would have computed: same lowering, same trap
  guards, same wrapping `+` / `*`, truncated signed division, and
  narrow-width widths.

An array length may name a `const`:

```rust
const N: u64 = 3u64
val a: [i64; N] = [1i64, 2i64, 3i64]
```

It may also be an expression the compiler can compute: a call to a
`const fn` with literal arguments, or arithmetic over literals and
`const`s. The compiler folds the call at compile time — the same fold
that evaluates `const` initialisers — and bakes the count in before
any backend sees it, so the computed spelling and the literal it
resolves to behave identically everywhere:

```rust
const fn double(n: u64) -> u64 { n * 2u64 }
val a: [i64; double(2u64) + 1u64] = [1i64, 2i64, 3i64, 4i64, 5i64]  # [i64; 5]
const N: u64 = 2u64
val b: [i64; N + 1u64] = [1i64, 2i64, 3i64]                        # [i64; 3]
```

A length the compiler cannot fold is refused with the reason: a call
to a function that is not declared `const fn`, or a `const fn` call
whose arguments are not all literals (`[i64; double(N)]` — the fold
only runs calls it can prove constant). A length that merely names a
`const` must be declared earlier in the file, and that const's own
initialiser must be a literal (or another such const) — the parser
resolves those at parse time, before the fold can run. One thing a
computed length does not do: the element count is not cross-checked
against the array literal that initialises it (a literal length is).

Example: `interpreter/example/const_fn.t`.

### Top-level `type` declarations

`type Name = TargetType` declares a top-level type alias. See
[Type aliases](#type-aliases) above for the full semantics
(per-file vs cross-module resolution, generic alias arity, alias
chains, forward references).

```rust
type Byte = u32                 # non-generic
type Pair<T> = Box<T>           # generic
pub type ApiVersion = u32       # `pub` is parsed but not yet
                                # enforced at module boundaries
```

#### Tuple destructuring

```rust
val (a, b)        = make_pair()
val ((x, y), z)   = nested_call()      # nested patterns work
var (sum, count)  = (0u64, 0u64)
```

The parser desugars tuple destructuring into a hidden temporary plus
per-name `tmp.0`, `tmp.1`, … bindings. Outer `val` / `var` propagates
to leaf bindings only.

### Control flow

```rust
if cond { ... } elif cond { ... } else { ... }
for i in start..end { ... }
for x in iter { ... }        # iterator protocol (see below)
while cond { ... }
loop { ... }                 # infinite loop (desugars to while true)
break
continue
return                       # returns Unit
return value                 # returns a value
```

#### `for` loop forms

Three shapes share the `for IDENT in EXPR { body }` syntax. The
parser picks the desugaring based on what follows `EXPR`:

Throughout the header — the iterable, and both ends of a range —
**struct literals are not recognised**, exactly as in an `if` or
`while` condition. That is what lets `for x in MyIter { ... }` read
`{` as the loop body rather than as `MyIter {}`, and it costs nothing:
a range bound is an integer, so a literal could never be valid there.
Bind the struct first if you need one (`val it = MyIter { .. }`).

1. **Integer range, `..` form** — `for i in 0i64..10i64 { ... }`.
   Bare `start..end` produces a fast-path `Stmt::For`; the body
   sees `i` typed as the range's element type. **Any integer type
   drives a range**, narrow widths included (`for i in -3i32..2i32`,
   `for b in 0u8..255u8`) — both ends must be the same type, since
   integers never convert implicitly ([NUM-W](#integer-literals)).
2. **Integer range, `to` form** — `for i in 0i64 to 10i64 { ... }`.
   Legacy spelling, semantically identical to `..`.
3. **Iterator protocol** — `for x in EXPR { body }` where EXPR is
   any value whose type exposes `fn next(&mut self) -> Option<T>`.
   The parser desugars at parse time:

       for x in EXPR { body }
     ⇒ {
           var __iter_for_<n> = EXPR
           while true {
               match __iter_for_<n>.next() {
                   Option::Some(x) => { body; continue },
                   Option::None    => { break },
               }
           }
       }

   The trailing `continue` after `body` exists purely to unify the
   match arm types at `Unit`, so the user's body may end in any
   expression — `for x in it { x + 1u64 }` has a `u64` that would
   otherwise clash with the `None` arm's `break`.

   The protocol is **structural**, not nominal: there is no
   `trait Iterator<T>` declaration to implement, because generic
   trait declarations (`trait Foo<T>`) are not yet supported. Any
   struct providing the right `next` shape participates. See
   `core/std/iter.t` for the full contract.

   ```rust
   struct Counter { current: i64, end: i64 }
   impl Counter {
       fn new(end: i64) -> Self { Counter { current: 0i64, end: end } }
       fn next(&mut self) -> Option<i64> {
           if self.current >= self.end {
               Option::None
           } else {
               val v = self.current
               self.current = self.current + 1i64
               Option::Some(v)
           }
       }
   }

   var sum = 0i64
   var iter = Counter::new(5i64)
   for x in iter { sum = sum + x }   # sum == 10
   ```

   **Stdlib adapters** (STDLIB-ITER-ADAPT, `core/std/collections/vec.t`):
   `VecIter<T>` (returned by `v.iter()`) composes through ordinary
   adapter structs that expose the same `next(&mut self) ->
   Option<T>` protocol:

   ```rust
   var v: Vec<u64> = Vec::new()
   v.push(1u64); v.push(2u64); v.push(3u64); v.push(4u64)

   var m = v.iter().map(fn(x: u64) -> u64 { x * 2u64 })    # 2, 4, 6, 8
   for x in m { println(x) }

   var f = v.iter().filter(fn(x: u64) -> bool { x % 2u64 == 0u64 })   # 2, 4
   for x in f { println(x) }

   var e = v.iter().enumerate()                            # (0, 1) (1, 2) ...
   for kv in e { println(kv.0 * kv.1) }

   var z = v.iter().zip(v.iter())                          # (1, 1) (2, 2) ...
   for p in z { println(p.0 + p.1) }

   var c = v.iter().map(fn(x: u64) -> u64 { x + 10u64 }).collect()   # Vec<u64>
   ```

   `collect` takes the iterator **by value** (`self: Self`) and
   drains it into a fresh `Vec`. Because compound values alias in
   toylang, the caller's iterator binding keeps its state afterwards
   — a second `collect()` call starts again from the beginning.
   `collect` is provided on `VecIter` / `MapIter` / `FilterIter`
   only: `Vec<(A, B)>` (from `zip` / `enumerate`) needs
   `__builtin_sizeof` on a tuple value, which the AOT backend
   cannot resolve yet. The adapters work on all three backends
   (interpreter / AOT / JIT).

   `DictIter<K, V>` (`d.iter()`) gains `map` / `filter` in
   `core/std/dict.t`; `StringIter` (`s.iter()`) gains `map` /
   `filter` / `enumerate` / `collect` in `core/std/string.t`. Two
   backend constraints shape the Dict adapters: the closure receives
   the key and value as **separate scalar arguments**
   (`fn (K, V) -> U`) because an AOT closure cannot take a tuple
   parameter, and the iterator state is kept flat inside the adapter
   (no nested `DictIter`) with `count` packed into `index`'s high 32
   bits, staying within the 8-return register budget.

   `break` / `continue` / `return` inside the body propagate
   through the desugared `match` and `while` to the expected target
   (the enclosing for-loop, the next iteration, or the surrounding
   function respectively).

   **Backend coverage**: interpreter, AOT compiler, and the
   cranelift JIT all run the protocol end-to-end. Range-based
   `for` loops keep the dedicated AOT fast path. The desugaring
   skips the synthetic `var __iter_for_<n> = EXPR` temporary when
   EXPR is already a bare identifier, so the loop advances the
   user's own iterator; `&mut self`
   writeback through `iter.next()` mutates the user's original
   binding correctly between iterations.

By default, `break` / `continue` apply to the innermost enclosing
loop. **Labelled loops** (LABEL feature) let you target an outer
loop directly:

- `@label: while cond { ... }` / `@label: loop { ... }` /
  `@label: for i in 0..N { ... }` / `@label: for x in iter { ... }` — declare a name for the loop.
  The label uses the `@` prefix (toylang reserves `@` only for
  this) followed by an identifier and a `:`.
- `break @label` — exit the named loop, possibly skipping over
  one or more inner loops.
- `continue @label` — skip to the next iteration of the named
  loop (inner loops in between are abandoned).
- Bare `break` / `continue` keep the innermost-loop semantics.

```rust
fn search() -> i64 {
    @outer: for i in 0i64 to 10i64 {
        for j in 0i64 to 10i64 {
            if i * j == 42i64 {
                return i * 100i64 + j
            }
            if j > 5i64 { continue @outer }   # skip rest of inner, advance i
        }
    }
    -1i64
}
```

The type checker validates that every `break` / `continue`
references a label that's actually in scope (or is inside *some*
loop, for the bare form). Forgetting to wrap a `break` in a loop
or referencing a misspelled label both fail at type-check time
rather than at runtime. Labels do not propagate across function
boundaries — a closure / nested function cannot reference a
label declared by its enclosing function.

For `@label: for x in iter { ... }` (iterator-protocol form), the
parser desugars to a synthetic `while true { match iter.next()
{ Some(x) => body, None => break } }` and propagates the label
to that synthetic while, so user-written `break @label` inside
the body resolves to the correct loop.

### `if val` / `while val`

Pattern-binding conditional and loop. Toylang uses `val` (not `let`)
as its immutable-binding keyword, so the construct is `if val` /
`while val` rather than Rust's `if let` / `while let` — same
shape, same semantics, consistent vocabulary:

```rust
# unwrap an Option succinctly
if val Option::Some(x) = opt {
    println("got {x}")
} else {
    println("none")
}

# drain an iterator-style enum until it returns None
while val Option::Some(item) = it.next() {
    process(item)
}

# user-defined enum variant
enum Shape { Circle(i64), Square(i64), Point }
val s: Shape = Shape::Circle(5i64)
val area: i64 = if val Shape::Circle(r) = s { r * r * 3i64 } else { 0i64 }
```

The construct is **pure parser-level desugar** — it lowers to a
two-arm `match` (and, for `while val`, an outer `while true`):

| Surface | Desugars to |
|---|---|
| `if val PAT = EXPR { THEN } else { ELSE }` | `match EXPR { PAT => THEN, _ => ELSE }` |
| `if val PAT = EXPR { THEN }` (no else) | same, with the else-arm Unit-yielding so the construct sits at statement position |
| `while val PAT = EXPR { BODY }` | `while true { match EXPR { PAT => { BODY; continue }, _ => break } }` |

Because the desugar produces only existing AST shapes, **all three
backends** (interpreter / AOT / cranelift JIT) handle it without
further changes. Labelled forms (`@label: while val ...`) propagate
the label to the synthetic `while true` so user-written
`break @label` from inside the body still escapes correctly.

**Pattern surface**: any pattern that `match` accepts — enum
variants (with sub-patterns, guards), literals, wildcard, and
nested forms. `if var PAT = ...` (mutable binding) is not yet
supported; bindings introduced by `if val` follow the standard
match-arm immutability rule.

**AOT note**: as with any `match`, the scrutinee must be an
enum-typed binding, a call returning an enum (method or free
function, including generic ones), or a scalar primitive. All
three backends agree on each of those.

### Match

See [Enums and pattern matching](#enums-and-pattern-matching).

---

## Functions

### Declaration

```rust
fn divide(a: i64, b: i64) -> i64 {
    a / b
}
```

- Return type is required. A function that produces no value either
  omits the `->` clause or writes `-> ()` — both mean Unit, and `()`
  written as a type *is* the unit type rather than a zero-element
  tuple.
- Parameters require explicit types.
- The last expression in the body is the return value (no implicit
  `return` statement needed).
- A `return` value is checked against the declared return type on
  every path — returning the wrong type is a type error, and
  returning a value from a Unit function is rejected. A `return`
  **diverges**: the block it sits in produces no value, so an early
  return inside a `match` arm does not have to agree with its
  siblings (`Option::None => { return Option::None }` beside
  `Option::Some(v) => { acc = v }` is fine). `?` inside a
  function propagates by returning, so it requires a `Result` / 
  `Option` return type to exist; see the [`?` operator](#-operator-early-return).

### Generic parameters and bounds

```rust
fn identity<T>(x: T) -> T { x }
```

Generic parameters appear in `<...>` after the function name. Bounds
use the `<T: Bound>` syntax and **are enforced**: at every call site the
type checker verifies that the inferred type argument implements the
named trait, and inside the body the bounded parameter may call that
trait's methods. See [Generics and bounds](#generics-and-bounds) and
[Trait bounds on generics](#trait-bounds-on-generics).

Functions that need to allocate read the active allocator off the
runtime stack — callers wrap the call in `with allocator = ... { ... }`
and the body's `__builtin_heap_alloc` (and related builtins) routes
through that allocator automatically. No allocator parameter on the
function signature is needed.

### Visibility

```rust
pub fn add(a: u64, b: u64) -> u64 { ... }   # exported from a module
fn helper() -> u64 { ... }                  # private (default)
```

### `unsafe fn` — raw memory access

A function whose own body reads or writes raw memory must say so in
its declaration:

```rust
unsafe fn load(p: ptr) -> u64 {
    val v: u64 = __builtin_ptr_read(p, 0u64)
    v
}
```

Without the modifier the type checker refuses the body:

```text
[E0024] `__builtin_ptr_read` performs a raw memory access, so `load`
        must be declared `unsafe fn load(...)` — or go through the
        stdlib's `Ptr<T>` / `Span<T>`, which concentrate the raw
        access behind a typed API
```

**What requires it.** The builtins that touch what a pointer points
at: `__builtin_ptr_read` / `__builtin_ptr_write`, the `mem_*` family
(`mem_copy` / `mem_move` / `mem_set`), `__builtin_str_from_bytes`,
`__builtin_record_allocator_layout`, and `__simd_load` /
`__simd_store`. Producing and comparing addresses does **not**:
`__builtin_ptr_offset`, `__builtin_ptr_eq`, `__builtin_ptr_is_null`,
`__builtin_null_ptr` and `__builtin_str_to_ptr` are ordinary safe
calls, as are `__builtin_heap_alloc` / `heap_free` / `heap_realloc`
(they are the allocator's business, tracked as `alloc` / `free` in
[effects](../design-docs/EFFECT_SYSTEM.md)).

**The check is direct.** A function is asked only what its own
statements do, never what its callees do — calling an `unsafe fn`
does not make the caller unsafe:

```rust
unsafe fn poke(p: ptr, v: u64) -> () { __builtin_ptr_write(p, 0u64, v) }

fn main() -> u64 {          # safe: the raw write is not in this body
    val p: ptr = __builtin_heap_alloc(8u64)
    poke(p, 7u64)
    0u64
}
```

That is what lets the stdlib concentrate the raw builtins:
`Vec<T>::push`, `String::push`, `Ptr<T>::get` and
`Span<T>::set` carry the declaration inside `core/std/*.t`, and code
built on them needs none of its own.

The modifier applies to free functions, `impl` methods, and trait
method **default bodies** (a signature without a body has nothing to
check, but a default body is inherited by every impl that omits the
method, so the declaration travels with it). On an `extern fn` it is
accepted as a declaration and not checked — the implementation lives
outside the language. Ordering with the other prefix modifiers is
free: `never_allocates unsafe fn`, `unsafe const fn`.

`unsafe` is a **declaration, not a permission system**: it does not
unlock anything the language otherwise refuses, and it is not
transitive. It marks the bodies where the raw-pointer dialect is
actually written.

### `extern fn` declarations

`extern fn name(params) -> ret` declares a function whose body is
provided by the runtime / linker rather than the source program.
The declaration carries the signature only — no body block, no
contract clauses.

```rust
extern fn __extern_sin_f64(x: f64) -> f64
extern fn __extern_pow_f64(base: f64, exp: f64) -> f64
```

Generic parameters are accepted on `extern fn`:

```rust
extern fn __extern_test_identity<T>(x: T) -> T
```

The interpreter dispatches generic externs through the same
type-erased `extern_registry` keyed by literal name, so a single
Rust closure satisfies every `T` the type-checker accepts. The JIT
and AOT compiler don't yet name-mangle monomorph instances, so a
generic extern call falls back to the interpreter (JIT) or fails
to resolve at link time (AOT) until each backend grows
per-instance dispatch.

### Linking to real C libraries

FFI_PLAN P1: an `extern fn` can name its own library and symbol,
making it a real C ABI call instead of a backend-registered one:

```rust
extern fn add(a: i64, b: i64) -> i64 from "mylib"          # symbol = add
extern fn my_sin(x: f64) -> f64 from "m" as "sin"          # renamed symbol
```

- `from "lib"` is the linker `-l` name (no `lib` prefix /
  extension): the AOT link gets `-l<lib>`, the JIT dlopens it, and
  the interpreter loads it through `libloading` (search order:
  `TOYLANG_LINK_PATHS` directories, then the loader defaults). The
  special name `"c"` resolves to the already-linked libc.
- `as "sym"` renames the symbol; without it the function's own name
  is the symbol.
- **Types**: only scalars cross the boundary — ints of every width
  (narrow ints ride the integer register class), `f64`, `bool`,
  `ptr`, `usize`, and a `()` return. `str` and compound types are
  rejected by the type checker; pass `__builtin_str_to_ptr(s)` as a
  `ptr` instead. At most 4 arguments. The declaration's signature
  must match the C function — correctness is the caller's
  responsibility, and nothing checks it (an `extern fn` may be
  written `unsafe extern fn` as a declaration, but there is no body
  to walk).
- `from "toylang_rt"` names the language's own runtime crate: its
  symbols marshal internally (e.g. `str` handles), so the scalar
  restriction does not apply to them. `core/std/io.t` uses this for
  the argv / env / file helpers, and declares `getchar` / `time`
  `from "c"` with the rest of the I/O written in toylang.

Each backend resolves the call differently:

- **interpreter** looks the source-level name up in
  `evaluation::extern_math::build_default_registry` (a
  `HashMap<&str, fn(&[Value]) -> Result<Value, _>>`) and
  invokes the matching Rust closure. A `from`-declared extern the
  registry does not serve goes through `evaluation::extern_ffi`
  (dlopen + a trampoline over the arg / return register classes).
  The interpreter deliberately does *not* dlopen libc — its
  `extern_io` registry serves the libc names with std-based
  implementations (see RUNTIME-PORT R2).
- **JIT** routes through `jit::eligibility::JIT_EXTERN_DISPATCH`,
  which maps each name to either a runtime `HelperKind` (for
  ops cranelift can't lower natively, like `sin` / `cos` / `pow`)
  or a native cranelift instruction (`sqrt` / `floor` / `ceil` /
  `fabs`). A `from`-declared extern resolves through a
  symbol-lookup closure that dlopens the declared libraries.
- **AOT compiler** declares the function as `Linkage::Import` with
  the declared symbol (or the libm symbol name returned by
  `lower::program::libm_import_name_for` for `__extern_sin_f64 ->
  sin`-style registry externs) and passes `-l<lib>` to the link.

Bare-name calls into `extern fn`s are always allowed regardless of
import / namespace context — they're runtime bindings, not
user-visible symbols. The stdlib uses this to expose the math
intrinsics through `core/std/math.t`'s `pub fn` wrappers.

Calling an `extern fn` whose name isn't registered with any
backend produces a clean `extern fn '<name>' is not yet
implemented` error rather than crashing.

### Calling convention

Arguments are evaluated left-to-right. All values are passed by
`Rc<RefCell<Object>>` reference at runtime — the language has no
explicit reference / pointer to a binding (`ptr` is for raw heap
addresses, not for taking the address of a local).

### Design-by-Contract clauses

`requires` (preconditions) and `ensures` (postconditions) follow the
return type. See [Design by Contract](#design-by-contract).

---

## Closures

Anonymous function values use the same `fn` keyword as top-level
declarations, distinguished by the next token: `fn(` is a closure
literal; `fn name(` is a top-level function or method.

```rust
val add_two = fn(x: i64) -> i64 { x + 2i64 }
val r = add_two(40i64)        # 42
```

Closures are first-class values: assignable to `val` / `var`,
passable to higher-order functions through a function-type
annotation, and returnable from functions.

### Function type syntax

A function value's type is written as either:

- `fn (T1, T2, ...) -> R` — preferred; the leading `fn` keyword
  makes the intent explicit and lines up visually with closure
  literals (`fn(x: T) -> R { body }`).
- `(T1, T2, ...) -> R` — bare form, kept for backwards
  compatibility. May feel ambiguous next to a tuple literal,
  but the parser distinguishes the two by looking for `->` after
  the matching `)`.

Use either on parameter / return type positions:

```rust
fn apply(f: fn (i64) -> i64, x: i64) -> i64 { f(x) }

fn make_adder(n: i64) -> fn (i64) -> i64 {
    fn(x: i64) -> i64 { x + n }
}

val add: fn (i64, i64) -> i64 = fn(a: i64, b: i64) -> i64 { a + b }
val zero: fn () -> i64 = fn() -> i64 { 0i64 }
```

The empty-parameter form `fn () -> R` (and `() -> R`) is valid
(zero-arg function value).

### Captures

Free variables in the body are captured when the closure is
created. *How* they are captured depends on whether the closure can
outlive them.

**A closure that is only called where it is defined shares them.**
Reads see the current value and writes reach the outer binding, so a
counter can live inside a closure:

```rust
var count: u64 = 0u64
val bump = fn() -> u64 { count = count + 1u64  count }
bump()                        # 1
bump()                        # 2
count                         # 2 — the closure wrote to this binding
```

```rust
var n: i64 = 10i64
val add_n = fn(x: i64) -> i64 { x + n }
n = 100i64
add_n(32i64)                  # 132 — the read is live
```

**A closure that can outlive them takes a copy.** The frame that owns
the bindings may be gone by the time such a closure runs, so it
carries their values instead of their storage:

```rust
fn run(g: fn (i64) -> i64, v: i64) -> i64 { g(v) }

var n: i64 = 10i64
val add_n = fn(x: i64) -> i64 { x + n }
n = 100i64
run(add_n, 32i64)             # 42 — built from the value `n` had
```

Writing to a copy would reach nothing, so it is rejected
(`[E0021]`, `--explain E0021`). This applies to writing *through* a
capture too (`p.x = ...`, `a[i] = ...`).

A closure escapes by being used as a value rather than called:
returned, passed to a function, stored in a struct, or called from
inside another closure. The judgement is syntactic and deliberately
blunt — being wrong that way costs a diagnostic, being wrong the
other way would read a frame that is gone.

A shared capture resolves in the scope the closure was *written* in,
not the one it is called from. A binding declared between the literal
and the call does not come between them:

```rust
var n: u64 = 1u64
val f = fn() -> u64 { n }
if true {
    val n: u64 = 99u64
    f() + n                   # 100 — `f` reads the outer `n`, not this one
}
```

*Which shapes* may be captured depends on the backend. The interpreter
captures a binding of any shape; the compiled backends carry only
primitive scalars in a closure env and refuse anything else by name:

```
compiler MVP: capturing closure cannot capture `p` — only primitive
scalars fit a closure env yet, and this binding is a compound
```

So a closure that captures a struct, tuple, array or dict runs in the
interpreter but cannot be AOT-compiled.

Example: `interpreter/example/closure_counter.t`.

### Type-checker rules

- Closure parameters must carry an explicit type annotation; the
  return type is optional and inferred from the body when omitted.
- Capturing a value whose type mentions an enclosing function's
  generic parameter (`<T>`) is rejected — *generic-parameterised
  closures are not yet supported*. Concrete captures are fine.
- Calling through a function-typed value produces the value type's
  `R`. Argument count and per-position compatibility are enforced
  at the call site exactly as for direct calls.

### Backend coverage

- **Interpreter** — full support: literals, captures, HOF
  arguments, closure return values, nested closures.
- **JIT** — silently falls back to the interpreter when a
  function would need to lower a closure. The `INTERPRETER_JIT=1`
  verbose log surfaces the precise reason ("JIT does not yet
  support closure / lambda values").
- **AOT compiler** — supports `val name = fn(...) -> R { body }`
  direct calls, closures passed to higher-order functions
  (`fn (T1, T2) -> R` parameter types), closures returned
  from functions, and closures stored in struct fields
  (called via `obj.field(args)`). All four shapes work for
  capturing **and** non-capturing closures. Every closure
  value is an env-pointer (`Type::U64`) into a heap-allocated
  env tuple `[fn_ptr, cap0, cap1, ...]` — non-capturing
  closures still get an env containing just the fn-pointer.
  The lifted body's first IR parameter is the env, captures
  are read from known offsets, and `InstKind::CallIndirect`
  recovers the fn-pointer from `env+0` and prepends env to
  the user-visible args. Captures support both 8-byte scalars
  (i64 / u64 / f64 / bool) and narrow ints (u8 / u16 / u32 /
  i8 / i16 / i32). A *shared* capture puts the address of the
  outer local in the env slot instead of its value, and the
  body binds it as a borrow — the same machinery a `&mut`
  argument uses, so reads and writes go through the pointer. A
  closure written inside a sharing closure works too: it is
  handed the same pointer when it shares, and reads through it
  when it takes a copy. A capture of any other shape (struct,
  tuple, array, dict) is refused by name rather than lowered —
  see *Captures* above.
  Stdlib HOF methods on generic enums
  (`Option::map` / `Result::map` / `map_err` /
  `unwrap_or_else`) work on every backend. The remaining gap
  is the same shape on a *user-defined* generic enum, where
  the impl-level parameter reaches the compiler unresolved.

---

## Structs and methods

### Declaration

```rust
struct Point {
    x: i64,
    y: i64,
}

impl Point {
    # Associated function (no self) — call as `Point::new(...)`
    fn new(x: i64, y: i64) -> Self {
        Point { x: x, y: y }
    }

    # Method (takes `self: Self`) — call as `p.distance_sq()`
    fn distance_sq(self: Self) -> i64 {
        self.x * self.x + self.y * self.y
    }
}
```

A field's type may be any type that has a definition: a primitive, a
tuple, an array, another struct, a function type, or an **enum** —
generic ones (`value: Option<i64>`) included. Declaration order does
not matter; a field can name a type declared further down the file, or
one that comes from an auto-loaded module.

> **Backend coverage.** An enum-typed field works on every backend. It
> occupies a tag slot plus one payload slot per variant element — the
> same storage a whole enum value gets — so it can be built by a
> literal, read back, matched on, assigned to as a whole, and passed
> across a function boundary.

### Tuple structs

A struct may declare its fields **by position** instead of by name.
This is the form to reach for when the wrapper *is* the point — a unit,
an id, a newtype over a primitive — and inventing a field name would
only add noise.

```rust
struct Meters(i64)
struct Seconds(i64)
struct Sample(i64, str)
struct Wrap<T>(T)

fn speed(distance: Meters, elapsed: Seconds) -> i64 {
    distance.0 / elapsed.0
}

val m = Meters(120i64)
val raw = m.0                       # read by position
println(m)                          # ⇒ Meters(120)
```

`Meters` and `Seconds` are **distinct types**, so a function that takes
one cannot silently be handed the other even though both wrap an
`i64`. That separation is the reason to write the form at all.

The declaration is sugar: fields are named by their index (`"0"`,
`"1"`, ...), and the two sugared uses are rewritten before lowering —
`Meters(v)` to the struct literal `Meters { 0: v }`, and `m.0` to a
field access. Everything else follows from that, with no extra rules:

- `impl` blocks, `&self` / `self: Self` receivers, `Self` returns, and
  associated functions all work as they do for a named struct.
- Generic tuple structs (`Wrap<T>`) behave like generic named structs,
  including needing an explicit annotation
  (`val w: Wrap<i64> = Wrap(9i64)`) where a named one would.
- Trait impls, `Drop`, and ownership transfer are unchanged.
- All three backends are supported, because none of them sees the
  sugar.

Fields may carry `pub` individually (`struct Meters(pub i64)`).

A **function of the same name wins**: with `fn Meters(v: i64) -> i64`
in scope, `Meters(21i64)` calls the function. The struct form is only
reached through a name that is not otherwise callable.

Two forms are rejected:

- `struct Empty()` — no arity to index; write `struct Empty {}`.
- Positional access on a named struct (`p.0` where `Point` has `x`),
  and index access past the declared arity.

They also destructure in `match` — see
[Tuple-struct patterns](#tuple-struct-patterns).

### Field access and assignment

```rust
val p = Point { x: 3i64, y: 4i64 }
val x = p.x                         # read
var q = p
q.x = 5i64                          # write to a `var`
```

### Struct update

A struct literal may end with `..base`, which fills every field the
literal does not write from `base`:

```rust
val a = Point { x: 1i64, y: 2i64 }
val b = Point { y: 20i64, ..a }     # x: 1, y: 20
val c = Point { ..a }               # a plain copy
```

- `base` must be a value of **the same struct**. A different struct is
  a type error naming both, even when the fields would have lined up.
- `..base` is the **last** item in the literal; a field after it is a
  parse error.
- The result is a **new value**, not a second name for `base` —
  writing through it does not reach `base`. (A plain `val q = p`
  between compound values *is* an alias; this is the form that copies.)
- A field whose type is itself a struct, a tuple, or an enum is
  carried over whole.
- `base` may be any expression, and is evaluated **once** however many
  fields it fills. It is evaluated **before** the written field values.
- Positional fields cannot be written explicitly (a field name has to
  lex as an identifier), so on a tuple struct the form only copies:
  `Pair { ..p }`.

The omitted names come from the declaration, so the rewrite happens in
the type checker rather than the parser — `Point` may be declared
further down the file, or imported. Backends only ever see the
equivalent hand-written literal.

> **Backend coverage.** Every form works on all three backends. A base
> that is a name or a field path (`..a`, `..self`, `..o.inner`) needs
> no temporary at all — re-reading a path is free, so the literal is
> the whole rewrite. Any other base (`..make_config()`) keeps one, and
> a binding whose right-hand side is a block is bound the same way an
> `if` chain or a `match` is.

Example: `interpreter/example/struct_update.t`.

### Generic structs

```rust
struct Container<T> {
    value: T,
}

impl Container<T> {
    fn new(v: T) -> Self {
        Container { value: v }
    }
    fn get(self: Self) -> T {
        self.value
    }
}
```

The type parameter list on `impl` is implicit — `impl Container<T>`
re-uses the parameter declared on `struct`. `impl<T> Container<T>` is
equivalent and does the same thing explicitly.

A name counts as a parameter only when the type's declaration lists
it, which is what keeps `impl Vec<u8>` a concrete-argument impl (see
*Concrete-args impl dispatch*) rather than an impl over a type
parameter named `u8`. The implicit form therefore requires the
`struct` / `enum` to be declared **before** the `impl`; the explicit
form has no such ordering requirement.

### `__getitem__` / `__setitem__`

A struct can opt into bracket syntax by implementing the magic methods:

```rust
impl Bag {
    fn __getitem__(self: Self, k: str) -> i64 { ... }
    fn __setitem__(self: Self, k: str, v: i64) { ... }
}

bag["x"]            # calls __getitem__
bag["x"] = 1i64     # calls __setitem__
```

The receiver may also be written in the short form — `&self` /
`&mut self` occupy no parameter slot, so the key / value are the
first two *user* parameters in either spelling (POINTER P2). On a
generic struct the declared return type is substituted against the
receiver's type arguments, so `s[0u64]` on a `Slot<u64>` is a `u64`.
On the compiled lanes a bracket access on a struct binding lowers as
the same method call (`monomorphisation`, contracts, and `&mut self`
writeback included); a compound-returning `__getitem__` still has to
be bound with `val` first.

### `drop`

A struct can declare a `drop(&mut self)` method that runs at
end-of-scope. The destructor mechanism is the same one the allocator
system uses for arena cleanup, and the signature matches the stdlib
`Drop` trait in `core/std/drop.t`.

---

## Traits

A `trait` declares a set of method signatures that conforming structs
must provide. Trait method declarations are signature-only by default;
they record contracts only.

```rust
trait Greet {
    fn greet(self: Self) -> str
}
```

A signature may also carry `requires` / `ensures` clauses, which apply
to every `impl` of the trait — see
[Design by Contract](#design-by-contract).

A trait method can optionally carry a `{ ... }` **default body**;
impls that omit the method inherit that body as an ordinary inherent
method. See *Default method bodies* below.

### Implementing a trait

Use `impl <Trait> for <Struct> { ... }` to provide the bodies. Every
trait method must appear with a matching signature; extra methods are
allowed and become inherent methods on the struct as well.

```rust
struct Dog { name: str }

impl Greet for Dog {
    fn greet(self: Self) -> str { "Woof!" }
}
```

`Self` in a trait signature resolves to the implementing struct, so a
trait method declared as `fn m(self: Self) -> Self` is satisfied by an
impl method written the same way.

### Default method bodies

A trait method can carry a `{ ... }` body. The body becomes the
**default implementation**: any `impl <Trait> for <T>` that omits
this method inherits the default automatically, and any impl that
*does* provide a body for it overrides the default.

```rust
trait Num {
    fn value(self: Self) -> i64
    fn doubled(self: Self) -> i64 { self.value() + self.value() }
}

struct Cell { v: i64 }

impl Num for Cell {
    fn value(self: Self) -> i64 { self.v }
    # `doubled` is inherited from the trait default as-is
}

fn main() -> i64 {
    val c = Cell { v: 7i64 }
    c.doubled()    # => 14
}
```

Notes:

- The default body sees `Self` as the impl's target struct, so it
  may call other trait methods on `self` (`self.value()` above).
  Defaults that call other defaults work too — after expansion they
  are all inherent methods on the same struct.
- Providing the method in the impl shadows the default with no
  diagnostic — there's no `override` keyword and no warning.
- Defaults compose with the bounded-generic dispatch path: a
  generic `fn f<T: Num>(x: T) -> i64 { x.doubled() }` resolves
  through the inherited default the same way it would through a
  user-written body.

How it works internally: a pre-pass
(`frontend::type_checker::expand_trait_defaults_in_pool`) walks the
parsed AST once before type-checking and **rewrites each
`Stmt::ImplBlock { trait_name: Some(_), methods }`** so that every
trait method the impl omitted is appended to `methods` as a
synthesized `MethodFunction` whose `code` reuses the trait's
default-body `StmtRef`. Downstream type-checking and every backend
(interpreter / cranelift JIT / AOT compiler) therefore see the
synthesized methods exactly like user-written ones — no backend
needs a separate dispatch path for defaults. The 3-way
`assert_consistent` tests in `compiler/tests/consistency.rs`
(`trait_default_body_*`) pin this contract.

Current limitations:

- A default body on a generic trait (`trait Iterator<T> { fn count(self) -> u64 { ... } }`)
  that references the trait's type parameter `T` is **not yet
  supported**: substituting `T → <concrete>` at expansion time is
  pending A2+. Defaults that do not reference `T` work even on
  generic traits.
- No `super` / `Trait::default_method` syntax for invoking the
  default from within an override.

### Extension traits over primitives

`impl <Trait> for <PrimitiveType> { ... }` is allowed for every
primitive built into the language: `i64`, `u64`, the six narrow ints
(`u8` / `u16` / `u32` / `i8` / `i16` / `i32`), `f64`, `f32`, `bool`,
`str`, `ptr` and `usize`. The narrow widths and `f32` parsed and
type-checked but were unreachable until 2026-08-31 — the
receiver-type-to-target-name mapping had four copies and each one that
omitted a width disabled it silently. The impl methods become callable through
the regular `value.method(args)` syntax — there's no special
machinery for primitive receivers; they participate in the same
`method_registry` dispatch struct methods use.

```rust
trait Negate {
    fn neg(self: Self) -> Self
}

impl Negate for i64 {
    fn neg(self: Self) -> Self {
        0i64 - self
    }
}

fn main() -> u64 {
    val a: i64 = 7i64
    val b: i64 = a.neg().neg()    # interpreter / JIT / AOT all work
    b as u64
}
```

`Self` inside a primitive impl resolves to the matching primitive
`TypeDecl` (`Self == i64` for `impl Negate for i64`), so method
bodies can take and return `Self` without the type-checker
complaining about a phantom struct.

Backend support:

- **interpreter** dispatches the call through the user-method
  registry before falling back to any hardcoded value-method
  arms (Step B of the extension-trait migration).
- **JIT** monomorphises the impl method per receiver primitive
  (`toy_i64__neg`, `toy_f64__neg`). The receiver must be a
  bare local identifier; chained calls (`x.neg().neg()`) keep
  falling back to the interpreter at the second receiver
  because the JIT's call lowering still requires an identifier
  receiver.
- **AOT compiler** declares each impl method as
  `toy_<TypeName>__<method>` and emits regular cranelift calls.

The numeric stdlib (`core/std/i64.t`, `core/std/f64.t`) uses
exactly this machinery — `n.abs()` / `r.sqrt()` are not
language-built-ins, they're `impl Abs for i64` / `impl Sqrt for f64`
loaded from the core directory.

### Trait bounds on generics

A type parameter can be bounded by a trait. Inside the function the
bounded parameter supports the trait's methods; at the call site the
type-checker verifies the supplied concrete type implements the trait.

```rust
fn announce<T: Greet>(x: T) -> str {
    x.greet()
}

fn main() -> u64 {
    val d = Dog { name: "Rex" }
    announce(d)
    0u64
}
```

The bound chain is transparent: a caller's own `<U: Greet>` parameter
satisfies `<T: Greet>` without further conversion.

#### Multiple bounds (`<T: A + B>`)

A generic parameter can require **multiple** traits at once by joining
them with `+`. The bounded value can call any method from any of the
listed traits, and the call site must supply a concrete type that
implements **all** of them.

```rust
trait Greet { fn greet(self: Self) -> i64 }
trait Named { fn id(self: Self)    -> i64 }

struct Dog { tag: i64 }
impl Greet for Dog { fn greet(self: Self) -> i64 { 1i64 } }
impl Named for Dog { fn id(self: Self)    -> i64 { self.tag } }

fn describe<T: Greet + Named>(x: T) -> i64 {
    x.greet() + x.id()    # uses one method from each trait
}

fn main() -> i64 {
    val d = Dog { tag: 41i64 }
    describe(d)    # => 42
}
```

Semantics:

- The bound list is order-independent — `<T: A + B>` and
  `<T: B + A>` accept the same set of concrete types and dispatch
  the same methods.
- Three or more bounds work the same way: `<T: A + B + C>`.
- Bound-chain pass-through extends to multi-bounds: a caller's
  `<U: A + B>` satisfies a callee's `<T: A>`, `<T: B>`, or
  `<T: A + B>` without conversion. (A `<U: A>`-only caller does
  **not** satisfy `<T: A + B>` — Named is still missing.)
- Method dispatch on the bounded `T` searches the bound list in
  declaration order and takes the first trait whose signature
  table contains the called method. Two bound traits providing a
  method of the same name is allowed but the first-listed trait
  wins; rename the trait method to avoid ambiguity if that
  matters.

Failure messages pinpoint the first missing trait so the user knows
which `impl` is needed:

```
Function 'describe' generic parameter 'T' bound violation:
  expected Greet + Named, got Cat
  (struct `Cat` does not implement trait `Named`)
```

Internally, single bounds (`<T: A>`) keep the bare
`TypeDecl::Identifier(trait_sym)` form in
`Function::generic_bounds`; multi-bounds promote to the new
`TypeDecl::TraitIntersection(Vec<DefaultSymbol>)` variant. Backends
(interpreter / cranelift JIT / AOT) see only monomorphized concrete
types, so they need no changes — the work is entirely in the parser
and type checker. The 3-way consistency tests
`multi_bound_dispatch_round_trip` and `multi_bound_three_traits_round_trip`
in `compiler/tests/consistency.rs` pin this.

### Dynamic dispatch with `dyn Trait`

`dyn TraitName` is a **trait object** — a static type that abstracts
over every concrete type implementing the trait. Where bounded
generics (`<T: Trait>`) monomorphise per concrete type at call
sites, `dyn Trait` carries the trait's vtable at runtime and
dispatches the method through it.

```rust
trait Animal {
    fn sound(self: Self) -> i64
}
struct Dog {}
struct Cat {}
impl Animal for Dog { fn sound(self: Self) -> i64 { 1i64 } }
impl Animal for Cat { fn sound(self: Self) -> i64 { 2i64 } }

fn describe(a: &dyn Animal) -> i64 {
    a.sound()
}

fn main() -> i64 {
    val d = Dog {}
    val c = Cat {}
    describe(d) + describe(c)    # auto-borrow + dyn coercion -> 3
}
```

Notes:

- Use `&dyn Trait` (parameter / receiver) for borrowed trait
  objects. `&mut dyn Trait` carries the mutation back to the
  caller's struct binding. Bare `dyn Trait` value positions
  (struct field, `val` binding, return type) are rejected — owned
  trait objects need a sized-erasure mechanism (`Box<dyn Trait>`)
  which lands in A5 Phase 4.
- At a call site, `T` and `&T` automatically coerce to `&dyn Trait`
  when `T` implements the trait. Explicit borrow `&value` works
  the same way. `&mut value` coerces to `&mut dyn Trait` when the
  variable is `var`-bound.
- Default method bodies (A1) work with `dyn Trait` dispatch — the
  expansion pre-pass installs the default as an inherent method on
  every impl, so `(&dyn Trait).default_method()` resolves through
  the regular method registry.
- Trait methods may return any shape supported by the AOT compiler
  — `i64`, `Self`, structs, tuples, and enums all flow through the
  thunk + multi-result `call_indirect` machinery. `&mut self`
  methods that return a compound value also work; the impl's
  writeback leaves and the user-visible return leaves share the
  same call.
- The static type-check at the call site is the load-bearing
  guarantee. The interpreter's runtime arg check skips the
  structural comparison when the expected type is `Dyn` because
  the actual value is just the underlying `Object` and a
  structural compare would always reject.

#### Backend implementation

The AOT backend lowers `&dyn Trait` to a 2-word fat pointer:
`(data_ptr: u64, vtable_ptr: u64)`. For each `impl Trait for Type`
the compiler emits a vtable data symbol
`toy_vtable_<trait>_<struct>` plus a per-method **thunk function**
`toy_dyn_thunk_<trait>_<struct>_<method>`. The thunk has the
uniform signature `(data_ptr: u64, ...user_args) -> ret_ty`, reads
the receiver's leaves from `data_ptr` via `PtrRead`, and forwards
to the impl method's flat-leaf cranelift call.

At a coercion site (`describe(d)` where `d: Dog`), the caller
allocates a per-call stack slot sized by the struct's natural-sum
byte count, `PtrWrite`s each leaf into the slot, and pairs the
slot address with the vtable address as the fat pointer. For
`&mut dyn Trait` the dispatch site also schedules a post-call
`PtrRead` walk over the slot to copy the mutated leaves back into
the caller's struct binding.

Phase status (A5):

- **A5-P1 (2026-05-18)** — interpreter dispatch. Parser,
  type-checker, and tree-walker support `&dyn Trait` parameters
  and the auto-coercion described above.
- **A5-P2 (2026-05-19)** — AOT compiler support. Landed in six
  sub-phases:
  - **MVP-A** — empty struct receivers (no fields). Vtable points
    at impl methods directly because `() -> R` lines up.
  - **MVP-B** — scalar-field structs. Introduces the uniform
    thunk ABI with `data_ptr` and stack-slot-backed leaf passing.
  - **MVP-C** — nested struct fields and `&mut dyn Trait`
    writeback (`pending_dyn_mut_writebacks` drain at the
    caller).
  - **MVP-D** — compound (struct) return through
    `CallIndirectFnStruct`.
  - **MVP-E** — tuple and enum returns through
    `CallIndirectFnTuple` / `CallIndirectFnEnum`.
  - **MVP-F** — `&mut self` methods that *also* return a
    compound type (`CallWithSelfWritebackCompound` fans both
    return leaves and writeback leaves out of the same call).
- **A5-P3 (planned)** — support in the *interpreter's* cranelift
  JIT. Programs that thread `Dyn` types fall back there via the
  JIT eligibility's catch-all; they run correctly but skip JIT
  optimisation. The compiler's JIT mode
  (`compiler <file> --all-backends`) already agrees with the
  interpreter and the AOT binary on `dyn` dispatch.
- **A5-P4 (planned)** — owned trait objects via `Box<dyn Trait>`
  and heterogeneous `Vec<Box<dyn Trait>>` collections. `Box<T>`
  landed in the meantime (`core/std/box.t`); what is still missing
  is erasing a trait object into it.

### Errors caught at type-check time

- An `impl Trait for Type` block missing a method: `missing method 'm' required by trait`
- A signature mismatch (parameter count, type, or return type): `parameter type mismatch` / `return type mismatch`
- Calling a trait-bounded generic with a non-conforming struct: `bound violation: ... (struct 'X' does not implement trait 'T')`
- Passing a non-conforming struct where `&dyn Trait` is expected: `Type error: expected Ref { ..., inner: Dyn(...) }, found Struct(...). Function '...' argument N type mismatch`
- Duplicate trait declaration or duplicate method name within a trait

### Out of scope (initial implementation)

Landed since this list was first written, kept here so the history
reads straight:

- ~~Generics on traits themselves (`trait Foo<T> { ... }`)~~ — done
  (ITER-PROTOCOL-TRAIT, 2026-05-07).
- ~~Default method bodies in traits~~ — done (A1, 2026-05-18); see
  *Default method bodies* above. Remaining gap: default bodies on
  generic traits that reference `T`.
- ~~Multiple bounds (`<T: A + B>`)~~ — done (A2, 2026-05-18); see
  *Trait bounds on generics → Multiple bounds* above.
- ~~Dynamic dispatch via `dyn Trait` objects~~ — done in the
  interpreter (A5 Phase 1, 2026-05-18) and in the AOT compiler
  (A5 Phase 2 MVP-A…F, 2026-05-19); see *Dynamic dispatch with
  `dyn Trait`* above.

Still out of scope:

- Trait inheritance (`trait B: A`).
- Associated types.
- `dyn Trait` in the interpreter's cranelift JIT (A5-P3). The
  compiler's JIT mode already runs it; `INTERPRETER_JIT=1` falls
  back to the tree-walker for programs that thread `Dyn` types.
- `Box<dyn Trait>` for owned trait objects (A5-P4). `Box<T>` itself
  exists now (`core/std/box.t`), so what is missing is the erased
  vtable-carrying form, not the box.
- `dyn TraitA + TraitB` multi-trait objects and `dyn Iterator<T>`
  generic-trait objects.
- `&dyn Trait` / `&mut dyn Trait` in return position or in a struct
  field — the REF-Stage-2 escape rule rejects ref-typed returns and
  fields.

---

## Enums and pattern matching

### Declaration

```rust
enum Shape {
    Circle(i64),       # tuple variant with payload
    Rect(i64, i64),
    Point,             # unit variant
}

# Generic enum
enum Option<T> {
    None,
    Some(T),
}
```

### Construction

```rust
val a = Shape::Circle(5i64)
val b = Shape::Point
val c: Option<i64> = Option::None      # type annotation infers T = i64
val d              = Option::Some(7i64) # T inferred from payload
```

### `match`

```rust
fn area(s: Shape) -> i64 {
    match s {
        Shape::Circle(r)   => r * r * 3i64,
        Shape::Rect(w, h)  => w * h,
        Shape::Point       => 0i64,
    }
}
```

Patterns:

- `Enum::Variant` — unit variant
- `Enum::Variant(p, q, ...)` — tuple variant; sub-patterns may be
  identifiers, `_`, literals, or further nested variants
- `_` — wildcard (catch-all)
- `(p, q)` — tuple patterns (any arity ≥ 2)
- `Type { field: p, ... }` — struct patterns; `{ x }` is shorthand for
  `x: x`, and a trailing `..` ignores the fields not named (see
  [Struct patterns](#struct-patterns))
- `42i64`, `true`, `"hello"` — literal patterns for primitives
- `a | b | c` — alternatives, all sharing one arm body
- `lo..hi` — a half-open integer range
- `name @ p` — bind the matched value while `p` still tests it; `p`
  is any pattern, and `@` nests

Each arm is an expression; all arms must produce the same type.

#### Alternatives, ranges, and `@` bindings

```rust
match c {
    Color::Red | Color::Green => "warm",
    Color::Blue               => "cool",
}

match s {
    Shape::Circle(1i64 | 2i64)      => "small circle",
    Shape::Circle(n)                => "circle",
    Shape::Rect(n) | Shape::Dot(n)  => "other",     # both bind `n`
}

match n {
    0i64..5i64   => "low",         # 0 through 4
    x @ 5i64     => "exactly five",
    y @ 6i64..10i64 => "six to nine",
    _            => "high",
}

match c {
    Color::Red        => 0i64,
    same @ Color::Green => rank_of(same),   # names the value, still only Green
    Color::Blue       => 2i64,
}

match p {
    whole @ Point { x: 0i64, y } => whole.y + y,   # on the y axis
    Point { x, y }               => x + y,
}
```

- **`a | b`** puts several alternatives on one arm. They share the
  body, and each alternative is checked for reachability and
  exhaustiveness on its own — so `Color::Red | Color::Green` covers
  two variants, and `1i64 | 1i64` is an unreachable-arm error. `|`
  works at any depth: `Circle(1i64 | 2i64)` and
  `Point { x: 0i64 | 1i64, y }` expand the whole pattern, and slots
  multiply (`Rect(1i64 | 2i64, 3i64 | 4i64)` is four alternatives, up
  to a limit of 64). **Every alternative must bind the same names** —
  only one of them runs and they share the body, so
  `Circle(x) | Rect(x, y)` is a parse error.
- **`lo..hi`** is **half-open**, matching the `..` expression form:
  `0i64..5i64` covers 0 through 4. Endpoints are integer literals, and
  an empty range (`5i64..5i64`, or a `hi` below `lo`) is a type error
  rather than an arm that can never run.
- **`name @ p`** binds the matched value to `name`, which the arm body
  and any guard can use, while `p` still decides whether the arm runs.
  `p` is any pattern — a literal, a range, an enum variant, a struct or
  tuple pattern — and `@` may appear at any depth, so `Some(n @ 3i64)`
  reads a payload and tests it in one pattern.

A binding never rejects a value, so `@` is **invisible to
exhaustiveness and reachability**: `x @ Color::Red` covers `Red` the
way the bare variant does, and `x @ 1i64` makes a later `1i64` arm
unreachable.

Ranges and literals are tracked as **one set of intervals** over the
scrutinee's type, which decides both questions the checker asks:

- An arm whose values an earlier arm already covers is **unreachable**
  — `0i64..10i64` then `3i64..5i64`, or `0i64..10i64` then `5i64`.
- Adjacent intervals merge, so arms that **partition the type** are
  exhaustive and need no `_`:

```rust
fn size(n: u64) -> str {
    match n {
        0u64..10u64                       => "tiny",
        10u64..100u64                     => "small",
        100u64..18446744073709551615u64   => "big",
        18446744073709551615u64           => "max",   # `..` excludes it
    }
}
```

In practice most integer matches still want a `_`; spanning `i64` by
hand is only worth it when the bounds are meaningful. An or-pattern
counts toward exhaustiveness the same way — an enum whose variants are
all named across alternatives needs no wildcard.

### Guards

```rust
match x {
    v if v < 0i64 => "negative",
    0i64           => "zero",
    _              => "positive",
}
```

A guard is a `bool` expression evaluated **after** the pattern matches
and its bindings are in scope. A guarded arm doesn't count as fully
covering its variant for exhaustiveness checking.

### Exhaustiveness and reachability

- Every `match` must be exhaustive: missing variants without a `_`
  fallback is a type error. The error names the missing variant.
- A duplicate variant arm or any arm placed after a `_` catch-all is a
  type error (unreachable code).

### Struct patterns

A struct is taken apart by naming its fields:

```rust
struct Point { x: i64, y: i64 }

match p {
    Point { x: 0i64, y: 0i64 } => "origin",
    Point { x: 0i64, y } => "on the y axis",
    Point { x, y } => "somewhere else",
}
```

- `field: <pattern>` matches the field against any pattern, so struct
  patterns nest inside one another and inside enum patterns.
- The shorthand `{ x }` binds the field to a name of its own — the
  same as writing `x: x`.
- Field order in the pattern is free; the declaration's order does not
  matter.
- **Every field must be named**, unless the pattern ends in `..`. A
  pattern that quietly ignored what it does not mention would read as
  complete when it is not, and a field added later would slip past
  every existing pattern.
- A struct has one shape, so an arm whose field patterns are all
  irrefutable covers every value — that arm makes the match
  exhaustive, and one is required unless a wildcard is present.
- The same patterns work in [`if val` / `while val`](#if-val--while-val).

Struct patterns run on every backend. Note that passing a struct
literal straight into a call (`f(Point { x: 1i64, y: 2i64 })`) is a
separate compiler-MVP gap — bind it first — and it has nothing to do
with the pattern.

### Tuple-struct patterns

A [tuple struct](#tuple-structs) is taken apart by position:

```rust
struct Meters(i64)
struct Sample(i64, str)

match m {
    Meters(0i64) => "zero",
    Meters(_)    => "some distance",
}

match s {
    Sample(n, name) => name,
}

match s {
    Sample(n, ..) => n,             # ignore the rest
}
```

This is the same machinery as the named form: the fields are named by
index, so every rule above carries over unchanged — sub-patterns
nest, field coverage is required unless the pattern ends in `..`, an
all-irrefutable arm makes the match exhaustive, and `if val` accepts
the same patterns. The diagnostics name the position (`does not
mention field 1`).

The two forms are not interchangeable: `Point(v)` on a struct with
named fields is an error (`struct \`Point\` has no field \`0\``), and
`Meters { .. }` is how a tuple struct would be written in the named
form. An enum variant is always spelled with `::`
(`Shape::Circle(r)`), so it never collides with this form.

### Nested patterns

```rust
match x {
    Option::Some(Option::Some(v)) => v,
    _                              => 0i64,
}
```

---

## Generics and bounds

Generic parameters appear in `<...>` on `fn`, `struct`, `impl`, and
`enum`. Type inference unifies parameters from argument shapes,
literal payloads, return-type annotations, and explicit type
arguments.

```rust
fn pair<T, U>(a: T, b: U) -> (T, U) { (a, b) }

val p = pair(1u64, true)              # T = u64, U = bool
val q: (str, str) = pair("a", "b")    # T, U from annotation
```

Bound syntax (`<T: SomeBound>`) is enforced at the call site: the
inferred type argument must implement the named trait. A generic
trait bound must match on its type arguments too (`<I: Iter<i64>>`
is not satisfied by an `impl Iter<str>`), and a multiple bound
(`<T: A + B>`) requires every trait in it.

```rust
trait Greet {
    fn greet(self: Self) -> str
}

fn announce<T: Greet>(x: T) -> str { x.greet() }

fn relay<U: Greet>(x: U) -> str { announce(x) }   # bound passes through
```

A caller's own bounded parameter satisfies the same bound, as
`relay` shows — the bound chain is transparent, so a bounded generic
can hand its value to another function with the same requirement.
Primitives satisfy a bound through an extension impl
(`impl Ord for u64` in `core/std/ord.t` makes `<T: Ord>` accept
`u64`).

The check covers methods as well as free functions: a method from an
`impl<T: Ord> Vec<T>` block rejects a receiver whose element type has
no `Ord` impl, so `v.sort()` on a `Vec<SomeStructWithoutOrd>` is a
type error rather than a run-time dispatch failure.

The allocator system doesn't use generic-bound parameters at all — see
[Allocators](#allocators) for the active-stack convention.

---

## Modules

### Declaration

```rust
package my.helpers

pub fn add(a: u64, b: u64) -> u64 {
    a + b
}
```

The `package` declaration is optional and, when present, must be the
first non-comment line. Path components are dot-separated identifiers.

> **Note**: package segments must be `Identifier` tokens; reserved
> keywords (`i64`, `f64`, etc.) are rejected by the parser. Files
> living under `core/std/i64.t` / `core/std/f64.t` therefore omit
> the `package` line — the auto-load path derives the module path
> from the file system instead.

### Core modules (auto-load)

Each binary discovers a **core modules directory** at startup and
auto-loads every `.t` file under it. User code can call exported
functions through the qualified `module::name(...)` form without
writing an explicit `import` line.

Resolution priority for the core directory (both interpreter and
compiler):

1. CLI flag — `--core-modules <DIR>` overrides everything.
2. Env var `TOYLANG_CORE_MODULES`. The empty value (`TOYLANG_CORE_MODULES=`)
   opts out entirely; auto-load becomes a no-op.
3. Executable-relative search — `<exe>/core/`,
   `<exe>/../share/toylang/core/`, `<exe>/../../core/`. The third
   entry is the dev-tree fallback so `target/debug/{interpreter,compiler}`
   finds `<repo>/core/` automatically.

Module paths come from the file system layout under the core dir:

| Path                       | Module path        | Alias    |
|----------------------------|--------------------|----------|
| `<core>/foo.t`             | `["foo"]`          | `foo`    |
| `<core>/foo/foo.t`         | `["foo"]`          | `foo`    |
| `<core>/foo/mod.t`         | `["foo"]`          | `foo`    |
| `<core>/std/math.t`        | `["std", "math"]`  | `math`   |
| `<core>/std/i64.t`         | `["std", "i64"]`   | `i64`    |

The alias is always the last path segment, so `math::sin(x)` resolves
through `core/std/math.t` even though the on-disk path is nested.

Auto-loaded modules opt out of bare-call enforcement, so user code
can shadow auto-loaded names with same-name local definitions
(`fn sin(x: i64) -> i64 { ... }` works even though `math::sin`
exists). The qualified form keeps working through the synthetic
`ImportDecl` the auto-load path inserts.

The function table is keyed by `(module_qualifier, name)` end-to-end
(IR `function_index`, type-checker `context.functions`, interpreter
runtime `function_qualified` map), so two modules that each export
`pub fn foo` no longer collide. Bare `foo(...)` first looks up
`(None, "foo")` (the user-authored slot); if missed, it falls back
to the unique `(Some(_), "foo")` entry across modules. Qualified
`bar::foo(...)` looks up `(Some("bar"), "foo")` directly. The
compiler also mangles each integrated function's exported symbol to
`toy_<qualifier>__<name>` so distinct cranelift entries are emitted
for cross-module same-name functions.

### Import (explicit, optional)

```rust
import my.helpers           # bare import
import my.helpers as h      # aliased import
```

Most user programs don't need `import` at all — the core directory
covers the stdlib. Use `import` for non-core modules or for paths
that aren't on the auto-load search root.

Resolution order for `import a.b.c`:

1. `<core_modules_dir>/a/b/c.t`, `<core_modules_dir>/a/b/c/c.t`,
   `<core_modules_dir>/a/b/c/mod.t`
2. `modules/a/b/c.t`, `modules/a/b/c/c.t`, `modules/a/b/c/mod.t`
   (cwd-relative legacy fallback)

`import` of a module that auto-load already integrated is a no-op
(deduped by module path), so adding an explicit `import math` to a
program that already had `math::sin(x)` working causes no error.

### Qualified identifiers

```rust
math::sin(x)
h::add(1u64, 2u64)              # via alias
```

`::` is the scope-resolution operator; `.` is field/method access only.

---

## Allocators

The allocator system has two halves. Stdlib **wrapper structs**
(`Arena`, `FixedBuffer`, …) carry an allocation policy written in
toylang and are used by calling their methods directly. `with allocator
= expr { body }` is the other half: it lexically rebinds the raw
`Allocator` handle that the `__builtin_heap_*` builtins consult inside
`body`. The two are less connected than they look — see *Tracking only
follows the wrapper's own methods* below.

```rust
var arena = Arena::new()
val p: ptr = arena.alloc(64u64)   # served by `arena`, tracked by it
# ... use p ...
arena.reset()                     # frees everything the arena handed out
```

### Allocator type

`Allocator` is an opaque handle. Two values are equal iff cloned from
the same `Rc` (`==` and `!=` only — no ordering).

### Getting an allocator

Only two allocator builtins remain; every allocation *policy* lives in
the stdlib (`core/std/allocator.t`), written in toylang on top of them:

| Expression | Returns |
|---|---|
| `__builtin_default_allocator()` | The process-wide global allocator |
| `__builtin_current_allocator()` | The allocator at the top of the active stack |
| `ambient` | Sugar for `__builtin_current_allocator()` |

The earlier `__builtin_arena_allocator()` /
`__builtin_fixed_buffer_allocator(cap)` constructors no longer exist —
calling them is an `E0003` "function not found". Use the stdlib wrapper
structs instead; each implements `trait Alloc`
(`alloc` / `free` / `realloc`) plus its own introspection:

| Wrapper | Constructor | Extra methods |
|---|---|---|
| `Global` | `Global::new()` | — (forwards to the default allocator) |
| `Arena` | `Arena::new()` | `bytes_used()`, `reset()`, `drop()` (bulk free) |
| `FixedBuffer` | `FixedBuffer::new(cap)` | `capacity()`, `used()`, `remaining()`, `is_empty()`, `reset()`; allocation past `cap` returns a null `ptr` |
| `SlotRegion` | `SlotRegion::new(slot_bytes, slot_count)` | `capacity()`, `live()`, `layout_report()` |

```rust
var arena = Arena::new()
val p: ptr = arena.alloc(32u64)
arena.bytes_used()               # 32
arena.reset()                    # frees in one go; bytes_used() == 0
```

**Tracking only follows the wrapper's own methods.** A wrapper is a
toylang struct holding an `(addr, size)` table beside a plain
`Allocator` handle, and its policy lives in its own `alloc` / `free` /
`realloc` bodies. `with allocator = <wrapper>` pushes the *handle* the
wrapper holds, which for all four wrappers is the process-wide default
allocator — so a raw `__builtin_heap_alloc(n)` inside
`with allocator = fb { ... }` is served by the global allocator: it is
neither counted by `fb.used()` nor bounded by `fb`'s capacity (a
1 KiB request against a 16-byte `FixedBuffer` returns a valid
pointer). Call `fb.alloc(n)` / `arena.alloc(n)` when you want the
policy and the bookkeeping.

### Allocator-aware functions

Functions don't need to thread an allocator through their parameters.
Wrap the call site in `with allocator = ... { ... }` and the callee's
`__builtin_heap_alloc` (and `realloc` / `free`) reads the active
handle off the runtime stack — no allocator parameter, no plumbing.
With today's stdlib wrappers that handle is still the global allocator
(see the note above), so this is a scoping mechanism ready for
custom handles rather than a way to redirect a callee into an arena:

```rust
fn collect(items: [u64; 4]) -> ptr {
    __builtin_heap_alloc(32u64)
}

# Caller:
var arena = Arena::new()
with allocator = arena {
    val p = collect([1u64, 2u64, 3u64, 4u64])
}
arena.drop()
```

### `with` semantics

- `with allocator = expr { body }` evaluates `expr`, pushes the
  resulting `Allocator` onto the active stack for the duration of
  `body`, and pops on every exit path (value, return, break, error).
  `expr` may be an `Allocator` directly, or a struct with exactly one
  `Allocator` field (the stdlib wrappers) — in that case the field's
  handle is what gets pushed.
- Nested `with` works as a stack; `ambient` always sees the innermost.
- The body's type is the body block's type.

### Region escape (`[E0022]`)

An arena hands back everything it allocated at once, so memory taken
from one must not outlive it. A value that came from an allocation made
under a *scoped* allocator may not be returned, and may not be bound or
assigned to a name declared outside that allocator's own scope:

```rust
fn leak() -> ptr {
    val arena = Arena::new()
    with allocator = arena { __builtin_heap_alloc(8u64) }   # [E0022]
}                                          # arena frees it here
```

Staying inside the scope is fine — the arena is still alive there:

```rust
val arena = Arena::new()
val p = with allocator = arena { __builtin_heap_alloc(8u64) }
__builtin_ptr_write(p, 0u64, 7u64)                         # fine
```

What matters is where the memory came from, not the type of the value.
A scalar read back out of arena memory is a copy and escapes nothing,
so `with allocator = arena { list.get(0u64) }` is legal while
`with allocator = arena { list }` is not. "Came from an allocation" is
the same reachability answer `never_allocates` uses, so a call several
levels deep still counts and nothing needs annotating.

Only allocators whose lifetime the check can see are scoped: one bound
by a `val` / `var` in the same function, or one built inline
(`with allocator = Arena::new() { ... }`, which dies with the block).
A parameter or a field belongs to the caller, so allocating from it and
returning the result — what `Arena::alloc` itself does — is correct and
not checked here.

Not covered: a pointer stored through `__builtin_ptr_write`, one passed
to a function that keeps it, and use after `arena.reset()` (the region
ends at a scope boundary, not at a call).

### Pointer / memory builtins

These always go through the active allocator:

| Builtin | Signature |
|---|---|
| `__builtin_heap_alloc(size: u64)` | `-> ptr` |
| `__builtin_heap_free(p: ptr)` | `-> ()` |
| `__builtin_heap_realloc(p: ptr, new_size: u64)` | `-> ptr` |
| `__builtin_ptr_read(p: ptr, offset: u64)` | `-> T` (return type from context) |
| `__builtin_ptr_write(p: ptr, offset: u64, v: T)` | `-> ()` |
| `__builtin_ptr_is_null(p: ptr)` | `-> bool` |
| `__builtin_mem_copy(src: ptr, dst: ptr, size: u64)` | `-> ()` |
| `__builtin_mem_move(src: ptr, dst: ptr, size: u64)` | `-> ()` |
| `__builtin_mem_set(p: ptr, byte: u8, size: u64)` | `-> ()` |
| `__builtin_ptr_offset(p: ptr, bytes: u64)` | `-> ptr` (address arithmetic; no allocation) |
| `__builtin_ptr_eq(a: ptr, b: ptr)` | `-> bool` (address equality) |
| `__builtin_null_ptr()` | `-> ptr` (address 0; `__builtin_heap_alloc(0u64)` may return non-null, so use this when you need a portable null) |

`__builtin_ptr_read` is type-polymorphic: it returns the type required
by its surrounding context (the lhs annotation of `val v: T = ...`,
typically). `__builtin_ptr_write` accepts any type.

The builtins that dereference (`ptr_read` / `ptr_write` / the `mem_*`
family) may only appear in a body declared
[`unsafe fn`](#unsafe-fn--raw-memory-access); the address-arithmetic
ones (`ptr_offset` / `ptr_eq` / `ptr_is_null` / `null_ptr`) and the
allocator ones are safe.

### `Ptr<T>` — a typed window (stdlib)

`core/std/ptr.t` wraps the raw builtins in a typed window. The
pointee lives in the type, so the stride and the read/write shape
both come from `T` instead of being hand-multiplied and
hand-annotated at every access:

```rust
val p: Ptr<u64> = Ptr::alloc(4u64)   # 4 * sizeof::<u64>() bytes
p.set(0u64, 7u64)                    # or p[0u64] = 7u64
val v: u64 = p.get(1u64)             # or p[1u64]
val q: Ptr<u64> = p.offset(2u64)     # window 2 elements forward
val raw: ptr = p.as_raw()            # the bare address
```

It is an ordinary struct + impl (`addr: ptr` is its only field;
`T` appears in no field, the same rule that makes `Box<T>` legal),
with no compiler special-casing and no backend differences.
`Ptr<T>` is a **window, not an owner**: `alloc` sizes a buffer the
caller owns (free it with `__builtin_heap_free(p.as_raw())`), the
indexes are unchecked, and `offset` shares the allocation with the
window it came from. `get` / `set` and the bracket forms are
declared [`unsafe fn`](#unsafe-fn--raw-memory-access) inside the
stdlib, so the raw access stays there and callers of `Ptr<T>` stay
safe. See [`design-docs/POINTER.md`](../design-docs/POINTER.md) for
the layer map.

**Non-null (POINTER P5).** A `Ptr<T>` value is non-null by
construction — every backend answers `heap_alloc(0)` with null, so
`alloc` rounds an empty request up to one byte and never hands back
address 0. Absence is modelled one level up, as `Option<Ptr<T>>`:

```rust
struct Node { v: i64, next: Option<Ptr<Node>> }

val empty: Option<Ptr<Node>> = Option::None
val head: Ptr<Node> = cons(1i64, empty)
```

The `next: ptr` + `has_next: bool` pairing a raw-`ptr` list needs
disappears — the match arm is the truth. The trade is size: the
enum layout is a `u64` tag plus every variant's payload with no
slot sharing, so `Option<Ptr<T>>` is 16 bytes, not 8. The invariant
is by construction in `ptr.t`, not compiler-enforced (struct field
visibility is recorded but unenforced); a hand-written
`Ptr { addr: ... }` literal is the raw-pointer escape hatch.

### `Span<T>` — a bounds-checked view (stdlib)

`core/std/span.t` pairs a `Ptr<T>` window with a length, so a
parameter carries the element type *and* the bounds — the library
answer to a slice type:

```rust
val p: Ptr<u64> = Ptr::alloc(4u64)
val s: Span<u64> = Span::from_parts(p, 4u64)
s.set(0u64, 7u64)
val v: u64 = s[2u64]        # or s.get(2u64)
s.len()                     # 4
__simd_load(s.as_raw(), 0u64)   # element-indexed addressing
```

`get` / `set` (and the bracket sugar) panic on an out-of-range
index, the same contract a built-in array's trap has. A span is a
**view, not an owner** — `from_parts` copies a `(Ptr<T>, len)` pair
and nothing is allocated or freed; sub-windows are
`Span::from_parts(p.offset(k), shorter_len)`. A span crosses
function boundaries by value (`fn sum(s: Span<u64>) -> u64`).

**A window may not outlive the buffer it views** (`[E0026]`). When the
buffer is a binding in the same frame, the window cannot leave the
frame with it — it may not be returned, nor bound or assigned outside
the buffer's scope. Staying beside the buffer is fine, which is what a
window is for, and a window on a *parameter* belongs to the caller, so
handing that one back out is correct (`fn as_span(&self) ->
Option<Span<T>>` is exactly that shape). The rule is
[Region escape](#region-escape-e0022)'s over a different owner, and
shares its pass; `--explain E0026` has the reasoning.

Two hazards stay uncovered: a window captured by a closure, and one
held across a `push` that reallocates. Neither is lifetime-shaped.

Converting to and from `str`:

| Builtin | Signature |
|---|---|
| `__builtin_str_to_ptr(s: str)` | `-> ptr` (UTF-8 bytes, NUL-terminated) |
| `__builtin_str_len(s: str)` | `-> u64` (bytes, not characters) |
| `__builtin_str_from_bytes(p: ptr, len: u64)` | `-> str` |

`__builtin_str_from_bytes` is the **only way to build a `str` from
bytes computed at run time**. The bytes are **copied**, so writing to
the buffer afterwards does not change the `str` that came out. It does
not validate UTF-8 — the buffer's correctness is the program's
responsibility, as with every other raw-pointer builtin.

---

## Built-in functions and methods

### Output

```rust
print(value)        # to stdout, no newline
println(value)      # to stdout + newline
eprint(value)       # to stderr, no newline
eprintln(value)     # to stderr + newline
```

All four accept any type; rendering goes through
`Object::to_display_string` (strings are unquoted, structs/dicts
deterministic via sorted keys). These are user-facing names without
the `__builtin_` prefix.

`eprint` / `eprintln` (RUNTIME-LIB P0-A) differ from their stdout
counterparts in the descriptor and nothing else: the same any-type
argument, the same rendering, the same [`Display`](#display)
dispatch, the same `io` effect. Write a program's *output* with
`print` and its *diagnostics* with `eprint`, so a caller can pipe one
without the other. The two streams are separate all the way down —
each print instruction carries which stream it belongs to, and every
backend keeps them apart (the interpreter's own JIT declines a
program that uses them and falls back to the tree-walker, which is
not observable).

f64 rendering (the same rule every backend shares, since RUNTIME_PORT
R1): Rust's `Display` (shortest round-trip, so `0.1f64 + 0.2f64`
prints as `0.30000000000000004`), except that integral values get a
trailing `.0` so floats stay visually distinct from ints (`1f64`
prints as `1.0`). This is a change from the old AOT runtime, which
used C's `%g` (6 significant digits: `1234567.75` used to print as
`1.23457e+06`).

`f32` rendering (SIMD-F32) follows the same rule at single precision:
Rust's `Display` on the `f32` value (shortest round-trip), with the
trailing `.0` convention for integral values (`1.0f32` prints `1.0`).
Format *specs* (`{x:.2}`) are not accepted on `f32` yet — the
formattable set is unchanged — so write `{v}` or convert first.

### I/O module (`io::`)

`core/std/io.t` exposes the process environment through the `io::`
module, auto-loaded like the rest of the stdlib:

```rust
io::read_line() -> Result<str, IoError>  # one line from stdin, no trailing newline; Err(IoError::EndOfInput) at EOF
io::argc() -> u64               # number of program arguments (excluding program name)
io::arg(i: u64) -> str          # the i-th argument; "" out of range
io::env_var(name: str) -> Result<str, IoError>   # the environment variable; Err(IoError::NotFound) when unset
io::read_file(path: str) -> Result<str, IoError> # file contents; Err(err) when unreadable
io::write_file(path: str, contents: str) -> Result<u64, IoError>  # replace the file; Ok(bytes written)
io::append_file(path: str, contents: str) -> Result<u64, IoError> # add to the end; Ok(bytes written)
io::file_exists(path: str) -> bool
io::exit(code: u64)             # end the process now with this status
io::now() -> u64                # seconds since the Unix epoch
io::random() -> u64             # pseudo-random; not reproducible
io::random_seed(seed: u64)      # re-seed `random()`; reproducible afterwards
io::strftime(fmt: str, secs: u64) -> str  # format epoch seconds (UTC)
io::env_count() -> u64          # number of environment variables
io::env_name(i: u64) -> str     # the i-th environment variable's name
io::env_value(i: u64) -> str    # the i-th environment variable's value
```

Each function delegates to an `extern fn`; see [Linking to real C
libraries](#linking-to-real-c-libraries) for the
boundary rules. An `extern fn` boundary carries only scalars, so the
payload-carrying call records a failure status in the runtime and a
paired `__extern_io_*_status` call reads it back immediately after —
from toylang's point of view the pair is atomic, and the stdlib
wrapper turns it into a `Result<_, IoError>`. `read_line` raises
`Err(IoError::EndOfInput)` only before any byte was read (a final
line without a newline is still an `Ok`, an empty line is `Ok("")`);
an empty environment value is a valid `Ok("")`. The `IoError`
variants (declared in `core/std/io.t`):

| Variant | Failure |
|---|---|
| `IoError::NotFound` | the path does not exist |
| `IoError::PermissionDenied` | the OS denied the open |
| `IoError::IsADirectory` | the path names a directory |
| `IoError::ReadError` | any other read failure (also non-UTF-8 contents on the interpreter — the compiled backends read raw bytes and do not validate UTF-8, which stays a known divergence for invalid files) |
| `IoError::WriteError` | any other write failure (a short write, or a failure at close time) |
| `IoError::EndOfInput` | `read_line`: EOF before any byte |
| `IoError::Unknown` | a failure with no errno behind it |

A `match` over the variants is exhaustive — handle every case or fall
back to `_` (compare that with the string API this replaces, where a
misspelled `reason == "not found"` silently took the else branch).
`IoError` implements `Display`, so `println(err)` prints the reason
text (`not found`, ...). Because these functions return a compound,
bind the result with `val` — see
[Known limitations](#known-limitations) for the two positions where an
enum-returning call needs no binding.

`write_file` replaces what the file held and `append_file` adds to
its end; both create the file when it is missing, and both answer
with the number of bytes written — always `contents`' length on
success, which is why a zero-byte write is `Ok(0)` rather than
indistinguishable from a failure. A path whose directory does not
exist fails as `NotFound`, not as a write error.

`io::exit(code)` ends the process immediately with `code` as its
status: nothing is printed, no `Drop` runs, buffered output is
flushed, and the call does not return in any backend (an embedder
running a program that calls it ends with it). The low 8 bits are
what a shell sees, the same truncation
[`main`'s return value](#process-exit-code) goes through. Use it for
a normal-path exit status; a failure that should say why is a
[`panic`](#termination).

Determinism: `random()` is seeded from the clock and process id, so it
is not reproducible across runs — but `random_seed(s)` makes the
sequence reproducible (identical across runs *and* backends for the
same `s`, with `0` honoured literally). `strftime(fmt, secs)` formats
a fixed subset of C `strftime` specifiers and is **UTC, never local
time**, so a fixed timestamp formats identically regardless of the
host timezone (this is what keeps the 3-backend consistency suite
byte-identical). The environment list (`env_count` / `env_name` /
`env_value`) iterates `environ` order, which the interpreter's
`std::env::vars` matches.

### Parsing numbers (`parse::`)

`core/std/parse.t` reads numbers back out of text — the direction
`__builtin_to_string` and [string interpolation](#string-interpolation)
do not cover, and what `io::read_line()` / `io::arg(i)` /
`io::read_file(path)` need to be useful:

```rust
parse::to_u64(s: str) -> Result<u64, ParseError>
parse::to_i64(s: str) -> Result<i64, ParseError>
parse::to_f64(s: str) -> Result<f64, ParseError>
parse::to_bool(s: str) -> Result<bool, ParseError>
```

```rust
val n = parse::to_u64(io::arg(0u64))
match n {
    Result::Ok(v) => v,
    Result::Err(e) => { eprintln("not a number: {e}") 0u64 }
}
```

| `ParseError` | Meaning |
|---|---|
| `ParseError::Empty` | the input had no characters |
| `ParseError::Invalid` | the input is not in the grammar below |
| `ParseError::Overflow` | the value does not fit the target type |

`ParseError` implements `Display` (`empty input`, `invalid number`,
`out of range`), and `?` propagates it like any other `Result`.

**The grammar is narrow on purpose**, and identical on every backend:

- **No surrounding whitespace.** `" 42"` is `Err(Invalid)`; call
  `.trim()` first when the input is a line of text. A parser that
  trims silently cannot be made strict again by its caller.
- **Decimal only.** No `0x` / `0b` prefixes and no `_` separators —
  those are *source literal* syntax, not input syntax.
- **Sign.** `to_i64` / `to_f64` accept `+` and `-`; `to_u64` accepts a
  leading `+` and reads `"-1"` as `Err(Invalid)` rather than wrapping.
- **Bounds are checked, not wrapped.** `"18446744073709551616"` is
  `Err(Overflow)`, and `to_i64("-9223372036854775808")` succeeds —
  the magnitude is read first and compared against the bound for its
  sign, so the one value whose positive form does not fit is not lost.
- **Floats** are `[+-]? ( digits ('.' digits?)? | '.' digits ) ([eE]
  [+-]? digits)?` — so `"3"`, `".5"`, `"5."`, `"2.5e3"` and `"25E-1"`
  are all accepted, and exponents are reachable even though the
  language has no exponent *literal*. There is no `inf` / `nan`
  spelling and no hex float: a value too large to represent is
  `Err(Overflow)`, never `Ok(infinity)`. Underflow is not an error —
  the nearest representable value is `0.0`.
- **`to_bool`** takes exactly `true` or `false`. No case folding, no
  `1` / `0` / `yes`.

The integer and bool parsers are written in toylang and need no
runtime support. `to_f64` validates the grammar in toylang and then
hands the accepted string to the host's decimal → binary conversion
(`str::parse` in the interpreter, `strtod` in the compiled runtime):
both are correctly rounded, and deciding the grammar *before* the
call is what keeps a C `strtod`'s extra spellings (`inf`, hex floats,
leading whitespace) from being accepted on one backend only.

Round-tripping holds in the direction that matters: whatever the
language prints for a `u64` / `i64` / `f64`, `parse::` reads back to
the same value.

### Termination

```rust
panic(message: str)              # aborts the run with `panic: <message>`
assert(cond: bool, message: str) # panics with `message` when `cond` is false
```

`panic` evaluates its argument, prints `panic: <message>` to stderr,
and stops with a non-zero exit code. Type-wise the call is treated as
`Unknown`, which means it unifies with any context — `panic` may sit
in a value position like the `then` branch of an `if`-expression and
the surrounding type is fixed by the *other* branches:

```rust
fn divide(a: i64, b: i64) -> i64 {
    if b == 0i64 { panic("division by zero") } else { a / b }
}
```

A function whose body diverges via `panic` (no value path) also
typechecks regardless of the declared return type, since the
divergent body is treated as `Unknown`:

```rust
fn unimplemented() -> i64 { panic("not implemented") }
```

`panic` cannot be caught from user code in this iteration; the run
stops immediately.

`assert(cond, msg)` is sugar for `if !cond { panic(msg) }` with a
clearer call-site reading. The condition is evaluated first; the
message expression is only evaluated when the condition fails. Type
signature: `(bool, str) -> ()`.

```rust
fn divmod(a: i64, b: i64) -> (i64, i64) {
    assert(b != 0i64, "divmod: divisor must be non-zero")
    (a / b, a % b)
}
```

#### `assert_eq` / `assert_ne`

`assert_eq(a, b)` and `assert_ne(a, b)` are parser-level macros that
desugar to a value comparison plus `assert(cond, msg)`. The synthesized
panic message includes the source line and pretty-printed left/right
operands, so a failing equality check reads as

```text
panic: assertion `left == right` failed at line 42
  left:  3
  right: 4
```

Both operands may be of any type that supports `==` / `!=` (primitives
+ struct types with a user-provided `eq` overload). Each call binds the
operands to fresh synthetic locals before the check, so side-effecting
expressions (`assert_eq(counter.next(), 1u64)`) evaluate exactly once.

JIT eligibility falls back when the panic message ends up as a runtime
concat chain; interpreter and AOT both run the desugared form
unchanged.

### Source location and `__builtin_dbg`

```rust
__builtin_source_file()    -> str   # path of the current source file
__builtin_source_line()    -> u64   # call-site line, 1-indexed
__builtin_source_column()  -> u64   # call-site column, 1-indexed
__builtin_function_name()  -> str   # enclosing function, e.g. "S::boom"
__builtin_dbg(value: T)    -> T     # print "[file:line] expr = value", return value
__builtin_backtrace()      -> str   # how the program got here
```

The first five are parser-level macros — none of them reach the type
checker or the backends. The four `__builtin_source_*` /
`__builtin_function_name` calls are substituted in-place with a
literal of the matching type at parse time.

`__builtin_function_name()` names a method the way a backtrace frame
does (`S::boom`, not `boom`), and reads `<toplevel>` outside any
function body.

`__builtin_backtrace()` is the one that is *not* a macro: only the
running program knows its own call stack. It returns the same text a
panic prints, so a program that reports its own failures says what the
runtime would have:

```text
   = backtrace (innermost first):
       inner (called at line 2)
       outer (called at line 4)
       main
```

Every execution engine answers it from whatever stack it keeps, and a
`--release` build — which records no frames — returns just the entry
frame. The interpreter's own JIT declines the builtin and falls back to
the tree-walker, which is not observable beyond speed. `__builtin_dbg(EXPR)` captures `EXPR`'s source text
verbatim from the input buffer and rewrites the call to:

```text
{
    val __dbg_<n> = EXPR
    println("[<file>:<line>] <captured_text> = ".concat(__builtin_to_string(__dbg_<n>)))
    __dbg_<n>
}
```

so the call returns `EXPR`'s value while emitting a trace line — useful
inside expression chains where introducing a temporary would be
disruptive (`val r = f(__builtin_dbg(g(x)))`). The captured text comes
from a byte-range slice of the source, not from re-rendering the AST,
so user formatting is preserved exactly.

When `set_source_file` is not wired (e.g. inline test strings without
a path), `__builtin_source_file()` returns `"<source>"`.

### Type introspection

```rust
__builtin_sizeof(value: T) -> u64
__builtin_sizeof::<T>() -> u64
```

Both answer the byte size of a type. Primitives use fixed
widths (`u64`/`i64`/`f64`/`ptr` = 8, `bool` = 1); structs sum their
fields; tuples and arrays sum their elements; an enum is a `u64` tag
plus **every** variant's payload laid end to end.

The enum figure is a property of the type, not of the value in hand —
`Shape::Point` and `Shape::Rect(1, 2)` report the same width. That is
what makes it usable as a stride: `Vec<T>` takes its element size from
the first element pushed, so a per-variant answer would give a
`Vec<Option<i64>>` a different layout depending on which element
happened to arrive first.

The **type-argument form** (POINTER P1) takes no value: the written
type is the whole call, so an allocator can size a slot without a
representative value in hand
(`__builtin_heap_alloc(__builtin_sizeof::<T>() * n)`). `T` may be a
primitive, a tuple, a declared struct / enum (with type arguments), or
a generic parameter in scope — inside a generic function the
parameter resolves from the call site's arguments, inside a generic
impl method from the receiver, and in a `Self`-returning associated
call from the `val` / `var` annotation
(`val s: Slice2<u64> = Slice2::alloc(n)`). Arrays, dicts, function
types and trait objects have no size to report. The interpreter-side
JIT silently falls back for the type-argument form (the value form is
supported).

### Allocation counters

```rust
__builtin_alloc_count() -> u64
__builtin_free_count() -> u64
__builtin_realloc_count() -> u64
__builtin_cumulative_bytes() -> u64
__builtin_live_bytes() -> u64
__builtin_peak_live_bytes() -> u64
```

These return what the current run has requested from the allocator so
far. The names and their meanings are **identical** to the fields
`--profile=mem` reports.

- **Request-based.** A `realloc` counts as one resize; whether the
  allocation actually moved the block never shows up. That is what
  makes the numbers identical across all three backends.
- **What the program asked for, not what the runtime spends.** The
  counters cover `__builtin_heap_alloc` / `__builtin_heap_realloc` and
  the stdlib built on them. Memory the language runtime uses to hold a
  `str` — concatenation, `to_string`, interpolation, literals — is not
  counted, because each backend holds strings differently and counting
  them would make the numbers engine-specific. `println("n = {n}")`
  therefore costs nothing on every backend, while
  `__builtin_heap_alloc(32u64)` costs 32 everywhere.
- **Per run.** Every counter is 0 at the start of `main` (or of a
  single `test` block). In a compiled binary that coincides with
  process start.
- **No profiling flag needed.** A program that reads these gets the
  real numbers without `--profile=mem`.

They are callable from `requires` / `ensures` and from `test` blocks,
so memory can be bounded by contract.

To bound what a **single call** does rather than the run so far, the
usual spelling is one of the three budget clauses —
`ensures allocates(N)` / `retains(N)` / `allocations(N)`, see
[Design by Contract](#old-and-allocation-contracts) — which read these
same counters against an entry snapshot. Reading a counter directly is
for an absolute bound, or for one of the fields the clauses do not
cover:

```rust
fn parse(s: str) -> u64
    ensures __builtin_live_bytes() <= 4096u64
{ ... }

test "tidy leaks nothing" {
    val before: u64 = __builtin_live_bytes()
    val r: u64 = tidy(64u64)
    assert_eq(__builtin_live_bytes(), before)
}
```

There is no builtin for `peak_at_request`. That field is the axis the
report uses to say *when* a peak happened reproducibly — not a
quantity a program should hold an opinion about.

### Numeric value methods

```rust
val n: i64 = -5i64
val r: f64 = 16f64
val s: f64 = -3.5f64

n.abs()    # -> i64  (i64::wrapping_abs semantics)
s.abs()    # -> f64  (IEEE 754 fabs: sign-bit flip, preserves NaN)
r.sqrt()   # -> f64  (IEEE 754; NaN for negative inputs)
```

These are **regular extension-trait methods**, not hardcoded
builtins. The trait declarations and impl blocks live in
`core/std/i64.t` and `core/std/f64.t`:

```rust
trait Abs { fn abs(self: Self) -> Self }
impl Abs for i64 { fn abs(self: Self) -> Self { math::abs(self) } }
impl Abs for f64 { fn abs(self: Self) -> Self { math::fabs(self) } }

trait Sqrt { fn sqrt(self: Self) -> Self }
impl Sqrt for f64 { fn sqrt(self: Self) -> Self { math::sqrt(self) } }
```

Because they're auto-loaded with the rest of `core/`, the call sites
work with no `import` line. All three backends (interpreter / JIT /
AOT compiler) dispatch the call through the same `method_registry`
the user-defined extension traits go through (see *Traits → Extension
traits over primitives* below). Programs that opt out of auto-load
lose `n.abs()` / `r.sqrt()` — call `math::abs(n)` / `math::sqrt(r)`
through an explicit `import std.math` instead.

Use whichever shape reads more naturally at the call site — the
method form is typically clearer when the receiver is a single value,
the qualified form better when the operand is itself a sub-expression.

Chained primitive method calls work in all three backends:

```rust
val r: f64 = (4.0f64).sqrt().abs()    # 2.0f64
val n: i64 = (-5i64).abs()            # 5
```

The receiver of an outer call may itself be a `MethodCall` /
`Call` / arithmetic expression — both interpreter and JIT route
the inner result through the same primitive-method dispatch. The
AOT compiler does the same via the `value_scalar`-driven
`lower_method_call` arm.

### Overflow-aware arithmetic

`core/std/checked.t` provides the escape hatch from wrapping
arithmetic: ask for the result as an `Option`, or ask for it clamped
to the type's bound.

```rust
val big: u64 = 18446744073709551615u64
val five: u64 = 5u64
val ten: u64 = 10u64

val sum = big.checked_add(five)      # -> Option<u64>, here Option::None
match sum {
    Option::Some(v) => println(v),
    Option::None => println("overflows"),
}

val clamped = five.saturating_sub(ten)   # -> 0u64, not a huge number
```

One trait, `Checked`, with an impl for **every integer width** —
`u8` / `u16` / `u32` / `u64` and `i8` / `i16` / `i32` / `i64`:

| | signature |
|---|---|
| `checked_add` / `checked_sub` / `checked_mul` / `checked_div` | `(self: Self, other: Self) -> Option<Self>` |
| `saturating_add` / `saturating_sub` / `saturating_mul` | `(self: Self, other: Self) -> Self` |

Each impl clamps to its own type's bounds, so the same call reads the
same way at every width: `saturating_add` on a `u8` gives `255u8`
where the `u64` impl gives `u64::MAX`.

`checked_div` answers `Option::None` for both traps the `/` operator
raises — a zero divisor and `MIN / -1` — so it is the way to divide by
a value that might be either.

Narrow widths differ from `u64` in one way worth knowing: `a - b`
below zero **wraps** on `u8` / `u16` / `u32`, where the same
expression on `u64` traps (*Runtime traps* lists that trap for `u64`
only). So at a narrow width `checked_sub` / `saturating_sub` are not
a nicer spelling of something the operator would have caught — they
are the only thing that reports the underflow at all.

Two call shapes matter for the compiled backends: the **receiver must
be a name**, not a literal (`five.checked_add(...)`, not
`5u64.checked_add(...)`), and an enum result must be **bound with
`val` before it is matched** (or folded with `??`, which the type
checker rewrites into that binding for you). Both are existing
compiler-MVP limits rather than anything specific to this module.

### Math (via the `math` module)

```rust
math::abs(x: i64) -> i64
math::fabs(x: f64) -> f64
math::sqrt(x: f64) -> f64
math::min_i64(a: i64, b: i64) -> i64
math::min_u64(a: u64, b: u64) -> u64
math::max_i64(a: i64, b: i64) -> i64
math::max_u64(a: u64, b: u64) -> u64
math::pow(base: f64, exp: f64) -> f64

# f64 transcendentals (libm-backed)
math::sin(x: f64) -> f64
math::cos(x: f64) -> f64
math::tan(x: f64) -> f64
math::log(x: f64) -> f64    # natural log (ln)
math::log2(x: f64) -> f64
math::exp(x: f64) -> f64

# f64 rounding (cranelift-native)
math::floor(x: f64) -> f64
math::ceil(x: f64) -> f64
```

The math intrinsics live in the standard `math` module at
`core/std/math.t`. The auto-load path picks the file up at
startup and aliases it as `math` (the alias derives from the
last path segment). **No `import math` line is required** — call
sites use the `math::name(...)` qualified form directly.

Each wrapper forwards to a backend-side `extern fn __extern_*`
helper that the runtime / JIT / AOT compiler implement
directly. The `__extern_` prefix signals "runtime binding,
resolved through the per-backend dispatch tables." Programs
that opt out of auto-load (`TOYLANG_CORE_MODULES=`) lose the
`math::*` qualifier; call the matching `__extern_*` symbol
directly when that matters.

Semantics:

- `abs(x: i64)` matches Rust's `wrapping_abs`, so
  `abs(i64::MIN)` returns `i64::MIN` rather than panicking.
- `min_*` / `max_*` come in `_i64` and `_u64` flavours; the
  type-checker enforces that both operands match the wrapper's
  declared type.
- `sqrt` and `pow` follow IEEE 754: `sqrt(-1f64)` returns NaN,
  `pow(0f64, 0f64)` returns `1f64`.

All entries are wired through every backend (interpreter / JIT /
AOT compiler). The JIT lowers `min_*` / `max_*` / `abs` to a
cranelift `select` chain, `sqrt` / `floor` / `ceil` / `fabs` to
the native cranelift instructions, and `sin` / `cos` / `tan` /
`log` / `log2` / `exp` / `pow` to small Rust helpers. The AOT
compiler re-declares each `__extern_*_f64` as a `Linkage::Import`
cranelift function pointing at the matching libm symbol.

### Option and Result (via the auto-loaded `option` / `result` modules)

```rust
enum Option<T> {
    None,
    Some(T),
}

enum Result<T, E> {
    Ok(T),
    Err(E),
}
```

Both enums live in `core/std/option.t` and `core/std/result.t` and
are auto-loaded into every program (no `import` line needed). The
implementations carry a small set of stack-only methods — Option
and Result are tagged unions, not heap-allocated containers, so
they don't deal with allocators (heap responsibility belongs to
whatever T or E carries).

```rust
val o: Option<u64> = Option::Some(42u64)

o.is_some()                                  # bool
o.is_none()                                  # bool
o.unwrap_or(0u64)                            # T  — payload or `default`
o.unwrap()                                   # T  — panics on None
o.expect("must be Some")                     # T  — panics with the literal message on None
o.map(fn(x: u64) -> u64 { x + 1u64 })        # Option<U>
o.unwrap_or_else(fn() -> u64 { 0u64 })       # T  — default computed lazily

val r: Result<u64, str> = Result::Err("boom")

r.is_ok()                                    # bool
r.is_err()                                   # bool
r.unwrap_or(99u64)                           # T
r.unwrap()                                   # T  — panics on Err
r.expect("ok required")                      # T  — panics on Err
r.map(fn(x: u64) -> u64 { x * 2u64 })        # Result<U, E>
r.map_err(fn(e: str) -> str { e })           # Result<T, F>
```

`unwrap_or(default)` takes the default eagerly; `unwrap_or_else(f)`
calls `f` only on `None`. `expect(msg)` accepts a string literal and
lowers to the same `panic("...")` machinery the runtime already
provides. The closure-taking methods (`map` / `map_err` /
`unwrap_or_else`) run on every backend.

For error-propagation rather than panicking unwrap, see the
postfix [`?` operator](#-operator-early-return): `divide(a, b)?`
unwraps `Ok` / `Some` and short-circuits the enclosing function
with the `Err` / `None` value on failure.

**A discarded `Result` is a warning** (`[E0025]`). The language has no
exceptions by design — a failure travels in the return value or not at
all — so a statement that produces a `Result` and drops it reports
success whatever happened:

```rust
fn main() -> u64 {
    io::write_file("out.txt", body)   # E0025: the disk could be full
    0u64
}
```

Handle it (`match`), propagate it (`?`), or say the result is ignored
on purpose by binding it — `val _ignored = write_file(path, body)`,
which needs no syntax of its own. Only a statement that is *not* the
last one in its block counts, since a block's last statement is its
value; `Option` is not covered, because an ignored `Option` is usually
a lookup whose absence is the answer. `--explain E0025` has the
reasoning.

User code can shadow either type by declaring a same-name local
`enum` or `struct` — module integration silently skips the stdlib
declaration when the user's program already defines the name, so
no "already defined" error fires (the user's version wins
end-to-end). This shadowing is intentional: `Option` and `Result`
are common enough names that occasional reuse is expected.

Backend coverage:

- **Interpreter** dispatches the methods through the same
  `method_registry` extension-trait machinery the user-defined
  `impl<T> MyEnum<T> { ... }` blocks use.
- **AOT compiler** lowers each method as a monomorph instance via
  `instantiate_generic_method_with_self_type`; enum payload types
  include `i64` / `u64` / `f64` / `bool` / `str` / nested enum /
  struct / tuple.
- **JIT** (the interpreter's, `INTERPRETER_JIT=1`) compiles enum
  values whose payloads are JIT scalars: tuple and unit variant
  constructors, `match` over them with payload binding, generic
  enums, enums crossing function boundaries, and receiver-method
  dispatch (`opt.unwrap_or(0i64)`) — the JE-2 → JE-6 family in
  [`JIT.md`](../design-docs/JIT.md). What still falls back:
  non-scalar payloads (a struct or tuple inside a variant),
  nested generic enums (`Option<Option<T>>`), and match arms with
  guards. The fallback is silent; `-v` names the reason.

### `From` / `Into` (via the auto-loaded `convert` module)

```rust
trait From<T> { fn from(value: T) -> Self }
trait Into<T> { fn into(self: Self) -> T }
```

Both traits live in `core/std/convert.t` and are auto-loaded. You
write the `From` side only:

```rust
enum MyErr { Fail(u64) }

impl From<str> for MyErr {
    fn from(value: str) -> MyErr { MyErr::Fail(7u64) }
}
```

and the `Into` side is derived at the call site: `expr.into()`
rewrites to `Target::from(expr)` when the expected type `Target` —
typically the `val` annotation or parameter type at the call site —
implements `From<typeof(expr)>`. There is no blanket impl written
out — the language has no `where` clauses, so the type checker
supplies the `U: From<T> → T: Into<U>` rule syntactically.
Conversions live in the module that owns the target type (e.g.
`impl From<str> for String` in `core/std/string.t`).

`?` consults the same impls for cross-error propagation: an inner
`Result<T, E1>` inside a function returning `Result<T2, E2>`
converts through `E2::from(e)` — see the [`?` operator](#-operator-early-return).

Because `from` on an enum target returns a compound, bind the result
with `val` — see [Known limitations](#known-limitations) for the two
positions where an enum-returning call needs no binding.

### `Display`

`core/std/display.t`:

```rust
pub trait Display {
    fn to_str(&self) -> str
}
```

A type that has `to_str(&self) -> str` decides how it prints through
`print` / `println` and what string interpolation `"{v}"` splices in:

```rust
struct Point { x: i64, y: i64 }
impl Display for Point {
    fn to_str(&self) -> str { "({self.x}, {self.y})" }
}

val p = Point { x: 1i64, y: 2i64 }
println(p)          # (1, 2)
println("at {p}")   # at (1, 2)
```

Without an implementation the value still prints structurally
(`Point { x: 1, y: 2 }`). That is useful while debugging but rarely
what human-facing output wants.

- **Dispatch keys off the method, not off an `impl Display for`
  registration** — the same way `==` finds `eq` and `+` finds `add`.
  An inherent `fn to_str(&self) -> str` works just as well. The trait
  exists to name the contract, to surface it in `--api`, and to let
  you write `<T: Display>`.
- **Only the matching shape is a renderer.** A
  `fn to_str(&self, radix: u64) -> str`, or one returning `-> u64`,
  is left alone and keeps its ordinary meaning. Otherwise `println`
  would report arity errors about a call the user never wrote.
- The type checker rewrites `println(v)` into `println(v.to_str())`,
  so **the backends only ever see a normal method call**.
- `String` has `impl Display for String`, so `println(s)` prints the
  text it holds (before that existed it printed
  `String { cap: 2, data: 12, elem_size: 1, len: 2 }`).
- **Interpolating your own type inside its `to_str` recurses forever.**
  As with user-defined `Display` in other languages, that is the
  implementer's responsibility.

`str` and `String` are different types, so the method is named
`to_str`, not `to_string`: `String::to_string() -> String` already
exists as the idempotent clone Rust has, and what interpolation
concatenates is the `str` side.

### String methods

Method-call syntax on `str` (the static-string primitive):

| Method | Signature |
|---|---|
| `str.len()` | `-> u64` |
| `str.concat(other: str)` | `-> str` |
| `str.contains(needle: str)` | `-> bool` |
| `str.trim()` | `-> str` |
| `str.to_upper()` | `-> str` |
| `str.to_lower()` | `-> str` |

Two more shapes type-check but do **not** run: `str.substring(start,
end)` and `str.split(sep)` reach an unimplemented arm of the
interpreter's string dispatch and stop the program with
`Internal error: Method '<name>' not found for String type`. Both work
on [`String`](#string-heap-byte-buffer), through the `Substring` /
`Split` trait impls — build one with `String::from_str(s)` when you
need them.

### `String` (heap byte buffer)

`core/std/string.t::struct String { data, len, cap, elem_size }`
is a **nominal struct**. Memory layout matches `Vec<u8>`
exactly (the `__builtin_heap_*` / `ptr_read` / `ptr_write`
family operates on the underlying byte buffer with no per-type
special-casing) but the type system treats `String` as its own
identity — it is not interchangeable with `Vec<u8>`.

```rust
val s: String = String::from_str("hello")
val n: u64 = s.len()          # 5
val also_n: u64 = s.size()    # 5  — alias of len
val empty: bool = s.is_empty()
val p: ptr = s.as_ptr()
s.push_char('!')              # UTF-8 encoded into 1-4 bytes
s.push_str(other)             # other: &String via auto-borrow
val eq: bool = s == other     # operator overload via String::eq
s.clear()
```

Method dispatch:

- `impl String` (in `core/std/string.t`) — inherent byte-buffer
  helpers: `new`, `from_str`, `push`, `pop`, `get`, `set`,
  `size`, `len`, `as_ptr`, `capacity`, `is_empty`, `clear`,
  `extend_bytes`, `push_str`, `push_char`, `eq`, `to_string`.
- Extension trait impls (in `core/std/string.t`):
  - `impl Substring for String` / `impl Trim for String` /
    `impl CaseConvert for String` (from `core/std/str_ops.t`) —
    `.substring(s, e)`, `.trim()`, `.to_upper()`, `.to_lower()`.
  - `impl Concat<String> for String` /
    `impl Contains<String> for String` /
    `impl Split<String, Vec<String>> for String` — `.concat(t)`,
    `.contains(needle)`, `.split(sep)`.

`Vec<u8>` (in `core/std/collections/vec.t`) is a separate
generic type for byte-level work that doesn't need string
semantics. It is **not** `String` — convert across the boundary
with `String::from_str(s)` (str → String) or `s.as_ptr()` +
manual byte handling (String → raw pointer).

### `is_null`

Not available. The interpreter still carries a universal `is_null()`
implementation, but the type checker has no rule that reaches it, so
every receiver — `i64`, `ptr`, `str`, a struct, a `dict` — fails with
`[E0007] Method 'is_null' ... method not found` (the message suggests
the supported spellings below). Its only argument would have been the
[`null` literal](#boolean-and-null-literals), which the type checker
refuses (`E0015`).

For a raw pointer, test the address instead:

```rust
val p: ptr = __builtin_heap_alloc(32u64)
__builtin_ptr_is_null(p)                  # bool
__builtin_ptr_eq(p, __builtin_null_ptr()) # same question, spelled out
```

For an absent value, use `Option<T>` and `is_none()`.

---

## Test blocks

A `test "name" { ... }` block is a top-level declaration holding
assertions about the program it sits in:

```rust
fn add(a: u64, b: u64) -> u64 { a + b }

test "add works" {
    assert_eq(add(1u64, 2u64), 3u64)
}
```

- `test` is a **contextual** keyword — it only starts a test block at
  the top level, followed by a string literal and a block. `fn
  test(...)` and `val test = ...` keep working.
- Each block lowers to a zero-argument function, so type checking and
  every backend treat it as ordinary code — there is no special form
  to support.
- A normal run (`interpreter <file>`) ignores test blocks entirely and
  calls `main`.
- `interpreter --test <file>` runs them instead of `main`, each in its
  own evaluation context, and prints a `N passed, M failed` summary.
  A failing block reports the assertion's left / right values and the
  source line; the process exits non-zero when anything failed.
- The body is ordinary code, so `assert` / `assert_eq` / `assert_ne`,
  `panic`, and the [allocation counters](#allocation-counters) are all
  available.

```
$ interpreter --test example.t
FAILED  this one fails (example.t:7)
     8 |     assert_eq(add(1u64, 2u64), 4u64)
       |     ^^^^^^^^^ panic: assertion `left == right` failed at line 8
      left:  3
      right: 4
1 passed, 1 failed
```

`interpreter --check <file>` is the neighbouring tool: it treats a
function's — or an `impl` method's — `requires` clauses as an input
filter and its `ensures` clauses as the oracle, property-tests the
callable, and prints a minimised counterexample (`self` included for
methods, whose receiver is generated by recursively sampling the
struct's fields). `--seed=N` reproduces a run.

A trial is capped at a fixed number of loop iterations. Sampling
reaches inputs no caller would pass — `while i <= n` is finite for
every `n` a program uses and effectively endless for the `u64::MAX`
the generator eventually draws — so without a cap the checker would
hang on a correct function. Exceeding it ends that function's check
with `EXHAUSTED`, which is neither a pass nor a failure: add a
`requires` bounding the inputs the function was written for. The cap
counts iterations rather than elapsed time, so `--seed=N` replays to
the same verdict on any machine.

---

## Design by Contract

Functions and methods may declare preconditions and postconditions
between the return type and the body block.

```rust
fn divide(a: i64, b: i64) -> i64
    requires b != 0i64
    requires !(a == -9223372036854775808i64 && b == -1i64)
    ensures  result * b + (a % b) == a
{
    a / b
}
```

The postcondition accounts for the remainder because integer division
truncates — `result * b == a` is false for `7 / 2` — and the second
precondition rules out `MIN / -1`, which a non-zero divisor does not.
Both are the kind of detail `--check` surfaces in seconds; see the
[Design by Contract guide](design_by_contract.md) for the working
practice around these clauses.

Rules:

- `requires` clauses run on entry, with parameters in scope.
- `ensures` clauses run on **every** value-returning exit path — the
  tail expression, an early `return`, and a `?` propagation alike —
  with the same parameters in scope plus the special identifier
  `result` bound to the value that exit returns (an `Err` for a `?`
  propagation).
- Multiple clauses of either kind are AND-composed; the failure
  diagnostic identifies the specific clause by 1-based index.
- Each clause must type-check as `bool`.
- Methods can use `self` in both clauses.
- A contract declared on a **trait** method applies to every `impl` of
  that trait; the trait's clauses run before any the impl adds. The
  impl must keep the trait's parameter names, since a clause is an
  expression over them — renaming is a type error when the trait
  declares a contract.
- An `impl` may **not add a `requires` clause of its own** (`[E0023]`).
  A precondition is what callers are told to satisfy, and a caller
  reaching the method through `&dyn Trait` or a `<T: Trait>` bound can
  read the trait's clauses and nothing else — so an implementation that
  demands more breaks calls that were written correctly. This holds
  whether or not the trait declares a precondition: a trait that says
  nothing lets callers pass anything the types allow. Move the clause
  to the trait, or handle the case in the body. An **inherent** `impl`
  (no trait) is unaffected — there is no promise to break.
- An `impl` **may** add `ensures` clauses. Promising more than the
  trait did breaks nobody, so both sets are checked, the trait's first.
- Failures abort the call with `ContractViolation` and propagate to the
  process exit unless caught.
- `ensures` clauses may call `old(expr)`, which is the value `expr` had
  **on entry** to the function. See *`old(...)` and allocation
  contracts* below.

### `old(...)` and allocation contracts

A postcondition often needs to talk about what changed, not just about
the final state. `old(expr)` inside an `ensures` clause is the value
`expr` had on entry:

```rust
struct Counter { n: u64 }

impl Counter {
    fn bump(&mut self, by: u64) -> u64
        ensures result == old(self.n) + by
    {
        self.n = self.n + by
        self.n
    }
}
```

Without it the clause cannot be written at all: by the time `ensures`
runs, `self.n` already holds the new value.

Each snapshot is evaluated once, **after** the `requires` clauses and
**before** the body, so a precondition can be what makes the snapshot
expression legal. When postconditions are switched off (see *Runtime
gating*) the snapshots are not taken either — nothing would read them.

Because the [allocation counters](#allocation-counters) are callable
from contracts, `old` is what makes a function's **memory behaviour**
part of its signature:

```rust
# "allocates nothing" — enforced, not a comment that rots
fn triangle(n: u64) -> u64
    ensures allocates(0u64)
{ ... }

# "requests at most 256 bytes, in one go, and hands them all back"
fn scratch(n: u64) -> u64
    ensures allocates(256u64)
    ensures allocations(1u64)
    ensures retains(0u64)
{ ... }
```

`allocates(N)` / `retains(N)` / `allocations(N)` are contextual forms
usable only inside an `ensures` clause. Each desugars to a comparison
against the matching counter's entry snapshot:

| Clause | Counter | Question it answers |
|---|---|---|
| `allocates(N)` | `__builtin_cumulative_bytes()` | were any bytes requested at all |
| `retains(N)` | `__builtin_live_bytes()` | were any bytes not handed back |
| `allocations(N)` | `__builtin_alloc_count()` | how many requests were made |

The three do not collapse into one: `retains(0)` also holds for a
function that allocated and freed, while `allocates(0)` forbids the
request in the first place.

A violated budget reports the measurement, which a hand-written
predicate cannot:

```
Contract violation: `ensures` clause #1 of function `leaky`: retained 128 bytes, budget 0 bytes
```

The same sentence comes out of a compiled binary. Writing the
comparison by hand still works and is the way to reach a counter the
three clauses do not cover — but note that a subtraction underflows
when the counter *falls* (a function that frees a pointer it was
handed), so prefer `counter() <= old(counter()) + N`, which is what
the sugar expands to.

The counters are request-based and identical across backends, so an
allocation contract means the same thing in the interpreter and in a
compiled binary. What they cover is the program's own allocations —
see [Allocation counters](#allocation-counters).
`interpreter/example/alloc_contract.t` is the worked example.

Rules and limits:

- `old(...)` is **contextual**: only a call spelled `old` directly
  inside an `ensures` clause is the snapshot form. A program that
  already has a function or variable named `old` keeps working, and
  writing `old(...)` in a `requires` clause or in a body is a
  type-check error naming the reason.
- Nested `old(old(x))` is refused — the inner one would snapshot the
  same instant.
- The snapshot expression is checked in the entry scope: it may read
  parameters and `self`, but not `result`.

### Contracts and traps

A precondition that rules out a [runtime trap](#runtime-traps) takes
the guard's place: the check happens once, on entry, instead of at
every operation.

```rust
fn div(a: i64, b: i64) -> i64
    requires b != 0i64          # checked once here...
{
    a / b                       # ...so this needs no divide-by-zero guard
}

fn take(a: u64, b: u64) -> u64
    requires a >= b             # the exact condition the guard would test
{
    a - b
}
```

Writing the contract is therefore not only a correctness statement —
it is how the operation gets cheaper. In a loop doing one division and
one subtraction per iteration, the contracted version measured **~2x**
the throughput of the same code without the clauses (100M iterations,
AOT, cranelift `speed`; the guards are not something cranelift can
remove on its own, since it cannot see the precondition). A loop doing
one signed array access per iteration measured **~2.7x** with
`requires i >= 0i64` + `requires i < 4i64` replacing the bounds guard
(0.06s vs 0.16s, same setup).

What can be elided, and when:

| Clause | Guard it replaces |
|---|---|
| `x != 0`, `0 != x`, `x > 0`, `x >= 1` | divide-by-zero on `a / x`, `a % x` |
| `a >= b`, `b <= a` | `u64` subtraction underflow on `a - b` |
| `i < N`, `i <= N` (integer literal `N`) | bounds check on `arr[i]`, when the array is no longer than `N` and the index is unsigned |
| `i >= 0` **and** `i < N` | bounds check on `arr[i]` with a **signed** index — including the negative-adjustment path, which `i >= 0` rules out |
| `x != -1`, `x >= 0`, `x > 0`, `x >= 1` | the signed `MIN / -1` division trap (`x` may be either operand) |

- Only **parameters** qualify. The type checker refuses assignment to a
  parameter, so a fact proved on entry holds for the whole body,
  including inside loops. A field, an index, or any computed
  expression keeps its guard.
- **Facts chain.** The clauses are closed transitively, so the middle
  of a chain can be implicit: `a >= b` and `b >= c` prove `a - c` safe
  to subtract; `j <= i` and `i < N` bound `arr[j]`; `j >= 0` and
  `j <= i` prove `i` non-negative.
- A `val` / `var` in the body that **takes over the parameter's name**
  drops the fact from that point on — the guard site would be reading
  the new binding.
- `&&` contributes both halves; `||` contributes neither.
- **`--release` keeps every guard.** Preconditions are not emitted
  there, so nothing verifies them, and an unverified contract must not
  be allowed to remove a memory-safety check. This is deliberately the
  opposite of the usual arrangement: the optimisation is on in checked
  builds and off in unchecked ones.

### Branches and loops say the same things

A `requires` is not the only place a program states what it knows. An
`if` states it too, and states it where most code actually does:

```rust
fn div(a: u64, b: u64) -> u64 {
    if b != 0u64 { a / b } else { 0u64 }    # no divide-by-zero guard
}

fn take(a: u64, b: u64) -> u64 {
    if a < b { 0u64 } else { a - b }        # no underflow guard: the
}                                           # else means `a >= b`
```

A `for` loop states the range of its induction variable, which is
exactly what a bounds check tests:

```rust
val arr: [u64; 8] = [...]
for i in 0u64..8u64 {
    total = total + arr[i]                  # no bounds check
}
```

The same table above applies, read as conditions rather than clauses,
and the `else` branch reads each condition negated (`if x == 0u64` gives
`x != 0` in its `else`; `!`, and `||` under negation, are followed
through). A `for` range gives `i >= 0` from a non-negative literal start
and `i < N` from a literal end — both spellings (`0..N` and `0 to N`)
are half-open, so `N` is the bound.

Two differences from a precondition's facts:

- **They hold under `--release`.** The branch is evaluated either way,
  so nothing was switched off with the contract checks.
- **They are about whatever is in scope, not only parameters** — so the
  immutability that makes a precondition safe has to be established
  instead. A fact is dropped when the guarded code assigns the name in
  any way: an assignment, a re-binding, a `&mut` borrow, or a method
  call on it. The induction variable of a `for` loop cannot be assigned
  at all, which is what makes the loop case work.

Measured on 160M array reads (8-element array, inner loop
`0u64..8u64`, AOT, cranelift `speed`): **0.18s → 0.13s**.

### `never_allocates` — the static half

`ensures allocates(0u64)` measures one call; `never_allocates` says
the call cannot allocate at all, and the compiler checks it:

```rust
never_allocates fn triangle(n: u64) -> u64 { ... }

never_allocates extern fn getchar() -> i32 from "c"
```

The check walks every path out of the function and reports one that
reaches `__builtin_heap_alloc` / `__builtin_heap_realloc`, naming the
chain (`build -> new -> __builtin_heap_alloc`, `E0016`). Callees need
no annotation of their own — `Vec::new` is refused for what it
reaches, not for what it is missing.

- Costs nothing at run time; the modifier generates no code.
- Calls through a closure value, a `dyn Trait` receiver or an
  `extern fn` cannot be followed and are refused. On an `extern`
  declaration the modifier is a **declaration** rather than a check,
  since the implementation is outside the language.
- `println("{x}")` is allowed: what the runtime spends holding a `str`
  is not the program's allocation, and the counters exclude it too.
- Contextual, like `old` and the budget clauses — only a
  `never_allocates` immediately before `fn` or `extern` is the
  modifier.
- Usable on methods as well as free functions; a violation there is
  named by owner and method (`Counter::bad`).
- A method call is resolved by the receiver's type, so `Vec::new` and
  `Counter::new` are not confused with one another. Where the type is
  not recoverable, every same-named body is walked instead — the
  direction that refuses too much rather than missing an allocation.

### Contract predicates must be free of effects

A contract is a statement *about* the program, not a part of it:
switching the checks off must not change what the program does. The
compiler follows every path out of a `requires` / `ensures` clause and
reports one that allocates, frees, writes through a pointer, or
prints, naming the chain (`E0018`):

```rust
fn noisy(n: u64) -> bool { println("checking")  n > 0u64 }
fn f(n: u64) -> u64 requires noisy(n) { n }    # E0018
```

- Reads are fine, the allocation counters included —
  `ensures __builtin_live_bytes() == old(__builtin_live_bytes())` is
  what allocation contracts are made of.
- Callees need no annotation: a predicate may call any function whose
  own reachable set is clean.
- A call the check cannot follow — an `extern fn`, a closure value, a
  `dyn Trait` receiver — is reported too. An implementation outside
  the language cannot be shown to do nothing.
- **Reported as a warning today.** It becomes an error in a later
  release; the warning is the migration window.

`E0018` also covers the other direction. When every argument of a call
is a constant, the compiler evaluates the precondition itself:

```rust
const fn half(n: u64) -> u64 requires n % 2u64 == 0u64 { n / 2u64 }
val b: u64 = half(3u64)                        # E0018, with `n = 3`
```

That stays a warning even after the migration, because nothing at that
point knows whether the call is reached. In a position that *forces* a
value — a `const` initialiser — the same failure is an error
(`E0017`).

### Runtime gating

The `INTERPRETER_CONTRACTS` environment variable selects which clauses
run (the equivalent of D's `-release`):

| Value (case-insensitive) | `requires` | `ensures` |
|---|---|---|
| `all` (default; also unset, `on`, `1`, `true`) | evaluated | evaluated |
| `pre` | evaluated | skipped |
| `post` | skipped | evaluated |
| `off` (also `0`, `false`) | skipped | skipped |

Unrecognised values print a warning and fall back to `all`.

`all` and `off` map straight onto the IR (they are the same switch
`--release` throws). `pre` and `post` do not — the IR has no shape for
"check only the preconditions" — so a program run under either of them
executes on the tree-walker rather than the IR VM. It is slower, and it
is the only way to run the checks the setting actually asked for.

> **Operational guidance.** Keep `INTERPRETER_CONTRACTS=all` in
> production unless a clause has measurable performance cost. Disabling
> contracts (`pre` / `post` / `off`) is the very condition that tends
> to let latent bugs survive into release — the same reason D's
> `-release` flag is widely discouraged. The knob exists for hot-path
> benchmarks and other performance-sensitive runs; treat any other use
> as a deliberate, narrowly-scoped exception.

### Out of scope (planned)

- Named-tuple returns (`-> (q: i64, r: i64)`) for binding result
  components
- `invariant` clauses on `impl` blocks
- Static verification beyond runtime checking, other than
  `never_allocates` (above).

---

## Runtime model

### Execution

The default backend is a tree-walking interpreter. An optional cranelift
JIT (cargo feature `jit`, default on) handles a numeric subset when
`INTERPRETER_JIT=1`; everything else falls back to the tree walker.
See [`JIT.md`](../design-docs/JIT.md) for the supported subset and limitations.

### Process exit code

`main`'s integer return value becomes the process exit code:

- `Object::UInt64(v)` or `Object::Int64(v)` → `v as i32`.
- Other return types → 0.

[`io::exit(code)`](#io-module-io) ends the run earlier with a chosen
status; a [`panic`](#termination) exits non-zero with a diagnostic.

### Errors

A failure that happens while the program runs prints the same thing
whichever engine ran it — the position, the source line with a caret
under what failed, and the path that reached it:

```text
Runtime error occurred:
Error at demo.t:2:20:
   |
 2 |     if n == 0u64 { panic("bottom") }
   |                    ^^^^^ panic: bottom
   |
   = backtrace (innermost first):
       f (x7, called at line 3)
       main
```

The backtrace is innermost-first. Repeated frames — the same call from
the same line — fold into one line with a count, so a deep recursion
does not bury the message that explains it. Frames beyond a display
budget are dropped from the middle with a count of what went missing,
never silently.

A failure inside an imported module names *that* file, not the one
being run:

```text
Error at core/std/option.t:57:29:
```

`--release` keeps the position and drops the backtrace: the position is
`.rodata` the program never reads unless it dies, while the backtrace
costs a store per call.

`--diagnostics=json` reports the same failure as data (see *Errors*
under the CLI, above).

Categories include: `TypeError`, `UndefinedVariable`,
`ImmutableAssignment`, `IndexOutOfBounds`, `NullDereference`,
`ContractViolation`, and a generic `InternalError` reserved for
interpreter bugs.

### Runtime traps

A *trap* is a `panic` the language raises on an operation with no
correct answer: the program stops with a message and a non-zero exit
status, and — like every other panic — it cannot be caught. All four
engines (tree-walker, IR VM, AOT compiler, JIT) raise the same trap on
the same input; `compiler/tests/consistency.rs` pins that.

| Operation | Trap |
|---|---|
| `a - b` on `u64` where `a < b` | `u64 subtraction underflowed: 1 - 5` |
| `a / b` or `a % b` where `b == 0` (any integer width) | `integer division by zero` |
| `a / b` or `a % b` where `a` is the type's most negative value and `b == -1` | `integer division overflowed` |
| `arr[i]` / `arr[i] = v` where `i` is at or past the array's length | `array index out of bounds: index 5, length 3` |

The two that have values report them, in every engine. So does a
contract violation:

```text
Contract violation: `requires` clause #1 of function `f` evaluated to
false (with n = 0)
```

The values listed are the **scalar** parameters (and `result` for an
`ensures`). A parameter held as a struct, tuple or enum is left out —
the same rule everywhere, so the sentence does not depend on which
engine ran the program.

Two more failures stop the program the same way, though they come from
the standard library and from the engine rather than from an operator:

| Situation | Message |
|---|---|
| `v.get(i)` / `v.set(i, x)` / `s.get(i)` past the end, `v.pop()` on an empty `Vec` | `Vec::get index out of bounds`, … |
| More nested calls than the engine's stack allows | `recursion limit exceeded (N frames deep)` |

The recursion ceiling is a property of the engine, not of the language:
the tree-walker spends a host stack frame per toylang call and stops at
30, while the IR VM and the compiled backends stop at 1024. The number
is in the message so a reader can tell a legitimately deep program from
a runaway one. A `--release` build keeps no depth counter, so an
infinite recursion there ends the way C's does.

What is deliberately **not** a trap:

- **`+`, `*` and signed `-` overflow** — these wrap (see *Numeric
  semantics*). The three arithmetic traps above are the cases where a
  wrapped result is actively misleading rather than merely modular.
- **`f64` division by zero** — IEEE-754 defines it as an infinity,
  which is a value.
- **`a - b` below zero on `u8` / `u16` / `u32`** — the trap in the
  table above is `u64`'s alone; the narrow unsigned widths wrap, so
  `5u8 - 10u8` is `251u8` and the program carries on. `checked_sub` /
  `saturating_sub` (*Overflow-aware arithmetic*) are what report it at
  those widths.

A `requires` clause that already rules a trap out **removes the
guard** — see *Contracts and traps* below.

Array indices are checked against the length the binding was declared
with. A **constant** index out of range is rejected earlier, at compile
time, rather than trapping at run time. A **negative** index counts
from the end (`arr[-1i64]` is the last element) before the check
applies, so `-1` through `-length` are in bounds and anything beyond
them traps.

### No exception machinery

The language deliberately does **not** have runtime exceptions. There is
no `try` / `catch` / `throw` / `finally`, no exception type, and no
unwinding-style control flow that user code can intercept. The reserved
words for those keywords are not part of the grammar — the parser
rejects them.

Failures fall into two buckets, with separate idioms:

- **Unrecoverable failures** — surfaced via `panic("msg")` (or the
  `assert(cond, msg)` sugar). Execution stops immediately and the
  process exits with a non-zero status. `requires` / `ensures`
  violations route through the same panic path; they are not
  catchable from user code. `panic` is always active — there is no
  release-mode flag that disables it (see *Known limitations*).
- **Recoverable failures** — represented as values. The stdlib
  `enum Result<T, E>` and `enum Option<T>` are the canonical shapes;
  the call site `match`es on the variant or threads it through a
  helper (`unwrap_or` / `is_some` / `is_ok`). Generic enums + the
  pattern matcher's exhaustiveness check make this ergonomic.

Rationale: exception machinery imposes an implicit control-flow graph
(every call edge can throw) on every backend. Keeping the language
free of it lets the AOT compiler emit straight-line code, lets the
type checker reason locally, and forces error-handling to be visible
at the call site.

### Recursion

**Recursive functions** are unrestricted. The default execution engine
keeps its frames on the heap, so depth is bounded by memory rather than
by the host stack (a 200 000-deep chain runs). The tree-walker — the
fallback engine, used when a program contains something the IR does not
cover yet — recurses on the host stack instead and dies with a process
abort a few hundred frames in; that is a defect, tracked in
`design-docs/todo.md`, not a language rule.

**Recursive types** are rejected. A struct or enum that contains
itself, directly or through other types, has no finite layout: every
backend flattens a compound value down to its leaf scalars, so there is
nothing to lay out.

```rust
enum List { Cons(i64, List), Nil }   # [E0013]
struct Node { v: i64, next: Node }   # [E0013]
struct A { b: B }                    # [E0013] — A.b: B -> B.a: A
struct B { a: A }
```

A type argument counts as containment only when the type it is passed
to holds that parameter **by value**:

```rust
struct Tree { v: i64, kids: Vec<Tree> }   # fine — Vec holds a ptr
struct Wrapper<T> { v: T }
struct Held { w: Wrapper<Held> }          # [E0013] — Wrapper stores its T
```

The positions that break a cycle are the ones that hold no value of the
named type: `ptr`, function types, and `dyn Trait`. `&T` is **not** one
of them — a reference is erased to its inner type at lowering, so
`next: &Node` recurses exactly like `next: Node` (and a reference-typed
struct field is separately rejected anyway).

Write the indirection explicitly. Either keep the nodes in a `Vec` and
make the edge an index:

```rust
struct Node { v: i64, next: u64 }   # index into a Vec<Node>
```

or hold a raw `ptr` and go through the heap builtins:

```rust
struct Node { v: i64, next: ptr, has_next: bool }

val p: ptr = __builtin_heap_alloc(__builtin_sizeof(rest))
__builtin_ptr_write(p, 0u64, rest)
val rest: Node = __builtin_ptr_read(n.next, 0u64)
```

or — with the stdlib's typed window — make the edge an
`Option<Ptr<Node>>` (`core/std/ptr.t`): the pointer is non-null by
construction and the match arm is the truth, so the `has_next` flag
disappears:

```rust
struct Node { v: i64, next: Option<Ptr<Node>> }

match node.next {
    Option::None => 0i64,
    Option::Some(p) => { val n: Node = p.get(0u64) ... }
}
```

`interpreter/example/linked_list_ptr.t` and
`linked_list_typed_ptr.t` are the two shapes side by side.

The annotation on the read is not optional: it names the type whose
leaves are pulled back out of the buffer, and the read has no other way
to know its shape. A struct, tuple or enum may be named there — so the
`ptr` can equally sit in an enum payload, which is the shape a list
usually wants:

```rust
enum List { Cons(i64, ptr), Nil }
```

An enum occupies a `u64` tag followed by **every** variant's payload,
laid end to end — the same layout it has when crossing a function
boundary, and what `__builtin_sizeof` reports. The width therefore does
not depend on which variant a value holds, which is what lets
`Vec<Option<T>>` stride over its elements.

or hold the value in a `Box<T>`, which is the stdlib's name for one
heap-allocated `T`:

```rust
enum List {
    Cons(i64, Box<List>),
    Nil,
}
```

`Box` is an ordinary struct whose only field is a `ptr`. Nothing about
it is built into the compiler — it works because its type parameter
appears in no field, which is exactly the rule above.

`interpreter/example/box_linked_list.t`, `linked_list_arena.t` and
`linked_list_ptr.t` are worked examples of the three shapes.

### Ownership

A type with an `impl Drop` owns something the runtime hands back, and
the scope that built the value frees it on the way out. Handing such a
value to something that outlives the scope therefore **transfers**
ownership, and the old binding is an error to read afterwards
(`[E0014]`):

```rust
val c: Box<i64> = Box::new(7i64)
store.push(c)              # ownership goes into the Vec
val v: i64 = c.get()       # [E0014]
```

The positions that transfer are: an argument in a by-value parameter,
a member of an aggregate being built (struct literal, tuple, array,
enum payload), and the right-hand side of an assignment. A parameter
declared `&T` / `&mut T` borrows instead, so passing to one of those
leaves the caller in charge.

`val b = a` is **not** a transfer. Compound bindings alias: `b.x = 42`
shows up in `a.x`, and one drop fires for the pair. The value has one
owner; it just answers to two names.

Two limits worth knowing:

- A transfer inside a branch or a loop body is refused rather than
  tracked, because whether the binding still owns anything at scope
  exit would depend on the path taken. Build the value inside the
  branch instead.
- Ownership is transitive (DROP-GLUE): a `Vec`, a struct field or an
  enum payload that received a transferred value frees it when the
  container dies. `Vec<T>` frees each element and its buffer, a
  struct's drop glue reaches its fields, an enum's reaches the active
  payload, and `Box<T>` frees its contents before its slot. The drop
  recurses through the value (`Box<List>` → `List` → `Box<List>`), and
  frees are *idempotent*: a value reachable through several aliases (a
  `get()` copy, a shared boxed node) is freed once and later visits are
  no-ops. Both heaps are bump allocators that never reuse an address,
  so a second visit reads the block's original contents.

---

## Known limitations

These are real today; some appear in `design-docs/todo.md` as planned work.

- **Closures: partial support** — closures use `fn(params) -> R { body }`
  and the function type `fn (T1, T2) -> R` (or bare
  `(T1, T2) -> R`). Fully supported in the interpreter
  (literals, captures, HOF arguments, return values, nested
  closures). The JIT silently falls back to the interpreter
  when a program contains a closure. The AOT compiler covers
  direct calls, HOF dispatch, closure return values, and
  closures stored in struct fields — capturing and
  non-capturing alike — via a unified env-based ABI
  (Phase 6b/8). Captures support 8-byte scalars and narrow
  ints; a compound capture (struct, tuple, array, dict) is
  interpreter-only and the compiled backends refuse it by
  name. Stdlib HOF methods on generic enums (`Option::map`,
  `Result::map`, `map_err`, `unwrap_or_else`) work on every
  backend. The remaining gap is that same shape on a
  *user-defined* generic enum. See
  [Closures → Backend coverage](#closures).
- **No `else if`** — use `elif`.
- **`null` is reserved and rejected** — the literal still parses, so
  that it can be diagnosed rather than read as an identifier, but the
  type checker refuses it (`E0015`). The universal `is_null()` method
  is refused for the same reason. Model absence with `Option<T>`; for
  raw pointers use `__builtin_null_ptr()` / `__builtin_ptr_is_null(p)`.
- **`var` without an initializer does not parse** — every `val` / `var`
  needs a value at declaration.
- **`str + str` is not concatenation** — there is no string `+`, and
  neither `str` nor `String` provides an `add` overload, so the type
  checker refuses it (`E0004`, naming the alternatives). Use
  `a.concat(b)` or interpolation (`"{a}{b}"`).
- **`str.substring` / `str.split` run in the interpreter only** — they
  type-check and work there, but the compiled backends reject the call
  (`the method receiver must be a struct or enum binding`), so they
  cannot appear in a program you AOT-compile. The `String` methods have
  no such limit.
- **No bare `self`** — `self: Self` is mandatory in method signatures.
- **`val` is a keyword** — cannot be used as a parameter or field name.
- **Literals in a generic struct literal are not converted by the
  binding's annotation** — `val p: P = P { v: 5 }` works for a plain
  struct (the field type drives the literal), but
  `val c: C<i64> = C { value: 5 }` fails: for a generic struct the
  field's literal must already carry the right suffix
  (`C { value: 5i64 }`), or the annotation can be dropped and
  inference left to do the work.
- **Float literals require the `f64` suffix** — `1.5` is not a token;
  write `1.5f64`.
- **`panic` / `assert` are always active by design** — there is no
  env-var to disable them in release builds. Stripping assertions in
  production is the failure mode D's `-release` flag is criticised
  for; this language deliberately keeps them on so that invariant
  violations surface the same way regardless of build profile. (The
  `INTERPRETER_CONTRACTS` gate exists only because contract clauses
  can carry non-trivial cost; even there, `all` is the recommended
  setting — see "Operational guidance" above.)
- **No raw strings or multi-line strings** — only the regular
  `"..."` literal with backslash escapes today.
- **Compound-returning calls in expression position** — a compound
  never travels as one SSA value, so a call producing one needs
  locals to write its leaves into. Two positions have those:

  - an **argument** — `take(mk(3i64))`, `take(o.twin())`,
    `take(P::origin())`, `count(Vec::new())`, for a struct, a tuple
    or an enum alike, in any of the three call shapes;
  - an enum's **payload** — `Option::Some(mk(2i64))`.

  An enum-producing position (an `if` or `match` arm, a block tail, an
  enum payload) also takes an **associated function that returns that
  enum** — `Handle::open(id) -> Option<Handle>` written straight into
  the arm, without a `val` in between.

  Enum *constructions* (`take(Color::Red)`, `take(Option::None)`) are
  unrestricted in argument position too. Everywhere else — a tail
  expression, an operand, a condition, the right-hand side of an
  element assignment — the call still has to be bound with `val`
  first.
- **Trait limitations** — no trait inheritance; no associated
  types. Generic trait declarations (`trait Foo<T>`), default
  method bodies, multiple bounds (`<T: A + B>`) and `dyn Trait`
  dynamic dispatch are all supported — see *Traits*. What is
  still missing: `Box<dyn Trait>` for owned trait objects,
  `dyn` support in the *interpreter's* JIT (the compiler's JIT
  mode already runs it), and generic-trait default bodies that
  reference the trait's type parameter `T`. See *Traits → Out of
  scope* for the full list.
- **`extern fn` generic params: backend monomorph not yet wired** —
  the parser accepts `extern fn name<T>(x: T) -> T` and the
  interpreter dispatches via the type-erased `extern_registry` by
  literal name, so a single Rust closure satisfies every `T`. The
  JIT and AOT compiler don't yet name-mangle per-instance entries
  (they fall back to the interpreter / fail to resolve at link
  time respectively).
- **3-part qualified call paths** — `std::math::abs(x)` is not a
  direct call form; `import std.math` (or auto-load) registers the
  `math` alias and you call through `math::abs(x)`. The parser
  drops module path components beyond the last two when the head
  isn't a known struct / enum.
- **Enum support in JIT: partial** — the interpreter's JIT
  compiles tuple / unit variants with JIT-scalar payloads,
  `match` over them, generic enums, enum-typed function
  boundaries, and enum receiver methods (JE-2 → JE-6). It still
  falls back for non-scalar payloads (a struct or tuple inside a
  variant), nested generic enums (`Option<Option<T>>`), and
  guarded match arms. The AOT compiler handles all of these
  through its monomorph pipeline.
- **Generic struct / method JIT** — `struct Cell<T>` and methods
  on it run in the interpreter only; the JIT eligibility rejects
  generic struct types because `struct_layouts` isn't yet keyed by
  type args. AOT compiler handles them through its monomorph
  pipeline.
- **JIT tuple parameter shape** — only flat scalar tuples (`(i64,
  i64, bool)`) reach the JIT; nested tuples (`((a, b), c)`) and
  tuple-of-struct (`(Point, i64)`) fall back to the interpreter
  until `ParamTy::Tuple` becomes a tree of element shapes.
- **JIT generic-struct fallback is permanent for now** — see the
  entry above; generic struct types land in the interpreter
  fallback regardless of the program shape. This is acceptable
  because the AOT pipeline handles the same code through its
  monomorph machinery.

---

## See also

- [`design_by_contract.md`](design_by_contract.md) — working practice for
  `requires` / `ensures`: what to put in a contract, how to check one
  with `--check`, and the traps worth knowing before you hit them
- [`README.md`](../README.md) — project overview and quickstart
- [`interpreter/README.md`](../interpreter/README.md) — interpreter CLI
  and environment variables
- [`JIT.md`](../design-docs/JIT.md) — cranelift JIT details
- [`ALLOCATOR_PLAN.md`](../design-docs/ALLOCATOR_PLAN.md) — allocator design
- [`BUILTIN_ARCHITECTURE.md`](../design-docs/BUILTIN_ARCHITECTURE.md) — builtin
  function machinery
- [`todo.md`](../design-docs/todo.md) — planned work and feature backlog
