# Levelled logging (STDLIB-LOG).
#
# Everything here goes to stderr; `println` below is the program's own
# output, and the two never mix.

fn work(n: u64) -> u64 {
    # The pattern to copy in a hot loop: read the level once, and put
    # the interpolation inside the branch. An argument is evaluated
    # before the call, so `log::trace("i={i}")` would build its string
    # on every iteration whatever the level is.
    val tracing: bool = log::enabled(Level::Trace)
    var total: u64 = 0u64
    for i in 0u64..n {
        total = total + i
        if tracing { log::trace("i={i} total={total}") }
    }
    total
}

fn main() -> u64 {
    # The starting level comes from `TOY_LOG`; set it here so the
    # output does not depend on the environment.
    log::set_level(Level::Info)

    log::info("starting")
    log::debug("this one is below the level, so it is not written")

    val total: u64 = work(4u64)
    println(total)

    log::set_level(Level::Trace)
    val traced: u64 = work(3u64)
    println(traced)

    log::warn("something looked odd")
    log::error("and then it failed")
    0u64
}
