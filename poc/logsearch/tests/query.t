# QUERY.md §1 — 時刻の下限・上限がどう読まれるか。
#
# 時刻の境界は**枝刈りの入口**である。読めなかった値が 0 になると
# 「範囲指定なし」と区別がつかず、フィルタが落ちたまま全件が返る —
# それは空振りではなく**答えに見える誤り**なので、受理する書き方を
# 1 つずつ書き下しておく。

import std.time
import query

# 2030-01-01T00:00:00Z。以下の書き方はすべてこの 1 つの瞬間を指す。
fn y2030() -> i64 { 1893456000i64 }

fn from_of(text: str, now: i64) -> i64 {
    val q = query::parse_query(text, now)
    q.ts_from
}

fn to_of(text: str, now: i64) -> i64 {
    val q = query::parse_query(text, now)
    q.ts_to
}

# **日付だけの形**。`2030-01-01` は 10 文字で数字から始まるので、
# 「先頭が数字なら UNIX 秒」という判定に吸い込まれ、パースに失敗して
# 0 になっていた (2026-09-11 に修正)。境界が消えたまま全件を返すので、
# 速いクエリが遅いクエリに化けるだけでなく、答えの件数が変わる。
test "a date on its own is a date, not a failed number" {
    assert_eq(from_of("from=2030-01-01", 0i64), y2030())
    assert_eq(to_of("to=2030-01-01", 0i64), y2030())
}

test "the other three ways of naming the same instant agree" {
    assert_eq(from_of("from=1893456000", 0i64), y2030())
    assert_eq(from_of("from=2030-01-01T00:00:00Z", 0i64), y2030())
    # 相対指定は `now` からの引き算。
    val now = y2030() + 3600i64
    assert_eq(from_of("from=-1h", now), y2030())
    assert_eq(from_of("from=-60m", now), y2030())
    val day_later = y2030() + 86400i64
    assert_eq(from_of("from=-1d", day_later), y2030())
}

# 境界が無いことは 0 で表す。**読めなかった値も 0 になる**ので、
# 今のところ「指定しなかった」と「読めなかった」は同じに見える。
# 直す価値はあるが、まずは現状をここに固定しておく — 変えたときに
# このテストが落ちて、変えたことに気づける。
test "a bound nobody could read is indistinguishable from no bound" {
    assert_eq(from_of("status=404", 0i64), 0i64)
    assert_eq(from_of("from=yesterday", 0i64), 0i64)
    assert_eq(from_of("from=2030-1-1", 0i64), 0i64)
    assert_eq(from_of("from=", 0i64), 0i64)
}

# 範囲は半開である。上端に等しいレコードは入らない。
test "the range is half-open at the top" {
    val q = query::parse_query("from=2030-01-01 to=2030-01-02", 0i64)
    assert_eq(q.ts_from, y2030())
    assert_eq(q.ts_to, y2030() + 86400i64)
}
