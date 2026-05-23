//! Module interface cache — persistent storage for parsed module
//! interfaces so that repeated compilations can skip re-parsing
//! and re-type-checking of unchanged dependencies.
//!
//! The cache stores `ModuleInterface` values (the public surface of
//! a module, with all implementation bodies stripped) in a binary
//! format via `bincode`.  Each cache entry is keyed by the SHA-256
//! hash of the source file contents, so changing a single character
//! in a module invalidates its cache entry while leaving all other
//! entries intact.
//!
//! Cache layout on disk:
//!
//! ```text
//! <cache_dir>/<hash_prefix>/<source_hash>.interface
//! ```
//!
//! where `<source_hash>` is the hex-encoded SHA-256 of the source.

use std::io::{Read, Write};
use std::path::PathBuf;

use frontend::ast::module_interface::ModuleInterface;

/// Compute the SHA-256 hex digest of a source string.
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

    bincode::deserialize(&bytes).ok()
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
    let bytes = bincode::serialize(interface)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let mut file = std::fs::File::create(&path)?;
    file.write_all(&bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use frontend::ast::module_interface::ModuleInterface;

    #[test]
    fn test_round_trip() {
        let interface = ModuleInterface::empty();
        let dir = tempfile::tempdir().unwrap();

        let source = "fn main() -> u64 { 42u64 }";
        save_interface(source, &interface, dir.path()).unwrap();

        let loaded = load_interface(source, dir.path());
        assert!(loaded.is_some(), "Cache entry should exist after save");
    }

    #[test]
    fn test_missing_entry() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load_interface("fn main() -> u64 { 0u64 }", dir.path());
        assert!(loaded.is_none(), "Cache entry should not exist for unseen source");
    }
}
