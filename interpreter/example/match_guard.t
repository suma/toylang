# match arm に `if <cond>` の guard を付けると、パターンが一致した
# 後にその guard を評価し、false の場合は次の arm にフォールスルー
# する。pattern bindings は guard 内で参照できる。
#
# 各 arm の振る舞い (classify):
#   classify(0)   -> 0  (リテラル一致)
#   classify(-3)  -> 1  (Name v + guard v < 0)
#   classify(50)  -> 2  (Name v + guard v < 100)
#   classify(500) -> 3  (wildcard)
#
# classify_pair はタプルパターン + guard:
#   classify_pair((0, 0))     -> 0
#   classify_pair((1, 200))   -> 4   (n > 100)
#   classify_pair((1, 2))     -> 9   (default)
fn classify(n: i64) -> i64 {
    match n {
        0i64 => 0i64,
        v if v < 0i64 => 1i64,
        v if v < 100i64 => 2i64,
        _ => 3i64,
    }
}

fn classify_pair(p: (i64, i64)) -> i64 {
    match p {
        (0i64, 0i64) => 0i64,
        (x, y) if x == y => 1i64,
        (_, n) if n > 100i64 => 4i64,
        _ => 9i64,
    }
}

fn main() -> i64 {
    classify(0i64)                 # 0
        + classify(0i64 - 3i64)    # 1
        + classify(50i64)          # 2
        + classify(500i64)         # 3
        + classify_pair((1i64, 200i64))   # 4
}
