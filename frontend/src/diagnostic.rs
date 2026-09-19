//! Structured diagnostics (LLM-LOOP P3).
//!
//! The human-readable rendering stays the primary output. This module
//! adds the machine-readable shape behind it, so a tool driving the
//! compiler can pick out the span it needs or apply a fix without
//! scraping formatted text.
//!
//! Two rules shape what goes in here:
//!
//! * **A suggestion is only emitted when applying it is guaranteed to
//!   compile.** A speculative "did you mean…?" that turns out to be
//!   wrong costs an agent a full round trip *and* leaves it with less
//!   trust in the next suggestion. Anything less than certain is left
//!   to the message text.
//! * **Codes are stable identifiers, not Rust's.** They look like
//!   `E0001`, but they are toylang's own numbering — reusing Rust's
//!   numbers for different meanings would be worse than having none.

use crate::parser::error::{ParserError, ParserErrorKind};
use crate::source_map::FileId;
use crate::type_checker::{SourceLocation, TypeCheckError, TypeCheckErrorKind};
use crate::type_decl::TypeDecl;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// How safe it is to apply a suggestion without human review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum Applicability {
    /// Applying the replacement verbatim resolves this diagnostic.
    MachineApplicable,
    /// Probably right, but verify. Never emitted today -- kept so the
    /// distinction is explicit in the schema rather than implied.
    MaybeIncorrect,
}

/// A byte range in a source file, with the line/column of its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Span {
    /// Which file the span is in (DEBUG-OBS D2).
    ///
    /// Not serialized: a `FileId` is an index into *this* program's
    /// `SourceMap` and means nothing to a reader of the JSON. What a
    /// consumer needs is the path, and putting that on the wire is
    /// D5's job — together with the rest of the runtime-side
    /// machine-readable output. Until then `Diagnostic::origin_module`
    /// stays the signal that a span is not in `Diagnostic::file`.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub file: FileId,
    pub line: u32,
    pub column: u32,
    pub offset: u32,
    pub end_offset: u32,
}

impl From<SourceLocation> for Span {
    fn from(loc: SourceLocation) -> Self {
        Span {
            file: loc.file,
            line: loc.line,
            column: loc.column,
            offset: loc.offset,
            end_offset: loc.end_offset,
        }
    }
}

/// An edit that resolves the diagnostic: replace `span` with
/// `replacement`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Suggestion {
    pub message: String,
    pub replacement: String,
    /// Range to replace. `None` means "the diagnostic's own span" --
    /// used by suggestions built before the error has been anchored,
    /// which is the normal order for name-resolution failures.
    pub span: Option<Span>,
    pub applicability: Applicability,
}

impl Suggestion {
    pub fn machine_applicable(message: &str, replacement: String, span: Span) -> Self {
        Suggestion {
            message: message.to_string(),
            replacement,
            span: Some(span),
            applicability: Applicability::MachineApplicable,
        }
    }

    /// A replacement for whatever the diagnostic itself points at.
    pub fn over_primary_span(message: &str, replacement: String) -> Self {
        Suggestion {
            message: message.to_string(),
            replacement,
            span: None,
            applicability: Applicability::MachineApplicable,
        }
    }

    /// Resolve `span`, defaulting to the diagnostic's primary span.
    pub fn effective_span(&self, primary: Option<Span>) -> Option<Span> {
        self.span.or(primary)
    }
}

/// One reported problem, in the shape a tool consumes.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub file: String,
    pub span: Option<Span>,
    /// Set when `span` refers to an imported module's source rather than
    /// `file`. Consumers must not resolve the span against `file`.
    pub origin_module: Option<String>,
    pub suggestions: Vec<Suggestion>,
    /// How the failure was reached, innermost first (DEBUG-OBS D5).
    ///
    /// Only a *runtime* failure has one — a type error is not reached,
    /// it is found. Empty for everything else, and omitted from the
    /// JSON entirely so the shape a tool already parses is unchanged.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Vec::is_empty"))]
    pub backtrace: Vec<BacktraceFrame>,
}

/// One rendered backtrace frame, in the shape a tool consumes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct BacktraceFrame {
    /// What the user calls the function — `S::boom`, not a mangled name.
    pub function: String,
    /// The line the call was written on. Absent for the entry
    /// function, which nothing called.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub line: Option<u32>,
}

