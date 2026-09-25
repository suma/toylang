# CODE-SIZE — 生成バイナリが太る理由

> 実装: [`compiler/src/codegen/mod.rs`](../compiler/src/codegen/mod.rs)
> (`cranelift_signature_with_writeback`)、
> [`compiler/src/codegen/lower_inst.rs`](../compiler/src/codegen/lower_inst.rs)
> (`lower_self_writeback`)
> 計測対象: `poc/logsearch` (toylang 5,109 行) を AOT `--release` で
> aarch64 macOS に。2026-09-05。

## 要点

`&mut self` メソッドの ABI が **struct を leaf スカラーに展開して
引数と戻り値に並べる**。leaf 数が引数レジスタ本数 (aarch64 で 8) を
超えると、超えた分は呼び出しのたびにメモリを往復する。

`ArchiveWriter` (19 フィールド → **52 leaf**) の 16 メソッドだけで
**toylang 生成コードの 29%** を占めた。太い関数は全体の 10% しか
ないのに、コードの **49%** を持っていく。

コード品質そのものは良い — 8 leaf 以下なら 1 フィールドを +1 する
メソッドは **3 命令**に落ちる。問題は codegen ではなく署名の形。

**進捗** (すべて 2026-09-05): 戻り側は CODE-SIZE-WB-PRUNE、引数側は
S1+S2 (幅の広い by-reference **receiver**) と S3a (`&T` / `&mut T` の
**compound 引数**)。`__text` は合わせて **191,080 → 162,912 B (−14.7%)**。
S3b (幅の広い**ローカル束縛**を stack slot に常駐) と、演算子
オーバーロードを含む**あらゆる参照引数**まで入り、`__text` は
**191,080 → 153,056 B (−19.9%)**。

## 2026-09-17 の再計測 — 残っている幅広は別の形

`poc/logsearch` は当時より育っている (706 関数、`__text` 292,148 B)。
幅の広い署名 (params + returns > 16) は **38 本**で、`ArchiveWriter` の
メソッドはもう上位に居ない。残るのは:

- **値で返す compound** — `ArchiveWriter::new` は戻り値 60、
  `query::parse_query` 25、`segfile::head_of` 24。多くは 1 回しか呼ばない
- **8 leaf 以下の参照引数が何本も並ぶ関数** — `query::search` は参照 7 本で
  19 引数。閾値は**引数ごと**に見るので、1 本ずつは閾値以下でも合計は膨らむ

閾値を署名全体で見る形に広げる価値を測るため、`PTR_SELF_LEAF_THRESHOLD`
を下げて実測した:

| 閾値 | `__text` | archive (3 回の最速) | `status=404` の検索 |
|---|---:|---:|---:|
| 8 (現状) | 292,148 B | 1,615 ms | 0.46 s |
| 4 | 290,472 B (−0.6%) | 1,611 ms | 0.45 s |
| 2 | `ptr_self_verify` がビルドを止める | — | — |

閾値 4 の出力セグメントは閾値 8 と 1 バイトも違わない。**−0.6% のために
3 レーンに跨る ABI を触る価値は無い**ので、この線は追わない。閾値 2 で
止まったのは `String::concat` の呼び出し 1 か所で、沈黙ではなく
ビルドエラーとして出た (2 重の網が効いている)。

## 計測 — 331,832 B の内訳

| 部分 | bytes | 中身 |
|---|---:|---|
| `__text` | 191,080 | 機械語。toylang 生成 86% / Rust runtime 12% |
| `__const` | 37,776 | 診断文字列 (下記) |
| `__LINKEDIT` | 65,536 | シンボル表。`strip -x` で 291 KB (−40 KB) |
| その他 + ページ整列 | ~37,000 | macOS の 16 KB セグメント境界 |

**デッドコード削除は効いている。** auto-load は `core/` 下の `.t` を
全部読むが、`base64` / `hex` / `json` / `sha256` / `net` / `poll` の
シンボルは 1 つも入らない。下限は測れる:

| プログラム | ファイル | `__text` |
|---|---:|---:|
| `fn main() -> u64 { 0u64 }` | 16,792 | 8 |
| `println("hi")` を足す | 54,168 | 4,172 |
| `poc/logsearch` (5,109 行) | 331,832 | 191,080 |

