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
    Underscore,  // _
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

    NewLine,
    EOF,
}
