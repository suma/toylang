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
    Val,
    Var,

    Bool,
    U64,
    I64,
    USize,
    Str,
    Ptr,
    Null,

    ParenOpen,
    ParenClose,
    BraceOpen,
    BraceClose,
    BracketOpen,
    BracketClose,
    Comma,
    Dot,
    DoubleColon,
    Colon,
    Semicolon,   // ;
    Arrow,       // ->
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

    IAdd,
    ISub,
    IMul,
    IDiv,
    FAdd,
    FSub,
    FMul,
    FDiv,

    Int64(i64),
    UInt64(u64),
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
            Kind::Val | Kind::Var | Kind::Bool | Kind::U64 | Kind::I64 | Kind::USize | 
            Kind::Str | Kind::Ptr | Kind::Null | Kind::True | Kind::False
        )
    }
}
