use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use super::core::{ParserWithInterner};
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::TypeCheckerVisitor;
use rstest::rstest;

#[cfg(test)]
mod lexer_tests{
    use crate::token::Kind;
    use rayon::prelude::*;

    mod lexer {
        include!(concat!(env!("OUT_DIR"), "/lexer.rs"));
    }

    // Helper function: Create lexer and verify single token
    fn assert_token(input: &str, expected: Kind) {
        let mut l = lexer::Lexer::new(input, 1u64);
        assert_eq!(l.yylex().unwrap().kind, expected, "Input: '{}'", input);
    }

    // Helper function: Verify multiple tokens in sequence
    fn assert_tokens(input: &str, expected: Vec<Kind>) {
        let mut l = lexer::Lexer::new(input, 1u64);
        for exp in expected {
            assert_eq!(l.yylex().unwrap().kind, exp, "Input: '{}'", input);
        }
    }

    #[test]
    fn lexer_keyword_tests_parallel() {
        let test_cases = vec![
            (" if ", Kind::If),
            (" else ", Kind::Else),
            (" while ", Kind::While),
            (" break ", Kind::Break),
            (" continue ", Kind::Continue),
            (" return ", Kind::Return),
            (" for ", Kind::For),
            (" class ", Kind::Class),
            (" fn ", Kind::Function),
            (" val ", Kind::Val),
            (" var ", Kind::Var),
            (" bool ", Kind::Bool),
        ];
        
        test_cases.par_iter().for_each(|&(input, ref expected)| {
            let mut l = lexer::Lexer::new(input, 1u64);
            assert_eq!(l.yylex().unwrap().kind, *expected);
        });
    }

    #[test]
    fn lexer_integer_tests_parallel() {
        let test_cases = vec![
            (" -1i64 ", Kind::Int64(-1)),
            (" 1i64 ", Kind::Int64(1)),
            (" 2u64 ", Kind::UInt64(2u64)),
            (" true ", Kind::True),
            (" false ", Kind::False),
            (" null ", Kind::Null),
            (" 100u64 ", Kind::UInt64(100)),
            (" 123i64 ", Kind::Int64(123)),
        ];
        
        test_cases.par_iter().for_each(|&(input, ref expected)| {
            let mut l = lexer::Lexer::new(input, 1u64);
            assert_eq!(l.yylex().unwrap().kind, *expected);
        });
    }

    #[test]
    fn lexer_symbol_tests_parallel() {
        let test_cases = vec![
            (" ( ", Kind::ParenOpen),
            (" ) ", Kind::ParenClose),
            (" { ", Kind::BraceOpen),
            (" } ", Kind::BraceClose),
            (" [ ", Kind::BracketOpen),
            (" ] ", Kind::BracketClose),
            (" , ", Kind::Comma),
            (" . ", Kind::Dot),
            (" :: ", Kind::DoubleColon),
            (" : ", Kind::Colon),
            (" = ", Kind::Equal),
            (" ! ", Kind::Exclamation),
            (" == ", Kind::DoubleEqual),
            (" != ", Kind::NotEqual),
            (" <= ", Kind::LE),
            (" < ", Kind::LT),
            (" >= ", Kind::GE),
            (" > ", Kind::GT),
            (" + ", Kind::IAdd),
            (" - ", Kind::ISub),
            (" * ", Kind::IMul),
            (" / ", Kind::IDiv),
        ];
        
        test_cases.par_iter().for_each(|&(input, ref expected)| {
            let mut l = lexer::Lexer::new(input, 1u64);
            assert_eq!(l.yylex().unwrap().kind, *expected);
        });
    }

