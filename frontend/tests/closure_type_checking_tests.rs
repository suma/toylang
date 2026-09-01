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

/// What the captured-assign error says, and nothing else does.
///
/// These assertions used to look for the *kind* name `CapturedAssign`,
/// which only the derived `Debug` spells — so the helper below had to
/// render errors with `{:?}` and a failure printed the whole error
/// struct, interned symbol ids and all (DIAG-SYMBOL-NAME). The message
/// separates this error from the immutable-binding one just as well,
/// and pinning it means the tests pin what a user actually reads.
const CAPTURED_ASSIGN: &str = "from inside a closure";

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
            // DIAG-SYMBOL-NAME: render the way a user would see it.
            // `{:?}` here is the derived Debug of the whole error
            // struct, which spells a type as
            // `Struct(SymbolU32 { value: 60 }, [])`.
            errors.push(e.message_with(Some(tc.core.string_interner)));
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
// CLOSURE-CAPTURE E1/E2/E3 — what a closure may do to what it
// captured.
//
// A closure that is only called where it is defined shares its
// captures: reads see the current value, writes reach the outer
// binding. One that can outlive them keeps a copy, and a write to a
// copy reaches nothing — that write is E0021. Before the rule
// existed the four compiled engines discarded it silently (a counter
// closure answered 1, 1, 0) and the tree-walker rejected it at run
// time claiming the `var` had been declared `val`.
// ---------------------------------------------------------------

/// The shape that motivated the whole thing: a counter closed over a
/// mutable outer binding, called where it was defined.
#[test]
fn a_counter_closure_type_checks() {
    parse_and_type_check(
        "fn main() -> u64 {
            var count: u64 = 0u64
            val bump = fn() -> u64 { count = count + 1u64  count }
            bump()
            bump()
            count
        }",
    )
    .expect("a closure that is only called here shares its captures");
}

/// Handing the closure to someone else puts it where the frame may
/// already have gone, so it keeps a copy and the write is refused.
#[test]
fn assigning_to_a_capture_of_an_escaping_closure_is_rejected() {
    let err = parse_and_type_check(
        "fn run(g: fn () -> u64) -> u64 { g() }
        fn main() -> u64 {
            var count: u64 = 0u64
            val bump = fn() -> u64 { count = count + 1u64  count }
            run(bump)
        }",
    )
    .expect_err("expected a write in an escaping closure to be rejected");
    assert!(
        err.contains(CAPTURED_ASSIGN) && err.contains("count"),
        "expected the captured-assign error naming `count`, got: {err}"
    );
}

/// Returning it is the other way out of the frame. The literal is not
/// even bound to a name here, which is enough on its own.
#[test]
fn assigning_to_a_capture_of_a_returned_closure_is_rejected() {
    let err = parse_and_type_check(
        "fn make() -> fn () -> u64 {
            var count: u64 = 0u64
            fn() -> u64 { count = count + 1u64  count }
        }",
    )
    .expect_err("expected a write in a returned closure to be rejected");
    assert!(
        err.contains(CAPTURED_ASSIGN),
        "expected the captured-assign error for the returned closure, got: {err}"
    );
}

/// A shared capture of a `val` is refused for the ordinary reason,
/// and "use `var`" is now advice that works.
#[test]
fn assigning_to_a_shared_capture_of_a_val_reports_the_immutability() {
    let err = parse_and_type_check(
        "fn main() -> u64 {
            val n: u64 = 1u64
            val f = fn() -> u64 { n = n + 1u64  n }
            f()
        }",
    )
    .expect_err("a `val` is not assignable however it is captured");
    assert!(
        !err.contains(CAPTURED_ASSIGN) && err.contains("immutable"),
        "expected the immutable-binding error, got: {err}"
    );
}

