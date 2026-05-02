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
    let e2e_path = workspace.join("compiler/tests/e2e.rs");
    let e2e = std::fs::read_to_string(&e2e_path).expect("read e2e.rs");

    let exit = extract_exit(&e2e);
    let stdout = extract_stdout(&e2e);
    eprintln!("# extracted: {} exit, {} stdout", exit.len(), stdout.len());

    println!("// AUTO-GENERATED via `cargo run --release -p compiler --example dump_extracted`.");
    println!("// Source data for the batched e2e fixtures. Each entry mirrors the");
    println!("// `#[test]` definitions that previously lived in `compiler/tests/e2e.rs`.");
    println!("");
    println!("pub(super) static EXIT_SUBTESTS: &[(&str, &str, u64)] = &[");
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
    println!("pub(super) static STDOUT_SUBTESTS: &[(&str, &str, &str)] = &[");
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
        let expected = match extract_raw_string(
            block,
            "assert_eq!(String::from_utf8_lossy(&out.stdout), \"",
            "\")",
        ) {
            Some(s) => unescape(&s),
            None => continue,
        };
        out.push((name, source, expected));
    }
    out
}
