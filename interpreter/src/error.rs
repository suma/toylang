use frontend::type_checker::SourceLocation;
use frontend::type_decl::TypeDecl;
use crate::object::ObjectError;
use std::fmt;

#[derive(Debug)]
pub enum InterpreterError {
    TypeError { expected: TypeDecl, found: TypeDecl, message: String },
    UndefinedVariable(String),
    ImmutableAssignment(String),
    FunctionNotFound(String),
    FunctionParameterMismatch { message: String, expected: usize, found: usize },
    InternalError(String),
    ObjectError(ObjectError),
    IndexOutOfBounds { index: isize, size: usize },
    /// A `requires` or `ensures` clause evaluated to false at runtime.
    /// `kind` is `"requires"` or `"ensures"`; `function` is the human-readable
    /// function name; `clause_index` identifies which clause (0-based) failed
    /// when multiple are declared. The original predicate text isn't kept,
    /// so the diagnostic refers to the clause by position.
    /// LLM-LOOP P6: `bindings` carries the values the predicate saw —
    /// the parameters, plus `result` for an `ensures`. Without them the
    /// report says which clause failed but not why, so reproducing the
    /// failure meant instrumenting the call and running again.
    /// Boxed because it is the widest variant by some way, and
    /// `InterpreterError` is the `Err` of nearly every function in the
    /// crate: DEBUG-OBS D5 added a backtrace and a position to it, and
    /// the whole enum grew with it.
    ContractViolation(Box<ContractViolation>),
    /// Explicit user-triggered abort via the `panic("msg")` builtin.
    /// The message is exactly what the user passed.
    ///
    /// LLM-LOOP P6: `location` is where the `panic` / failed `assert`
    /// sits, and `backtrace` is the chain of toylang calls that reached
    /// it, innermost first. Before this the diagnostic was the message
    /// and nothing else — `panic: boom` gave no way to tell which of
    /// several call paths had fired without adding prints and re-running.
    Panic {
        message: String,
        location: Option<SourceLocation>,
        backtrace: Vec<CallFrame>,
    },
    /// The run exhausted a caller-imposed budget on loop iterations
    /// (CHECK-NONTERMINATION).
    ///
    /// Only `--check` sets a budget: a property trial feeds generated
    /// inputs to a function that was never written to accept them, so
    /// `while i <= n` with `n = u64::MAX` is an ordinary outcome of
    /// sampling rather than a bug. Ordinary execution leaves the
    /// budget unset, so a program the user runs is never cut short.
    StepBudgetExceeded { steps: u64 },
}

/// One toylang-level call, for panic backtraces.
#[derive(Debug, Clone)]
pub struct CallFrame {
    /// Name of the function or method being entered.
    pub function: String,
    /// Where it was called from, when a location was recorded.
    pub call_site: Option<SourceLocation>,
}

/// A `requires` / `ensures` clause that was false at run time.
#[derive(Debug)]
pub struct ContractViolation {
    pub kind: &'static str,
    pub function: String,
    pub clause_index: usize,
    pub bindings: Vec<(String, String)>,
    /// ALLOC-CONTRACT-SUGAR: what the clause was actually about, when
    /// the clause is one the compiler wrote. A budget violation
    /// reports the amount and the allowance ("retained 128 bytes,
    /// budget 0 bytes") — the numbers a plain bool predicate cannot
    /// give a reader.
    pub detail: Option<String>,
    /// DEBUG-OBS D5: how the call that broke the contract was reached.
    /// Rendered by the same code a `Panic` uses — a reader asking
    /// "which call passed the bad argument" is asking the same
    /// question either way, and until this it was the one runtime
    /// failure that answered it for panics only.
    pub backtrace: Vec<CallFrame>,
    /// Where the failing clause is.
    pub location: Option<SourceLocation>,
}

impl fmt::Display for InterpreterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InterpreterError::TypeError { expected, found, message } => {
                write!(f, "Type error: expected {expected:?}, found {found:?}. {message}")
            }
            InterpreterError::UndefinedVariable(name) => {
                write!(f, "Undefined variable: {name}")
            }
            InterpreterError::ImmutableAssignment(name) => {
                write!(f, "Cannot assign to immutable variable: {name}")
            }
            InterpreterError::FunctionNotFound(name) => {
                write!(f, "Function not found: {name}")
            }
            InterpreterError::FunctionParameterMismatch { message, expected, found } => {
                write!(f, "Function parameter mismatch: {message}. Expected {expected} parameters, found {found}")
            }
            InterpreterError::InternalError(message) => {
                write!(f, "Internal error: {message}")
            }
            InterpreterError::ObjectError(err) => {
                write!(f, "Object error: {err:?}")
            }
            InterpreterError::IndexOutOfBounds { index, size } => {
                write!(f, "Array index {index} out of bounds for array of size {size}")
            }
            InterpreterError::ContractViolation(v) => {
                let ContractViolation { kind, function, clause_index, bindings, detail, .. } = &**v;
                match detail {
                    Some(detail) => write!(
                        f,
                        "Contract violation: `{kind}` clause #{idx} of function `{function}`: {detail}",
                        idx = clause_index + 1
                    )?,
                    None => write!(f, "Contract violation: `{kind}` clause #{idx} of function `{function}` evaluated to false",
                       idx = clause_index + 1)?,
                }
                // LLM-LOOP P6: the values the predicate saw. Which
                // clause failed is only half the answer; this is the
                // other half, and it is the half that says what to fix.
                if !bindings.is_empty() {
                    let rendered = bindings
                        .iter()
                        .map(|(name, value)| format!("{name} = {value}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    write!(f, " (with {rendered})")?;
                }
                Ok(())
            }
            InterpreterError::Panic { message, .. } => {
                write!(f, "panic: {message}")
            }
            InterpreterError::StepBudgetExceeded { steps } => {
                write!(f, "step budget exceeded ({steps} loop iterations)")
            }
        }
    }
}