impl Diagnostic {
    /// A diagnostic that carries only prose -- used for the handful of
    /// driver-level failures (module resolution, impl-block wiring) that
    /// never produced a `TypeCheckError` to begin with.
    pub fn message_only(message: String, file: &str) -> Self {
        Diagnostic {
            severity: Severity::Error,
            code: codes::UNCATEGORISED,
            message,
            file: file.to_string(),
            span: None,
            origin_module: None,
            suggestions: Vec::new(),
            backtrace: Vec::new(),
        }
    }

    /// `interner` spells user types by their source names in the
    /// message (see `TypeCheckError::message_with`); pass `None` when
    /// none is available and the interner-free spelling is used.
    pub fn from_type_check_error(
        error: &TypeCheckError,
        file: &str,
        interner: Option<&string_interner::DefaultStringInterner>,
    ) -> Self {
        Diagnostic {
            severity: Severity::Error,
            code: code_for(&error.kind),
            message: error.message_with(interner),
            file: file.to_string(),
            span: error.location.map(Span::from),
            origin_module: error.origin_module.clone(),
            suggestions: error.suggestions.clone(),
            backtrace: Vec::new(),
        }
    }

    /// A parse error as a structured diagnostic. Lex errors carry
    /// their own code (E0012); the parser's other failures have no
    /// category yet and share the catch-all.
    pub fn from_parser_error(error: &ParserError, file: &str) -> Self {
        Diagnostic {
            severity: Severity::Error,
            code: match &error.kind {
                ParserErrorKind::LexError { .. } => codes::LEXICAL,
                _ => codes::UNCATEGORISED,
            },
            message: error.to_string(),
            file: file.to_string(),
            span: Some(Span::from(error.location)),
            origin_module: None,
            suggestions: Vec::new(),
            backtrace: Vec::new(),
        }
    }
}

pub mod codes {
    pub const TYPE_MISMATCH: &str = "E0001";
    pub const TYPE_MISMATCH_OPERATION: &str = "E0002";
    pub const NOT_FOUND: &str = "E0003";
    pub const UNSUPPORTED_OPERATION: &str = "E0004";
    pub const CONVERSION: &str = "E0005";
    pub const ARRAY: &str = "E0006";
    pub const METHOD: &str = "E0007";
    pub const INVALID_LITERAL: &str = "E0008";
    pub const ACCESS_DENIED: &str = "E0009";
    pub const UNCATEGORISED: &str = "E0010";
    /// The answer to a `val x: _ = expr` type hole (LLM-LOOP P7). Not a
    /// defect in the program — it reports what was asked for.
    pub const TYPE_HOLE: &str = "E0011";
    /// A lexical failure: a literal or character the lexer could not
    /// read (`"\q"`, `"\x80"`, an unterminated interpolation, ...).
    pub const LEXICAL: &str = "E0012";
    /// A struct / enum that contains itself with no indirection
    /// (RECURSIVE-TYPES). It has no finite layout, so no backend can
    /// represent it.
    pub const RECURSIVE_TYPE: &str = "E0013";
    /// A resource-owning value used after it was handed to something
    /// else, or handed over where the handover cannot be modelled
    /// (BOX-T).
    pub const MOVED_VALUE: &str = "E0014";
    /// A literal the grammar accepts but no backend implements
    /// (TYPECHECK-LIES). `null` is the only one.
    pub const RESERVED_LITERAL: &str = "E0015";
    /// A function declared `never_allocates` that can reach the
    /// allocator (NEVER-ALLOCATES).
    pub const NEVER_ALLOCATES: &str = "E0016";
    /// A function declared `const fn` that reaches something the
    /// compiler cannot run while compiling (COMPILE-TIME-EVAL).
    pub const CONST_FN: &str = "E0017";
    /// A `requires` / `ensures` clause that is not free of effects, or
    /// a constant call that breaks its own precondition
    /// (COMPILE-TIME-EVAL C4). Reported as a warning for one release.
    pub const CONTRACT_PURITY: &str = "E0018";
    /// The program stopped while running: a `panic`, a failed
    /// `assert`, or a RUNTIME-TRAP guard (DEBUG-OBS D5).
    pub const RUNTIME_PANIC: &str = "E0019";
    /// A `requires` / `ensures` clause was false at run time
    /// (DEBUG-OBS D5).
    pub const CONTRACT_VIOLATION: &str = "E0020";
    /// A closure body assigns to a binding it captured from an
    /// enclosing scope (CLOSURE-CAPTURE E1). The capture is a
    /// snapshot, so the write reaches nothing.
    pub const CAPTURED_ASSIGN: &str = "E0021";

