# Assertions that say what happened (TEST-TOOL T3).
#
# The three the language ships with -- `assert` / `assert_eq` /
# `assert_ne` -- cover equality on scalars and nothing else. Everything
# below was written by hand in `poc/logsearch` until this file existed,
# and the hand-written forms all had the same defect: they said the
# check failed without saying what the values were. An assertion
# library exists so that a failure reads like a report.
#
# Every function here **panics** on failure, which is what `assert_eq`
# does and what the test runner counts as a failure. They are ordinary
# functions, so they work outside a `test` block too -- in a `requires`
# clause the contract machinery is the better tool, but in a `main`
# that is checking its own work these are the same thing.
#
# Auto-loaded from `<core>/std/testing.t`.

# `a` and `b` agree to within `eps`.
#
# There is no `==` for floats worth writing in a test: the value that
# comes out of a computation differs from the one you wrote down in
# the last bits, and a test that demands they match exactly fails for
# a reason that has nothing to do with the program. The message
# carries both values **and the difference**, because the useful
# question on a failure is whether `eps` is too tight or the answer is
# wrong, and the difference is what separates them.
pub fn assert_close(a: f64, b: f64, eps: f64) {
    val diff: f64 = if a > b { a - b } else { b - a }
    if diff > eps {
        panic("assert_close failed: left {a}, right {b}, differ by {diff} (eps {eps})")
    }
}

# Byte-wise equality of two `str`s, naming the first byte that differs.
#
# `assert_eq` compares `str` already; this adds the position. Two
# strings that differ at byte 400 of 4,000 produce a diagnostic that
# is unreadable as two whole values and obvious as an offset.
pub fn assert_str_eq(a: str, b: str) {
    if a == b {
        return
    }
    val la: u64 = a.len()
    val lb: u64 = b.len()
    var i: u64 = 0u64
    val shorter: u64 = if la < lb { la } else { lb }
    val pa: Ptr<u8> = Ptr { addr: a.as_ptr() }
    val pb: Ptr<u8> = Ptr { addr: b.as_ptr() }
    while i < shorter {
        val x: u8 = pa.get(i)
        val y: u8 = pb.get(i)
        if x != y {
            panic("assert_str_eq failed: byte {i} differs: left {x}, right {y} (lengths {la} and {lb})")
        }
        i = i + 1u64
    }
    # No differing byte in the common prefix, so one is a prefix of
    # the other.
    panic("assert_str_eq failed: left is {la} bytes, right is {lb} bytes, common prefix matches")
}

# Byte-wise equality of two windows, naming the first offset that
# differs.
#
# This is the one the design calls out: a 4 MiB segment that is one
# byte wrong is not helped by being told that it is wrong.
# `Span::bytes_eq` already answers the yes/no question in a single
# runtime call (MEMORY-ACCESS M3), so the scan only runs on failure --
# a passing assertion costs exactly what `bytes_eq` costs.
pub fn assert_bytes_eq(a: Span<u8>, b: Span<u8>) {
    val la: u64 = a.len()
    val lb: u64 = b.len()
    if la != lb {
        panic("assert_bytes_eq failed: left is {la} bytes, right is {lb} bytes")
    }
    if a.bytes_eq(b) {
        return
    }
    var i: u64 = 0u64
    while i < la {
        val x: u8 = a.get(i)
        val y: u8 = b.get(i)
        if x != y {
            panic("assert_bytes_eq failed: byte {i} differs: left {x}, right {y} (of {la} bytes)")
        }
        i = i + 1u64
    }
    # `bytes_eq` said no and the scan found nothing: the two disagree
    # about the same memory, which is a runtime bug rather than a test
    # failure. Say so rather than passing silently.
    panic("assert_bytes_eq: bytes_eq reported a difference the scan cannot find ({la} bytes)")
}

# The payload of a `Some`, or a failure.
#
# `unwrap()` panics too, but with a message about an `Option`; this
# one says an assertion failed, which is what a reader of a test
# report is looking for.
pub fn assert_some<T>(v: Option<T>, what: str) -> T {
    match v {
        Option::Some(x) => x,
        Option::None => panic("assert_some failed: {what} was None"),
    }
}

# The payload of an `Ok`, or a failure.
#
# The error is **not** in the message: `E` is a type parameter, and
# rendering it would require every error type a test touches to
# implement `Display`. The caller's `what` is the thing that says
# which call it was.
pub fn assert_ok<T, E>(v: Result<T, E>, what: str) -> T {
    match v {
        Result::Ok(x) => x,
        Result::Err(_) => panic("assert_ok failed: {what} returned Err"),
    }
}

# The `Err` side, for a test that expects a failure.
pub fn assert_err<T, E>(v: Result<T, E>, what: str) -> E {
    match v {
        Result::Ok(_) => panic("assert_err failed: {what} returned Ok"),
        Result::Err(e) => e,
    }
}

