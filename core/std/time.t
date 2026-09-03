# Clocks, sleeping, measurement and dates (STDLIB-TIME).
#
# `io::now()` gave whole wall-clock seconds and nothing else, so
# neither a benchmark nor an exponential backoff could be written, and
# dates had output (`io::strftime`) with no input.
#
# `io::now()` and `io::strftime()` keep working; this module is where
# new code should look.

extern fn __extern_time_now_mono_ns() -> u64 from "toylang_rt" as "toy_time_now_mono_ns"
extern fn __extern_time_mono_res_ns() -> u64 from "toylang_rt" as "toy_time_mono_res_ns"
extern fn __extern_time_cpu_ns() -> u64 from "toylang_rt" as "toy_time_cpu_ns"
extern fn __extern_time_now_unix_ns() -> i64 from "toylang_rt" as "toy_time_now_unix_ns"
extern fn __extern_time_sleep_ns(ns: u64) from "toylang_rt" as "toy_time_sleep_ns"
extern fn __extern_time_civil_from_days(days: i64) -> i64 from "toylang_rt" as "toy_time_civil_from_days"
extern fn __extern_time_days_from_civil(packed: i64) -> i64 from "toylang_rt" as "toy_time_days_from_civil"

# ---------------------------------------------------------------------
# Clocks (TM0 / TM1).

# A monotonically non-decreasing nanosecond count.
#
# **The origin is unspecified.** Only differences mean anything: do
# not store one, and do not compare one across processes.
#
# **Non-decreasing, not strictly increasing.** Two reads closer
# together than `mono_res_ns()` give the same answer, so a test that
# demands `a < b` will eventually fail for no reason.
#
# **It does not advance while the machine is suspended.** If elapsed
# wall time is what you want, `now_unix_ns()` is the clock that has
# it.
pub fn now_mono_ns() -> u64 { __extern_time_now_mono_ns() }

# The monotonic clock's granularity, as the OS reports it.
pub fn mono_res_ns() -> u64 { __extern_time_mono_res_ns() }

# CPU time this process has consumed, user plus system. Non-decreasing
# like the monotonic clock, and unrelated to it: a sleeping process
# accumulates elapsed time and no CPU time.
pub fn cpu_time_ns() -> u64 { __extern_time_cpu_ns() }

# Nanoseconds since 1970-01-01T00:00:00Z. UTC, no leap seconds --
# Unix time by definition, so a leap second is simply the same value
# twice.
#
# **Not monotonic**: it jumps when the machine's clock is corrected.
pub fn now_unix_ns() -> i64 { __extern_time_now_unix_ns() }

# Whole seconds since the epoch. The same value `io::now()` gives.
pub fn now_unix_secs() -> i64 { now_unix_ns() / 1000000000i64 }

# Sleep for **at least** this long. A signal cannot cut it short --
# the retry is in the runtime rather than in every caller -- and there
# is no return value on purpose, because reporting the actual duration
# invites using it as a clock.
pub fn sleep_ns(ns: u64) { __extern_time_sleep_ns(ns) }
pub fn sleep_ms(ms: u64) { __extern_time_sleep_ns(ms * 1000000u64) }

# ---------------------------------------------------------------------
# Measuring (TM2).

# Elapsed time from a fixed point.
struct Stopwatch { start_ns: u64 }

impl Stopwatch {
    fn start() -> Self { Stopwatch { start_ns: now_mono_ns() } }

    fn elapsed_ns(&self) -> u64 { now_mono_ns() - self.start_ns }
    fn elapsed_ms(&self) -> u64 { self.elapsed_ns() / 1000000u64 }

    # The elapsed time, and back to zero in one step.
    fn restart(&mut self) -> u64 {
        val now: u64 = now_mono_ns()
        val was: u64 = now - self.start_ns
        self.start_ns = now
        was
    }
}

# There is no `bench` here.
#
# It was written -- read the clocks twice, run a closure `n` times,
# report the totals -- and it does not work: a call taking a
# `fn () -> ()` comes back with an unresolved return type, so the
# `Bench` it hands over cannot have a field read off it. A named
# function cannot be passed as a value either (`run(work)` is
# "expected fn () -> (), but got ()"), so a closure literal is the
# only spelling, and it fails the same way. Recorded as
# HOF-RETURN-UNKNOWN in design-docs/todo.md.
#
# `Stopwatch` covers the need in the meantime, and is what `bench`
# would have been built on:
#
#     val sw = Stopwatch::start()
#     var i: u64 = 0u64
#     while i < iters { ...; i = i + 1u64 }
#     val per_iter = sw.elapsed_ns() / iters
#
# The clocks stay outside the loop for the reason `bench` would have:
# a crossing costs about 6.7 µs on the interpreter, more than most
# things worth measuring.

