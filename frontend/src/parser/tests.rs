use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use super::core::Parser;
use crate::ast::*;
use crate::type_decl::*;
use crate::type_checker::TypeCheckerVisitor;
use rstest::rstest;

mod lexer_tests{
    use super::*;
    use crate::token::Kind;

    mod lexer {
        include!(concat!(env!("OUT_DIR"), "/lexer.rs"));
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
        let s = " \"string\" ";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::String("string".to_string()));
    }

    #[test]
    fn lexer_simple_symbol1() {
        let s = " ( ) { } [ ] , . :: : = !";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::ParenOpen);
        assert_eq!(l.yylex().unwrap().kind, Kind::ParenClose);
        assert_eq!(l.yylex().unwrap().kind, Kind::BraceOpen);
        assert_eq!(l.yylex().unwrap().kind, Kind::BraceClose);
        assert_eq!(l.yylex().unwrap().kind, Kind::BracketOpen);
        assert_eq!(l.yylex().unwrap().kind, Kind::BracketClose);
        assert_eq!(l.yylex().unwrap().kind, Kind::Comma);
        assert_eq!(l.yylex().unwrap().kind, Kind::Dot);
        assert_eq!(l.yylex().unwrap().kind, Kind::DoubleColon);
        assert_eq!(l.yylex().unwrap().kind, Kind::Colon);
        assert_eq!(l.yylex().unwrap().kind, Kind::Equal);
        assert_eq!(l.yylex().unwrap().kind, Kind::Exclamation);
    }

    #[test]
    fn lexer_simple_number() {
        let s = " 100u64 123i64 ";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::UInt64(100));
        assert_eq!(l.yylex().unwrap().kind, Kind::Int64(123));
    }

    #[test]
    fn lexer_simple_symbol2() {
        let s = "== != <= < >= >";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::DoubleEqual);
        assert_eq!(l.yylex().unwrap().kind, Kind::NotEqual);
        assert_eq!(l.yylex().unwrap().kind, Kind::LE);
        assert_eq!(l.yylex().unwrap().kind, Kind::LT);
        assert_eq!(l.yylex().unwrap().kind, Kind::GE);
        assert_eq!(l.yylex().unwrap().kind, Kind::GT);
    }

    #[test]
    fn lexer_arithmetic_operator_symbol() {
        let s = " + - * /";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::IAdd);
        assert_eq!(l.yylex().unwrap().kind, Kind::ISub);
        assert_eq!(l.yylex().unwrap().kind, Kind::IMul);
        assert_eq!(l.yylex().unwrap().kind, Kind::IDiv);
    }

    #[test]
    fn lexer_simple_identifier() {
        let s = " A _name Identifier ";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::Identifier("A".to_string()));
        assert_eq!(l.yylex().unwrap().kind, Kind::Identifier("_name".to_string()));
        assert_eq!(
            l.yylex().unwrap().kind,
            Kind::Identifier("Identifier".to_string())
        );
    }

    #[test]
    fn lexer_multiple_lines() {
        let s = " A \n B ";
        let mut l = lexer::Lexer::new(&s, 1u64);
        assert_eq!(l.yylex().unwrap().kind, Kind::Identifier("A".to_string()));
        assert_eq!(l.yylex().unwrap().kind, Kind::NewLine);
        assert_eq!(l.yylex().unwrap().kind, Kind::Identifier("B".to_string()));
        assert_eq!(l.get_current_line(), 2);
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

    #[test]
    fn parser_util_lookahead() {
        let mut p = Parser::new("1u64 + 2u64");

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
        let mut p = Parser::new("1u64 + 2u64 # another comment");
        let _ = p.parse_stmt().unwrap();
        assert_eq!(3, p.get_expr_pool().len(), "ExprPool.len must be 3");
    }

    #[test]
    fn parser_simple_expr_test1() {
        let mut p = Parser::new("1u64 + 2u64 ");
        let _ = p.parse_stmt().unwrap();
        assert_eq!(3, p.get_expr_pool().len(), "ExprPool.len must be 3");
        let a = p.get_expr_pool().get(0).unwrap();
        assert_eq!(Expr::UInt64(1), *a);
        let b = p.get_expr_pool().get(1).unwrap();
        assert_eq!(Expr::UInt64(2), *b);
        let c = p.get_expr_pool().get(2).unwrap();
        assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(1)), *c);

        println!("p.stmt: {:?}", p.get_stmt_pool());
        println!("INSTRUCTION {:?}", p.get_stmt_pool().get(0));
        println!("INSTRUCTION {:?}", p.get_stmt_pool().get(1));
        assert_eq!(1, p.get_stmt_pool().len(), "stmt.len must be 1");

        let d = p.get_stmt_pool().get(0).unwrap();
        assert_eq!(Stmt::Expression(ExprRef(2)), *d);
    }

    #[test]
    fn parser_simple_expr_mul() {
        let mut p = Parser::new("(1u64) + 2u64 * 3u64");
        let e = p.parse_stmt();
        assert!(e.is_ok());

        assert_eq!(5, p.get_expr_pool().len(), "ExprPool.len must be 3");
        let a = p.get_expr_pool().get(0).unwrap();
        assert_eq!(Expr::UInt64(1), *a);
        let b = p.get_expr_pool().get(1).unwrap();
        assert_eq!(Expr::UInt64(2), *b);
        let c = p.get_expr_pool().get(2).unwrap();
        assert_eq!(Expr::UInt64(3), *c);

        let d = p.get_expr_pool().get(3).unwrap();
        assert_eq!(Expr::Binary(Operator::IMul, ExprRef(1), ExprRef(2)), *d);
        let e = p.get_expr_pool().get(4).unwrap();
        assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(3)), *e);
    }

    #[test]
    fn parser_simple_relational_expr() {
        let mut p = Parser::new("0u64 < 2u64 + 4u64");
        let e = p.parse_stmt();
        assert!(e.is_ok());

        assert_eq!(5, p.get_expr_pool().len(), "ExprPool.len must be 3");
        let a = p.get_expr_pool().get(0).unwrap();
        assert_eq!(Expr::UInt64(0), *a);
        let b = p.get_expr_pool().get(1).unwrap();
        assert_eq!(Expr::UInt64(2), *b);
        let c = p.get_expr_pool().get(2).unwrap();
        assert_eq!(Expr::UInt64(4), *c);

        let d = p.get_expr_pool().get(3).unwrap();
        assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(1), ExprRef(2)), *d);
        let e = p.get_expr_pool().get(4).unwrap();
        assert_eq!(Expr::Binary(Operator::LT, ExprRef(0), ExprRef(3)), *e);
    }

    #[test]
    fn parser_simple_logical_expr() {
        let mut p = Parser::new("1u64 && 2u64 < 3u64");
        let e = p.parse_stmt();
        assert!(e.is_ok());

        assert_eq!(5, p.get_expr_pool().len(), "ExprPool.len must be 3");
        let a = p.get_expr_pool().get(0).unwrap();
        assert_eq!(Expr::UInt64(1), *a);
        let b = p.get_expr_pool().get(1).unwrap();
        assert_eq!(Expr::UInt64(2), *b);
        let c = p.get_expr_pool().get(2).unwrap();
        assert_eq!(Expr::UInt64(3), *c);

        let d = p.get_expr_pool().get(3).unwrap();
        assert_eq!(Expr::Binary(Operator::LT, ExprRef(1), ExprRef(2)), *d);
        let e = p.get_expr_pool().get(4).unwrap();
        assert_eq!(Expr::Binary(Operator::LogicalAnd, ExprRef(0), ExprRef(3)), *e);
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
        let mut p = Parser::new(input);
        let e = p.parse_stmt();
        assert!(e.is_ok(), "failed: {}", input);
    }

    #[test]
    fn parser_simple_ident_expr() {
        let mut p = Parser::new("abc + 1u64");
        let e = p.parse_stmt();
        assert!(e.is_ok());

        assert_eq!(3, p.get_expr_pool().len(), "ExprPool.len must be 3");
        let expected_symbol = p.get_string_interner().get_or_intern("abc".to_string());
        let a = p.get_expr_pool().get(0).unwrap();
        assert_eq!(Expr::Identifier(expected_symbol), *a);
        let b = p.get_expr_pool().get(1).unwrap();
        assert_eq!(Expr::UInt64(1), *b);

        let c = p.get_expr_pool().get(2).unwrap();
        assert_eq!(Expr::Binary(Operator::IAdd, ExprRef(0), ExprRef(1)), *c);
    }

    #[test]
    fn parser_simple_apply_empty() {
        let mut p = Parser::new("abc()");
        let e = p.parse_stmt();
        assert!(e.is_ok());

        assert_eq!(2, p.get_expr_pool().len(), "ExprPool.len must be 2");
        let expected_symbol = p.get_string_interner().get_or_intern("abc".to_string());
        let a = p.get_expr_pool().get(0).unwrap();
        assert_eq!(Expr::ExprList(vec![]), *a);
        let b = p.get_expr_pool().get(1).unwrap();
        assert_eq!(Expr::Call(expected_symbol, ExprRef(0)), *b);
    }

    #[test]
    fn parser_simple_assign_expr() {
        let mut p = Parser::new("a = 1u64");
        let e = p.parse_stmt();
        assert!(e.is_ok());

        assert_eq!(3, p.get_expr_pool().len(), "ExprPool.len must be 3");
        let expected_symbol = p.get_string_interner().get_or_intern("a".to_string());
        let a = p.get_expr_pool().get(0).unwrap();
        assert_eq!(Expr::Identifier(expected_symbol), *a);
        let b = p.get_expr_pool().get(1).unwrap();
        assert_eq!(Expr::UInt64(1u64), *b);
        let c = p.get_expr_pool().get(2).unwrap();
        assert_eq!(Expr::Assign(ExprRef(0), ExprRef(1)), *c);
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
        let mut parser = Parser::new(input);
        let err = parser.parse_stmt();
        assert!(err.is_ok(), "input: {} err: {:?}", input, err);
    }

    #[rstest]
    #[case("1u64+")]
    #[case("*2u64")]
    #[case("(1u64+2u64")]
    fn parser_errors_parse_expr(#[case] input: &str) {
        let mut parser = Parser::new(input);
        assert!(parser.parse_expr_impl().is_err(), "input: {}", input);
    }

    #[test]
    fn parser_simple_apply_expr() {
        let mut p = Parser::new("abc(1u64, 2u64)");
        let e = p.parse_stmt();
        assert!(e.is_ok(), "{:?}", p.get_expr_pool());

        assert_eq!(4, p.get_expr_pool().len(), "ExprPool.len must be 4");
        let a = p.get_expr_pool().get(0).unwrap();
        assert_eq!(Expr::UInt64(1), *a);
        let b = p.get_expr_pool().get(1).unwrap();
        assert_eq!(Expr::UInt64(2), *b);

        let c = p.get_expr_pool().get(2).unwrap();
        assert_eq!(Expr::ExprList(vec![ExprRef(0), ExprRef(1)]), *c);
        let expected_symbol = p.get_string_interner().get_or_intern("abc".to_string());
        let d = p.get_expr_pool().get(3).unwrap();
        assert_eq!(Expr::Call(expected_symbol, ExprRef(2)), *d);
    }

    #[test]
    fn parser_param_def() {
        let mut p = Parser::new("test: u64");
        let param = p.parse_param_def();
        assert!(param.is_ok());
        let param = param.unwrap();
        let test_id = p.string_interner.get_or_intern("test".to_string());
        assert_eq!((test_id, TypeDecl::UInt64), param);
    }

    #[test]
    fn parser_param_def_list_empty() {
        let param = Parser::new("").parse_param_def_list(vec![]);
        assert!(param.is_ok());
        let p = param.unwrap();
        assert_eq!(0, p.len());
    }

    #[test]
    fn parser_param_def_list() {
        let mut p = Parser::new("test: u64, test2: i64, test3: some_type");
        let param = p.parse_param_def_list(vec![]);
        assert!(param.is_ok());
        let some_type = p.string_interner.get_or_intern("some_type".to_string());
        assert_eq!(
            vec![
                (p.string_interner.get_or_intern("test".to_string()), TypeDecl::UInt64),
                (p.string_interner.get_or_intern("test2".to_string()), TypeDecl::Int64),
                (p.string_interner.get_or_intern("test3".to_string()), TypeDecl::Identifier(some_type)),
            ],
            param.unwrap()
        );
    }

    #[rstest]
    fn syntax_test(#[files("tests/syntax*.txt")] path: PathBuf) {
        let file = File::open(&path);
        let mut input = String::new();
        assert!(file.unwrap().read_to_string(&mut input).is_ok());
        let mut p = Parser::new(input.as_str());
        let result = p.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err().unwrap());
        let program = result.unwrap();

        let stmt_pool = &program.statement;
        let mut expr_pool = program.expression;
        let string_interner = &program.string_interner;

        let mut tc = TypeCheckerVisitor::new(stmt_pool, &mut expr_pool, string_interner);
        program.function.iter().for_each(|f| { tc.add_function(f.clone()) });

        program.function.iter().for_each(|f| {
            let res = tc.type_check(f.clone());
            assert!(res.is_ok(), "type check err {:?}", res.err().unwrap());
        });
    }

    #[rstest]
    fn syntax_error_test(#[files("tests/err_syntax*.txt")] path: PathBuf) {
        let file = File::open(&path);
        let mut input = String::new();
        assert!(file.unwrap().read_to_string(&mut input).is_ok());
        let mut p = Parser::new(input.as_str());
        let result = p.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err().unwrap());
        let program = result.unwrap();

        let stmt_pool = program.statement;
        let mut expr_pool = program.expression;
        let interner = program.string_interner;

        let mut tc = TypeCheckerVisitor::new(&stmt_pool, &mut expr_pool, &interner);
        let mut res = true;
        program.function.iter().for_each(|f| {
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
        let mut parser = Parser::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let program = result.unwrap();
        assert_eq!(1, program.statement.len(), "should have one struct declaration");
        
        match program.statement.get(0).unwrap() {
            Stmt::StructDecl { name, fields } => {
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
        let mut parser = Parser::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let program = result.unwrap();
        assert_eq!(1, program.statement.len(), "should have one struct declaration");
        
        match program.statement.get(0).unwrap() {
            Stmt::StructDecl { name, fields } => {
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
        let mut parser = Parser::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let program = result.unwrap();
        assert_eq!(1, program.statement.len(), "should have one struct declaration");
        
        match program.statement.get(0).unwrap() {
            Stmt::StructDecl { name, fields } => {
                assert_eq!("Empty", name);
                assert_eq!(0, fields.len());
            }
            _ => panic!("Expected struct declaration"),
        }
    }

    #[test]
    fn parser_struct_decl_with_newlines() {
        let input = "struct Point {\n    x: i64,\n    y: i64\n}";
        let mut parser = Parser::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let program = result.unwrap();
        assert_eq!(1, program.statement.len(), "should have one struct declaration");
        
        match program.statement.get(0).unwrap() {
            Stmt::StructDecl { name, fields } => {
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
        let mut parser = Parser::new(input);
        let result = parser.parse_program();
        assert!(result.is_ok(), "parse err {:?}", result.err());
        
        let program = result.unwrap();
        assert!(program.statement.len() >= 1, "should have at least one struct declaration");
        assert_eq!(1, program.function.len(), "should have one function");
        
        match program.statement.get(0).unwrap() {
            Stmt::StructDecl { name, fields } => {
                assert_eq!("Point", name);
                assert_eq!(2, fields.len());
            }
            _ => panic!("Expected struct declaration as first statement"),
        }
        
        let func = &program.function[0];
        assert_eq!(program.string_interner.resolve(func.name), Some("main"));
    }

    #[test]
    fn parser_impl_block_simple() {
        let input = "impl Point { fn new(x: i64, y: i64) -> i64 { 42i64 } }";
        let mut parser = Parser::new(input);
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
                assert_eq!(program.string_interner.resolve(method.name), Some("new"));
                assert!(!method.has_self_param);
                assert_eq!(2, method.parameter.len());
            }
            _ => panic!("Expected impl block declaration"),
        }
    }

    #[test]
    fn parser_impl_block_with_self() {
        let input = "impl Point { fn distance(&self) -> i64 { 42i64 } }";
        let mut parser = Parser::new(input);
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
                assert_eq!(program.string_interner.resolve(method.name), Some("distance"));
                assert!(method.has_self_param);
                assert_eq!(0, method.parameter.len());
            }
            _ => panic!("Expected impl block declaration"),
        }
    }

    #[test]
    fn parser_impl_block_multiple_methods() {
        let input = "impl Point { fn new() -> i64 { 42i64 } fn get_x(&self) -> i64 { 0i64 } }";
        let mut parser = Parser::new(input);
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
                assert_eq!(program.string_interner.resolve(method1.name), Some("new"));
                assert!(!method1.has_self_param);
                
                let method2 = &methods[1];
                assert_eq!(program.string_interner.resolve(method2.name), Some("get_x"));
                assert!(method2.has_self_param);
            }
            _ => panic!("Expected impl block declaration"),
        }
    }

    #[test]
    fn parser_struct_with_impl() {
        let input = "struct Point { x: i64, y: i64 }\nimpl Point { fn new() -> i64 { 42i64 } }";
        let mut parser = Parser::new(input);
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
            Stmt::StructDecl { name, fields } => {
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
}