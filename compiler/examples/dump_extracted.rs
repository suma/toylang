//! One-shot helper: prints to stdout a Rust literal array of all
//! the sub-tests the e2e_batched extractor currently picks up.
//! Used during the e2e.rs → e2e_batched.rs migration to bake the
//! extracted data inline so the per-test `e2e.rs` definitions
//! can be removed.
//!
//! Run:
//!   cargo run --release -p compiler --example dump_extracted > /tmp/extracted_data.rs

use std::path::PathBuf;

fn main() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    // `DUMP_EXTRACTED_E2E_PATH=/tmp/e2e_old.rs` overrides the
    // input source — useful when re-extracting after some tests
    // have already been migrated out of the live `e2e.rs` (the
    // dumper otherwise sees only the surviving subset).
    let e2e_path = match std::env::var("DUMP_EXTRACTED_E2E_PATH") {
        Ok(p) => PathBuf::from(p),
        Err(_) => workspace.join("compiler/tests/e2e.rs"),
    };
    let e2e = std::fs::read_to_string(&e2e_path).expect("read e2e.rs");

    let exit = extract_exit(&e2e);
    let stdout = extract_stdout(&e2e);
    eprintln!("# extracted: {} exit, {} stdout", exit.len(), stdout.len());

    println!("// AUTO-GENERATED via `cargo run --release -p compiler --example dump_extracted`.");
    println!("// Source data for the batched e2e fixtures. Each entry mirrors the");
    println!("// `#[test]` definitions that previously lived in `compiler/tests/e2e.rs`.");
    println!("");
    // `pub static` (not `pub(super)`) because this file is
    // pulled in via `include!()` at the crate root rather than
    // as a real submodule — `super` would over-resolve.
    println!("pub static EXIT_SUBTESTS: &[(&str, &str, u64)] = &[");
    for (name, src, exp) in &exit {
        println!(
            "    (\"{name}\", \"{src}\", {exp}),",
            name = name,
            src = escape_rust_string_literal(src),
            exp = exp,
        );
    }
    println!("];");
    println!("");
    println!("pub static STDOUT_SUBTESTS: &[(&str, &str, &str)] = &[");
    for (name, src, expected) in &stdout {
        println!(
            "    (\"{name}\", \"{src}\", \"{expected}\"),",
            name = name,
            src = escape_rust_string_literal(src),
            expected = escape_rust_string_literal(expected),
        );
    }
    println!("];");
}

fn escape_rust_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn extract_raw_string(body: &str, start_marker: &str, end_marker: &str) -> Option<String> {
    let s = body.find(start_marker)? + start_marker.len();
    let rest = &body[s..];
    let e = rest.find(end_marker)?;
    Some(rest[..e].to_string())
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_int(raw: &str) -> Option<u64> {
    let raw = raw.trim().trim_end_matches(';').trim();
    let raw = raw.split(" as ").next().unwrap_or(raw).trim();
    raw.parse::<u64>().ok()
}

fn extract_assert_expected(body: &str) -> Option<u64> {
    if let Some(s) = body.find("assert_eq!(code,") {
        let s = s + "assert_eq!(code,".len();
        let rest = &body[s..];
        let e = rest.find(')')?;
        return parse_int(&rest[..e]);
    }
    let macro_start = body.find("assert_eq!(compile_and_run")?;
    let body_after = &body[macro_start + "assert_eq!(".len()..];
    let mut depth = 0;
    let mut comma_pos: Option<usize> = None;
    for (i, ch) in body_after.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                comma_pos = Some(i);
                break;
            }
            _ => {}
        }
    }
    let comma_pos = comma_pos?;
    let after_comma = &body_after[comma_pos + 1..];
    let mut depth = 0;
    let mut close_pos: Option<usize> = None;
    for (i, ch) in after_comma.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' if depth == 0 => {
                close_pos = Some(i);
                break;
            }
            ')' => depth -= 1,
            _ => {}
        }
    }
    let close_pos = close_pos?;
    parse_int(&after_comma[..close_pos])
}

fn extract_exit(e2e: &str) -> Vec<(String, String, u64)> {
    let test_starts: Vec<usize> = e2e
        .match_indices("\n#[test]\n")
        .map(|(idx, _)| idx + 1)
        .collect();
    let mut out = Vec::new();
    for (i, &start) in test_starts.iter().enumerate() {
        let end = test_starts.get(i + 1).copied().unwrap_or(e2e.len());
        let block = &e2e[start..end];
        let fn_idx = match block.find("fn ") {
            Some(p) => p + 3,
            None => continue,
        };
        let paren = match block[fn_idx..].find('(') {
            Some(p) => p,
            None => continue,
        };
        let name = block[fn_idx..fn_idx + paren].trim().to_string();
        if name.is_empty() {
            continue;
        }
        if block.contains("compile_and_capture")
            || block.matches("compile_and_run").count() != 1
            || (!block.contains("assert_eq!(code,") && !block.contains("assert_eq!(compile_and_run"))
            || block.contains("panic(")
            || block.contains("assert(")
        {
            continue;
        }
        let source = if let Some(s) = extract_raw_string(block, "let src = r#\"", "\"#") {
            s
        } else if let Some(s) = extract_raw_string(block, "compile_and_run(r#\"", "\"#,") {
            s
        } else if let Some(s) = extract_raw_string(block, "compile_and_run(\"", "\",") {
            unescape(&s)
        } else {
            continue;
        };
        let expected = match extract_assert_expected(block) {
            Some(n) => n,
            None => continue,
        };
        out.push((name, source, expected));
    }
    out
}

