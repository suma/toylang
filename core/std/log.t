# Levelled logging (STDLIB-LOG).
#
# **Output goes to stderr and nowhere else.** stdout is the program's
# own output, and mixing the two breaks `prog > out.txt` for anyone
# who redirects it. Writing somewhere else is a shell redirect, and
# inside a test it is the runtime's error sink -- neither needs an
# API here.
#
# **In a hot loop, read the level once:**
#
#     val trace: bool = log::enabled(Level::Trace)
#     for i in 0u64..n {
#         if trace { log::trace("i={i}") }
#     }
#
# Nothing about this is a workaround for a missing feature: an
# argument is evaluated before the call, so `log::trace("i={i}")`
# builds its string whatever the level is. Only a branch can skip
# that, and the branch has to be written where the argument is. A
# level that removes the call at compile time cannot exist yet for
# three separate reasons (the stdlib cannot see a user `const`,
# arguments evaluate first, and a logging function cannot be
# `const fn` because `print` is not allowed in one).
#
# **A literal log does not allocate**, so `never_allocates fn` may
# call `log::info("started")` but not `log::info("port={p}")` --
# the interpolation is the allocation, and `[E0016]` names it.

extern fn __extern_log_level() -> u32 from "toylang_rt" as "toy_log_level"
extern fn __extern_log_set_level(level: u32) from "toylang_rt" as "toy_log_set_level"
extern fn __extern_log_timestamps() -> bool from "toylang_rt" as "toy_log_timestamps"

# Ordered loudest-first: a level is enabled when its rank is at most
# the active one, so `TOY_LOG=warn` shows errors and warnings.
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

pub fn level_rank(l: Level) -> u32 {
    match l {
        Level::Error => 0u32,
        Level::Warn => 1u32,
        Level::Info => 2u32,
        Level::Debug => 3u32,
        Level::Trace => 4u32,
    }
}

# `if` rather than `match` because a literal pattern is only
# available for `bool` / `i64` / `u64` / `str`, and this rank is the
# `u32` the extern hands back.
pub fn level_from_rank(rank: u32) -> Level {
    if rank == 0u32 {
        Level::Error
    } elif rank == 1u32 {
        Level::Warn
    } elif rank == 2u32 {
        Level::Info
    } elif rank == 3u32 {
        Level::Debug
    } else {
        Level::Trace
    }
}

# Upper case, and not padded to a common width: padding puts a
# trailing space inside the level name, which is then a space that
# anyone grepping for `INFO` has to know about.
pub fn level_name(l: Level) -> str {
    match l {
        Level::Error => "ERROR",
        Level::Warn => "WARN",
        Level::Info => "INFO",
        Level::Debug => "DEBUG",
        Level::Trace => "TRACE",
    }
}

impl Display for Level {
    fn to_str(&self) -> str { log::level_name(self) }
}

# The active level.
#
# It lives in the runtime rather than in a binding here, because this
# language has no mutable global and threading a logger value through
# every function that might log is how logging stops being used.
#
# Its initial value is `TOY_LOG` -- `error` / `warn` / `info` /
# `debug` / `trace`, lower case. Anything else is `info` with one
# warning line on stderr, since someone who misspells `debug`
# otherwise sees only that their logging never appears.
pub fn current_level() -> Level { log::level_from_rank(__extern_log_level()) }

pub fn set_level(l: Level) { __extern_log_set_level(log::level_rank(l)) }

# Whether a message at this level would be written. Hoist it out of a
# loop; see the header.
pub fn enabled(l: Level) -> bool { log::level_rank(l) <= __extern_log_level() }

# One record per line, `LEVEL message`.
#
# **This module never leans on a name it does not qualify.** Every
# call it makes to its own functions is written
# `log::` even though it is inside `log`. An unqualified call from a
# stdlib body resolves to a user function of the same name if the
# program defines one, so a program with its own `fn at(...)` used to
# break the logging module from the inside -- with the error reported
# against a line of `log.t`. Short names like `at`, `enabled` and
# `info` are exactly the ones a program is likely to reuse.
#
# Named `at` rather than `log`: every call is written through the
# module, and `log::log(Level::Warn, msg)` stutters where
# `log::at(Level::Warn, msg)` does not. (`log::log` does resolve --
# a free `log` here and `math::log` coexist -- so this is a reading
# choice, not a workaround.)
#
# A newline inside `msg` is left alone rather than escaped: these
# lines are for people to read, and a machine-readable form belongs
# in a serializer, not in an escape rule applied to every message.
pub fn at(l: Level, msg: str) {
    if log::enabled(l) {
        val name: str = log::level_name(l)
        if __extern_log_timestamps() {
            # ISO 8601 UTC, the one format this stdlib has --
            # `DateTime::to_str`, not a second spelling of it.
            val now: DateTime = DateTime::from_unix(time::now_unix_secs())
            val stamp: str = now.to_str()
            eprintln("{stamp} {name} {msg}")
        } else {
            eprintln("{name} {msg}")
        }
    }
}

pub fn error(msg: str) { log::at(Level::Error, msg) }
pub fn warn(msg: str) { log::at(Level::Warn, msg) }
pub fn info(msg: str) { log::at(Level::Info, msg) }
pub fn debug(msg: str) { log::at(Level::Debug, msg) }
pub fn trace(msg: str) { log::at(Level::Trace, msg) }
