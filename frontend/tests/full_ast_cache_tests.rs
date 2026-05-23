//! Full AST serde round-trip tests for the Phase 4 incremental
//! compilation cache. These tests only check that the AST + a paired
//! `DefaultStringInterner` survive a bincode encode/decode unchanged
//! — they do not exercise the on-disk cache layer (that lives with
//! `CachedModule` / `load_full_module` in Phase 4b).

#![cfg(feature = "serde")]

use bincode::Options;
use frontend::ParserWithInterner;
use frontend::ast::File;
use string_interner::DefaultStringInterner;

/// Bincode options used for the Phase 4 AST cache. Varint encoding
/// is mandatory: `string_interner::SymbolU32` has asymmetric
/// `Serialize` / `Deserialize` impls (writes a `usize`, reads a
/// `u32`) so any fixed-width encoding makes the byte stream
/// misalign mid-pool. Varint encoding produces an identical byte
/// sequence for small values regardless of the source integer
/// width, dodging the asymmetry. See string-interner 0.20.0
/// `src/serde_impl.rs::impl_serde_for_symbol`.
fn cache_bincode_options() -> impl bincode::Options {
    bincode::DefaultOptions::new().with_varint_encoding()
}

/// Wrapper used for the round-trip check: the cache will pair an
/// interner with the AST it was parsed against, so the test mirrors
/// that shape.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct AstSnapshot {
    interner: DefaultStringInterner,
    file: File,
}

fn parse(source: &str) -> AstSnapshot {
    let mut parser = ParserWithInterner::new(source);
    let file = parser
        .parse_program()
        .unwrap_or_else(|e| panic!("parse failed: {:?}\nsource:\n{}", e, source));
    let interner = parser.get_string_interner().clone();
    AstSnapshot { interner, file }
}

fn assert_round_trip(snap: AstSnapshot) {
    let encoded = cache_bincode_options().serialize(&snap).expect("serialize");
    let decoded: AstSnapshot = cache_bincode_options()
        .deserialize(&encoded)
        .expect("deserialize");

    // Pools must be bit-identical so backends downstream produce the
    // same lowered output.
    assert_eq!(
        snap.file.expression, decoded.file.expression,
        "ExprPool round-trip mismatch"
    );
    assert_eq!(
        snap.file.statement, decoded.file.statement,
        "StmtPool round-trip mismatch"
    );
    assert_eq!(
        snap.file.location_pool, decoded.file.location_pool,
        "LocationPool round-trip mismatch"
    );
    assert_eq!(
        snap.file.function.len(),
        decoded.file.function.len(),
        "function count differs after round-trip"
    );
    for (a, b) in snap.file.function.iter().zip(decoded.file.function.iter()) {
        assert_eq!(a, b, "Function round-trip mismatch");
    }
    assert_eq!(snap.file.consts, decoded.file.consts, "consts mismatch");
    assert_eq!(snap.file.imports, decoded.file.imports, "imports mismatch");
    assert_eq!(
        snap.file.package_decl, decoded.file.package_decl,
        "package_decl mismatch"
    );
    assert_eq!(
        snap.file.function_module_paths, decoded.file.function_module_paths,
        "function_module_paths mismatch"
    );
}

#[test]
fn test_full_ast_roundtrip_minimal() {
    let snap = parse("fn main() -> u64 { 0u64 }");
    assert_round_trip(snap);
}

#[test]
fn test_full_ast_roundtrip_arithmetic() {
    let snap = parse(
        r#"
fn add(a: i64, b: i64) -> i64 { a + b }
fn main() -> i64 {
    val x: i64 = add(1i64, 2i64)
    x * 3i64
}
"#,
    );
    assert_round_trip(snap);
}

#[test]
fn test_full_ast_roundtrip_control_flow() {
    let snap = parse(
        r#"
fn classify(n: i64) -> i64 {
    if n < 0i64 { 0 - 1i64 } elif n == 0i64 { 0i64 } else { 1i64 }
}
fn main() -> i64 {
    var i: i64 = 0i64
    var total: i64 = 0i64
    while i < 5i64 {
        total = total + classify(i)
        i = i + 1i64
    }
    total
}
"#,
    );
    assert_round_trip(snap);
}

#[test]
fn test_full_ast_roundtrip_struct_and_impl() {
    let snap = parse(
        r#"
struct Point {
    x: i64,
    y: i64,
}
impl Point {
    fn new(x: i64, y: i64) -> Point {
        Point { x: x, y: y }
    }
    fn manhattan(self: Self) -> i64 {
        val ax: i64 = if self.x < 0i64 { 0i64 - self.x } else { self.x }
        val ay: i64 = if self.y < 0i64 { 0i64 - self.y } else { self.y }
        ax + ay
    }
}
fn main() -> i64 {
    val p: Point = Point::new(3i64, 0i64 - 4i64)
    p.manhattan()
}
"#,
    );
    assert_round_trip(snap);
}

#[test]
fn test_full_ast_roundtrip_enum_and_match() {
    let snap = parse(
        r#"
enum Shape {
    Circle(i64),
    Rect(i64, i64),
    Point,
}
fn area(s: Shape) -> i64 {
    match s {
        Shape::Circle(r) => r * r * 3i64,
        Shape::Rect(w, h) => w * h,
        Shape::Point => 0i64,
    }
}
fn main() -> i64 {
    area(Shape::Rect(4i64, 5i64))
}
"#,
    );
    assert_round_trip(snap);
}

