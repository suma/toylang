# STDLIB LOG — レベル付きログ、状態をどこに置くか

> 対象: 新設する `core/std/log.t` と、`toylang_rt` の出力シンク
> 状態の正本: [`todo.md`](todo.md) の **STDLIB-LOG**
> 俯瞰と優先順位: [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) の P2
> 調査: 2026-09-03

## Status snapshot

| 項目 | 状態 |
|---|---|
| stderr への出力 | `eprint` / `eprintln` (RUNTIME-LIB P0-A) |
| 出力シンク | runtime に 2 本 (`sink` / `err_sink`)。**差し替え可能**で、`pthread_key` の TLS で per-thread |
| レベル | 無い |
| 言語の可変なグローバル | **無い** (top-level は `const` のみ。しかも module の `const` は他モジュールにも自分の body にも届かない — MODULE-CONST) |
| 環境変数で runtime を切り替える先例 | `TOY_PROFILE_MEM` (`toylang_rt` が `getenv` で読む) |

## なぜ今これを設計するか

todo が挙げていた論点は 2 つだった — **出力先を stderr 固定にするか**と、
**レベルのコンパイル時除去を `const fn` で畳めるか**。前者は 3 行で
決まるが、**後者の答えは「畳めない」で、理由が 3 つある**。それを
書き残さないと、同じ問いが毎回持ち上がる。

そしてこの分野の本題は、実は**状態をどこに置くか**にある。この言語には
可変なグローバルが無い。ログレベルという「プロセスに 1 つの可変な値」は、
**言語の中に置き場所が無い**。

## 調べたこと (2026-09-03)

1. **出力の口はもうある。** `eprint` / `eprintln` は IR の print 命令に
   `stderr` フラグを持たせる形で通っていて、AOT / JIT は
   `toy_print_stream(stderr)` で挟む。**ログのために新しい出力の
   extern は要らない。**

2. **シンクは差し替えられる。** `toylang_rt` は `sink` / `err_sink` の
   2 本を持ち、`set_err_sink` で差し替えられる (`lib.rs:678`)。
   **テストがログ出力を捕まえられる**ということで、これは受け入れ条件を
   書くうえで重要。

3. **シンクは per-thread、環境変数は process-global。**
   シンクは `pthread_key_create` の TLS (`lib.rs:140`、`#[thread_local]`
   が使えないため)。一方 `TOY_PROFILE_MEM` は `getenv` で
   プロセス全体 (`lib.rs:1021`)。**レベルは後者の側**に置く。

4. **module の `const` は届かない** (MODULE-CONST)。`poll.t` が
   `pub const` をあきらめて `pub fn interest_read()` を並べている
   のと同じ制約。**stdlib のログ関数から user の
   `const LOG_LEVEL` は見えない。**

5. **`const fn` は `print` / `println` を呼べない** (COMPILE-TIME-EVAL の
   適格性、`E0017`)。**ログ関数は定義上 `const fn` になれない。**

## 既存の決定から引く制約

1. **stdout はプログラムの出力、stderr は診断** (`eprint` を入れた
   ときの区別)。
2. **決定性** — 4 レーン一致のテストに乗せるには、既定の出力が
   実行ごとに変わってはいけない。
3. **extern 1 回のコスト**: interpreter +6.7 µs / AOT +5 ns
   (STDLIB_TIME 実測 3)。**レベルの読みは extern 1 回**になるので、
   抑制されたログもその分は払う。
4. **`never_allocates` の検査** — 文字列補間は確保する。
5. **失敗しない**。ログは `Result` を返さない (書けなくても
   プログラムは進む)。

## 1. 出力先は stderr 固定

理由は 3 つで、どれも単独で足りる:

- **stdout はプログラムの出力**。ログを混ぜると、`prog > out.txt` が
  壊れる。
- **差し替えはもう runtime にある** (調査 2)。ファイルに出したい人は
  シェルのリダイレクトで足りる。
- **出力先を選べる API は状態を増やす** — 次の §2 の問題を 2 倍にする。

`log::` は `eprintln` の上に**純 toylang**で書く (新しい extern を
出力側に足さない)。結果として `set_err_sink` がそのまま効く。

## 2. レベルはどこに置くか — runtime に 1 つ

置ける場所は 3 つしかない:

| 案 | 評価 |
|---|---|
| (a) 呼び出し側が `Logger` 値を持ち回る | 状態は増えないが、**全関数の引数にログが生える**。使われなくなる |
| (b) user の top-level `const` | **stdlib から見えない** (調査 4) |
| (c) runtime に 1 つ持ち、extern で読み書き | `TOY_PROFILE_MEM` と同じ形 (調査 3) |

**(c) を採る。**

```
extern fn __extern_log_level() -> u32 from "toylang_rt" as "toy_log_level"
extern fn __extern_log_set_level(level: u32) from "toylang_rt" as "toy_log_set_level"
```

- **初期値は環境変数 `TOY_LOG`** — runtime が最初の読みで一度だけ
  `getenv` する。値は `error` / `warn` / `info` / `debug` / `trace` の
  小文字のみ。**不正な値は既定 (`info`) にして、警告を 1 行 stderr に
  出す** (黙って無視すると、綴りを間違えた人は「ログが出ない」だけを
  見る)。
- `log::set_level(l)` はプログラムから上書きする口。`--test` の中で
  使う。

## 3. コンパイル時にレベルを畳めるか — **畳めない**

todo の問いへの答え。理由が 3 つ積み重なっている:

1. **stdlib は user の `const` を見られない** (調査 4)。
   `const LOG_LEVEL` を書いてもらっても、`log::debug` の body からは
   参照できない。
