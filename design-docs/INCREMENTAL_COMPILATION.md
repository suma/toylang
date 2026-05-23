# Incremental Compilation Design

## Goal

Enable toylang to avoid re-parsing and re-type-checking unchanged core
modules on every compilation, reducing startup latency for programs that
import the standard library.

## Current State (Post Phase 1-4)

- `ModuleInterface` — pool-reference-free public surface of a module
- `bincode` serialization / deserialization for `ModuleInterface`
- Cache directory layout: `.toycache/<hash_prefix>/<source_hash>.interface`
- **Phase 4 (2026-05-23): Full AST cache landed.** `CachedModule {
  schema_version, interner, file }` is serialized to
  `.toycache/<hash_prefix>/<source_hash>.full`. The fast path lives at
  the entry of `integrate_module_into_program_with_options_full` —
  cache hit deserializes a `File` + module-local
  `DefaultStringInterner` and feeds them into the same
  `AstIntegrationContext::integrate()` the cold path uses, so
  cross-interner symbol translation flows through the existing
  `remap_symbol`. `TOY_CACHE_DISABLE=1` skips both load and save.
- Bincode uses `with_varint_encoding()` (mandatory) — `string_interner
  0.20`'s `SymbolU32` has asymmetric serde (writes `usize`, reads
  `u32`); varint encoding produces the same byte sequence for both
  widths and dodges the misalignment.
- Measured speedup on `fib.t`: warm 0.01s vs cold 0.04s (~4x).

## Remaining Work for Full Incremental Compilation

### 1. Full AST Serialization (Phase 0 prerequisite)

`File` (full AST with `StmtPool` / `ExprPool` / `LocationPool`) must be
serializable.  This requires:

- `serde` derives on `StmtPool`, `ExprPool`, `LocationPool`
- `serde` derives on all `Stmt` and `Expr` variants
- `serde` support for `Rc<Function>` (enable `serde/rc` feature)
- Cache format version in the header for forward compatibility

### 2. Integration Hook in `module_integration.rs`

Modify `integrate_modules` in `interpreter/src/lib.rs`:

```rust
fn integrate_modules(...) -> Result<(), Vec<String>> {
    // 1. Try cache for each core module
    for module in &modules {
        if let Some(cached_file) = try_load_cached_module(source, cache_dir) {
            // Fast path: skip parse + type-check
            merge_cached_module(main_program, cached_file);
            continue;
        }
        // Slow path: parse + type-check + save cache
        let file = parse_and_type_check(source)?;
        save_module_cache(source, &file, cache_dir);
        merge_cached_module(main_program, file);
    }
}
```

### 3. Dependency Graph

Build a DAG of module dependencies:

```rust
struct DependencyGraph {
    modules: HashMap<String, ModuleNode>,
}

struct ModuleNode {
    source_path: PathBuf,
    imports: Vec<String>,  // dotted module paths this module imports
    interface_hash: String, // hash of the serialized ModuleInterface
}
```

### 4. Dirty Detection

A module is "dirty" (needs recompilation) when any of the following is
true:

1. Source file mtime/size changed (or SHA-256 mismatch)
2. Any imported module's `interface_hash` changed
3. Cache entry missing or corrupt

### 5. Partial Recompilation

When only module B's **implementation** changes (not its interface):
- Re-parse and re-lower B only
- Modules that import B do NOT need recompilation

When B's **interface** changes (e.g. pub fn signature changed):
- Re-parse and re-lower B
- Transitively recompile all modules that depend on B

### 6. IR Linker (Phase 2 proper)

For true per-module IR compilation, an IR linker is needed:

1. Each module is lowered to its own `ir::Module`
2. `link_modules(Vec<ir::Module>) -> ir::Module` merges them:
   - Offset `FuncId` values across modules
   - Deduplicate `struct_defs` / `enum_defs`
   - Merge `function_index` maps
   - Resolve cross-module `Call` instructions

## Performance Targets

- **Cold start** (cache empty): no regression vs. current
- **Warm start** (all core modules cached): 30–50% faster compilation
- **Incremental** (single module changed): recompile only changed module
  and its transitive dependents

## Testing Strategy

1. `test_cache_round_trip` — serialize + deserialize produces identical
   `ModuleInterface` ✅ (done in Phase 2)
2. `test_incremental_no_regression` — warm-cache compilation produces
   identical executable as cold-start compilation
3. `test_dirty_detection` — touching a source file invalidates only the
   correct cache entries
4. `test_interface_change_propagation` — changing a pub fn signature
   triggers recompilation of all importers

## Open Questions

1. **String interner compatibility** — cached modules use their own
   `DefaultStringInterner`.  On load, symbols must be re-interned into
   the main interner and all `DefaultSymbol` values in the cached AST
   must be remapped.  This is the same problem `module_integration.rs`
   already solves via `expr_mapping` / `stmt_mapping`.

2. **Generic monomorphization** — `lower_program` collects
   monomorphizations across the entire program.  Per-module lowering
   would require either:
   - Link-time monomorphization (defer instantiation until all modules
     are known), or
   - Duplicate instantiation in each module and deduplicate at link time

3. **Vtable generation for `dyn Trait`** — vtables reference `FuncId`
   values.  If each module lowers independently, vtable entries must be
   patched at link time.

## Timeline Estimate

- Full AST serialization: 2–3 days
- Integration hook + dependency graph: 2–3 days
- Dirty detection + partial recompilation: 2–3 days
- IR linker: 1–2 weeks
- Testing + debugging: 1 week

**Total: 4–5 weeks** for complete separate / incremental compilation.

## Immediate Next Steps

1. Add `serde` derives to `StmtPool`, `ExprPool`, `LocationPool`
2. Add `try_load_cached_module` / `save_module_cache` to
   `module_integration.rs`
3. Write a benchmark comparing cold-start vs. warm-start compilation
   times for a program that imports `std.math` and `std.string`
4. Implement the dependency graph in `interpreter/src/lib.rs`
