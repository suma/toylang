// NEVER-ALLOCATES: `never_allocates fn f()` — the compile-time
// counterpart to `ensures allocates(0u64)`.
//
// The clause measures one call and reports what it cost; this rules
// the possibility out before the program runs. The check follows the
// call graph rather than requiring every callee to carry the
// modifier, so `Vec::push` is rejected for reaching
// `__builtin_heap_realloc` and not for missing an annotation.

use crate::common::{assert_program_result_u64, test_program};

#[test]
fn a_function_that_does_not_allocate_is_accepted() {
    assert_program_result_u64(
        r#"
        never_allocates fn triangle(n: u64) -> u64 {
            var total: u64 = 0u64
            var i: u64 = 1u64
            while i <= n {
                total = total + i
                i = i + 1u64
            }
            total
        }

        fn main() -> u64 { triangle(8u64) }
        "#,
        36,
    );
}

#[test]
fn a_direct_allocation_is_refused() {
    let err = test_program(
        r#"
        never_allocates fn leaky(n: u64) -> u64 {
            val p: ptr = __builtin_heap_alloc(32u64)
            n
        }

        fn main() -> u64 { leaky(1u64) }
        "#,
    )
    .expect_err("the declaration cannot be honoured");
    assert!(err.contains("E0016"), "{err}");
    assert!(err.contains("leaky -> __builtin_heap_alloc"), "{err}");
}

#[test]
fn the_diagnostic_names_the_path_not_just_the_function() {
    // The allocation is usually not in the function that was
    // declared; without the chain the reader has to find it.
    let err = test_program(
        r#"
        fn inner(n: u64) -> u64 {
            val p: ptr = __builtin_heap_alloc(32u64)
            n
        }

        fn middle(n: u64) -> u64 { inner(n) }

        never_allocates fn outer(n: u64) -> u64 { middle(n) }

        fn main() -> u64 { outer(1u64) }
        "#,
    )
    .expect_err("the transitive allocation must be found");
    assert!(
        err.contains("outer -> middle -> inner -> __builtin_heap_alloc"),
        "{err}"
    );
}

#[test]
fn a_stdlib_call_that_allocates_is_refused() {
    // Nothing in `core/std` carries the modifier; `Vec::new` is
    // rejected because the walk reaches the allocator through it.
    let err = test_program(
        r#"
        never_allocates fn build(n: u64) -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(n)
            n
        }

        fn main() -> u64 { build(1u64) }
        "#,
    )
    .expect_err("Vec allocates");
    assert!(err.contains("E0016"), "{err}");
    assert!(err.contains("__builtin_heap_alloc"), "{err}");
}

#[test]
fn string_interpolation_is_allowed() {
    // What the runtime spends holding a `str` is not the program's
    // allocation — the counters exclude it (MEM-COUNTER-INTERP-DRIFT),
    // so this check does too. Otherwise no function that reports
    // anything could ever be `never_allocates`.
    assert_program_result_u64(
        r#"
        never_allocates fn report(n: u64) -> u64 {
            println("n = {n}")
            n
        }

        fn main() -> u64 { report(7u64) }
        "#,
        7,
    );
}

#[test]
fn a_call_through_a_function_value_is_refused() {
    // The body runs wherever the value points, which this pass cannot
    // see. Assuming it is clean would make the guarantee worthless.
    let err = test_program(
        r#"
        never_allocates fn run(f: fn (u64) -> u64, n: u64) -> u64 {
            f(n)
        }

        fn main() -> u64 {
            val double = fn(x: u64) -> u64 { x * 2u64 }
            run(double, 21u64)
        }
        "#,
    )
    .expect_err("an indirect call cannot be followed");
    assert!(err.contains("cannot follow"), "{err}");
}

