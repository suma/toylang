//! DIAG-SYMBOL-NAME-LOWER: what a *lowering* refusal says.
//!
//! `frontend/tests/diagnostic_spelling_tests.rs` checks the rule at
//! the source — no hand-built message in the type checker or in
//! `compiler_lower` formats a value with `{:?}`. This file checks it
//! at the other end, through the real path: compile a program that the
//! lowering pass refuses and read the sentence it produced.
//!
//! Both are needed, for the reason the type-checker pair records: the
//! source scan cannot see a leak that arrives through a shared helper,
//! and a corpus cannot see a site no program in it reaches.
//!
//! The lowering pass refuses a lot — it is an MVP, and the refusals
//! are how a reader learns where its edges are. Before this, several
//! of them handed back the compiler's private vocabulary:
//!
//! ```text
//! compiler MVP cannot lower expression yet: QualifiedIdentifier([SymbolU32 { value: 60 }, ...])
//! enum `Shape::Circle` has unsupported payload type `Struct(SymbolU32 { value: 71 }, [])`
//! ```

use std::path::PathBuf;

use compiler::{compile_to_jit_main_with_options, CompilerOptions};

/// Spellings only Debug produces — an interned id where a name
/// belongs, or an IR / AST variant name where a source type belongs.
const DEBUG_LEAKS: &[&str] = &[
    "SymbolU32",
    "StructId(",
    "EnumId(",
    "TupleId(",
    "UInt64",
    "Int64",
    "Float64",
    "UInt8",
    "TypeDecl",
    "QualifiedIdentifier",
    "AssociatedFunctionCall",
    "MethodCall(",
    "FieldShape",
    "Binding::",
];

/// Lower `source` without auto-loading core, and return the refusal.
fn refusal_for(source: &str) -> String {
    let options = CompilerOptions::new(PathBuf::from("<lower-diag>"));
    match compile_to_jit_main_with_options(source, &options) {
        Ok(_) => panic!("expected the lowering pass to refuse this program:\n{source}"),
        Err(e) => e,
    }
}

/// Each case: a name, a program the lowering pass refuses, and the
/// words its message must contain. The expectations are *user*
/// spellings — `P`, `u64`, `.len(...)` — which is the whole point.
fn cases() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    vec![
        (
            "struct-returning call in expression position",
            r#"struct P { x: i64 }
               fn mk() -> P { P { x: 1i64 } }
               fn take(p: P) -> i64 { p.x }
               fn main() -> i64 { take(mk()) }"#,
            vec!["struct-returning call", "`mk`"],
        ),
        (
            "compound capture in a closure",
            r#"struct P { x: i64 }
               fn main() -> i64 {
                 val p = P { x: 1i64 }
                 val f = fn() -> i64 { p.x }
                 f()
               }"#,
            vec!["capture"],
        ),
        (
            "an unsupported enum payload type",
            r#"enum E { A(f64x2), B }
               fn main() -> u64 { val e = E::B
                 0u64 }"#,
            vec!["E::A", "payload type", "f64x2"],
        ),
        (
            // The headline refusal — the one that used to print the
            // whole AST node. It now names the form instead.
            "an expression form the lowering pass does not handle",
            r#"fn main() -> u64 {
                 val r = 0u64..10u64
                 0u64
               }"#,
            vec!["cannot lower a range"],
        ),
        (
            "a method call on something that is not a binding",
            r#"struct P { x: i64 }
               impl P { fn get(&self) -> i64 { self.x } }
               fn mk() -> P { P { x: 1i64 } }
               fn main() -> i64 { mk().get() }"#,
            vec!["method call"],
        ),
    ]
}

#[test]
fn a_lowering_refusal_spells_what_it_means() {
    for (name, source, expected) in cases() {
        let message = refusal_for(source);
        for leak in DEBUG_LEAKS {
            assert!(
                !message.contains(leak),
                "`{name}`: the refusal shows `{leak}`, which only Debug produces:\n  {message}"
            );
        }
        for want in &expected {
            assert!(
                message.contains(want),
                "`{name}`: the refusal does not mention `{want}`:\n  {message}"
            );
        }
    }
}

/// Guards the guard: a corpus whose programs all compile would pass
/// the test above without checking anything.
#[test]
fn every_case_is_actually_refused() {
    for (name, source, _) in cases() {
        let options = CompilerOptions::new(PathBuf::from("<lower-diag>"));
        assert!(
            compile_to_jit_main_with_options(source, &options).is_err(),
            "`{name}` now compiles — the case has stopped testing anything, \
             so either drop it or replace it with one that still reaches a refusal"
        );
    }
}
