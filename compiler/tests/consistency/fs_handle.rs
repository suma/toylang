//! STDLIB-FS-HANDLE: `fs::File` — a descriptor, and reading a range of
//! a file through it.
//!
//! Everything `fs.t` had before this addressed a file by path and
//! touched the whole of it, which is why `poc/logsearch` split its
//! index into a second file and capped a segment at 8 MiB
//! (`RUNTIME_GAPS.md` R2). The tests here are about the two things
//! that removes: `read_at` answering from an offset, and the cursor
//! being somewhere the caller put it.
//!
//! Each test writes a fixture and reads it back, so what is pinned is
//! the bytes rather than a number the harness happens to produce — the
//! three lanes have to agree about what landed on disk.
//!
//! Every enum-producing call is bound to a `val` before it is matched.
//! That is the compiled lanes' MVP rule for `match` scrutinees, not a
//! style choice: an inline `match f.size() { ... }` is rejected with
//! "`match` on scalar scrutinee only supports i64 / u64 / bool".

use super::harness::{assert_consistent, unique_path};

fn scratch_path(stem: &str) -> std::path::PathBuf {
    let p = unique_path(&format!("{stem}.bin"));
    let _ = std::fs::remove_file(&p);
    p
}

/// Write `bytes` to `path` through a handle, as a toylang statement
/// block. Shared by the tests that need a fixture on disk before they
/// can ask anything interesting.
fn write_fixture(path: &std::path::Path, bytes: &[u8]) -> String {
    let sets: String = bytes
        .iter()
        .enumerate()
        .map(|(i, b)| format!("                    dst.set({i}u64, {b}u8)\n"))
        .collect();
    format!(
        r#"
            var out: Vec<u8> = Vec::with_capacity({cap}u64)
            val room: Option<Span<u8>> = out.capacity_span()
            match room {{
                Option::Some(dst) => {{
{sets}                    val made: Result<File, IoError> = File::create("{path}")
                    match made {{
                        Result::Ok(w) => {{
                            val wrote: Result<u64, IoError> = w.write(dst)
                            match wrote {{
                                Result::Ok(_) => {{ }}
                                Result::Err(_) => {{ acc = acc + 900000u64 }}
                            }}
                        }}
                        Result::Err(_) => {{ acc = acc + 800000u64 }}
                    }}
                }}
                Option::None => {{ acc = acc + 700000u64 }}
            }}
"#,
        cap = bytes.len(),
        path = path.display(),
    )
}

