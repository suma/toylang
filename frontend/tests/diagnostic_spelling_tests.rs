//! DIAG-SYMBOL-NAME: a diagnostic must never show an interned id.
//!
//! The type checker builds most of its messages by hand with
//! `format!`, and for years some of those reached for `{:?}` on a
//! `DefaultSymbol` or a `TypeDecl`. The result told the reader nothing:
//!
//! ```text
//! Associated function 'new' not found for struct 'SymbolU32 { value: 60 }'
//! method 'f' return type mismatch (expected UInt64, found Struct(SymbolU32 { value: 60 }, []))
//! ```
//!
//! The fix was to route every such site through the interner-aware
//! spellings (`resolve_symbol_name` for a name,
//! `type_name_for_error` for a type). That is a per-site fix, and it
//! had already been made twice before — once in `context.rs`, once in
//! `struct_literal.rs` — only for the next hand-written message to
//! reintroduce it. So the rule is checked here rather than remembered.
//!
//! The companion test, `interpreter/tests/diagnostic_spelling_tests.rs`,
//! asserts the same rule from the other end: it runs erroring programs
//! and reads what a user actually sees.

use std::path::{Path, PathBuf};

/// A `{:?}` / `{name:?}` / `{:#?}` format spec.
fn has_debug_spec(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut i = 0;
    while let Some(open) = line[i..].find('{') {
        let start = i + open + 1;
        let mut j = start;
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
            j += 1;
        }
        if line[j..].starts_with(":?}") || line[j..].starts_with(":#?}") {
            return true;
        }
        i = start;
    }
    false
}

/// Lines that are wholly a comment. The rule is about what a message
/// *contains*, and the surrounding prose has to be free to quote the
/// very spelling it is warning against.
fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with("/*") || t.starts_with('*')
}

fn type_checker_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/type_checker")
}

/// The escape hatch, spelled out at the site it applies to.
const ALLOW_MARKER: &str = "DIAG-DEBUG-FMT-OK";

/// How far back the marker is looked for. Small on purpose: a marker
/// justifies the site under it, not a whole file.
const MARKER_LOOKBACK: usize = 10;

#[test]
fn no_type_checker_diagnostic_formats_with_debug() {
    let mut offenders = Vec::new();

    let mut files: Vec<PathBuf> = std::fs::read_dir(type_checker_dir())
        .expect("type_checker source directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        // The checker's own unit tests print error kinds on failure,
        // which is Debug used as Debug.
        .filter(|p| p.file_name().is_some_and(|n| n != "tests.rs"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "found no type_checker sources to scan");

    for path in &files {
        let text = std::fs::read_to_string(path).expect("read source");
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if is_comment(line) || !has_debug_spec(line) {
                continue;
            }
            let from = i.saturating_sub(MARKER_LOOKBACK);
            let marked = lines[from..=i].iter().any(|l| l.contains(ALLOW_MARKER));
            if !marked {
                let name = path.file_name().unwrap().to_string_lossy();
                offenders.push(format!("  {}:{}  {}", name, i + 1, line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these lines format a value with `{{:?}}` inside the type checker.\n\
         A diagnostic must spell names through the interner — use \
         `self.resolve_symbol_name(sym)` for a name and \
         `self.type_name_for_error(&ty)` for a type, or Debug will put \
         `SymbolU32 {{ value: 63 }}` in front of the reader.\n\
         If the string is genuinely not a diagnostic (a hash key, say), \
         say so in a comment carrying `{}` within {} lines above.\n{}",
        ALLOW_MARKER,
        MARKER_LOOKBACK,
        offenders.join("\n")
    );
}

#[test]
fn the_scanner_recognises_the_specs_it_is_looking_for() {
    // Guards the guard: a scanner that matches nothing would pass the
    // test above no matter how bad the sources got.
    assert!(has_debug_spec(r#"format!("got {:?}", ty)"#));
    assert!(has_debug_spec(r#"format!("got {ty:?}")"#));
    assert!(has_debug_spec(r#"format!("got {:#?}", ty)"#));
    assert!(has_debug_spec(r#"format!("{a:?} and {b:?}")"#));
    assert!(!has_debug_spec(r#"format!("got {}", ty)"#));
    assert!(!has_debug_spec(r#"format!("got {ty}")"#));
    assert!(!has_debug_spec(r#"format!("{x:.2}")"#));
    assert!(!has_debug_spec(r#"let q = a ? b : c;"#));
}