空プログラムの 16.8 KB はページ整列で、コードではない。

## 主因 — `&mut self` writeback が O(struct) の呼出規約になる

`&mut self` メソッドは「self の全 leaf を引数で受け、全 leaf を
戻り値で返す」形に lower される
([`BACKEND.md`](BACKEND.md) の `CallWithSelfWriteback`)。CLIF を見ると:

```
toy_ArchiveWriter__write_seg:  58 params -> 53 returns
toy_ArchiveWriter__emit_terms: 63 params -> 52 returns
toy_ArchiveWriter__ts_min:     52 params ->  1 return   # 1 フィールド読むだけ
```

`ts_min()` が 52 引数取るのが症状を一番よく表している。**読むだけの
getter でも self 全体が呼出規約を通る**。

aarch64 の引数レジスタは 8 本。溢れた分は todo.md の
`enable_multi_ret_implicit_sret` (return-area ポインタ) を通る —
これは「署名が拒否される」のを直した変更で、**メモリ往復自体は
残っている**。

`write_seg` の命令内訳 (3,452 命令):

| | |
|---|---:|
| load / store | 2,413 (**70%**) |
| `bl` (呼び出し) | 249 |
| `add` (実際の計算) | 90 |
| スタックフレーム | 3,024 B |

toylang 生成コード全体では load/store が **50%** (Rust runtime 側は
24%)。

### 崖は leaf 8 個ちょうど

1 フィールドを `+1` するだけの `&mut self` メソッドの命令数:

| leaf 数 | 1 | 8 | 9 | 12 | 16 | 24 | 32 | 52 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| 命令 | 3 | **3** | 8 | 14 | 24 | 48 | 80 | **162** |

8 以下は完璧。9 から線形に増え、1 leaf あたり ~3.2 命令。

**狭い関数の codegen は良い**ことを押さえておく (同じコンパイラ、
同じ `--release`):

```
fn sum3(a,b,c) -> u64 { a+b+c }     -> add, add, ret        (3)
fn addp(p: &P, q: &P) -> u64        -> add, add, add, ret   (4)
fn bump(p: &mut P)                  -> add, ret             (2)
```

つまり直すべきは署名であって、命令選択でも cranelift の設定でもない。

### 全体への寄与

`poc/logsearch` の 326 関数を params+returns の幅で分けると:

| | 関数数 | bytes | 割合 | 1 関数平均 |
|---|---:|---:|---:|---:|
| 幅 > 16 | 34 (10%) | 76,380 | **49%** | 2,246 B |
| 幅 ≤ 16 | 292 (90%) | 78,504 | 51% | 269 B |

`ArchiveWriter` の 16 メソッドだけで 44,880 B = toylang コードの 29%。

### cranelift の opt level は無関係

`TOYLANG_CRANELIFT_OPT_LEVEL` を振っても `write_seg` は変わらない:

| | `none` | `speed` |
|---|---:|---:|
| `ldr` | 1,507 | 1,500 |
| `str` | 578 | 527 |

cranelift は展開済みの署名を受け取っているだけなので、この段では
直せない。**`.cargo/config.toml` の `[env]` が `none` を輸出している**
ので `cargo run -p compiler` は最適化なしで走るが、サイズには効かない
(速度には効く)。

## 副因 — 事前レンダリングされた診断文字列 (~39 KB, 12%)

DEBUG-OBS D3 は panic サイトごとに文面を `.rodata` に置く
(`declare_frame_strings`):

| シンボル | 個数 | bytes | 1 個あたり |
|---|---:|---:|---:|
| `toy_panic_msg_*` | 102 | 16,952 | 166 |
| `toy_frame_pre_*` | 89 | 14,817 | 166 |
| `toy_print_str_*` | 180 | 7,260 | 40 |
| `toy_frame_suf_*` | 89 | 534 | 6 |

機能としては正しい (行番号とソース断片が出るのはこの設計の眼目)。

