# Lua Backend TODO

## 完了済み (Completed)

### 基本実装
- [x] **Luaバックエンドの基本設計とディレクトリ構造作成**
  - `lua_backend/` ディレクトリ作成
  - `Cargo.toml` 設定（CompilerSession依存追加）
  - 単一Luaコード生成アプローチで実装決定

- [x] **AST→Luaコード変換エンジンの実装**
  - `LuaCodeGenerator` 構造体実装
  - CompilerSessionとの統合
  - string_interner を使用した文字列解決

- [x] **基本的なLua構文対応**
  - ローカル変数宣言（`val`/`var` → `local`）
  - 算術演算子（`+`, `-`, `*`, `/`）
  - 比較演算子（`==`, `!=` → `~=`, `<`, `<=`, `>`, `>=`）
  - 論理演算子（`&&` → `and`, `||` → `or`）
  - リテラル値（数値、文字列、真偽値）

- [x] **関数定義・呼び出しのLuaコード生成**
  - 関数定義（パラメータ、戻り値）
  - 関数呼び出し（引数リスト展開）
  - 識別子の適切な変換

- [x] **制御フロー（if/else）のLuaコード生成**
  - `IfElifElse` 式の対応
  - 即座実行関数を使った式の値返し実装
  - `elseif` 構文の適切な展開

- [x] **テスト環境とサンプル変換の実装**
  - `lua_gen` バイナリ作成
  - フィボナッチ関数での動作確認
  - シンプルな関数での実行テスト（`add(2, 3) = 5`）

- [x] **代入式（Assign）のLuaコード生成**
  - `x = x + 1` のような代入式を即座実行関数でラップして式として扱う

- [x] **val/var変数の接頭語変換**
  - `val`変数に`V_`接頭語を追加（例：`val pi` → `V_pi`）
  - `var`変数に`v_`接頭語を追加（例：`var counter` → `v_counter`）
  - forループ変数も不変なので`V_`接頭語
  - 関数パラメータは変換しない

## 実装詳細

### 出力形式の特徴
```lua
-- 関数の本体は即座実行関数でラップされる
function add(a, b)
  (function()
    (a + b)
  end)()
end

-- if/else式も即座実行関数で値を返す
(function() if condition then return value1 else return value2 end end)()
```

### CompilerSession統合
- `CompilerSession::new()` でパーサと文字列インターナー管理
- `session.parse_program()` で統一的なAST生成
- `session.string_interner()` で識別子解決

## 未実装 (TODO)

### 高優先度
- [x] **ループ構文対応**
  - [x] `for` ループ（`for i in start to end`）→ Luaの数値forループに変換
  - [x] `while` ループ → Luaのwhileループに変換
  - [x] `break` 文 → Luaのbreakに直接変換
  - [ ] `continue` 文 → 現在はコメントのみ、goto使用での実装が必要

- [ ] **出力コード品質向上**
  - 不要な即座実行関数の除去
  - インデントの改善
  - より読みやすいLuaコード生成

- [ ] **エラーハンドリング改善**
  - 詳細なエラーメッセージ
  - 行番号情報の保持
  - 未対応構文の明確な報告

### 基本機能拡張
- [x] **配列・インデックスアクセス**
  - [x] 配列リテラル（`[1, 2, 3]`）→ Luaテーブルリテラル `{1, 2, 3}` に変換
  - [x] インデックスアクセス（`arr[0]`）→ Lua 1-based インデックス `arr[1]` に変換
  - [x] 配列操作の基本対応

### 基本機能拡張
- [x] **構造体・オブジェクト**
  - [x] 構造体定義 → Luaテーブルコンストラクタ関数に変換
  - [x] フィールドアクセス → Luaテーブルフィールドアクセスに変換
  - [x] メソッド呼び出し → `StructType_method(obj, args)` 形式の関数呼び出しに変換
  - [x] 構造体リテラル → Luaテーブルリテラル `{field = value}` に変換
  - [x] implブロック → `StructType_method` 形式の関数定義に変換

### 中優先度

- [ ] **変数スコープ改善**
  - ブロックスコープの適切な変換
  - 変数名の衝突回避

### 低優先度
- [ ] **モジュールシステム**
  - `import`/`package` 文の対応
  - モジュール間の依存関係管理

- [ ] **型情報の活用**
  - Lua型注釈の生成（オプション）
  - 型に基づく最適化

- [ ] **ビルトイン関数対応**
  - ヒープ操作関数の変換
  - 標準ライブラリ関数のマッピング

## 開発メモ

### 設計判断
1. **単一ファイル出力**: モジュール単位ではなく全体を1つのLuaファイルに出力
2. **即座実行関数**: 式の値を返すために `(function() ... end)()` パターンを使用
3. **CompilerSession使用**: 既存のコンパイラインフラを活用

### テスト方法
```bash
# Luaコード生成
cargo run --bin lua_gen <source_file.t>

# 生成されたLuaコードの実行テスト
cargo run --bin lua_gen test.t > output.lua
lua output.lua
```

### 既知の制限
- continue文は未対応（コメントのみ生成）
- Qualified Identifier関数呼び出し（`Type::method()`）は部分対応（`method()`のみ生成）
- ビルトイン関数未対応
- エラーメッセージが基本的
- メソッド呼び出し時の型名自動検出未対応（現在は`StructType_`プレフィックス使用）

## 次のステップ

1. 出力コード品質向上（不要な関数ラップの除去）
2. continue文の適切な実装（Lua 5.2+のgoto使用）
3. Qualified Identifier関数呼び出しの完全対応
4. メソッド呼び出し時の型名自動検出機能
5. より複雑なサンプルプログラムでのテスト

