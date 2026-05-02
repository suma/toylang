mod common;
use common::test_program;

// ============================================================================
// Module system tests
// ============================================================================

#[test]
fn test_module_package_declaration() {
    let source = r"
        package math

        fn main() -> u64 {
            42u64
        }
        ";

    let result = test_program(source);
    assert!(result.is_ok(), "Program with package declaration should run");
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 42);
}

#[test]
fn test_module_import_declaration() {
    let source = r"
        import math

        fn main() -> u64 {
            42u64
        }
        ";

    let result = test_program(source);
    assert!(result.is_ok(), "Program with import declaration should run");
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 42);
}

#[test]
fn test_module_bare_call_rejected() {
    // Namespace-only enforcement: imported `pub fn`s must be called
    // via the qualified `module::func(args)` form. A bare `func(args)`
    // call into an imported function is rejected at type-check time
    // so users always spell out where the function lives.
    let source = r"
        import math

        fn main() -> u64 {
            add(1u64, 2u64)
        }
        ";

    let result = test_program(source);
    assert!(result.is_err(), "Bare call into imported fn should be rejected");
    let err_text = format!("{:?}", result.err().unwrap());
    assert!(
        err_text.contains("must be called with the qualified form"),
        "diagnostic should mention qualified form, got: {}",
        err_text
    );
}

#[test]
fn test_extern_fn_declaration_type_checks() {
    // Phase 1 of the math externalisation work: `extern fn`
    // declarations parse + type-check (signature only — no body).
    // Calling them is still a runtime error because the
    // backend dispatch doesn't exist yet (lands in Phase 2).
    let source = r"
        extern fn extern_sin(x: f64) -> f64

        fn main() -> u64 {
            42u64
        }
        ";
    let result = test_program(source);
    assert!(result.is_ok(), "extern fn declaration should parse + type-check: {:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 42);
}

#[test]
fn test_extern_fn_call_errors_cleanly() {
    // Calling an extern fn before Phase 2 dispatch lands surfaces
    // a targeted "not yet implemented" runtime error rather than
    // returning Unit / panicking.
    let source = r"
        extern fn extern_cos(x: f64) -> f64

        fn main() -> u64 {
            val r: f64 = extern_cos(0f64)
            r as u64
        }
        ";
    let result = test_program(source);
    assert!(result.is_err(), "extern fn call should error: {:?}", result.ok());
    let err = format!("{:?}", result.err().unwrap());
    assert!(
        err.contains("extern fn") && err.contains("not yet implemented"),
        "diagnostic should mention extern fn + not-implemented, got: {}",
        err
    );
}

#[test]
fn test_value_method_i64_abs() {
    // `x.abs()` should call the built-in `i64.abs()` method and
    // return `wrapping_abs(x)` semantics — `i64::MIN` stays at
    // `i64::MIN` instead of panicking.
    let source = r"
        fn main() -> u64 {
            val n: i64 = -42i64
            n.abs() as u64
        }
        ";
    let result = test_program(source);
    assert!(result.is_ok(), "x.abs() should run: {:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 42);
}

#[test]
fn test_value_method_f64_abs() {
    // `x.abs()` on an f64 should call the IEEE 754 fabs (sign-bit
    // flip; preserves NaN). C's `fabs` semantics.
    let source = r"
        fn main() -> u64 {
            val x: f64 = -7.5f64
            (x.abs() * 2f64) as u64
        }
        ";
    let result = test_program(source);
    assert!(result.is_ok(), "f64.abs() should run: {:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 15);
}

#[test]
fn test_builtin_abs_polymorphic_f64() {
    // `__builtin_abs(x)` is polymorphic: i64 -> wrapping_abs,
    // f64 -> IEEE 754 fabs. Mirrors C's `abs` / `fabs` distinction
    // in a single user-facing intrinsic.
    let source = r"
        fn main() -> u64 {
            val x: f64 = -3.5f64
            (__builtin_abs(x) * 2f64) as u64
        }
        ";
    let result = test_program(source);
    assert!(result.is_ok(), "__builtin_abs(f64) should run: {:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 7);
}

#[test]
fn test_value_method_f64_sqrt() {
    // `x.sqrt()` should call the built-in `f64.sqrt()` method
    // (IEEE 754) and return the principal root.
    let source = r"
        fn main() -> u64 {
            val r: f64 = 81f64
            r.sqrt() as u64
        }
        ";
    let result = test_program(source);
    assert!(result.is_ok(), "x.sqrt() should run: {:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 9);
}

#[test]
fn test_module_qualified_call_executes() {
    // Regression test for the module integration fix
    // (`update_with_remapped_content` used to leave imported function
    // bodies as `Stmt::Break` placeholders). With the fix in place,
    // calling an imported `pub fn` via the qualified `module::func`
    // form must execute the real body and return the right value.
    let source = r"
        import math

        fn main() -> u64 {
            math::add(10u64, 20u64)
        }
        ";

    let result = test_program(source);
    assert!(
        result.is_ok(),
        "Qualified module call should execute: {:?}",
        result.err()
    );
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 30);
}

#[test]
fn test_module_package_and_import() {
    let source = r"
        package main
        import math

        fn main() -> u64 {
            42u64
        }
        ";

    let result = test_program(source);
    assert!(result.is_ok(), "Program with package and import should run");
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 42);
}

// ============================================================================
// Property-based tests (arithmetic, comparison, logical)
// ============================================================================

#[test]
fn test_arithmetic_properties_extended() {
    // Test arithmetic properties with different values
    let test_cases = vec![
        (10i64, 20i64, "+", 30i64),
        (100i64, 50i64, "-", 50i64),
        (7i64, 8i64, "*", 56i64),
        (21i64, 3i64, "/", 7i64),
    ];

    for (a, b, op, expected) in test_cases {
        let program = format!(r"
        fn main() -> i64 {{
            {}i64 {} {}i64
        }}
        ", a, op, b);

        let res = test_program(&program);
        assert!(res.is_ok(), "Failed for {} {} {}", a, op, b);
        assert_eq!(res.unwrap().borrow().unwrap_int64(), expected);
    }
}

#[test]
fn test_comparison_properties_extended() {
    // Test comparison properties
    let test_cases = vec![
        (10i64, 20i64, "<", true),
        (20i64, 10i64, ">", true),
        (15i64, 15i64, "==", true),
        (10i64, 20i64, "!=", true),
        (25i64, 20i64, ">=", true),
        (15i64, 20i64, "<=", true),
    ];

    for (a, b, op, expected) in test_cases {
        let program = format!(r"
        fn main() -> bool {{
            {}i64 {} {}i64
        }}
        ", a, op, b);

        let res = test_program(&program);
        assert!(res.is_ok(), "Failed for {} {} {}", a, op, b);
        assert_eq!(res.unwrap().borrow().unwrap_bool(), expected);
    }
}

#[test]
fn test_logical_operations() {
    let test_cases = vec![
        (true, "&&", true, true),
        (true, "&&", false, false),
        (false, "&&", true, false),
        (false, "&&", false, false),
        (true, "||", true, true),
        (true, "||", false, true),
        (false, "||", true, true),
        (false, "||", false, false),
    ];

    for (a, op, b, expected) in test_cases {
        let program = format!(r"
        fn main() -> bool {{
            {} {} {}
        }}
        ", a, op, b);

        let res = test_program(&program);
        assert!(res.is_ok(), "Failed for {} {} {}", a, op, b);
        assert_eq!(res.unwrap().borrow().unwrap_bool(), expected);
    }
}