# ---------------------------------------------------------------------
# Dates (TM3 / TM4).
#
# Always UTC. There is no time zone here: an offset is folded away
# when it is parsed, and nothing remembers it afterwards.
#
# **`DateTime` holds an instant, not seven fields.** The design called
# for `{ year, month, day, hour, minute, second, nanos }`, and that
# does not fit: a `Result<DateTime, TimeError>` is a tag plus seven
# fields plus the error, and cranelift refuses it -- "too many return
# values to fit in registers", the same eight-register budget
# COLLECTIONS works within. Two fields fit with room to spare.
#
# It is also the better shape for a second reason. The civil fields
# are *derived*, so they cannot disagree with each other: there is no
# way to hold a `DateTime` whose day is 31 and whose month is
# February. Validation happens once, where a date is built from parts.

struct DateTime { secs: i64, nanos: u32 }

# Why a failed date is not an `Option`: the three reasons a caller can
# act on differ. Vocabulary matched to `ParseError`.
pub enum TimeError {
    Empty,
    Invalid,
    OutOfRange,
}

impl Display for TimeError {
    fn to_str(&self) -> str {
        match self {
            TimeError::Empty => "empty date",
            TimeError::Invalid => "invalid date",
            TimeError::OutOfRange => "date out of range",
        }
    }
}

impl DateTime {
    # The instant `secs` seconds after the epoch. Negative seconds are
    # before 1970, and work.
    fn from_unix(secs: i64) -> Self { DateTime { secs: secs, nanos: 0u32 } }

    fn from_unix_nanos(secs: i64, nanos: u32) -> Self {
        DateTime { secs: secs, nanos: nanos }
    }

    # Build from civil parts, or say why they are not an instant.
    #
    # The month and day are checked **through the calendar** rather
    # than against a table of month lengths: a date that survives the
    # trip to a day number and back is a real one, which rejects
    # 2025-02-29 without anyone writing a leap-year rule down twice.
    #
    # An associated function rather than a free one: a free
    # `from_parts` collides with `Span::from_parts` at the
    # tree-walker's dispatch, which sent a span's `(ptr, len)` into
    # this function's `(year, month, ...)` and failed on the first
    # comparison.
    fn from_civil(
        year: i64, month: u32, day: u32,
        hour: u32, minute: u32, second: u32, nanos: u32,
    ) -> Result<DateTime, TimeError> {
        if month < 1u32 || month > 12u32 { return Result::Err(TimeError::Invalid) }
        if day < 1u32 || day > 31u32 { return Result::Err(TimeError::Invalid) }
        if hour > 23u32 || minute > 59u32 { return Result::Err(TimeError::Invalid) }
        # 60 would be a leap second, which Unix time has nowhere to put.
        if second > 59u32 { return Result::Err(TimeError::Invalid) }
        if nanos > 999999999u32 { return Result::Err(TimeError::Invalid) }
        val packed: i64 = (year * 65536i64) + ((month as i64) * 256i64) + (day as i64)
        val days: i64 = __extern_time_days_from_civil(packed)
        if __extern_time_civil_from_days(days) != packed {
            return Result::Err(TimeError::Invalid)
        }
        val secs: i64 = (days * 86400i64)
            + ((hour as i64) * 3600i64)
            + ((minute as i64) * 60i64)
            + (second as i64)
        val dt: DateTime = DateTime { secs: secs, nanos: nanos }
        Result::Ok(dt)
    }

    fn to_unix(&self) -> i64 { self.secs }
    fn nanos_part(&self) -> u32 { self.nanos }

    # The civil date, packed as the calendar hands it over:
    # `y * 65536 + m * 256 + d`. The three accessors below unpack it.
    fn civil(&self) -> i64 {
        __extern_time_civil_from_days(math::div_floor_i64(self.secs, 86400i64))
    }

    fn year(&self) -> i64 { self.civil() >> 16u64 }
    fn month(&self) -> u32 { ((self.civil() >> 8u64) & 255i64) as u32 }
    fn day(&self) -> u32 { (self.civil() & 255i64) as u32 }

