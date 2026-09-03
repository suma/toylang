//! `core/std/hex.t` and `core/std/base64.t` (STDLIB-SERIALIZE S0 / S2).
//!
//! The encoders are checked against RFC 4648's own test vectors
//! rather than against a round trip: a round trip passes for any pair
//! of functions that disagree with the world in the same direction,
//! which is precisely the bug that matters in an interchange format.

use super::harness::assert_renders;

/// Shared helpers.
///
/// Decoded bytes are shown as hex rather than as text: the
/// interesting inputs decode to bytes that are not valid UTF-8, and
/// `String::to_str` panics on those by design (STDLIB-TEXT §2). Hex
/// is printable for every byte, and it makes each test cross-check
/// the other codec.
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

    # Decoded bytes are shown as hex, not as text: the interesting
    # inputs decode to bytes that are not valid UTF-8, and
    # `String::to_str` panics on those by design (STDLIB-TEXT).
    #
    # These print rather than return the line, because a `str`-
    # returning function whose match arms are blocks that build their
    # own `str` is refused by the AOT lowering
    # (AOT-MATCH-STR-ARM-BLOCK).
    fn show_hex(s: str) {
        val r = hex::decode(s)
        match r {
            Result::Ok(v) => {
                val h: String = hex::encode(&v)
                println(h)
            }
            Result::Err(e) => { println(e) }
        }
    }

    fn show_b64(s: str) {
        val r = base64::decode(s)
        match r {
            Result::Ok(v) => {
                val h: String = hex::encode(&v)
                println(h)
            }
            Result::Err(e) => { println(e) }
        }
    }

    # For inputs whose bytes really are text.
    fn show_b64_text(s: str) {
        val r = base64::decode(s)
        match r {
            Result::Ok(v) => {
                var t: String = String::new()
                var i: u64 = 0u64
                while i < v.size() {
                    t.push(v.get(i))
                    i = i + 1u64
                }
                println(t)
            }
            Result::Err(e) => { println(e) }
        }
    }
"#;

#[test]
fn hex_writes_lower_case_and_reads_either() {
    let src = format!(
        r#"
        {HELPERS}
        fn main() -> u64 {{
            var v: Vec<u8> = Vec::new()
            v.push(0u8)
            v.push(15u8)
            v.push(171u8)
            v.push(255u8)
            val enc: String = hex::encode(&v)
            println(enc)
            # Either case reads back, and both give the same bytes.
            show_hex("000fabff")
            show_hex("000FABFF")
            # An empty input is a length of zero bytes, not a failure.
            show_hex("")
            0u64
        }}
    "#
    );
    assert_renders(&src, "hex_roundtrip", "000fabff\n000fabff\n000fabff\n\n");
}

#[test]
fn hex_reports_where_it_stopped() {
    let src = format!(
        r#"
        {HELPERS}
        fn main() -> u64 {{
            # An odd digit count is a length problem, not a character
            # problem: there is no way to know which half is missing.
            show_hex("abc")
            # The offset is into the text the caller passed, so it can
            # be pointed at.
            show_hex("00zz")
            # Nothing but digits: no `0x`, no space, no separator.
            # A space makes the count odd, so that one is reported as
            # a length rather than as a character.
            show_hex("0xff")
            show_hex("00 ff")
            0u64
        }}
    "#
    );
    assert_renders(&src, "hex_errors", "truncated input\ninvalid character at byte 2\ninvalid character at byte 1\ntruncated input\n");
}

#[test]
fn base64_matches_the_rfc_4648_vectors() {
    // Section 10 of the RFC, all seven, which is the only reason to
    // trust the padding arithmetic at all.
    let src = format!(
        r#"
        {HELPERS}
        fn enc(s: str) -> String {{
            val v: Vec<u8> = bytes_of(s)
            val out: String = base64::encode(&v)
            out
        }}

        fn main() -> u64 {{
            val e0: String = enc("")
            val e1: String = enc("f")
            val e2: String = enc("fo")
            val e3: String = enc("foo")
            val e4: String = enc("foob")
            val e5: String = enc("fooba")
            val e6: String = enc("foobar")
            println(e0.eq_str(""))
            println(e1.eq_str("Zg=="))
            println(e2.eq_str("Zm8="))
            println(e3.eq_str("Zm9v"))
            println(e4.eq_str("Zm9vYg=="))
            println(e5.eq_str("Zm9vYmE="))
            println(e6.eq_str("Zm9vYmFy"))
            show_b64_text("Zm9vYmFy")
            show_b64_text("Zg==")
            show_b64_text("Zm8=")
            0u64
        }}
    "#
    );
    assert_renders(&src, "base64_vectors", "true\ntrue\ntrue\ntrue\ntrue\ntrue\ntrue\nfoobar\nf\nfo\n");
}

#[test]
fn base64_refuses_what_it_cannot_tell_apart_from_damage() {
    let src = format!(
        r#"
        {HELPERS}
        fn main() -> u64 {{
            # Unpadded input reads exactly like truncated input.
            show_b64("Zg=")
            show_b64("Zm9vYmE")
            # `-` and `_` are the URL-safe alphabet, a different
            # encoding that shares the name.
            show_b64("ab-_")
            # MIME's line wrapping is not part of the encoding. The
            # length check reports first here, which is the honest
            # answer: a wrapped line is not a whole number of groups.
            show_b64("Zm9v\nYmFy")
            # Padding in the middle is not padding.
            show_b64("Zg==Zg==")
            # `Zh==` would be a second spelling of the byte `Zg==`
            # already encodes, so the unused bits have to be zero.
            show_b64("Zh==")
            0u64
        }}
    "#
    );
    assert_renders(&src, "base64_strict", "truncated input\ntruncated input\ninvalid character at byte 2\ntruncated input\ninvalid character at byte 2\ninvalid character at byte 1\n");
}