# `lo <= v <= hi`, inclusive at both ends.
#
# Inclusive because the values a test bounds are usually measurements
# -- a byte count, an elapsed time -- and the endpoints of a
# measurement are legitimate answers.
pub fn assert_in_range(v: i64, lo: i64, hi: i64) {
    if v < lo || v > hi {
        panic("assert_in_range failed: {v} is outside [{lo}, {hi}]")
    }
}

# The unsigned form. Separate rather than generic because the message
# has to render the values, and a type parameter cannot be formatted.
pub fn assert_in_range_u64(v: u64, lo: u64, hi: u64) {
    if v < lo || v > hi {
        panic("assert_in_range_u64 failed: {v} is outside [{lo}, {hi}]")
    }
}

# ---------------------------------------------------------------------
# Allocation
#
# `ensures allocates(N)` (ALLOC-CONTRACT-SUGAR) states a *function's*
# budget. These state a *test's*: the interval between two points in
# one block, which is where "reading a segment does not grow the heap"
# lives. Both are wanted, and neither replaces the other.
#
# The counters are the ones `--profile=mem` reports, so a number here
# and a number in the profile mean the same thing.

# Live bytes right now, to be handed to `assert_no_growth` later.
pub fn heap_mark() -> u64 {
    __builtin_live_bytes()
}

# Nothing the program has done since `mark` is still holding memory.
#
# Live bytes rather than cumulative: a helper that allocates a buffer
# and frees it has not grown the heap, and a test that forbade the
# allocation outright would be testing the implementation rather than
# the promise.
pub fn assert_no_growth(mark: u64) {
    val now: u64 = __builtin_live_bytes()
    if now > mark {
        val grew: u64 = now - mark
        panic("assert_no_growth failed: heap grew by {grew} bytes ({mark} -> {now})")
    }
}

# The heap grew by at most `budget` bytes since `mark`.
#
# The form for code that legitimately allocates and wants to say how
# much. A failure reports the excess, not just the total, because the
# question is how far over the line it went.
pub fn assert_growth_at_most(mark: u64, budget: u64) {
    val now: u64 = __builtin_live_bytes()
    if now > mark {
        val grew: u64 = now - mark
        if grew > budget {
            val over: u64 = grew - budget
            panic("assert_growth_at_most failed: grew {grew} bytes, budget {budget}, over by {over}")
        }
    }
}

# ---------------------------------------------------------------------
# Golden files (TEST-TOOL T5)
#
# A format's promise -- "no change makes existing data unreadable" --
# is not a check until the bytes are written down. These compare a
# freshly-built buffer against a file recorded earlier, and report the
# first offset that moved.
#
# `toy test --bless` sets `TOY_BLESS`, and every `assert_golden` then
# *writes* its file instead of reading it. That is the only way to
# update one, and it is deliberately a separate run: a test that
# rewrites its own expectation whenever it fails is not a test.

# Whether this run is recording rather than checking.
fn blessing() -> bool {
    val v = io::env_var("TOY_BLESS")
    match v {
        Result::Ok(s) => s != "",
        Result::Err(_) => false,
    }
}

# `actual` matches the bytes stored at `path`.
#
# Paths are relative to the package root, which is where `toy` runs
# tests from. A missing file is a failure that names the remedy rather
# than a silent pass — recording on first sight would mean a test that
# has never once been looked at still goes green.
pub fn assert_golden(path: str, actual: Span<u8>) {
    if blessing() {
        val wrote = io::write_file_bytes(path, actual)
        match wrote {
            Result::Ok(n) => { println("blessed {path} ({n} bytes)") }
            Result::Err(e) => { panic("assert_golden: cannot write {path}: {e}") }
        }
        return
    }
    val sized = fs::file_size(path)
    val n: u64 = match sized {
        Result::Ok(v) => v,
        Result::Err(_) => {
            panic("assert_golden: no golden file at {path} -- run `toy test --bless` to record it")
        }
    }
    var buf: Vec<u8> = Vec::with_capacity(n)
    val room = buf.capacity_span()
    val window: Span<u8> = match room {
        Option::Some(s) => s,
        Option::None => { panic("assert_golden: cannot hold {n} bytes for {path}") }
    }
    val got = io::read_file_into(path, window)
    val read: u64 = match got {
        Result::Ok(v) => v,
        Result::Err(e) => { panic("assert_golden: cannot read {path}: {e}") }
    }
    buf.set_size(read)
    val stored = buf.as_span()
    val expected: Span<u8> = match stored {
        Option::Some(s) => s,
        Option::None => { panic("assert_golden: {path} is empty") }
    }
    # The offset report is the point: a 4 MiB file that moved one byte
    # is not helped by being told it differs.
    assert_bytes_eq(expected, actual)
}
