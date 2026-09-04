# RUNTIME LIBRARY — 実プログラムを書けるようにする標準ライブラリの道筋

> 現状調査: 2026-08-30 (`core/std/` 21 ファイル ~3,450 行 + `toylang_rt` を全数で舐めた)
> 優先度の基準: 「これが無いと実プログラムが書けない」を最上位とする。
> 各項目の状態管理は `todo.md` (未実装節) が正本、この文書は**俯瞰と設計判断**を持つ。

## なぜこの文書が要るか

stdlib は機能別に必要に応じて landing してきた (Vec → String → Dict → iterator
アダプタ → …)。個々の項目は `todo.md` に載っているが、「実プログラムを 1 本
書こうとしたときに何が欠けているか」の俯瞰が一度も取れていなかった。
2026-08-30 に全モジュールを実際に読んで洗い出した結果、**書き込み系 IO・
パース・exit がまるごと無い** — 読んで計算して stdout に出す以外のことが
何もできないのが現状で、これが最優先。

## 現状の足場

### `core/std/` (toylang で書かれた層)

| モジュール | 提供するもの | 備考 |
|---|---|---|
| `collections/vec.t` | `Vec<T>` (push/pop/get/set/sort/iter + map/filter/enumerate/zip) | Drop glue 対応済み |
| `dict.t` | `Dict<K, V>` (insert/get/remove/iter) | **insert/get が線形探索** (`dict.t:56`) |
| `string.t` | `String` (nominal struct、byte buffer) | str→String 変換は `from_str` |
| `box.t` | `Box<T>` (再帰型の切断用) | |
| `allocator.t` | `trait Alloc` + `Global`/`Arena`/`FixedBuffer` | introspection API 付き |
| `io.t` | `read_line`/`argc`/`arg`/`env_*`/`read_file`/`file_exists`/`now`/`random`/`strftime` | **読み取り専用**。`Result<_, IoError>` 化済み |
| `math.t` | f64 libm intrinsics + min/max/abs | extern 形の模範実装 |
| `checked.t` | `trait Checked` の `checked_*` / `saturating_*` | **全 8 幅** (`u8`〜`u64` / `i8`〜`i64`、2026-08-31) |
| `hash.t` | `trait Hash { fn hash() -> u64 }` | primitive のみ、identity/parity の簡易実装。open addressing 用 mixer は将来と明記済み |
| `cmp.t` / `str.t` / `fmt.t` / `option.t` / `result.t` / `convert.t` / `drop.t` / `iter.t` | 契約と拡張 trait | `convert.t` は `From`/`Into` のみ、**パース関数は存在しない** |

### `toylang_rt` (Rust の層)

bump ヒープ (never-reuse、free 冪等) / メモリプロファイラ (`--profile=mem`) /
str 結合・`to_string`・format spec ヘルパ / IO extern 実装 (argv / env /
read_file / strftime) / panic・backtrace (shadow stack) / 出力シンク
(pthread_key TLS で per-thread 化済み — 並行性の下地)。

### 無いもの (実測)

- ~~**ファイル書き込み / 追記**~~ — `write_file` / `append_file` が入った (P0-A)
- ~~**stderr への user 出力**~~ — `eprint` / `eprintln` が入った (P0-A)
- ~~**`exit(code)`**~~ — `io::exit(code)` が入った (P0-A)
- ~~**str → 数値のパース**~~ — `core/std/parse.t` が入った (P0-B)
- **単調時計 / sleep** — `now()` は libc `time` の wall 秒のみ
- **`Set<T>` / 優先度付きキュー / deque**
- **ログ**

## 設計の制約 (既存の決定を引く)

1. **2 つの経路の使い分け** — ロジックは**純 toylang** (vec.t 形: struct +
   impl + heap builtins)、perf・OS 境界は**extern 形** (io.t / math.t 形:
   `extern fn __extern_*` + バックエンド別実装)。RUNTIME-PORT R3/R4 で
   「str 系ヘルパを toylang 化すると interpreter が 20〜1000 倍遅い」ことを
   実測済みなので、数値パース・時間・IO は Rust 側 (`toylang_rt`) に置く。
2. **新 extern は 4 箇所セット** — stdlib の `extern fn` 宣言 +
   `toylang_rt` 実装 + interpreter registry (`extern_io::build_io_registry`
   系) + `compiler/tests/consistency/` の 3 バックエンド一致テスト。
   `Result` を返すなら RUNTIME-IO の status extern ペア方式
   (payload を運ぶ extern が失敗 status を記録し、ペアの
   `__extern_*_status` が直後に読む) を使う。**戻りが scalar なら**
   (parse 等) status スロットで `Result` も `Option` も同じ機械で表現できる。
