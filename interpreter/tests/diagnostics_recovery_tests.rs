// LLM-LOOP P1 — statement-level error recovery.
//
// Before P1 the type checker unwound out of a whole function on the
// first bad statement, so a file with N independent mistakes reported
// exactly one of them and the author had to re-run after every single
// fix. These tests pin the property that matters: one run reports every
// independent problem, in source order, without inventing cascades.
//
// Design notes on what is deliberately *not* asserted:
//   - exact error wording (P2/P3 will rewrite the messages)
//   - errors within a single statement (expression-level recovery is an
//     explicit non-goal; see design-docs/LLM_FEEDBACK_LOOP.md)

mod common;

use common::test_program;

/// Type-check the source and return the joined diagnostics. Panics if
/// the program unexpectedly succeeds.
fn diagnostics(source: &str) -> String {
    match test_program(source) {
        Ok(_) => panic!("expected the program to fail type checking:\n{source}"),
        Err(e) => e,
    }
}

/// Number of separately reported diagnostics. Each formatted error
/// carries one `Error at <file>:<line>:<col>` header.
fn error_count(diags: &str) -> usize {
    diags.matches("Error at").count()
}

#[test]
fn independent_errors_in_one_function_are_all_reported() {
    let diags = diagnostics(
        "fn main() -> u64 {
            val a: bool = 1u64
            val b: bool = 2u64
            val c: bool = 3u64
            0u64
        }",
    );
    assert_eq!(
        error_count(&diags),
        3,
        "each bad binding should be reported once:\n{diags}"
    );
}

#[test]
fn errors_are_reported_across_multiple_functions() {
    let diags = diagnostics(
        "fn first() -> u64 {
            val a: bool = 1u64
            0u64
        }
        fn second() -> u64 {
            val b: bool = 2u64
            0u64
        }
        fn main() -> u64 { 0u64 }",
    );
    assert_eq!(error_count(&diags), 2, "{diags}");
}

#[test]
fn errors_inside_nested_blocks_are_reported() {
    // Recovery has to reach `visit_block`, not just the function's
    // top-level statement loop, or anything inside an `if` / loop body
    // still aborts the enclosing function.
    let diags = diagnostics(
        "fn main() -> u64 {
            val a: bool = 1u64
            if true {
                val b: bool = 2u64
            }
            while false {
                val c: bool = 3u64
            }
            0u64
        }",
    );
    assert_eq!(error_count(&diags), 3, "{diags}");
}

#[test]
fn undefined_function_and_type_mismatch_are_reported_together() {
    let diags = diagnostics(
        "fn main() -> u64 {
            val z = definitely_not_defined(1u64)
            val w: bool = 2u64
            0u64
        }",
    );
    assert_eq!(error_count(&diags), 2, "{diags}");
    assert!(diags.contains("definitely_not_defined"), "{diags}");
    assert!(diags.contains("Bool"), "{diags}");
}

#[test]
fn a_failed_binding_does_not_cascade_through_a_cast() {
    // `as` had the same leak as the binary operators: casting a
    // recovery placeholder reported "Cannot cast Unknown to UInt64",
    // naming an internal type the user never wrote.
    let diags = diagnostics(
        "fn helper(a: i64) -> i64 { a }
        fn main() -> u64 {
            val c = helper(1u64)
            c as u64
        }",
    );
    assert_eq!(error_count(&diags), 1, "{diags}");
    assert!(!diags.contains("Unknown"), "internal poison type leaked:\n{diags}");
}

#[test]
fn a_failed_binding_does_not_cascade_into_its_uses() {
    // `z` never got a type, but the user did declare it. Reporting
    // "variable not found" (or a mismatch against the internal
    // `Unknown` placeholder) at every later mention would bury the one
    // real error under noise about a name that is not the problem.
    let diags = diagnostics(
        "fn main() -> u64 {
            val z = definitely_not_defined(1u64)
            val w = z + 1u64
            val v = z * 2u64
            println(z)
            0u64
        }",
    );
    assert_eq!(
        error_count(&diags),
        1,
        "only the undefined call is a real error:\n{diags}"
    );
    assert!(!diags.contains("Unknown"), "internal poison type leaked:\n{diags}");
}

#[test]
fn a_failed_body_does_not_also_report_a_return_type_mismatch() {
    // With the body broken, the inferred body type is a placeholder, so
    // any return-type complaint on top of it is a cascade.
    let diags = diagnostics(
        "fn main() -> u64 {
            val a: bool = 1u64
            a
        }",
    );
    assert_eq!(error_count(&diags), 1, "{diags}");
    assert!(!diags.contains("return type"), "{diags}");
}

#[test]
fn diagnostics_are_ordered_by_source_position() {
    let diags = diagnostics(
        "fn main() -> u64 {
            val a: bool = 1u64
            val b: bool = 2u64
            0u64
        }",
    );
    let first = diags
        .find("test.t:2:")
        .unwrap_or_else(|| panic!("no line 2 diagnostic:\n{diags}"));
    let second = diags
        .find("test.t:3:")
        .unwrap_or_else(|| panic!("no line 3 diagnostic:\n{diags}"));
    assert!(first < second, "diagnostics out of source order:\n{diags}");
}

#[test]
fn binding_diagnostics_point_at_the_declaration_line() {
    // The parser used to record a `val` / `var` location *after* parsing
    // the right-hand side, which left it on the following statement.
    // That is invisible while errors carry no location at all, and
    // actively misleading once they do.
    let diags = diagnostics(
        "fn main() -> u64 {
            val w: bool = 1u64
            0u64
        }",
    );
    assert!(
        diags.contains("test.t:2:"),
        "expected the diagnostic on line 2 (the `val`), got:\n{diags}"
    );
    assert!(
        !diags.contains("test.t:3:"),
        "diagnostic pointed past the declaration:\n{diags}"
    );
}

