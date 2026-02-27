# 3層抽象化アーキテクチャ: 組み込み関数システム設計

## 概要

インタープリターおよびLLVM IRネイティブコード生成の複数バックエンドに対応できる組み込み関数システムの設計文書です。
既存のコンパイラアーキテクチャを活用し、段階的に実装可能な3層抽象化アプローチを採用します。

## アーキテクチャ全体図

```
┌─────────────────────────────────────────────────────────┐
│                    Frontend                             │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────────┐  │
│  │   Parser    │→│     AST     │→│  Type Checker   │  │
│  │             │  │  (Layer 1)  │  │                 │  │
│  └─────────────┘  └─────────────┘  └─────────────────┘  │
└─────────────────────────────────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────┐
│              Backend Abstraction (Layer 2)              │
│                    ExecutionBackend                     │
└─────────────────────────────────────────────────────────┘
      │                    │                    │
      ▼                    ▼                    ▼
┌─────────────┐    ┌─────────────┐    ┌─────────────┐
│Interpreter  │    │ LLVM Codegen│    │Lua Bytecode │
│   Backend   │    │   Backend   │    │   Backend   │
│             │    │             │    │             │
│┌───────────┐│    │┌───────────┐│    │┌───────────┐│
││ Builtin   ││    ││ LLVM IR   ││    ││Lua Bytecode││
││ Runtime   ││    ││Generation ││    ││Generation ││
││(Layer 3)  ││    ││(Layer 3)  ││    ││(Layer 3)  ││
│└───────────┘│    │└───────────┘│    │└───────────┘│
└─────────────┘    └─────────────┘    └─────────────┘
```

## Layer設計

### Layer 1: AST拡張

#### 1.1 BuiltinFunction定義

```rust
// frontend/src/ast.rs
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BuiltinFunction {
    // 数学関数
    AbsI64, AbsU64, SqrtU64, MinI64, MaxI64, MinU64, MaxU64,
    
    // 文字列関数
    StrLen, StrConcat, StrSubstring, StrContains,
    
    // 配列関数
    ArrayLen, ArrayPush, ArrayPop, ArrayGet, ArraySet,
    
    // 型変換関数
    I64ToString, U64ToString, BoolToString,
    StringToI64, StringToU64, StringToBool,
    
    // IO関数（将来用）
    Print, PrintLn, ReadLine,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    // 既存のバリアント...
    BuiltinCall(BuiltinFunction, Vec<ExprRef>),
}
```

#### 1.2 パーサー拡張

```rust
// frontend/src/parser/expr.rs
impl Parser {
    fn parse_builtin_call(&mut self, func_name: &str) -> ParserResult<Expr> {
        let builtin_func = match func_name {
            "__builtin_str_len" => BuiltinFunction::StrLen,
            "__builtin_str_concat" => BuiltinFunction::StrConcat,
            "__builtin_abs_i64" => BuiltinFunction::AbsI64,
            "__builtin_array_len" => BuiltinFunction::ArrayLen,
            // ...
            _ => return Err(ParserError::unknown_builtin(func_name)),
        };
        
        self.expect(&Kind::ParenOpen)?;
        let args = self.parse_expression_list()?;
        self.expect(&Kind::ParenClose)?;
        
        Ok(Expr::BuiltinCall(builtin_func, args))
    }
}
```

#### 1.3 型チェッカー拡張

