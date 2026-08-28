use crate::source_map::FileId;
use crate::type_decl::TypeDecl;

/// A position in a source file, with the extent of what it covers.
///
/// `#[non_exhaustive]` on purpose: `end_offset` was added in LLM-LOOP
/// P2 and broke every struct-literal construction outside this crate.
/// Build one with [`SourceLocation::new`] or [`SourceLocation::point`]
/// so the next such addition costs one edit rather than a sweep.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct SourceLocation {
    /// Which file the position is in (DEBUG-OBS D2).
    ///
    /// [`FileId::ENTRY`] unless something re-anchored it, which is why
    /// every existing producer stayed correct when this was added:
    /// only `module_integration` copies positions across a file
    /// boundary, and it now says so.
    pub file: FileId,
    pub line: u32,
    pub column: u32,
    pub offset: u32,
    /// Byte offset one past the end of the span this location covers.
    ///
    /// LLM-LOOP P2: a location used to be a single point, so a
    /// diagnostic could say *where* it started but not *how much* it
    /// covered. The formatter compensated by guessing -- it scanned the
    /// source line for an identifier lifted out of the error message --
    /// which worked only for messages that happened to quote a name and
    /// silently pointed at the wrong thing otherwise. With an end offset
    /// the caret is derived, not guessed.
    ///
    /// Always `>= offset`; equal when the extent is unknown, in which
    /// case the formatter falls back to a one-column caret.
    pub end_offset: u32,
}

impl SourceLocation {
    /// A position in the file the user ran. The overwhelming majority
    /// of producers want this; a parser does not know, and should not
    /// need to know, that its output will be integrated into someone
    /// else's program.
    pub fn new(line: u32, column: u32, offset: u32, end_offset: u32) -> Self {
        Self { file: FileId::ENTRY, line, column, offset, end_offset }
    }

    /// A position in a named file.
    pub fn new_in(file: FileId, line: u32, column: u32, offset: u32, end_offset: u32) -> Self {
        Self { file, line, column, offset, end_offset }
    }

    /// The same position, said to belong to `file`.
    ///
    /// What integration applies to every location it copies out of a
    /// module: the line and column are already right *for that
    /// module's text*, and this is the missing half.
    pub fn in_file(self, file: FileId) -> Self {
        Self { file, ..self }
    }

    /// A location with no known extent -- the formatter falls back to a
    /// one-column caret. Use only where the producer genuinely cannot
    /// say how far the construct reaches.
    pub fn point(line: u32, column: u32, offset: u32) -> Self {
        Self { file: FileId::ENTRY, line, column, offset, end_offset: offset }
    }

    /// Same span, re-anchored to a recomputed line/column. The driver
    /// recalculates these from `offset` against the real source.
    pub fn with_line_col(self, line: u32, column: u32) -> Self {
        Self { line, column, ..self }
    }

