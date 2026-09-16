use crate::common::{
    test_program, test_program_no_core, test_program_with_core, test_program_with_core_modules,
};

// ============================================================================
// Module system tests
// ============================================================================

#[test]
fn test_module_package_declaration() {
    let source = r"
        package math

        fn main() -> u64 {
            42u64
        }
        ";

    let result = test_program(source);
    assert!(result.is_ok(), "File with package declaration should run");
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 42);
}

#[test]
fn test_module_auto_load_no_import_needed() {
    // Core modules in the configured `core/` dir are integrated
    // automatically — no `import math` line required. The
    // qualified form `math::name(...)` still resolves through the
    // synthetic `ImportDecl` the auto-load path inserts.
    // `min_i64(7, 35) + max_i64(7, 35) = 7 + 35 = 42`.
    let source = r"
        fn main() -> u64 {
            (math::min_i64(7i64, 35i64) + math::max_i64(7i64, 35i64)) as u64
        }
        ";

    let result = test_program_with_core_modules(source);
    assert!(
        result.is_ok(),
        "math::* should resolve via auto-load: {:?}",
        result.err()
    );
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 42);
}

#[test]
fn test_extern_fn_declaration_type_checks() {
    // Phase 1 of the math externalisation work: `extern fn`
    // declarations parse + type-check (signature only — no body).
    // Calling them is still a runtime error because the
    // backend dispatch doesn't exist yet (lands in Phase 2).
    let source = r"
        extern fn extern_sin(x: f64) -> f64

        fn main() -> u64 {
            42u64
        }
        ";
    let result = test_program(source);
    assert!(result.is_ok(), "extern fn declaration should parse + type-check: {:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 42);
}

#[test]
fn test_extern_fn_call_dispatches_to_registry() {
    // Phase 2: extern fn call is now wired up to the interpreter's
    // extern fn registry. `extern_cos(0f64)` resolves to f64::cos
    // and returns 1.0 — `(r * 7.0) as u64` -> 7.
    let source = r"
        extern fn extern_cos(x: f64) -> f64

        fn main() -> u64 {
            val r: f64 = extern_cos(0f64)
            (r * 7f64) as u64
        }
        ";
    let result = test_program(source);
    assert!(result.is_ok(), "extern fn call should dispatch: {:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 7);
}

#[test]
fn test_extern_fn_unregistered_errors_cleanly() {
    // An extern fn whose name has no Rust impl in the registry must
    // still surface the targeted "not yet implemented" diagnostic
    // (i.e. unknown extern fns don't silently return Unit).
    let source = r"
        extern fn extern_unknown_xyz(x: f64) -> f64

        fn main() -> u64 {
            val r: f64 = extern_unknown_xyz(0f64)
            r as u64
        }
        ";
    let result = test_program(source);
    assert!(result.is_err(), "unknown extern fn call should error: {:?}", result.ok());
    let err = format!("{:?}", result.err().unwrap());
    assert!(
        err.contains("extern fn") && err.contains("not yet implemented"),
        "diagnostic should mention extern fn + not-implemented, got: {}",
        err
    );
}

#[test]
fn test_value_method_i64_abs() {
    // `x.abs()` should call the built-in `i64.abs()` method and
    // return `wrapping_abs(x)` semantics — `i64::MIN` stays at
    // `i64::MIN` instead of panicking.
    let source = r"
        fn main() -> u64 {
            val n: i64 = -42i64
            n.abs() as u64
        }
        ";
    let result = test_program(source);
    assert!(result.is_ok(), "x.abs() should run: {:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 42);
}

#[test]
fn test_value_method_f64_abs() {
    // `x.abs()` on an f64 should call the IEEE 754 fabs (sign-bit
    // flip; preserves NaN). C's `fabs` semantics.
    let source = r"
        fn main() -> u64 {
            val x: f64 = -7.5f64
            (x.abs() * 2f64) as u64
        }
        ";
    let result = test_program(source);
    assert!(result.is_ok(), "f64.abs() should run: {:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 15);
}

#[test]
fn test_builtin_abs_polymorphic_f64() {
    // `__builtin_abs(x)` is polymorphic: i64 -> wrapping_abs,
    // f64 -> IEEE 754 fabs. Mirrors C's `abs` / `fabs` distinction
    // in a single user-facing intrinsic.
    let source = r"
        fn main() -> u64 {
            val x: f64 = -3.5f64
            (__builtin_abs(x) * 2f64) as u64
        }
        ";
    let result = test_program(source);
    assert!(result.is_ok(), "__builtin_abs(f64) should run: {:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 7);
}

#[test]
fn test_value_method_f64_sqrt() {
    // `x.sqrt()` should call the built-in `f64.sqrt()` method
    // (IEEE 754) and return the principal root.
    let source = r"
        fn main() -> u64 {
            val r: f64 = 81f64
            r.sqrt() as u64
        }
        ";
    let result = test_program(source);
    assert!(result.is_ok(), "x.sqrt() should run: {:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 9);
}

#[test]
fn test_module_qualified_call_executes() {
    // Regression test for the module integration fix
    // (`update_with_remapped_content` used to leave imported function
    // bodies as `Stmt::Break` placeholders). With the fix in place,
    // calling an imported `pub fn` via the qualified `module::func`
    // form must execute the real body and return the right value.
    // No `import math` line — the auto-load path picks math up from
    // the configured `core/` directory. `math::abs(-30) = 30`.
    let source = r"
        fn main() -> u64 {
            math::abs(-30i64) as u64
        }
        ";

    let result = test_program_with_core_modules(source);
    assert!(
        result.is_ok(),
        "Qualified module call should execute: {:?}",
        result.err()
    );
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 30);
}

#[test]
fn test_module_package_and_no_import_needed() {
    // `package main` declaration alongside auto-loaded core modules
    // — confirms the package directive doesn't disturb the
    // auto-load path's synthetic ImportDecl insertion.
    let source = r"
        package main

        fn main() -> u64 {
            42u64
        }
        ";

    let result = test_program_with_core_modules(source);
    assert!(result.is_ok(), "File with package + auto-load should run");
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 42);
}

// ============================================================================
// Property-based tests (arithmetic, comparison, logical)
// ============================================================================

#[test]
fn test_arithmetic_properties_extended() {
    // Test arithmetic properties with different values
    let test_cases = vec![
        (10i64, 20i64, "+", 30i64),
        (100i64, 50i64, "-", 50i64),
        (7i64, 8i64, "*", 56i64),
        (21i64, 3i64, "/", 7i64),
    ];

    for (a, b, op, expected) in test_cases {
        let program = format!(r"
        fn main() -> i64 {{
            {}i64 {} {}i64
        }}
        ", a, op, b);

        let res = test_program_no_core(&program);
        assert!(res.is_ok(), "Failed for {} {} {}", a, op, b);
        assert_eq!(res.unwrap().borrow().unwrap_int64(), expected);
    }
}

#[test]
fn test_comparison_properties_extended() {
    // Test comparison properties
    let test_cases = vec![
        (10i64, 20i64, "<", true),
        (20i64, 10i64, ">", true),
        (15i64, 15i64, "==", true),
        (10i64, 20i64, "!=", true),
        (25i64, 20i64, ">=", true),
        (15i64, 20i64, "<=", true),
    ];

    for (a, b, op, expected) in test_cases {
        let program = format!(r"
        fn main() -> bool {{
            {}i64 {} {}i64
        }}
        ", a, op, b);

        let res = test_program_no_core(&program);
        assert!(res.is_ok(), "Failed for {} {} {}", a, op, b);
        assert_eq!(res.unwrap().borrow().unwrap_bool(), expected);
    }
}

#[test]
fn test_logical_operations() {
    let test_cases = vec![
        (true, "&&", true, true),
        (true, "&&", false, false),
        (false, "&&", true, false),
        (false, "&&", false, false),
        (true, "||", true, true),
        (true, "||", false, true),
        (false, "||", true, true),
        (false, "||", false, false),
    ];

    for (a, op, b, expected) in test_cases {
        let program = format!(r"
        fn main() -> bool {{
            {} {} {}
        }}
        ", a, op, b);

        // No stdlib symbols here — skip the ~600 ms core-modules load
        // that `test_program` would otherwise do for every iteration.
        let res = test_program_no_core(&program);
        assert!(res.is_ok(), "Failed for {} {} {}", a, op, b);
        assert_eq!(res.unwrap().borrow().unwrap_bool(), expected);
    }
}

// ============================================================================
// MODULE-SYSTEM P2: the qualifier is the module's full path, matched by
// suffix.
//
// Before P2 the qualifier was the leaf file name alone, so two modules
// in different directories with the same file name shared one table
// slot: the type checker resolved to whichever was registered last and
// the IR builder aborted with `function_index collision`. These tests
// build a throwaway core tree to pin the three outcomes.
// ============================================================================

/// Write a core-modules tree: `(relative path, source)` pairs under a
/// fresh temp dir. Returned dir must outlive the run.
fn core_tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (rel, src) in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir");
        std::fs::write(&path, src).expect("write module");
    }
    dir
}

#[test]
fn module_qualifier_matches_the_tail_of_the_path() {
    // `core/std/a/dup.t` is reachable as `dup::f()` — one segment of a
    // three-segment path (`std.a.dup`).
    let core = core_tree(&[("std/a/dup.t", "pub fn f() -> u64 { 7u64 }\n")]);
    let result = test_program_with_core(
        "fn main() -> u64 { dup::f() }",
        Some(core.path().to_path_buf()),
    );
    assert!(result.is_ok(), "dup::f() should resolve: {:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 7);
}

#[test]
fn same_leaf_name_different_functions_both_resolve() {
    let core = core_tree(&[
        ("std/a/dup.t", "pub fn f() -> u64 { 1u64 }\n"),
        ("std/b/dup.t", "pub fn g() -> u64 { 2u64 }\n"),
    ]);
    let result = test_program_with_core(
        "fn main() -> u64 { dup::f() + dup::g() }",
        Some(core.path().to_path_buf()),
    );
    assert!(result.is_ok(), "both should resolve: {:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 3);
}

#[test]
fn same_leaf_name_same_function_is_ambiguous_not_a_panic() {
    // This is the case that used to reach `function_index collision`
    // in the IR builder. Both competing paths must be named: the
    // reader cannot fix what the diagnostic will not identify.
    let core = core_tree(&[
        ("std/a/dup.t", "pub fn f() -> u64 { 1u64 }\n"),
        ("std/b/dup.t", "pub fn f() -> u64 { 2u64 }\n"),
    ]);
    let err = test_program_with_core(
        "fn main() -> u64 { dup::f() }",
        Some(core.path().to_path_buf()),
    )
    .expect_err("colliding module file names should be reported");
    // `dup::f()` names a module, and two modules end in `dup`, so
    // the qualifier itself is what cannot be resolved -- a different
    // problem from a bare call, with a different remedy (rename a
    // file), and now a different message.
    assert!(err.contains("ambiguous module path `dup::f`"), "{err}");
    assert!(err.contains("std::a::dup::f"), "{err}");
    assert!(err.contains("std::b::dup::f"), "{err}");
}

#[test]
fn bare_call_reports_ambiguity_rather_than_not_found() {
    let core = core_tree(&[
        ("std/a/dup.t", "pub fn f() -> u64 { 1u64 }\n"),
        ("std/b/dup.t", "pub fn f() -> u64 { 2u64 }\n"),
    ]);
    let err = test_program_with_core(
        "fn main() -> u64 { f() }",
        Some(core.path().to_path_buf()),
    )
    .expect_err("an ambiguous bare call should be reported");
    // Both roots are one root here, so the two candidates rank
    // equally and there is nothing to prefer -- which is the case the
    // bare-call message is for. (A candidate from a *later*
    // `--core-modules` root wins outright; see
    // `a_later_root_wins_a_bare_name`.)
    assert!(err.contains("ambiguous call `f`"), "{err}");
}

#[test]
fn user_function_still_wins_over_a_module_one() {
    let core = core_tree(&[("std/a/dup.t", "pub fn f() -> u64 { 1u64 }\n")]);
    let result = test_program_with_core(
        "fn f() -> u64 { 9u64 }\nfn main() -> u64 { f() }",
        Some(core.path().to_path_buf()),
    );
    assert!(result.is_ok(), "{:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 9);
}

// ============================================================================
// MODULE-IMPORTS D1: `import a.b as h` binds the module to `h`.
//
// The alias used to be parsed and then dropped, so `h::f()` reported
// `Struct 'h' not found` while the un-aliased name kept working
// (MODULE_SYSTEM.md's measured gap #4). The parser now substitutes the
// alias for the module path's last segment, which is what a qualifier
// resolves against -- so nothing downstream learns about aliases.
// ============================================================================

#[test]
fn import_alias_binds_the_module() {
    let core = core_tree(&[("std/a/helpers.t", "pub fn f() -> u64 { 7u64 }\n")]);
    let result = test_program_with_core(
        "import std.a.helpers as h\nfn main() -> u64 { h::f() }",
        Some(core.path().to_path_buf()),
    );
    assert!(result.is_ok(), "h::f() should resolve: {:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 7);
}

#[test]
fn import_alias_does_not_leak_into_other_names() {
    // The substitution is keyed on the alias symbol, so a same-named
    // local binding or a call to a function called `h` is untouched.
    let core = core_tree(&[("std/a/helpers.t", "pub fn f() -> u64 { 7u64 }\n")]);
    let result = test_program_with_core(
        "import std.a.helpers as h\n\
         fn h() -> u64 { 30u64 }\n\
         fn main() -> u64 { val h = 5u64\n h + helpers::f() + h() }",
        Some(core.path().to_path_buf()),
    );
    assert!(result.is_ok(), "{:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 42);
}

#[test]
fn import_alias_works_inside_a_module() {
    // A module's own `import ... as` is file-local: `util` aliases
    // `helpers`, and the entry program neither sees nor needs it.
    let core = core_tree(&[
        ("std/a/helpers.t", "pub fn f() -> u64 { 7u64 }\n"),
        (
            "std/util.t",
            "import std.a.helpers as h\npub fn g() -> u64 { h::f() + 1u64 }\n",
        ),
    ]);
    let result = test_program_with_core(
        "fn main() -> u64 { util::g() }",
        Some(core.path().to_path_buf()),
    );
    assert!(result.is_ok(), "{:?}", result.err());
    assert_eq!(result.unwrap().borrow().unwrap_uint64(), 8);
}

#[test]
fn unknown_qualifier_names_both_possibilities() {
    // `X::f(...)` that resolves to neither a type nor a module used to
    // report `Struct 'X' not found`, which sends a reader with a
    // mistyped module alias looking for a struct.
    let core = core_tree(&[("std/a/helpers.t", "pub fn f() -> u64 { 7u64 }\n")]);
    let err = test_program_with_core(
        "import std.a.helpers as h\nfn main() -> u64 { hh::f() }",
        Some(core.path().to_path_buf()),
    )
    .expect_err("a mistyped qualifier should be reported");
    assert!(err.contains("Type or module 'hh' not found"), "{err}");
}

// ============================================================================
// Expression shapes a module body could not hold.
//
// Integration copies every expression of a module into the main pool,
// and `remap_expression` used to end in a catch-all that refused the
// shapes it had no arm for -- array indexing among them. The entry file
// never goes through that copy, so each of these worked there and
// failed with `Unsupported expression type for remapping` once the same
// function moved into a module (`poc/logsearch`, 2026-09-16).
// ============================================================================

fn run_module_fn(module_src: &str, call: &str) -> Result<u64, String> {
    let core = core_tree(&[("std/shapes.t", module_src)]);
    let main = format!("fn main() -> u64 {{ {call} }}");
    let result = test_program_with_core(&main, Some(core.path().to_path_buf()))?;
    let value = result.borrow().unwrap_uint64();
    Ok(value)
}

#[test]
fn module_can_index_an_array() {
    let src = "pub fn third() -> u64 {\n\
               \x20   val a: [u64; 3] = [1u64, 2u64, 3u64]\n\
               \x20   a[2u64]\n\
               }\n";
    assert_eq!(run_module_fn(src, "shapes::third()"), Ok(3));
}

#[test]
fn module_can_assign_an_array_element() {
    let src = "pub fn f() -> u64 {\n\
               \x20   var a: [u64; 3] = [1u64, 2u64, 3u64]\n\
               \x20   a[0u64] = 40u64\n\
               \x20   a[0u64] + a[1u64]\n\
               }\n";
    assert_eq!(run_module_fn(src, "shapes::f()"), Ok(42));
}

#[test]
fn module_can_take_a_range_slice() {
    let src = "pub fn f() -> u64 {\n\
               \x20   val a: [u64; 4] = [1u64, 2u64, 3u64, 4u64]\n\
               \x20   val s = a[1u64..3u64]\n\
               \x20   s[0u64] + s[1u64]\n\
               }\n";
    assert_eq!(run_module_fn(src, "shapes::f()"), Ok(5));
}

#[test]
fn module_can_hold_a_closure_literal() {
    let src = "pub fn f() -> u64 {\n\
               \x20   val k = 5u64\n\
               \x20   val g = fn(n: u64) -> u64 { n + k }\n\
               \x20   g(37u64)\n\
               }\n";
    assert_eq!(run_module_fn(src, "shapes::f()"), Ok(42));
}

#[test]
fn module_can_use_struct_update() {
    let src = "pub struct Pt { x: u64, y: u64 }\n\
               pub fn f() -> u64 {\n\
               \x20   val p = Pt { x: 1u64, y: 2u64 }\n\
               \x20   val q = Pt { x: 40u64, ..p }\n\
               \x20   q.x + q.y\n\
               }\n";
    assert_eq!(run_module_fn(src, "shapes::f()"), Ok(42));
}

#[test]
fn module_can_hold_a_range_value() {
    let src = "pub fn f() -> u64 {\n\
               \x20   val r = 1u64..3u64\n\
               \x20   7u64\n\
               }\n";
    assert_eq!(run_module_fn(src, "shapes::f()"), Ok(7));
}

#[test]
fn module_can_hold_a_dict_literal() {
    let src = "pub fn f() -> u64 {\n\
               \x20   val d = dict{\"a\": 1u64, \"b\": 41u64}\n\
               \x20   d[\"b\"]\n\
               }\n";
    assert_eq!(run_module_fn(src, "shapes::f()"), Ok(41));
}
