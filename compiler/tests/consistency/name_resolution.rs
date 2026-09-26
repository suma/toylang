//! STDLIB-FN-SHADOWED-BY-USER-FN: whose function a bare name means.
//!
//! `import` makes a module's names visible to the program. It does
//! not make the program's names visible to the module — a module was
//! written without any knowledge of what would import it, and a bare
//! call in its body means the function *it* declares.
//!
//! Before this rule, a user function shadowed the module's from the
//! inside. The loud half was a stdlib body type-checked against the
//! wrong signature, reported against a line of the user's file that
//! had nothing to do with it. The quiet half is what this file is
//! for: where the signatures happened to match, the wrong function
//! was **called**, on every lane, with no diagnostic anywhere.

use super::harness::*;

#[test]
fn a_stdlib_body_calls_its_own_helper() {
    // `core/std/time.t` formats through its own `pad2_field(u32)`.
    // A user `pad2_field` of a different type used to be picked up
    // by that body: the type checker reported an argument mismatch
    // *in the user's file*, at a line number the file does not have.
    //
    // The user's own call still reaches the user's function — the
    // rule is about which body is asking, not about hiding names.
    let src = r#"
        fn pad2_field(n: u64) -> u64 { n * 2u64 }

        fn main() -> u64 {
            val dt: DateTime = time::DateTime::from_unix(1700000000i64)
            println(dt.to_str())
            println(pad2_field(21u64))
            0u64
        }
    "#;
    assert_renders(src, "stdlib_helper_not_shadowed", "2023-11-14T22:13:20Z\n42\n");
}

#[test]
fn a_user_function_still_wins_its_own_bare_call() {
    // The other direction, so the fix cannot be "modules always win".
    // `size` is a name the stdlib uses widely; a program that
    // declares one gets its own.
    let src = r#"
        fn size(n: u64) -> u64 { n + 7u64 }

        fn main() -> u64 {
            println(size(1u64))
            0u64
        }
    "#;
    assert_renders(src, "user_bare_call_wins", "8\n");
}

/// QUALIFIER-BARE-FALLBACK: a qualifier that names a module which does
/// not have the function is an error, not a detour. `hex::abs(-3i64)`
/// used to fall back to the bare name, ran `std::math::abs` and
/// answered 3 on every lane -- the qualifier said "hex's", the answer
/// came from somewhere else.
#[test]
fn a_qualifier_does_not_fall_back_to_another_modules_function() {
    let src = r#"
        fn main() -> u64 {
            val x = hex::abs(-3i64)
            x as u64
        }
    "#;
    let errors = type_check_errors(src);
    assert!(
        errors.iter().any(|e| e.contains("module 'hex' has no exported function 'abs'")),
        "{errors:?}"
    );
    // The right module still answers.
    let ok = r#"
        fn main() -> u64 {
            val x = math::abs(-3i64)
            x as u64
        }
    "#;
    assert_eq!(interpreter_value(ok), 3);
    assert_consistent(ok, "qualified_abs");
}
