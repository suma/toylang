# compiler

The toylang AOT compiler. It turns a `.t` source file into a native
executable through Cranelift, and it hosts a second Cranelift backend —
an in-process JIT — that the cross-backend test suites and
`--all-backends` run against.

## Usage

```bash
cargo run -p compiler -- input.t -o output
./output; echo $?
```

`main`'s return value (`u64` / `i64`) becomes the process exit status.
A POSIX shell truncates that to the low 8 bits, so anything at or above
256 wraps.

### CLI flags

| Flag | Meaning |
|---|---|
| `<file>` | Input source (required). `-` reads the program from stdin |
| `-o <path>` | Output path, whatever `--emit` produces. Defaults to the input path with the extension replaced: none for `exe`, `.o` / `.ir` / `.clif` for the rest |
| `--emit <kind>` / `--emit=<kind>` | `exe` (default) / `obj` / `ir` / `clif` |
| `--release` | Skip design-by-contract checks (`requires` / `ensures`) — the build-flag equivalent of `INTERPRETER_CONTRACTS=off` |
| `--format=text\|json` | Output shape. `text` (default) prints diagnostics as a snippet with a caret; `json` prints the result (what was built, or the `--all-backends` verdict) as one document on stdout, diagnostics as an array on stderr with spans and suggestions, and the `--profile=mem` report as JSON. Replaces the former `--diagnostics` and `--profile-format` |
| `-v` / `--verbose` | Progress log to stderr |
| `--core-modules <DIR>` / `--core-modules=<DIR>` | Override the core-modules directory |
| `--all-backends` | Run the program on the tree-walker, this crate's JIT and AOT, and report only disagreements (one stderr line when they agree) |
| `--profile=mem` | Print an allocation profile (totals + leaks + allocator layout) to stderr after the run |
| `-h` / `--help` | Usage |

### Environment variables

| Variable | Meaning |
|---|---|
| `TOYLANG_CORE_MODULES` | Core-modules directory (empty string opts out) |
| `TOY_CACHE_DIR` | Incremental-cache root (default `.toycache/`) |
| `TOY_CACHE_DISABLE=<non-empty>` | Skip both cache load and save |
| `TOY_LINK_CACHE_DIR` | Content-addressed cache of linked binaries (see *Link cache*). Unset in production, set for the test suite by `.cargo/config.toml` |
| `TOYLANG_CRANELIFT_OPT_LEVEL` | `speed` (default) / `none` / `speed_and_size` |
| `TOY_PROFILE_MEM` | Make a generated AOT binary profile itself (`1` = text, `json` = JSON). Byte-identical to the interpreter's `--profile=mem` |
| `CC` | C compiler used for the link step (default `cc`) |

### Core modules (auto-load)

At start-up the compiler recursively integrates everything under
`core/`, so `math::sin(x)`, `String`, `Vec<T>` and the rest are callable
without an `import` line. Resolution order:

1. `--core-modules` flag
2. `TOYLANG_CORE_MODULES` environment variable
3. Executable-relative search: `<exe>/core/` → `<exe>/../share/toylang/core/`
   → `<exe>/../../core/`

Running `target/debug/compiler` straight out of the dev tree finds the
repository's `core/` through the third candidate.

### Incremental-compilation cache

The front end is reached through
`interpreter::check_typing_with_core_modules`, so the Full AST cache
applies unchanged. Each core module's `File` plus its module-local
interner is stored at
`<cache_dir>/<hash_prefix>/<source_hash>.full`, and later runs skip the
parse. A schema mismatch, a corrupt entry or a missing file is a silent
fall-back rather than an error. Design notes:
[`design-docs/INCREMENTAL_COMPILATION.md`](../design-docs/INCREMENTAL_COMPILATION.md).

### Link cache

Setting `TOY_LINK_CACHE_DIR` turns the link step into a lookup: the
compiler hashes the toylang object bytes, the embedded runtime archive,
the `cc` selection and the platform flags, and copies
`<dir>/<hash>.bin` into place on a hit instead of invoking `cc`. On
macOS the dominant cost being skipped is the Mach-O ad-hoc code-signing
pass (~150–300 ms per binary on Apple Silicon).

**This only works because codegen is reproducible.** The key is a hash
of the object bytes, so output that drifts between runs turns the cache
into a write-only directory — which is exactly what happened while two
passes iterated a `HashMap`. `tests/reproducible_build.rs` pins the
property, because a comment cannot.

## What is supported

The AOT backend covers essentially the whole language: every scalar
type including the narrow ints and `f32`, structs, tuples, arrays
(including `soa`), enums and `match`, traits (`dyn Trait` included),
generics, closures, `str` and `String`, allocators, design by contract,
`extern fn`, SIMD vectors, and the pointer/window types. The language
reference is [`docs/language.md`](../docs/language.md); per-feature
history is in [`design-docs/todo.md`](../design-docs/todo.md).