3. **allocator / 契約 / プロファイラとの整合** — 純 toylang
   コレクションは既存どおり `__builtin_heap_*` (active allocator 経由) で
   書く。そうすれば `--profile=mem`、`ensures allocates(N)`、
   REGION 検査 (E0022) が stdlib にも自動で効く。
   `toylang_rt` 直の malloc (str 保持等) はカウンタが数えない
   (MEM-COUNTER-INTERP-DRIFT で固定した定義) — 新規 Rust 側実装もこれに従う。
4. **決定性の規約を守る** — 時刻は UTC 固定、乱数は `random_seed` 再現、
   dict の反復順は「決定的」であること。単調時計・sleep は**本質的に
   非決定**なので、pin テストから除外する方針を最初から明記する。
5. **stdlib は toylang で書かれている利点を壊さない** — 型検査・エフェクト
   検査 (`--effects`)・DbC が stdlib の body にも効く (書き換えを伴う型検査
   のため「stdlib を検査しない」は禁じ手 — TEST-PERF 節の実測参照)。

## リストと優先順位

| 優先度 | ライブラリ | 内容 | 経路 | 状態 |
|---|---|---|---|---|
| **P0** | io 書き込み系 | `write_file` / `append_file` / `eprint` / `io::exit(code)` | extern | ✅ 2026-08-30 (P0-A) |
| **P0** | str パース | `parse::to_i64/to_u64/to_f64/to_bool(str) -> Result<_, ParseError>` | 純 toylang + extern 1 本 | ✅ 2026-08-30 (P0-B) |
| **P0** | 衛生項目 | ~~narrow int の `checked_*` (RUNTIME-TRAP-NARROW)~~ ✅ 2026-08-31 / ~~`str` の `Ord` (STDLIB-ORD)~~ ✅ 2026-09-03 / `arg(i)` 等の範囲外 `Result` 化 | 混在 | 残りは todo 既載 |
| **P1** | Dict hash 化 | 線形探索 → open addressing。`hash.t` の mixer 更新を含む | 純 toylang | ✅ 2026-09-02 (C1) |
| **P1** | `Set<T>` | hash 化した表を共有 | 純 toylang | ✅ 2026-09-02 (C2) |
| **P1** | Vec 拡張 | `insert`/`remove`/`contains`/`index_of`/`reverse`/`sort_by` | 純 toylang | ✅ 2026-09-03 (C3) |
| **P2** | 時間 | `now_mono()` / `sleep(ms)` / `DateTime` | extern | ✅ 2026-09-03 ([STDLIB_TIME](STDLIB_TIME.md)) |
| **P2** | PriorityQueue / Deque | `Vec<T>` + `Ord` の binary heap / ring buffer | 純 toylang | ✅ 2026-09-03 (C4 / C5) |
| **P2** | ロギング | レベル付き `log::at(level, msg)` → stderr | 純 toylang | ✅ 2026-09-03 ([STDLIB_LOG](STDLIB_LOG.md)) |
| **P3** | 並行性 | `spawn` + join ハンドル + channel | extern + コンパイラ | 設計文書から (todo CONCURRENCY) |
| **P4** | FFI P2 | dlopen / `NativeLibrary` | extern | FFI_PLAN Phase 2 設計済み |
| **P4** | JSON / hex / base64 | writer 先行、reader は後 | 純 toylang | ✅ 2026-09-03 ([STDLIB_SERIALIZE](STDLIB_SERIALIZE.md)) |
| **P4** | 暗号ハッシュ | SHA-256 → SHA-512 → HMAC。SHA-3 / BLAKE3 / パスワード KDF は非目標 | 純 toylang | C0/C1 ✅ 2026-09-04 ([STDLIB_CRYPTO](STDLIB_CRYPTO.md)) |
| 保留 | プロセス spawn / ネットワーク / regex / 多倍長 | 需要未確認 | — | 下記「非目標」 |

### 各項目の論点

**P0-A 実装メモ (2026-08-30、landing 済み)** — 3 つのうち `eprint` /
`eprintln` だけが**バックエンドの変更**になった。ファイル書き込みと
`exit` は extern 境界で済む (`toy_io_write_file` + ペアの status
extern、`exit` は libc をそのまま呼ぶ) 一方、stderr は print 経路その
ものにあるので:

- IR の 3 命令 (`Print` / `PrintStr` / `PrintRaw`) に **`stderr` フラグ**
  を足した。命令が自分でどちらの流れに出るかを言う形で、モードを
  バックエンドが持ち回る形にはしていない
- `toy_print_*` ヘルパは**二重化していない** (24 個が 48 個になる)。
  代わりに runtime に **`toy_print_stream(stderr)`** を 1 つ足し、
  AOT / JIT は stderr の print 命令をこの呼び出しで挟む。stdout 経路の
  命令数は不変
