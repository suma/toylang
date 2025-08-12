use frontend::type_decl::TypeDecl;
use crate::evaluation::EvaluationResult;
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
    PropagateFlow(EvaluationResult),
    ObjectError(ObjectError),
    IndexOutOfBounds { index: isize, size: usize },
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
            InterpreterError::PropagateFlow(result) => {
                write!(f, "Propagate flow: {result:?}")
            }
            InterpreterError::ObjectError(err) => {
                write!(f, "Object error: {err:?}")
            }
            InterpreterError::IndexOutOfBounds { index, size } => {
                write!(f, "Array index {index} out of bounds for array of size {size}")
            }
        }
    }
}