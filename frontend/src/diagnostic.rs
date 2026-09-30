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

/// One change to one file: replace `span` with `replacement`.
///
/// An insertion is a span whose `offset == end_offset`; a deletion is
/// an empty `replacement`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Edit {
    /// The file the edit applies to. `None` until the diagnostic is
    /// resolved (`Diagnostic::resolve_edits`), which names it from the
    /// span's `FileId` — so an edit can land in a file other than the
    /// one the diagnostic is reported in.
    pub file: Option<String>,
    /// Range to replace. `None` means "the diagnostic's own span" --
    /// used by suggestions built before the error has been anchored,
    /// which is the normal order for name-resolution failures.
    pub span: Option<Span>,
    pub replacement: String,
    /// Where `span` is to be found, when the producer could not say:
    /// the given word inside the diagnostic's own span. The type
    /// checker knows *what* was misspelled but not the text around
    /// it; `Diagnostic::anchor_in`, which has the text, turns this
    /// into `span`. A suggestion whose word is not found is dropped
    /// rather than applied to the whole primary span.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub word: Option<WordInSpan>,
    /// Insert `replacement` (one item per line) before the `}` that
    /// closes the first `{` after the diagnostic's span — new `match`
    /// arms, say. Resolved by `anchor_in`, which also indents the lines
    /// one level deeper than the brace when it sits on its own line.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub before_closing_brace: bool,
}

/// A word to locate inside a diagnostic's primary span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WordInSpan {
    pub word: String,
    /// Only search at or after this offset (the end of a receiver, so
    /// `xpoz.xpoz` finds the member, not the variable).
    pub after: Option<u32>,
    /// Search up to this offset instead of the span's end (the start
    /// of an argument list or a field's value, so `q.nrom(nrom)` finds
    /// the method). May lie past the span: a qualified call's span is
    /// its qualifier, and the name follows it.
    pub before: Option<u32>,
    /// Take the last occurrence in the window rather than the first.
    pub last: bool,
}

/// A fix for the diagnostic: every edit in `edits`, applied together.
///
/// Serialised with `edits` as the complete description. A suggestion
/// with exactly one edit also carries that edit's `replacement` and
/// `span` at the top level, the shape the JSON had before a
/// suggestion could make more than one change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub message: String,
    pub applicability: Applicability,
    pub edits: Vec<Edit>,
}

impl Suggestion {
    pub fn machine_applicable(message: &str, replacement: String, span: Span) -> Self {
        Suggestion::with_edits(
            message,
            Applicability::MachineApplicable,
            vec![Edit { file: None, span: Some(span), replacement, word: None, before_closing_brace: false }],
        )
    }

    /// A replacement for whatever the diagnostic itself points at.
    pub fn over_primary_span(message: &str, replacement: String) -> Self {
        Suggestion::with_edits(
            message,
            Applicability::MachineApplicable,
            vec![Edit { file: None, span: None, replacement, word: None, before_closing_brace: false }],
        )
    }

    /// Several edits that only make sense together (both ends of a
    /// signature change, say).
    pub fn with_edits(message: &str, applicability: Applicability, edits: Vec<Edit>) -> Self {
        Suggestion { message: message.to_string(), applicability, edits }
    }

    /// Replace the word `old` inside the diagnostic's span with `new`
    /// — a misspelled member or field, found by name because the
    /// producer does not have the text.
    pub fn rename_in_primary(message: &str, new: &str, target: WordInSpan) -> Self {
        Suggestion::with_edits(
            message,
            Applicability::MachineApplicable,
            vec![Edit {
                file: None,
                span: None,
                replacement: new.to_string(),
                word: Some(target),
                before_closing_brace: false,
            }],
        )
    }