```rust
// frontend/src/type_checker.rs
impl TypeCheckerVisitor {
    fn check_builtin_call(&mut self, func: &BuiltinFunction, args: &[ExprRef]) -> Result<TypeDecl, TypeCheckError> {
        match func {
            BuiltinFunction::StrLen => {
                self.check_arg_count(args, 1, "str_len")?;
                let arg_type = self.visit_expr(&args[0])?;
                self.expect_type(&arg_type, &TypeDecl::String, "str_len first argument")?;
                Ok(TypeDecl::UInt64)
            }
            BuiltinFunction::StrConcat => {
                self.check_arg_count(args, 2, "str_concat")?;
                let arg1_type = self.visit_expr(&args[0])?;
                let arg2_type = self.visit_expr(&args[1])?;
                self.expect_type(&arg1_type, &TypeDecl::String, "str_concat first argument")?;
                self.expect_type(&arg2_type, &TypeDecl::String, "str_concat second argument")?;
                Ok(TypeDecl::String)
            }
            BuiltinFunction::AbsI64 => {
                self.check_arg_count(args, 1, "abs_i64")?;
                let arg_type = self.visit_expr(&args[0])?;
                self.expect_type(&arg_type, &TypeDecl::Int64, "abs_i64 argument")?;
                Ok(TypeDecl::Int64)
            }
            // ...
        }
    }
}
```

### Layer 2: バックエンド抽象化

#### 2.1 ExecutionBackendトレイト

```rust
// compiler_core/src/backend.rs
pub trait ExecutionBackend {
    type Value;
    type Error;
    
    // 組み込み関数実行
    fn execute_builtin(
        &mut self, 
        func: BuiltinFunction, 
        args: &[Self::Value]
    ) -> Result<Self::Value, Self::Error>;
    
    // プログラム実行
    fn execute_program(&mut self, program: &Program) -> Result<Self::Value, Self::Error>;
    
    // 式評価
    fn evaluate_expression(&mut self, expr: &Expr) -> Result<Self::Value, Self::Error>;
}
```

#### 2.2 インタープリターバックエンド

```rust
// interpreter/src/backend.rs
pub struct InterpreterBackend {
    environment: Environment,
    // 既存のフィールド...
}

impl ExecutionBackend for InterpreterBackend {
    type Value = Rc<RefCell<Object>>;
    type Error = String;
    
    fn execute_builtin(
        &mut self, 
        func: BuiltinFunction, 
        args: &[Self::Value]
    ) -> Result<Self::Value, Self::Error> {
        match func {
            BuiltinFunction::StrLen => {
                let s = self.extract_string(&args[0])?;
                Ok(Rc::new(RefCell::new(Object::UInt64(s.len() as u64))))
            }
            BuiltinFunction::StrConcat => {
                let s1 = self.extract_string(&args[0])?;
                let s2 = self.extract_string(&args[1])?;
                Ok(Rc::new(RefCell::new(Object::String(format!("{}{}", s1, s2)))))
            }
            BuiltinFunction::AbsI64 => {
                let n = self.extract_i64(&args[0])?;
                Ok(Rc::new(RefCell::new(Object::Int64(n.abs()))))
            }
            // ...
        }
    }
}
```

#### 2.3 LLVM IRバックエンド（将来実装）

```rust
// native/src/backend.rs (将来実装)
pub struct LLVMBackend<'ctx> {
    context: &'ctx Context,
    module: Module<'ctx>,
    builder: Builder<'ctx>,
    // LLVM関連のフィールド...
}

impl<'ctx> ExecutionBackend for LLVMBackend<'ctx> {
    type Value = BasicValueEnum<'ctx>;
    type Error = CodegenError;
    
    fn execute_builtin(
        &mut self, 
        func: BuiltinFunction, 
        args: &[Self::Value]
    ) -> Result<Self::Value, Self::Error> {
        match func {
            BuiltinFunction::StrLen => {
                // LLVM IRでstrlen呼び出しを生成
                let strlen_fn = self.get_or_declare_strlen();
                Ok(self.builder.build_call(strlen_fn, &[args[0]], "strlen_call")
                    .try_as_basic_value().left().unwrap())
            }
            BuiltinFunction::AbsI64 => {
                // LLVM IRでabs関数を生成
                let value = args[0].into_int_value();
                let zero = self.context.i64_type().const_zero();
                let is_negative = self.builder.build_int_compare(
                    IntPredicate::SLT, value, zero, "is_negative"
                );
                let negated = self.builder.build_int_sub(zero, value, "negated");
                Ok(self.builder.build_select(is_negative, negated, value, "abs")
                    .as_basic_value_enum())
            }
            // ...
        }
    }
}
```