    # Seconds into the day. Floor semantics, so a negative instant
    # lands on the previous day rather than truncating toward zero.
    fn day_secs(&self) -> i64 { math::mod_floor_i64(self.secs, 86400i64) }

    fn hour(&self) -> u32 { (self.day_secs() / 3600i64) as u32 }
    fn minute(&self) -> u32 { ((self.day_secs() % 3600i64) / 60i64) as u32 }
    fn second(&self) -> u32 { (self.day_secs() % 60i64) as u32 }

    # 0 = Sunday, matching `strftime`'s `%w`. 1970-01-01 was a
    # Thursday, so the epoch day is 4.
    fn weekday(&self) -> u32 {
        val d: i64 = math::mod_floor_i64(
            math::div_floor_i64(self.secs, 86400i64) + 4i64,
            7i64,
        )
        d as u32
    }

    # 1 for January 1st.
    fn day_of_year(&self) -> u32 {
        val jan1_days: i64 = __extern_time_days_from_civil(self.year() * 65536i64 + 256i64 + 1i64)
        val today: i64 = math::div_floor_i64(self.secs, 86400i64)
        ((today - jan1_days) + 1i64) as u32
    }
}

# ISO 8601, always with the `Z` -- this type is always UTC.
#
# `parse_iso8601(dt.to_str())` gives `dt` back, which is what the
# tests check rather than either half alone.
impl Display for DateTime {
    unsafe fn to_str(&self) -> str {
        var out: String = String::new()
        out.push_str(pad4_years(self.year()))
        out.push_str("-")
        out.push_str(pad2_field(self.month()))
        out.push_str("-")
        out.push_str(pad2_field(self.day()))
        out.push_str("T")
        out.push_str(pad2_field(self.hour()))
        out.push_str(":")
        out.push_str(pad2_field(self.minute()))
        out.push_str(":")
        out.push_str(pad2_field(self.second()))
        out.push_str("Z")
        out.to_str()
    }
}

fn pad2_field(v: u32) -> str {
    if v < 10u32 { "0{v}" } else { "{v}" }
}

fn pad4_years(v: i64) -> str {
    if v < 0i64 { return "{v}" }
    if v < 10i64 { return "000{v}" }
    if v < 100i64 { return "00{v}" }
    if v < 1000i64 { return "0{v}" }
    "{v}"
}

