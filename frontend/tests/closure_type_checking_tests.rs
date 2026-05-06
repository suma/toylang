// Phase 2 (frontend-only) closure / lambda type-checker tests.
//
// Verifies the new `Expr::Closure` arm in `TypeCheckerVisitor`:
//   - body type-checks under a fresh scope binding declared params
//   - declared return type, when present, must match body type
//   - call sites via function-typed bindings (`val f = fn ...; f(x)`)
//   - free-variable capture lands in `closure_captures` side-table
//   - generic-param leakage is rejected
//
// Phase 3+ (interpreter / JIT / AOT) are not exercised here — these
// tests stop at type-check.

#![allow(dead_code)]

use frontend::ParserWithInterner;
use frontend::ast::Expr;
use frontend::type_checker::TypeCheckerVisitor;
use frontend::type_decl::TypeDecl;

/// Parse a complete program and run the type checker on every
/// user-authored function. Returns Ok(()) on type-check success or a
/// concatenated error string on first failure.
fn parse_and_type_check(source: &str) -> Result<(), String> {
    let mut parser = ParserWithInterner::new(source);
    let mut program = parser.parse_program().map_err(|e| format!("parse error: {:?}", e))?;
    let functions = program.function.clone();
    let string_interner = parser.get_string_interner();
    let mut tc = TypeCheckerVisitor::with_program(&mut program, string_interner);
    let mut errors = Vec::new();
    for f in functions.iter() {
        if let Err(e) = tc.type_check(f.clone()) {
            errors.push(format!("{:?}", e));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

#[test]
fn closure_literal_with_return_annotation_type_checks() {
    parse_and_type_check(
        "fn main() -> i64 {
            val f = fn(x: i64) -> i64 { x + 1i64 }
            0i64
        }",
    )
    .expect("expected closure with declared return to type-check");
}

#[test]
fn closure_literal_inferred_return_type() {
    parse_and_type_check(
        "fn main() -> i64 {
            val f = fn(x: i64) { x + 1i64 }
            0i64
        }",
    )
    .expect("expected closure with inferred return to type-check");
}

#[test]
fn closure_assigned_then_called_through_value() {
    parse_and_type_check(
        "fn main() -> i64 {
            val f = fn(x: i64) -> i64 { x + 1i64 }
            f(41i64)
        }",
    )
    .expect("indirect call via function-typed binding should type-check");
}

#[test]
fn function_typed_param_accepts_closure_argument() {
    parse_and_type_check(
        "fn apply(f: (i64) -> i64, x: i64) -> i64 { f(x) }
        fn main() -> i64 {
            apply(fn(x: i64) -> i64 { x * 2i64 }, 21i64)
        }",
    )
    .expect("HOF + closure literal as arg should type-check");
}

#[test]
fn closure_return_type_mismatch_is_rejected() {
    let err = parse_and_type_check(
        "fn main() -> i64 {
            val f = fn(x: i64) -> bool { x + 1i64 }
            0i64
        }",
    )
    .expect_err("body returning i64 but declared bool should be rejected");
    assert!(
        err.contains("declared return type") || err.contains("Bool") || err.contains("bool"),
        "unexpected error message: {}",
        err
    );
}

#[test]
fn indirect_call_arg_count_mismatch_is_rejected() {
    let err = parse_and_type_check(
        "fn main() -> i64 {
            val f = fn(x: i64, y: i64) -> i64 { x + y }
            f(1i64)
        }",
    )
    .expect_err("calling 2-arg fn value with 1 arg should be rejected");
    assert!(
        err.contains("argument count mismatch") || err.contains("expected 2"),
        "unexpected error message: {}",
        err
    );
}

#[test]
fn indirect_call_arg_type_mismatch_is_rejected() {
    let err = parse_and_type_check(
        "fn main() -> i64 {
            val f = fn(x: i64) -> i64 { x + 1i64 }
            f(true)
        }",
    )
    .expect_err("passing bool to i64 fn value should be rejected");
    assert!(
        err.contains("type mismatch") || err.contains("Bool"),
        "unexpected error message: {}",
        err
    );
}