**2026-09-25 に詰めた (CODE-SIZE-DIAG-STRINGS)。** その時点で panic 関係は
551 個・64,265 バイト (バイナリの 11%) に育っていた。内訳を割ると
共有できる部分が大きい — 見出し `Runtime error occurred:\nError at ` が
13.1 KB、同じファイル名の繰り返しが 5.1 KB、同じメッセージの繰り返しが
5.9 KB (`panic: requires violation` が 200 サイト)。そこで全部を
**1 つの `.rodata` オブジェクト (`toy_diag_pool`)** に入れ、サイトは
「共有文字列を**自分からの相対 offset** で指す記録」にした
(`compiler/src/codegen/diag_pool.rs`):

```text
0x02 | rel_file: i32 | rel_msg: i32 | flags: u8 | サイト固有の中間部 | 0
```

runtime (`write_diag_fd`) が見出し + ファイル名 + 中間部 + メッセージ +
閉じ罫線と書けば、以前の blob と同じバイト列になる。offset は同じ
オブジェクトの中なので relocation も引数も増えない。形が合わない文面は
平文のまま入る。

| | 変更前 | 変更後 |
|---|---:|---:|
| ファイル | 570,096 B | **548,504 B** (−3.8%) |
| `__const` | 81,264 | 70,016 |
| `__text` | 341,708 | 346,152 (サイトごとの `iadd` 1 つ) |

`__text` の増分はサイトの番地に offset を足す命令。offset を
relocation の addend に載せる形も試したが、`__text` は 347,440 と
かえって大きくなった。シンボル 551 個が 1 個になったぶんファイルは
さらに縮んでいる。

## 直し方

### 済: 書かない leaf は返さない (CODE-SIZE-WB-PRUNE, 2026-09-05)

`&mut self` メソッドが返す leaf のうち、**body が一度も書かないもの**は
呼び出し側が既に持っている値なので、返しても情報が増えない。
[`compiler_lower/src/writeback_prune.rs`](../compiler_lower/src/writeback_prune.rs)
が lowering 後に落とす。

**不動点まで回す**のが要点。`&mut self` メソッドが別の `&mut self`
メソッドを呼ぶと、その call の `self_dests` が全 leaf を覆うので
「全部書く」ように見える。callee を細くすると caller の dests も
細くなり、caller が今度は細くなる — この連鎖が効く:

| | 戻り値 (before → after) |
|---|---|
| `ArchiveWriter::write_seg` | 53 → **1** |
| `flush_segment` | 53 → 1 |
| `ArchiveWriter::write_frames` | 57 → 5 |
| `ArchiveWriter::finish` | 55 → 3 |
| `ArchiveWriter::rehash` / `rehash_links` | 52 → 4 |
| `ArchiveWriter::link` | 52 → 12 |
| `ArchiveWriter::emit` | 53 → 25 |
| `Vec::clear` / `Vec::set_size` | 4 → 1 |

`poc/logsearch` で **`__text` 191,080 → 171,424 B (−10.3%)**、
ファイル 331,832 → 315,336 B (−5.0%、差は段の整列と symbol 表)。
`write_seg` は 3,453 → 2,369 命令 (−31%)。archive の出力は
**変更前のコンパイラと byte 単位で一致**、`verify` は 12 segments ok。

**安全側の作り**。落とすと lost update になるので、判定は
`InstKind::writes_locals` の**網羅 match** (新しい命令が local を書くなら
分類しないとビルドが通らない)。加えて次は全 slot を残す:

- `address_taken_locals` にある leaf (追えない store が来うる)
- 関数の番地が漏れている場合 (`FuncAddr` / `MakeClosure` / vtable) —
  直接呼び出し以外は署名を書き換えられない
- **`dyn` thunk が呼ぶ callee** — thunk は writeback を受けるためだけに
  新しい local を確保して `data_ptr` に書き戻すので、slot を落とすと
  その local が未定義のまま残る。「落とす dest が呼び出し側の他の場所で
  定義されているか」を確かめ、駄目なら callee ごと諦める

`CallStruct` / `CallTuple` / `CallEnum` は**戻り leaf と writeback leaf を
1 本の `dests` に連結する**ので、writeback 部分は位置ではなく長さで
切り出している (最初これを見落として arity 不一致で落ちた)。

テストは
[`compiler/tests/consistency/writeback_prune.rs`](../compiler/tests/consistency/writeback_prune.rs)
の 6 本。**書き換えた field 以外も全部読み戻す**形なので、落としすぎは
クラッシュではなく古い値として出る。pass をわざと壊す (全 slot を落とす)
と 6 本中 5 本が落ち、`dyn` の 1 本だけは veto が効いて通る。