    #[test]
    fn lexer_simple_keyword() {
        let s = " if else while break continue return for class fn val var bool";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::If);
        assert_eq!(l.yylex().unwrap().kind, Kind::Else);
        assert_eq!(l.yylex().unwrap().kind, Kind::While);
        assert_eq!(l.yylex().unwrap().kind, Kind::Break);
        assert_eq!(l.yylex().unwrap().kind, Kind::Continue);
        assert_eq!(l.yylex().unwrap().kind, Kind::Return);
        assert_eq!(l.yylex().unwrap().kind, Kind::For);
        assert_eq!(l.yylex().unwrap().kind, Kind::Class);
        assert_eq!(l.yylex().unwrap().kind, Kind::Function);
        assert_eq!(l.yylex().unwrap().kind, Kind::Val);
        assert_eq!(l.yylex().unwrap().kind, Kind::Var);
        assert_eq!(l.yylex().unwrap().kind, Kind::Bool);
    }

    #[test]
    fn lexer_simple_integer() {
        let s = " -1i64 1i64 2u64  true false null 1234";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::Int64(-1));
        assert_eq!(l.yylex().unwrap().kind, Kind::Int64(1));
        assert_eq!(l.yylex().unwrap().kind, Kind::UInt64(2u64));
        assert_eq!(l.yylex().unwrap().kind, Kind::True);
        assert_eq!(l.yylex().unwrap().kind, Kind::False);
        assert_eq!(l.yylex().unwrap().kind, Kind::Null);
        assert_eq!(l.yylex().unwrap().kind, Kind::Integer("1234".to_string()));
    }

    #[test]
    fn lexer_simple_string() {
        assert_token(" \"string\" ", Kind::String("string".to_string()));
    }

    #[test]
    fn lexer_simple_symbol1() {
        let s = " ( ) { } [ ] , . :: : = !";
        assert_tokens(&s, vec![
            Kind::ParenOpen,
            Kind::ParenClose,
            Kind::BraceOpen,
            Kind::BraceClose,
            Kind::BracketOpen,
            Kind::BracketClose,
            Kind::Comma,
            Kind::Dot,
            Kind::DoubleColon,
            Kind::Colon,
            Kind::Equal,
            Kind::Exclamation,
        ]);
    }

    #[test]
    fn lexer_simple_number() {
        let s = " 100u64 123i64 ";
        assert_tokens(&s, vec![
            Kind::UInt64(100),
            Kind::Int64(123),
        ]);
    }

    #[test]
    fn lexer_simple_symbol2() {
        let s = "== != <= < >= >";
        assert_tokens(&s, vec![
            Kind::DoubleEqual,
            Kind::NotEqual,
            Kind::LE,
            Kind::LT,
            Kind::GE,
            Kind::GT,
        ]);
    }

    #[test]
    fn lexer_arithmetic_operator_symbol() {
        let s = " + - * /";
        assert_tokens(&s, vec![
            Kind::IAdd,
            Kind::ISub,
            Kind::IMul,
            Kind::IDiv,
        ]);
    }

    #[test]
    fn lexer_simple_identifier() {
        let s = " A _name Identifier ";
        assert_tokens(&s, vec![
            Kind::Identifier("A".to_string()),
            Kind::Identifier("_name".to_string()),
            Kind::Identifier("Identifier".to_string()),
        ]);
    }

    #[test]
    fn lexer_multiple_lines() {
        let s = " A \n B ";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::Identifier("A".to_string()));
        assert_eq!(l.yylex().unwrap().kind, Kind::NewLine);
        assert_eq!(l.yylex().unwrap().kind, Kind::Identifier("B".to_string()));
        assert_eq!(l.get_current_line_count(), 2);
    }

    #[test]
    fn lexer_comment_test() {
        let s = "# this is a comment\n val x = 1u64";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::Comment(" this is a comment".to_string()));
        assert_eq!(l.yylex().unwrap().kind, Kind::NewLine);
        assert_eq!(l.yylex().unwrap().kind, Kind::Val);
        assert_eq!(l.yylex().unwrap().kind, Kind::Identifier("x".to_string()));
        assert_eq!(l.yylex().unwrap().kind, Kind::Equal);
        assert_eq!(l.yylex().unwrap().kind, Kind::UInt64(1));
    }
}

mod parser_tests {
    use super::*;
    use crate::token::Kind;

    // Helper function: Create parser and execute parse_stmt()
    fn parse_stmt_success(input: &str) -> ParserWithInterner {
        let mut p = ParserWithInterner::new(input);
        let result = p.parse_stmt();
        assert!(result.is_ok(), "Failed to parse: {} - Error: {:?}", input, result);
        p
    }

    // Helper function: Get element from ExprPool and verify
    fn assert_expr_at(parser: &ParserWithInterner, index: usize, expected: Expr) {
        let actual = parser.get_expr_pool().get(index).unwrap();
        assert_eq!(*actual, expected, "ExprPool[{}] mismatch", index);
    }