#[test]
fn a_range_reads_without_moving_the_cursor() {
    // The whole point of the handle: read the tail of a file, leave
    // the sequential reader where it was, and never touch the bytes
    // in between.
    let path = scratch_path("fs_handle_read_at");
    let src = format!(
        r#"
        fn main() -> u64 {{
            var acc: u64 = 0u64
{fixture}
            var back: Vec<u8> = Vec::with_capacity(2u64)
            val space: Option<Span<u8>> = back.capacity_span()
            match space {{
                Option::Some(dst) => {{
                    val opened: Result<File, IoError> = File::open("{path}")
                    match opened {{
                        Result::Ok(f) => {{
                            # The size comes from the handle, not the
                            # path, and does not move the cursor.
                            val sized: Result<u64, IoError> = f.size()
                            match sized {{
                                Result::Ok(s) => {{ acc = acc + s * 10u64 }}
                                Result::Err(_) => {{ acc = acc + 600u64 }}
                            }}
                            val got: Result<u64, IoError> = f.read_at(5u64, dst)
                            match got {{
                                Result::Ok(n) => {{
                                    back.set_size(n)
                                    acc = acc + n * 100u64
                                    acc = acc + back.get(0u64) as u64 * 1000u64
                                    acc = acc + back.get(1u64) as u64 * 100000u64
                                }}
                                Result::Err(_) => {{ acc = acc + 500u64 }}
                            }}
                            # Still at the start: `read_at` takes an
                            # offset precisely so that it is not a seek.
                            val here: Result<u64, IoError> = f.tell()
                            match here {{
                                Result::Ok(p) => {{ acc = acc + p * 10000000u64 }}
                                Result::Err(_) => {{ acc = acc + 400u64 }}
                            }}
                        }}
                        Result::Err(_) => {{ acc = acc + 300u64 }}
                    }}
                }}
                Option::None => {{ acc = acc + 200u64 }}
            }}
            acc
        }}
    "#,
        fixture = write_fixture(&path, &[10, 20, 30, 40, 50, 60, 70, 80]),
        path = path.display()
    );
    assert_consistent(&src, "fs_handle_read_at");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_cursor_advances_and_seeks_the_three_ways() {
    // `seek_to` / `seek_by` / `seek_end` are the three `lseek(2)`
    // whences behind names, each answering the new absolute position.
    // Reading twice in a row must not re-read the first bytes.
    let path = scratch_path("fs_handle_seek");
    let src = format!(
        r#"
        fn main() -> u64 {{
            var acc: u64 = 0u64
{fixture}
            var buf: Vec<u8> = Vec::with_capacity(2u64)
            val space: Option<Span<u8>> = buf.capacity_span()
            match space {{
                Option::Some(dst) => {{
                    val opened: Result<File, IoError> = File::open("{path}")
                    match opened {{
                        Result::Ok(f) => {{
                            # Sequential: the cursor moves by what was read.
                            val first: Result<u64, IoError> = f.read(dst)
                            match first {{
                                Result::Ok(_) => {{ buf.set_size(2u64) }}
                                Result::Err(_) => {{ acc = acc + 600u64 }}
                            }}
                            acc = acc + buf.get(0u64) as u64
                            val at: Result<u64, IoError> = f.tell()
                            match at {{
                                Result::Ok(p) => {{ acc = acc + p * 10u64 }}
                                Result::Err(_) => {{ acc = acc + 500u64 }}
                            }}
                            val second: Result<u64, IoError> = f.read(dst)
                            match second {{
                                Result::Ok(_) => {{ }}
                                Result::Err(_) => {{ acc = acc + 400u64 }}
                            }}
                            acc = acc + buf.get(0u64) as u64 * 100u64

                            val a: Result<u64, IoError> = f.seek_to(1u64)
                            match a {{
                                Result::Ok(p) => {{ acc = acc + p * 1000u64 }}
                                Result::Err(_) => {{ acc = acc + 300u64 }}
                            }}
                            val b: Result<u64, IoError> = f.seek_by(2i64)
                            match b {{
                                Result::Ok(p) => {{ acc = acc + p * 10000u64 }}
                                Result::Err(_) => {{ acc = acc + 200u64 }}
                            }}
                            # `0i64` from the end is the size.
                            val c: Result<u64, IoError> = f.seek_end(0i64)
                            match c {{
                                Result::Ok(p) => {{ acc = acc + p * 100000u64 }}
                                Result::Err(_) => {{ acc = acc + 100u64 }}
                            }}
                            # Reading at the end is `Ok(0)`, not an error.
                            val eof: Result<u64, IoError> = f.read(dst)
                            match eof {{
                                Result::Ok(n) => {{ acc = acc + n * 1000000u64 }}
                                Result::Err(_) => {{ acc = acc + 7000000u64 }}
                            }}
                        }}
                        Result::Err(_) => {{ acc = acc + 50u64 }}
                    }}
                }}
                Option::None => {{ acc = acc + 20u64 }}
            }}
            acc
        }}
    "#,
        fixture = write_fixture(&path, &[1, 2, 3, 4, 5, 6]),
        path = path.display()
    );
    assert_consistent(&src, "fs_handle_seek");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_record_updates_in_place_and_the_file_can_be_cut() {
    // `open_rw` keeps what is there, `write_at` overwrites one range
    // of it, and `truncate` cuts the rest. That trio is what a file
    // format needs to rewrite a header once the body is known.
    let path = scratch_path("fs_handle_update");
    let src = format!(
        r#"
        fn main() -> u64 {{
            var acc: u64 = 0u64
{fixture}
            var patch: Vec<u8> = Vec::with_capacity(2u64)
            val room: Option<Span<u8>> = patch.capacity_span()
            match room {{
                Option::Some(dst) => {{
                    dst.set(0u64, 41u8)
                    dst.set(1u64, 42u8)
                    val opened: Result<File, IoError> = File::open_rw("{path}")
                    match opened {{
                        Result::Ok(f) => {{
                            # `open_rw` keeps the six bytes rather than
                            # truncating -- `create` would not.
                            val before: Result<u64, IoError> = f.size()
                            match before {{
                                Result::Ok(s) => {{ acc = acc + s }}
                                Result::Err(_) => {{ acc = acc + 500u64 }}
                            }}
                            val put: Result<u64, IoError> = f.write_at(2u64, dst)
                            match put {{
                                Result::Ok(n) => {{ acc = acc + n * 10u64 }}
                                Result::Err(_) => {{ acc = acc + 400u64 }}
                            }}
                            # Durability is what `sync` is for; a test
                            # can only check that it succeeds.
                            val synced: Result<(), IoError> = f.sync()
                            match synced {{
                                Result::Ok(_) => {{ }}
                                Result::Err(_) => {{ acc = acc + 350u64 }}
                            }}
                            val cut: Result<(), IoError> = f.truncate(4u64)
                            match cut {{
                                Result::Ok(_) => {{ }}
                                Result::Err(_) => {{ acc = acc + 300u64 }}
                            }}
                            val after: Result<u64, IoError> = f.size()
                            match after {{
                                Result::Ok(s) => {{ acc = acc + s * 100u64 }}
                                Result::Err(_) => {{ acc = acc + 200u64 }}
                            }}
                        }}
                        Result::Err(_) => {{ acc = acc + 100u64 }}
                    }}
                }}
                Option::None => {{ acc = acc + 50u64 }}
            }}

            var back: Vec<u8> = Vec::with_capacity(4u64)
            val space: Option<Span<u8>> = back.capacity_span()
            match space {{
                Option::Some(dst) => {{
                    val reopened: Result<File, IoError> = File::open("{path}")
                    match reopened {{
                        Result::Ok(f) => {{
                            val got: Result<u64, IoError> = f.read(dst)
                            match got {{
                                Result::Ok(n) => {{
                                    back.set_size(n)
                                    acc = acc + n * 1000u64
                                    # 9, 9, 41, 42 -- the patch landed
                                    # in the middle and the tail is gone.
                                    acc = acc + back.get(2u64) as u64 * 10000u64
                                    acc = acc + back.get(3u64) as u64 * 1000000u64
                                }}
                                Result::Err(_) => {{ acc = acc + 30u64 }}
                            }}
                        }}
                        Result::Err(_) => {{ acc = acc + 20u64 }}
                    }}
                }}
                Option::None => {{ acc = acc + 10u64 }}
            }}
            acc
        }}
    "#,
        fixture = write_fixture(&path, &[9, 9, 9, 9, 9, 9]),
        path = path.display()
    );
    assert_consistent(&src, "fs_handle_update");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn opening_what_is_not_there_names_the_reason() {
    // The failure vocabulary is `IoError`, decoded from the same
    // status table `fs.t`'s path calls use -- and `open` never
    // creates, so this is `NotFound` rather than an empty file.
    let path = scratch_path("fs_handle_missing");
    let src = format!(
        r#"
        fn main() -> u64 {{
            val opened: Result<File, IoError> = File::open("{path}")
            match opened {{
                Result::Ok(f) => {{ 1u64 }}
                Result::Err(e) => {{
                    println(e)
                    match e {{
                        IoError::NotFound => 2u64,
                        _ => 3u64,
                    }}
                }}
            }}
        }}
    "#,
        path = path.display()
    );
    assert_consistent(&src, "fs_handle_missing");
}

#[test]
fn close_is_idempotent_and_append_adds_to_the_end() {
    // `close` parks the descriptor at `-1`, so the `Drop` that
    // follows does nothing -- the rule `TcpStream` follows, and the
    // reason a second close cannot shut down an unrelated file.
    let path = scratch_path("fs_handle_append");
    let src = format!(
        r#"
        fn main() -> u64 {{
            var acc: u64 = 0u64
{fixture}
            var more: Vec<u8> = Vec::with_capacity(2u64)
            val room: Option<Span<u8>> = more.capacity_span()
            match room {{
                Option::Some(dst) => {{
                    dst.set(0u64, 3u8)
                    dst.set(1u64, 4u8)
                    # Append adds rather than replacing, whatever the
                    # cursor says.
                    val opened: Result<File, IoError> = File::append("{path}")
                    match opened {{
                        Result::Ok(w) => {{
                            var h: File = w
                            val wrote: Result<u64, IoError> = h.write(dst)
                            match wrote {{
                                Result::Ok(_) => {{ }}
                                Result::Err(_) => {{ acc = acc + 700u64 }}
                            }}
                            val shut: Result<(), IoError> = h.close()
                            match shut {{
                                Result::Ok(_) => {{ acc = acc + 1u64 }}
                                Result::Err(_) => {{ acc = acc + 600u64 }}
                            }}
                            # Closing again is a no-op, not a failure.
                            val again: Result<(), IoError> = h.close()
                            match again {{
                                Result::Ok(_) => {{ acc = acc + 2u64 }}
                                Result::Err(_) => {{ acc = acc + 500u64 }}
                            }}
                            if h.is_open() {{ acc = acc + 400u64 }}
                        }}
                        Result::Err(_) => {{ acc = acc + 300u64 }}
                    }}
                }}
                Option::None => {{ acc = acc + 200u64 }}
            }}
            val reopened: Result<File, IoError> = File::open("{path}")
            match reopened {{
                Result::Ok(f) => {{
                    val sized: Result<u64, IoError> = f.size()
                    match sized {{
                        Result::Ok(s) => {{ acc = acc + s * 10u64 }}
                        Result::Err(_) => {{ acc = acc + 100u64 }}
                    }}
                }}
                Result::Err(_) => {{ acc = acc + 50u64 }}
            }}
            acc
        }}
    "#,
        fixture = write_fixture(&path, &[1, 2]),
        path = path.display()
    );
    assert_consistent(&src, "fs_handle_append");
    let _ = std::fs::remove_file(&path);
}