#[test]
fn test_full_ast_roundtrip_generic_struct_and_trait() {
    let snap = parse(
        r#"
trait Greet {
    fn greet(self: Self) -> str
}
struct Dog {}
impl Greet for Dog {
    fn greet(self: Self) -> str { "Woof!" }
}
fn main() -> str {
    val d: Dog = Dog {}
    d.greet()
}
"#,
    );
    assert_round_trip(snap);
}

#[test]
fn test_full_ast_roundtrip_stdlib_math() {
    let source = include_str!("../../core/std/math.t");
    let snap = parse(source);
    assert_round_trip(snap);
}

#[test]
fn test_full_ast_roundtrip_stdlib_dict() {
    let source = include_str!("../../core/std/dict.t");
    let snap = parse(source);
    assert_round_trip(snap);
}

// --- Phase 4b: on-disk cache API --------------------------------

use frontend::cache::{
    CachedModule, FULL_AST_CACHE_SCHEMA_VERSION, load_full_module, save_full_module,
};

fn cached_module_from(source: &str) -> CachedModule {
    let snap = parse(source);
    CachedModule {
        schema_version: FULL_AST_CACHE_SCHEMA_VERSION,
        interner: snap.interner,
        file: snap.file,
    }
}

#[test]
fn test_full_module_save_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let source = "fn main() -> u64 { 42u64 }";
    let cached = cached_module_from(source);

    save_full_module(source, &cached, dir.path()).unwrap();
    let loaded = load_full_module(source, dir.path()).expect("cache hit");

    assert_eq!(loaded.schema_version, FULL_AST_CACHE_SCHEMA_VERSION);
    assert_eq!(loaded.file.expression, cached.file.expression);
    assert_eq!(loaded.file.statement, cached.file.statement);
    assert_eq!(loaded.file.function.len(), cached.file.function.len());
    for (a, b) in loaded
        .file
        .function
        .iter()
        .zip(cached.file.function.iter())
    {
        assert_eq!(a, b);
    }
}

#[test]
fn test_full_module_cache_missing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load_full_module("fn main() -> u64 { 0u64 }", dir.path()).is_none());
}

#[test]
fn test_full_module_schema_version_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let source = "fn main() -> u64 { 7u64 }";
    let mut cached = cached_module_from(source);

    // Forge a schema version older than the current one.
    cached.schema_version = FULL_AST_CACHE_SCHEMA_VERSION.wrapping_sub(1);
    save_full_module(source, &cached, dir.path()).unwrap();

    assert!(
        load_full_module(source, dir.path()).is_none(),
        "version mismatch must degrade to cache miss"
    );
}

#[test]
fn test_full_module_corrupt_cache() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let source = "fn main() -> u64 { 1u64 }";
    let cached = cached_module_from(source);
    save_full_module(source, &cached, dir.path()).unwrap();

    // Locate the .full file (single entry, named after the hash) and
    // truncate it so the trailing payload is lost. Truncation is
    // more reliable than tail-byte corruption: bincode tolerates
    // some trailing-byte changes inside string buffers without
    // erroring (it just decodes garbage), but never tolerates a
    // length prefix that asks for more bytes than the file has.
    let hash_dirs = std::fs::read_dir(dir.path()).unwrap();
    let mut full_path = None;
    for entry in hash_dirs.flatten() {
        if entry.file_type().unwrap().is_dir() {
            for sub in std::fs::read_dir(entry.path()).unwrap().flatten() {
                if sub.path().extension().and_then(|s| s.to_str()) == Some("full") {
                    full_path = Some(sub.path());
                }
            }
        }
    }
    let path = full_path.expect(".full file should exist");
    let bytes = std::fs::read(&path).unwrap();
    let mut handle = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    handle.write_all(&bytes[..bytes.len() / 2]).unwrap();
    drop(handle);

    assert!(
        load_full_module(source, dir.path()).is_none(),
        "truncated cache must degrade to cache miss"
    );
}

#[test]
fn test_full_module_save_round_trip_stdlib_math() {
    let dir = tempfile::tempdir().unwrap();
    let source = include_str!("../../core/std/math.t");
    let cached = cached_module_from(source);
    save_full_module(source, &cached, dir.path()).unwrap();

    let loaded = load_full_module(source, dir.path()).expect("cache hit");
    assert_eq!(loaded.file.function.len(), cached.file.function.len());
}

#[test]
fn test_full_ast_roundtrip_preserves_interner_resolution() {
    // After round-tripping, resolving a symbol on the decoded interner
    // must return the same string the original interner returned. This
    // is the load-bearing property for Phase 4c's cross-interner
    // remap_symbol path: cached_interner.resolve(sym) must yield the
    // identifier string so main_interner.get_or_intern(s) can re-link.
    let snap = parse("fn alpha_beta_gamma() -> u64 { 42u64 }");
    let original_strings: Vec<String> = snap
        .interner
        .iter()
        .map(|(_, s)| s.to_string())
        .collect();

    let encoded = cache_bincode_options().serialize(&snap).unwrap();
    let decoded: AstSnapshot = cache_bincode_options().deserialize(&encoded).unwrap();

    let decoded_strings: Vec<String> = decoded
        .interner
        .iter()
        .map(|(_, s)| s.to_string())
        .collect();
    assert_eq!(
        original_strings, decoded_strings,
        "interner string set must be preserved across round-trip"
    );

    // For every symbol in the original AST that points at a string,
    // both interners must resolve to the same string.
    for sym in snap.file.expression.symbol_val.iter().flatten() {
        let original = snap.interner.resolve(*sym).unwrap();
        let after = decoded.interner.resolve(*sym).unwrap();
        assert_eq!(
            original, after,
            "symbol {:?} resolves differently after round-trip",
            sym
        );
    }
}
