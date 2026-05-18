# A5-P2 / P3 / P4: `dyn Trait` backend support (design)

A5-P1 (2026-05-18, commit `eb32c3a`) landed `&dyn Trait` dispatch
in the interpreter. P2 / P3 / P4 extend the same surface to the
AOT compiler, the cranelift JIT, and owned trait objects via
`Box<dyn Trait>`. This document is the implementation plan.

## Status snapshot

| Phase | Backend / scope | Status |
|---|---|---|
| **P1** | interpreter dispatch | ✅ landed (`eb32c3a`) |
| **P2** | AOT compiler | planning (this doc) |
| **P3** | cranelift JIT (compiler-side + interpreter-side) | planning |
| **P4** | `Box<dyn Trait>` (owned trait objects) | not started |

## Why this is large

The interpreter side fits in a single session because every value is
already a typed `Rc<RefCell<Object>>` — `dyn Trait` dispatch is just
"look up the method on the object's underlying type". The AOT compiler
flattens struct values to leaf scalars at every function boundary
(see `compiler/src/lower/method_call.rs::lower_method_call`,
`compound_storage.rs::flatten_struct_locals`). That ABI does **not**
support hetero-typed values sharing the same parameter slot, so
`fn describe(a: &dyn Animal)` cannot accept both `Dog` and `Cat`
leaves under the existing scheme.

P2 introduces a parallel **fat-pointer ABI** for `dyn` parameters,
keeping the flat-leaf ABI for every non-dyn call. The two ABIs
coexist at the IR level; the choice is made per-parameter based on
whether the lowered type is `Dyn`.

## P2-MVP-A: empty struct only (next session target)

Smallest end-to-end slice that exercises every new piece:

```rust
trait Animal { fn sound(self: Self) -> i64 }
struct Dog {}
struct Cat {}
impl Animal for Dog { fn sound(self: Self) -> i64 { 1i64 } }
impl Animal for Cat { fn sound(self: Self) -> i64 { 2i64 } }
fn describe(a: &dyn Animal) -> i64 { a.sound() }
fn main() -> i64 { describe(Dog {}) + describe(Cat {}) }   # 3
```

Empty struct = zero receiver leaves = no thunk needed.

### Pieces

1. **IR representation of `&dyn Trait`** —
   intern `Type::Tuple([U64, U64])` for the (data_ptr, vtable_ptr)
   pair. Falls out of existing tuple infrastructure
   (`compiler/src/lower/types.rs::intern_tuple`). Function param /
   return type for `&dyn Trait` lowers to this `Type::Tuple`.

2. **Vtable data symbols** —
   one cranelift `DataId` per `(trait_sym, struct_sym)` pair.
   Symbol naming: `toy_vtable_<trait_name>_<struct_name>`.
   Content: a sequence of function pointers, one per trait method,
   in trait declaration order. Emitted via `module.declare_data`
   + `define_data` with `add_data_relocation` for each function
   address.

   For MVP-A, vtable entries point directly at the impl's
   `FuncId` because empty-struct methods have signature
   `() -> R` (no self leaves) and the indirect call from the
   dyn site is also `() -> R`. No thunks.

3. **Coercion site** —
   when an arg of type `Struct(Empty, [])` (after auto-borrow) is
   passed to a parameter of type `Ref { inner: Dyn(trait_sym) }`,
   the lower pass emits:
   - `data_ptr = const U64 0` (null sentinel — empty struct
     carries no data)
   - `vtable_ptr = AddressOf(toy_vtable_<trait>_<struct>)`
   - construct the tuple value and pass it

4. **Method dispatch** —
   on a method call where receiver type is
   `Ref { inner: Dyn(trait_sym) }`:
   - extract data_ptr (tuple field 0) and vtable_ptr (field 1)
   - load fn_ptr = `*(vtable_ptr + method_idx * 8)`
     where method_idx is the position of the called method in
     the trait's declaration list
   - emit `InstKind::CallIndirect { callee: fn_ptr,
     args: vec![/* data_ptr if empty: passed but unused */, ...],
     param_tys: signature_per_method, ret_ty }`
   - for MVP-A, since empty-struct methods take no self leaves,
     `args` is just the method's regular args (no data_ptr).
     `param_tys` mirrors the trait method signature.

5. **Trait method index resolution** —
   thread the trait's method ordering into the lower pass.
   `context.traits` in the type checker already records this in
   declaration order; expose it through `compiler_core::TypeCheckResults`
   or recompute by walking `Stmt::TraitDecl` in `lower_program`.

### Modules touched (estimated)

