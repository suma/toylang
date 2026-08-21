//! Module interface cache — persistent storage for parsed module
//! interfaces so that repeated compilations can skip re-parsing
//! and re-type-checking of unchanged dependencies.
//!
//! The cache stores `ModuleInterface` values (the public surface of
//! a module, with all implementation bodies stripped) in a binary
//! format via `bincode`.  Each cache entry is keyed by the hash
//! of the source file contents.

use std::io::{Read, Write};
use std::path::PathBuf;

use bincode::Options;
use string_interner::DefaultStringInterner;

use crate::ast::File;
use crate::ast::module_interface::ModuleInterface;

/// Schema version for the on-disk Full AST cache (`.full` files).
///
/// Bump on any breaking change to:
/// - the layout of `File`, `ExprPool`, `StmtPool`, `LocationPool`
/// - the `Expr` / `Stmt` / `Pattern` / `MatchArm` enums
/// - the `Function` / `MethodFunction` / `ConstDecl` / `TestCase` structs
/// - the `File` struct's own field list
/// - the bincode options used by [`cache_bincode_options`]
/// - **the set of names `BuiltinFunctionSymbols::new` interns**, or
///   anything else pre-seeded into the interner before parsing. A
///   cache entry stores `DefaultSymbol`s, which are indices into that
///   interner; adding a name shifts every symbol after it and the
///   stored indices then mean something else.
///
/// Mismatched versions are treated as a cache miss by
/// [`load_full_module`].
pub const FULL_AST_CACHE_SCHEMA_VERSION: u32 = 13;
// v2: `File` gained `id` (JIT cache key) and `tests` (LLM-LOOP P4).
// v3: `BuiltinFunctionSymbols` interns the MEMORY_PROFILING M4 counter
// names, shifting every later symbol id.
// v4: `BuiltinFunctionSymbols` interns `__builtin_record_allocator_layout`
// (allocator layout registry), shifting every later symbol id.
// v5: `BuiltinFunctionSymbols` interns `__builtin_ptr_offset` (interior
// pointer arithmetic), shifting every later symbol id.
// v6: same again for `__builtin_str_from_bytes` (the bytes-to-str
// primitive the Display trait needs).
// v7: `File` gained `transferred_bindings` (BOX-T ownership transfer).
// v8: `BuiltinFunctionSymbols` interns the FFI_PLAN P1 `from` / `as`
// extern-link keywords.
// v9: `core/std/convert.t` adds `From` / `Into` / `from` / `into` to
// the shared interner, shifting every symbol interned after `string.t`.
// v10: `Expr::Try` gained `converted_binding` / `result_binding` (the
// `?` cross-error conversion temporaries).
// Forgetting this bump is not a subtle failure: stale entries
// deserialize into the new layout and the program silently comes out
// wrong — every stdlib trait reported "is not defined". The M4 bump
// was found the same way: an unrelated `val a: u64 = 5u64` started
// failing with three type errors from the stdlib.

/// Bincode options for the AST cache.
///
/// **Varint encoding is mandatory** because `string_interner::SymbolU32`
/// has asymmetric `Serialize` / `Deserialize` impls (writes a `usize`,
/// reads a `u32`). Under fixed-width encoding the byte stream
/// misaligns at every symbol; varint encoding produces an identical
/// byte sequence for small values regardless of the source integer
/// width, dodging the asymmetry. See `string-interner` 0.20.0
/// `src/serde_impl.rs::impl_serde_for_symbol`.
///
/// Both the `ModuleInterface` cache and the Full AST cache use the
/// same options so their on-disk encodings stay consistent.
fn cache_bincode_options() -> impl Options {
    bincode::DefaultOptions::new().with_varint_encoding()
}

/// Compute a simple hash of a source string.
fn source_hash(source: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Return the default cache directory.
///
/// Tries `$TOY_CACHE_DIR`, then falls back to a `.toycache`
/// directory next to the current working directory.
pub fn default_cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("TOY_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    PathBuf::from(".toycache")
}

/// Load a cached `ModuleInterface` for the given source.
///
/// Returns `None` if no cache entry exists or if deserialization
/// fails (e.g. format mismatch after an upgrade).
pub fn load_interface(source: &str, cache_dir: &std::path::Path) -> Option<ModuleInterface> {
    let hash = source_hash(source);
    let prefix = &hash[..2];
    let path = cache_dir.join(prefix).join(format!("{}.interface", hash));

    let mut file = std::fs::File::open(&path).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;

    cache_bincode_options().deserialize(&bytes).ok()
}

/// Save a `ModuleInterface` to the cache for the given source.
///
/// Creates parent directories as needed.  Overwrites any existing
/// entry for the same source hash.
pub fn save_interface(
    source: &str,
    interface: &ModuleInterface,
    cache_dir: &std::path::Path,
) -> std::io::Result<()> {
    let hash = source_hash(source);
    let prefix = &hash[..2];
    let dir = cache_dir.join(prefix);
    std::fs::create_dir_all(&dir)?;

    let path = dir.join(format!("{}.interface", hash));
    let bytes = cache_bincode_options()
        .serialize(interface)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let mut file = std::fs::File::create(&path)?;
    file.write_all(&bytes)?;
    Ok(())
}

// --- Full AST cache -----------------------------------------------

/// A parsed module's AST paired with the `DefaultStringInterner`
/// that minted its symbols.
///
/// The interner is essential: every `DefaultSymbol` in the `File`
/// belongs to that interner's id space and resolves to its strings.
/// On load, downstream code (e.g. `AstIntegrationContext`) translates
/// each symbol into the main interner via `resolve + get_or_intern`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedModule {
    /// Schema fingerprint — see [`FULL_AST_CACHE_SCHEMA_VERSION`].
    /// Stored first so [`load_full_module`] can reject mismatches
    /// before walking any further fields.
    pub schema_version: u32,
    pub interner: DefaultStringInterner,
    pub file: File,
}

/// Load a cached `CachedModule` for the given source.
///
/// Returns `None` if no cache entry exists, the file is corrupt,
/// or the on-disk schema version does not match
/// [`FULL_AST_CACHE_SCHEMA_VERSION`]. Both deserialization failure
/// and version mismatch are silent — they degrade to a cache miss
/// so the caller can fall back to a normal parse.
pub fn load_full_module(source: &str, cache_dir: &std::path::Path) -> Option<CachedModule> {
    let hash = source_hash(source);
    let prefix = &hash[..2];
    let path = cache_dir.join(prefix).join(format!("{}.full", hash));

    let mut file = std::fs::File::open(&path).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;

    let cached: CachedModule = cache_bincode_options().deserialize(&bytes).ok()?;
    if cached.schema_version != FULL_AST_CACHE_SCHEMA_VERSION {
        return None;
    }
    Some(cached)
}

/// Save a `CachedModule` to the cache for the given source.
///
/// Creates parent directories as needed. Overwrites any existing
/// entry for the same source hash. Callers should treat I/O
/// failure as non-fatal: the warm-cache fast path is a
/// best-effort optimization.
pub fn save_full_module(
    source: &str,
    cached: &CachedModule,
    cache_dir: &std::path::Path,
) -> std::io::Result<()> {
    let hash = source_hash(source);
    let prefix = &hash[..2];
    let dir = cache_dir.join(prefix);
    std::fs::create_dir_all(&dir)?;

    let path = dir.join(format!("{}.full", hash));
    let bytes = cache_bincode_options()
        .serialize(cached)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let mut file = std::fs::File::create(&path)?;
    file.write_all(&bytes)?;
    Ok(())
}
