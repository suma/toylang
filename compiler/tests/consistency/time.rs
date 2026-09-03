//! STDLIB-TIME: clocks, sleeping, and dates.
//!
//! What is pinned by value and what only by property is the whole
//! design of this file. The calendar and the parser are deterministic,
//! so they are compared byte for byte across the lanes; a clock is
//! not, so only its invariants are.

use super::harness::*;

#[test]
fn the_calendar_round_trips_through_the_awkward_instants() {
    // Every one of these has broken a date library: the epoch itself,
    // the second before it, a leap day in a leap century, a leap day
    // in an ordinary leap year, and a date before 1970 in a century
    // that is *not* a leap year.
    //
    // `to_str` emits ISO 8601 and `parse_iso8601` reads it, so the
    // round trip tests both against each other rather than either
    // against a table.
    let src = r#"
        fn show(r: Result<DateTime, TimeError>) -> u64 {
            match r {
                Result::Ok(v) => { println(v) }
                Result::Err(e) => { println(e) }
            }
            0u64
        }

        fn main() -> u64 {
            var v: Vec<i64> = Vec::new()
            v.push(0i64)
            v.push(-1i64)
            v.push(-86400i64)
            v.push(951782400i64)
            v.push(1709164800i64)
            v.push(-2208988800i64)
            var i: u64 = 0u64
            while i < v.size() {
                val secs: i64 = v.get(i)
                val d = DateTime::from_unix(secs)
                println(d)
                println(d.to_unix() == secs)
                val again = time::parse_iso8601(d.to_str())
                val _s1 = show(again)
                i = i + 1u64
            }
            0u64
        }
    "#;
    assert_stdout_consistent(src, "datetime_round_trip");
}

#[test]
fn the_parser_is_strict_and_says_which_way_it_failed() {
    // Strict like `parse::to_u64`: no surrounding whitespace, fixed
    // digit counts, ranges checked. The leap-day case is checked
    // *through the calendar* -- a date that survives the trip to a day
    // number and back is real -- so nobody writes a leap-year rule
    // down twice.
    let src = r#"
        fn show(r: Result<DateTime, TimeError>) -> u64 {
            match r {
                Result::Ok(v) => { println(v) }
                Result::Err(e) => { println(e) }
            }
            0u64
        }

        fn main() -> u64 {
            val r1 = time::parse_iso8601("")
            val _s2 = show(r1)
            val r2 = time::parse_iso8601("2026-9-3")
            val _s3 = show(r2)
            val r3 = time::parse_iso8601(" 2026-01-01")
            val _s4 = show(r3)
            val r4 = time::parse_iso8601("2026-01-01 ")
            val _s5 = show(r4)
            val r5 = time::parse_iso8601("2025-02-29")
            val _s6 = show(r5)
            val r6 = time::parse_iso8601("2024-02-29")
            val _s7 = show(r6)
            val r7 = time::parse_iso8601("2026-13-01")
            val _s8 = show(r7)
            val r8 = time::parse_iso8601("2026-00-01")
            val _s9 = show(r8)
            val r9 = time::parse_iso8601("2026-01-32")
            val _s10 = show(r9)
            val r10 = time::parse_iso8601("2026-01-01T25:00:00Z")
            val _s11 = show(r10)
            val r11 = time::parse_iso8601("2026-01-01T00:60:00Z")
            val _s12 = show(r11)
            # A leap second has nowhere to live in Unix time.
            val r12 = time::parse_iso8601("2026-01-01T00:00:60Z")
            val _s13 = show(r12)
            val r13 = time::parse_iso8601("2026-01-01T12:00:00")
            val _s14 = show(r13)
            val r14 = time::parse_iso8601("2026-01-01T12:00:00Z")
            val _s15 = show(r14)
            # An offset is folded away on the spot; nothing remembers it.
            val r15 = time::parse_iso8601("2026-01-01T12:00:00+09:00")
            val _s16 = show(r15)
            val r16 = time::parse_iso8601("2026-01-01T12:00:00-05:30")
            val _s17 = show(r16)
            val r17 = time::parse_iso8601("2026-01-01T12:00:00.5Z")
            val _s18 = show(r17)
            val r18 = time::parse_iso8601("2026-01-01T12:00:00.123456789Z")
            val _s19 = show(r18)
            val r19 = time::parse_iso8601("2026-01-01T12:00:00.Z")
            val _s20 = show(r19)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "iso8601_strict");
}

#[test]
fn the_calendar_agrees_with_strftime() {
    // §4 keeps one implementation of the calendar, shared with
    // `strftime`. This is that decision checked from the outside: if
    // a second one ever appears, these two lines stop matching.
    let src = r#"
        fn main() -> u64 {
            var v: Vec<i64> = Vec::new()
            v.push(0i64)
            v.push(1709164800i64)
            v.push(951782400i64)
            v.push(-2208988800i64)
            var i: u64 = 0u64
            while i < v.size() {
                val t: i64 = v.get(i)
                val d = DateTime::from_unix(t)
                println(io::strftime("%Y-%m-%d", t as u64))
                println(time::format(d, "%Y-%m-%d"))
                # `weekday` is 0 = Sunday, the same as `%w`.
                println(io::strftime("%w", t as u64))
                println(d.weekday())
                i = i + 1u64
            }
            0u64
        }
    "#;
    assert_stdout_consistent(src, "calendar_vs_strftime");
}

#[test]
fn day_of_year_counts_from_one() {
    let src = r#"
        fn main() -> u64 {
            val jan1 = DateTime::from_unix(1704067200i64)
            println(jan1)
            println(jan1.day_of_year())
            # 2024 is a leap year, so the last day is 366.
            val dec31 = DateTime::from_unix(1735603200i64)
            println(dec31)
            println(dec31.day_of_year())
            0u64
        }
    "#;
    assert_stdout_consistent(src, "day_of_year");
}

#[test]
fn the_clocks_hold_their_invariants() {
    // Values are never pinned: they differ every run and between
    // machines. What is pinned is what the doc comments promise.
    //
    // `a <= b`, not `a < b` -- the monotonic clock is non-decreasing,
    // so two reads closer together than its resolution give the same
    // answer and a strict comparison would fail at random.
    //
    // The sleep is checked from below only. There is no upper bound to
    // check against: a loaded machine can oversleep by any amount, and
    // a test that says otherwise fails in CI rather than in the code.
    let src = r#"
        fn main() -> u64 {
            val a = time::now_mono_ns()
            val b = time::now_mono_ns()
            println(a <= b)
            println(time::mono_res_ns() > 0u64)
            val c0 = time::cpu_time_ns()
            var i: u64 = 0u64
            var acc: u64 = 0u64
            while i < 20000u64 { acc = acc + i i = i + 1u64 }
            println(time::cpu_time_ns() >= c0)
            # The wall clock agrees with the one `io` already had.
            println(time::now_unix_secs() == (io::now() as i64))
            val sw = Stopwatch::start()
            time::sleep_ms(20u64)
            println(sw.elapsed_ms() >= 15u64)
            0u64
        }
    "#;
    assert_stdout_consistent(src, "clock_invariants");
}
