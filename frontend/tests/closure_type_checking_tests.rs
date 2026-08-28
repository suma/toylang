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
        // No `;` — toylang separates statements by newline, and the
        // parser now rejects the semicolon instead of silently
        // recovering from it.
        "fn outer<T>(x: T) -> i64 {
            val c = fn() -> i64 {
                x
                0i64
            }
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

// ---------------------------------------------------------------
// CLOSURE-CAPTURE E1 — writing to a captured binding.
//
// A capture is a snapshot taken when the closure is created, so a
// store into it reaches nothing. Before this check the four compiled
// engines discarded the write silently (a counter closure answered
// 1, 1, 0) and the tree-walker rejected it at run time claiming the
// `var` had been declared `val`. The rule is about *where* the
// binding lives, not how it was declared.
// ---------------------------------------------------------------

/// The shape that motivated the whole check: a counter closed over a
/// mutable outer binding.
#[test]
fn assigning_to_a_captured_var_is_rejected() {
    let err = parse_and_type_check(
        "fn main() -> u64 {
            var count: u64 = 0u64
            val bump = fn() -> u64 { count = count + 1u64  count }
            bump()
        }",
    )
    .expect_err("expected a write to a captured `var` to be rejected");
    assert!(
        err.contains("CapturedAssign") && err.contains("count"),
        "expected a CapturedAssign error naming `count`, got: {err}"
    );
}

/// A captured `val` gets the same error rather than the immutability
/// one. "Use `var`" would be a dead end: the `var` spelling is
/// rejected too, so the advice would fix one error into another.
#[test]
fn assigning_to_a_captured_val_reports_the_capture_not_the_immutability() {
    let err = parse_and_type_check(
        "fn main() -> u64 {
            val n: u64 = 1u64
            val f = fn() -> u64 { n = n + 1u64  n }
            f()
        }",
    )
    .expect_err("expected a write to a captured `val` to be rejected");
    assert!(
        err.contains("CapturedAssign"),
        "expected CapturedAssign rather than the immutable-binding error, got: {err}"
    );
}

/// The closure's own parameters are not captures.
#[test]
fn assigning_to_a_closure_parameter_is_not_a_capture() {
    let err = parse_and_type_check(
        "fn main() -> u64 {
            val f = fn(x: u64) -> u64 { x = x + 1u64  x }
            f(1u64)
        }",
    )
    .expect_err("a parameter is still an immutable binding");
    assert!(
        !err.contains("CapturedAssign"),
        "a parameter is local to the closure, not captured: {err}"
    );
}

/// Nor is anything the closure body declares itself.
#[test]
fn assigning_to_a_closure_local_var_is_allowed() {
    parse_and_type_check(
        "fn main() -> u64 {
            val step: u64 = 2u64
            val f = fn(x: u64) -> u64 { var acc: u64 = x  acc = acc + step  acc }
            f(1u64)
        }",
    )
    .expect("a `var` declared inside the closure is not a capture");
}

/// Reading a capture is untouched — that half is consistent across
/// every engine and is what the language documents.
#[test]
fn reading_a_captured_binding_still_type_checks() {
    parse_and_type_check(
        "fn main() -> u64 {
            var n: u64 = 1u64
            val f = fn() -> u64 { n }
            f()
        }",
    )
    .expect("reading a capture is legal");
}

/// The floor is per-closure: an inner closure's own bindings are
/// local to it, and the outer function's are captured by both.
#[test]
fn nested_closures_each_get_their_own_capture_floor() {
    parse_and_type_check(
        "fn main() -> u64 {
            val a: u64 = 1u64
            val outer = fn(b: u64) -> u64 {
                val inner = fn(c: u64) -> u64 { var t: u64 = a  t = t + b + c  t }
                inner(3u64)
            }
            outer(2u64)
        }",
    )
    .expect("locals of the inner closure are not captures");

    let err = parse_and_type_check(
        "fn main() -> u64 {
            var a: u64 = 1u64
            val outer = fn(b: u64) -> u64 {
                val inner = fn(c: u64) -> u64 { a = a + b + c  a }
                inner(3u64)
            }
            outer(2u64)
        }",
    )
    .expect_err("the outer function's binding is captured twice over");
    assert!(
        err.contains("CapturedAssign"),
        "expected CapturedAssign from the inner closure, got: {err}"
    );
}

/// Once the closure is closed, assignment in the enclosing function
/// is ordinary again — the floor must be popped.
#[test]
fn assignment_after_the_closure_is_unaffected() {
    parse_and_type_check(
        "fn main() -> u64 {
            var n: u64 = 1u64
            val f = fn() -> u64 { n }
            n = 5u64
            f()
        }",
    )
    .expect("the capture floor must not outlive the closure body");
}