    // Helper function: Get element from StmtPool and verify
    fn assert_stmt_at(parser: &ParserWithInterner, index: usize, expected: Stmt) {
        let actual = parser.get_stmt_pool().get(index).unwrap();
        assert_eq!(*actual, expected, "StmtPool[{}] mismatch", index);
    }

    // Helper function: Verify ExprPool size
    fn assert_expr_pool_size(parser: &ParserWithInterner, expected: usize) {
        assert_eq!(parser.get_expr_pool().len(), expected, "ExprPool size mismatch");
    }

    // Helper function: Verify StmtPool size
    fn assert_stmt_pool_size(parser: &ParserWithInterner, expected: usize) {
        assert_eq!(parser.get_stmt_pool().len(), expected, "StmtPool size mismatch");
    }

    #[test]
    fn parser_util_lookahead() {
        let mut p = ParserWithInterner::new("1u64 + 2u64");

        let t0 = p.peek_n(0).unwrap().clone();
        let t1 = p.peek_n(1).unwrap().clone();
        assert_eq!(Kind::UInt64(1), t0);
        assert_eq!(Kind::IAdd, t1);
        
        // Advance 2 tokens
        p.next();
        p.next();

        let t2 = p.peek().unwrap();
        assert_eq!(Kind::UInt64(2), *t2);
    }

    #[test]
    fn parser_comment_skip_test() {
        let p = parse_stmt_success("1u64 + 2u64 # another comment");
        assert_expr_pool_size(&p, 3);
    }

    #[test]
    fn parser_simple_expr_test1() {
        let p = parse_stmt_success("1u64 + 2u64 ");
        assert_expr_pool_size(&p, 3);
        assert_expr_at(&p, 0, Expr::UInt64(1));
        assert_expr_at(&p, 1, Expr::UInt64(2));
        assert_expr_at(&p, 2, Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(1)));

