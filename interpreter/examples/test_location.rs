use frontend::{Parser, type_checker::{SourceLocation, TypeCheckError, TypeCheckErrorKind}};
use frontend::type_decl::TypeDecl;

fn main() {
    println!("=== SourceLocation機能のテスト ===");
    
    // テスト用のコード例
    let test_code = r#"fn main() -> u64 {
    val x: i64 = 42;
    val y = "hello";
    x + 1
}"#;
    
    println!("テストコード:");
    for (i, line) in test_code.lines().enumerate() {
        println!("{:2}: {}", i + 1, line);
    }
    println!();
    
    // パーサーの位置情報機能をテスト
    let mut parser = Parser::new(test_code);
    
    println!("パーサーの位置情報:");
    let mut token_count = 0;
    
    while let Some(token_kind) = parser.peek().cloned() {
        if let Some(location) = parser.current_source_location() {
            println!("Token {}: {:?} at line:{}, col:{}, offset:{}", 
                    token_count, token_kind, location.line, location.column, location.offset);
        }
        parser.next();
        token_count += 1;
        
        // 最初の10トークンだけを表示
        if token_count >= 10 {
            break;
        }
    }
    println!();
    
    // SourceLocation構造体の基本テスト
    let example_location = SourceLocation {
        line: 3,
        column: 18,
        offset: 45,
    };
    
    println!("SourceLocation example: {:?}", example_location);
    println!();
    
    // TypeCheckErrorの位置情報テスト
    println!("位置情報付きエラーメッセージのテスト:");
    
    let error_with_location = TypeCheckError::type_mismatch(
        TypeDecl::Int64,
        TypeDecl::String
    ).with_location(example_location.clone());
    
    println!("位置情報付きエラー: {}", error_with_location);
    
    let error_with_context_and_location = TypeCheckError::not_found("Variable", "unknown_var")
        .with_context("variable lookup")
        .with_location(SourceLocation {
            line: 5,
            column: 10,
            offset: 78,
        });
    
    println!("コンテキストと位置情報付きエラー: {}", error_with_context_and_location);
    
    println!("\n=== テスト完了 ===");
}