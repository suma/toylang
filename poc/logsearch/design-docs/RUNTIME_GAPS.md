# RUNTIME_GAPS — 足りないもの、踏んだもの

`poc/logsearch` を書きながら実際に叩いて確かめた、**機能単位**の空白と、
処理系側で踏んだ不具合。各項目は 3 つを持つ:

- **無いと何が困るか** (この設計のどこに効くか)
- **回避策** — 今日の toylang で回るか。回るなら設計はそれを採っている
- **入ったら何が変わるか**

優先度は **★★★ = 無いと設計を曲げている / ★★ = 回避策が高くつく /
★ = あると素直になる**。言語側の作業台帳は `design-docs/todo.md` が正本で、
ここに二重の台帳を作らない。

> **この文書は現状だけを書く。** 解消した項目は §Z に 1 行で残し、
> 経緯は git log にある (この POC 自身が todo.md の戒めを踏まないため)。

> **最終確認: 2026-09-05** (stdlib が `File` / `String` の `Drop` /
> 借用レシーバまで進み、`toy build` / `toy test` が入った時点)。
> **残っていると書いた項目はこの日に最小プログラムで叩き直している** —
> 「前はそうだった」を残さないため。消えた項目は §Z へ移した。
>
> **§G19 だけは 2026-09-23 に足した** — それまでの節が
> ライブラリとバックエンドの話しかしていなかったので、
> **構文と型システムの表現力**を同じやり方で 1 節にまとめた。

---

## G2. ファイルの部分入出力 — 解消し、設計を書き直した

**2026-09-05**: `core/std/fs.t` に **`File`** が入った
(`open` / `create` / `append` / `open_rw` / `read` / `write` /
`read_at` / `write_at` / `seek_to` / `seek_by` / `seek_end` /
`tell` / `size` / `sync` / `truncate` / `close` / `as_fd`)。
buffer は `Span<u8>` で確保もコピーも無い。

**この POC で最も効いた 1 項目**なので、回避策を消すところまでやった
([`STORAGE_FORMAT.md`](STORAGE_FORMAT.md) v3、同日):

| 何が回避策だったか | 今 |
|---|---|
| 索引を `.idx` に分けていた (索引だけ読めないから) | **1 ファイル `.seg`**。セクション表が中の位置を持つ |
| セグメント 8 MiB 上限 (丸ごと読むから) | フレーム単位で読む。上限ではなく選択 |
| 公開が rename 2 回 (順序に規約が要る) | **rename 1 回**。中途半端な公開状態が消えた |
| 耐久性は OS 任せ | `File::sync()` を公開の前に呼ぶ |
| 枝刈りにファイル全体を読む | **320 バイト**。時刻で全部枝刈りされるクエリが 33 MB → **1,280 バイト / 0 ms** |
| セグメントごとにアリーナを確保 | 走査全体で 1 組。peak live 22.7 MB (実測) |

**残っている全体読みは入力側だけ** — ログファイルは
`io::read_file_into` で 1 回に読む (16 MiB 上限)。行がチャンク境界を
跨ぐ扱いが要るだけなので、`File::read` で流せる。

## G4. システム情報 ★

**無いもの**: ディスクの空き容量 (`statfs`)、ホスト名、PID、CPU 数、
プロセスのメモリ使用量。

**困ること**: マウントの容量管理が**宣言値** (`quota=100G`) に頼る。
他のプロセスが同じディスクを食っていても気づけないので、
「書けるはずなのに `No space left`」は書き込み失敗としてしか見えない。
ホスト名が無いのでマウントヘッダの作成者欄は環境変数 `HOSTNAME` 頼み。

**回避策**: quota 宣言 + 書き込み失敗での degraded 化。実運用では
「宣言値を実容量の 80% にする」という運用回避になる。

**入ったら**: 配置ポリシーが実測ベースになり、`full` の判定が正確になる。

---

## G5. プロセス制御 ★★

**無いもの**: シグナルハンドラ、`fork`/`exec`、デーモン化、`atexit`。

**困ること**: **`SIGTERM` できれいに止められない**。systemd や docker が
送る停止シグナルを受け取れないので、`stop` は SIGKILL 相当になり、
アクティブセグメント (最大 8 MiB / 60 秒ぶん) が消える。

**回避策**: `POST /v1/admin/shutdown` を用意し、systemd の
`ExecStop=curl -XPOST ...` で叩いてもらう。**回避はできるが、
運用文書に「必ずこう書け」と書く必要があり、書き漏らすと黙ってログが消える**
という点で、回避策としては弱い。フラッシュ間隔を短く (60 秒→10 秒) して
損失を減らすのは緩和にしかならない。

**入ったら**: `SIGTERM` で §5 の停止手順に入れる。**常駐サービスとしての
体裁が整う最後の 1 ピース**。
→ todo に対応項目**無し** (新規要望)。

---

## G7. 圧縮とチェックサム ★★

**無いもの**: 標準圧縮形式 (gzip / zlib / zstd)、**CRC-32**。
`core/std/crypto/` に SHA-256 / SHA-224 は入ったが、これは
**フレームごとに掛けるには重すぎる** (この設計がフレーム末尾に置くのは
32 バイトのダイジェストではなく 4 バイトの CRC)。`core/std/hash.t` に
あるのはハッシュ表向けの `Hash` trait と splitmix64 の混ぜ器で、
**チェックサム用途ではない** (衝突耐性も安定した値の保証も別物)。

**困ること**:

- 圧縮が無い → **自前コーデック `LSZ1`** を書く
  ([`STORAGE_FORMAT.md`](STORAGE_FORMAT.md) §5)。これは 200 行程度で
  書けるので費用としては許容だが、**標準ツールで開けないアーカイブ**に
  なるのが本当の代償。`zcat` できないログアーカイブは運用で嫌われる
