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
    ContractViolation {
        kind: &'static str,
        function: String,
        clause_index: usize,
    },
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
            InterpreterError::ContractViolation { kind, function, clause_index } => {
                write!(f, "Contract violation: `{kind}` clause #{idx} of function `{function}` evaluated to false",
                       idx = clause_index + 1)
            }
        }
    }
}