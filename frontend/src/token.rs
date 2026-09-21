#[derive(Debug, PartialEq, Clone)]
pub struct Token {
    pub kind: Kind,
    pub position: std::ops::Range<usize>,
}

/// One segment of a `Kind::InterpolatedString`. Literal segments
/// hold verbatim text (escapes already processed); Expr segments
/// hold the raw source text from inside `{...}` so the parser can
/// re-tokenize them on demand.
#[derive(Debug, PartialEq, Clone)]
pub enum StringPart {
    Literal(String),
    /// `{expr}` or, with a STR-INTERP-FMT format spec, `{expr:spec}`.
    /// The lexer only splits the two at the depth-0 `:`; the spec
    /// text is validated by the parser (see
    /// [`crate::format_spec::FormatSpec`]) so a bad spec is reported
    /// with the rest of the parse diagnostics.
    Expr {
        text: String,
        spec: Option<String>,
        /// Absolute byte offset of `text` in the source file
        /// (INTERP-DIAG-SPAN). The parser re-lexes `text` with a
        /// fresh lexer whose positions start at 0; adding this
        /// offset puts every synthesized token back where the user
        /// wrote it, so a type error inside `"{a + b}"` points at
        /// the offending sub-expression instead of the file's first
        /// line.
        offset: usize,
    },
}

#[derive(Debug, PartialEq, Clone)]
pub enum Kind {
    If,
    Elif,
    Else,
    For,
    In,
    To,
    While,
    Loop,
    Break,
    Continue,
    Class,
    Struct,
    Trait,
    Impl,
    Dyn,
    Function,
    Return,
    Extern,
    Public,
    Package,
    Import,
    As,
    Val,
    Var,
    /// `mut` keyword. Reserved for the `&mut self` method receiver
    /// (Phase 1 of `&` references) and any future mutability
    /// annotations. Prior to its introduction the lexer treated
    /// `mut` as a regular identifier; a workspace grep confirmed
    /// no toylang source uses it that way before reservation.
    Mut,
    Const,
    With,
    Ambient,
    Enum,
    Match,
    Requires,
    Ensures,
    /// `type Name = Type` — top-level type alias declaration.
    /// The parser eagerly substitutes occurrences of `Name` in type
    /// positions with the alias target, so downstream layers see the
    /// fully-expanded type and need no special handling.
    Type,

    Bool,
    U64,
    I64,
    F64,
    F32,
    /// SIMD: one of the 128-bit vector type keywords (`f64x2` etc.).
    /// A single token kind carrying the type keeps the lexer / parser
    /// tables from growing a row per lane type.
    Vector(crate::type_decl::VectorType),
    USize,
    // Narrow integer keywords (NUM-W). Same surface shape as
    // U64/I64 — keyword + literal-suffix + value-carrying token
    // for parsed numeric literals (`42u8` / `0xFFi32`).
    U8,
    U16,
    U32,
    I8,
    I16,
    I32,
    Str,
    Ptr,
    Null,
    Dict,
    Self_,       // Self keyword

    ParenOpen,
    ParenClose,
    BraceOpen,
    BraceClose,
    BracketOpen,
    BracketClose,
    Comma,
    Dot,
    DotDot,      // ..
    DoubleColon,
    Colon,
    Semicolon,   // ;
    Arrow,       // ->
    FatArrow,    // =>
    Exclamation, // !
    At,          // @ — labelled-loop prefix (`@outer: while ...`, `break @outer`)
    Question,    // ? — postfix early-return operator (`expr?` for Result / Option)
    DoubleQuestion, // ?? — null-coalesce (`opt ?? default`, desugared to a lazy match)

    Equal,

    DoubleEqual, // ==
    NotEqual,    // !=
    LT,          // <
    LE,          // <=
    GT,          // >
    GE,          // >=

    DoubleAnd, // &&
    DoubleOr,  // ||
    And,       // &
    Or,        // |
    Xor,       // ^
    Tilde,     // ~
    LeftShift, // <<
    RightShift,// >>

    IAdd,
    ISub,
    IMul,
    IDiv,
    IMod,
    FAdd,
    FSub,
    FMul,
    FDiv,
    // Compound-assignment operators. Parser desugars these into
    // `lhs = lhs op rhs` so the AST stays small.
    PlusEqual,    // +=
    MinusEqual,   // -=
    StarEqual,    // *=
    SlashEqual,   // /=
    PercentEqual, // %=
    AndEqual,        // &=
    OrEqual,         // |=
    XorEqual,        // ^=
    LeftShiftEqual,  // <<=
    RightShiftEqual, // >>=