#[test]
fn var_declarations_report_at_their_own_line() {
    // `var` gets its location from the same parser path as `val`.
    // (An annotation mismatch can't be used here: `var w: bool = 1u64`
    // currently type checks, unlike the `val` form. That gap is
    // unrelated to recovery and is tracked separately.)
    let diags = diagnostics(
        "fn main() -> u64 {
            var w = definitely_not_defined(1u64)
            0u64
        }",
    );
    assert!(diags.contains("test.t:2:"), "{diags}");
    assert!(!diags.contains("test.t:3:"), "{diags}");
}

#[test]
fn a_correct_program_still_type_checks() {
    // Recovery must not turn genuine successes into reported errors.
    common::assert_program_result_u64(
        "fn add(a: u64, b: u64) -> u64 { a + b }
        fn main() -> u64 {
            var total = 0u64
            for i in 0u64 to 4u64 {
                total = add(total, i)
            }
            total
        }",
        6,
    );
}

// --- syntax that must be rejected rather than mis-parsed -------------
//
// Both of these used to slip through: `parse_program` discarded every
// collected parser error except "reserved keyword" ones, so a bad parse
// produced an `Ok` over a tree that was not what the user wrote. The
// damage then surfaced somewhere unrelated, or not until runtime.

#[test]
fn else_if_is_rejected_and_points_at_elif() {
    let diags = diagnostics(
        "fn classify(n: u64) -> u64 {
            if n > 10u64 { 1u64 } else if n > 5u64 { 2u64 } else { 3u64 }
        }
        fn main() -> u64 { classify(20u64) }",
    );
    assert!(diags.contains("elif"), "the fix should be named:\n{diags}");
    // The old failure mode: the rest of the file was swallowed and the
    // user was told their `main` did not exist.
    assert!(
        !diags.contains("'main' not found"),
        "error still blames an unrelated declaration:\n{diags}"
    );
}

#[test]
fn elif_still_parses() {
    common::assert_program_result_u64(
        "fn classify(n: u64) -> u64 {
            if n > 10u64 { 1u64 } elif n > 5u64 { 2u64 } else { 3u64 }
        }
        fn main() -> u64 { classify(7u64) }",
        2,
    );
}

#[test]
fn a_semicolon_is_a_parse_error_not_a_silent_recovery() {
    // toylang separates statements by newline. A stray `;` used to be
    // collected and dropped, leaving whatever the parser had recovered
    // into.
    let diags = diagnostics(
        "fn main() -> u64 {
            val a = 1u64; val b = 2u64
            a + b
        }",
    );
    assert!(!diags.is_empty());
}

#[test]
fn bare_return_before_a_closing_brace_is_accepted() {
    // The `}` terminating the block is not the start of a return value.
    // Getting this wrong collected a bogus "expected expression", which
    // only became visible once parse errors stopped being discarded.
    // The bare `return` needs a function with no declared return type —
    // in a `-> u64` function it would be a genuine type error.
    common::assert_program_result_u64(
        "fn note(n: u64) {
            if n > 0u64 {
                return
            }
            println(n)
        }
        fn main() -> u64 {
            note(1u64)
            7u64
        }",
        7,
    );
}

// --- `var` gets the same checks as `val` -----------------------------

#[test]
fn var_annotation_mismatch_is_rejected_like_val() {
    // `visit_val_impl` checked the annotation against the initializer;
    // `var` went through a different path that skipped it, so this
    // program type checked and ran while the `val` form was rejected.
    let diags = diagnostics(
        "fn main() -> u64 {
            var w: bool = 1u64
            0u64
        }",
    );
    assert_eq!(error_count(&diags), 1, "{diags}");
    assert!(diags.contains("Bool"), "{diags}");
}

#[test]
fn var_annotation_mismatch_suggests_the_same_cast_as_val() {
    let diags = diagnostics(
        "fn main() -> u64 {
            var d: f64 = 5u64
            0u64
        }",
    );
    assert!(
        diags.contains("5u64 as f64"),
        "expected the same cast suggestion `val` gets:\n{diags}"
    );
}

#[test]
fn var_with_a_compatible_annotation_still_works() {
    common::assert_program_result_u64(
        "fn main() -> u64 {
            var n: u64 = 41u64
            n = n + 1u64
            n
        }",
        42,
    );
}

#[test]
fn a_user_type_nested_in_a_tuple_annotation_is_accepted() {
    // `(Point, u64)` reaches the checker as `Tuple([Identifier(Point),
    // U64])` while the value is `Tuple([Struct(Point, []), U64])`. Only
    // the top-level pair used to be reconciled, so the annotation was
    // rejected outright under `val` — and silently unchecked under
    // `var`.
    common::assert_program_result_u64(
        "struct Point { x: u64, y: u64 }
        fn main() -> u64 {
            val t: (Point, u64) = (Point { x: 1u64, y: 2u64 }, 40u64)
            t.0.y + t.1
        }",
        42,
    );
}

#[test]
fn assigning_a_wrong_type_into_an_array_element_is_rejected() {
    // The element-type check was guarded on `element_types.len() == 1`,
    // but an array literal carries one entry per element — so the check
    // was skipped for every array longer than one.
    let diags = diagnostics(
        "fn main() -> u64 {
            var arr = [1u64, 2u64, 3u64]
            arr[0u64] = \"text\"
            0u64
        }",
    );
    assert!(diags.contains("Type mismatch"), "{diags}");
}