| File | Change |
|---|---|
| `compiler/src/lower/types.rs` | `lower_scalar` keeps returning `None` for `Dyn`; `lower_param_or_return_type` gains a `Ref { inner: Dyn(_) }` arm that interns `Type::Tuple([U64, U64])` |
| `compiler/src/lower/program.rs` | scan `Stmt::ImplBlock { trait_name: Some(t), .. }` after method declarations are in; per impl, declare a vtable `DataId`; defer `define_data` until function `FuncId`s are known |
| `compiler/src/lower/expr.rs` | arg-coercion site: when expected param type is the fat-pointer tuple and actual value is a struct binding, emit data_ptr=null + vtable AddressOf + tuple construction |
| `compiler/src/lower/method_call.rs` | receiver kind detection: if receiver type lowers to `Type::Tuple([U64, U64])` AND its source TypeDecl is `Dyn(_)`, switch to vtable-dispatch path |
| `compiler/src/codegen/mod.rs` | new helper `define_vtable_data(data_id, func_ids)` using `DataDescription::set_value_relocs`; honor `Linkage::Local` for vtable symbols |
| `compiler/src/ir.rs` | possibly a dedicated `InstKind::VtableLoad { fat_ptr, method_idx }` for clarity, or do it inline via tuple-extract + ptr load |

### Out of scope for MVP-A

- structs with any fields (need thunks; MVP-B)
- `&mut dyn Trait` (writeback through fat pointer; MVP-C)
- `Box<dyn Trait>` (owned, heap-allocated; P4)
- multi-trait objects `dyn TraitA + TraitB`

### Risks

- **`Type::Tuple([U64, U64])` ambiguity** — a user's `(u64, u64)`
  tuple would lower to the same `TupleId`, indistinguishable from
  the fat pointer at the IR level. Either: (a) intern a dedicated
  `Type::FatPtr(trait_sym)` variant, or (b) thread the trait
  identity through call-site metadata. Option (a) is cleaner but
  touches every Type match.

- **Vtable layout finality** — once MVP-A ships, future phases
  (thunks for non-empty structs, `&mut` writeback) must not
  invalidate already-emitted vtables. Pin the layout in this doc
  before writing the codegen.

- **Cranelift relocation API surface** — `define_data` with
  function-address relocations is well-trodden territory, but
  the exact API call sequence may need a tracer-bullet experiment
  in `codegen/mod.rs` before it ships.

## P2-MVP-B: scalar-field struct via thunks

Builds on MVP-A. Adds:

- coercion: `&Cell { v: i64 }` heap-allocates an `i64` cell, stores
  `cell.v` into it, sets `data_ptr = heap_addr`
- thunk function per impl method: takes `(data_ptr: U64, args...)`,
  reads `cell.v = load(data_ptr)`, calls the underlying flat method
  with `(cell.v, args...)`, returns
- vtable entries point at thunks, not at the raw impl methods
- runtime: reuse `__builtin_heap_alloc` at the coercion site

Open question: when does the heap-allocated cell get freed? The
A5-P1 interpreter doesn't worry about this because values are
`Rc<RefCell<Object>>`. For AOT, an obvious answer is "leak it"
(matches Rust's `&dyn Trait` lifetime semantics: the trait object
borrows from a longer-lived owner). Better: lower `&dyn Trait` at a
call site to a stack-allocated cell in the caller's frame, so the
lifetime is bounded by the call. Cranelift stack slots (`ss0`,
`ss1`, ...) are the right primitive.

## P2-MVP-C: compound-field struct

Extends MVP-B to structs with multiple / nested fields. The thunk
unpacks the full leaf list from `data_ptr` (using existing
`flatten_struct_locals` shapes) before calling the impl method.

Layout in `data_ptr`: contiguous leaves at struct-natural offsets,
matching the same layout used by `__builtin_ptr_read` /
`__builtin_ptr_write` for `Vec<Compound>`.

## P3: cranelift JIT (compiler-side)

Mirrors P2 in `compiler/src/jit.rs`. The compiler JIT shares the
codegen pipeline with AOT, so most pieces transfer directly:
declare_data + define_data for vtables, indirect call lowering.

Open: the **interpreter-side JIT** (`interpreter/src/jit/`) is a
separate codebase. P3 covers it too if budget permits, but it can
fall back to "skip + interpreter" silently (this is already its
behavior for many features).

## P4: `Box<dyn Trait>`

Owned trait objects. Two routes:

- **`Box<dyn Trait>`** — single field box with heap ownership.
  Construction via `Box::new(value)`, drop semantics required.
- **`Vec<Box<dyn Trait>>`** — heterogeneous collection. Element
  type `Box<dyn Trait>` is sized (fat pointer to heap), so the
  existing `Vec<T>` infra handles the array side; only the
  element layout is new.

P4 is meaningful only after P2 lands. Defer the detailed plan to
post-P2-MVP-C.

## Test plan

Each MVP slice gets its own 3-way consistency test in
`compiler/tests/consistency.rs` (interpreter / cranelift JIT / AOT
agreement), parallel to the existing
`trait_default_body_*` / `multi_bound_*` tests.

Existing interpreter unit tests in
`interpreter/tests/trait_tests.rs::dyn_trait` already cover the
A5-P1 surface; no new interpreter tests are needed for P2 itself.