### 済: compound は番地 1 本で渡し、幅の広いローカルは常駐させる (S1〜S3b, 2026-09-05)

**leaf 数が閾値を超える compound `self` はポインタで渡す。**
展開はレジスタに乗る間だけの最適化で、既定にすべきものではない
(C ABI が構造体に対してやっているのと同じ判断)。閾値は
「引数レジスタ本数」= aarch64 / x86-64 とも 8 前後。

#### なぜ「使わない引数を落とす」では届かないか

戻り側と対称に「**body が読まない leaf param を落とす**」pass が
書けそうに見える。実際 26% の param leaf (2,297 中 598) はどこからも
参照されていない。だが**効くのは accessor だけ**だった:

| | params | 読まれる |
|---|---:|---:|
| `ArchiveWriter::ts_min` | 52 | **1** |
| `ArchiveWriter::count` / `is_empty` | 52 | 1 |
| `ArchiveWriter::write_seg` | 58 | **58** |
| `ArchiveWriter::emit_terms` | 63 | **63** |

大きい方が全 param を読むのは、**`self` を下へ渡し続けるから**
(`write_seg` は `self.write_frames(...)` と `self.emit_terms(...)` を
呼ぶので 52 leaf 全部が生きる)。呼び出し回数で重みづけすると
**節約は ~1,258 命令 / 36,157 (3.5%)** で、IR に leaf マスクを足して
codegen・IR VM を両方直す代償に見合わない。**この案は採らない。**

#### 効くのは「鎖を通してポインタを 1 本流す」形

同じ形を `Ptr<S>` (既存機能) で手書きして測った
(`scratchpad/sz/byval.t` 対 `byptr.t`、52 field・3 段の呼び出し鎖、
両者とも同じ答え):

| | leaf | mid | top | 合計 |
|---|---:|---:|---:|---:|
| by-value `&mut self` | 8 | 295 | 295 | 598 |
| `Ptr<S>` 経由 | 198 | 124 | 124 | 446 |

**転送層 (`mid` + `top`) が 590 → 248 命令 (−58%)。** `leaf` が
8 → 198 と増えているのは `p.get` / `p.set` が**構造体を丸ごと**
読み書きする素朴な形だから — 実装では触った field だけを
`PtrRead` / `PtrWrite` するので、ここは増えない。つまり
**この計測は下限**であって、それでも合計 −25% になっている。

#### 段取り (単独で出荷できる最小単位は S1+S2)

| | 内容 | 単独の効果 |
|---|---|---|
| **S1** 転送形式 | 幅の広い compound param をポインタで渡し、callee の prologue が `PtrRead` で既存の leaf local に読む。呼び出し側は slot を作って番地を渡す | **ゼロか悪化** (1 回の呼び出しなら往復量は同じ) |
| **S2** 転送 | 呼び出しの receiver が**それ自身ポインタ由来の param** なら、slot を作り直さずポインタをそのまま渡す | ここで初めて効く (上表の −58%) |
| **S3** 根の常駐化 | 幅の広い struct の**ローカル束縛**を stack slot に置く。鎖の根 (`var w = ArchiveWriter::new()`) だけでよい | 根の 1 回分 |

S2 には「leaf local とメモリが同期している」不変が要る (書き込みが
ポインタ側にも通っていること)。**S1 だけでは退化するので、
S1+S2 を 1 つの変更として入れる。**

#### 入った形

**閾値は leaf 8 個** (aarch64 / x86-64 の引数レジスタ本数)。これを超える
**by-reference の receiver** (`&self` / `&mut self`) が対象で、
by-value (`self: Self`) は**対象外** — 呼び出し側の記憶域を共有すると
callee の書き込みが外へ漏れるため。

- **callee**: 署名は `params[0]` の leaf 群の代わりに 1 本のポインタ。
  body は変えない — 同じ leaf local を読み書きし続け、**codegen が
  その `LoadLocal` / `StoreLocal` をポインタ経由の load / store に
  読み替える** (`ptr_self_leaves`)。呼び出し側の記憶域が唯一の実体に
  なるので、**writeback の戻り値は要らない**
