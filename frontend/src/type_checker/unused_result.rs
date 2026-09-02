//! MUST-USE: warn when a statement produces a `Result` and drops it.
//!
//! `?` gave failures a way to travel; nothing made ignoring one
//! visible. A call whose value is thrown away compiles silently:
//!
//! ```text
//! fn main() -> u64 {
//!     io::write_file("out.txt", body)   # the disk was full
//!     0u64
//! }
//! ```
//!
//! The program reports success. That is the failure mode `Result`
//! exists to prevent, and the only thing standing between a reader and
//! it is remembering to look.
//!
//! ## What counts as discarded
//!
//! A `Stmt::Expression` that is **not** the last statement of its
//! block. A block's last statement is its value, so it is not
//! discarded here — whether the enclosing position wants that value is
//! a question this pass cannot answer from the block alone.
//!
//! That rule is deliberately narrow. Every case it reports is one
//! where the value provably goes nowhere, so there is no judgement
//! call and no need for an escape hatch beyond the one the language
//! already has: bind it (`val _unused = ...`), which says in the
//! source that the result was considered.
//!
//! ## Why only `Result`
//!
//! An ignored `Option` is usually a lookup whose absence is the
//! answer; an ignored `Result` is an unreported failure. Rust marks
//! both `#[must_use]` and pays for it in `let _ =` noise. This is the
//! half that catches bugs.
//!
//! ## Why a warning
//!
//! Ignoring a failure can be deliberate — a best-effort write on a
//! shutdown path, say — and there was no way to say so until now.
//! A warning names the site without refusing the program.

use std::collections::HashMap;

use string_interner::DefaultStringInterner;

use crate::ast::{Expr, ExprRef, File, Stmt};
use crate::type_decl::TypeDecl;
use crate::type_checker::error::TypeCheckError;

/// The enum whose values must not be dropped on the floor.
const MUST_USE_ENUM: &str = "Result";

pub fn check_unused_results(
    program: &File,
    interner: &DefaultStringInterner,
    expr_types: &HashMap<ExprRef, TypeDecl>,
) -> Vec<TypeCheckError> {
    let mut warnings = Vec::new();
    for index in 0..program.expression.len() {
        let expr_ref = ExprRef(index as u32);
        let Some(Expr::Block(stmts)) = program.expression.get(&expr_ref) else {
            continue;
        };
        // The last statement is the block's value, so it is not
        // discarded here.
        let discarded = stmts.len().saturating_sub(1);
        for stmt_ref in stmts.iter().take(discarded) {
            let Some(Stmt::Expression(inner)) = program.statement.get(stmt_ref) else {
                continue;
            };
            let Some(ty) = expr_types.get(&inner) else {
                continue;
            };
            if !is_must_use(ty, interner) {
                continue;
            }
            let error = TypeCheckError::unused_result(
                ty.spell_with(Some(interner)),
                describe(program, interner, &inner),
            );
            warnings.push(match program.location_pool.get_expr_location(&inner) {
                Some(location) => error.with_location(*location),
                None => error,
            });
        }
    }
    warnings
}

fn is_must_use(ty: &TypeDecl, interner: &DefaultStringInterner) -> bool {
    match ty {
        TypeDecl::Enum(name, _) | TypeDecl::Struct(name, _) | TypeDecl::Identifier(name) => {
            interner.resolve(*name) == Some(MUST_USE_ENUM)
        }
        _ => false,
    }
}

/// What the message calls the discarded expression. Naming the call
/// is most of the value — "a call produces `Result`" sends the reader
/// looking, `the call `write_file(...)`` does not.
fn describe(program: &File, interner: &DefaultStringInterner, expr_ref: &ExprRef) -> String {
    let name = |sym| interner.resolve(sym).unwrap_or("?");
    match program.expression.get(expr_ref) {
        Some(Expr::Call(fn_name, _)) => format!("the call `{}(...)`", name(fn_name)),
        Some(Expr::AssociatedFunctionCall(owner, fn_name, _)) => {
            format!("the call `{}::{}(...)`", name(owner), name(fn_name))
        }
        Some(Expr::MethodCall(_, method, _)) => {
            format!("the method call `.{}(...)`", name(method))
        }
        _ => "this expression".to_string(),
    }
}