    /// REGION: a value allocated from a scoped allocator outlives it
    /// (`with allocator = arena { ... }`).
    pub const REGION_ESCAPE: &str = "E0022";

    /// DBC-LISKOV: an `impl` of a trait method demands more than the
    /// trait promised its callers.
    pub const IMPL_PRECONDITION: &str = "E0023";

    /// POINTER P6: a body reaches a raw-memory builtin without the
    /// `unsafe fn` declaration.
    pub const UNSAFE_REQUIRED: &str = "E0024";

    /// MUST-USE: a statement produced a `Result` and discarded it, so
    /// a failure it reports goes unnoticed. A warning, not an error —
    /// ignoring one can be deliberate.
    pub const UNUSED_RESULT: &str = "E0025";

    /// WINDOW-ESCAPE: a `Span<T>` / `Column<T>` outlives the buffer it
    /// views (POINTER P4's deferred half).
    pub const WINDOW_ESCAPE: &str = "E0026";

    /// ELEMENT-BORROW 2-d: an owning value copied out of a borrow,
    /// which would make a second owner of one resource.
    pub const BORROW_COPY_OUT: &str = "E0027";

    /// Every code, in order. `crate::explain` is checked against this
    /// list by a test, so a new code cannot ship without prose.
    pub const ALL: &[&str] = &[
        TYPE_MISMATCH,
        TYPE_MISMATCH_OPERATION,
        NOT_FOUND,
        UNSUPPORTED_OPERATION,
        CONVERSION,
        ARRAY,
        METHOD,
        INVALID_LITERAL,
        ACCESS_DENIED,
        UNCATEGORISED,
        TYPE_HOLE,
        LEXICAL,
        RECURSIVE_TYPE,
        MOVED_VALUE,
        RESERVED_LITERAL,
        NEVER_ALLOCATES,
        CONST_FN,
        CONTRACT_PURITY,
        RUNTIME_PANIC,
        CONTRACT_VIOLATION,
        CAPTURED_ASSIGN,
        REGION_ESCAPE,
        IMPL_PRECONDITION,
        UNSAFE_REQUIRED,
        UNUSED_RESULT,
        WINDOW_ESCAPE,
        BORROW_COPY_OUT,
    ];
}

fn code_for(kind: &TypeCheckErrorKind) -> &'static str {
    match kind {
        TypeCheckErrorKind::TypeMismatch { .. } => codes::TYPE_MISMATCH,
        TypeCheckErrorKind::TypeMismatchOperation(_) => codes::TYPE_MISMATCH_OPERATION,
        TypeCheckErrorKind::NotFound { .. } => codes::NOT_FOUND,
        TypeCheckErrorKind::UnsupportedOperation { .. } => codes::UNSUPPORTED_OPERATION,
        TypeCheckErrorKind::ConversionError { .. } => codes::CONVERSION,
        TypeCheckErrorKind::ArrayError { .. } => codes::ARRAY,
        TypeCheckErrorKind::MethodError(_) => codes::METHOD,
        TypeCheckErrorKind::InvalidLiteral { .. } => codes::INVALID_LITERAL,
        TypeCheckErrorKind::AccessDenied { .. } => codes::ACCESS_DENIED,
        TypeCheckErrorKind::GenericError { .. } => codes::UNCATEGORISED,
        TypeCheckErrorKind::TypeHole { .. } => codes::TYPE_HOLE,
        TypeCheckErrorKind::RecursiveType { .. } => codes::RECURSIVE_TYPE,
        TypeCheckErrorKind::UseAfterMove { .. }
        | TypeCheckErrorKind::ConditionalMove { .. } => codes::MOVED_VALUE,
        TypeCheckErrorKind::ReservedLiteral { .. } => codes::RESERVED_LITERAL,
        TypeCheckErrorKind::NeverAllocates { .. } => codes::NEVER_ALLOCATES,
        TypeCheckErrorKind::ConstFn { .. } | TypeCheckErrorKind::ConstEval { .. } => {
            codes::CONST_FN
        }
        TypeCheckErrorKind::ContractPurity { .. }
        | TypeCheckErrorKind::BrokenPrecondition { .. } => codes::CONTRACT_PURITY,
        TypeCheckErrorKind::CapturedAssign { .. } => codes::CAPTURED_ASSIGN,
        TypeCheckErrorKind::RegionEscape { .. } => codes::REGION_ESCAPE,
        TypeCheckErrorKind::ImplPrecondition { .. } => codes::IMPL_PRECONDITION,
        TypeCheckErrorKind::UnsafeRequired { .. } => codes::UNSAFE_REQUIRED,
        TypeCheckErrorKind::UnusedResult { .. } => codes::UNUSED_RESULT,
        TypeCheckErrorKind::WindowEscape { .. } => codes::WINDOW_ESCAPE,
        TypeCheckErrorKind::BorrowCopyOut { .. } => codes::BORROW_COPY_OUT,
    }
}