        println!("p.stmt: {:?}", p.get_stmt_pool());
        println!("INSTRUCTION {:?}", p.get_stmt_pool().get(0));
        println!("INSTRUCTION {:?}", p.get_stmt_pool().get(1));
        assert_stmt_pool_size(&p, 1);
        assert_stmt_at(&p, 0, Stmt::Expression(ExprRef(2)));
    }

    #[test]
    fn parser_simple_expr_mul() {
        let p = parse_stmt_success("(1u64) + 2u64 * 3u64");
        assert_expr_pool_size(&p, 5);
        assert_expr_at(&p, 0, Expr::UInt64(1));
        assert_expr_at(&p, 1, Expr::UInt64(2));
        assert_expr_at(&p, 2, Expr::UInt64(3));
        assert_expr_at(&p, 3, Expr::Binary(Operator::IMul, ExprRef(1), ExprRef(2)));
        assert_expr_at(&p, 4, Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(3)));
    }

    #[test]
    fn parser_simple_relational_expr() {
        let p = parse_stmt_success("0u64 < 2u64 + 4u64");
        assert_expr_pool_size(&p, 5);
        assert_expr_at(&p, 0, Expr::UInt64(0));
        assert_expr_at(&p, 1, Expr::UInt64(2));
        assert_expr_at(&p, 2, Expr::UInt64(4));
        assert_expr_at(&p, 3, Expr::Binary(Operator::IAdd, ExprRef(1), ExprRef(2)));
        assert_expr_at(&p, 4, Expr::Binary(Operator::LT, ExprRef(0), ExprRef(3)));
    }

    #[test]
    fn parser_simple_logical_expr() {
        let p = parse_stmt_success("1u64 && 2u64 < 3u64");
        assert_expr_pool_size(&p, 5);
        assert_expr_at(&p, 0, Expr::UInt64(1));
        assert_expr_at(&p, 1, Expr::UInt64(2));
        assert_expr_at(&p, 2, Expr::UInt64(3));
        assert_expr_at(&p, 3, Expr::Binary(Operator::LT, ExprRef(1), ExprRef(2)));
        assert_expr_at(&p, 4, Expr::Binary(Operator::LogicalAnd, ExprRef(0), ExprRef(3)));
    }

    #[rstest]
    #[case("1u64")]
    #[case("(1u64 + 2u64)")]
    #[case("1u64 && 2u64 < 3u64")]
    #[case("1u64 || 2u64 < 3u64")]
    #[case("1u64 || (2u64) < 3u64 + 4u64")]
    #[case("variable")]
    #[case("a + b")]
    #[case("a + 1u64")]
    #[case("a() + 1u64")]
    #[case("a(b,c) + 1u64")]
    fn parser_expr_accept(#[case] input: &str) {
        let mut p = ParserWithInterner::new(input);
        let e = p.parse_stmt();
        assert!(e.is_ok(), "failed: {}", input);
    }

    #[test]
    fn parser_simple_ident_expr() {
        let mut p = parse_stmt_success("abc + 1u64");
        assert_expr_pool_size(&p, 3);
        let expected_symbol = p.get_string_interner().get_or_intern("abc".to_string());
        assert_expr_at(&p, 0, Expr::Identifier(expected_symbol));
        assert_expr_at(&p, 1, Expr::UInt64(1));
        assert_expr_at(&p, 2, Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(1)));
    }

    #[test]
    fn parser_simple_apply_empty() {
        let mut p = parse_stmt_success("abc()");
        assert_expr_pool_size(&p, 2);
        let expected_symbol = p.get_string_interner().get_or_intern("abc".to_string());
        assert_expr_at(&p, 0, Expr::ExprList(vec![]));
        assert_expr_at(&p, 1, Expr::Call(expected_symbol, ExprRef(0)));
    }

    #[test]
    fn parser_simple_assign_expr() {
        let mut p = parse_stmt_success("a = 1u64");
        assert_expr_pool_size(&p, 3);
        let expected_symbol = p.get_string_interner().get_or_intern("a".to_string());
        assert_expr_at(&p, 0, Expr::Identifier(expected_symbol));
        assert_expr_at(&p, 1, Expr::UInt64(1u64));
        assert_expr_at(&p, 2, Expr::Assign(ExprRef(0), ExprRef(1)));
    }

    #[rstest]
    #[case("1u64")]
    #[case("1i64")]
    #[case("true")]
    #[case("false")]
    #[case("null")]
    #[case("\"string\"")]
    #[case("val x = 1u64")]
    #[case("val x: u64 = 1u64")]
    #[case("val x: u64 = if true { 1u64 } else { 2u64 }")]
    #[case("var x = 1u64")]
    #[case("x = y = z = 1u64")]
    #[case("x = 1u64")]
    #[case("if true { 1u64 }")]
    #[case("if true { 1u64 } else { 2u64 }")]
    #[case("{ if true { 1u64 } else { 2u64 } }")]
    #[case("fn_call()")]
    #[case("fn_call(a, b, c)")]
    #[case("a + b * c / d")]
    #[case("a || b && c")]
    #[case("a <= b && c >= d && e < f && g > h")]
    #[case("a == b && c != d")]
    #[case("for i in 0u64 to 9u64 { continue }")]
    #[case("while true { break }")]
    #[case("return true")]
    #[case("return")]
    fn parser_test_parse_stmt(#[case] input: &str) {
        let mut parser = ParserWithInterner::new(input);
        let err = parser.parse_stmt();
        assert!(err.is_ok(), "input: {} err: {:?}", input, err);
    }

    #[rstest]
    #[case("1u64+")]
    #[case("*2u64")]
    #[case("(1u64+2u64")]
    fn parser_errors_parse_expr(#[case] input: &str) {
        let mut parser = ParserWithInterner::new(input);
        assert!(parser.parse_expr_impl().is_err() || parser.errors.len() > 0, "input: {}", input);
    }

    #[test]
    fn parser_simple_apply_expr() {
        let mut p = parse_stmt_success("abc(1u64, 2u64)");
        assert_expr_pool_size(&p, 4);
        assert_expr_at(&p, 0, Expr::UInt64(1));
        assert_expr_at(&p, 1, Expr::UInt64(2));
        assert_expr_at(&p, 2, Expr::ExprList(vec![ExprRef(0), ExprRef(1)]));
        let expected_symbol = p.get_string_interner().get_or_intern("abc".to_string());
        assert_expr_at(&p, 3, Expr::Call(expected_symbol, ExprRef(2)));
    }

    #[test]
    fn parser_param_def() {
        let mut p = ParserWithInterner::new("test: u64");
        let param = p.parse_param_def();
        assert!(param.is_ok());
        let param = param.unwrap();
        let test_id = p.get_string_interner().get_or_intern("test".to_string());
        assert_eq!((test_id, TypeDecl::UInt64), param);
    }

    #[test]
    fn parser_param_def_list_empty() {
        let param = ParserWithInterner::new("").parse_param_def_list(vec![]);
        assert!(param.is_ok());
        let p = param.unwrap();
        assert_eq!(0, p.len());
    }

    #[test]
    fn parser_param_def_list() {
        let mut p = ParserWithInterner::new("test: u64, test2: i64, test3: some_type");
        let param = p.parse_param_def_list(vec![]);
        assert!(param.is_ok());
        let some_type = p.get_string_interner().get_or_intern("some_type".to_string());
        assert_eq!(
            vec![
                (p.get_string_interner().get_or_intern("test".to_string()), TypeDecl::UInt64),
                (p.get_string_interner().get_or_intern("test2".to_string()), TypeDecl::Int64),
                (p.get_string_interner().get_or_intern("test3".to_string()), TypeDecl::Identifier(some_type)),
            ],
            param.unwrap()
        );
    }

    #[rstest]
    fn syntax_test(#[files("tests/syntax*.txt")] path: PathBuf) {
        let file = File::open(&path);
        let mut input = String::new();
        assert!(file.unwrap().read_to_string(&mut input).is_ok());
        let mut p = ParserWithInterner::new(input.as_str());
        let result = p.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err().unwrap());
        let mut program = result.unwrap();
        let string_interner = p.get_string_interner();

        // Collect functions before creating type checker
        let functions_to_register: Vec<_> = program.function.iter().cloned().collect();
        let functions_to_check: Vec<_> = program.function.iter().cloned().collect();

        let mut tc = TypeCheckerVisitor::with_program(&mut program, string_interner);
        functions_to_register.iter().for_each(|f| { tc.add_function(f.clone()) });

        functions_to_check.iter().for_each(|f| {
            let res = tc.type_check(f.clone());
            assert!(res.is_ok(), "type check err {:?}", res.err().unwrap());
        });
    }

    #[rstest]
    fn syntax_error_test(#[files("tests/err_syntax*.txt")] path: PathBuf) {
        let file = File::open(&path);
        let mut input = String::new();
        assert!(file.unwrap().read_to_string(&mut input).is_ok());
        let mut p = ParserWithInterner::new(input.as_str());
        let result = p.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err().unwrap());
        let mut program = result.unwrap();
        let string_interner = p.get_string_interner();

        // Collect functions before creating type checker
        let functions_to_check: Vec<_> = program.function.iter().cloned().collect();

        let mut tc = TypeCheckerVisitor::with_program(&mut program, string_interner);
        let mut res = true;
        functions_to_check.iter().for_each(|f| {
            let r = tc.type_check(f.clone());
            if r.is_err() {
                res = false;
            }
        });

        assert!(!res, "{:?}: type check should fail", path.to_str().unwrap());
    }

    #[test]
    fn parser_struct_decl_simple() {
        let input = "struct Point { x: i64, y: i64 }";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let program = result.unwrap();
        assert_eq!(1, program.statement.len(), "should have one struct declaration");
        
        match program.statement.get(0).unwrap() {
            Stmt::StructDecl { name, fields, .. } => {
                assert_eq!("Point", name);
                assert_eq!(2, fields.len());
                
                assert_eq!("x", fields[0].name);
                assert_eq!(TypeDecl::Int64, fields[0].type_decl);
                assert_eq!(Visibility::Private, fields[0].visibility);
                
                assert_eq!("y", fields[1].name);
                assert_eq!(TypeDecl::Int64, fields[1].type_decl);
                assert_eq!(Visibility::Private, fields[1].visibility);
            }
            _ => panic!("Expected struct declaration"),
        }
    }

    #[test]
    fn parser_struct_decl_with_visibility() {
        let input = "struct Person { pub name: str, age: u64 }";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let program = result.unwrap();
        assert_eq!(1, program.statement.len(), "should have one struct declaration");
        
        match program.statement.get(0).unwrap() {
            Stmt::StructDecl { name, fields, .. } => {
                assert_eq!("Person", name);
                assert_eq!(2, fields.len());
                
                assert_eq!("name", fields[0].name);
                assert_eq!(TypeDecl::String, fields[0].type_decl);
                assert_eq!(Visibility::Public, fields[0].visibility);
                
                assert_eq!("age", fields[1].name);
                assert_eq!(TypeDecl::UInt64, fields[1].type_decl);
                assert_eq!(Visibility::Private, fields[1].visibility);
            }
            _ => panic!("Expected struct declaration"),
        }
    }

    #[test]
    fn parser_struct_decl_empty() {
        let input = "struct Empty { }";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let program = result.unwrap();
        assert_eq!(1, program.statement.len(), "should have one struct declaration");
        
        match program.statement.get(0).unwrap() {
            Stmt::StructDecl { name, fields, .. } => {
                assert_eq!("Empty", name);
                assert_eq!(0, fields.len());
            }
            _ => panic!("Expected struct declaration"),
        }
    }

    #[test]
    fn parser_struct_decl_with_newlines() {
        let input = "struct Point {\n    x: i64,\n    y: i64\n}";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let program = result.unwrap();
        assert_eq!(1, program.statement.len(), "should have one struct declaration");
        
        match program.statement.get(0).unwrap() {
            Stmt::StructDecl { name, fields, .. } => {
                assert_eq!("Point", name);
                assert_eq!(2, fields.len());
                assert_eq!("x", fields[0].name);
                assert_eq!("y", fields[1].name);
            }
            _ => panic!("Expected struct declaration"),
        }
    }

    #[test]
    fn parser_struct_with_function() {
        let input = "struct Point { x: i64, y: i64 }\nfn main() -> u64 { 42u64 }";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let program = result.unwrap();
        assert!(program.statement.len() >= 1, "should have at least one struct declaration");
        assert_eq!(1, program.function.len(), "should have one function");
        
        match program.statement.get(0).unwrap() {
            Stmt::StructDecl { name, fields, .. } => {
                assert_eq!("Point", name);
                assert_eq!(2, fields.len());
            }
            _ => panic!("Expected struct declaration as first statement"),
        }
        
        let func = &program.function[0];
        assert_eq!(parser.get_string_interner().resolve(func.name), Some("main"));
    }

    #[test]
    fn parser_impl_block_simple() {
        let input = "impl Point { fn new(x: i64, y: i64) -> i64 { 42i64 } }";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let program = result.unwrap();
        assert!(program.statement.len() >= 1, "should have at least one impl block");
        
        let impl_stmt = program.statement.0.iter().find(|stmt| {
            matches!(stmt, Stmt::ImplBlock { .. })
        }).expect("Should have impl block");
        
        match impl_stmt {
            Stmt::ImplBlock { target_type, methods } => {
                assert_eq!("Point", target_type);
                assert_eq!(1, methods.len());
                
                let method = &methods[0];
                assert_eq!(parser.get_string_interner().resolve(method.name), Some("new"));
                assert!(!method.has_self_param);
                assert_eq!(2, method.parameter.len());
            }
            _ => panic!("Expected impl block declaration"),
        }
    }

    #[test]
    fn parser_impl_block_with_self() {
        let input = "impl Point { fn distance(&self) -> i64 { 42i64 } }";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let program = result.unwrap();
        assert!(program.statement.len() >= 1, "should have at least one impl block");
        
        let impl_stmt = program.statement.0.iter().find(|stmt| {
            matches!(stmt, Stmt::ImplBlock { .. })
        }).expect("Should have impl block");
        
        match impl_stmt {
            Stmt::ImplBlock { target_type, methods } => {
                assert_eq!("Point", target_type);
                assert_eq!(1, methods.len());
                
                let method = &methods[0];
                assert_eq!(parser.get_string_interner().resolve(method.name), Some("distance"));
                assert!(method.has_self_param);
                assert_eq!(0, method.parameter.len());
            }
            _ => panic!("Expected impl block declaration"),
        }
    }

    #[test]
    fn parser_impl_block_multiple_methods() {
        let input = "impl Point { fn new() -> i64 { 42i64 } fn get_x(&self) -> i64 { 0i64 } }";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let program = result.unwrap();
        assert!(program.statement.len() >= 1, "should have at least one impl block");
        
        let impl_stmt = program.statement.0.iter().find(|stmt| {
            matches!(stmt, Stmt::ImplBlock { .. })
        }).expect("Should have impl block");
        
        match impl_stmt {
            Stmt::ImplBlock { target_type, methods } => {
                assert_eq!("Point", target_type);
                assert_eq!(2, methods.len());
                
                let method1 = &methods[0];
                let string_interner = parser.get_string_interner();
                assert_eq!(string_interner.resolve(method1.name), Some("new"));
                assert!(!method1.has_self_param);
                
                let method2 = &methods[1];
                assert_eq!(string_interner.resolve(method2.name), Some("get_x"));
                assert!(method2.has_self_param);
            }
            _ => panic!("Expected impl block declaration"),
        }
    }

    #[test]
    fn parser_struct_with_impl() {
        let input = "struct Point { x: i64, y: i64 }\nimpl Point { fn new() -> i64 { 42i64 } }";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let program = result.unwrap();
        assert!(program.statement.len() >= 2, "should have struct and impl declarations");
        
        let struct_stmt = program.statement.0.iter().find(|stmt| {
            matches!(stmt, Stmt::StructDecl { .. })
        }).expect("Should have struct declaration");
        
        let impl_stmt = program.statement.0.iter().find(|stmt| {
            matches!(stmt, Stmt::ImplBlock { .. })
        }).expect("Should have impl block");
        
        match struct_stmt {
            Stmt::StructDecl { name, fields, .. } => {
                assert_eq!("Point", name);
                assert_eq!(2, fields.len());
            }
            _ => panic!("Expected struct declaration"),
        }
        
        match impl_stmt {
            Stmt::ImplBlock { target_type, methods } => {
                assert_eq!("Point", target_type);
                assert_eq!(1, methods.len());
            }
            _ => panic!("Expected impl block declaration"),
        }
    }

    #[test]
    fn parser_nested_field_access_simple() {
        let input = "obj.field";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_stmt();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        // Get symbols before accessing expr_pool
        let expected_obj = parser.get_string_interner().get_or_intern("obj".to_string());
        let expected_field = parser.get_string_interner().get_or_intern("field".to_string());
        
        let expr_pool = parser.get_expr_pool();
        assert_eq!(2, expr_pool.len(), "should have 2 expressions for obj.field");
        
        // obj (identifier)
        let obj_expr = expr_pool.get(0).unwrap();
        assert_eq!(Expr::Identifier(expected_obj), *obj_expr);
        
        // field access
        let field_access = expr_pool.get(1).unwrap();
        assert_eq!(Expr::FieldAccess(ExprRef(0), expected_field), *field_access);
    }

    #[test]
    fn parser_nested_field_access_chain() {
        let input = "obj.inner.field";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_stmt();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        // Get symbols before accessing expr_pool
        let expected_obj = parser.get_string_interner().get_or_intern("obj".to_string());
        let expected_inner = parser.get_string_interner().get_or_intern("inner".to_string());
        let expected_field = parser.get_string_interner().get_or_intern("field".to_string());
        
        let expr_pool = parser.get_expr_pool();
        assert_eq!(3, expr_pool.len(), "should have 3 expressions for obj.inner.field");
        
        // obj (identifier)
        let obj_expr = expr_pool.get(0).unwrap();
        assert_eq!(Expr::Identifier(expected_obj), *obj_expr);
        
        // obj.inner
        let inner_access = expr_pool.get(1).unwrap();
        assert_eq!(Expr::FieldAccess(ExprRef(0), expected_inner), *inner_access);
        
        // obj.inner.field
        let field_access = expr_pool.get(2).unwrap();
        assert_eq!(Expr::FieldAccess(ExprRef(1), expected_field), *field_access);
    }

    #[test]
    fn parser_deeply_nested_field_access() {
        let input = "a.b.c.d.e.f";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_stmt();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        // Get symbols before accessing expr_pool
        let expected_a = parser.get_string_interner().get_or_intern("a".to_string());
        let expected_b = parser.get_string_interner().get_or_intern("b".to_string());
        let expected_c = parser.get_string_interner().get_or_intern("c".to_string());
        let expected_d = parser.get_string_interner().get_or_intern("d".to_string());
        let expected_e = parser.get_string_interner().get_or_intern("e".to_string());
        let expected_f = parser.get_string_interner().get_or_intern("f".to_string());
        
        let expr_pool = parser.get_expr_pool();
        assert_eq!(6, expr_pool.len(), "should have 6 expressions for deeply nested access");
        
        // Verify the chain is built correctly
        assert_eq!(Expr::Identifier(expected_a), *expr_pool.get(0).unwrap());
        assert_eq!(Expr::FieldAccess(ExprRef(0), expected_b), *expr_pool.get(1).unwrap());
        assert_eq!(Expr::FieldAccess(ExprRef(1), expected_c), *expr_pool.get(2).unwrap());
        assert_eq!(Expr::FieldAccess(ExprRef(2), expected_d), *expr_pool.get(3).unwrap());
        assert_eq!(Expr::FieldAccess(ExprRef(3), expected_e), *expr_pool.get(4).unwrap());
        assert_eq!(Expr::FieldAccess(ExprRef(4), expected_f), *expr_pool.get(5).unwrap());
    }

    #[test]
    fn parser_field_access_with_method_call() {
        let input = "obj.field.method()";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_stmt();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        // Get symbols before accessing expr_pool
        let expected_obj = parser.get_string_interner().get_or_intern("obj".to_string());
        let expected_field = parser.get_string_interner().get_or_intern("field".to_string());
        let expected_method = parser.get_string_interner().get_or_intern("method".to_string());
        
        let expr_pool = parser.get_expr_pool();
        assert_eq!(3, expr_pool.len(), "should have 3 expressions for field access with method call");
        
        assert_eq!(Expr::Identifier(expected_obj), *expr_pool.get(0).unwrap());
        assert_eq!(Expr::FieldAccess(ExprRef(0), expected_field), *expr_pool.get(1).unwrap());
        assert_eq!(Expr::MethodCall(ExprRef(1), expected_method, vec![]), *expr_pool.get(2).unwrap());
    }

    #[test] 
    fn parser_nested_field_access_stress_test() {
        // Test with very deep nesting to potentially trigger infinite loop issues
        let parts: Vec<&str> = (0..50).map(|i| match i {
            0 => "root",
            _ => "field"
        }).collect();
        let input = parts.join(".");
        
        let mut parser = ParserWithInterner::new(&input);
        let result = parser.parse_stmt();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let expr_pool = parser.get_expr_pool();
        assert_eq!(50, expr_pool.len(), "should have 50 expressions for 50-level nesting");
    }

    // Test for underscore variable infinite loop issue - ORIGINAL
    #[test]
    fn parser_underscore_variable_usage_hang_test() {
        let input = "fn main() -> i64 {\nval _ = 1i64\n_\n}";
        println!("DEBUG: Starting parse with input: {:?}", input);
        let mut parser = ParserWithInterner::new(input);
        println!("DEBUG: Parser created, calling parse_program");
        let result = parser.parse_program();
        println!("DEBUG: parse_program completed: {:?}", result.is_ok());
        
        // This pattern previously caused infinite loop, now fixed
        assert!(result.is_ok(), "Underscore variable definition and usage should parse correctly");
    }

    #[test]
    fn parser_underscore_single_expression_works() {
        // Test just the problematic part: single underscore as expression
        let input = "_";
        println!("DEBUG: Testing single underscore expression: {:?}", input);
        let mut parser = ParserWithInterner::new(input);
        println!("DEBUG: Parser created, calling parse_expr_impl");
        let result = parser.parse_expr_impl();
        println!("DEBUG: parse_expr_impl completed: {:?}", result.is_ok());
        
        // Single underscore should be parsed as identifier expression
        assert!(result.is_ok(), "Single underscore should parse as identifier");
    }

    #[test] 
    fn parser_underscore_prefix_variable_should_work() {
        let input = "fn main() -> i64 {\nval _var = 1i64\n_var\n}";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program();
        
        // This should work since underscore-prefixed variables are allowed
        assert!(result.is_ok(), "Underscore-prefixed variable '_var' should be accepted");
    }

    #[test]
    fn parser_valid_underscore_in_middle_variable() {
        let input = "fn main() -> i64 {\nval var_name = 1i64\n0i64\n}";
        let mut parser = ParserWithInterner::new(input);
        let result = parser.parse_program();
        
        // This should pass because underscores in the middle are allowed
        assert!(result.is_ok(), "Variable with underscore in middle 'var_name' should be accepted");
    }
}