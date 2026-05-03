#[derive(Debug, PartialEq, Clone)]
pub struct Token {
    pub kind: Kind,
    pub position: std::ops::Range<usize>,
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
    Break,
    Continue,
    Class,
    Struct,
    Trait,
    Impl,
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

    Bool,
    U64,
    I64,
    F64,
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

    Int64(i64),
    UInt64(u64),
    Float64(f64),
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
    String(String),
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
    pub fn is_keyword(&self) -> bool {
        matches!(self, 
            Kind::If | Kind::Elif | Kind::Else | Kind::For | Kind::In | Kind::To | 
            Kind::While | Kind::Break | Kind::Continue | Kind::Class | Kind::Struct |
            Kind::Trait | Kind::Impl | Kind::Function | Kind::Return | Kind::Extern | Kind::Public |
            Kind::Val | Kind::Var | Kind::Mut | Kind::Const | Kind::With | Kind::Ambient | Kind::Enum | Kind::Match | Kind::Requires | Kind::Ensures | Kind::Bool | Kind::U64 | Kind::I64 | Kind::F64 | Kind::USize |
            Kind::U8 | Kind::U16 | Kind::U32 | Kind::I8 | Kind::I16 | Kind::I32 |
            Kind::Str | Kind::Ptr | Kind::Null | Kind::Dict | Kind::Self_ | Kind::True | Kind::False
        )
    }
}
