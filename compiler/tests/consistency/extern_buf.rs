//! EXTERN-BUF: an `extern fn` reaching toylang memory.
//!
//! The extern boundary carries scalars and `str` handles, which is
//! enough until something wants to hand the OS a *buffer*. The
//! compiled lanes need nothing new for that — a toylang `ptr` there is
//! a real address, so `fread` writes straight into the caller's bytes.
//! The tree-walker's `ptr` is an index into its own heap, and its
//! registry entries took only the argument values, so they could not
//! resolve one at all.
//!
//! The fix lends the range instead of copying it: `HeapManager`'s byte
//! vector is contiguous, so a `(ptr, len)` pair resolves to a real
//! `&mut [u8]` and the same `read` call runs on every lane. The
//! borrow is handed to a closure rather than returned, because the
//! heap grows on allocation and a borrow kept across one would
//! dangle — the closure makes that a fact about the type instead of a
//! rule to remember.
//!
//! `io::read_file_into` / `write_file_bytes` are the first users and
//! what these tests exercise. They are also the binary-safe file API:
//! a `str` on the tree-walker is a Rust `String` and cannot hold
//! arbitrary bytes, which is why `read_file` reports a non-UTF-8 file
//! as a read error there while the compiled lanes accept it. Bytes
//! have no such split, and the round-trip below carries a NUL and a
//! 0xFF to prove it.

use super::harness::{assert_consistent, interpreter_value, unique_path};