#[test]
fn every_byte_survives_both_codecs() {
    // Byte 0 and byte 255 are the two that a sign-extended or
    // NUL-terminated implementation loses, and neither is reachable
    // from a string literal.
    let src = format!(
        r#"
        {HELPERS}
        fn main() -> u64 {{
            var v: Vec<u8> = Vec::new()
            var i: u64 = 0u64
            while i < 256u64 {{
                v.push(i as u8)
                i = i + 1u64
            }}
            val h: String = hex::encode(&v)
            val b: String = base64::encode(&v)
            println(h.size())
            println(b.size())
            var same_hex: bool = true
            val back_h = hex::decode(h.to_str())
            match back_h {{
                Result::Ok(d) => {{
                    var j: u64 = 0u64
                    while j < 256u64 {{
                        if d.get(j) != v.get(j) {{ same_hex = false }}
                        j = j + 1u64
                    }}
                }}
                Result::Err(_) => {{ same_hex = false }}
            }}
            println(same_hex)
            var same_b64: bool = true
            val back_b = base64::decode(b.to_str())
            match back_b {{
                Result::Ok(d) => {{
                    var j: u64 = 0u64
                    while j < 256u64 {{
                        if d.get(j) != v.get(j) {{ same_b64 = false }}
                        j = j + 1u64
                    }}
                }}
                Result::Err(_) => {{ same_b64 = false }}
            }}
            println(same_b64)
            0u64
        }}
    "#
    );
    assert_renders(&src, "codec_all_bytes", "512\n344\ntrue\ntrue\n");
}

/// The vector paths (SIMD.md strategy B) start at 32 bytes of hex
/// text and 32 characters of base64, and each leaves a tail to the
/// scalar loop. Lengths on both sides of every boundary, so a chunk
/// that is one element short — or a tail that starts one element
/// early — shows up here rather than as a wrong byte in the middle
/// of somebody's file.
#[test]
fn codec_round_trips_across_the_vector_boundaries() {
    let src = format!(
        r#"
        {HELPERS}
        fn round(n: u64) -> bool {{
            var v: Vec<u8> = Vec::new()
            var k: u64 = 0u64
            while k < n {{
                v.push(((k * 37u64 + 5u64) % 256u64) as u8)
                k = k + 1u64
            }}
            val h: String = hex::encode(v)
            val b: String = base64::encode(v)
            if h.len() != n * 2u64 {{ return false }}
            if b.len() != ((n + 2u64) / 3u64) * 4u64 {{ return false }}
            var ok: bool = true
            val dh = hex::decode(h.to_str())
            match dh {{
                Result::Ok(d) => {{
                    if d.size() != n {{ ok = false }}
                    var j: u64 = 0u64
                    while j < n {{
                        if d.get(j) != v.get(j) {{ ok = false }}
                        j = j + 1u64
                    }}
                }}
                Result::Err(_) => {{ ok = false }}
            }}
            val db = base64::decode(b.to_str())
            match db {{
                Result::Ok(d) => {{
                    if d.size() != n {{ ok = false }}
                    var j: u64 = 0u64
                    while j < n {{
                        if d.get(j) != v.get(j) {{ ok = false }}
                        j = j + 1u64
                    }}
                }}
                Result::Err(_) => {{ ok = false }}
            }}
            ok
        }}
        fn main() -> u64 {{
            var bad: u64 = 0u64
            var n: u64 = 0u64
            while n < 70u64 {{
                if !round(n) {{ bad = bad + 1u64 }}
                n = n + 1u64
            }}
            println(bad)
            0u64
        }}
    "#
    );
    assert_renders(&src, "codec_vector_boundaries", "0\n");
}

/// A bad character *after* a whole vector chunk still reports its own
/// index.
///
/// This is the part of the vector path that could quietly regress
/// into a worse diagnostic: the chunk loop cannot say which lane was
/// wrong, so it stops and hands the position back to the scalar loop.
/// Get that handoff wrong and the error still says `Invalid`, just
/// about the wrong byte — which no round-trip test would catch.
#[test]
fn codec_reports_the_bad_character_after_a_full_chunk() {
    let src = format!(
        r#"
        {HELPERS}
        fn hex_bad(s: str) -> u64 {{
            val d = hex::decode(s)
            match d {{
                Result::Ok(_) => 9999u64
                Result::Err(e) => {{
                    match e {{
                        CodecError::Invalid(k) => k
                        CodecError::BadLength => 8888u64
                    }}
                }}
            }}
        }}
        fn b64_bad(s: str) -> u64 {{
            val d = base64::decode(s)
            match d {{
                Result::Ok(_) => 9999u64
                Result::Err(e) => {{
                    match e {{
                        CodecError::Invalid(k) => k
                        CodecError::BadLength => 8888u64
                    }}
                }}
            }}
        }}
        fn main() -> u64 {{
            # 32 good hex digits (one whole chunk), then a bad one.
            println(hex_bad("00112233445566778899aabbccddeeffZ0"))
            # A bad digit inside the first chunk.
            println(hex_bad("0011223344556677889_aabbccddeeff"))
            # 36 good base64 characters (two chunks' worth of the
            # bound), then a bad one.
            println(b64_bad("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA!AAA"))
            # A bad character inside the first chunk.
            println(b64_bad("AAAAAAAA!AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"))
            0u64
        }}
    "#
    );
    assert_renders(
        &src,
        "codec_bad_character_after_chunk",
        "32\n19\n36\n8\n",
    );
}