### Layer 3: 言語内組み込みモジュール

#### 3.1 builtin/string.t

```rust
// builtin/string.t
package builtin.string

# 文字列長取得
pub fn len(s: str) -> u64 {
    __builtin_str_len(s)
}

# 文字列連結
pub fn concat(a: str, b: str) -> str {
    __builtin_str_concat(a, b)
}

# 部分文字列取得
pub fn substring(s: str, start: u64, end: u64) -> str {
    __builtin_str_substring(s, start, end)
}

# 文字列検索
pub fn contains(haystack: str, needle: str) -> bool {
    __builtin_str_contains(haystack, needle)
}
```

#### 3.2 builtin/math.t

```rust
// builtin/math.t
package builtin.math

# 絶対値
pub fn abs(x: i64) -> i64 {
    __builtin_abs_i64(x)
}

pub fn abs_u(x: u64) -> u64 {
    __builtin_abs_u64(x)
}

# 最小・最大
pub fn min(a: i64, b: i64) -> i64 {
    __builtin_min_i64(a, b)
}

pub fn max(a: i64, b: i64) -> i64 {
    __builtin_max_i64(a, b)
}
```

#### 3.3 builtin/array.t

```rust
// builtin/array.t
package builtin.array

# 配列長取得
pub fn len(arr: [T]) -> u64 {
    __builtin_array_len(arr)
}

# 要素取得（境界チェック付き）
pub fn get(arr: [T], index: u64) -> T {
    __builtin_array_get(arr, index)
}

# 要素設定（境界チェック付き）
pub fn set(arr: [T], index: u64, value: T) -> [T] {
    __builtin_array_set(arr, index, value)
}
```

## 実装ロードマップ

### Phase 1: Layer 1基盤実装 (Week 1-2)

**Priority:** 🔥 High

**Tasks:**
1. BuiltinFunction enum定義
2. Expr::BuiltinCall追加  
3. パーサーで__builtin_*関数の解析
4. 基本的な組み込み関数（StrLen, AbsI64）の型チェック実装

**Deliverables:**
- AST拡張完了
- 基本的な組み込み関数のパースと型チェック
- テストケース: `builtin_function_parsing_test.t`

**実装手順:**
1. `frontend/src/ast.rs`にBuiltinFunction enumを追加
2. `Expr`バリアントにBuiltinCallを追加
3. `frontend/src/parser/expr.rs`でパーサー拡張
4. `frontend/src/type_checker.rs`で型チェック実装
5. パースと型チェックのテストケース作成

### Phase 2: インタープリター統合 (Week 3)

**Priority:** 🔥 High

**Tasks:**  
1. InterpreterBackendでの組み込み関数実行実装
2. 既存のevaluationロジックとの統合
3. 基本的な組み込み関数ライブラリ（string, math）実装

**Deliverables:**
- 動作する組み込み関数システム
- テストケース: `builtin_execution_test.t`

**実装手順:**
1. `interpreter/src/evaluation.rs`でBuiltinCall評価実装
2. 基本的な組み込み関数（StrLen, StrConcat, AbsI64）実装
3. エラーハンドリングとデバッグメッセージ
4. 実行テストケース作成

### Phase 3: バックエンド抽象化 (Week 4)

**Priority:** 🟡 Medium

**Tasks:**
1. ExecutionBackendトレイト設計・実装
2. 既存InterpreterをBackendトレイトに適合
3. バックエンド切り替え機構の実装

**Deliverables:**
- バックエンド抽象化完了
- 将来のLLVMバックエンド追加への準備完了