    /// Lines to add before the brace closing the construct the
    /// diagnostic points at (see [`Edit::before_closing_brace`]).
    pub fn insert_before_closing_brace(message: &str, applicability: Applicability, lines: Vec<String>) -> Self {
        Suggestion::with_edits(
            message,
            applicability,
            vec![Edit {
                file: None,
                span: None,
                replacement: lines.join("\n"),
                word: None,
                before_closing_brace: true,
            }],
        )
    }

    /// The replacement text of a single-edit suggestion.
    pub fn replacement(&self) -> Option<&str> {
        match self.edits.as_slice() {
            [only] => Some(only.replacement.as_str()),
            _ => None,
        }
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Suggestion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let single = match self.edits.as_slice() {
            [only] => Some(only),
            _ => None,
        };
        let mut st = serializer
            .serialize_struct("Suggestion", if single.is_some() { 5 } else { 3 })?;
        st.serialize_field("message", &self.message)?;
        if let Some(only) = single {
            st.serialize_field("replacement", &only.replacement)?;
            st.serialize_field("span", &only.span)?;
        }
        st.serialize_field("applicability", &self.applicability)?;
        st.serialize_field("edits", &self.edits)?;
        st.end()
    }
}

/// Another place a diagnostic is about: "first defined here", "moved
/// here". Resolved like an edit — `file` from the span's `FileId`,
/// line and column from the text — by `Diagnostic::anchor_in`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Related {
    pub file: Option<String>,
    pub span: Option<Span>,
    pub message: String,
    /// Narrow `span` to the first occurrence of this word at or after
    /// its start (a declaration's name, found from where it begins).
    #[cfg_attr(feature = "serde", serde(skip))]
    pub word: Option<String>,
}

