/*
 * Struct update syntax: `P { field: value, ..base }`
 *
 * Fields written explicitly win; every other field is copied from
 * `base`, which must be a value of the same struct. The result is a
 * new value, not a second name for `base`.
 */

struct Config {
    host: str,
    port: u64,
    retries: u64,
    verbose: bool
}

struct Rgba { r: u64, g: u64, b: u64, a: u64 }

struct Style {
    fg: Rgba,
    bg: Rgba,
    weight: u64
}

impl Config {
    # `self` is a path, so this fills `host` / `retries` / `verbose`
    # by reading it directly -- no temporary is needed.
    fn with_port(&self, port: u64) -> Config {
        Config { port: port, ..self }
    }
}

fn defaults() -> Config {
    Config { host: "localhost", port: 8080u64, retries: 3u64, verbose: false }
}

fn main() -> u64 {
    val base = defaults()

    # One field replaced, the rest carried over.
    val staging = Config { host: "staging.internal", ..base }
    println(staging.host)
    println(staging.port)
    println(staging.retries)

    # Through a method, with `self` as the base.
    val moved = staging.with_port(9090u64)
    println(moved.host)
    println(moved.port)

    # A copy is a copy: writing through it leaves the base alone.
    var mutable: Config = Config { verbose: true, ..base }
    mutable.port = 1u64
    println(base.port)
    println(mutable.port)
    println(mutable.verbose)

    # Struct-typed fields are carried over whole.
    val plain = Style {
        fg: Rgba { r: 255u64, g: 255u64, b: 255u64, a: 255u64 },
        bg: Rgba { r: 0u64, g: 0u64, b: 0u64, a: 255u64 },
        weight: 400u64
    }
    val bold = Style { weight: 700u64, ..plain }
    println(bold.fg.r)
    println(bold.bg.a)
    println(bold.weight)

    # The base may be any expression, not just a name. A call is
    # evaluated once no matter how many fields it fills -- that is what
    # the temporary this form keeps is for.
    val fresh = Config { verbose: true, ..defaults() }
    println(fresh.host)
    println(fresh.verbose)

    0u64
}
