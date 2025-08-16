use crate::type_decl::TypeDecl;

#[derive(Debug, Clone, PartialEq)]
pub struct SourceLocation {
    pub line: u32,
    pub column: u32,
    pub offset: u32,
}

#[derive(Debug)]
pub struct MultipleTypeCheckResult<T> {
    pub result: Option<T>,
    pub errors: Vec<TypeCheckError>,
}

impl<T> MultipleTypeCheckResult<T> {
    pub fn success(value: T) -> Self {
        Self {
            result: Some(value),
            errors: Vec::new(),
        }
    }

    pub fn failure(errors: Vec<TypeCheckError>) -> Self {
        Self {
            result: None,
            errors,
        }
    }

    pub fn with_errors(value: T, errors: Vec<TypeCheckError>) -> Self {
        Self {
            result: Some(value),
            errors,
        }
    }

    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

#[derive(Debug, Clone)]
pub enum TypeCheckErrorKind {
    TypeMismatch { expected: TypeDecl, actual: TypeDecl },
    TypeMismatchOperation(Box<TypeMismatchOperationError>),
    NotFound { item_type: String, name: String },
    UnsupportedOperation { operation: String, type_name: TypeDecl },
    ConversionError { from: String, to: String },
    ArrayError { message: String },
    MethodError(Box<MethodErrorData>),
    InvalidLiteral { value: String, expected_type: String },
    AccessDenied { message: String },
    GenericError { message: String },
}

#[derive(Debug, Clone)]
pub struct TypeMismatchOperationError {
    pub operation: String,
    pub left: TypeDecl,
    pub right: TypeDecl,
}

#[derive(Debug, Clone)]
pub struct MethodErrorData {
    pub method: String,
    pub type_name: TypeDecl,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct TypeCheckError {
    pub kind: TypeCheckErrorKind,
    pub context: Option<String>,
    pub location: Option<SourceLocation>,
}

impl TypeCheckError {
    pub fn type_mismatch(expected: TypeDecl, actual: TypeDecl) -> Self {
        Self {
            kind: TypeCheckErrorKind::TypeMismatch { expected, actual },
            context: None,
            location: None,
        }
    }

    pub fn type_mismatch_operation(operation: &str, left: TypeDecl, right: TypeDecl) -> Self {
        Self {
            kind: TypeCheckErrorKind::TypeMismatchOperation(Box::new(TypeMismatchOperationError {
                operation: operation.to_string(),
                left,
                right,
            })),
            context: None,
            location: None,
        }
    }

    pub fn not_found(item_type: &str, name: &str) -> Self {
        Self {
            kind: TypeCheckErrorKind::NotFound {
                item_type: item_type.to_string(),
                name: name.to_string(),
            },
            context: None,
            location: None,
        }
    }

    pub fn unsupported_operation(operation: &str, type_name: TypeDecl) -> Self {
        Self {
            kind: TypeCheckErrorKind::UnsupportedOperation {
                operation: operation.to_string(),
                type_name,
            },
            context: None,
            location: None,
        }
    }

    pub fn conversion_error(from: &str, to: &str) -> Self {
        Self {
            kind: TypeCheckErrorKind::ConversionError {
                from: from.to_string(),
                to: to.to_string(),
            },
            context: None,
            location: None,
        }
    }

    pub fn array_error(message: &str) -> Self {
        Self {
            kind: TypeCheckErrorKind::ArrayError {
                message: message.to_string(),
            },
            context: None,
            location: None,
        }
    }

    pub fn method_error(method: &str, type_name: TypeDecl, reason: &str) -> Self {
        Self {
            kind: TypeCheckErrorKind::MethodError(Box::new(MethodErrorData {
                method: method.to_string(),
                type_name,
                reason: reason.to_string(),
            })),
            context: None,
            location: None,
        }
    }

    pub fn invalid_literal(value: &str, expected_type: &str) -> Self {
        Self {
            kind: TypeCheckErrorKind::InvalidLiteral {
                value: value.to_string(),
                expected_type: expected_type.to_string(),
            },
            context: None,
            location: None,
        }
    }

    pub fn access_denied(message: &str) -> Self {
        Self {
            kind: TypeCheckErrorKind::AccessDenied {
                message: message.to_string(),
            },
            context: None,
            location: None,
        }
    }

    pub fn generic_error(message: &str) -> Self {
        Self {
            kind: TypeCheckErrorKind::GenericError {
                message: message.to_string(),
            },
            context: None,
            location: None,
        }
    }

    pub fn with_context(mut self, context: &str) -> Self {
        self.context = Some(context.to_string());
        self
    }

    pub fn with_location(mut self, location: SourceLocation) -> Self {
        self.location = Some(location);
        self
    }

    pub fn new(msg: String) -> Self {
        Self::generic_error(&msg)
    }
}

impl std::fmt::Display for TypeCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let base_message = match &self.kind {
            TypeCheckErrorKind::TypeMismatch { expected, actual } => {
                format!("Type mismatch: expected {:?}, but got {:?}", expected, actual)
            }
            TypeCheckErrorKind::TypeMismatchOperation(data) => {
                format!("Type mismatch in {} operation: incompatible types {:?} and {:?}", data.operation, data.left, data.right)
            }
            TypeCheckErrorKind::NotFound { item_type, name } => {
                format!("{} '{}' not found", item_type, name)
            }
            TypeCheckErrorKind::UnsupportedOperation { operation, type_name } => {
                format!("Unsupported operation '{}' for type {:?}", operation, type_name)
            }
            TypeCheckErrorKind::ConversionError { from, to } => {
                format!("Cannot convert '{}' to {}", from, to)
            }
            TypeCheckErrorKind::ArrayError { message } => {
                format!("Array error: {}", message)
            }
            TypeCheckErrorKind::MethodError(data) => {
                format!("Method '{}' error for type {:?}: {}", data.method, data.type_name, data.reason)
            }
            TypeCheckErrorKind::InvalidLiteral { value, expected_type } => {
                format!("Invalid {} literal: '{}'", expected_type, value)
            }
            TypeCheckErrorKind::AccessDenied { message } => {
                format!("Access denied: {}", message)
            }
            TypeCheckErrorKind::GenericError { message } => {
                message.clone()
            }
        };

        let mut result = base_message;

        if let Some(location) = &self.location {
            result = format!("{}:{}:{}: {}", location.line, location.column, location.offset, result);
        }

        if let Some(context) = &self.context {
            result = format!("{} (in {})", result, context);
        }

        write!(f, "{}", result)
    }
}