- CRC-32 が無い → 自前 (表は起動時に `Vec<u32>` へ作る。`const fn` は
  スカラーしか畳めないので表は定数にできない)

**回避策**: 自前で全部書く。ラウンドトリップ性質は `--check` の
プロパティテストで縛れるので、正しさの担保は取れる。

**入ったら**: gzip があればアーカイブを `.gz` にして外部ツールと繋がる。
`LSZ1` は残しておいてよい (ホットなセグメントには速い方が向く)。
CRC-32 が stdlib に来れば `src/crc.t` (74 行) がまるごと消える。
→ todo に対応項目**無し** (新規要望。`RUNTIME_LIBRARY.md` の P4 圏)。

---

## G8. 文字列とバイト列の走査 ★ (大半が解消済み)

> **2026-09-05 追記**: 下記の「無いもの」のうち `find` / `starts_with` /
> `ends_with` / `str` の `Ord` / memchr 相当は**すべて入っている**
> (`str.t` の `StrSearch`、`string.t` の `find`/`rfind`/`replace`/
> `lines`/`split_whitespace`/`join`/`chars`、`span.t` の `find`/`find_seq`)。
> 残るのは `split_once` / `strip_prefix` / `strip_suffix` /
> `trim_start` / `trim_end` / 大小無視比較と、`Span<u8>` の
> `rfind` / `starts_with` / `ends_with`。整理は言語側の
> `design-docs/RUNTIME_LIBRARY.md`「関数粒度の空白」節 D にある。
> 以下は解消前の記述として残す。

**無いもの**: 部分文字列の**位置**を返す検索 (`find` / `index_of`)、
`starts_with` / `ends_with`、大文字小文字を無視した比較、
`str` の `Ord`、バイト列の高速探索 (memchr 相当)。

あるのは `Contains` (bool のみ)、`Split` (`Vec<String>` を**確保して**返す)、
`Substring` / `Trim` / `CaseConvert` (いずれも新しい値を確保)。

**困ること**: HTTP のパース (`\r\n\r\n` の位置)、ラベルの切り出し、
語の切り出し — **この設計の hot path はほぼ全部が「位置を返す探索」**
なのに、stdlib の文字列 API は**すべて新しい値を確保する形**をしている。
確保はこのランタイムでは永続なので、hot path では 1 つも使えない。

**回避策**: `Span<u8>` の上に自前の走査を書く (`src/bytes.t`)。
確保しない代わりに `never_allocates` が付けられるので、**規律としては
むしろこちらが望ましい**。ただし stdlib の文字列 API がほぼ全滅である
ことは、言語側にとっての発見だと思う。

**入ったら**: `find` が `Option<u64>` を返す形で入れば、確保せずに書ける
API が増える。`str` の `Ord` はカタログのソートで欲しい。
→ todo の **STDLIB-TEXT** / **STDLIB-ORD**。

---

## G9. ネットワーク ★

**あるもの**: TCP (listener / stream)、UDP、`resolve`、`Poller` (epoll/kqueue)、
`peer_addr`、`set_nodelay`、タイムアウト。**この設計に必要なものは揃っている。**

**無いもの**: TLS、Unix ドメインソケット、`SO_REUSEPORT`、
`sendfile`/`splice`、IPv6 の明示的な扱い。

**困ること**: TLS が無いので公開ネットワークに直接置けない
([`OVERVIEW.md`](OVERVIEW.md) の非目標)。Unix ソケットが無いので
ローカル専用の管理経路は「TCP の 127.0.0.1 のみ」で代用する。

**回避策**: 前段に reverse proxy。実運用として普通の構成なので、
この項目の優先度は低い。

---

## G10. 確保の失敗 ★ (解消済み)

> **2026-09-05 追記**: `try_with_capacity` / `try_reserve` が
> `Result<_, AllocError>` を返す形で `Vec` / `String` に入っている。
> `push` 経由の `grow_to` も realloc の戻り値を検査し、null なら
> **`panic("Vec::grow: allocation failed (N bytes)")`** で止まる
> (`core/std/collections/vec.t:174`)。「null に書きに行く」は
> もう起きない。残るのは「詰まったら 503 を返す」ための
> `push` の非 panic 版だけ。以下は解消前の記述として残す。

**無いもの**: 確保失敗の検査経路。`Vec::push` / `String::push` /
`Box` は `__builtin_heap_realloc` の戻り値を**誰も検査せず**、
null に書きに行く (todo の STDLIB-ERROR-MODEL がそう明記している)。
`try_reserve` / `try_with_capacity` も無い。

**困ること**: 常駐サーバでメモリが尽きたときの挙動が
**未定義の書き込み**になる。ログサービスは「詰まったら落ちる」より
「詰まったら遅くなる / 取り込みを止める」であってほしい。

**回避策**: **確保しない設計** ([`MEMORY.md`](MEMORY.md))。起動時に
全部取り、以後伸ばさないので、失敗しうる箇所は起動時に集約される。
起動時の確保が失敗したら、そこで気づいて `exit(1)` すればよい。
— これは回避策として十分に機能する。

**入ったら**: 上限を超えた要求を `503` で返せるようになり、
バッファの上限をもっと緩められる。
→ todo の **STDLIB-ERROR-MODEL** (E0〜E5)。

---

## G11. コレクションと `never_allocates` の相性 ★★

**問題**: `never_allocates` の検査は**到達可能性**なので、容量が足りていて
実際には伸びない `push` も拒否される。実測:

```
[E0016] `poke` is declared `never_allocates`, but it can reach the
        allocator: poke -> push -> __builtin_heap_realloc
```

同じ関数を `v.set(i, b)` で書けば通る。

