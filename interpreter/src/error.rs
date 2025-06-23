use frontend::type_decl::TypeDecl;
use crate::evaluation::EvaluationResult;
use crate::object::ObjectError;

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
}