fn extract_stdout(e2e: &str) -> Vec<(String, String, String)> {
    let test_starts: Vec<usize> = e2e
        .match_indices("\n#[test]\n")
        .map(|(idx, _)| idx + 1)
        .collect();
    let mut out = Vec::new();
    for (i, &start) in test_starts.iter().enumerate() {
        let end = test_starts.get(i + 1).copied().unwrap_or(e2e.len());
        let block = &e2e[start..end];
        let fn_idx = match block.find("fn ") {
            Some(p) => p + 3,
            None => continue,
        };
        let paren = match block[fn_idx..].find('(') {
            Some(p) => p,
            None => continue,
        };
        let name = block[fn_idx..fn_idx + paren].trim().to_string();
        if name.is_empty() {
            continue;
        }
        if !block.contains("compile_and_capture")
            || block.matches("compile_and_capture").count() != 1
            || !block.contains("assert_eq!(out.status.code(), Some(0))")
            || !block.contains("String::from_utf8_lossy(&out.stdout)")
        {
            continue;
        }
        let source = match extract_raw_string(block, "let src = r#\"", "\"#") {
            Some(s) => s,
            None => continue,
        };
        // Two assert_eq layouts to handle:
        //   1. single-line: assert_eq!(String::from_utf8_lossy(&out.stdout), "X")
        //   2. multi-line:
        //        assert_eq!(
        //            String::from_utf8_lossy(&out.stdout),
        //            "X",
        //        );
        // For (2), the marker between `String::from_utf8_lossy(&out.stdout)`
        // and the literal is `,\n        "` rather than `, "`. Try the
        // single-line first, then fall back.
        let expected = if let Some(s) = extract_raw_string(
            block,
            "assert_eq!(String::from_utf8_lossy(&out.stdout), \"",
            "\")",
        ) {
            unescape(&s)
        } else if let Some(s) =
            extract_after_lossy_multiline(block)
        {
            unescape(&s)
        } else {
            continue;
        };
        out.push((name, source, expected));
    }
    out
}

/// Multi-line variant: locate `String::from_utf8_lossy(&out.stdout)`,
/// scan past the trailing `,` (and any whitespace / newlines)
/// until the next `"`, then read the literal up to its closing
/// `"`. Returns `None` if the structure doesn't match.
///
/// Guards: only fires when the marker is the first arg of an
/// `assert_eq!(` macro call. The pattern
/// ```text
///   let stdout = String::from_utf8_lossy(&out.stdout);
///   assert!(stdout.starts_with("X"), "unexpected: {stdout:?}");
/// ```
/// must not be picked up — the literal at the comma is the
/// assert message, not the expected stdout. Without the
/// `assert_eq!(` prefix check, the extractor latches onto
/// `"unexpected ..."`.
fn extract_after_lossy_multiline(body: &str) -> Option<String> {
    let marker = "String::from_utf8_lossy(&out.stdout)";
    let i = body.find(marker)?;
    // Walk backwards from `i` looking for the most recent
    // non-whitespace token. It must be `(` of an
    // `assert_eq!(` call (possibly followed by a newline +
    // indent).
    let head = &body[..i];
    let mut k = head.len();
    // Skip trailing whitespace.
    while k > 0 && head.as_bytes()[k - 1].is_ascii_whitespace() {
        k -= 1;
    }
    if k == 0 || head.as_bytes()[k - 1] != b'(' {
        return None;
    }
    // The `(` must be from `assert_eq!(`.
    let bang_idx = head[..k - 1].rfind('!')?;
    let macro_name_end = bang_idx;
    let macro_start = head[..macro_name_end]
        .rfind(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .map_or(0, |p| p + 1);
    let macro_name = &head[macro_start..macro_name_end];
    if macro_name != "assert_eq" {
        return None;
    }
    let after_marker_idx = i + marker.len();
    let after = &body[after_marker_idx..];
    let comma = after.find(',')?;
    let rest = &after[comma + 1..];
    let quote = rest.find('"')?;
    let body = &rest[quote + 1..];
    let bytes = body.as_bytes();
    let mut j = 0;
    while j < bytes.len() {
        if bytes[j] == b'\\' {
            j += 2;
            continue;
        }
        if bytes[j] == b'"' {
            return Some(body[..j].to_string());
        }
        j += 1;
    }
    None
}
