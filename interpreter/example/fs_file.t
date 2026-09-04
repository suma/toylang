# STDLIB-FS-HANDLE: an open file, and reading one range of it.
#
# `io::read_file` / `fs::copy_file` address a file **by path** and
# touch the whole of it. `fs::File` is the other half: open it once,
# then read or write a range. That is what lets a format keep its
# index inside the file it describes — reading the footer no longer
# means reading the file.
#
# The four constructors are `open` (read, must exist), `create`
# (write, discards), `append` (write at the end), and `open_rw` (read
# and write, keeps what is there). `Drop` closes the descriptor at
# scope exit; `close()` does it early and is idempotent.

fn temp_dir() -> str {
    val v = io::env_var("TMPDIR")
    match v {
        Result::Ok(dir) => dir,
        Result::Err(_) => "/tmp",
    }
}

# A little record format: four bytes of payload behind a two-byte
# header that is only known once the payload has been written. The
# header is patched in place afterwards, which is the shape `write_at`
# exists for.
fn main() -> u64 {
    val path: str = "{temp_dir()}/toylang_example_fs_file.bin"

    var body: Vec<u8> = Vec::with_capacity(6u64)
    val room: Option<Span<u8>> = body.capacity_span()
    val payload: Span<u8> = match room {
        Option::Some(s) => s,
        Option::None => { return 1u64 }
    }
    # Two header bytes left blank, then the payload.
    payload.set(0u64, 0u8)
    payload.set(1u64, 0u8)
    payload.set(2u64, 11u8)
    payload.set(3u64, 22u8)
    payload.set(4u64, 33u8)
    payload.set(5u64, 44u8)

    val made: Result<File, IoError> = File::create(path)
    match made {
        Result::Ok(f) => {
            val wrote: Result<u64, IoError> = f.write(payload)
            match wrote {
                Result::Ok(n) => { println("wrote {n} bytes") }
                Result::Err(e) => { eprintln("write failed: {e}") }
            }
            # `sync` is what makes the bytes survive a power cut, not
            # merely the process ending.
            val synced: Result<(), IoError> = f.sync()
            match synced {
                Result::Ok(_) => { println("synced") }
                Result::Err(e) => { eprintln("sync failed: {e}") }
            }
        }
        Result::Err(e) => { eprintln("create failed: {e}") }
    }

    # Patch the header now that the length is known, without rewriting
    # the file. `open_rw` keeps what is there; `create` would not.
    var head: Vec<u8> = Vec::with_capacity(2u64)
    val space: Option<Span<u8>> = head.capacity_span()
    val hdr: Span<u8> = match space {
        Option::Some(s) => s,
        Option::None => { return 2u64 }
    }
    hdr.set(0u64, 1u8)      # version
    hdr.set(1u64, 4u8)      # payload length

    val opened: Result<File, IoError> = File::open_rw(path)
    match opened {
        Result::Ok(f) => {
            val put: Result<u64, IoError> = f.write_at(0u64, hdr)
            match put {
                Result::Ok(n) => { println("patched {n} header bytes") }
                Result::Err(e) => { eprintln("patch failed: {e}") }
            }
        }
        Result::Err(e) => { eprintln("open_rw failed: {e}") }
    }

    # Read the payload alone, from its offset. The header is never
    # read and the cursor never moves.
    var back: Vec<u8> = Vec::with_capacity(4u64)
    val dst_room: Option<Span<u8>> = back.capacity_span()
    val dst: Span<u8> = match dst_room {
        Option::Some(s) => s,
        Option::None => { return 3u64 }
    }

    val reopened: Result<File, IoError> = File::open(path)
    match reopened {
        Result::Ok(f) => {
            val sized: Result<u64, IoError> = f.size()
            match sized {
                Result::Ok(s) => { println("file holds {s} bytes") }
                Result::Err(e) => { eprintln("size failed: {e}") }
            }
            val got: Result<u64, IoError> = f.read_at(2u64, dst)
            match got {
                Result::Ok(n) => {
                    back.set_size(n)
                    println("read {n} payload bytes from offset 2")
                    println("first byte is {back.get(0u64)}")
                    println("last byte is {back.get(3u64)}")
                }
                Result::Err(e) => { eprintln("read_at failed: {e}") }
            }
            # `read_at` is not a seek, so the cursor is still at 0.
            val here: Result<u64, IoError> = f.tell()
            match here {
                Result::Ok(p) => { println("cursor is at {p}") }
                Result::Err(e) => { eprintln("tell failed: {e}") }
            }
        }
        Result::Err(e) => { eprintln("open failed: {e}") }
    }

    # A failure the program can see: `open` never creates.
    val missing: Result<File, IoError> = File::open("/no/such/directory/toy.bin")
    match missing {
        Result::Ok(_) => { println("unexpectedly opened") }
        Result::Err(e) => { eprintln("expected failure: {e}") }
    }

    val gone: Result<(), IoError> = fs::remove_file(path)
    match gone {
        Result::Ok(_) => { }
        Result::Err(e) => { eprintln("cleanup failed: {e}") }
    }
    0u64
}