#[test]
fn an_extern_call_is_refused_unless_the_declaration_says_otherwise() {
    let src = r#"
        DECL
        never_allocates fn read_one() -> i32 {
            getchar()
        }

        fn main() -> u64 { 0u64 }
    "#;
    let err = test_program(&src.replace("DECL", r#"extern fn getchar() -> i32 from "c""#))
        .expect_err("an extern body cannot be walked");
    assert!(err.contains("extern fn"), "{err}");

    // The escape hatch: the author takes responsibility. This is a
    // declaration, not a proof — the implementation is outside the
    // language.
    assert!(
        test_program(
            &src.replace("DECL", r#"never_allocates extern fn getchar() -> i32 from "c""#)
        )
        .is_ok(),
        "a declared-allocation-free extern should be accepted"
    );
}

#[test]
fn the_modifier_works_on_methods_too() {
    let err = test_program(
        r#"
        struct Counter { n: u64 }

        impl Counter {
            never_allocates fn bad(&self) -> u64 {
                val p: ptr = __builtin_heap_alloc(16u64)
                self.n
            }
        }

        fn main() -> u64 {
            val c = Counter { n: 7u64 }
            c.bad()
        }
        "#,
    )
    .expect_err("the method's declaration cannot be honoured");
    // Named by owner and method, since the same method name can
    // belong to several types.
    assert!(err.contains("`Counter::bad`"), "{err}");

    assert_program_result_u64(
        r#"
        struct Counter { n: u64 }

        impl Counter {
            never_allocates fn get(&self) -> u64 { self.n }
        }

        fn main() -> u64 {
            val c = Counter { n: 7u64 }
            c.get()
        }
        "#,
        7,
    );
}

#[test]
fn a_same_named_method_on_another_type_is_not_confused_with_it() {
    // `Vec::new` allocates and `Counter::new` does not. Resolving by
    // name alone would reject this program for what `Vec` does.
    assert_program_result_u64(
        r#"
        struct Counter { n: u64 }

        impl Counter {
            fn new(start: u64) -> Counter { Counter { n: start } }
            fn get(&self) -> u64 { self.n }
        }

        never_allocates fn make(n: u64) -> u64 {
            val c = Counter::new(n)
            c.get()
        }

        fn main() -> u64 { make(7u64) }
        "#,
        7,
    );
}

#[test]
fn the_name_stays_available_outside_a_declaration() {
    // Contextual, like `old` and the budget clauses: only a
    // `never_allocates` immediately before `fn` or `extern` is the
    // modifier.
    assert_program_result_u64(
        r#"
        fn never_allocates(n: u64) -> u64 { n * 2u64 }

        fn main() -> u64 {
            val never_allocates: u64 = 21u64
            never_allocates + never_allocates(0u64)
        }
        "#,
        21,
    );
}

#[test]
fn recursion_does_not_confuse_the_walk() {
    assert_program_result_u64(
        r#"
        never_allocates fn countdown(n: u64) -> u64 {
            if n == 0u64 {
                0u64
            } else {
                countdown(n - 1u64)
            }
        }

        fn main() -> u64 {
            countdown(5u64)
            7u64
        }
        "#,
        7,
    );
}

#[test]
fn a_method_may_stack_never_allocates_and_unsafe() {
    // NEVER-ALLOCATES-METHOD-STACK: the modifier run in an `impl`
    // block asked whether `fn` came *immediately* after the word it
    // was looking at, so `unsafe fn` and `never_allocates fn` parsed
    // and `never_allocates unsafe fn` did not — while the
    // free-function parser accepted all three. `Vec`'s read-only half
    // is nearly all `unsafe fn`, and this is why none of it could also
    // promise not to allocate.
    //
    // Both orders, because the two say different things about the
    // same method and neither qualifies the other.
    assert_program_result_u64(
        r#"
        struct Cell { v: u64 }

        impl Cell {
            never_allocates unsafe fn one(&self) -> u64 { self.v }
            unsafe never_allocates fn two(&self) -> u64 { self.v * 2u64 }
        }

        fn main() -> u64 {
            val c = Cell { v: 3u64 }
            c.one() + c.two()
        }
        "#,
        9,
    );
}

#[test]
fn unsafe_is_still_an_ordinary_name_in_an_impl_block() {
    // The run is recognised as a whole before any of it is consumed,
    // so a binding that happens to be called `unsafe` is not eaten by
    // a parser that has already committed to a modifier.
    assert_program_result_u64(
        r#"
        struct Cell { v: u64 }

        impl Cell {
            fn plain(&self) -> u64 {
                val unsafe = 7u64
                val never_allocates = 2u64
                unsafe * never_allocates
            }
        }

        fn main() -> u64 {
            val c = Cell { v: 0u64 }
            c.plain()
        }
        "#,
        14,
    );
}
