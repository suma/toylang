# 高階関数 (HOF) と closure のデモ。
#
# Phase 5b で AOT compiler が `(T1, T2) -> R` 型 parameter と
# closure-as-argument に対応したので、ここに置いた free function
# はすべて 3 backend (interpreter / JIT silent fallback / AOT)
# で動作します。
#
# Note: `impl<T> Option<T> { fn map<U>(...) }` のような generic
# method + HOF の組み合わせは type checker の generic-method
# lookup + function-type の交互作用に未解決の課題があり、
# stdlib への追加は将来課題に残しています。当面は free function
# 形 (e.g. `option_map_i64`) で書くか、`match` を直接使う
# パターンが推奨。

# ---- HOF 1: 値を一回適用 ------------------------------------
fn apply(f: (i64) -> i64, x: i64) -> i64 {
    f(x)
}

# ---- HOF 2: 値を二回適用 ------------------------------------
fn apply_twice(f: (i64) -> i64, x: i64) -> i64 {
    f(f(x))
}

# ---- HOF 3: HOF を別の HOF にパススルー ----------------------
fn run_via(g: (i64) -> i64, x: i64) -> i64 {
    apply(g, x)
}

# ---- 自由関数形の Option::map (Option<i64> 専用) -------------
# 本来は `impl<T> Option<T> { fn map<U>(...) -> Option<U> }`
# にしたいが、generic method と function-typed parameter の
# 組み合わせが未対応なので具体型版で。
fn option_map_i64(o: Option<i64>, f: (i64) -> i64) -> Option<i64> {
    match o {
        Option::Some(v) => Option::Some(f(v)),
        Option::None => Option::None,
    }
}

fn main() -> i64 {
    # closure を val binding 経由で渡す
    val plus_two = fn(x: i64) -> i64 { x + 2i64 }
    println(apply(plus_two, 40i64))            # 42

    # closure literal を直接渡す
    println(apply(fn(x: i64) -> i64 { x * 3i64 }, 14i64))  # 42

    # 同じ closure を 2 回適用
    val plus_three = fn(x: i64) -> i64 { x + 3i64 }
    println(apply_twice(plus_three, 36i64))    # 42

    # HOF→HOF パススルー
    val plus_one = fn(x: i64) -> i64 { x + 1i64 }
    println(run_via(plus_one, 41i64))          # 42

    # Option::map 相当
    val o: Option<i64> = Option::Some(20i64)
    val mapped = option_map_i64(o, fn(x: i64) -> i64 { x + 22i64 })
    val result = match mapped {
        Option::Some(x) => x,
        Option::None => 0i64,
    }
    println(result)                            # 42

    0i64
}
