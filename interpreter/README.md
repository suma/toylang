# interpreter

Tree-walking interpreter for the toy language defined in `frontend/`. Includes
an optional cranelift-based JIT (default-on cargo feature; opt-in at runtime
via env var). See the workspace-root [`README.md`](../README.md) for the
language reference; this file documents only the binary's CLI and runtime
knobs.

## CLI

```
interpreter <file> [-v] [--core-modules <DIR>]
```

| Flag | Meaning |
|---|---|
| `<file>` | Required. Source file to parse, type-check, and execute. By convention `*.t`. |
| `-v` / `--verbose` | Verbose mode. Prints "Core modules directory: …", "Parsing source file: …", "Performing type checking", "Executing program" between phases, and any JIT decisions ("JIT compiled: …" or "JIT: skipped (…)" with a reason). |
| `--core-modules <DIR>` (also `--core-modules=<DIR>`) | Override the core-modules directory the interpreter auto-loads at startup. See *Core modules* below. |

The exit code is the integer returned by `main`:

- `Object::Int64(v) as i32` or `Object::UInt64(v) as i32` becomes the process
  exit code (so `fn main() -> u64 { 7u64 }` exits with status 7).
- Any other return type exits 0.
- Parse / type-check / runtime errors are formatted to stderr and exit 1.

```bash
$ cargo run example/fib.t          # exits 8 (the 6th Fibonacci)
$ cargo run example/contracts.t    # exits 22
$ cargo run example/fib.t -v       # show pipeline phases on stderr
```

## Core modules (auto-load)

At startup the interpreter resolves a **core-modules directory**
and integrates every `.t` file under it before parsing the user's
source. Functions exported from those files are reachable through
the qualified `module::name(...)` form without any `import` line —
this is how the standard `math::sin(x)` / `math::sqrt(x)` /
`i64.abs()` / `f64.abs()` surfaces become available.

Resolution priority (the first hit wins):

1. `--core-modules <DIR>` CLI flag.
2. `TOYLANG_CORE_MODULES` env var. Set to the empty string
   (`TOYLANG_CORE_MODULES=`) to opt out entirely — auto-load
   becomes a no-op and only the embedded prelude (currently empty)
   stays in scope.
3. Executable-relative search:
   - `<exe_dir>/core/`
   - `<exe_dir>/../share/toylang/core/`
   - `<exe_dir>/../../core/` (the dev-tree fallback —
     `target/debug/interpreter` finds `<repo>/core/` here).

Module paths come from the file system layout: `core/std/math.t`
becomes module `["std", "math"]` aliased as `math` (the alias is
always the last path segment). Run with `-v` to confirm which
directory the binary picked up.

```bash
# Use a custom core directory
$ cargo run -- example/fib.t --core-modules /path/to/my-core

# Disable auto-load entirely (only the embedded prelude is active)
$ TOYLANG_CORE_MODULES= cargo run example/fib.t
```

Auto-loaded modules use `enforce_namespace = false`, so a user-
defined `fn add(...)` shadows a same-named stdlib export for bare
calls; the qualified form (`<alias>::add`) keeps working through
the synthetic `ImportDecl` the auto-load path inserts.

## Environment variables

All env-vars are read once at process start. Unset = the default in the table.

| Variable | Values | Default | Effect |
|---|---|---|---|
| `INTERPRETER_JIT` | `1` (any other value = off) | unset (off) | When `1`, eligible functions are compiled to native code via cranelift before execution. Ineligible functions silently fall back to the tree walker. Requires the `jit` cargo feature (on by default). With `-v`, each function prints either `JIT compiled: <name>` or `JIT: skipped (<reason>)`. |
| `INTERPRETER_CONTRACTS` | `all` \| `pre` \| `post` \| `off` (case-insensitive; `on`/`1`/`true` ≡ `all`, `0`/`false` ≡ `off`) | `all` | Selects which Design-by-Contract clauses run. `all` = both `requires` and `ensures`; `pre` = only `requires`; `post` = only `ensures`; `off` = neither (D's `-release` equivalent). Unrecognised values log a warning to stderr and fall back to `all` so a typo can't silently disable contracts. |
| `TOYLANG_CORE_MODULES` | path to a directory \| empty string | unset (uses exe-relative search) | Override the core-modules directory. Empty string opts out of auto-load entirely. Lower priority than the `--core-modules` CLI flag; see *Core modules* above. |

```bash
# Native code path (about 100×–1000× faster on numeric kernels)
$ INTERPRETER_JIT=1 cargo run --release example/fib.t

# Strip postcondition checks for a hot-path benchmark
$ INTERPRETER_CONTRACTS=pre cargo run --release example/contracts.t

# Disable all contract checks (release-mode build)
$ INTERPRETER_CONTRACTS=off cargo run --release example/contracts.t
```

> **Recommended setting: `all` (the default).** Disabling contracts
> can let invariant violations slip into release the same way D's
> `-release` flag does. Reach for `pre` / `post` / `off` only when a
> specific clause has measurable cost on a hot path. The `panic` and
> `assert` builtins have no analogous gate by design — they are always
> active so safety checks behave identically across build profiles.

## Build features

Set in `Cargo.toml`. Toggle with `--features` / `--no-default-features`.

| Feature | Default | Effect |
|---|---|---|
| `jit` | on | Pulls in cranelift and compiles JIT support into the binary. Disabling shrinks the binary and removes the `INTERPRETER_JIT` code path entirely (the env var becomes a no-op). Build with `--no-default-features` to drop it. |
| `debug-logging` | off | Activates the runtime destruction-tracking log used by some tests. Adds a small per-drop cost. Implied by debug builds. |
| `test-logging` | off | Forces `debug-logging` on under `cargo test`, useful when reproducing intermittent destruction-related test output. |

```bash
$ cargo build                                 # default: with JIT
$ cargo build --release                       # default + optimised
$ cargo build --no-default-features           # no JIT (cranelift dropped)
$ cargo test --features test-logging          # noisier destruction log
```

## Examples

`example/` contains runnable programs covering most language features. A few
high-signal ones:

| File | Demonstrates |
|---|---|
| `fib.t` | Recursive Fibonacci (the canonical micro-benchmark). |
| `contracts.t` | `requires` / `ensures` on functions and methods, with `result`. |
| `float64.t` | f64 arithmetic, comparisons, and `as` casts. |
| `match_guard.t` | Pattern matching with per-arm `if` guards. |
| `tuple_destructure_nested.t` | `val ((a, b), c) = …` style destructuring. |
| `allocator_basic.t` / `allocator_list.t` | `with allocator = …` scopes and a user-space `List<T>`. |
| `jit_*.t` | Programs hand-tuned to land on the JIT happy path; pair with `INTERPRETER_JIT=1 -v` to confirm. |

## Tests

```bash
$ cargo test                                  # all tests, default features
$ cargo test --test contract_mode_tests       # env-var matrix only
$ cargo test --test jit_integration           # JIT vs interpreter parity
$ cargo test --no-default-features            # interpreter-only path
```

The integration tests under `tests/` spawn the compiled binary so they
exercise the same env-var and CLI surfaces that users do. The matrix in
`contract_mode_tests.rs` walks every `INTERPRETER_CONTRACTS` value;
`jit_integration.rs` runs each example with and without `INTERPRETER_JIT=1`
and asserts the two modes produce byte-identical output.