**困ること**: 「確保しないと約束する層」で `Vec::push` が使えない。
容量を先に取ってあるのに、`push` ではなく `set` + `set_size` を
手で回すことになる (`String::push` も同様)。

**回避策**: `set` / `Span<u8>` 経由で書く。設計としてはむしろ健全だが、
**stdlib の自然な書き方から離れる**ぶん、書き手が間違えやすい。

**入ったら**: `push_within_capacity(v) -> bool` のような
「伸びないことが型で分かる」API が 1 つあれば、hot path が普通の
コレクション操作で書ける。
→ todo に対応項目**無し** (新規要望。NEVER-ALLOCATES と
STDLIB-COLLECTIONS の交差点)。

**関連する既知の穴** (どれも設計を曲げてはいないが、実装で踏む。
**2026-09-05 に全行を叩き直した**):

| 項目 | 内容 | 出典 |
|---|---|---|
| FN-NAME-AS-VALUE | トップレベル関数を `sort_by` の比較器として渡せない (`[E0001] expected fn (u64) -> u64, but got u64`)。クロージャに束縛するか `impl Ord` にする | todo |
| クロージャ捕捉 | AOT はスカラーしか捕捉できない。比較器にバッファを捕捉させられない | CLAUDE.md |
| メソッド戻り値へのフィールドアクセス | `v.get(0u64).first` が AOT で不可 (`compiler MVP only supports field-access chains rooted at a bare identifier`)。`val` に束縛してから | 本設計 |
| `Vec<(A, B)>` | タプル要素の Vec が AOT 不可 (`__builtin_sizeof: could not infer arg type at AOT`)。ペアは struct にする | todo |
| ~~確保カウンタがレーンで割れる~~ (解決済み) | tree-walker だけが `str::as_ptr` の受け皿を数え、解放もしていなかったので、`String::from_str` を呼ぶたびに live バイトが増えた。**2026-09-18 に修正** (todo の STR-PTR-UNCOUNTED)。`tests/steady.t` はこれで全レーンで測れる | todo |

---

## G12. 並行性 ★

**無いもの**: スレッド、非同期ファイル I/O、プロセス間通信。

**困ること**: ファイル I/O がイベントループを止める。8 MiB の書き出し中は
取り込みも検索も止まる。**設計はこれを前提に、1 回の I/O を小さく保つ**
方向に倒してある ([`ARCHITECTURE.md`](ARCHITECTURE.md) §2)。

**回避策**: ある (小さく刻む)。取り込み 5,000 行/秒の目標なら、
8 MiB の書き出しが数十 ms でも間に合う。

**入ったら**: 書き出しと索引構築を別スレッドに出せる。ただし
**move / Drop モデルとの接合**が本体なので、言語側では大きな仕事になる。
→ todo の **CONCURRENCY** (検討中の機能、★★★)。

---

## G18. 統合テストの土台 ★ (道具は入った)

> **2026-09-05 追記**: `toy test` が入り、**この POC が書けるテストの
> 器は揃った** — `tests/*.t` とモジュール内の `test` ブロックを
> **既定 AOT** で走らせ (`--backend vm` は失敗を全部まとめて報告する)、
> `core/std/testing.t` が位置と両辺を言うアサーションを、
> `test "..." panics "text"` が契約違反の検査を、`--bless` が
> ゴールデンファイルを持つ。**書いていないだけ**で、書けない状態は
> もう終わっている ([`ROADMAP.md`](ROADMAP.md) §5)。

**残っている穴はプロセスを起こせないこと**だけである。サーバを立てて
**外から**叩くテストは書けないので、**同一プロセス内にクライアントを置く**形
(`compiler/tests/consistency/net.rs` と同じ流儀) で書くか、AOT で組んだ
バイナリを Rust 側のテストから叩くことになる。

> **番号について**: この項目は以前 `G13` を名乗っていたが、§Z の
> 「G13 レベル付きログ」と衝突していた (どちらも 13 だった)。解消済みの
> 側は git log から辿れる履歴なので、**現役のこちらを G18 に振り直した**。

## G14. 実装中に見つけた言語側のバグ ★★★

### ⚠ 行末の識別子と次の行頭の `(` が繋がる ★★

LSZ1 のハッシュをベクトル化していて踏んだ。**最初は SIMD の
問題だと思ったが、まったく関係が無かった** — スカラーでも同じことが起きる。