- **caller**: receiver が**自分自身の pointer receiver** なら
  ポインタをそのまま渡す (S2 の転送)。そうでなければ per-call slot に
  leaf を書き出して番地を渡し、**呼び出し後に読み戻す** (S3 の根)
- **`dyn` thunk**: `data_ptr` は既に同じ `dyn_struct_leaf_layout` で
  並べてあるので**そのまま渡す** — leaf の読み出しも書き戻しも消える
- **auto-drop glue**: `drop` も `&mut self` なので slot を作って渡す。
  値は捨てられるので読み戻さない

**決定は宣言時**に行う (呼び出し側が callee の body より先に lower
されうるため)。leaf local は body を lower する時点で埋める。

**適用しないもの**: 演算子オーバーロード (`add` / `eq` / `lt` ...)。
被演算子は `lower_arg_values` が 1 つずつ潰すので callee を知らない。
名前で除外し、外れたものは下の検査が**ビルドを止める**。

#### 沈黙しないための 2 重の網

この変更で失われうるのは「書いたはずの値」なので、取りこぼしは
必ず音を立てるようにした:

1. **`ptr_self_verify`** — lowering 後に全 call の引数個数を callee の
   署名と突き合わせる。書き換え忘れた経路は
   `internal error (CODE-SIZE-SELF-ABI): call from X to Y passes N
   argument(s), but the callee's signature takes M` で**ビルドが止まる**
2. **`ReceiverReload`** — slot を作った呼び出しは読み戻しの義務を負う。
   `#[must_use]` が「作って捨てた」を捕まえ、`Drop` の panic が
   「`CompoundMethodCall` の field ごと落とした」を捕まえる
   (実際にこれで base64 と struct-literal の 2 経路の取りこぼしが出た)

閾値を **0 にして全 receiver を通す**ストレス実行で経路を洗い出した。
残る失敗は 2 件だがどちらも**上の網に掛かる**(ビルドエラーと
診断テキストの不一致) — 沈黙する誤りは無い。

#### S3a — `&T` / `&mut T` の compound 引数も同じ形に

receiver だけでなく、**参照で受ける compound 引数**が同じ扱いになった。
`poc/logsearch` で一番効いたのはここ:

```rust
fn flush_segment(w: &mut ArchiveWriter, out: str, segid: u64, crc: &Crc32) -> u64
```

`w` が番地で来ると、その中の `w.ts_min()` / `w.write_seg(...)` は
**番地をそのまま転送**できる (受け取った pointer param はどれでも
転送元になる — receiver に限らない)。`flush_segment` は
**5,536 → 455 命令**。

**by-value の compound 引数は対象外**。値渡しは自分のコピーを持つ
規約なので、記憶域を共有すると callee の書き込みが外に漏れる。

#### S3b — 幅の広いローカル束縛を stack slot に常駐させる

鎖の**根**が最後まで残っていた。`var w = ArchiveWriter::new()` を持つ
関数は受け側が leaf local なので、番地を要求する callee を呼ぶたびに
slot へ 52 個書き出して読み戻していた。

leaf 8 個を超える**ローカルの compound 束縛**は、生涯を通じて
stack slot に住む。実装は S1 と同じ仕掛けの使い回しで、codegen が
その leaf の `LoadLocal` / `StoreLocal` を slot への load/store に
読み替えるだけ — **lowering は何も変わらない** (`Binding::Struct` の
74 か所は leaf local を扱い続ける)。番地が要るときは
`DynCoerceSlotAddr` を出すので、materialise も読み戻しも消える。

**パラメータは対象外**。記憶域は呼び出し側のもので、entry block が
leaf を定義するか、そもそも番地で来ている。

**1 つの leaf に家は 1 つ** — これが唯一の落とし穴だった。
`&mut wide.field` は `AddressOf` を出し、それが専用 slot を指す一方で
読み書きは常駐 slot に行っていた。`AddressOf` が**囲っている記憶域**
(常駐 slot + offset、あるいはポインタ + offset) を返すよう直して
1 つに揃えた。閾値 0 のストレスで出ていた 3 件
(`method_reborrows_its_own_mut_parameter` /
`ref_stage2_field_mut_borrow` / `ref_stage2_nested_chain_mut_borrow`)
はすべてこれが原因だった。

