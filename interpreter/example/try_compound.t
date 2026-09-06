# TRY-COMPOUND: `?` when what comes out is not a scalar.
#
# `expr?` unwraps a `Result<T, E>` and returns early on `Err`. The
# interesting half is what `T` is allowed to be. For a while it had to
# be a scalar: the desugar typed its success arm as `__try_v as T`, and
# `as` is a scalar conversion in every backend, so `Point as Point` was
# not a no-op but a refusal. A compound success type therefore had no
# working spelling and every caller wrote the `match` by hand.
#
# Now the arm ends in the bare binding when `T` is a compound, and all
# four shapes below work: a struct, a tuple, an enum, and a generic
# instance the `val` never names.

fn temp_dir() -> str {
    val v = io::env_var("TMPDIR")
    match v {
        Result::Ok(dir) => dir,
        Result::Err(_) => "/tmp",
    }
}

struct Extent { start: u64, len: u64 }

enum Kind { Header, Body(u64) }

# --- 1. a struct -----------------------------------------------------

fn extent_of(len: u64) -> Result<Extent, str> {
    if len == 0u64 {
        Result::Err("empty")
    } else {
        Result::Ok(Extent { start: 16u64, len: len })
    }
}

fn last_byte(len: u64) -> Result<u64, str> {
    val e = extent_of(len)?
    Result::Ok(e.start + e.len - 1u64)
}

# --- 2. a tuple ------------------------------------------------------

fn split(n: u64) -> Result<(u64, u64), str> {
    if n < 2u64 { Result::Err("too small") } else { Result::Ok((n / 2u64, n % 2u64)) }
}

fn halves(n: u64) -> Result<u64, str> {
    val pair = split(n)?
    Result::Ok(pair.0 * 10u64 + pair.1)
}

# --- 3. an enum ------------------------------------------------------

fn classify(tag: u64) -> Result<Kind, str> {
    if tag == 0u64 {
        Result::Ok(Kind::Header)
    } elif tag < 100u64 {
        Result::Ok(Kind::Body(tag))
    } else {
        Result::Err("unknown tag")
    }
}

fn weight(tag: u64) -> Result<u64, str> {
    val k = classify(tag)?
    match k {
        Kind::Header => Result::Ok(0u64),
        Kind::Body(n) => Result::Ok(n),
    }
}

# --- 4. a generic instance, with no annotation to read it from -------
#
# `val v = digits(n)?` never spells `Vec<u64>`. The instance comes out
# of the scrutinee's own variant payload, which the callee's signature
# already instantiated.

fn digits(n: u64) -> Result<Vec<u64>, str> {
    if n == 0u64 { return Result::Err("no digits") }
    var out: Vec<u64> = Vec::new()
    var rest = n
    while rest > 0u64 {
        out.push(rest % 10u64)
        rest = rest / 10u64
    }
    Result::Ok(out)
}

fn digit_sum(n: u64) -> Result<u64, str> {
    val v = digits(n)?
    var sum: u64 = 0u64
    var i: u64 = 0u64
    while i < v.size() {
        sum = sum + v.get(i)
        i = i + 1u64
    }
    Result::Ok(sum)
}

# --- 5. an owning payload -------------------------------------------
#
# `File` closes its descriptor when it dies, which is what makes this
# the shape worth showing: the value `?` hands out is the payload of a
# temporary the desugar made, and the temporary must not take the file
# with it when it goes.

fn write_then_measure(path: str) -> Result<u64, IoError> {
    val out = File::create(path)?
    var text = String::from_str("nine byte")
    val window = text.as_span() ?? panic("no span")
    val written = out.write(window)?
    out.close()?

    val back = File::open(path)?
    val size = back.size()?
    if size != written { return Result::Err(IoError::ReadError) }
    Result::Ok(size)
}

fn main() -> u64 {
    println(last_byte(4u64) ?? 0u64)          # 19
    println(last_byte(0u64) ?? 0u64)          # 0  -- the Err path
    println(halves(7u64) ?? 0u64)             # 31
    println(weight(0u64) ?? 999u64)           # 0
    println(weight(42u64) ?? 999u64)          # 42
    println(weight(500u64) ?? 999u64)         # 999 -- the Err path
    println(digit_sum(1234u64) ?? 0u64)       # 10
    println(digit_sum(0u64) ?? 0u64)          # 0  -- the Err path

    val path: str = "{temp_dir()}/toylang_example_try_compound.bin"
    println(write_then_measure(path) ?? 0u64) # 9
    0u64
}
