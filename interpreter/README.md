# interpreter

Tree-walking interpreter for the toy language defined in `frontend/`. Includes
an optional cranelift-based JIT (default-on cargo feature; opt-in at runtime
via env var). See the workspace-root [`README.md`](../README.md) for the
language reference; this file documents only the binary's CLI and runtime
knobs.

## CLI

```
interpreter <file> [-v]
```

| Flag | Meaning |
|---|---|
| `<file>` | Required. Source file to parse, type-check, and execute. By convention `*.t`. |
| `-v` | Verbose mode. Prints "Parsing source file: …", "Performing type checking", "Executing program" between phases, and any JIT decisions ("JIT compiled: …" or "JIT: skipped (…)" with a reason). |

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

## Environment variables

All env-vars are read once at process start. Unset = the default in the table.

| Variable | Values | Default | Effect |
|---|---|---|---|
| `INTERPRETER_JIT` | `1` (any other value = off) | unset (off) | When `1`, eligible functions are compiled to native code via cranelift before execution. Ineligible functions silently fall back to the tree walker. Requires the `jit` cargo feature (on by default). With `-v`, each function prints either `JIT compiled: <name>` or `JIT: skipped (<reason>)`. |
| `INTERPRETER_CONTRACTS` | `all` \| `pre` \| `post` \| `off` (case-insensitive; `on`/`1`/`true` ≡ `all`, `0`/`false` ≡ `off`) | `all` | Selects which Design-by-Contract clauses run. `all` = both `requires` and `ensures`; `pre` = only `requires`; `post` = only `ensures`; `off` = neither (D's `-release` equivalent). Unrecognised values log a warning to stderr and fall back to `all` so a typo can't silently disable contracts. |

```bash
# Native code path (about 100×–1000× faster on numeric kernels)
$ INTERPRETER_JIT=1 cargo run --release example/fib.t

# Strip postcondition checks for a hot-path benchmark
$ INTERPRETER_CONTRACTS=pre cargo run --release example/contracts.t

# Disable all contract checks (release-mode build)
$ INTERPRETER_CONTRACTS=off cargo run --release example/contracts.t
```

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