impl Related {
    pub fn at(location: SourceLocation, message: impl Into<String>) -> Self {
        Related { file: None, span: Some(Span::from(location)), message: message.into(), word: None }
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
    /// The module a diagnostic came from, when one is known.
    ///
    /// It used to mean "the span is **not** in `file`, do not resolve
    /// it there", because `file` was always the file being compiled.
    /// `Diagnostic::anchor_in` fixed that: `file` now names the file
    /// the span is actually in, so this is a hint about *provenance*
    /// rather than a warning about a trap.
    ///
    /// Which is why the whole-program checks (ownership, regions,
    /// effects) leave it `None` and are still correct — they run
    /// after the per-module walk that would set it, and they no
    /// longer need to.
    pub origin_module: Option<String>,
    pub suggestions: Vec<Suggestion>,
    /// Other places this diagnostic is about (LLM-TOOLING #3). Always
    /// present in the JSON, empty when there are none.
    pub related: Vec<Related>,
    /// The error only follows from another (`TypeCheckError::follows_unknown`);
    /// `drop_cascades` removes it next to a real error.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub cascade: bool,
    /// See `TypeCheckError::location_word`; consumed by `anchor_in`.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub span_word: Option<String>,
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
            related: Vec::new(),
            cascade: false,
            span_word: None,
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
            related: error.anchors.as_ref().map(|a| a.related.clone()).unwrap_or_default(),
            cascade: error.follows_unknown(),
            span_word: error.anchors.as_ref().and_then(|a| a.location_word.clone()),
            backtrace: Vec::new(),
        }
    }

    /// The same diagnostic with `file` naming the file its span is
    /// actually in.
    ///
    /// DEBUG-OBS D2: `file` is the file being compiled, which is not
    /// where an imported module's span lives. The text renderer has
    /// resolved the span's `FileId` against the source map for a
    /// while; a JSON consumer could not, because the id means nothing
    /// outside the program that produced it. Resolving it here puts
    /// the path on the wire, so `file` + `span` is a position a tool
    /// can open. `origin_module` still says *why* it is not the entry.
    pub fn anchor_in(&mut self, source_map: &crate::source_map::SourceMap) {
        let entry_file = self.file.clone();
        // A span given as "this word, from here" (a declaration's
        // name) or with offsets only (line 0) is completed from the text.
        if let Some(span) = self.span {
            let located = match self.span_word.take() {
                Some(word) => source_map.find_word(span.file, span.offset as usize, usize::MAX, &word),
                None if span.line == 0 => {
                    source_map.location(span.file, span.offset as usize, span.end_offset as usize)
                }
                None => None,
            };
            if let Some(at) = located {
                self.span = Some(Span::from(at));
            }
        }
        if let Some(span) = self.span
            && span.file != crate::source_map::FileId::ENTRY
            && let Some(path) = source_map.path(span.file)
        {
            self.file = path.to_string();
        }
        // Locate the words the checker named but could not place.
        let primary = self.span;
        self.suggestions.retain_mut(|suggestion| {
            suggestion.edits.iter_mut().all(|edit| {
                let Some(target) = edit.word.take() else { return true };
                let Some(p) = primary else { return false };
                let from = p.offset.max(target.after.unwrap_or(0)) as usize;
                let to = target.before.unwrap_or(p.end_offset) as usize;
                let found = if target.last {
                    source_map.rfind_word(p.file, from, to, &target.word)
                } else {
                    source_map.find_word(p.file, from, to, &target.word)
                };
                edit.span = found.map(Span::from);
                edit.span.is_some()
            })
        });
        // Insertions before a closing brace, laid out as lines.
        self.suggestions.retain_mut(|suggestion| {
            suggestion.edits.iter_mut().all(|edit| {
                if !edit.before_closing_brace {
                    return true;
                }
                edit.before_closing_brace = false;
                let Some(p) = primary else { return false };
                let Some(source) = source_map.source(p.file) else { return false };
                let Some(close) = source_map.closing_brace_after(p.file, p.offset as usize) else {
                    return false;
                };
                let line_start = source[..close].rfind('\n').map(|i| i + 1).unwrap_or(0);
                let indent = &source[line_start..close];
                let (at, text) = if indent.chars().all(|c| c == ' ' || c == '\t') {
                    // `}` on its own line: whole lines before it.
                    let lines: String = edit
                        .replacement
                        .lines()
                        .map(|l| format!("{indent}    {l}\n"))
                        .collect();
                    (line_start, lines)
                } else {
                    (close, format!(" {} ", edit.replacement.replace('\n', " ")))
                };
                edit.replacement = text;
                edit.span = source_map.location(p.file, at, at).map(Span::from);
                edit.span.is_some()
            })
        });
        // Each edit names its own file: a fix for a call's error can
        // be in the callee's module, and vice versa.
        for edit in self.suggestions.iter_mut().flat_map(|s| s.edits.iter_mut()) {
            if edit.file.is_some() {
                continue;
            }
            let span = edit.span.or(primary);
            edit.file = Some(match span {
                Some(span) if span.file != crate::source_map::FileId::ENTRY => source_map
                    .path(span.file)
                    .map(str::to_string)
                    .unwrap_or_else(|| entry_file.clone()),
                _ => entry_file.clone(),
            });
        }
        // Related places: the same resolution, plus line and column
        // worked out from the text (a declaration's node has offsets
        // only), and narrowed to a name when one was given.
        self.related.retain_mut(|related| {
            let Some(span) = related.span else { return false };
            let located = match related.word.take() {
                Some(word) => source_map.find_word(span.file, span.offset as usize, usize::MAX, &word),
                None => source_map.location(span.file, span.offset as usize, span.end_offset as usize),
            };
            if let Some(at) = located {
                related.span = Some(Span::from(at));
            }
            related.file = Some(if span.file == crate::source_map::FileId::ENTRY {
                entry_file.clone()
            } else {
                source_map.path(span.file).map(str::to_string).unwrap_or_else(|| entry_file.clone())
            });
            true
        });
        self.resolve_edits();
    }

    /// Give every edit an explicit file and span, so a consumer never
    /// has to know the "`None` means the diagnostic's own" rule. What
    /// `anchor_in` could not name is in the diagnostic's own file.
    pub fn resolve_edits(&mut self) {
        for related in &mut self.related {
            related.word = None;
            if related.file.is_none() {
                related.file = Some(self.file.clone());
            }
        }
        // Nothing located a word-targeted edit (no source map reached
        // this diagnostic); applying it to the whole span would be
        // wrong, so the suggestion goes.
        self.suggestions
            .retain(|s| s.edits.iter().all(|e| e.word.is_none() && !e.before_closing_brace));
        let primary = self.span;
        for edit in self.suggestions.iter_mut().flat_map(|s| s.edits.iter_mut()) {
            if edit.file.is_none() {
                edit.file = Some(self.file.clone());
            }
            if edit.span.is_none() {
                edit.span = primary;
            }
        }
    }

    /// A parse error as a structured diagnostic. Lex errors carry
    /// their own code (E0012), the two traps with a known fix theirs
    /// (E0033 / E0034), and every other failure to read the program
    /// is a syntax error (E0032).
    pub fn from_parser_error(error: &ParserError, file: &str) -> Self {
        Diagnostic {
            severity: Severity::Error,
            code: match &error.kind {
                ParserErrorKind::LexError { .. } => codes::LEXICAL,
                ParserErrorKind::ElseIf => codes::ELSE_IF,
                ParserErrorKind::UnsuffixedFloat { .. } => codes::UNSUFFIXED_FLOAT,
                ParserErrorKind::UnexpectedToken { .. }
                | ParserErrorKind::GenericError { .. }
                | ParserErrorKind::RecursionLimitExceeded => codes::SYNTAX,
                ParserErrorKind::IoError { .. } => codes::UNCATEGORISED,
            },
            // The formatter prints the code; a lex error's `Display` carries
            // it too (for renderings without a code), which printed it twice.
            message: match &error.kind {
                ParserErrorKind::LexError { message } => message.clone(),
                _ => error.to_string(),
            },
            file: file.to_string(),
            span: Some(Span::from(error.location)),
            origin_module: None,
            suggestions: error.suggestions.clone(),
            related: Vec::new(),
            cascade: false,
            span_word: None,
            backtrace: Vec::new(),
        }
    }
}

