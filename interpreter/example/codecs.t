# hex and base64 (STDLIB-SERIALIZE S0 / S2).

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

# Decoding reports where it stopped, so print that rather than
# throwing it away.
fn show_decoded(label: str, r: Result<Vec<u8>, CodecError>) {
    match r {
        Result::Ok(v) => {
            val h: String = hex::encode(&v)
            println("{label}: {h}")
        }
        Result::Err(e) => { println("{label}: {e}") }
    }
}

fn main() -> u64 {
    val raw: Vec<u8> = bytes_of("toylang")

    val h: String = hex::encode(&raw)
    val b: String = base64::encode(&raw)
    println(h)
    println(b)

    # Either case reads back; the output is always lower case.
    val a1 = hex::decode("746F796C616E67")
    show_decoded("hex upper", a1)

    val a2 = base64::decode(b.to_str())
    show_decoded("base64", a2)

    # The refusals. Each one is a case where accepting the input
    # would mean it could no longer be told apart from damaged input.
    val e1 = hex::decode("abc")
    show_decoded("odd digits", e1)
    val e2 = hex::decode("00zz")
    show_decoded("not hex", e2)
    val e3 = base64::decode("Zg=")
    show_decoded("unpadded", e3)
    val e4 = base64::decode("ab-_")
    show_decoded("url-safe alphabet", e4)

    0u64
}