2. **引数はレベル判定より先に評価される。** `log::debug("x={v}")` の
   補間は**呼び出しの前**に走る。レベルで消せるのは
   「stderr に書くこと」だけで、**費用の大部分 (文字列の組み立て) は
   消えない**。遅延評価は `??` の右辺のように**言語が知っている位置**
   でしか作れない。
3. **ログ関数は `const fn` になれない** (調査 5)。`print` / `println` は
   `const fn` の適格性が禁じている。

**そのかわりに置くもの:**

```
pub fn enabled(level: Level) -> bool
```

熱いループでは**ループの外で 1 回読んで `bool` を持つ**:

```
val trace: bool = log::enabled(Level::Trace)
for i in 0u64..n {
    if trace { log::trace("i={i}") }     # 補間は分岐の中
}
```

これは言語の機能を 1 つも増やさずに、3 つの費用 (extern の読み /
補間 / 書き込み) を全部消す。**doc comment の先頭にこの形を書く。**

将来この形が要らなくなるとしたら、それは**マクロ**が入ったとき
(`assert_eq` がパーサマクロで一時束縛に展開されているのと同じ機構) —
それは言語側の話なので、この分野では待たない。

## 4. レベルと形式

```
pub enum Level { Error, Warn, Info, Debug, Trace }
pub fn error(msg: str) / warn / info / debug / trace
pub fn log(level: Level, msg: str)
pub fn level_name(l: Level) -> str
impl Display for Level
```

**出力の形は 1 行 1 レコード:**

```
ERROR could not open config
INFO  listening on port 8080
```

- **既定でタイムスタンプを付けない** (制約 2)。付けると 4 レーンの
  出力比較ができなくなる。
- `TOY_LOG_TIME=1` で先頭に **ISO 8601 (UTC)** を足す:
  `2026-09-03T12:34:56Z INFO listening`。これは STDLIB_TIME の
  `DateTime::to_str` をそのまま呼ぶ (書式を 2 つ持たない)。
- **レベル名は 5 文字に揃えない** — 揃えると `INFO ` の末尾の空白が
  「意味のある空白」になり、grep する人が引っかかる。1 スペース区切り。
- **改行は `eprintln` が付ける**。`msg` に改行が入っていても
  何もしない (エスケープしない) — ログは人が読むもので、
  機械が読むなら §6 の JSON を使う。

## 5. `never_allocates` との関係

`log::info("started")` (リテラル) は確保しない。
`log::info("port={p}")` は**補間が確保する**ので、
`never_allocates` な関数の中では**リテラルのログだけ**が書ける。

これは制限ではなく**そう書いてある通りに検査される**ということで、
`E0016` の診断が到達経路を出すので気づける。doc comment に 1 行書く。

## 6. 構造化ログは今は置かない

`key=value` や JSON 行 (`{"level":"info","msg":...}`) は、
STDLIB_SERIALIZE の `JsonWriter` (S1) の上に**純 toylang で 20 行**で
書ける。だが:

- **フィールドを渡す API** (`log::info_kv("port", p)`) は、任意個・
  任意型の引数を要求する。この言語に可変長引数は無い。
- struct を渡す形にすると derive 相当が要る (SERIALIZE §7 の非目標)。

→ **`log::json(level, body: str)`** だけを置く余地を残す (呼び出し側が
`JsonWriter` で本体を組み立てる)。S1 が landing してから決める。

## Phase 分割

| Phase | 内容 | 受け入れ |
|---|---|---|
| **L0** | `Level` / 5 つの関数 / `set_level` / `enabled` / extern 2 本 / `TOY_LOG` | 4 レーン一致。**シンクを差し替えて出力文字列を突き合わせる** (調査 2) |
| **L1** | `TOY_LOG` の不正値の警告、`log(level, msg)`、`Display for Level` | 同上 + 環境変数の表を pin |
| **L2** | `TOY_LOG_TIME=1` のタイムスタンプ | STDLIB_TIME の TM3 (`DateTime`) の後。既定 off なので L0 のテストは無変更 |
| **L3** | (任意) `log::json` | STDLIB_SERIALIZE の S1 の後 |

L0 だけで実用になる。**L2 を分けてあるのは、タイムスタンプが
分野をまたぐ依存 (TIME) を持ち込む唯一の項目だから。**

## 非目標

- **出力先の切り替え API** (`log::to_file(path)`) — §1。runtime の
  `set_err_sink` とシェルのリダイレクトで足りる。
- **ログの回転 / サイズ制限** — ファイルに書かないので要らない。
- **モジュールごとのレベル** (`TOY_LOG=net=debug,json=info`) —
  「どのモジュールから呼ばれたか」を知る手段が無い
  (`__builtin_function_name()` は自分の名前しか答えない)。
  フィルタが要るなら `grep`。
- **非同期 / バッファリング** — stderr は既に行バッファ。並行性が
  入ってから考える。
- **構造化ログ** — §6。
- **コンパイル時のレベル除去** — §3。マクロが入ったときに再訪する。

## 関連

- [`RUNTIME_LIBRARY.md`](RUNTIME_LIBRARY.md) — P2 (`eprint` 依存と
  書いてあった項目)
- [`STDLIB_TIME.md`](STDLIB_TIME.md) — L2 のタイムスタンプ
- [`STDLIB_SERIALIZE.md`](STDLIB_SERIALIZE.md) — L3 の JSON 行
- [`COMPILE_TIME_EVAL.md`](COMPILE_TIME_EVAL.md) — `const fn` が
  `print` を禁じている根拠 (§3 の理由 3)
- [`EFFECT_SYSTEM.md`](EFFECT_SYSTEM.md) — `never_allocates` と io
  エフェクト (§5)
