//! DIAG-SYMBOL-NAME-LOWER: how a lowering diagnostic names things.
//!
//! Lowering builds its messages by hand with `format!`, and reaching
//! for `{:?}` on an interned symbol, an IR type or an AST node prints
//! the compiler's private vocabulary at a reader who wrote none of it:
//!
//! ```text
//! compiler MVP cannot lower expression yet: QualifiedIdentifier([SymbolU32 { value: 60 }, SymbolU32 { value: 61 }])
//! enum `Shape::Circle` has unsupported payload type `Struct(SymbolU32 { value: 71 }, [])`
//! ```
//!
//! The type checker settled this in 2026-09-01 (DIAG-SYMBOL-NAME) by
//! routing every hand-built message through interner-aware spellings.
//! These are the same spellings for the lowering pass's three
//! vocabularies: the IR's `Type`, the frontend's `TypeDecl`, and the
//! AST itself.
//!
//! The rule is enforced, not remembered:
//! `frontend/tests/diagnostic_spelling_tests.rs` scans both source
//! trees for `{:?}` and fails on anything not marked
//! `DIAG-DEBUG-FMT-OK`.

use frontend::ast::{Expr, Pattern};
use frontend::type_decl::TypeDecl;
use string_interner::{DefaultStringInterner, DefaultSymbol};

use crate::ir::{Module, Type};

/// One interned name, or `?` when the interner has never seen it.
pub(crate) fn name(interner: &DefaultStringInterner, sym: DefaultSymbol) -> &str {
    interner.resolve(sym).unwrap_or("?")
}

/// How a diagnostic should spell an IR type.
///
/// `Struct` / `Tuple` / `Enum` carry ids into `module`, so this
/// resolves them there and says `Option` rather than
/// `Enum(EnumId(7))`.
pub(crate) fn spell_type(module: &Module, interner: &DefaultStringInterner, ty: Type) -> String {
    match ty {
        Type::I64 => "i64".to_string(),
        Type::U64 => "u64".to_string(),
        Type::I8 => "i8".to_string(),
        Type::U8 => "u8".to_string(),
        Type::I16 => "i16".to_string(),
        Type::U16 => "u16".to_string(),
        Type::I32 => "i32".to_string(),
        Type::U32 => "u32".to_string(),
        Type::F64 => "f64".to_string(),
        Type::F32 => "f32".to_string(),
        Type::Bool => "bool".to_string(),
        Type::Unit => "()".to_string(),
        Type::Str => "str".to_string(),
        Type::Vector(v) => crate::types::ir_to_vector(v).source_name().to_string(),
        Type::Struct(id) => module
            .struct_defs
            .get(id.0 as usize)
            .map(|d| name(interner, d.base_name).to_string())
            .unwrap_or_else(|| "struct".to_string()),
        Type::Enum(id) => module
            .enum_defs
            .get(id.0 as usize)
            .map(|d| name(interner, d.base_name).to_string())
            .unwrap_or_else(|| "enum".to_string()),
        Type::Tuple(id) => match module.tuple_defs.get(id.0 as usize) {
            Some(elems) => {
                let elems = elems.clone();
                let parts: Vec<String> = elems
                    .iter()
                    .map(|e| spell_type(module, interner, *e))
                    .collect();
                format!("({})", parts.join(", "))
            }
            None => "tuple".to_string(),
        },
    }
}

/// How a diagnostic should spell a frontend type — the source
/// spelling (`Vec<u8>`, `&mut Point`), not the AST's Debug form.
pub(crate) fn spell_type_decl(interner: &DefaultStringInterner, ty: &TypeDecl) -> String {
    ty.spell_with(Some(interner))
}

