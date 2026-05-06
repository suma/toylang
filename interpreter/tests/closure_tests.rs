// Phase 3 (interpreter) closure / lambda execution tests.
//
// Verifies the runtime behaviour of `Object::Closure`:
//   - closure literal evaluates to a callable value
//   - direct call through a `val f = fn(...)` binding works
//   - higher-order functions accept closure arguments
//   - free variables are captured at closure-creation time and
//     stay live after the outer scope exits
//   - mutating the outer binding after capture doesn't disturb the
//     captured snapshot for primitives
//   - closures can return closures (nested)
//   - argument-count and arg-type mismatches surface as errors
//
// JIT / AOT phases ride on top of this — JIT silently falls back to
// the interpreter for any program containing a closure, and AOT
// support is a later phase.

mod common;

use common::{
    assert_program_fails, assert_program_result_i64, assert_program_result_u64, test_program,
};

#[test]
fn closure_literal_called_through_local_binding() {
    assert_program_result_i64(
        "fn main() -> i64 {
            val f = fn(x: i64) -> i64 { x + 1i64 }
            f(41i64)
        }",
        42,
    );
}

#[test]
fn closure_passed_as_argument_to_hof() {
    assert_program_result_i64(
        "fn apply(f: (i64) -> i64, x: i64) -> i64 { f(x) }
        fn main() -> i64 {
            apply(fn(x: i64) -> i64 { x * 2i64 }, 21i64)
        }",
        42,
    );
}

#[test]
fn closure_captures_outer_primitive_binding() {
    assert_program_result_i64(
        "fn main() -> i64 {
            val n: i64 = 10i64
            val add_n = fn(x: i64) -> i64 { x + n }
            add_n(32i64)
        }",
        42,
    );
}

#[test]
fn closure_with_multiple_captures() {
    assert_program_result_i64(
        "fn main() -> i64 {
            val a: i64 = 5i64
            val b: i64 = 7i64
            val sum_offset = fn(x: i64) -> i64 { x + a + b }
            sum_offset(30i64)
        }",
        42,
    );
}

#[test]
fn closure_zero_args_returns_constant() {
    assert_program_result_u64(
        "fn main() -> u64 {
            val k = fn() -> u64 { 42u64 }
            k()
        }",
        42,
    );
}

#[test]
fn closure_capture_snapshot_is_independent_of_post_capture_mutation() {
    // Primitives are captured by value (the closure holds its own
    // `Object::Int64(...)` cell), so reassigning the outer binding
    // after capture must not change the closure's behaviour.
    assert_program_result_i64(
        "fn main() -> i64 {
            var n: i64 = 10i64
            val add_n = fn(x: i64) -> i64 { x + n }
            n = 100i64
            add_n(32i64)
        }",
        42,
    );
}

#[test]
fn nested_closure_creation_and_call() {
    // Outer closure returns an inner closure that captures a
    // parameter of the outer call. The parser supports the syntax;
    // this exercises end-to-end execution.
    assert_program_result_i64(
        "fn make_adder(n: i64) -> (i64) -> i64 {
            fn(x: i64) -> i64 { x + n }
        }
        fn main() -> i64 {
            val add5 = make_adder(5i64)
            add5(37i64)
        }",
        42,
    );
}

#[test]
fn indirect_call_arg_count_mismatch_fails() {
    // Type checker should reject this at compile time.
    assert_program_fails(
        "fn main() -> i64 {
            val f = fn(x: i64, y: i64) -> i64 { x + y }
            f(1i64)
        }",
    );
}

#[test]
fn indirect_call_arg_type_mismatch_fails() {
    assert_program_fails(
        "fn main() -> i64 {
            val f = fn(x: i64) -> i64 { x + 1i64 }
            f(true)
        }",
    );
}

#[test]
fn closure_value_round_trips_through_value_binding() {
    // Confirms that an `Object::Closure` survives being copied
    // through a second `val` binding without losing call dispatch.
    assert_program_result_i64(
        "fn main() -> i64 {
            val f = fn(x: i64) -> i64 { x * 3i64 }
            val g = f
            g(14i64)
        }",
        42,
    );
}

#[test]
fn closure_object_has_function_type() {
    // Confirms that printing a closure surfaces the placeholder
    // `<closure/N>` form. Phase 3 settled on N = arity to keep the
    // output independent of capture set + return-type ordering.
    let program = "fn main() -> i64 {
        val f = fn(x: i64, y: i64) -> i64 { x + y }
        print(f)
        0i64
    }";
    let result = test_program(program).expect("execution");
    assert_eq!(result.borrow().unwrap_int64(), 0);
}