fn scratch_path(stem: &str) -> std::path::PathBuf {
    let p = unique_path(&format!("{stem}.bin"));
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn bytes_round_trip_through_a_file_without_passing_through_a_str() {
    let path = scratch_path("extern_buf_roundtrip");
    let src = format!(
        r#"
        fn main() -> u64 {{
            var out: Vec<u8> = Vec::with_capacity(4u64)
            val room: Option<Span<u8>> = out.capacity_span()
            var acc: u64 = 0u64
            match room {{
                Option::Some(dst) => {{
                    # A NUL and a 0xFF: neither survives a `str` on
                    # every lane, both are ordinary bytes here.
                    dst.set(0u64, 104u8)
                    dst.set(1u64, 0u8)
                    dst.set(2u64, 255u8)
                    dst.set(3u64, 33u8)
                    val w: Result<u64, IoError> = io::write_file_bytes("{path}", dst)
                    val written = match w {{
                        Result::Ok(k) => k,
                        Result::Err(_) => 900u64,
                    }}
                    acc = acc + written
                }}
                Option::None => {{ acc = acc + 900u64 }}
            }}
            var back: Vec<u8> = Vec::with_capacity(8u64)
            val space: Option<Span<u8>> = back.capacity_span()
            match space {{
                Option::Some(dst) => {{
                    val r: Result<u64, IoError> = io::read_file_into("{path}", dst)
                    val n = match r {{
                        Result::Ok(k) => k,
                        Result::Err(_) => 900u64,
                    }}
                    back.set_size(n)
                    acc = acc + n * 10u64
                        + back.get(0u64) as u64
                        + back.get(1u64) as u64
                        + back.get(2u64) as u64
                }}
                Option::None => {{ acc = acc + 900u64 }}
            }}
            acc
        }}
    "#,
        path = path.display()
    );
    // 4 written + 40 for the four bytes read back + 104 + 0 + 255.
    assert_eq!(interpreter_value(&src) & 0xffff, 403);
    assert_consistent(&src, "extern_buf_roundtrip");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_file_longer_than_the_buffer_fills_it_and_stops() {
    let path = scratch_path("extern_buf_short");
    std::fs::write(&path, b"0123456789").expect("fixture");
    let src = format!(
        r#"
        fn main() -> u64 {{
            var back: Vec<u8> = Vec::with_capacity(4u64)
            val space: Option<Span<u8>> = back.capacity_span()
            match space {{
                Option::Some(dst) => {{
                    val r: Result<u64, IoError> = io::read_file_into("{path}", dst)
                    match r {{
                        Result::Ok(n) => n,
                        Result::Err(_) => 900u64,
                    }}
                }}
                Option::None => 900u64,
            }}
        }}
    "#,
        path = path.display()
    );
    // The count is what fit, not a failure: the caller compares it
    // with the span's length to notice the file was longer.
    assert_eq!(interpreter_value(&src) & 0xff, 4);
    assert_consistent(&src, "extern_buf_short_read");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_missing_file_reports_the_same_reason_it_does_for_read_file() {
    let path = scratch_path("extern_buf_absent");
    let src = format!(
        r#"
        fn main() -> u64 {{
            var back: Vec<u8> = Vec::with_capacity(4u64)
            val space: Option<Span<u8>> = back.capacity_span()
            match space {{
                Option::Some(dst) => {{
                    val r: Result<u64, IoError> = io::read_file_into("{path}", dst)
                    match r {{
                        Result::Ok(_) => 100u64,
                        Result::Err(IoError::NotFound) => 1u64,
                        Result::Err(_) => 50u64,
                    }}
                }}
                Option::None => 900u64,
            }}
        }}
    "#,
        path = path.display()
    );
    // The buffer path shares the runtime's status slot with
    // `read_file`, so the failure vocabulary is the same one.
    assert_eq!(interpreter_value(&src) & 0xff, 1);
    assert_consistent(&src, "extern_buf_missing");
}

/// A buffer built by `push` — the shape every `String` has — reaches
/// an `extern fn` with its bytes intact.
///
/// The tree-walker keeps a byte buffer in one of two places. A
/// `__builtin_ptr_write` of a narrow integer, which is what
/// `String::push` becomes, is recorded only as a typed slot; the raw
/// byte vector is stamped for 64-bit writes alone. Every toylang-side
/// read consults both (`HeapManager::read_byte_at`), but a borrow
/// handed to an `extern fn` cannot — the callee gets an address.
///
/// So `io::write_file_bytes` on a `String` wrote the **right number of
/// zeros**: the length was correct and the content was gone, on that
/// lane only. The borrow now flushes the typed slots into the raw
/// bytes first.
///
/// The test above does not cover this: it fills its buffer with
/// `Span::set`, which lands in the raw bytes, so both views already
/// agreed.
#[test]
fn a_buffer_built_by_push_reaches_an_extern_with_its_bytes() {
    let path = scratch_path("extern_buf_push");
    let src = format!(
        r#"
        fn main() -> u64 {{
            var s = String::new()
            s.push(104u8)
            s.push(105u8)
            val window: Option<Span<u8>> = s.as_span()
            var acc: u64 = 0u64
            match window {{
                Option::Some(src) => {{
                    val w: Result<u64, IoError> = io::write_file_bytes("{path}", src)
                    val written = match w {{
                        Result::Ok(k) => k,
                        Result::Err(_) => 900u64,
                    }}
                    acc = acc + written
                }}
                Option::None => {{ acc = acc + 900u64 }}
            }}
            var back: Vec<u8> = Vec::with_capacity(8u64)
            val space: Option<Span<u8>> = back.capacity_span()
            match space {{
                Option::Some(dst) => {{
                    val r: Result<u64, IoError> = io::read_file_into("{path}", dst)
                    val n = match r {{
                        Result::Ok(k) => k,
                        Result::Err(_) => 900u64,
                    }}
                    back.set_size(n)
                    acc = acc + back.get(0u64) as u64 + back.get(1u64) as u64
                }}
                Option::None => {{ acc = acc + 900u64 }}
            }}
            acc
        }}
    "#,
        path = path.display()
    );
    // 2 written + 'h' + 'i'. Before the fix the tree-walker answered
    // 2 — the bytes were zeros — while the compiled lanes answered
    // 211, so this disagrees rather than merely being wrong.
    assert_eq!(interpreter_value(&src) & 0xffff, 211);
    assert_consistent(&src, "extern_buf_push");
    let _ = std::fs::remove_file(&path);
}