/// How a diagnostic should refer to an expression the lowering pass
/// could not handle.
///
/// Where the node carries names, they are spelled — `Color::Red`,
/// `Point { .. }`, `f(...)` — because that is what the reader is
/// looking for in their own source. The rest name the *form*, since
/// "an `if` expression" locates the problem better than a Debug dump
/// of the whole subtree ever did.
pub(crate) fn describe_expr(interner: &DefaultStringInterner, expr: &Expr) -> String {
    match expr {
        Expr::Identifier(sym) => format!("`{}`", name(interner, *sym)),
        Expr::QualifiedIdentifier(path) => {
            let parts: Vec<&str> = path.iter().map(|s| name(interner, *s)).collect();
            format!("`{}`", parts.join("::"))
        }
        Expr::Call(fn_name, _) => format!("the call `{}(...)`", name(interner, *fn_name)),
        Expr::AssociatedFunctionCall(owner, fn_name, _) => format!(
            "the call `{}::{}(...)`",
            name(interner, *owner),
            name(interner, *fn_name)
        ),
        Expr::MethodCall(_, method, _) => {
            format!("the method call `.{}(...)`", name(interner, *method))
        }
        Expr::BuiltinMethodCall(_, method, _) => {
            // DIAG-DEBUG-FMT-OK: `BuiltinMethod` is a closed set of
            // builtin names whose Debug spelling is the name itself.
            format!("the builtin method `{method:?}`")
        }
        Expr::BuiltinCall(func, _) => {
            // DIAG-DEBUG-FMT-OK: as above, for builtin functions.
            format!("the builtin `{func:?}`")
        }
        Expr::FieldAccess(_, field) => format!("the field access `.{}`", name(interner, *field)),
        Expr::TupleAccess(_, index) => format!("the tuple access `.{index}`"),
        Expr::StructLiteral(struct_name, _) => {
            format!("the struct literal `{} {{ .. }}`", name(interner, *struct_name))
        }
        Expr::String(sym) => format!("the string literal `\"{}\"`", name(interner, *sym)),
        Expr::Number(sym) => format!("the literal `{}`", name(interner, *sym)),
        Expr::Int64(v) => format!("the literal `{v}i64`"),
        Expr::UInt64(v) => format!("the literal `{v}u64`"),
        Expr::Int8(v) => format!("the literal `{v}i8`"),
        Expr::Int16(v) => format!("the literal `{v}i16`"),
        Expr::Int32(v) => format!("the literal `{v}i32`"),
        Expr::UInt8(v) => format!("the literal `{v}u8`"),
        Expr::UInt16(v) => format!("the literal `{v}u16`"),
        Expr::UInt32(v) | Expr::CharLiteral(v) => format!("the literal `{v}u32`"),
        Expr::Float64(v) => format!("the literal `{v}f64`"),
        Expr::Float32(v) => format!("the literal `{v}f32`"),
        Expr::True => "the literal `true`".to_string(),
        Expr::False => "the literal `false`".to_string(),
        Expr::Null => "the literal `null`".to_string(),
        Expr::Assign(..) => "an assignment".to_string(),
        Expr::IfElifElse(..) => "an `if` expression".to_string(),
        Expr::Match(..) => "a `match` expression".to_string(),
        Expr::Block(..) => "a block".to_string(),
        Expr::Binary(op, _, _) => {
            // DIAG-DEBUG-FMT-OK: `Operator`'s Debug spelling is the
            // operator's own name (`Add`, `LogicalAnd`).
            format!("a `{op:?}` operation")
        }
        Expr::Unary(op, _) => {
            // DIAG-DEBUG-FMT-OK: as above, for unary operators.
            format!("a unary `{op:?}` operation")
        }
        Expr::Cast(_, ty) => format!("a cast to `{}`", spell_type_decl(interner, ty)),
        Expr::ArrayLiteral(..) => "an array literal".to_string(),
        Expr::TupleLiteral(..) => "a tuple literal".to_string(),
        Expr::DictLiteral(..) => "a dict literal".to_string(),
        Expr::ExprList(..) => "an expression list".to_string(),
        Expr::SliceAccess(..) => "an index / slice access".to_string(),
        Expr::SliceAssign(..) => "an index / slice assignment".to_string(),
        Expr::Range(..) => "a range".to_string(),
        Expr::With(..) => "a `with allocator` expression".to_string(),
        Expr::Closure { .. } => "a closure literal".to_string(),
        Expr::Try { .. } => "a `?` expression".to_string(),
        Expr::NullCoalesce { .. } => "a `??` expression".to_string(),
        Expr::StructUpdate { type_name, .. } => format!(
            "the struct update `{} {{ ..base }}`",
            name(interner, *type_name)
        ),
    }
}

/// How a diagnostic should refer to a pattern. Same principle as
/// [`describe_expr`]: spell what the reader wrote where the node
/// carries it, name the form otherwise.
pub(crate) fn describe_pattern(interner: &DefaultStringInterner, pattern: &Pattern) -> String {
    match pattern {
        Pattern::Wildcard => "`_`".to_string(),
        Pattern::Name(sym) => format!("the binding `{}`", name(interner, *sym)),
        Pattern::EnumVariant(enum_name, variant_name, _) => format!(
            "the pattern `{}::{}`",
            name(interner, *enum_name),
            name(interner, *variant_name)
        ),
        Pattern::Struct(type_name, _, _) => {
            format!("the pattern `{} {{ .. }}`", name(interner, *type_name))
        }
        Pattern::Tuple(..) => "a tuple pattern".to_string(),
        Pattern::Literal(..) => "a literal pattern".to_string(),
        Pattern::Range(..) => "a range pattern".to_string(),
        Pattern::Binding(sym, _) => format!("the `@` pattern `{}`", name(interner, *sym)),
    }
}