/// Spelling of a type as it would be written in source, for types that
/// can appear in an `as` cast. `None` for anything else -- a suggestion
/// is only worth emitting when we can name the target exactly.
pub fn castable_type_name(ty: &TypeDecl) -> Option<&'static str> {
    Some(match ty {
        TypeDecl::UInt64 => "u64",
        TypeDecl::Int64 => "i64",
        TypeDecl::Float64 => "f64",
        TypeDecl::UInt8 => "u8",
        TypeDecl::UInt16 => "u16",
        TypeDecl::UInt32 => "u32",
        TypeDecl::Int8 => "i8",
        TypeDecl::Int16 => "i16",
        TypeDecl::Int32 => "i32",
        _ => return None,
    })
}

/// Levenshtein distance, capped: returns `None` once the distance is
/// certainly above `limit` so a long scan can bail early.
fn edit_distance_within(a: &str, b: &str, limit: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > limit {
        return None;
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let d = prev[b.len()];
    (d <= limit).then_some(d)
}

/// Pick a "did you mean" candidate for `name`.
///
/// Deliberately strict: a single best match, strictly closer than every
/// other candidate, within a distance that scales with the name's
/// length. A tie means we cannot tell which was meant, and guessing
/// would send the reader down the wrong path -- so nothing is emitted.
pub fn closest_candidate<'a, I>(name: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    // One edit for short names, two for longer ones. Anything looser
    // starts matching unrelated identifiers.
    let limit = if name.chars().count() <= 4 { 1 } else { 2 };
    let mut best: Option<(usize, &str)> = None;
    let mut tied = false;
    for candidate in candidates {
        if candidate == name {
            continue;
        }
        let Some(d) = edit_distance_within(name, candidate, limit) else {
            continue;
        };
        match best {
            None => best = Some((d, candidate)),
            Some((best_d, _)) if d < best_d => {
                best = Some((d, candidate));
                tied = false;
            }
            Some((best_d, _)) if d == best_d => tied = true,
            _ => {}
        }
    }
    if tied { None } else { best.map(|(_, c)| c) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_the_single_close_match() {
        let candidates = ["println", "panic"];
        assert_eq!(closest_candidate("printlnn", candidates), Some("println"));
    }

    #[test]
    fn declines_when_a_typo_sits_between_two_real_names() {
        // `printn` is one edit from both `println` and `print`. Either
        // could be what was meant, so neither is offered -- a wrong
        // suggestion costs more than no suggestion.
        let candidates = ["println", "print", "panic"];
        assert_eq!(closest_candidate("printn", candidates), None);
    }

    #[test]
    fn declines_when_two_candidates_are_equally_close() {
        // `cat` is one edit from both; picking either would be a guess.
        let candidates = ["bat", "hat"];
        assert_eq!(closest_candidate("cat", candidates), None);
    }

    #[test]
    fn declines_when_nothing_is_close() {
        let candidates = ["println", "panic"];
        assert_eq!(closest_candidate("completely_different", candidates), None);
    }

    #[test]
    fn short_names_require_a_closer_match() {
        // Two edits away, but `ab` is short enough that two edits could
        // reach almost anything.
        assert_eq!(closest_candidate("ab", ["xy"]), None);
        assert_eq!(closest_candidate("ab", ["axb"]), Some("axb"));
    }

    #[test]
    fn ignores_an_exact_match() {
        assert_eq!(closest_candidate("print", ["print"]), None);
    }
}