#[test]
fn closure_with_undefined_free_var_is_rejected() {
    let err = parse_and_type_check(
        "fn main() -> i64 {
            val c = fn(x: i64) -> i64 { x + nope }
            0i64
        }",
    )
    .expect_err("undefined free var in closure body should be rejected");
    assert!(
        err.contains("nope") || err.contains("not found") || err.contains("Variable not found"),
        "unexpected error message: {}",
        err
    );
}

#[test]
fn closure_capture_lands_in_side_table() {
    // White-box: after type-checking, the side table should record `n`
    // as a captured `i64` keyed by the closure body's ExprRef.
    let mut parser = ParserWithInterner::new(
        "fn main() -> i64 {
            val n: i64 = 10i64
            val c = fn(x: i64) -> i64 { x + n }
            c(5i64)
        }",
    );
    let mut program = parser.parse_program().expect("parse");
    // Find the closure body's ExprRef before creating the type checker —
    // the visitor takes `&mut program`, so we can't query the pool
    // afterwards through `program.expression`.
    let mut closure_body_ref = None;
    for i in 0..program.expression.len() {
        if let Some(Expr::Closure { body, .. }) =
            program.expression.get(&frontend::ast::ExprRef(i as u32))
        {
            closure_body_ref = Some(body);
            break;
        }
    }
    let body_ref = closure_body_ref.expect("Expr::Closure not found in pool");
    let functions = program.function.clone();
    let string_interner = parser.get_string_interner();
    let mut tc = TypeCheckerVisitor::with_program(&mut program, string_interner);
    for f in functions.iter() {
        tc.type_check(f.clone()).expect("type check");
    }
    let captures = tc
        .context
        .closure_captures
        .get(&body_ref)
        .expect("captures missing for closure body");
    assert_eq!(captures.len(), 1, "expected 1 capture, got {:?}", captures);
    let (_, ty) = &captures[0];
    assert!(matches!(ty, TypeDecl::Int64), "expected Int64 capture, got {:?}", ty);
}

#[test]
fn closure_capturing_generic_param_typed_var_is_rejected() {
    // `outer<T>(x: T)` binds `x: T`; the closure captures `x` whose
    // type mentions enclosing generic `T` — currently rejected.
    let err = parse_and_type_check(
        "fn outer<T>(x: T) -> i64 {
            val c = fn() -> i64 { x; 0i64 }
            0i64
        }",
    )
    .expect_err("generic-typed capture should be rejected");
    assert!(
        err.contains("generic-parameterised closures are not yet supported"),
        "unexpected error message: {}",
        err
    );
}

#[test]
fn closure_with_no_free_vars_records_empty_capture_set() {
    let mut parser = ParserWithInterner::new(
        "fn main() -> i64 {
            val c = fn(x: i64) -> i64 { x + 1i64 }
            c(10i64)
        }",
    );
    let mut program = parser.parse_program().expect("parse");
    let mut closure_body_ref = None;
    for i in 0..program.expression.len() {
        if let Some(Expr::Closure { body, .. }) =
            program.expression.get(&frontend::ast::ExprRef(i as u32))
        {
            closure_body_ref = Some(body);
            break;
        }
    }
    let body_ref = closure_body_ref.expect("Expr::Closure not found in pool");
    let functions = program.function.clone();
    let string_interner = parser.get_string_interner();
    let mut tc = TypeCheckerVisitor::with_program(&mut program, string_interner);
    for f in functions.iter() {
        tc.type_check(f.clone()).expect("type check");
    }
    let captures = tc
        .context
        .closure_captures
        .get(&body_ref)
        .expect("captures entry missing");
    assert!(captures.is_empty(), "expected no captures, got {:?}", captures);
}
