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
            Kind::Impl | Kind::Function | Kind::Return | Kind::Extern | Kind::Public | 
            Kind::Val | Kind::Var | Kind::With | Kind::Ambient | Kind::Enum | Kind::Match | Kind::Requires | Kind::Ensures | Kind::Bool | Kind::U64 | Kind::I64 | Kind::F64 | Kind::USize |
            Kind::Str | Kind::Ptr | Kind::Null | Kind::Dict | Kind::Self_ | Kind::True | Kind::False
        )
    }
}
