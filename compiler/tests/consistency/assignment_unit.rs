//! An assignment produces no value, and a block that ends in one is
//! `()`.
//!
//! `visit_assign` used to answer the assigned value's type, so a block
//! ending in an assignment carried it. Two things followed. A `match`
//! whose arms wrote to an accumulator was rejected —
//! "arm 0 is i64, arm 1 is ()" — naming a type the author never wrote.
//! And `fn f() -> u64 { a = 5u64 }` compiled, returning 5, though the
//! spec never granted assignment a value: it cannot appear in an
//! expression position (`val x = (a = b)` is a parse error) and
//! `a = b = c` does not run. The value was observable only as a
//! block's tail, which is the accident.
//!
//! `a[i] = v` already answered `()`, so this is one form joining the
//! other rather than a new rule.

use super::harness::*;

/// The shape that motivated the change: arms that write, and nothing
//  else, on both sides.
#[test]
fn match_arms_that_only_assign_are_unit() {
    let src = r#"
        fn main() -> u64 {
            var acc: u64 = 0u64
            val x: Option<u64> = Option::Some(5u64)
            match x {
                Option::Some(v) => { acc = acc + v }
                Option::None => {}
            }
            val y: Option<u64> = Option::None
            match y {
                Option::Some(v) => { acc = acc + v }
                Option::None => {}
            }
            println(acc)
            0u64
        }
    "#;
    assert_eq!(interpreter_stdout(src, "assign_unit_match", true), "5\n");
    assert_stdout_consistent(src, "assign_unit_match");
}

/// The same for `if` / `else`, and for the other assignment forms —
/// a field write, an index write, and a compound operator (which
/// desugars to `a = a OP b` before the checker sees it).
#[test]
fn every_assignment_form_is_unit_in_a_branch() {
    let src = r#"
        struct P { v: u64 }
        fn main() -> u64 {
            var p = P { v: 1u64 }
            var a: [u64; 2] = [0u64, 0u64]
            var n: u64 = 0u64
            if n == 0u64 { p.v = 7u64 } else { p.v = 9u64 }
            if n == 0u64 { a[0u64] = 3u64 } else { a[1u64] = 4u64 }
            if n == 0u64 { n += 2u64 } else { n -= 1u64 }
            println(p.v)
            println(a[0u64])
            println(n)
            0u64
        }
    "#;
    assert_eq!(
        interpreter_stdout(src, "assign_unit_forms", true),
        "7\n3\n2\n"
    );
    assert_stdout_consistent(src, "assign_unit_forms");
}

/// A `var` shadowed in a nested block gives the outer binding back.
///
/// `interpreter/example/scope.t` is this program, and it ended on the
/// assignment rather than reading `x` afterwards — so while an
/// assignment's value was the function's result, no engine was ever
/// asked what `x` held once the block closed. The interpreter JIT
/// answered 1011: its binding maps are keyed by name and were never
/// unwound, so the inner `var x` replaced the outer one for good.
#[test]
fn a_var_shadowed_in_a_nested_block_is_restored() {
    let src = r#"
        fn main() -> u64 {
            var x = 100u64
            println(x)
            {
                var x = 10u64
                x = x + 1000u64
                println(x)
            }
            println(x)
            x = x + 1u64
            println(x)
            0u64
        }
    "#;
    assert_eq!(
        interpreter_stdout(src, "shadow_restore", true),
        "100\n1010\n100\n101\n"
    );
    assert_stdout_consistent(src, "shadow_restore");
    // `assert_stdout_consistent` short-circuits on its lite path, which
    // does not include the *interpreter's* JIT — the engine that was
    // wrong. This one asserts it compiled rather than fell back.
    assert_jit_compiled_and_matches(src, "shadow_restore_jit");
}

/// Shadowing the other binding shapes, so the restore covers every
/// per-name map and not just the scalar one.
#[test]
fn a_shadowed_struct_or_tuple_binding_is_restored() {
    let src = r#"
        struct P { v: u64 }
        fn main() -> u64 {
            val p = P { v: 1u64 }
            val t = (2u64, 3u64)
            {
                val p = P { v: 40u64 }
                val t = (50u64, 60u64)
                println(p.v + t.0 + t.1)
            }
            println(p.v + t.0 + t.1)
            0u64
        }
    "#;
    assert_eq!(
        interpreter_stdout(src, "shadow_compound", true),
        "150\n6\n"
    );
    assert_stdout_consistent(src, "shadow_compound");
}