/// In an escaping closure the immutability is beside the point: the
/// write reaches nothing whichever way the binding was declared, so
/// "use `var`" would fix one error into another.
#[test]
fn assigning_to_a_copied_capture_of_a_val_reports_the_capture() {
    let err = parse_and_type_check(
        "fn make() -> fn () -> u64 {
            val n: u64 = 1u64
            fn() -> u64 { n = n + 1u64  n }
        }",
    )
    .expect_err("expected a write in a returned closure to be rejected");
    assert!(
        err.contains(CAPTURED_ASSIGN),
        "expected the captured-assign error rather than the immutable-binding one, got: {err}"
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
        !err.contains(CAPTURED_ASSIGN),
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

/// Reading a capture is legal under either mode.
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

    // The inner closure is declared inside another closure, so it is
    // not a candidate for sharing — the frame it would reach out to
    // is the outer closure's, which runs whenever *that* is called.
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
    .expect_err("a closure nested in a closure keeps its copies");
    assert!(
        err.contains(CAPTURED_ASSIGN),
        "expected the captured-assign error from the inner closure, got: {err}"
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

// ---------------------------------------------------------------
// E2 — writing *through* a capture follows the same rule as writing
// to it. A captured compound kept its cell, so `p.x = ...` reached
// the outer binding on the three interpreter engines while the two
// compiled ones could not build the program at all.
// ---------------------------------------------------------------

#[test]
fn assigning_to_a_field_of_a_copied_captured_struct_is_rejected() {
    let err = parse_and_type_check(
        "struct P { x: i64 }
        fn run(g: fn () -> i64) -> i64 { g() }
        fn main() -> i64 {
            var p = P { x: 1i64 }
            val f = fn() -> i64 { p.x = p.x + 1i64  p.x }
            run(f)
        }",
    )
    .expect_err("expected a write through a copied capture to be rejected");
    assert!(
        err.contains(CAPTURED_ASSIGN) && err.contains("p.x"),
        "expected the captured-assign error naming the whole path, got: {err}"
    );
}

/// The message quotes what was written but the rule is about the root,
/// so a nested path names the binding that is actually captured.
#[test]
fn a_nested_path_names_the_captured_root() {
    let err = parse_and_type_check(
        "struct Inner { v: i64 }
        struct Outer { inner: Inner }
        fn run(g: fn () -> i64) -> i64 { g() }
        fn main() -> i64 {
            var o = Outer { inner: Inner { v: 1i64 } }
            val f = fn() -> i64 { o.inner.v = 7i64  o.inner.v }
            run(f)
        }",
    )
    .expect_err("expected a write through a copied capture to be rejected");
    assert!(
        // The target is the whole path; the captured root is just `o`,
        // which the message names as the binding that can be outlived.
        err.contains("assign to `o.inner.v`") && err.contains("outlive `o`"),
        "expected the path as target and `o` as root, got: {err}"
    );
}

/// `a[i] = v` is its own expression rather than an `Assign` with a
/// slice target, so it needs the rule applied on its own path. It
/// used to reach the backends and die as an internal error.
#[test]
fn assigning_into_a_copied_captured_array_is_rejected() {
    let err = parse_and_type_check(
        "fn run(g: fn () -> i64) -> i64 { g() }
        fn main() -> i64 {
            var a: [i64; 3] = [1i64, 2i64, 3i64]
            val f = fn() -> i64 { a[0] = 9i64  a[0] }
            run(f)
        }",
    )
    .expect_err("expected a write into a copied captured array to be rejected");
    assert!(
        err.contains(CAPTURED_ASSIGN) && err.contains("a[..]"),
        "expected the captured-assign error for the indexed write, got: {err}"
    );
}

/// Reading through a capture is untouched.
#[test]
fn reading_through_a_capture_still_type_checks() {
    parse_and_type_check(
        "struct P { x: i64, y: i64 }
        fn main() -> i64 {
            val p = P { x: 3i64, y: 4i64 }
            val f = fn() -> i64 { p.x + p.y }
            f()
        }",
    )
    .expect("reading a capture's field is legal");
}

/// A compound the closure declares itself is not a capture, so
/// writing through it is ordinary.
#[test]
fn writing_through_a_closure_local_compound_is_allowed() {
    parse_and_type_check(
        "struct P { x: i64 }
        fn main() -> i64 {
            val bump: i64 = 1i64
            val f = fn() -> i64 { var p = P { x: 0i64 }  p.x = p.x + bump  p.x }
            f()
        }",
    )
    .expect("a struct built inside the closure is local to it");
}
