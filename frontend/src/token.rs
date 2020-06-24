#[derive(Debug, PartialEq, Clone)]
pub enum Token {
    If,
    Else,
    For,
    While,
    Class,
    Struct,
    Function,
    Return,
    Extern,
    Public,
    Val,
    Var,

    U64,
    I64,
    USize,
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
    Arrow, // ->

    Equal,

    DoubleEqual, // ==
    NotEqual,    // !=
    LT,          // <
    LE,          // <=
    GT,          // >
    GE,          // >=

    DoubleAnd, // &&
    DoubleOr,  // ||

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
    Integer(String),

    Identifier(String),

    NewLine,
    EOF,
}
