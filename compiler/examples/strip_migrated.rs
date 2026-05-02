//! One-shot helper: rewrite `compiler/tests/e2e.rs` with every
//! `#[test] fn NAME()` block whose name appears in
//! `batched_data/extracted.rs` removed. The remaining tests are
//! the ones the batched runner couldn't extract (panic/assert
//! programs, exit-code expressions like `1 + 2 * 10`, custom
//! capture wrappers, etc.) and stay as the per-test debugging
//! surface.
//!
//! Run from workspace root after `dump_extracted` has refreshed
//! `batched_data/extracted.rs`:
//!   cargo run --release -p compiler --example strip_migrated

use std::path::PathBuf;

fn main() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let e2e_path = workspace.join("compiler/tests/e2e.rs");
    let data_path = workspace.join("compiler/tests/batched_data/extracted.rs");

    let e2e = std::fs::read_to_string(&e2e_path).expect("read e2e.rs");
    let data = std::fs::read_to_string(&data_path).expect("read extracted.rs");

    // Pull every `("NAME", ...` from the data file. The name is
    // always the first quoted string on the entry line.
    let mut migrated: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for line in data.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("(\"") {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("(\"") {
            if let Some(end) = rest.find('"') {
                migrated.insert(rest[..end].to_string());
            }
        }
    }
    eprintln!("# {} test names to strip from e2e.rs", migrated.len());

    // Walk e2e.rs by lines. When we see `#[test]`, look one line
    // ahead for `fn NAME(` and decide whether to drop the block.
    // A "block" is everything from the optional leading blank
    // line + `#[test]` line through the function's closing
    // `}` (matched at column 0). Naive brace counting works for
    // e2e.rs because Rust formatter places the closing `}` at
    // column 0 with no preceding `}` indented.
    let lines: Vec<&str> = e2e.lines().collect();
    let mut keep: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    let mut dropped_count = 0usize;
    while i < lines.len() {
        if lines[i].trim() == "#[test]" {
            // Look at the next non-blank line for `fn NAME(`.
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().is_empty() {
                j += 1;
            }
            let name = if j < lines.len() {
                let l = lines[j].trim();
                if let Some(rest) = l.strip_prefix("fn ") {
                    if let Some(end) = rest.find('(') {
                        Some(rest[..end].to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            let drop = name.as_ref().map_or(false, |n| migrated.contains(n));
            if drop {
                // Consume until the closing `}` at column 0.
                // Track brace depth from the function's own `{`.
                let mut k = j;
                while k < lines.len() && !lines[k].contains('{') {
                    k += 1;
                }
                if k >= lines.len() {
                    // Malformed — bail out, keep the rest verbatim.
                    keep.push(lines[i].to_string());
                    i += 1;
                    continue;
                }
                // Skip the function body until the closing `}` at
                // column 0 (Rust formatter convention).
                let mut k = k + 1;
                while k < lines.len() && lines[k] != "}" {
                    k += 1;
                }
                if k >= lines.len() {
                    keep.push(lines[i].to_string());
                    i += 1;
                    continue;
                }
                // k points at the closing `}`. Skip over it AND
                // the trailing blank line that separates tests.
                let mut next = k + 1;
                if next < lines.len() && lines[next].trim().is_empty() {
                    next += 1;
                }
                // Also drop the blank line that *preceded* the
                // `#[test]` if `keep` ends with one. Otherwise
                // we'd accumulate runs of blanks.
                if keep.last().map_or(false, |l| l.trim().is_empty()) {
                    keep.pop();
                }
                dropped_count += 1;
                i = next;
                continue;
            }
        }
        keep.push(lines[i].to_string());
        i += 1;
    }
    eprintln!("# stripped {} test blocks", dropped_count);

    let mut out = keep.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    std::fs::write(&e2e_path, out).expect("write back e2e.rs");
    eprintln!("# wrote {}", e2e_path.display());
}
