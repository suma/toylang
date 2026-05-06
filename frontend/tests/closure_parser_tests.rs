// Phase 1 (frontend-only) closure / lambda literal parser tests.
//
// Closures use the `fn(params) -> Ret { body }` form and parse into the
// new `Expr::Closure` variant. Function types `(T1, T2) -> R` parse into
// the new `TypeDecl::Function` variant. These tests confirm the parser
// shape only — the type checker is wired up in Phase 2 and the
// interpreter / JIT / AOT execution paths land in later phases.

#![allow(dead_code)]

use frontend::ParserWithInterner;
use frontend::ast::Expr;
use frontend::type_decl::TypeDecl;

/// Parse a complete program and assert success without checking types.
fn parse_program_ok(source: &str) {
    let mut parser = ParserWithInterner::new(source);
    let result = parser.parse_program();
    assert!(
        result.is_ok() && parser.errors.is_empty(),
        "expected parse success but got: result={:?}, errors={:?}",
        result.as_ref().err(),
        parser.errors
    );
}

#[test]
fn closure_literal_assigned_to_val_parses() {
    parse_program_ok(
        "fn main() -> i64 {
            val f = fn(x: i64) -> i64 { x + 1i64 }
            0i64
        }",
    );
}

#[test]
fn closure_with_multiple_parameters_parses() {
    parse_program_ok(
        "fn main() -> i64 {
            val add = fn(x: i64, y: i64) -> i64 { x + y }
            0i64
        }",
    );
}

#[test]
fn zero_arg_closure_parses() {
    parse_program_ok(
        "fn main() -> u64 {
            val k = fn() -> u64 { 42u64 }
            0u64
        }",
    );
}

#[test]
fn closure_passed_as_argument_parses() {
    parse_program_ok(
        "fn apply(f: (i64) -> i64, x: i64) -> i64 { x }
        fn main() -> i64 {
            apply(fn(x: i64) -> i64 { x * 2i64 }, 5i64)
        }",
    );
}

#[test]
fn function_type_in_param_position_parses() {
    parse_program_ok(
        "fn apply(f: (i64) -> i64, x: i64) -> i64 { x }
        fn main() -> i64 { 0i64 }",
    );
}

#[test]
fn zero_arg_function_type_parses() {
    parse_program_ok(
        "fn run(f: () -> u64) -> u64 { 0u64 }
        fn main() -> u64 { 0u64 }",
    );
}

#[test]
fn closure_returning_closure_parses() {
    // Phase 1 only checks parse — the body's free `x` would need
    // capture support before this could actually type-check.
    parse_program_ok(
        "fn main() -> i64 {
            val outer = fn(x: i64) -> (i64) -> i64 {
                fn(y: i64) -> i64 { y }
            }
            0i64
        }",
    );
}

#[test]
fn top_level_fn_decl_still_parses_after_closure_branch() {
    // Regression: the new `Function`-followed-by-`ParenOpen` lookahead
    // must not swallow regular top-level function declarations whose
    // next token is an `Identifier`.
    parse_program_ok(
        "fn add(a: i64, b: i64) -> i64 { a + b }
        fn main() -> i64 { add(1i64, 2i64) }",
    );
}

#[test]
fn closure_without_return_type_parses() {
    // The `-> Ret` annotation is optional; the body should drive the
    // inferred return type at the type-checker layer (Phase 2).
    parse_program_ok(
        "fn main() -> i64 {
            val f = fn(x: i64) { x + 1i64 }
            0i64
        }",
    );
}

#[test]
fn closure_expr_lands_in_pool_with_correct_shape() {
    // White-box check: confirm the parser produces an `Expr::Closure`
    // node with the expected param count + presence of return type.
    let mut parser = ParserWithInterner::new(
        "fn main() -> i64 {
            val f = fn(x: i64, y: i64) -> i64 { x + y }
            0i64
        }",
    );
    let program = parser.parse_program().expect("parse");
    let pool = &program.expression;
    let mut found = false;
    for i in 0..pool.len() {
        if let Some(Expr::Closure { params, return_type, .. }) =
            pool.get(&frontend::ast::ExprRef(i as u32))
        {
            assert_eq!(params.len(), 2, "expected 2 params, found {}", params.len());
            match return_type {
                Some(TypeDecl::Int64) => {}
                other => panic!("expected Some(Int64) return type, got {:?}", other),
            }
            found = true;
            break;
        }
    }
    assert!(found, "no Expr::Closure node landed in the pool");
}

#[test]
fn fn_prefixed_function_type_in_param_position_parses() {
    // The `fn (T1, T2) -> R` prefixed form should be accepted
    // wherever a bare `(T1, T2) -> R` is — Phase ARG syntax
    // sugar to make function types stand out at a glance and
    // line up visually with closure literals (`fn(x: T) -> R`).
    parse_program_ok(
        "fn apply(f: fn (i64) -> i64, x: i64) -> i64 { f(x) }
        fn main() -> i64 { 0i64 }",
    );
}

#[test]
fn fn_prefixed_function_type_in_val_annotation_parses() {
    parse_program_ok(
        "fn main() -> i64 {
            val f: fn (i64) -> i64 = fn(x: i64) -> i64 { x + 1i64 }
            val z: fn () -> u64 = fn() -> u64 { 42u64 }
            val g: fn (i64, i64) -> i64 = fn(a: i64, b: i64) -> i64 { a + b }
            0i64
        }",
    );
}

#[test]
fn fn_prefixed_function_type_parses_into_typedecl_function() {
    let mut parser = ParserWithInterner::new(
        "fn apply(f: fn (i64, i64) -> i64, x: i64) -> i64 { x }
        fn main() -> i64 { 0i64 }",
    );
    let program = parser.parse_program().expect("parse");
    let apply = program
        .function
        .iter()
        .find(|f| {
            parser
                .get_string_interner()
                .resolve(f.name)
                == Some("apply")
        })
        .expect("apply fn present");
    let (_, ftype) = &apply.parameter[0];
    match ftype {
        TypeDecl::Function(params, ret) => {
            assert_eq!(params.len(), 2, "expected 2 params in fn type");
            assert!(matches!(params[0], TypeDecl::Int64));
            assert!(matches!(params[1], TypeDecl::Int64));
            assert!(matches!(**ret, TypeDecl::Int64));
        }
        other => panic!("expected Function type, got {:?}", other),
    }
}

#[test]
fn function_type_parses_into_typedecl_function() {
    // White-box check: a `(i64) -> i64` annotation lands as
    // `TypeDecl::Function([Int64], Box<Int64>)`.
    let mut parser = ParserWithInterner::new(
        "fn apply(f: (i64) -> i64, x: i64) -> i64 { x }
        fn main() -> i64 { 0i64 }",
    );
    let program = parser.parse_program().expect("parse");
    let apply = program
        .function
        .iter()
        .find(|f| {
            parser
                .get_string_interner()
                .resolve(f.name)
                == Some("apply")
        })
        .expect("apply fn present");
    let (_, ftype) = &apply.parameter[0];
    match ftype {
        TypeDecl::Function(params, ret) => {
            assert_eq!(params.len(), 1, "expected 1 param in fn type");
            assert!(matches!(params[0], TypeDecl::Int64));
            assert!(matches!(**ret, TypeDecl::Int64));
        }
        other => panic!("expected Function type, got {:?}", other),
    }
}
