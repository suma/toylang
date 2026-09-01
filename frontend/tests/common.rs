//! Helpers shared by the type-checking integration tests.
//!
//! Six test files carried byte-identical copies of these: three of the
//! declaration-aware form, three of the functions-only form. They are the
//! same two helpers, so they live here once.
//!
//! The two are *not* interchangeable, which is why both stayed. The
//! functions-only form type-checks a program's functions against an empty
//! struct/impl registry; a test that wants `struct` and `impl` bodies
//! visible to method resolution needs the declaration-aware one. Whether
//! a given test wants the declarations registered is part of what it is
//! asserting, so the choice is spelled out at each call site rather than
//! unified away.

#![allow(dead_code)]

use frontend::ast::{Stmt, StmtRef};
use frontend::type_checker::TypeCheckerVisitor;
use frontend::ParserWithInterner;

/// Parse `source` and type-check its functions.
///
/// Struct and impl declarations are *not* visited first, so methods and
/// generic parameters they would register are not available.
pub fn type_check_functions(source: &str) -> Result<(), String> {
    let mut parser = ParserWithInterner::new(source);
    match parser.parse_program() {
        Ok(mut program) => {
            if program.statement.is_empty() && program.function.is_empty() {
                return Err("No statements or functions found".to_string());
            }

            let functions = program.function.clone();
            let string_interner = parser.get_string_interner();
            let mut type_checker =
                TypeCheckerVisitor::with_program(&mut program, string_interner);
            collect(&mut type_checker, &functions)
        }
        Err(e) => Err(format!("Parse error: {:?}", e)),
    }
}

/// Parse `source`, visit its `StructDecl` / `ImplBlock` statements so
/// their generic params and methods are registered, then type-check the
/// functions.
pub fn type_check_with_declarations(source: &str) -> Result<(), String> {
    let mut parser = ParserWithInterner::new(source);
    match parser.parse_program() {
        Ok(mut program) => {
            if program.statement.is_empty() && program.function.is_empty() {
                return Err("No statements or functions found".to_string());
            }

            let functions = program.function.clone();
            let stmt_count = program.statement.len();
            let string_interner = parser.get_string_interner();
            let mut type_checker =
                TypeCheckerVisitor::with_program(&mut program, string_interner);

            for i in 0..stmt_count {
                let stmt_ref = StmtRef(i as u32);
                let should_visit = type_checker
                    .core
                    .stmt_pool
                    .get(&stmt_ref)
                    .map(|stmt| {
                        matches!(stmt, Stmt::StructDecl { .. } | Stmt::ImplBlock { .. })
                    })
                    .unwrap_or(false);
                if should_visit
                    && let Err(e) = type_checker.visit_stmt(&stmt_ref) {
                        return Err(spell(&type_checker, &e));
                    }
            }

            collect(&mut type_checker, &functions)
        }
        Err(e) => Err(format!("Parse error: {:?}", e)),
    }
}

/// Render an error the way a user sees it.
///
/// DIAG-SYMBOL-NAME: the derived `Debug` these helpers used to print
/// dumps the error struct verbatim, so a type came out as
/// `Struct(SymbolU32 { value: 60 }, [])` and a test failure said
/// nothing about which type it meant. `message_with` is the same
/// rendering the driver uses.
fn spell(type_checker: &TypeCheckerVisitor<'_>, error: &frontend::type_checker::TypeCheckError) -> String {
    error.message_with(Some(type_checker.core.string_interner))
}

/// Type-check every function, joining the failures so a test sees all of
/// them rather than only the first.
fn collect(
    type_checker: &mut TypeCheckerVisitor<'_>,
    functions: &[std::rc::Rc<frontend::ast::Function>],
) -> Result<(), String> {
    let mut errors = Vec::new();
    for func in functions.iter() {
        if let Err(e) = type_checker.type_check(func.clone()) {
            errors.push(spell(type_checker, &e));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}