The authoritative list of what the AOT backend *cannot* build is the
`AOT_UNSUPPORTED` array in `tests/example_consistency.rs`. It is checked
**in both directions** — an entry that starts working fails the test, so
the list cannot outlive the gaps it describes. The ones worth knowing
here:

- **`dict` is interpreter-only.** Excluded from the three-way
  comparison entirely.
- **`f64` `%`** — Cranelift has no native `fmod`, and the compiler says
  so rather than emitting something approximate.
- **Reassigning a whole struct / tuple binding** (`p = P { .. }`).
  Assigning to a leaf field works.
- **`const` initialisers must fold to a scalar literal** or name an
  earlier `const`. `const fn` calls and arithmetic *do* work, because
  the front end folds them before lowering; a `const NAME: str = "..."`
  does not.
- **A compound-returning call in expression position** is sometimes
  rejected — binding it with `val` first works.
- **Operator overloads only in `let`-rhs position** (`val r = a + b`).
  Chains, literal operands, argument and condition positions are
  interpreter-only, so a program that works there can still fail here.
- **Closures capture scalars only.** A struct / tuple / array / dict
  capture is rejected by name.

Note that `bool` ↔ integer `as` casts are *not* on this list: those are
rejected by the type checker (`[E0010] Cannot cast Bool to UInt64`), so
they fail identically on every backend rather than being a gap here.

## Memory profiling

```bash
# Make a compiled binary profile itself (byte-identical to the interpreter)
TOY_PROFILE_MEM=1 ./fib

# Check that all three backends report the same numbers, in one command
cargo run -p compiler -- interpreter/example/box_linked_list.t \
    --all-backends --profile=mem
```

```text
memory profile
  alloc_count       3
  free_count        3
  realloc_count     0
  cumulative_bytes  72
  live_bytes        0
  peak_live_bytes   72
  peak_at_request   3
```

Counting is **request-based** — what the program asked for, not what an
allocator did with it — which is why every backend produces the same
figures. The counter builtins (`__builtin_live_bytes()` and five
others) are readable from `requires` / `ensures` / `test` and return
real numbers without any profiling flag, so **memory behaviour can be
stated as a contract** (`ensures __builtin_live_bytes() <= 4096u64`).
See "Allocation counters" in [`docs/language.md`](../docs/language.md)
and [`design-docs/MEMORY_PROFILING.md`](../design-docs/MEMORY_PROFILING.md).

## Design

The pipeline is **AST → IR → Cranelift IR → object bytes**. The
mid-level IR is what keeps backend concerns out of the AST, and it is
the layer where passes such as constant propagation, inlining and
devirtualization belong.

Most of that pipeline no longer lives in this crate. Lowering moved out
to `compiler_lower` so the interpreter can drive it too — the
interpreter cannot depend on `compiler`, which depends on it — and the
IR definitions to `compiler_ir`. Both are re-exported here
(`compiler::lower`, `compiler::ir`) for source compatibility.

| Crate / file | Role |
|---|---|
| `compiler_ir` | IR definitions (`Module` / `Function` / `Type` / `Linkage`) and layout |
| `compiler_lower` | AST → IR (29 modules; `program.rs` is the entry point) |
| `compiler_core` | `CompilerSession` — the shared front-end driver |
| `compiler_vm` | The IR VM, which is what the `interpreter` binary runs by default |
| `src/main.rs` | CLI |
| `src/lib.rs` | `compile_file()` / `resolve_core_modules_dir()` / `read_input()` |
| `src/options.rs` | `CompilerOptions` / `EmitKind` |
| `src/codegen/` | IR → Cranelift IR, `.o` emission, SIMD lowering |
| `src/jit.rs` | The in-process Cranelift JIT (`compile_to_jit_main`) |
| `src/driver.rs` | Linking through `cc`, plus the link cache |
| `src/all_backends.rs` | `--all-backends`: run everywhere, report disagreements |
| `src/cache.rs` | The module-interface cache — a module's public surface, bodies stripped, keyed by the SHA-256 of its source |
| `build.rs` | Pre-builds `runtime/toylang_rt/` into a staticlib with `rustc` and embeds it |

The front end and type checker are reused through
`compiler_core::CompilerSession` and
`interpreter::check_typing_with_core_modules`, so this crate, the
interpreter and both JITs see exactly the same checks.

`runtime/toylang_rt/` is the AOT runtime, written in Rust rather than C
and compiled `no_std` and dependency-free. `build.rs` embeds the
archive with `include_bytes!` so an AOT compile never builds it.
Because `lib.rs` now pulls in a platform-selected `sys_*.rs`, the build
script watches the whole source **directory** — naming only the crate
root left the staticlib stale while cargo happily rebuilt the JIT's
rlib, which surfaces as a backend disagreement nowhere near its cause.