#### 最後に残っていた例外 — 演算子オーバーロードと method の参照引数

`add` / `eq` / `lt` などは被演算子を `lower_arg_values` が 1 つずつ
潰すので**呼び先を知らず**、当初は名前で除外していた。だが呼び先は
被演算子を作る前に解決済みなので、**どの引数枠を埋めるかを渡す**だけで
済んだ (`lower_arg_values_for`)。除外リストは消えた。

同時に、**method の receiver 以外の参照引数**も対象になった
(`fn add(&self, o: &Self)` の `o`、`fn mix(&mut self, other: &Wide)` の
`other`)。それまで対象は自由関数の参照引数だけだったので、
`toy_V__add` は 13 引数のままだった → **2 引数**。

ここで 2 つ、対になっている取りこぼしを踏んだ:

1. **書き戻し先の数え方**。`&mut` 引数がポインタで渡ると writeback
   slot を持たないのに、呼び出し側は数え続けていた。数が合わないと
   呼び出しは**黙って素の `Call` に落ち**、道連れに *receiver* の
   writeback まで消える。`collect_compound_writeback_dests_for` が
   呼び先を見て飛ばすようにした
2. **宣言時と body 時で writeback の形が食い違う**。body 側の導出が
   ポインタ引数を飛ばしていなかった。こちらは
   `call_with_self_writeback returned 0 value(s), expected 1` と
   声を上げたので見つけやすかった

#### 効果 (`poc/logsearch`)

| | 変更前 | WB-PRUNE | S1+S2 | S3a | S3b | **参照引数** |
|---|---:|---:|---:|---:|---:|---:|
| `__text` | 191,080 | 171,424 | 167,736 | 162,912 | 156,252 | **153,056** (−19.9%) |
| ファイル | 331,832 | 315,336 | 298,824 | 298,824 | 298,824 | **298,824** (−9.9%) |
| archive | 1,687〜1,698 ms | — | — | — | — | **1,610〜1,662 ms** |

関数単位:

| | 変更前 | 現在 |
|---|---:|---:|
| `ArchiveWriter::emit_terms` | 3,092 命令 | **263** |
| `cmd_archive` | 2,706 | **1,211** |
| `flush_segment` | 1,384 | **455** |
| `ArchiveWriter::link` | 779 | 188 |
| `ArchiveWriter::write_seg` | 3,453 | 2,152 |
| `ArchiveWriter::ts_min` | 20 B | 3 命令 |

archive の出力セグメントは**変更前のコンパイラと byte 単位で一致**
(444,549 records / 12 segments ok)。

#### 閾値 0 のストレスに残るもの

全 struct をポインタ経路に通すと 66 件落ちるが、**43 件は上の 2 重の網**
(verifier / reload guard)。値が食い違うのは 4 件だけで:

- `a_return_type_instantiates_a_constructor` — 1 field の generic
  struct。**幅の広い generic struct**で閾値 8 の下で再現を試み、
  3 バックエンド一致を確認済み (テストに固定)
- `soa` の 3 件 — **AOT の stack frame の形**を検査するテスト。
  常駐させれば frame は変わるので、閾値 0 でだけ落ちるのは筋が通る

閾値 8 では全 2,933 テストが通る。

#### 閾値を下げる実験 (2026-09-25)

閾値を 8 より下げると小さな struct も番地で渡る。まず**抜けていた
呼び出しの形**が出た (閾値 8 でも leaf 9 個以上なら踏む形で、どれも
verifier がビルドを止めていた): struct / enum を返すメソッドへの参照
引数、一時値の参照引数、`for` の `match it.next()`。3 つとも塞いだ
(`prepare_compound_method_call` の引数枠検査、`temporary_address`、
`match` の対象を `prepare_compound_method_call` に寄せる)。

| 閾値 | テスト | `poc/logsearch` `__text` | `archive` |
|---:|---|---:|---:|
| 8 (既定) | 全通過 | 342,216 B | 4.76〜5.24 s |
| 4 | 全通過 | 339,744 B (−0.7%) | 4.69〜5.07 s |
| 2 | 8 件失敗 | — | — |