# ---------------------------------------------------------------------
# Parsing (TM4).
#
# **ISO 8601 only, and strictly.** A `strptime`-style runtime format
# string would hand the accepted set to libc, where it differs per
# OS -- the same reason `parse::to_f64` checks its grammar here and
# only crosses the boundary to convert. And `strftime` already owns
# *output* formats: two format languages, one per direction, is one
# too many.
#
#     YYYY-MM-DD
#     YYYY-MM-DDTHH:MM:SS
#     YYYY-MM-DDTHH:MM:SS.fff      (1-9 digits, truncated to ns)
#     ... then  Z  |  +HH:MM  |  -HH:MM
#
# Strict like `parse::to_u64`: no surrounding whitespace, fixed digit
# counts (`2026-9-3` is invalid), and ranges checked. An offset is
# folded to UTC on the spot.
pub unsafe fn parse_iso8601(s: str) -> Result<DateTime, TimeError> {
    val n: u64 = s.len()
    if n == 0u64 { return Result::Err(TimeError::Empty) }
    if n < 10u64 { return Result::Err(TimeError::Invalid) }
    val b: String = String::from_str(s)

    val year: i64 = match iso_digits(b, 0u64, 4u64) {
        Option::Some(v) => v as i64,
        Option::None => { return Result::Err(TimeError::Invalid) }
    }
    if b.get(4u64) != '-' { return Result::Err(TimeError::Invalid) }
    val month: u32 = match iso_digits(b, 5u64, 2u64) {
        Option::Some(v) => v as u32,
        Option::None => { return Result::Err(TimeError::Invalid) }
    }
    if b.get(7u64) != '-' { return Result::Err(TimeError::Invalid) }
    val day: u32 = match iso_digits(b, 8u64, 2u64) {
        Option::Some(v) => v as u32,
        Option::None => { return Result::Err(TimeError::Invalid) }
    }

    var hour: u32 = 0u32
    var minute: u32 = 0u32
    var second: u32 = 0u32
    var nanos: u32 = 0u32
    var i: u64 = 10u64
    var offset_secs: i64 = 0i64

    if i < n {
        if b.get(i) != 'T' { return Result::Err(TimeError::Invalid) }
        if i + 9u64 > n { return Result::Err(TimeError::Invalid) }
        hour = match iso_digits(b, i + 1u64, 2u64) {
            Option::Some(v) => v as u32,
            Option::None => { return Result::Err(TimeError::Invalid) }
        }
        if b.get(i + 3u64) != ':' { return Result::Err(TimeError::Invalid) }
        minute = match iso_digits(b, i + 4u64, 2u64) {
            Option::Some(v) => v as u32,
            Option::None => { return Result::Err(TimeError::Invalid) }
        }
        if b.get(i + 6u64) != ':' { return Result::Err(TimeError::Invalid) }
        second = match iso_digits(b, i + 7u64, 2u64) {
            Option::Some(v) => v as u32,
            Option::None => { return Result::Err(TimeError::Invalid) }
        }
        i = i + 9u64

        # Fractional seconds: 1-9 digits, truncated to nanoseconds.
        if i < n && b.get(i) == '.' {
            i = i + 1u64
            var seen: u64 = 0u64
            var scale: u32 = 100000000u32
            while i < n && seen < 9u64 {
                val c: u8 = b.get(i)
                val d: Option<u32> = c.digit_value(10u32)
                match d {
                    Option::Some(v) => {
                        nanos = nanos + v * scale
                        scale = scale / 10u32
                        seen = seen + 1u64
                        i = i + 1u64
                    }
                    Option::None => { break }
                }
            }
            if seen == 0u64 { return Result::Err(TimeError::Invalid) }
        }

        # Zone: `Z`, or an offset that is folded away here.
        if i < n {
            val z: u8 = b.get(i)
            if z == 'Z' {
                i = i + 1u64
            } elif z == '+' || z == '-' {
                if i + 6u64 > n { return Result::Err(TimeError::Invalid) }
                val oh: u64 = match iso_digits(b, i + 1u64, 2u64) {
                    Option::Some(v) => v,
                    Option::None => { return Result::Err(TimeError::Invalid) }
                }
                if b.get(i + 3u64) != ':' { return Result::Err(TimeError::Invalid) }
                val om: u64 = match iso_digits(b, i + 4u64, 2u64) {
                    Option::Some(v) => v,
                    Option::None => { return Result::Err(TimeError::Invalid) }
                }
                if oh > 23u64 || om > 59u64 { return Result::Err(TimeError::OutOfRange) }
                val mag: i64 = (oh as i64) * 3600i64 + (om as i64) * 60i64
                if z == '+' { offset_secs = mag } else { offset_secs = 0i64 - mag }
                i = i + 6u64
            } else {
                return Result::Err(TimeError::Invalid)
            }
        }
    }

    if i != n { return Result::Err(TimeError::Invalid) }

    val built = DateTime::from_civil(year, month, day, hour, minute, second, nanos)
    val dt: DateTime = match built {
        Result::Ok(v) => v,
        Result::Err(e) => { return Result::Err(e) }
    }
    if offset_secs == 0i64 { return Result::Ok(dt) }
    # Fold the offset away: the same instant, in UTC.
    val shifted: DateTime = DateTime { secs: dt.to_unix() - offset_secs, nanos: nanos }
    Result::Ok(shifted)
}

# `count` decimal digits starting at `at`, or `None` if any of them is
# not a digit. Fixed-width on purpose: the grammar has no variable
# fields.
unsafe fn iso_digits(b: String, at: u64, count: u64) -> Option<u64> {
    if at + count > b.size() { return Option::None }
    var acc: u64 = 0u64
    var i: u64 = 0u64
    while i < count {
        val c: u8 = b.get(at + i)
        val d: Option<u32> = c.digit_value(10u32)
        match d {
            Option::Some(v) => { acc = acc * 10u64 + (v as u64) }
            Option::None => { return Option::None }
        }
        i = i + 1u64
    }
    Option::Some(acc)
}

# ---------------------------------------------------------------------
# Formatting (TM5).

# Any `strftime` format, for a `DateTime`. Delegates rather than
# growing a second formatting engine.
pub fn format(dt: &DateTime, fmt: str) -> str {
    io::strftime(fmt, dt.to_unix() as u64)
}
