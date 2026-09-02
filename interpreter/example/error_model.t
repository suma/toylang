# ERROR_MODEL: how this language carries a failure.
#
# Three kinds, and the kind decides the shape (D1):
#
#   A. the caller's bug          -> panic. Not catchable.
#   B. the world's circumstances -> Result<T, E>.
#   C. the budget ran out        -> panic by default; ask first to recover.
#
# The test is "can this happen to a correct program?". An index past
# the end of a `Vec` cannot -- the caller could have checked -- so it
# panics. A file that is not there can happen to anyone, so it is an
# `Err`. This program shows the B shape end to end: an aggregate error
# type, `?` across two different failures, `Display`, and the one
# `Err` that is not a failure at all.

# --- The aggregate error type (D6) ---------------------------------
#
# The application declares one error type and teaches it to absorb
# each library's. The stdlib deliberately does *not* provide
# `IoError -> ParseError` style conversions: n error types would need
# n^2 of them, and which direction is right is the application's
# business, not the library's.

enum ConfigError {
    Io(IoError),
    Parse(ParseError),
    # A payload does not have to be another error. `str` is preferred
    # over `String` here: an error value should not need the heap, and
    # a `String` payload would move under the ownership rules.
    Empty(str),
}

# One `From` impl per source error. Two impls on one type used to be
# impossible -- the second replaced the first -- which is what made
# the ordinary aggregate error type unwritable.
impl From<IoError> for ConfigError {
    fn from(value: IoError) -> Self { ConfigError::Io(value) }
}

impl From<ParseError> for ConfigError {
    fn from(value: ParseError) -> Self { ConfigError::Parse(value) }
}

# Every type in an `E` position implements `Display` (D2). That is the
# whole contract -- there is no `Error` trait, because `dyn` does not
# carry enums, so a trait could not be used to erase the type anyway.
#
# The text is a sentence fragment (D7): lower case, no full stop, no
# `error:` prefix. The caller supplies the frame it goes in.
impl Display for ConfigError {
    fn to_str(&self) -> str {
        match self {
            ConfigError::Io(_) => "config unreadable",
            ConfigError::Parse(_) => "config is not a number",
            ConfigError::Empty(_) => "config is empty",
        }
    }
}

# --- Propagating with `?` ------------------------------------------
#
# Each `?` converts through the `From` impl that matches the error it
# is carrying, so two unrelated failures reach one return type.
fn load_port(path: str) -> Result<u64, ConfigError> {
    val text: str = io::read_file(path)?          # IoError  -> ConfigError
    if text.len() == 0u64 {
        return Result::Err(ConfigError::Empty(path))
    }
    val port: u64 = parse::to_u64(text)?          # ParseError -> ConfigError
    Result::Ok(port)
}

# `?` in statement position, on a `Result<(), E>`: the call is made
# for its effect and only its failure is interesting.
fn require_nonempty(path: str) -> Result<(), ConfigError> {
    val text: str = io::read_file(path)?
    if text.len() == 0u64 {
        return Result::Err(ConfigError::Empty(path))
    }
    Result::Ok(())
}

fn check_then_load(path: str) -> Result<u64, ConfigError> {
    require_nonempty(path)?                       # no value to bind
    load_port(path)
}

# --- An `Err` that is not a failure (D4) ---------------------------
#
# `WouldBlock` is what a non-blocking socket says when there is simply
# nothing to read yet. It arrives as `Err` because that is the shape
# the syscall has. Propagating it with `?` would report an event
# loop's ordinary state as the program's failure, so it is branched on
# instead.
fn describe_net(e: NetError) -> str {
    if e.is_retryable() {
        "not ready yet -- ask again"
    } elif e.is_pending() {
        "connect still under way"
    } else {
        e.to_str()
    }
}

fn main() -> u64 {
    # A path that is not there. The reason survives the conversion and
    # prints through `Display`.
    val missing = check_then_load("/no/such/directory/toylang-config")
    val a = match missing {
        Result::Ok(p) => p,
        Result::Err(e) => {
            println(e)
            1u64
        }
    }

    # The same code path with a file that exists but does not hold a
    # number: a different library, a different error, one return type.
    val path = "/tmp/toylang_error_model_demo.txt"
    val w = io::write_file(path, "not-a-number")
    val b = match w {
        Result::Ok(_) => {
            val bad = check_then_load(path)
            match bad {
                Result::Ok(p) => p,
                Result::Err(e) => {
                    println(e)
                    2u64
                }
            }
        }
        Result::Err(_) => 0u64,
    }

    # And the success path, so the conversions are shown working
    # rather than only failing.
    val w2 = io::write_file(path, "8080")
    val c = match w2 {
        Result::Ok(_) => {
            val good = check_then_load(path)
            match good {
                Result::Ok(p) => p,
                Result::Err(e) => {
                    println(e)
                    0u64
                }
            }
        }
        Result::Err(_) => 0u64,
    }
    println(c)

    # Bound first: the compiled lanes want an enum argument to come
    # from a binding rather than be built in the call.
    val blocked = NetError::WouldBlock
    val pending = NetError::InProgress
    val refused = NetError::ConnectionRefused
    println(describe_net(blocked))
    println(describe_net(pending))
    println(describe_net(refused))

    # `expect` is the one place a caller says *why* a value had to be
    # there, so its message is the one that gets printed.
    val present: Option<u64> = Option::Some(5u64)
    val v = present.expect("the demo just put a value here")

    a + b + v
}
