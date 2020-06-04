#[derive(Debug, PartialEq)]
pub enum Token {
    If,
    Else,
    For,
    While,
    Class,
    Function,
    Val,
    Var,

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

    Equal,

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
}
