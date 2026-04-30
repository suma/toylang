# Language Reference

A consolidated reference for the toy language implemented in this repo.
Authoritative for syntax and semantics of `.t` source files. For
implementation-side details, see the companion documents:

- [`JIT.md`](../JIT.md) — cranelift JIT supported subset, diagnostics, performance
- [`ALLOCATOR_PLAN.md`](../ALLOCATOR_PLAN.md) — allocator-system design and roadmap
- [`BUILTIN_ARCHITECTURE.md`](../BUILTIN_ARCHITECTURE.md) — three-layer builtin design
- [`TEST_PLAN.md`](../TEST_PLAN.md) — testing strategy
- [`interpreter/README.md`](../interpreter/README.md) — interpreter CLI and env vars

## Table of Contents

- [Hello, world](#hello-world)
- [Lexical structure](#lexical-structure)
- [Types](#types)
- [Literals](#literals)
- [Expressions](#expressions)
- [Statements](#statements)
- [Functions](#functions)
- [Structs and methods](#structs-and-methods)
- [Enums and pattern matching](#enums-and-pattern-matching)
- [Generics and bounds](#generics-and-bounds)
- [Modules](#modules)
- [Allocators](#allocators)
- [Built-in functions and methods](#built-in-functions-and-methods)
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
fn  val  var  const  return  break  continue
if  elif  else  for  in  to  while
class  struct  impl  enum  match  Self
true  false  null
pub  extern  package  import  as
with  ambient
requires  ensures
u64  i64  f64  bool  str  ptr  usize  dict
```

`else if` is **not** valid; use `elif`.

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

| Type | Description |
|---|---|
| `bool` | `true` / `false` |
| `u64` | 64-bit unsigned integer |
| `i64` | 64-bit signed integer |
| `f64` | IEEE 754 double-precision float |
| `str` | String (interned literal or runtime-built) |
| `ptr` | Raw heap pointer (0 = null) |
| `usize` | Reserved keyword, used in some builtin signatures |
| `dict[K, V]` | Hash dictionary, any `Object`-keyable type as `K` |
| `[T; N]` | Fixed-size array of `T` with length `N` |
| `[T]` | Dynamic-array slice (returned by slicing) |
| `(T1, T2, ...)` | Tuple — heterogeneous, fixed-arity |
| `Self` | The enclosing struct type within an `impl` block |
| `null` | Bottom type carried by `Object::Null(T)` for typed-null values |

Composite/user-defined types:

| Type | Description |
|---|---|
| `Name` | User-defined struct or enum |
| `Name<T1, ...>` | Generic struct or enum instantiation |
| `Allocator` | Opaque allocator handle (see [Allocators](#allocators)) |

### Type inference

Local variable annotations are optional when the rhs is unambiguous:

```rust
val a = 42        # u64 by default
val b: i64 = 42   # context infers i64; literal is silently widened/converted
val c = 3.14f64   # f64
```

Without an annotation, integer literals default to `u64`. With an
annotation that conflicts (`val x: bool = 42`), the type checker errors.

---

## Literals

### Integer literals

```
42u64       # u64
42i64       # i64
0xFFu64     # hex u64
0xFFi64     # hex i64
0xFF        # untyped Number, resolved by context (default u64)
42          # untyped Number, resolved by context
-3i64       # i64 with leading minus inside the lexer
```

### Float literals

Float literals always require the `f64` suffix to disambiguate them from
tuple-access syntax (`outer.0.1`):

```
3.14f64
42f64       # = 42.0f64
-2.5f64
```

A bare `1.5` is **not** a valid token in this language. To convert an
integer to a float, use `as`:

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

`null` carries a type at runtime (`Null(T)`). The type comes from the
binding or value position the null is assigned into.

### String literals

```rust
"hello"     # ConstString — interned, immutable
```

Multi-line string literals are not yet supported.

### Array, tuple, and dict literals

```rust
val arr: [i64; 3] = [1i64, 2i64, 3i64]
val tup           = (1u64, true, 3.5f64)
val dict          = dict{"a": 1u64, "b": 2u64}
```

---

## Expressions

### Operators

Listed lowest precedence first:

| Operator | Notes |
|---|---|
| `\|\|` | Logical OR (short-circuit) |
| `&&` | Logical AND (short-circuit) |
| `==` `!=` `<` `<=` `>` `>=` | Comparison; result is `bool` |
| `\|` `^` `&` | Bitwise (integer) |
| `<<` `>>` | Shift; rhs must be `u64` |
| `..` | Range expression `start..end` (half-open) |
| `+` `-` | Add / subtract (also `+` for `str` concat in the type checker) |
| `*` `/` `%` | Multiply / divide / remainder |
| Unary `-` | Negation (`i64`, `f64` only) |
| Unary `!` | Logical not (`bool`) |
| Unary `~` | Bitwise not (`u64`, `i64`) |
| `as` | Type cast (i64 ↔ u64, i64/u64 ↔ f64) |
| `.field` `.0` `.method(...)` | Field / tuple-index / method access |
| `[...]` | Indexing / slicing (arrays, dicts, structs with `__getitem__`) |

Compound assignment desugars at parse time: `x += 1` is rewritten to
`x = x + 1`. Supported forms: `+=`, `-=`, `*=`, `/=`, `%=`. The lhs may
be an identifier or a field/index access.

### Numeric semantics

- **Integer arithmetic**: standard two's-complement, panics on overflow
  in debug builds (Rust default).
- **Integer division and `%`**: truncated; `(-7) % 3 == -1`.
- **Float arithmetic**: standard IEEE 754. NaN compares false against
  everything (matching Rust's `PartialOrd`).
- **`as` casts**:
  - `i64 ↔ u64`: bit-preserving reinterpretation.
  - `f64 → i64/u64`: truncate toward zero, saturate on out-of-range,
    NaN becomes 0 (matching Rust's `as` since 1.45).
  - `i64/u64 → f64`: nearest-rounding conversion.

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
with allocator = arena_allocator() {
    # heap_alloc inside this block uses `arena_allocator`
}
```

See [Allocators](#allocators).

---

## Statements

### Variable declarations

```rust
val a: i64 = 7i64       # immutable; required initializer
val b      = 7i64       # type inferred from rhs
var c: u64 = 0u64       # mutable
var d                   # uninitialized (typed null until first assign)
```

`val` produces a binding that cannot be reassigned. `var` permits later
`=` assignment.

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
any function that references a `const` — see [`JIT.md`](../JIT.md).

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
while cond { ... }
break
continue
return                       # returns Unit
return value                 # returns a value
```

`break` / `continue` apply to the innermost enclosing loop; labelled
break is not implemented.

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

- Return type is required (use `()` implicitly by omitting the trailing
  expression to return Unit).
- Parameters require explicit types.
- The last expression in the body is the return value (no implicit
  `return` statement needed).

### Generic parameters and bounds

```rust
fn identity<T>(x: T) -> T { x }

fn run<A: Allocator>(a: A) -> u64 {
    with allocator = a { ... }
    0u64
}
```

Generic parameters appear in `<...>` after the function name. Bounds use
the `<T: Bound>` syntax. The only bound currently recognised is
`Allocator` (see [Allocators](#allocators)). Bound propagation:

- Function-level bounds are visible inside the body.
- `struct Name<T: Allocator>` propagates `T: Allocator` into every `impl`
  method.
- `impl<T: Allocator> Name<T>` likewise.
- Calls verify the caller's argument type satisfies any callee bound;
  bounded generic parameters compose transitively.

### Visibility

```rust
pub fn add(a: u64, b: u64) -> u64 { ... }   # exported from a module
fn helper() -> u64 { ... }                  # private (default)
```

### Calling convention

Arguments are evaluated left-to-right. All values are passed by
`Rc<RefCell<Object>>` reference at runtime — the language has no
explicit reference / pointer to a binding (`ptr` is for raw heap
addresses, not for taking the address of a local).

### Trailing-allocator inference

When a function takes a final parameter of type `Allocator` (or a
generic bounded by `Allocator`), the caller may omit it and the type
checker injects `ambient` (i.e. `__builtin_current_allocator()`):

```rust
fn alloc_block<A: Allocator>(size: u64, a: A) -> ptr { ... }

# Caller:
val p = alloc_block(64u64)            # `a` filled by ambient
val q = alloc_block(64u64, my_arena)  # explicit allocator
```

### Design-by-Contract clauses

`requires` (preconditions) and `ensures` (postconditions) follow the
return type. See [Design by Contract](#design-by-contract).

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

### Field access and assignment

```rust
val p = Point { x: 3i64, y: 4i64 }
val x = p.x                         # read
var q = p
q.x = 5i64                          # write to a `var`
```

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
re-uses the parameter declared on `struct`.

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

### `__drop__`

A struct can declare a `__drop__(self: Self)` method that runs at
end-of-scope. The destructor mechanism is the same one the allocator
system uses for arena cleanup.

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
- `42i64`, `true`, `"hello"` — literal patterns for primitives

Each arm is an expression; all arms must produce the same type.

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

The bound system currently recognises `<A: Allocator>` only; trait/
interface declarations are not yet supported.

---

## Modules

### Declaration

```rust
package math.basic

pub fn add(a: u64, b: u64) -> u64 {
    a + b
}
```

The `package` declaration is optional and, when present, must be the
first non-comment line. Path components are dot-separated identifiers.

### Import

```rust
import math.basic           # bare import
import math.basic as m      # aliased import
```

Imported modules are resolved by file path: `import foo` looks for
`modules/foo/foo.t` relative to the current working directory.

### Qualified identifiers

```rust
math::basic::add(1u64, 2u64)
m::add(1u64, 2u64)              # via alias
```

`::` is the scope-resolution operator; `.` is field/method access only.

---

## Allocators

The allocator system gives `with allocator = expr { body }` lexical
control over which allocator backs heap operations inside the body.

```rust
val arena = __builtin_arena_allocator()
with allocator = arena {
    val p = __builtin_heap_alloc(64u64)   # served by `arena`
    # ... use p ...
}
# `arena` drops here; everything it allocated is freed in one go.
```

### Allocator type

`Allocator` is an opaque handle. Two values are equal iff cloned from
the same `Rc` (`==` and `!=` only — no ordering).

### Built-in allocator constructors

| Builtin | Returns |
|---|---|
| `__builtin_default_allocator()` | The process-wide global allocator |
| `__builtin_arena_allocator()` | A fresh arena (bulk-free on drop) |
| `__builtin_fixed_buffer_allocator(cap: u64)` | Bounded by `cap` bytes; overflow returns null |
| `__builtin_current_allocator()` | The allocator at the top of the active stack |
| `ambient` | Sugar for `__builtin_current_allocator()` |

### `<A: Allocator>` bound

Functions, structs, and impl blocks may take a generic parameter
bounded by `Allocator`. The bound is checked at the call site and
propagated transitively.

```rust
fn collect<A: Allocator>(items: [u64; 4], a: A) -> ptr {
    with allocator = a {
        __builtin_heap_alloc(32u64)
    }
}
```

### `with` semantics

- `with allocator = expr { body }` evaluates `expr`, requires it to be
  an `Allocator`, pushes it onto the active stack for the duration of
  `body`, and pops on every exit path (value, return, break, error).
- Nested `with` works as a stack; `ambient` always sees the innermost.
- The body's type is the body block's type.

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

`__builtin_ptr_read` is type-polymorphic: it returns the type required
by its surrounding context (the lhs annotation of `val v: T = ...`,
typically). `__builtin_ptr_write` accepts any type.

---

## Built-in functions and methods

### Output

```rust
print(value)        # to stdout, no newline
println(value)      # to stdout + newline
```

Both accept any type; rendering goes through `Object::to_display_string`
(strings are unquoted, structs/dicts deterministic via sorted keys).
These are user-facing names without the `__builtin_` prefix.

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

### Type introspection

```rust
__builtin_sizeof(value: T) -> u64
```

Returns the byte size of the argument's type. Primitives use fixed
widths (`u64`/`i64`/`f64`/`ptr` = 8, `bool` = 1); structs sum their
fields; enums account for a 1-byte tag plus payload; tuples and
arrays sum their elements.

### String methods

Method-call syntax on `str`:

| Method | Signature |
|---|---|
| `str.len()` | `-> u64` |
| `str.concat(other: str)` | `-> str` |
| `str.substring(start: u64, end: u64)` | `-> str` |
| `str.contains(needle: str)` | `-> bool` |
| `str.split(sep: str)` | `-> [str]` |
| `str.trim()` | `-> str` |
| `str.to_upper()` | `-> str` |
| `str.to_lower()` | `-> str` |

### `is_null` (universal)

```rust
val n: i64 = null
n.is_null()                # true
```

Available on any type; returns `bool`.

---

## Design by Contract

Functions and methods may declare preconditions and postconditions
between the return type and the body block.

```rust
fn divide(a: i64, b: i64) -> i64
    requires b != 0i64
    ensures  result * b == a
{
    a / b
}
```

Rules:

- `requires` clauses run on entry, with parameters in scope.
- `ensures` clauses run on exit, with the same parameters in scope plus
  the special identifier `result` bound to the return value.
- Multiple clauses of either kind are AND-composed; the failure
  diagnostic identifies the specific clause by 1-based index.
- Each clause must type-check as `bool`.
- Methods can use `self` in both clauses.
- Failures abort the call with `ContractViolation` and propagate to the
  process exit unless caught.

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

> **Operational guidance.** Keep `INTERPRETER_CONTRACTS=all` in
> production unless a clause has measurable performance cost. Disabling
> contracts (`pre` / `post` / `off`) is the very condition that tends
> to let latent bugs survive into release — the same reason D's
> `-release` flag is widely discouraged. The knob exists for hot-path
> benchmarks and other performance-sensitive runs; treat any other use
> as a deliberate, narrowly-scoped exception.

### Out of scope (planned)

- `old(...)` for snapshotting pre-state in `ensures`
- Named-tuple returns (`-> (q: i64, r: i64)`) for binding result
  components
- `invariant` clauses on `impl` blocks
- Static verification beyond runtime checking

---

## Runtime model

### Execution

The default backend is a tree-walking interpreter. An optional cranelift
JIT (cargo feature `jit`, default on) handles a numeric subset when
`INTERPRETER_JIT=1`; everything else falls back to the tree walker.
See [`JIT.md`](../JIT.md) for the supported subset and limitations.

### Process exit code

`main`'s integer return value becomes the process exit code:

- `Object::UInt64(v)` or `Object::Int64(v)` → `v as i32`.
- Other return types → 0.

### Errors

Runtime errors are formatted with source-location context where
possible. Categories include: `TypeError`, `UndefinedVariable`,
`ImmutableAssignment`, `IndexOutOfBounds`, `NullDereference`,
`ContractViolation`, and a generic `InternalError` reserved for
interpreter bugs.

### Recursion

The interpreter limits recursion depth to 1000 frames to prevent stack
overflow on cyclic structures.

---

## Known limitations

These are real today; some appear in `todo.md` as planned work.

- **No closures or lambdas** — functions are not first-class values
  outside `fn`-named declarations.
- **No traits / interfaces** — only the built-in `Allocator` bound is
  recognised. Trait declarations are not yet a thing.
- **No `else if`** — use `elif`.
- **No bare `self`** — `self: Self` is mandatory in method signatures.
- **`val` is a keyword** — cannot be used as a parameter or field name.
- **`val name: TypeName = StructLiteral`** does not always typecheck;
  prefer `val name = StructLiteral` (let inference handle it).
- **Float literals require the `f64` suffix** — `1.5` is not a token;
  write `1.5f64`.
- **No labelled break / continue** — only the innermost loop is
  affected.
- **`panic` / `assert` are always active by design** — there is no
  env-var to disable them in release builds. Stripping assertions in
  production is the failure mode D's `-release` flag is criticised
  for; this language deliberately keeps them on so that invariant
  violations surface the same way regardless of build profile. (The
  `INTERPRETER_CONTRACTS` gate exists only because contract clauses
  can carry non-trivial cost; even there, `all` is the recommended
  setting — see "Operational guidance" above.)
- **No string interpolation, raw strings, multi-line strings**.
- **Modules resolve only on the local filesystem** under
  `modules/<name>/<name>.t`.

---

## See also

- [`README.md`](../README.md) — project overview and quickstart
- [`interpreter/README.md`](../interpreter/README.md) — interpreter CLI
  and environment variables
- [`JIT.md`](../JIT.md) — cranelift JIT details
- [`ALLOCATOR_PLAN.md`](../ALLOCATOR_PLAN.md) — allocator design
- [`BUILTIN_ARCHITECTURE.md`](../BUILTIN_ARCHITECTURE.md) — builtin
  function machinery
- [`todo.md`](../todo.md) — planned work and feature backlog
