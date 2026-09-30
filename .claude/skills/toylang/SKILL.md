---
name: toylang
description: toylang のプログラム (`.t` ファイル) を書く・直す・読むときの手引き。文法の罠 (elif、f64 サフィックス、セミコロンなし、所有と borrow) と、`toy` コマンドによる検査・修正・問い合わせの手順。`.t` を作成・編集する前、toylang の診断 (E00xx) を読むとき、toylang の型や定義を調べるときに使う。処理系 (Rust) 自体の開発には使わない。
---

# toylang を書く

toylang は Rust に似た見た目の、改行区切りの静的型付き言語。仕様の正本は
`docs/language.md`。ここにあるのは**書くときに踏みやすい罠**と、**`toy` で
確かめる手順**だけ。

## 手順: 書いたら toy に聞く

`toy` は release ビルドを直接叩く (`cargo run` より一桁速い)。無ければ
`cargo build --release -p toy`。

このリポジトリでは `.t` を Edit / Write するたびに **`toy hook` が自動で検査**
し、誤りがあれば 1 行 1 件で差し戻してくる (`.claude/settings.json` の
PostToolUse フック)。差し戻されたら、その行を直してから先へ進む。

1. **検査** — `target/release/toy check <パッケージか .t> --format=json`
   (1 行ずつ欲しければ `--format=short`)
   - 診断は `code` (E00xx)・`file`・`span`・`suggestions[].edits`・`related`
     (関連箇所) を持つ。`toy explain E00xx` で原因と直し方
2. **修正** — machine-applicable な提案は `target/release/toy fix <パッケージ>`
   で当てて再検査まで済む (`--dry-run` で先に見られる)。`maybe-incorrect` の
   提案 (欠けた match の腕、`.clone()`) は推測なので、読んでから手で当てる
3. **問い合わせ** — 推測せずに聞く:
   - `toy query type main.t:12:9` — その位置の式 / 束縛の型
   - `toy query def main.t:9:20` / `toy query refs main.t:5:4` — 定義と参照
   - `toy query callers NAME` / `toy query callees NAME`
   - `toy api core/std/string.t` — モジュールのシグネチャ一覧 (stdlib を grep しない)
4. **実行とテスト** — `toy run <パッケージ>`、`toy test <パッケージ>`
   (`test "名前" { assert_eq(a, b) }` ブロックを走らせる)

**`toy` はパッケージ単位で動く**: `main.t` か `src/` を持つディレクトリ
(その中の `.t` ファイルを渡してもよい)。単独の `.t` だけを置いた場所では
「no package found」になる。新しく作るなら `toy new <dir>` で雛形。
stdlib が見つからないと言われたら `TOYLANG_CORE_MODULES` にリポジトリの
`core` を指す。

## 文法の罠

- **`else if` は無い。`elif`** — `if a { } elif b { } else { }`。`else if` は E0033
- **小数リテラルはサフィックス必須** — `1.5f64` / `0.25f32`。`1.5` は E0034
  (タプル添字 `1 . 5` と読まれるため)。整数→浮動小数は `as f64`
- **セミコロンは書かない** — 文は改行で区切る。`;` は構文エラー
- **行頭の `-` と `&` は新しい式** — 前の行の続きにしたいなら、演算子を
  **前の行の末尾**に置く
- **整数リテラル** — `42u64` / `-3i64` / `7u8`。サフィックスなしの `42` は
  型を名指す位置 (注釈・引数・戻り値など) から決まり、決まらなければ `u64`。
  幅の違う整数どうしは暗黙に変換しない。`x as u8` と書く
- **関数は戻り値の型が必須**、`main() -> u64` が要る (戻り値が終了コード)
- 束縛は `val` (不変) / `var` (可変)。`val` への代入は E0044
- **`match` は網羅的** — 欠けた variant は E0035。パターンは
  `Enum::V(x)` / `Enum::V { f, .. }` / `_` / 範囲 `0u64..10u64` (半開)
- **`null` は無い** — 不在は `Option<T>`、失敗は `Result<T, E>`。
  `expr?` で早期 return、`a ?? b` で既定値。例外 (try/catch) は無く、
  回復不能なら `panic("...")`
- **所有** — `String` / `Vec<T>` / `Box<T>` は所有型で、値渡し・構造体への
  格納で**移動**する (移動後の使用は E0014)。読むだけなら `&String` / `&Vec<T>`
  で渡す。所有型の要素は `v.get(i)` ではなく `val e: &T = v.borrow(i)` (E0028)
- **文字列** — リテラルは `str`、伸びる文字列は `String::from_str("a")` と
  `push_str`。比較は `==`。補間 `"x = {x}"` の `{...}` の中は**単純な式**に
  する (文字列リテラルを含む式は書けない — 先に `val` に束縛する)
- 出力は `println(x)` / `print(x)`、stderr は `eprintln(x)`
- コメントは `#` と `/* */`

## 例 (検査・実行・テストが通ることを確かめたもの)

```rust
struct Point { x: i64, y: i64 }

fn classify(n: i64) -> str {
    if n > 10i64 { "big" } elif n > 0i64 { "small" } else { "none" }
}

fn parse_or_zero(s: str) -> u64 {
    parse::to_u64(s) ?? 0u64
}

fn total(v: &Vec<String>) -> u64 {
    var n = 0u64
    for i in 0u64..v.size() {
        val s: &String = v.borrow(i)
        n = n + s.len()
    }
    n
}

fn main() -> u64 {
    val p = Point { x: 1i64, y: 2i64 }
    val half: f64 = 1.5f64
    var names: Vec<String> = Vec::new()
    names.push(String::from_str("ab"))
    val kind = classify(p.y)
    val n = parse_or_zero("42")
    val len = total(&names)
    println("{kind} {half} {n} {len}")
    0u64
}

test "classify" {
    assert_eq(classify(20i64), "big")
}
```