### `str` runtime layout

A `str` value is the address of the trailing length field in

```text
[N bytes (UTF-8)] [1 byte NUL] [u64 len (8 bytes, LE)]
```

so `__builtin_str_len(s)` is `load.i64(s, 0)` and
`__builtin_str_to_ptr(s)` is `s - 1 - len`. The NUL is there so a C
callee can read the bytes as a `const char*`; pointing the value at the
length field instead of the bytes keeps heap-allocated strings
pointer-uniform with `.rodata` ones.

### `--emit=ir` output

`fn fib(n: u64) -> u64` plus `fn main() -> u64 { fib(8u64) }` lowers to:

```text
local function toy_fib(@l0: u64) -> u64 {
  locals:
    @l1: u64
  bb0:
    %v0: u64 = load @l0
    %v1: u64 = const 1u64
    %v2: bool = le %v0, %v1
    br %v2, bb2, bb3
  bb1:
    %v15: u64 = load @l1
    ret %v15
  bb2:
    %v3: u64 = load @l0
    store @l1, %v3
    jump bb1
  bb3:
    %v4: u64 = load @l0
    %v5: u64 = const 1u64
    %v6: bool = ge %v4, %v5
    br %v6, bb4, bb5
  bb4:
    %v7: u64 = sub %v4, %v5
    %v8: u64 = call fn#0(%v7)
    %v9: u64 = load @l0
    %v10: u64 = const 2u64
    %v11: bool = ge %v9, %v10
    br %v11, bb6, bb7
  bb5:
    panic_values #0 %v4, %v5
  bb6:
    %v12: u64 = sub %v9, %v10
    %v13: u64 = call fn#0(%v12)
    %v14: u64 = add %v8, %v13
    store @l1, %v14
    jump bb1
  bb7:
    panic_values #0 %v9, %v10
}

export function main() -> u64 {
  bb0:
    %v0: u64 = const 8u64
    %v1: u64 = call fn#0(%v0)
    ret %v1
}
```

Two things a first reading tends to trip over. The `bb5` /
`panic_values` blocks are the RUNTIME-TRAP guards — here, the `u64`
underflow check on `n - 1u64` — and a `requires` clause that already
rules the trap out removes them (CONTRACT-ELISION). And the dump ends
with a long tail of declarations whose bodies are empty: lowering is
demand-driven, so every stdlib function that this program does not
reach is declared and never lowered.

## Tests

| File | Tests | Contents |
|---|---:|---|
| `tests/consistency/` | 556 | Four-lane agreement (tree-walker / IR VM / AOT / JIT), grouped into 27 modules by feature |
| `tests/e2e.rs` | 36 | source → compile → spawn → compare exit code |
| `tests/jit_smoke.rs` | 15 | This crate's Cranelift JIT, in-process |
| `tests/example_consistency.rs` | 14 | Every program in `interpreter/example/` (178 files, minus two audited skip lists) across three backends, sharded |
| `tests/e2e_batched.rs` | 12 | Small samples batched into one spawn |
| `tests/ffi_tests.rs` | 7 | `extern fn` against real C symbols |
| `tests/reproducible_build.rs` | 4 | Object and CLIF bytes are identical run to run |
| `tests/all_backends_cli.rs` | 4 | The `--all-backends` CLI surface itself |
| `tests/net_abi_tests.rs` | 2 | Hand-written syscall constants checked against a C probe built with `cc` |

652 tests in total. All the integration tests share **one** binary:
the crate sets `autotests = false` and `tests/suite.rs` pulls each file
in with `#[path]` (the remaining two are unit tests inside the lib). A
new test file that is not registered in `suite.rs` is not a failure —
it is silently never run.

```bash
cargo nextest run -p compiler                      # parallel (recommended)
cargo nextest run -p compiler -E 'test(name)'      # substring filter, not exact match
COMPILER_E2E=skip cargo nextest run -p compiler    # skip everything that needs cc
```

`TOYLANG_CRANELIFT_OPT_LEVEL=none` is injected by `.cargo/config.toml`'s
`[env]` block, which makes codegen roughly **20x** faster; the
production path defaults to `speed`. That setting used to live in
`.config/nextest.toml` under `[profile.default.env]`, a key nextest does
not read — it warned on every run and never applied. Environment
variables for tests belong in `.cargo/config.toml`.

What dominates parallel wall-clock is AOT codegen, linking and process
spawn (~50 ms per test even with a warm cache), which is why both the
link cache and the pre-built runtime archive exist. See TEST-PERF and
BUILD-PERF in [`design-docs/todo.md`](../design-docs/todo.md) for the
measurements and what is left to shave.