閾値 2 の失敗のうち 2 件は AOT の frame の形を見る `soa` のテストで、
落ちるのが正しい。残りは**値は合ったまま確保の集計が lane 間で割れる**
(`SoaVec<Box<i64>>` の drop、`Vec<String>` の sort の peak) か、IR VM が
レーンから外れるもの。tree-walker は閾値に依らないので lowering 側の
話で、閾値 8 でも leaf 9 個以上の所有型で起きうる。**閾値は 8 のまま**
にして、todo の PTR-ABI-LOW-THRESHOLD に記録した。

#### 残っている費用の見積もり

`Binding::Struct` の consumer は **17 ファイル 74 か所**。S2 の同期
不変はそのすべてに関わるので、これは 1 セッションで安全に入る
規模ではない。間違えると**沈黙する誤コンパイル**になる
(CODE-SIZE-WB-PRUNE と同じ危険だが、範囲がずっと広い)。

材料自体は在る — `address_taken_locals` の explicit stack slot 経路
(REF-Stage-2) と、`dyn` thunk が既に「`data_ptr` から leaf を
`PtrRead` して impl に渡し、writeback を `PtrWrite` で戻す」形を
持っている。S1 の callee 側はこの thunk とほぼ同じコードになる。

3 バックエンドに跨る変更なので、
[`CLAUDE.md`](../CLAUDE.md) の「横断的な変更をするとき」に従い
`compiler/tests/consistency/` にテストを足すこと。

#### 残っている費用の内訳 (WB-PRUNE 後の実測)

| | 命令 | 全体比 |
|---|---:|---:|
| toylang 生成コード | 36,157 | — |
| うち load/store | 15,938 | 44% (以前は 50%) |
| レジスタ 8 本を超える引数 (呼び出し側の store + callee の load) | ~4,150 | **11.5%** |
| `write_seg` の prologue (58 param の展開・spill) | 112 | — |

`write_seg` は 3,452 → **2,310 命令**になり、呼び出し 249 回に対して
**呼び出し間隔の中央値が 6 命令**まで下がった (25 命令を超える隙間は
5 か所だけで、それが幅の広い self 呼び出し)。**ここはもう
「呼び出しの多い関数」であって、ABI が異常な関数ではない。**

### 書く側 (今できる回避)

**触るフィールドだけを渡す。** 実測:

```rust
struct Buf { a: u64, b: u64, c: u64 }
struct Big { buf: Buf, p0: u64, ... pb: u64 }   # 15 leaf

g.bump_self()   # &mut self  (15 leaf) -> 20 命令
g.buf.bump()    # &mut Buf   ( 3 leaf) ->  3 命令
```

大きい struct のメソッドが実際に触るのが 1〜2 フィールドなら、
その部分 struct の method にすると細い関数側へ移る。複数の
フィールドを同時に触るもの (`ArchiveWriter::write_seg` /
`emit_terms`) は素直には割れないので、回避策であって解ではない。

## 再現手順

```bash
# 1. サイズと内訳
size -m poc/logsearch/build/release/logsearch

# 2. 署名の幅 (CLIF を出して params/returns を数える)
./target/release/compiler --core-modules core \
    --core-modules poc/logsearch/src poc/logsearch/main.t \
    --release --emit clif -o /tmp/ls.clif
grep -A1 '^; --- toy_ArchiveWriter__write_seg' /tmp/ls.clif

# 3. 関数ごとのコードサイズ (シンボル間の差分)
nm -n poc/logsearch/build/release/logsearch | awk '$2=="t"||$2=="T"'

# 4. 命令内訳
objdump -d --disassemble-symbols=_toy_ArchiveWriter__write_seg \
    poc/logsearch/build/release/logsearch
```

## 関連

- [`BACKEND.md`](BACKEND.md) — `CallWithSelfWriteback` の現在の形
- [`DEBUG_OBSERVABILITY.md`](DEBUG_OBSERVABILITY.md) — D3 の診断文字列
- [`todo.md`](todo.md) の CODE-SIZE-SELF-ABI
- [`poc/logsearch/design-docs/RUNTIME_GAPS.md`](../poc/logsearch/design-docs/RUNTIME_GAPS.md)
  — この POC が踏んだ処理系側の穴の一覧