```rust
fn f(mask: u64) -> u64 {
    val a: u64 = 1000000u64
    val b: u64 = 3u64
    val prod = a * b          # ← 行末が識別子
    (prod >> 17u64) & mask    # ← 行頭が `(`
}
```

`[E0003] Function 'b' not found` (指すのは **1 行上の `a * b`**)。
セミコロンが無い言語なので、`b` と次行の `(` が繋がって
**`b(prod >> 17u64)` という呼び出し**に読まれている。JavaScript の
ASI が同じ罠を持つ。

- 行末が**リテラル**なら起きない (`val b = a + 0u64` の次に `(...)` は通る)
- 行頭を `val` にすれば起きない (`val r = (prod >> 17u64) & mask`)
- `[` で始めた場合は繋がらず `parse error: BracketClose` になる

**診断は原因の行を指していない**うえ、「関数が無い」という、書き手の
意図から最も遠い語彙で報告される。規則自体は妥当でも、これを知らずに
最小化すると (実際そうなった) 10 回近く試すことになる。

### ⚠ モジュールの中の `[E0014]` が入口ファイルを指す ★★ (2026-09-05)

`String` が所有型になった日に踏んだ。**エラーはモジュールの中にあるのに、
報告は入口ファイルに対して行われる** — 行番号だけがモジュールのもので、
ファイル名は入口のもの。入口が短いと `<line not available>` になる。

```
Error at scratch/probe/g.t:10:1:     # 入口は 9 行しかない
10 | <line not available>
   | ^ [E0014] `s` cannot be moved inside a branch or a loop body
```

最小再現: モジュール側に「分岐の中で `String` を `push` する関数」を 1 つ
置き、入口から呼ぶだけ。同じ状況で **`[E0010]` は正しく**
「`Error in imported module logsearch::query (line 680 of that module)`」
と出るので、**モジュール帰属を持っている診断と持っていない診断がある**
ということになる。実際にこの POC では、`query.t` と `logdir.t` の 5 か所の
`[E0014]` が全部 `main.t` の無関係な行を指していて、原因の特定が
grep 頼みになった。

## G15. SIMD に残っている穴 ★

**あるもの**: 5 つの 128bit 型、lane-wise の演算子、17 の intrinsic
(`bitmask` / `swizzle` / `bitcast` / `shuffle` を含む)。非整列ロードが動き、
`never_allocates unsafe fn` の中で使え、4 レーンで答えが一致する。
stdlib のバイト kernel も SIMD 化済み。使い方と実測は [`SIMD.md`](SIMD.md)。

**残っている穴**:

| 無いもの | この設計への影響 |
|---|---|
| **`u16x8` / `u32x4` の lane 型** | バイトを加算で広げられない (`bitcast` は再解釈であって拡張ではない)。**チェックサム系がベクトル化できず**、`u8` lane の累積は 255 で wrap するので 200 ブロックごとに畳む規律が要る |
| **carry-less multiply** | CRC-32 がスカラー固定。アーカイブ作成で無視できない割合になりうる |
| gather / scatter | 転置リストの間接参照 |
| **代入位置での lane 型推論** | `acc = __simd_splat(0u8)` が `[E0010]`。`val zero: u8x16 = ...` を先に束縛する。ループ内で毎回書けない形なので実際に踏む |
| 256bit / feature detection | Phase 4 未着手。**AOT が baseline ISA 固定**なので、128bit に留まる限りバイナリは可搬 |

**優先度が高いのは `u16x8` / `u32x4`** — 入るとチェックサムと時刻列フィルタの
両方が候補に戻る。`design-docs/SIMD.md` は「行を足すだけ」と書いている。

## G16. 実装して分かった AOT の形の制約 ★★

読み取りコンポーネント (`src/line.t` / `logdir.t` /
`reader.t` / `record.t`) を書いて踏んだもの。**どれも回避できるが、
回避策を知らないと診断からは原因に辿り着けない。**
**2026-09-05 に 1 行ずつ叩き直し、まだ再現するものだけを載せている。**

| 踏んだもの | 診断 | 回避策 |
|---|---|---|
| **compound な *フィールド* を引数に渡せない** | `call argument produced no value` / `method argument produced no value` | 窓を渡す (`self.buf.as_span()` を `val` に束縛して `Span<u8>` で渡す)。束縛・リテラル・呼び出し結果は通るので、**フィールドパスだけが穴**。`len_of(&self.data)` の形も同じく通らない |
| **`match` の arm から compound を代入できない** | `assignment rhs produced no value` | arm の中で使い切る (`Vec<String>` を外の `var` に代入せず、arm の内側でループを回す) |
| **struct を struct のフィールドへ代入できない** (所有型に限らない — `Drop` の無い 2 フィールドの struct でも同じ、2026-09-23) | `compiler MVP cannot assign whole struct to nested field \`name\` (assign individual leaf scalars instead)` | 値をフィールドに**後から入れず、構築時に渡す**。`mount.t` の `read_meta` は、識別子が分かった場所で `MountMeta { .. }` を組んで `return` する形になった。最小再現: `var h = Holder { name: n }` に対する `h.name = s` (`String` フィールド) |
| **既存の `var` へ struct を代入できない** (所有型に限らない、2026-09-23) | `assignment rhs produced no value` | `var` に溜めずに `val` で受け切る。上の行と同じ理由で同じ回避になるので、**この 2 つは一緒に踏む**。最小再現: `var name = String::new()` に対する `name = s`。既出の「`match` の arm から compound を代入できない」と診断は同じだが、**arm でなくても起きる** (通常の呼び出し結果でも) |
| **associated function に wide な `&mut` を渡せない** | `call argument produced no value` | 自由関数かモジュール関数にする。`fn f(w: &mut Wide, v: &mut Vec<u64>)` は通るが、同じものを `impl Ops { fn f(...) }` の associated function として書くと通らない — **呼び出し位置ごとに引数の lowering が呼び先を知っているかどうかが違う**のが原因で、同じ根から 3 件目 (2026-09-11、前の 2 件は本体側で直した)。最小再現: 12 leaf の struct への `&mut` を `Ops::touch(&mut w, &mut v)` に渡す |
| ~~**`match` の腕で受けたハンドルは複製で、元が閉じる**~~ (解決済み) | **2026-09-19 に本体側で直った** (todo の MATCH-PAYLOAD-COPY)。腕は payload の複製ではなく**別名**を張るようになり、所有者は 1 人になった。踏んだのはサーバの接続表で、当時の回避は `TcpListener::accept_fd` (今も表を作るには番号のほうが素直なので残っている) |
| ~~**容器から取り出したハンドルを束縛すると閉じる**~~ (解決済み) | **2026-09-19 / 20 に本体側で直った** (ELEMENT-BORROW)。`val s: TcpStream = conns.get(0u64)` は要素の**別名**に drop glue が付いて fd を閉じていた。今は `[E0028]` が**コンパイル時に断り**、`conns.borrow(0u64)` で名指す。固定スロットの表は `Vec<Option<TcpStream>>` — 空きは `None`、閉じるときは `Vec::replace` で所有を取り戻す (`set` は上書きするだけで fd を閉じない)。サーバ本体も 2026-09-20 に `Vec<Option<TcpStream>>` へ移した (同時接続 128 はそのまま) |
| **これらの診断に位置情報が無い** | `compile error: call argument produced no value` の 1 行だけ。ファイルも行番号も出ない | 二分探索するしかない。型検査の診断 (`[E0001]` など) はスニペット付きなので、**落ちる層で情報量が変わる** |

**最初の 2 行と最後の行は 2026-09-05 に、間の 3 行は 2026-09-11 に再現を確認した。** 一方、以前ここにあった
「8 leaf を超える struct を返せない」「`&mut self` の書き戻しが同じ予算を
食う」「str を返す関数の多重 `return`」の 3 行は WIDE-RETURN で消えた
(12 フィールドの struct を返す関数と、5 leaf のレシーバが `Result` を
返す `&mut self` method で確認)。**回避のために詰めた形は残っている** —
外すのは、外して測る理由ができたときでよい。

### `enum call returned 19 value(s), expected 15` (internal error)

自由関数が `&mut` の compound を 2 つ (`&mut ArchiveWriter` +
`&mut Lsz`) と `&Crc32` を取り、`u64` を返す形で出た。最小再現は
取れていない (7 フィールド struct を `Result` で返す形は通る)。
圧縮器を writer のフィールドにして `&mut` を 1 つに減らしたら消えた。
**internal error なので、ユーザ側に直し方の手掛かりが無い。**
2026-09-05 現在、この形は WIDE-RETURN 以降 1 度も再現していない
(回避した形のまま動いているので、**消えたのか隠れているのかは
分からない**)。

**言語側の穴** (バックエンドではない):

| 踏んだもの | 内容 |
|---|---|
| **bare な関数名がモジュールを跨いで衝突する** ★★ | `fn is_digit` が `std::json::is_digit` と衝突して `[E0010] ambiguous module path`。**`pub` でない関数でも衝突する**ので、auto-load される全モジュールが 1 つの名前空間を共有していることになる。`max_depth` も同じ。**この POC の回避策は改名** (`is_digit` → `is_digit_byte`)。**2026-09-05 に半分だけ解消** — 後の module root が bare 名を勝つようになった (BUILD-TOOL B0) ので、**ユーザの `src/` は stdlib に勝つ**。残っていた 2 件 (`decode`: `lsz.t` と `std::base64` / `std::hex`、`parse`: `record.t` と `std::json`) も**同日に改名で潰した** (`decode_frame` / `parse_line`) ので `toy check` は無警告。ただし**回避しただけ**で、stdlib に名前が増えるたびに踏みうる。**class ごと消す設計**は本体側 [`MODULE_IMPORTS.md`](../../../design-docs/MODULE_IMPORTS.md) (明示 import。D3 が「bare 名は自分のファイル + prelude だけ」にする) |
| **~~`&mut self` が struct 全体を呼出規約に展開する~~** (解決済み) | **2026-09-05 に直った** (CODE-SIZE-SELF-ABI の S1〜S3b)。leaf が 8 個を超える `&self` / `&mut self`、`&T` / `&mut T` 引数、幅の広いローカル束縛は**番地 1 本で渡る**。かつては `ArchiveWriter` (52 leaf) の 16 メソッドで**バイナリの 29%** を占め、1 フィールド読むだけの `ts_min()` が 52 引数取っていた。2026-09-17 に測り直すと `ArchiveWriter` のメソッドは幅の広い署名の上位から消えており、残る幅広は**値で返す compound** (`ArchiveWriter::new` は戻り値 60) と**8 leaf 以下の参照引数が何本も並ぶ関数** (`query::search` は参照 7 本で 19 引数)。閾値を 8 → 4 に下げる実験では `__text` が 292,148 → 290,472 B (−0.6%)、archive の所要時間は変わらず — **署名全体の幅で見る形に広げても、得るものは小さい**。計測は本体側 [`CODE_SIZE.md`](../../../design-docs/CODE_SIZE.md) |
| **プログラム本体をモジュール根に置くと自分の `const` を失う** | `src/main.t` を根に入れると auto-load で二重取り込みになり、複製側が `Identifier 'BUF_BYTES' not found` で落ちる。エントリは根の外に置くのが正解 ([`ARCHITECTURE.md`](ARCHITECTURE.md) §4) |

## G17. 所有型を分岐の中で move できない ★

**2026-09-05 に `String` が `impl Drop` を得た** (STRING-NO-DROP)。
`Vec<T>` / `Box<T>` と同じ所有型になったので、**コンテナに `String` を
入れるのは move** であり、**その move が分岐やループ本体の中にあると
`[E0014]`** になる (drop flag が無いので、脱出時に持っているかどうかが
実行時にしか分からない)。この POC はこれを 3 か所で踏んだ —
ディレクトリ走査の `out.push(full)`、クエリ解析の
`q.needles.push(tok)` / `q.host.push(value)`。どれも「条件に合ったら
容器に入れる」という、書かない方が難しい形をしている。

**回避策**: `.clone()` して**コピーを入れる** (`impl Clone for String`
はある)。走査 1 エントリあたり 1 確保で、走査自体が数千回なので費用は
測れない。hot path なら別の話になるが、この設計では language 側の
`String` は cold path にしか居ない (レコードは `Span<u8>` で運ぶ)。

**入ったら**: drop flag が入れば `.clone()` が消える。**入らなくても
設計は曲がらない**ので優先度は低い。ただし「条件つきで容器に入れる」は
初心者が最初に書く形なので、**診断が回避策 (`.clone()`) を提案すべき**
だとは思う。

## G19. 文法そのものの空白 ★★ (2026-09-23)

ここまでの節は**ライブラリとバックエンド**の話だった。この節は
**構文と型システムの表現力**だけを扱う。

> **確かめ方**: 候補の構文を 50 本ほど最小プログラムにして
> `--all-backends` に通した。「無い」と書いたものは parse か型検査で
> 落ちる。「もうある」と書いたものは**モジュール越しでも 3 レーン一致**
> で動くことまで見ている (過去に `SliceAccess` の remap 漏れで
> 「入口では動くがモジュールの中では動かない」があったため)。

### 1. ~~文字列リテラルに逃げ道が無い~~ — 2026-09-23 に解消 (§Z)

### 2. `String` をリテラル腕で match できない ★ (narrow int と const は解消)

**narrow int の match は 2026-09-23 に解消** (CHAR-LITERAL-MATCH、§Z)。
const 名を pattern に書くと黙って束縛になる件も同日に解消した
(MATCH-CONST-PATTERN)。どちらも関数で作ったタグ (`kind_ftable()`) には
まだ効かない — 先に `const` か enum に移す必要がある (§3)。

**残っているもの**: `String` のリテラル腕 — `[E0010] literal pattern
cannot be used in a match on a struct`。`String` は nominal struct
なので、`str` では通るリテラル腕が `String` では通らない。この POC の
`field_code` / `is_reserved` ほか **`eq_str` 33 箇所**がこれ
(ただし半分は化石。§8 を見ること)。→ todo の **MATCH-STRING-LITERAL**。

### 3. enum の表現力 ★★ — 2 つとも保存形式のモデリングに効く

> **2026-09-23: (a) discriminant と `as` は入った** (ENUM-DISCRIMINANT)。
> `enum Fmt { Plain, Syslog, Datetime, Apache, Epoch }` に対して
> `f as u32` がディスクに書く番号になる。**逆向き (番号 → enum) の
> 変換は無い**ので、読む側は `match` を 1 本書く。この POC のタグ関数は
> まだ移していない。(b) struct variant は未実装。

**無いもの**: (a) 明示 discriminant と整数変換 (`Red = 1` / `as u64`)、
(b) **struct variant** (`enum E { A { x: u64 } }` は parse エラー)。

**困ること**: レコードの形 (syslog / datetime / apache / epoch / plain) は
**variant ごとに持つフィールドが違う**ので struct variant がちょうど
当てはまるが、書けない。タグはディスクに数として出るので整数変換も要る。
結果、**この POC には `enum` 宣言が 1 つも無い**。代わりに
「数を返すだけの `pub fn`」が **74 本**あり、これが discriminant の
手書きそのものになっている (`field_*` 10 / `kind_*` 10 / `fmt_*` 5 /
`method_*` 3 …):

```rust
pub fn fmt_plain() -> u32 { 0u32 }       # src/record.t:34-38
pub fn fmt_syslog() -> u32 { 1u32 }      # 定数が無いので関数で作る
```

逆変換 (`fmt_name`) も手書きで、`rec.kind == 3u32` のような**生の数との
比較が 13 箇所**。`fmt_name` に 6 番目を足し忘れてもコンパイルは通る。

**回避策**: タグ (`u32`) + フラットな struct + `pack_span` の詰め込み。
**型がレコードの形を知らない**ので、どの field がどの形で有効かは
コメントの約束になっている。

**入ったら**: (a) はタグがそのままディスクの数になり、`fmt_name` の
if 連鎖が消える。(b) は「この形のときだけ在るフィールド」を型が持つ。
→ todo の **ENUM-DISCRIMINANT** (設計済み・未着手)。**struct variant は
todo にも `docs/language.md` にも項目が無い** (新規要望)。

### 4. 配列を値として渡せない ★★

**無いもの**: 配列型のパラメータ。`fn f(a: [u64; 3])` は compiled レーンで
``compiler MVP cannot lower parameter `a: [u64; 3]` yet``。

**困ること**: `const T: [u64; N]` の宣言と添字は 3 レーンで動くのに、
**名前で関数に渡せない**ので定数表が関数を跨げない。CRC 表は起動時に
`Vec<u32>` を組む形になっている (`src/crc.t`)。G7 に「`const fn` は
スカラーしか畳めないので表は定数にできない」と書いたが、**効いている
制約はこちら**だった。

**回避策**: 実行時構築 + `Vec` / `Span<u8>` で渡す。費用は起動時の 1 回。

**入ったら**: 表が `const` のまま関数へ渡り、`never_allocates` の層に
入れられる。→ todo の **CONST-ARRAY の残り**。

### 5. 結果を値で持ち出せないので `var` のフラグになる ★★

**無いもの**: `break <value>` / `loop` の値 / 早期脱出の一般形。
所有値を分岐の中から `return` できない (G17 と同根) ことと合わさって、
**関数が単一出口になり、成功フラグを積む**。

**困ること**: `var ok = true` が **14 本**、`ok = false` が **90 箇所**、
`if !ok` が **14 箇所**。`src/segfile.t:196-250` が最悪形で **7 段**。
`src/archive.t:1877` は理由をコメントで書いている — 「Single exit
again: `return out` from inside the guard would be a conditional move
of an owned value.」 **この 1 行のために関数全体が 1 段深い。**
「見つかったか」を持ち出すだけのフラグ (`scanning` / `placed` /
`more` / `running`) も **30 箇所以上**ある。

**入ったら**: `val k = loop { if b == '=' { break Option::Some(k) } ... }`
で `ok` の梯子が消える。drop flag が入れば条件つき `return` も通る。
→ todo の **MOVE-CONDITIONAL**。`break value` は項目が無い (新規要望)。

### 6. 小さな穴 (どれも回避できるが、書き方が 1 段遠くなる)

| 無いもの | この POC での現れ方 |
|---|---|
| struct literal の field shorthand `P { x, y }` | **パターン側には shorthand がある**のに構築側に無い (非対称) |
| `val P { x, y } = mk()` (struct の分割束縛) | タプルは分割できる。struct はフィールドを 1 つずつ |
| `Default` / 構造的な `==` (derive 相当) | ゼロ初期化コンストラクタ 6 本が全フィールド手書き (`src/segfile.t:78 SegHead::empty()` は **23 フィールド**)。`tests/catalog.t:33` は `!=` を 10 本並べて等値を書いている |
| 名前つき引数 (todo は「導入予定も無い」) | 6 引数以上の関数が **17 本**、最大 12 (`src/server.t:1681`)。`readable, writable, gone` の bool 3 連は順番を入れ替えても型が通る |
| compiled レーンで使えるコレクションリテラル | `dict{...}` は interpreter 限定、`Vec` リテラルは無い → 表は `push` の列 |
| `for x in v` (`.iter()` が要る) / `for (i, x) in ...` / 配列の iterate (`for v in [a, b]`) / `..=` / `.rev()` / step | `enumerate` は `p.0` `p.1` でしか受けられない。`main.t:1178` は候補 2 個を回すためだけに `while pass < 2u64` |
| char リテラルの型が文脈から取れない位置がある | `tests/segment_format.t:181-184` は `assert_eq` の中で型が名指されないので `val m0: u8 = 'L'` を 4 行 (todo の CHAR-LITERAL-SIBLING) |
| user generic の turbofish (`id::<u64>()`) / グローバル可変 `var` / 入れ子 `fn` | 引数で持ち回る |
| テストから見える共有ヘルパの置き場 | `span_of` が **7 ファイルに同一本体**、`*_line` が prefix 違いで 7 本 (中身は 2 行)。bare 名がモジュールを跨いで衝突する (G16) ので prefix を振っている |

trait 継承 / associated types / `impl Trait` 戻り / `Box<dyn Trait>` /
関数名を値として渡す (**FN-NAME-AS-VALUE**) は、todo にあるとおり
**無いことを再確認した**。

**空振りだった要望**: バイト列リテラル `b"..."` は**実害が出ていない** —
マジック値は `rd.take_magic(hb, "LSF1")` と `str` で通り、1 バイトずつ
push している箇所は 0 件だった。

### 7. 数で見る「書けるのに書かれていない」

**この POC の穴の半分は「無い」ではなく「18,000 行が一度も使っていない」**
だった。下は全部**仕様にあり、モジュール越しの 3 レーンで動く**ことを
確かめたもの (`--all-backends`)。

| ある構文 | 使用 | 代わりに書かれている形 |
|---|---|---|
| `if val` / `while val` | **1** / 0 | `Option` を剥く `match` が **192 箇所**、うち **None 腕が空なのが 85**。「1 ファイルを行に割る」6 行の入れ子が **14 回**複製されている (`tests/steady.t:57-75` ほか) |
| `?` | 4 | `Result::Err(e) => {...}` が **195 箇所**、`Ok(x) => { }` (成功を捨てる) が **75** |
| match arm guard | 0 | 腕の中を `if` で始める (331 腕のうち 18) |
| `loop` / `@label:` 付き `break` | 0 / 0 | 上の §5 のフラグ |
| `str` のリテラル腕 + `\|` | 0 | `if` の表が **10 本** (`src/record.t:159` の `Jan`..`Dec` 12 連ほか) |
| タプルの分割束縛 | 0 | §8 の `pack_span` |
| `const` (配列 / struct / `str`) | **1** (`main.t:70`) | 74 本のタグ関数、実行時に組む表 |
| `soa Vec<T>` / `Column<T>` | 1 | 並列 `Vec` の手書き SoA (`ArchiveWriter` は **Vec 18 本**、`Conns` 8 本) |

**インデント 24 桁 (6 段) 以上の行が 1,073 / 16,948 = 6.3%** で、
その最大の原因が 1 行目の `Option` 手展開である。**言語側の宿題は
文法追加ではなく、なぜ辿り着かなかったか** (診断・例・API の形) の方。

### 8. 化石と思われた回避策 — 叩き直した結果 (2026-09-23)

§G19 を書いた日に「穴が埋まったのに残っている形」として 4 つ挙げたが、
**実際に書き換えようとすると 2 つは化石ではなかった**。

| 形 | 箇所 | 結果 |
|---|---|---|
| `String::from_str` + `eq_str` で文字列の表を引く | 33 | **化石だった — 書き換えた**。表になっている 5 本 (`kind_code` / `field_code` / `is_index_key` / クエリの `key=` 振り分け / `main` の `idx_mode`) と整数の表 `http::reason` を `match` にした。`str` のまま比べるので確保も消えた。残る `eq_str` は 1 回きりの比較。**書き換えの途中で compiled レーンと tree-walker のバグを踏んだ** — 実行時に作った `str` がリテラル腕に当たらない (`b4a33055` で修正) |
| 兄弟の分岐ごとに束縛名を変える (`a_out` / `q_out` …) | 22 | **化石だった — 書き換えた**。`arg_or` は `str` を返すので、そもそも move の対象ではなかった |
| `pack_span` で 2 値を `u64` に詰める | 54 | **化石ではなかった**。元の理由 (戻り値は 8 leaf まで) は WIDE-RETURN で消えたが、span は struct を組んだ**後から**書くので、struct にすると G16 の「struct を struct のフィールドへ / 既存の `var` へ代入できない」に当たる。`record.t` のコメントを今の理由に書き直した。なお「バッファ 16 MiB 上限の根拠もこれ」と書いたのは向きが逆で、詰め方が上限に**頼っている**のであって上限の原因ではない |
| `(Span<u8>, at, len)` の 3 つ組パラメータ | 28 | **化石ではなかった**。`Span::slice` はあるが、この POC はこれを回避策と書いていない — 関数が**外側の窓での絶対位置**を返す設計で、slice にすると位置の意味が変わる。機械的に戻せる形ではない |

**言語側の台帳は `design-docs/todo.md` が正本**なので、ここには
対応する項目名だけを書いた。todo にも `docs/language.md` にも項目が
無かった**新規要望**は 6 つで、2026-09-23 に todo へ登録した:
**enum の struct variant** / ~~名前つき定数を pattern に書けること~~
(同日に解消) / **`break <value>`** / **field shorthand** /
**`val` の struct 分割束縛** / **compiled レーンのコレクションリテラル**。

## R. 前提条件 (回避策が無い / 弱いもの)

上のうち、**設計を曲げるだけでは済まない**ものを再掲する。実装を始める前に
入るなら、入ってから書いた方がよい。

| # | 項目 | なぜ前提か |
|---|---|---|
| ~~R1~~ | ~~ファイル削除~~ | **2026-09-03 に `fs::remove_file` で解消** |
| ~~R2~~ | ~~ファイルの部分読み (`open` + `seek`/`pread`)~~ | **2026-09-05 に `fs::File` で解消** (G2)。保存層は同日 v3 に書き直した (1 ファイル `.seg`、`read_at` で部分読み) |
| **R3** | `SIGTERM` の捕捉 | 標準的な停止でデータが消える。運用手順 (`ExecStop` で管理 API を叩く) で回避しているが、書き漏らしが直接ログ損失になる |
| ~~R4~~ | ~~単調時計~~ | **2026-09-03 に `time::now_mono_ns` で解消** |
| ~~R5~~ | ~~`fsync`~~ | **2026-09-05 に `File::sync()` で解消** (G2)。公開の rename の前に呼んでいるので、§9 の「電源断には強くない」は消えた |

**R3 が入るまでは**、その回避策 (`ExecStop` での明示停止と
`--repair` を勧める復旧手順) を**運用文書に必ず書く**こと。
R2 / R5 は 2026-09-05 に解消し、**同日その回避策を設計から外した** —
穴が埋まったら、埋まる前に曲げた設計を戻すところまでが 1 つの仕事である。

## Z. 解消済み (経緯は git log)

| 項目 | 解消 |
|---|---|
| **G1 ファイルシステム** — 列挙・作成・削除・改名・サイズ | `core/std/fs.t` / `path.t` (2026-09-03)。保存層を v2 に書き直した ([`STORAGE_FORMAT.md`](STORAGE_FORMAT.md)) |
| **G3 時間** — 単調時計 / ns / sleep / ISO 8601 | `core/std/time.t` (2026-09-03)。時刻を ns にし、レイテンシを測れるようになった |
| **G6 シリアライズ** — JSON (reader 込み) / hex / base64 | `core/std/json.t` ほか (2026-09-03) |
| **G13 レベル付きログ** | `core/std/log.t` (2026-09-03) |
| **モジュールの中で配列の添字が書けない** | 処理系の integration (`module_integration.rs` の remap) に `SliceAccess` ほか 5 種の腕が無かった。2026-09-17 に解消 |
| **SIMD の movemask / shuffle** | `__simd_bitmask` / `swizzle` / `bitcast` / `shuffle` (2026-09-03)。[`SIMD.md`](SIMD.md) が使っている |
| **method 引数が型検査されない** | `4866484` (2026-09-05)。**この POC が todo.md に登録した項目が直った最初の例**。`&mut` を値渡しして書き込みが黙って消える最悪の形が `[E0001]` になった |
| **`&mut` を別関数へ渡すと書き込みが捨てられる** | 同上。回避策 (ローカルに組んでから写す) はコピー 1 回の費用が無視できたのでそのまま残してある |
| **method が自分の `&mut` 引数を再借用できない** | `5dbb422` (2026-09-05) |
| **`Vec<T>` の境界が契約になった** | VEC-CONTRACTS (2026-09-04)。`requires` で境界検査が落ちる |
| **戻り値レジスタを超える compound を返せない** | WIDE-RETURN (2026-09-04)。`ParsedLine` の pack や `LogReader` のフィールド削りは不要になった。`&mut self` の書き戻し予算も同時に消えた (2026-09-05 に再確認) |
| **`&mut` 引数を再帰呼び出しに渡せない** (REF-REBORROW) | `5dbb422` (2026-09-05)。**自由関数でも、ループと分岐の内側でも通る**ことを確認した。`logdir::collect` の work list は回避策ではなく選択になった |
| **`Dict<u64, V>` が別モジュールから使えない** | 2026-09-05。`Dict` はハッシュ表になり、`impl Hash for u64` がモジュールを跨いで見える。**表側が splitmix64 で混ぜる**ので、`(from << 32) \| to` のような鍵でも退化しない (`archive.t` の自前表はこの理由では要らなくなった) |
| **G19-1 文字列リテラルに逃げ道が無い** — `\"` も raw リテラルも無く、`{` は必ず補間 | `4de9ce70` (2026-09-23) で `\"` と `r"..."` / `r#"..."#` が入った。**同日に回避策を戻した**: `\u{22}` 435 → **0**、`{{` / `}}` 128 → **4 リテラル** (値そのものに `}}` があるか `\n` と同居するものだけ)、`ui.t` の `put_str` 133 → **1** (ページ全体が raw リテラル 1 つ。出力は書き換え前とバイト一致を確認) |
| **G19-2 narrow int を match に掛けられない** | CHAR-LITERAL-MATCH (2026-09-23)。同日に `== '...'` の連鎖 3 本 (`mount.t` の単位、`query.t` の相対時刻、`record.t` の `fmt_name`) を match に戻した。残りの比較は 1 回きりの `if` で、表ではない |
| **SHA-256 / SHA-224** | `core/std/crypto/` (2026-09-05)。ただしフレームの検査には重すぎるので `src/crc.t` は残る (G7) |
