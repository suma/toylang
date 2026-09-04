//! `core/std/crypto/` (STDLIB-CRYPTO C0 / C1).
//!
//! Pinned against the published FIPS 180-4 vectors rather than
//! against a round trip. A hash has no inverse to round-trip through,
//! and "the streaming form agrees with the one-shot form" only says
//! the two spellings share a bug — both are checked here, but neither
//! stands in for the vectors.

use super::harness::{assert_consistent, assert_renders};

/// `str` indexes by byte through `String`, so every test needs the
/// same bridge from a literal to something hashable.
const HELPERS: &str = r#"
    fn bytes_of(s: str) -> Vec<u8> {
        val t: String = String::from_str(s)
        var v: Vec<u8> = Vec::new()
        var i: u64 = 0u64
        while i < t.size() {
            v.push(t.get(i))
            i = i + 1u64
        }
        v
    }

    fn repeat_byte(b: u8, n: u64) -> Vec<u8> {
        var v: Vec<u8> = Vec::new()
        var i: u64 = 0u64
        while i < n {
            v.push(b)
            i = i + 1u64
        }
        v
    }
"#;

/// FIPS 180-4's own examples, plus a length that runs many blocks.
///
/// The three short ones sit either side of the padding's awkward
/// case: `"abc"` pads inside its only block, the 56-byte message
/// leaves no room for the length and forces a second block, and the
/// empty message is all padding.
#[test]
fn sha256_matches_the_published_vectors() {
    let src = format!(
        r#"
        {HELPERS}
        fn main() -> u64 {{
            val a = bytes_of("abc")
            val da = sha256::sum(&a)
            println(da)

            val e = bytes_of("")
            val de = sha256::sum(&e)
            println(de)

            # 56 bytes: one byte too long to hold its own length.
            val m = bytes_of("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")
            val dm = sha256::sum(&m)
            println(dm)

            # 112 bytes: exactly two blocks of message, so the padding
            # gets a block to itself.
            val l = bytes_of("abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu")
            val dl = sha256::sum(&l)
            println(dl)

            # 1000 bytes: enough blocks that a bug in carrying state
            # from one to the next shows up.
            val r = repeat_byte(0x61u8, 1000u64)
            val dr = sha256::sum(&r)
            println(dr)
            0u64
        }}
    "#
    );
    assert_renders(
        &src,
        "sha256_vectors",
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n\
         e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
         248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1\n\
         cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1\n\
         41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3\n",
    );
}

/// SHA-224 differs from SHA-256 only in its starting chain and in
/// dropping the eighth word, so it is worth its own vectors: a
/// truncation applied to the wrong chain still produces 28
/// plausible-looking bytes.
#[test]
fn sha224_matches_the_published_vectors() {
    let src = format!(
        r#"
        {HELPERS}
        fn main() -> u64 {{
            val a = bytes_of("abc")
            val da = sha256::sum_224(&a)
            println(da)
            println(da.size())

            val e = bytes_of("")
            val de = sha256::sum_224(&e)
            println(de)

            val m = bytes_of("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")
            val dm = sha256::sum_224(&m)
            println(dm)
            0u64
        }}
    "#
    );
    assert_renders(
        &src,
        "sha224_vectors",
        "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7\n\
         28\n\
         d14a028c2a3a2bc9476102bb288234c415a2b01f828ea62ac5b3e42f\n\
         75388b16512776cc5dba5da1fd890150b0c6455cb4f58b1952522525\n",
    );
}

/// Where the caller splits the input must not change the answer.
///
/// One byte at a time is the split that exercises every path through
/// the buffer: it lands on a block boundary 1 time in 64 and in the
/// middle the rest of the time. A 112-byte message crosses the
/// boundary twice.
#[test]
fn streaming_in_any_chunking_agrees_with_the_one_shot_form() {
    let src = format!(
        r#"
        {HELPERS}
        fn main() -> u64 {{
            val m = bytes_of("abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu")
            val once = sha256::sum(&m)

            var h = Sha256::new()
            var i: u64 = 0u64
            while i < m.size() {{
                var one: Vec<u8> = Vec::new()
                one.push(m.get(i))
                h.update(&one)
                i = i + 1u64
            }}
            val byte_at_a_time = h.finalize()

            # And a split that lands exactly on the first block
            # boundary, where `nbuf` wraps to 0 between the calls.
            var g = Sha256::new()
            var head: Vec<u8> = Vec::new()
            var tail: Vec<u8> = Vec::new()
            i = 0u64
            while i < m.size() {{
                if i < 64u64 {{ head.push(m.get(i)) }} else {{ tail.push(m.get(i)) }}
                i = i + 1u64
            }}
            g.update(&head)
            g.update(&tail)
            val split = g.finalize()

            var ok: u64 = 0u64
            if byte_at_a_time == once {{ ok = ok + 1u64 }}
            if split == once {{ ok = ok + 2u64 }}
            ok
        }}
    "#
    );
    assert_consistent(&src, "sha256_streaming");
}

/// `digest::ct_eq` has to agree with `==` on the answer — it differs
/// only in how it spends its time getting there — including on the
/// length mismatch it folds in rather than short-circuits on.
#[test]
fn constant_time_compare_agrees_with_ordinary_equality() {
    let src = format!(
        r#"
        {HELPERS}
        fn main() -> u64 {{
            val a = sha256::sum(&bytes_of("abc"))
            val b = sha256::sum(&bytes_of("abc"))
            val c = sha256::sum(&bytes_of("abd"))
            # Different length: SHA-224 of the same message.
            val d = sha256::sum_224(&bytes_of("abc"))

            println(digest::ct_eq(&a, &b))
            println(a == b)
            println(digest::ct_eq(&a, &c))
            println(a == c)
            println(digest::ct_eq(&a, &d))
            println(a == d)
            0u64
        }}
    "#
    );
    assert_renders(
        &src,
        "sha256_ct_eq",
        "true\ntrue\nfalse\nfalse\nfalse\nfalse\n",
    );
}

/// The hex a digest prints is `hex::encode`'s, so it decodes back
/// through `hex::decode`. This is what keeps `Sum::to_hex` from
/// growing a second spelling of the same alphabet.
#[test]
fn a_digest_prints_hex_that_hex_decode_reads_back() {
    let src = format!(
        r#"
        {HELPERS}
        fn main() -> u64 {{
            val d = sha256::sum(&bytes_of("abc"))
            val text = d.to_hex()
            # Bound rather than matched in place: the compiled lanes
            # need a `val` between a call and an enum `match`.
            val back = hex::decode(text.to_str())
            var n: u64 = 0u64
            var first: u64 = 0u64
            match back {{
                Result::Ok(bs) => {{ n = bs.size()  first = bs.get(0u64) as u64 }}
                Result::Err(e) => {{ n = 0u64 }}
            }}
            # 32 bytes back, and the first one is 0xba.
            n + first
        }}
    "#
    );
    assert_consistent(&src, "sha256_hex_round_trip");
}