- runtime のシンクは 2 本になった (`sink` / `err_sink`)。JIT が stdout を
  捕捉しても stderr は素通りする — AOT バイナリと同じ見え方
- interpreter 側 JIT は `eprint` / `eprintln` を **silent fallback**
  (この JIT の print ヘルパは stdout 専用)

`exit` は in-process のレーンを道連れにするので consistency harness に
乗せられない (テストランナーごと落ちる)。子プロセスで exit コードを
pin する形にした。

**P0 io 書き込み系** — `exit` は `panic` と違い stderr に何も出さず、
指定コードで process exit する。`eprint` は `print` と同じ整形
(`to_str` / Display 経由) で stderr に書く。`write_file` /
`append_file` は `Result<_, IoError>` を返し、`IoError` に
`PermissionDenied` 等が既にあるので variant 追加は最小限
(書き込み専用 variant が要るかどうかだけ)。panic 経路の stderr 直書き
(`err_write`) が既にあるので Rust 側の下地は揃っている。

**P0-B 実装メモ (2026-08-30、landing 済み)** — `ParseError` は想定どおり
`Empty` / `Invalid` / `Overflow` の 3 variant。着手時に決めるとしていた
3 点は **すべて「厳しい側」**に倒し `docs/language.md` の
「Parsing numbers」に固定した: **trim しない** (呼び出し側が
`.trim()` できるが、勝手に trim すると厳しくできない)、**`+` / `-` は
受理** (`to_u64` の `-1` は wrap ではなく `Invalid`)、**`0x` / `_` は
不可** (あれはソースリテラルの構文で入力の構文ではない)。

**経路は表の「extern」から変わった** — 整数と bool のパーサは
**純 toylang** で書けた (`checked_mul` / `checked_add` が
オーバーフローを見てくれるので、範囲チェックが `Option` の match に
なる)。extern が要るのは `to_f64` の 10 進 → 2 進変換だけで、これは
正しく丸める実装を toylang で書き直すものではない。ただし
**文法の判定は toylang 側**に置いた: そうしないと interpreter の
Rust `str::parse` と compiled 側の libc `strtod` で受理する文字列が
食い違う (`inf` / hex float / 先頭空白)。変換に渡るのは両者が同じに
解釈する文字列だけになる。オーバーフローは status extern が
「結果が無限大か」で報告する (`1e999` は `Ok(inf)` ではなく
`Err(Overflow)`)。

**P0 str パース** — `ParseError` は `Empty` / `Invalid` / `Overflow` の
3 variant を想定 (u64 パースのオーバーフローは `Invalid` と区別が要る。
`checked_*` と同じ理由)。

**P1 Dict hash 化** — 並列配列 (keys/vals + 要素幅) の現 layout は
open addressing と相性が良い。論点は 3 つ:
(a) mixer の更新 (`hash.t` 自身が「将来は Wyhash / FxHash 相当が要る」と
明記している。trait 署名は安定契約なので実装差し替えで済む)、
(b) 削除の tombstone と成長閾値、
(c) **反復順** — 現在の `DictIter` は挿入順 (並列配列の並び)。
open addressing にすると物理順が変わる。**挿入順を維持する**
(index 配列を重ねる) か「順序は未規定」に仕様を引き下げるかを
docs/language.md で決めてから着手する。決定性 (seed 無しの純関数 hash)
はどちらでも保てる。

**→ この 3 点は [`COLLECTIONS.md`](COLLECTIONS.md) (2026-09-02) で決着
させた**: mixer は `Hash` impl ではなく**表側**に置く、反復順は
**挿入順を維持して仕様に書く** (`entries` + `slots` の IndexMap 形。
今の `remove` が swap-remove で既に順序を壊していることも実測)、
tombstone は 3 値 `slots` + load factor 7/8。

**P1 Vec 拡張** — `contains` / `index_of` / `remove` は「`T` に `==` が
要る」境界を書く必要がある。impl block の generic bound は呼び出し側で
強制される (E0010) ので機構は既にあるが、**operator `==` を bound で
要求する形が書けるか** (trait `Eq` を新設するか、structural `==` を
bound に使えるか) は着手時の実測事項。`sort_by(cmp: fn (T, T) -> bool)`
は comparator 引数で `Ord` 境界を回避できる。

**→ bound は要らないと実測で決着した** ([`COLLECTIONS.md`](COLLECTIONS.md)
の測定 1)。generic な `T` に対する `==` は 3 レーンで動き、`T` が `eq`
method を持つ struct ならそれに dispatch する。残る問題は逆で、`eq` を
**持たない**型を渡すと型検査を通って実行時に壊れた診断で落ちること
(同文書の C0)。

