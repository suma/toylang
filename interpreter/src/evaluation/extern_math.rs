// Built-in implementations of math `extern fn` declarations.
//
// Phase 2 of the math externalisation work: instead of dispatching
// math intrinsics through the frontend's `BuiltinFunction` enum, the
// interpreter registers Rust closures keyed by extern fn name. When
// the call evaluator hits an `is_extern: true` function, it looks up
// the closure in this registry and invokes it.
//
// Future phases will rewrite `interpreter/modules/math/math.t` to
// declare these as `extern fn` and remove the matching variants from
// `BuiltinFunction`.

use std::collections::HashMap;

use crate::error::InterpreterError;
use crate::value::Value;

/// Native implementation backing an `extern fn` declaration.
/// Receives pre-evaluated argument values; returns the result value
/// or an `InterpreterError` if the arguments don't type-check at the
/// boundary (the type-checker has already validated the static
/// signature, so this only fires for genuine internal bugs).
pub type ExternFn = fn(&[Value]) -> Result<Value, InterpreterError>;

/// Build the registry of extern fn implementations available at
/// interpreter startup. Keyed by the `extern fn` declaration's name
/// as written in the source program.
pub fn build_default_registry() -> HashMap<&'static str, ExternFn> {
    let mut m: HashMap<&'static str, ExternFn> = HashMap::new();

    // Canonical names used by the eventual stdlib `math.t` rewrite.
    m.insert("__extern_sin_f64", extern_sin_f64);
    m.insert("__extern_cos_f64", extern_cos_f64);
    m.insert("__extern_tan_f64", extern_tan_f64);
    m.insert("__extern_log_f64", extern_log_f64);
    m.insert("__extern_log2_f64", extern_log2_f64);
    m.insert("__extern_exp_f64", extern_exp_f64);
    m.insert("__extern_floor_f64", extern_floor_f64);
    m.insert("__extern_ceil_f64", extern_ceil_f64);
    m.insert("__extern_sqrt_f64", extern_sqrt_f64);
    m.insert("__extern_abs_f64", extern_abs_f64);
    m.insert("__extern_pow_f64", extern_pow_f64);
    // i64 wrapping_abs — used by the prelude's `impl Abs for i64`.
    // `i64::MIN` stays at `i64::MIN` (matches the legacy
    // `BuiltinMethod::I64Abs` semantics that the prelude replaces).
    m.insert("__extern_abs_i64", extern_abs_i64);

    // Test-only aliases used by Phase 1/2 regression tests so we can
    // exercise the extern dispatch without rewriting math.t yet.
    m.insert("extern_sin", extern_sin_f64);
    m.insert("extern_cos", extern_cos_f64);
    // Test-only generic-extern probe (#195): identity over T. Used by
    // tests that verify `extern fn name<T>(x: T) -> T` parses and the
    // interpreter dispatches the call via type-erased registry lookup.
    // The implementation just hands back the argument value, so it
    // works for every T the type-checker accepts.
    m.insert("__extern_test_identity", extern_test_identity);

    m
}

/// Type-erased identity used by #195 tests. The runtime sees only
/// `&[Value]`, so genuinely polymorphic dispatch costs nothing here
/// — the user's `extern fn id<T>(x: T) -> T` resolves by literal
/// name and this closure forwards the value unchanged.
fn extern_test_identity(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `__extern_test_identity` takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    Ok(args[0].clone())
}

fn unary_f64(name: &str, args: &[Value], op: fn(f64) -> f64) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: format!("extern fn `{name}` takes 1 argument"),
            expected: 1,
            found: args.len(),
        });
    }
    let x = match &args[0] {
        Value::Float64(v) => *v,
        other => {
            return Err(InterpreterError::InternalError(format!(
                "extern fn `{name}` expects an f64 argument, got {other:?}"
            )))
        }
    };
    Ok(Value::Float64(op(x)))
}

fn extern_sin_f64(args: &[Value]) -> Result<Value, InterpreterError> { unary_f64("sin", args, f64::sin) }
fn extern_cos_f64(args: &[Value]) -> Result<Value, InterpreterError> { unary_f64("cos", args, f64::cos) }
fn extern_tan_f64(args: &[Value]) -> Result<Value, InterpreterError> { unary_f64("tan", args, f64::tan) }
fn extern_log_f64(args: &[Value]) -> Result<Value, InterpreterError> { unary_f64("log", args, f64::ln) }
fn extern_log2_f64(args: &[Value]) -> Result<Value, InterpreterError> { unary_f64("log2", args, f64::log2) }
fn extern_exp_f64(args: &[Value]) -> Result<Value, InterpreterError> { unary_f64("exp", args, f64::exp) }
fn extern_floor_f64(args: &[Value]) -> Result<Value, InterpreterError> { unary_f64("floor", args, f64::floor) }
fn extern_ceil_f64(args: &[Value]) -> Result<Value, InterpreterError> { unary_f64("ceil", args, f64::ceil) }
fn extern_sqrt_f64(args: &[Value]) -> Result<Value, InterpreterError> { unary_f64("sqrt", args, f64::sqrt) }
fn extern_abs_f64(args: &[Value]) -> Result<Value, InterpreterError> { unary_f64("abs", args, f64::abs) }

fn extern_abs_i64(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 1 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `abs` (i64) takes 1 argument".to_string(),
            expected: 1,
            found: args.len(),
        });
    }
    match &args[0] {
        Value::Int64(v) => Ok(Value::Int64(v.wrapping_abs())),
        other => Err(InterpreterError::InternalError(format!(
            "extern fn `__extern_abs_i64` expects an i64 argument, got {other:?}"
        ))),
    }
}

fn extern_pow_f64(args: &[Value]) -> Result<Value, InterpreterError> {
    if args.len() != 2 {
        return Err(InterpreterError::FunctionParameterMismatch {
            message: "extern fn `pow` takes 2 arguments".to_string(),
            expected: 2,
            found: args.len(),
        });
    }
    let b = match &args[0] {
        Value::Float64(v) => *v,
        other => return Err(InterpreterError::InternalError(format!(
            "extern fn `pow` expects f64 base, got {other:?}"
        ))),
    };
    let e = match &args[1] {
        Value::Float64(v) => *v,
        other => return Err(InterpreterError::InternalError(format!(
            "extern fn `pow` expects f64 exponent, got {other:?}"
        ))),
    };
    Ok(Value::Float64(b.powf(e)))
}
