# SHA-256 / SHA-224 from `core/std/crypto/`.
#
# Design: `design-docs/STDLIB_CRYPTO.md`.
#
# Run:
#   cargo run -q -p interpreter -- interpreter/example/crypto_sha256.t
#   cargo run -q -p compiler -- interpreter/example/crypto_sha256.t --all-backends

# The stdlib hashes byte buffers, and `str` indexes by byte through
# `String`, so this is the bridge from a literal to something to hash.
fn bytes_of(s: str) -> Vec<u8> {
    val t = String::from_str(s)
    var v: Vec<u8> = Vec::new()
    val n = t.size()
    var i = 0u64
    while i < n {
        v.push(t.get(i))
        i = i + 1u64
    }
    v
}

fn main() -> u64 {
    # One-shot. `println` of a `Sum` prints the hex, which is how
    # every one of these standards writes a digest down.
    val msg = bytes_of("abc")
    val d = sha256::sum(&msg)
    println(d)                        # ba7816bf...f20015ad, FIPS 180-4

    # SHA-224 is the same compression function with a different
    # starting chain and a shorter result.
    val d224 = sha256::sum_224(&msg)
    println(d224)                     # 23097d22...7bda255b32aadbce4bda0b3f7e36c9da7
    println(d.size())                 # 32
    println(d224.size())              # 28

    # Streaming: the input need not be in one buffer. Splitting it
    # anywhere gives the same digest, which is what lets a caller hash
    # a file it never holds whole.
    var h = Sha256::new()
    val part1 = bytes_of("a")
    val part2 = bytes_of("bc")
    h.update(&part1)
    h.update(&part2)
    val streamed = h.finalize()
    println(streamed == d)            # true

    # Comparing digests. `==` stops at the first byte that differs and
    # is right for "is this the file I already have"; `digest::ct_eq`
    # is the one written not to let its running time say how far the
    # match got, for checking something an attacker chose.
    val other = sha256::sum(&bytes_of("abd"))
    println(digest::ct_eq(&streamed, &d))      # true
    println(digest::ct_eq(&streamed, &other))  # false

    # The hex spelling comes from `hex::encode`, so it round-trips
    # through the same decoder.
    val text = d.to_hex()
    println(text)
    val back = hex::decode(text.to_str())
    val n = match back {
        Result::Ok(bs) => bs.size(),
        Result::Err(e) => 0u64,
    }
    println(n)                        # 32

    0u64
}
