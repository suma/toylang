# REGION — スコープ付き allocator から取ったメモリを外へ出さない

> 実装: [`frontend/src/type_checker/region_check.rs`](../frontend/src/type_checker/region_check.rs)
> 診断: `E0022` (`--explain E0022`)
> 仕様: [`docs/language.md`](../docs/language.md) の「Region escape」

## なぜ

`with allocator = arena { ... }` は body 中の確保を全部 arena に通し、
arena は**まとめて**返す。これは arena の利点そのものであり、同時に危険:

```rust
fn leak() -> ptr {
    val arena = Arena::new()
    with allocator = arena { __builtin_heap_alloc(8u64) }   # E0022
}                                          # ここで arena が解放する
```

返ったポインタは呼び出し側に届いた時点で既に死んでいる。**他のどの検査も
これを捕まえない** — move check ([`move_check.rs`]) が追うのは「型が資源を
所有する値」で、`ptr` は何も所有していない (所有しているのは arena)。

## 規則

**スコープ付きリージョン**の下で行われた確保に由来する値は、そのリージョンを
所有する束縛より長生きする場所へ到達してはならない。具体的には

- `return` できない
- リージョンのスコープの**外側**で宣言された名前に束縛・代入できない

スコープ内に留まるのは合法 (arena はまだ生きている):

```rust
val arena = Arena::new()
val p = with allocator = arena { __builtin_heap_alloc(8u64) }
__builtin_ptr_write(p, 0u64, 7u64)      # OK
```

## どれがスコープ付きか

寿命が**この pass から見える**ものだけ:

| allocator 式 | 扱い |
|---|---|
| 同じ関数の `val` / `var` 束縛 | スコープ付き (その束縛のスコープが境界) |
| インライン (`with allocator = Arena::new() { ... }`) | スコープ付き (body が境界。何も出せない) |
| パラメータ (`fn f<A: Allocator>(a: A)`) | **対象外** |
| フィールド (`self._h`) | **対象外** |
| `ambient` / `__builtin_default_allocator()` | 対象外 |

パラメータとフィールドを外したのは規則の緩さではなく**正しさ**。
`Arena::alloc` 自身が

```rust
val p = with allocator = self._h { __builtin_heap_alloc(size) }
...
p
```

と書かれていて、このポインタを返すのは正しい — リージョンは `self` のもので、
`self` は呼び出し側が持っている。これを判定するにはリージョンを**シグネチャに
書く** (リージョン多相) 必要があり、それは Phase 1 の範囲外。

## 何がリージョン由来か

**エフェクト表 ([`EFFECT_SYSTEM.md`](EFFECT_SYSTEM.md)) が `Alloc` と言えば
確保**。`never_allocates` と同じ答えなので、3 段深い呼び出しでも数えられ、
どこにも注釈が要らない。加えて**ポインタを持ちうる型**であること:
`ptr` そのものか、中に `ptr` が入りうる compound。arena メモリから読んだ
`u64` はコピーなので `with allocator = arena { list.get(0u64) }` は通り、
`with allocator = arena { list }` は通らない。

この「型がポインタを持ちうるか」は `expr_types` を引く。**この作業で
`expr_types` の穴が 1 つ埋まった**: 文の末尾式 (ブロックの値を決める式) と
`return` の式は `check_expr_located` / `visit_block_stmt` 経由で型検査されて
いて、`visit_expr` を通らないので**型が記録されていなかった**。
ブロックの値がちょうど記録されない、という最悪の穴だったので、両経路で
`set_expr_type` するようにした (move check と effect walk のレシーバ型も
同じ穴を踏んでいた)。

## 既知の穴

- **生ポインタ経由の書き込み** — `__builtin_ptr_write(outer, 0, p)` で外へ
  置くのは追わない。move check と同じ割り切り。
- **呼び出し引数** — リージョンポインタを受け取って保存する関数は未追跡。
- **`reset()`** — ここでのリージョンはスコープ境界で終わる。スコープの
  途中の `arena.reset()` は配ったポインタを全部無効にするが、それを
  捕まえるには実行時 drop flag 相当の flow 感度が要る (MOVE-CONDITIONAL
  が待っているのと同じもの)。
- **関数を跨ぐ流れ** — ambient から確保して返す関数は、リージョンが型に
  入っていないので未検査。

## この先 (Phase 2 = リージョン多相)

`ptr` に「どのリージョンのものか」を付けてシグネチャに出す。そうすると
上の穴のうち「関数を跨ぐ流れ」と「呼び出し引数」が閉じ、パラメータ /
フィールドのリージョンも**対象外ではなく正しく検査**できる。Cyclone の
region system が到達点で、Rust のライフタイムより明確に小さい。
並行性の `Send` 相当 ([`EFFECT_SYSTEM.md`](EFFECT_SYSTEM.md) の「この先」)
は「リージョンを跨がない値」で定義するのが本命なので、そこにも要る。