/// Drop the errors that are consequences of another one (LLM-TOOLING
/// #6, CASCADE-BY-KIND).
///
/// A failed expression is typed `Unknown`, and checks downstream of it
/// are meant to stay quiet about it (the poison rule). The ones that do
/// not are marked where the error is built — `Diagnostic::cascade`,
/// from `TypeCheckError::follows_unknown` — and removed here, once,
/// rather than fixed one check at a time.
///
/// Only when a real error remains: an `Unknown` with nothing to explain
/// it is the checker's own bug, and hiding it would hide that.
pub fn drop_cascades(errors: &mut Vec<Diagnostic>) {
    if errors.iter().any(|d| !d.cascade) {
        errors.retain(|d| !d.cascade);
    }
}

/// Put diagnostics in their one reported order and drop exact repeats
/// (LLM-TOOLING #5).
///
/// The order is the entry file's first, then other files by name, and
/// within a file by position, code and message; a diagnostic without a
/// span keeps its place after the positioned ones of its file. Checks
/// run in whatever order the checker walks, and some of that order
/// comes from hash maps, whose iteration differs from one process to
/// the next — sorting here, once, is what makes the same input give
/// the same bytes.
pub fn normalize(diagnostics: &mut Vec<Diagnostic>, entry_file: &str) {
    diagnostics.sort_by(|a, b| {
        let key = |d: &Diagnostic| {
            let span = d.span.map(|s| (s.offset, s.end_offset)).unwrap_or((u32::MAX, u32::MAX));
            (d.file != entry_file, d.file.clone(), span, d.code, d.message.clone())
        };
        key(a).cmp(&key(b))
    });
    diagnostics.dedup_by(|a, b| {
        a.file == b.file && a.span == b.span && a.code == b.code && a.message == b.message
    });
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

    /// ELEMENT-BORROW E5: an owning element read out of a container by
    /// value, which leaves the container and the binding both owning
    /// it. `borrow` names the element instead.
    pub const OWNING_ELEMENT_COPY: &str = "E0028";

    /// CONCURRENCY A1: a `parallel for` body does something the
    /// order of the iterations would be visible in.
    pub const PARALLEL_BODY: &str = "E0029";

    /// MODULE-SYSTEM P3: a call names a module path no module has.
    pub const UNKNOWN_MODULE_PATH: &str = "E0030";

    /// LLM-TOOLING L0: one name declared twice in one file.
    pub const DUPLICATE_DEFINITION: &str = "E0031";

    // LLM-TOOLING #2: codes carved out of the E0010 catch-all, so a
    // tool can act on the code instead of parsing the message.

    /// The parser could not read the program here.
    pub const SYNTAX: &str = "E0032";

    /// `else if`; the language spells it `elif`.
    pub const ELSE_IF: &str = "E0033";

    /// `1.5`: a float literal without its `f64` / `f32` suffix.
    pub const UNSUFFIXED_FLOAT: &str = "E0034";

    /// A `match` leaves values uncovered, or an arm can never run.
    pub const MATCH_COVERAGE: &str = "E0035";

    /// A pattern does not fit the value it is matched against.
    pub const PATTERN_SHAPE: &str = "E0036";

    /// `?` / `??` applied to something that is not `Option` / `Result`.
    pub const TRY_OPERAND: &str = "E0037";

    /// A type does not satisfy a trait: a generic bound, an impl that
    /// does not match its trait, or a trait that does not exist.
    pub const TRAIT_BOUND: &str = "E0038";

    /// A write through a shared borrow (`&T`), which would reach a copy.
    pub const SHARED_BORROW_WRITE: &str = "E0039";

    // LLM-TOOLING-E0010-REST: the rest of the catch-all, by family.

    /// The compiler's own invariant broke; not a mistake in the program.
    pub const INTERNAL: &str = "E0040";

    /// A call or constructor was given the wrong number of arguments.
    pub const ARITY: &str = "E0041";

    /// A SIMD vector operation used with operands it does not take.
    pub const SIMD: &str = "E0042";

    /// A generic type parameter could not be worked out from the call.
    pub const GENERIC_INFERENCE: &str = "E0043";

    /// A write to (or `&mut` of) a binding that is not `var`.
    pub const IMMUTABLE_WRITE: &str = "E0044";

    /// A struct (or struct-variant) literal or pattern whose fields do
    /// not match the declaration.
    pub const STRUCT_FIELDS: &str = "E0045";

    /// `break` / `continue` outside a loop, or naming no enclosing label.
    pub const LOOP_CONTROL: &str = "E0046";

    /// A `requires` / `ensures` clause that is not a `bool`, or `old(..)`
    /// outside `ensures`.
    pub const CONTRACT_CLAUSE: &str = "E0047";

    /// An `extern fn` whose signature cannot cross the C ABI.
    pub const FFI_ABI: &str = "E0048";

    /// A name that more than one module defines, written without enough
    /// of a path to pick one.
    pub const AMBIGUOUS_NAME: &str = "E0049";

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
        OWNING_ELEMENT_COPY,
        PARALLEL_BODY,
        UNKNOWN_MODULE_PATH,
        DUPLICATE_DEFINITION,
        SYNTAX,
        ELSE_IF,
        UNSUFFIXED_FLOAT,
        MATCH_COVERAGE,
        PATTERN_SHAPE,
        TRY_OPERAND,
        TRAIT_BOUND,
        SHARED_BORROW_WRITE,
        INTERNAL,
        ARITY,
        SIMD,
        GENERIC_INFERENCE,
        IMMUTABLE_WRITE,
        STRUCT_FIELDS,
        LOOP_CONTROL,
        CONTRACT_CLAUSE,
        FFI_ABI,
        AMBIGUOUS_NAME,
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
        TypeCheckErrorKind::Coded { code, .. } => code,
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
        TypeCheckErrorKind::OwningElementCopy { .. } => codes::OWNING_ELEMENT_COPY,
        TypeCheckErrorKind::ParallelBody { .. } => codes::PARALLEL_BODY,
        TypeCheckErrorKind::UnknownModulePath { .. } => codes::UNKNOWN_MODULE_PATH,
        TypeCheckErrorKind::DuplicateDefinition { .. } => codes::DUPLICATE_DEFINITION,
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
/// Edit distance with an adjacent transposition counted as one edit
/// (optimal string alignment), or `None` when it exceeds `limit`.
///
/// Plain Levenshtein counts `nrom` -> `norm` as two edits, which put the
/// commonest typo there is past the one-edit limit short names get
/// (LLM-TOOLING-DID-YOU-MEAN-TRANSPOSE).
fn edit_distance_within(a: &str, b: &str, limit: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > limit {
        return None;
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (d[i - 1][j - 1] + cost).min(d[i - 1][j] + 1).min(d[i][j - 1] + 1);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = best;
        }
    }
    (d[n][m] <= limit).then_some(d[n][m])
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

    /// Every code that has shipped, in order. A tool keys its handling
    /// on a code, so a released code keeps its number and meaning: a
    /// new code is appended to `codes::ALL` (and here, once it ships),
    /// and a retired one stays in both, its `--explain` entry saying
    /// it is retired.
    const RELEASED: &[&str] = &[
        "E0001", "E0002", "E0003", "E0004", "E0005", "E0006", "E0007", "E0008", "E0009",
        "E0010", "E0011", "E0012", "E0013", "E0014", "E0015", "E0016", "E0017", "E0018",
        "E0019", "E0020", "E0021", "E0022", "E0023", "E0024", "E0025", "E0026", "E0027",
        "E0028", "E0029", "E0030", "E0031", "E0032", "E0033", "E0034", "E0035", "E0036",
        "E0037", "E0038", "E0039", "E0040", "E0041", "E0042", "E0043", "E0044", "E0045",
        "E0046", "E0047", "E0048", "E0049",
    ];

    #[test]
    fn released_codes_are_never_renumbered_or_removed() {
        assert!(
            codes::ALL.starts_with(RELEASED),
            "codes::ALL must begin with every released code, in order -- append new \
             codes at the end, never reuse or remove one"
        );
    }

    #[test]
    fn codes_are_consecutive_and_distinct() {
        for (i, code) in codes::ALL.iter().enumerate() {
            assert_eq!(*code, format!("E{:04}", i + 1), "codes::ALL[{i}]");
        }
    }

    #[test]
    fn a_cascade_is_dropped_only_next_to_a_real_error() {
        let d = |m: &str, cascade: bool| {
            let mut d = Diagnostic::message_only(m.to_string(), "t.t");
            d.cascade = cascade;
            d
        };
        let mut both = vec![d("Unknown field 'z' in struct 'P'", false), d("field access 'w' for type Unknown", true)];
        drop_cascades(&mut both);
        assert_eq!(both.len(), 1);
        assert!(both[0].message.starts_with("Unknown field"));
        let mut alone = vec![d("match scrutinee must be an enum, got Unknown", true)];
        drop_cascades(&mut alone);
        assert_eq!(alone.len(), 1, "an unexplained Unknown is kept");
    }

    #[test]
    fn a_swapped_pair_is_one_edit() {
        assert_eq!(closest_candidate("nrom", ["norm", "len"]), Some("norm"));
        assert_eq!(closest_candidate("lenght", ["length", "height"]), Some("length"));
        // Still declines a tie.
        assert_eq!(closest_candidate("ab", ["ba", "abc"]), None);
    }
}