**P3 並行性** — 最小形は `spawn(fn () -> ())` + join ハンドル + channel
(todo CONCURRENCY)。下地は 2 つある: `toylang_rt` の `ThreadState` が
pthread_key TLS で per-thread 化済み (出力シンクも)、
EFFECT_SYSTEM.md「この先」に **「region を跨がない値 = 送れる値」という
`Send` 相当の定義**の見通しがある。本体は move / Drop モデルとの接合で、
**設計文書を別に取ってから**着手する。allocator スタックのスレッド間扱い
(per-thread にするか共有 arena を許すか) が最初の論点になる。

**P4 JSON** — writer は `Display` (`to_str`) の上に純 toylang で書ける。
reader は文字列走査のパーサで、`String` / `StringIter` の上に純 toylang で
書けるが工数は writer の数倍。実需要が出てから reader を切る。

## Phase 分割

| Phase | 内容 | 受け入れ基準 |
|---|---|---|
| **P0-A** ✅ | `write_file` / `append_file` / `eprint` / `exit` | 3 バックエンド一致テスト (書き込み先は tempfile fixture)。`exit` は終了コード pin |
| **P0-B** ✅ | `parse_*` 4 種 | 3 バックエンド一致 + `ParseError` の網羅 match pin。境界 (空文字 / MAX+1 / 先頭空白) の単体テスト |
| **P0-C** | 衛生項目 3 件 | todo.md の該当項目を解消済みとして移動 |
| **P1-A** | Dict hash 化 | 既存 dict テスト全 green (意味論不変) + 反復順の仕様固定 + 性能実測 (n=1e4 insert/get の前後比較) |
| **P1-B** | `Set<T>` | Dict と共通の表実装で 3 バックエンド一致 |
| **P1-C** | Vec 拡張 | 各 method の consistency テスト。bound の書けない項目は理由付きで除外せず、`Eq` 境界の設計を先に閉じる |
| **P2** | 時間 / PriorityQueue / Deque / log | `now_mono` は単調性のみ pin (値は pin しない)。sleep は非決定グリーン |
| **P3** | 並行性 | **設計文書 → Send 相当の判定 → `spawn`/join/channel** の順。着手前条件: 共有可変性が move / Drop モデルに載ること |
| **P4** | FFI P2 / JSON | FFI_PLAN の Phase 2 節に従う |

P0 の 3 つ (A/B/C) は同じ `io.t` / `toylang_rt` を触るので 1 セットで
landing するのが効率的。**P1-A の反復順の論点だけは P0 と並行して決めて
よい** (仕様変更で、実装と切離せる)。

## 非目標

- **async runtime / プロセス spawn** — bump ヒープ (never-reuse) と `fork` の
  相性 (COW 後の冪等 free は保たれるが fd・TLS の扱い) を検討する前に
  着手しない。async runtime は言語に並行性が入る前にランタイムだけ
  先行させない。
  **ネットワークはここから外した (2026-08-31)** — 「需要が未確認」と
  していたが、nonblocking socket + epoll/kqueue は**単一スレッドで
  完結する**ので P3 並行性を待つ必要がないと分かった。設計は
  [`NETWORK_IO.md`](NETWORK_IO.md) / [`EVENT_POLLING.md`](EVENT_POLLING.md)、
  状態は `todo.md` の NET。
- **str 系ヘルパの toylang 化** — RUNTIME-PORT R3/R4 で実測却下済み。
  Layer 1 (Rust) に置くのが確定。
- **regex / 多倍長 / 直列化フレームワーク** — toy 言語の用途に対して
  過剰。JSON writer の需要を見てから次を考える。
- **共通鍵暗号 / パスワード KDF / SHA-3 / BLAKE3** — ハッシュの範囲は
  [`STDLIB_CRYPTO.md`](STDLIB_CRYPTO.md) の §5 で線を引いた。定数時間を
  保証する手段がこの処理系に無いので、それを前提にする分野は開けない。
- **ゼロコスト抽象化の追求** — stdlib は読みやすさと 3 バックエンド互換を
  優先する (iterator アダプタが「普通の struct + next」で書かれているのと
  同じ判断)。

## 関連

- [`../todo.md`](../design-docs/todo.md) — 未実装節 (STDLIB-RUNTIME /
  RUNTIME-TRAP / 検討中の機能) が項目の状態の正本
- [`FFI_PLAN.md`](FFI_PLAN.md) — P4 の設計 (Phase 2 骨子あり)
- [`EFFECT_SYSTEM.md`](EFFECT_SYSTEM.md) — `Send` 相当の見通し、extern の
  エフェクト申告
- [`REGIONS.md`](REGIONS.md) — allocator スコープ脱出検査
  (P3 の「送れる値」の土台)
- [`../docs/language.md`](../docs/language.md) — 決定性規約・`Result`/
  `IoError`・extern の正本