    Int64(i64),
    UInt64(u64),
    Float64(f64),
    Float32(f32),
    // Narrow numeric literal tokens (NUM-W). Each carries the
    // already-parsed value at its native width; the lexer
    // validates the suffixed text fits the range and falls back
    // to `Integer(text)` on overflow (mirrors the U64/I64 path).
    Int8(i8),
    Int16(i16),
    Int32(i32),
    UInt8(u8),
    UInt16(u16),
    UInt32(u32),
    /// A char literal — `'a'` / `'\n'` / `'\x41'` / `'\u{1F600}'`.
    /// Carries the code point in 32 bits: the literal's type is
    /// `u32` (the `char` alias), and it is the only integer literal
    /// a *narrower or wider* integer position may take without an
    /// `as` cast, provided the value fits — see
    /// `type_checker::coerce_char_literal`.
    CharLiteral(u32),
    String(String),
    /// String interpolation literal — `"hello {name}, sum={a + b}"`.
    /// Each `StringPart::Literal(s)` is a verbatim segment (escapes
    /// already processed); each `StringPart::Expr` is the raw
    /// source text inside `{...}` that the parser re-tokenizes and
    /// parses as a sub-expression, plus the optional format spec
    /// that followed a depth-0 `:`. `{{` / `}}` lex to literal `{` /
    /// `}` (Rust convention). At least one Expr part is present
    /// (otherwise the lexer emits a plain `String`).
    InterpolatedString(Vec<StringPart>),
    Integer(String),

    Identifier(String),
    True,
    False,

    Comment(String),

    NewLine,
    EOF,
}

impl Kind {
    /// Returns true if this token is a reserved keyword
    /// How a keyword is spelled in source, for a diagnostic that has
    /// to name it.
    ///
    /// A reserved word used as a name is one of the easiest mistakes
    /// to make and one of the worst to read about: `fn f(to: u64)`
    /// reported `ParenClose`, and `val to = 3u64` said "reserved
    /// keyword 'keyword'" — the catch-all arm of a match that had
    /// been written out by hand and never finished. One table, so
    /// every site says the same thing.
    pub fn keyword_spelling(&self) -> Option<&'static str> {
        let word = match self {
            Kind::If => "if",
            Kind::Elif => "elif",
            Kind::Else => "else",
            Kind::For => "for",
            Kind::In => "in",
            Kind::To => "to",
            Kind::While => "while",
            Kind::Loop => "loop",
            Kind::Break => "break",
            Kind::Continue => "continue",
            Kind::Class => "class",
            Kind::Struct => "struct",
            Kind::Trait => "trait",
            Kind::Impl => "impl",
            Kind::Function => "fn",
            Kind::Return => "return",
            Kind::Extern => "extern",
            Kind::Public => "pub",
            Kind::Val => "val",
            Kind::Var => "var",
            Kind::Mut => "mut",
            Kind::Const => "const",
            Kind::With => "with",
            Kind::Ambient => "ambient",
            Kind::Enum => "enum",
            Kind::Match => "match",
            Kind::Requires => "requires",
            Kind::Ensures => "ensures",
            Kind::Type => "type",
            Kind::Bool => "bool",
            Kind::U64 => "u64",
            Kind::I64 => "i64",
            Kind::F64 => "f64",
            Kind::F32 => "f32",
            Kind::USize => "usize",
            Kind::U8 => "u8",
            Kind::U16 => "u16",
            Kind::U32 => "u32",
            Kind::I8 => "i8",
            Kind::I16 => "i16",
            Kind::I32 => "i32",
            Kind::Str => "str",
            Kind::Ptr => "ptr",
            Kind::Null => "null",
            Kind::Dict => "dict",
            Kind::Self_ => "self",
            Kind::True => "true",
            Kind::False => "false",
            _ => return None,
        };
        Some(word)
    }

    /// What to say after naming a keyword that was used as a name.
    /// A few of them are easy to reach for by accident and have an
    /// obvious neighbour to suggest.
    pub fn keyword_hint(&self) -> Option<&'static str> {
        match self {
            Kind::To => Some("`to` is the range keyword (`for i in 0u64 to n`)"),
            Kind::In => Some("`in` belongs to `for x in ...`"),
            Kind::Type => Some("`type` declares an alias"),
            Kind::Self_ => Some("`self` is the receiver's name"),
            Kind::Match => Some("`match` starts a pattern match"),
            _ => None,
        }
    }

    /// The message for this reserved word written where a name
    /// goes. `position` is spelled as the reader would say it
    /// ("a parameter name"), and a keyword with an obvious
    /// neighbour adds the hint.
    pub fn as_name_error(&self, position: &str) -> Option<String> {
        let word = self.keyword_spelling()?;
        let mut out = format!("`{word}` is a keyword, so it cannot be {position}");
        if let Some(hint) = self.keyword_hint() {
            out.push_str(" — ");
            out.push_str(hint);
        }
        Some(out)
    }

    pub fn is_keyword(&self) -> bool {
        matches!(self, 
            Kind::If | Kind::Elif | Kind::Else | Kind::For | Kind::In | Kind::To | 
            Kind::While | Kind::Loop | Kind::Break | Kind::Continue | Kind::Class | Kind::Struct |
            Kind::Trait | Kind::Impl | Kind::Function | Kind::Return | Kind::Extern | Kind::Public |
            Kind::Val | Kind::Var | Kind::Mut | Kind::Const | Kind::With | Kind::Ambient | Kind::Enum | Kind::Match | Kind::Requires | Kind::Ensures | Kind::Type | Kind::Bool | Kind::U64 | Kind::I64 | Kind::F64 | Kind::F32 | Kind::Vector(_) | Kind::USize |
            Kind::U8 | Kind::U16 | Kind::U32 | Kind::I8 | Kind::I16 | Kind::I32 |
            Kind::Str | Kind::Ptr | Kind::Null | Kind::Dict | Kind::Self_ | Kind::True | Kind::False
        )
    }
}