    /// Width of the span in bytes, at least 1 so a caret is always drawn.
    pub fn width(&self) -> usize {
        (self.end_offset.saturating_sub(self.offset)).max(1) as usize
    }
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
    /// LLM-LOOP P7: the answer to a `val x: _ = expr` type hole.
    ///
    /// Reported as an error rather than a note because a hole is a
    /// question the author asked, not code they meant to keep — letting
    /// it compile would leave the query silently in the program. It gets
    /// its own kind (and code) so `--diagnostics=json` consumers can
    /// tell "here is the type you asked for" apart from "your program is
    /// wrong", which are opposite signals.
    TypeHole { name: String, inferred: String },
    /// RECURSIVE-TYPES: a struct / enum that contains itself with no
    /// indirection. `path` is the chain of members that closes the
    /// cycle, e.g. ``` `A.b: B` -> `B.a: A` ```.
    RecursiveType { name: String, path: String },
    /// BOX-T: a binding read after its value was handed to something
    /// that outlives it, or a transfer this pass will not model.
    UseAfterMove { name: String, moved_at_line: u32 },
    ConditionalMove { name: String },
    /// TYPECHECK-LIES: a literal the grammar accepts but no backend
    /// implements. `null` is the only one — it used to type-check as
    /// "whatever the context wants" and then stop the program when
    /// evaluated.
    ReservedLiteral { name: String, alternative: String },
    /// NEVER-ALLOCATES: a function declared `never_allocates` can
    /// reach the allocator, or reaches a call this check cannot
    /// follow. `path` is the chain that gets there.
    NeverAllocates { function: String, path: String, opaque: Option<&'static str> },
    /// COMPILE-TIME-EVAL: a function declared `const fn` reaches
    /// something the compiler cannot run while compiling. `what` names
    /// it and `path` is the chain that gets there; `opaque` separates
    /// "this cannot run at compile time" from "this pass cannot even
    /// see where the call goes".
    ConstFn { function: String, path: String, what: &'static str, opaque: bool },
    /// COMPILE-TIME-EVAL C3: a value that *must* be known at compile
    /// time could not be worked out. `context` names the position that
    /// forced the evaluation (`const D`), `detail` says what went
    /// wrong (a trap, a contract violation, a spent step budget, a
    /// callee that is not a `const fn`).
    ConstEval { context: String, detail: String },
    /// COMPILE-TIME-EVAL C4: a `requires` / `ensures` clause can do
    /// something other than answer a question, so switching the
    /// contract checks off would change what the program does.
    /// A warning for now (see `contract_purity`).
    ContractPurity { clause: String, path: String, what: &'static str, opaque: bool },
    /// COMPILE-TIME-EVAL C4: a call whose arguments are all constants,
    /// whose precondition the compiler evaluated, and which was false.
    BrokenPrecondition { function: String, detail: String },
    /// CLOSURE-CAPTURE E1: a closure body assigns to a binding that
    /// belongs to an enclosing scope. A capture is a snapshot taken
    /// when the closure was created, so the write reaches nothing —
    /// four of the five engines used to discard it silently and the
    /// tree-walker used to reject it at run time claiming the `var`
    /// had been declared `val`.
    CapturedAssign { name: String },
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
    /// Boxed so `Result<_, TypeCheckError>` -- which is the return type
    /// of essentially every type-checker function -- stays small. The
    /// kind is only read when an error is actually rendered.
    pub kind: Box<TypeCheckErrorKind>,
    pub context: Option<String>,
    pub location: Option<SourceLocation>,
    /// Name of the imported module whose source this error's `location`
    /// refers to, when that is not the file being compiled.
    ///
    /// LLM-LOOP P2: offsets from integrated modules land in the same
    /// pool as the user's own, with nothing to tell them apart. A
    /// diagnostic raised inside `core/std/option.t` would therefore be
    /// rendered against the *user's* file and underline whatever
    /// happened to sit at that offset — a confidently wrong location
    /// pointing at innocent code. Set this and the formatter knows to
    /// name the module instead of quoting a line it cannot trust.
    pub origin_module: Option<String>,
    /// Edits that would resolve this error, produced only where the fix
    /// is certain (LLM-LOOP P3). Empty for almost every diagnostic --
    /// see `crate::diagnostic` for why a wrong suggestion is worse than
    /// none.
    pub suggestions: Vec<crate::diagnostic::Suggestion>,
}

impl TypeCheckError {
    pub fn type_mismatch(expected: TypeDecl, actual: TypeDecl) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::TypeMismatch { expected, actual }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    pub fn type_mismatch_operation(operation: &str, left: TypeDecl, right: TypeDecl) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::TypeMismatchOperation(Box::new(TypeMismatchOperationError {
                operation: operation.to_string(),
                left,
                right,
            }))),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    pub fn not_found(item_type: &str, name: &str) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::NotFound {
                item_type: item_type.to_string(),
                name: name.to_string(),
            }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    pub fn unsupported_operation(operation: &str, type_name: TypeDecl) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::UnsupportedOperation {
                operation: operation.to_string(),
                type_name,
            }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    pub fn conversion_error(from: &str, to: &str) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::ConversionError {
                from: from.to_string(),
                to: to.to_string(),
            }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    pub fn array_error(message: &str) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::ArrayError {
                message: message.to_string(),
            }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    pub fn method_error(method: &str, type_name: TypeDecl, reason: &str) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::MethodError(Box::new(MethodErrorData {
                method: method.to_string(),
                type_name,
                reason: reason.to_string(),
            }))),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    pub fn invalid_literal(value: &str, expected_type: &str) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::InvalidLiteral {
                value: value.to_string(),
                expected_type: expected_type.to_string(),
            }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    pub fn access_denied(message: &str) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::AccessDenied {
                message: message.to_string(),
            }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    pub fn generic_error(message: &str) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::GenericError {
                message: message.to_string(),
            }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    /// LLM-LOOP P7: report what a `_` annotation resolved to.
    /// `inferred` is the type spelled as source, so the reader can paste
    /// it over the hole.
    pub fn type_hole(name: String, inferred: String) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::TypeHole { name, inferred }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    /// RECURSIVE-TYPES: a type that contains itself by value. `path`
    /// is the member chain that closes the cycle, so the reader can see
    /// which field or payload to put behind an indirection.
    pub fn recursive_type(name: String, path: String) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::RecursiveType { name, path }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    /// NEVER-ALLOCATES: the declaration cannot be honoured.
    pub fn never_allocates(
        function: String,
        path: String,
        opaque: Option<&'static str>,
    ) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::NeverAllocates { function, path, opaque }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    /// COMPILE-TIME-EVAL C1: the `const fn` declaration cannot be
    /// honoured.
    pub fn const_fn(
        function: String,
        path: String,
        what: &'static str,
        opaque: bool,
    ) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::ConstFn { function, path, what, opaque }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    /// COMPILE-TIME-EVAL C3: a forced compile-time evaluation failed.
    pub fn const_eval(context: String, detail: String) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::ConstEval { context, detail }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    /// COMPILE-TIME-EVAL C4: an impure contract predicate.
    pub fn contract_purity(
        clause: String,
        path: String,
        what: &'static str,
        opaque: bool,
    ) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::ContractPurity { clause, path, what, opaque }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    /// COMPILE-TIME-EVAL C4: a constant call that breaks its own
    /// `requires`.
    pub fn broken_precondition(function: String, detail: String) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::BrokenPrecondition { function, detail }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    /// CLOSURE-CAPTURE E1: a write to a captured binding.
    pub fn captured_assign(name: String) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::CapturedAssign { name }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    /// TYPECHECK-LIES: a reserved literal with no runtime meaning.
    /// `alternative` names what to write instead, and is part of the
    /// message rather than a machine-applicable suggestion because the
    /// right replacement depends on what the position is for.
    pub fn reserved_literal(name: &str, alternative: &str) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::ReservedLiteral {
                name: name.to_string(),
                alternative: alternative.to_string(),
            }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    /// BOX-T: reading a binding whose value was transferred away.
    pub fn use_after_move(name: String, moved_at_line: u32) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::UseAfterMove { name, moved_at_line }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
        }
    }

    /// BOX-T: a transfer whose drop would have to be decided at run
    /// time. Refused rather than tracked, for now.
    pub fn conditional_move(name: String) -> Self {
        Self {
            kind: Box::new(TypeCheckErrorKind::ConditionalMove { name }),
            context: None,
            location: None,
            origin_module: None,
            suggestions: Vec::new(),
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

    pub fn with_suggestion(mut self, suggestion: crate::diagnostic::Suggestion) -> Self {
        self.suggestions.push(suggestion);
        self
    }

    pub fn new(msg: String) -> Self {
        Self::generic_error(&msg)
    }
}

impl TypeCheckError {
    /// The diagnostic message. `interner` is optional: when present,
    /// user types are spelled by their source names (via
    /// `TypeDecl::spell_with`); when absent the interner-free
    /// `display_name` spelling is used, which shows primitives by
    /// their source name but falls back to Debug for user types.
    /// `Display` calls this without an interner; the driver's
    /// `Diagnostic::from_type_check_error` passes the real one.
    pub fn message_with(&self, interner: Option<&string_interner::DefaultStringInterner>) -> String {
        let spell = |ty: &TypeDecl| ty.spell_with(interner);
        let base_message = match &*self.kind {
            TypeCheckErrorKind::TypeMismatch { expected, actual } => {
                format!("Type mismatch: expected {}, but got {}", spell(expected), spell(actual))
            }
            TypeCheckErrorKind::TypeMismatchOperation(data) => {
                format!("Type mismatch in {} operation: incompatible types {} and {}", data.operation, spell(&data.left), spell(&data.right))
            }
            TypeCheckErrorKind::NotFound { item_type, name } => {
                format!("{} '{}' not found", item_type, name)
            }
            TypeCheckErrorKind::UnsupportedOperation { operation, type_name } => {
                format!("Unsupported operation '{}' for type {}", operation, spell(type_name))
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
            TypeCheckErrorKind::TypeHole { name, inferred } => {
                format!("type hole: `{}` has type `{}`", name, inferred)
            }
            TypeCheckErrorKind::UseAfterMove { name, moved_at_line } => {
                format!(
                    "`{}` was moved on line {} and cannot be used again",
                    name, moved_at_line
                )
            }
            TypeCheckErrorKind::ConditionalMove { name } => {
                format!(
                    "`{}` cannot be moved inside a branch or a loop body: whether it \
                     still owns its value would only be known at run time",
                    name
                )
            }
            TypeCheckErrorKind::NeverAllocates { function, path, opaque } => match opaque {
                Some(what) => format!(
                    "`{function}` is declared `never_allocates`, but it makes a call this check cannot follow ({what}): {path}"
                ),
                None => format!(
                    "`{function}` is declared `never_allocates`, but it can reach the allocator: {path}"
                ),
            },
            TypeCheckErrorKind::ConstFn { function, path, what, opaque } => {
                if *opaque {
                    format!(
                        "`{function}` is declared `const fn`, but it makes a call this check cannot follow ({what}): {path}"
                    )
                } else {
                    format!(
                        "`{function}` is declared `const fn`, but it can reach `{what}`, which the compiler cannot run while compiling: {path}"
                    )
                }
            }
            TypeCheckErrorKind::ConstEval { context, detail } => {
                format!("`{context}` must be known at compile time, but {detail}")
            }
            TypeCheckErrorKind::ContractPurity { clause, path, what, opaque } => {
                if *opaque {
                    format!(
                        "{clause} makes a call this check cannot follow ({what}), so it cannot be \
                         shown to be free of effects: {path}"
                    )
                } else {
                    format!(
                        "{clause} can reach `{what}`, so switching the contract checks off would \
                         change what the program does: {path}"
                    )
                }
            }
            TypeCheckErrorKind::BrokenPrecondition { function, detail } => {
                format!(
                    "this call to `{function}` breaks its own precondition, and every argument is \
                     a constant, so it breaks it on every run: {detail}"
                )
            }
            TypeCheckErrorKind::CapturedAssign { name } => {
                format!(
                    "cannot assign to `{name}` from inside a closure: a capture is a snapshot \
                     taken when the closure was created, so the write would not be seen by \
                     anything outside the closure"
                )
            }
            TypeCheckErrorKind::ReservedLiteral { name, alternative } => {
                format!(
                    "`{}` is reserved and has no runtime meaning: {}",
                    name, alternative
                )
            }
            TypeCheckErrorKind::RecursiveType { name, path } => {
                format!(
                    "recursive type `{}` contains itself with no indirection: {}",
                    name, path
                )
            }
        };

        // LLM-LOOP P2: the message is the message and nothing else.
        // It used to prefix `line:column:offset:`, which duplicated the
        // `Error at <file>:<line>:<col>` header the formatter already
        // prints and leaked `offset` -- a byte index into the source
        // that is meaningless to a reader and pure noise to an agent.
        // Callers that need a position read `self.location`.
        let mut result = base_message;

        if let Some(context) = &self.context {
            result = format!("{} (in {})", result, context);
        }

        result
    }
}

impl std::fmt::Display for TypeCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.message_with(None))
    }
}