**実装手順:**
1. `compiler_core/src/backend.rs`でExecutionBackendトレイト定義
2. `interpreter/src/backend.rs`でInterpeterBackend実装
3. 既存コードのリファクタリング
4. バックエンド選択機能の実装

### Phase 4: 組み込みモジュールシステム (Week 5-6)

**Priority:** 🟡 Medium

**Tasks:**
1. builtin.string, builtin.mathモジュール実装
2. モジュールシステムとの統合
3. 自動import機構の実装

**Deliverables:**
- 完全な組み込みモジュールシステム
- ユーザーフレンドリーなAPI

**実装手順:**
1. `builtin/`ディレクトリに組み込みモジュール作成
2. モジュールリゾルバーとの統合
3. 自動importとnamespace解決
4. ドキュメントとテストケース作成

### Phase 5: LLVM IR準備 (Future)

**Priority:** 🟢 Low (将来実装)

**Tasks:**
1. LLVM依存関係追加
2. LLVMBackend基本構造実装
3. 基本的な組み込み関数のLLVM IR生成

**Deliverables:**
- 概念実証レベルのLLVMバックエンド

**実装手順:**
1. `Cargo.toml`にllvm-sys依存関係追加
2. `native/src/backend.rs`でLLVMBackend実装
3. 基本的なIR生成とランタイム関数呼び出し
4. パフォーマンステストとベンチマーク

## 技術的考慮点

### メモリ管理

```rust
// インタープリター: Rc<RefCell<Object>>
// LLVM IR: LLVM値（スタック/ヒープ管理）
// 抽象化により全バックエンドに対応
```

### エラーハンドリング

```rust
// 統一的なエラー型
pub enum BuiltinError {
    ArgumentCountMismatch { expected: usize, actual: usize },
    TypeMismatch { expected: TypeDecl, actual: TypeDecl },
    RuntimeError(String),
    LLVMError(String),      // 将来用
}
```

### パフォーマンス最適化

```rust
// インタープリター: 関数ポインタテーブル
// LLVM IR: インライン展開 + 最適化
```

## 設計原則

1. **単一のAST**: フロントエンドは一つ、バックエンドは複数対応
2. **段階的実装**: Phase 1から順次実装、既存コードへの影響最小化
3. **型安全性**: 組み込み関数も完全な型チェック対象
4. **拡張性**: 新しい組み込み関数の追加が容易
5. **バックエンド中立性**: インタープリター/LLVMで同じAPI
6. **パフォーマンス選択**: 用途に応じて最適なバックエンドを選択可能

## 推奨実装開始点

**Phase 1: Layer 1基盤実装**から開始することを推奨します。

**理由:**
1. 既存コードベースへの影響が最小
2. 段階的な検証が可能
3. 将来のLLVM統合への基盤作り
4. すぐに実用的な組み込み関数が使用可能

**最初に実装すべき組み込み関数:**
1. **文字列操作** (`str.len`, `str.concat`) - 実用性が高い
2. **数学関数** (`math.abs`, `math.min`, `math.max`) - 実装が単純
3. **配列操作** (`array.len`, `array.get`) - 型システムとの統合確認

## ファイル構成

```
├── frontend/src/
│   ├── ast.rs                    # BuiltinFunction enum追加
│   ├── parser/expr.rs            # パーサー拡張
│   └── type_checker.rs           # 型チェック拡張
├── compiler_core/src/
│   └── backend.rs                # ExecutionBackendトレイト
├── interpreter/src/
│   ├── backend.rs                # InterpreterBackend実装  
│   └── evaluation.rs             # 組み込み関数評価
├── native/src/                   # 将来のLLVMバックエンド
│   └── backend.rs
└── builtin/                      # 組み込みモジュール
    ├── string.t
    ├── math.t
    └── array.t
```

---

**作成日**: 2025-08-17  
**バージョン**: 1.0  
**ステータス**: 設計完了、実装準備完了