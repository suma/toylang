// POINTER P6: `unsafe fn` — a body that performs a raw memory access
// has to say so.
//
// The check is *direct*: a function is asked only what its own
// statements do, never what its callees do. That is what lets the
// stdlib concentrate the raw builtins behind `Ptr<T>` / `Span<T>` and
// leave every caller safe — a transitive reading would put `unsafe`
// on every `main` that calls `Vec::push`.

use crate::common::{assert_program_result_u64, test_program};

#[test]
fn a_raw_write_without_the_declaration_is_refused() {
    let err = test_program(
        r#"
        fn store(p: ptr) -> u64 {
            __builtin_ptr_write(p, 0u64, 7u64)
            0u64
        }

        fn main() -> u64 { 0u64 }
        "#,
    )
    .expect_err("a raw write needs the declaration");
    assert!(err.contains("E0024"), "{err}");
    assert!(err.contains("store"), "{err}");
    assert!(err.contains("__builtin_ptr_write"), "{err}");
}

#[test]
fn a_raw_read_without_the_declaration_is_refused() {
    let err = test_program(
        r#"
        fn load(p: ptr) -> u64 {
            val v: u64 = __builtin_ptr_read::<u64>(p, 0u64)
            v
        }

        fn main() -> u64 { 0u64 }
        "#,
    )
    .expect_err("a raw read needs the declaration");
    assert!(err.contains("E0024"), "{err}");
    assert!(err.contains("__builtin_ptr_read"), "{err}");
}

#[test]
fn the_declaration_makes_the_raw_access_legal() {
    assert_program_result_u64(
        r#"
        unsafe fn roundtrip() -> u64 {
            val p: ptr = __builtin_heap_alloc(8u64)
            __builtin_ptr_write(p, 0u64, 42u64)
            val v: u64 = __builtin_ptr_read::<u64>(p, 0u64)
            __builtin_heap_free(p)
            v
        }

        fn main() -> u64 { roundtrip() }
        "#,
        42,
    );
}

#[test]
fn calling_an_unsafe_fn_does_not_make_the_caller_unsafe() {
    // The whole point of the direct reading: `main` here is safe code
    // that happens to call into a raw-memory routine.
    assert_program_result_u64(
        r#"
        unsafe fn poke(v: u64) -> u64 {
            val p: ptr = __builtin_heap_alloc(8u64)
            __builtin_ptr_write(p, 0u64, v)
            val out: u64 = __builtin_ptr_read::<u64>(p, 0u64)
            __builtin_heap_free(p)
            out
        }

        fn middle(v: u64) -> u64 { poke(v) }

        fn main() -> u64 { middle(9u64) }
        "#,
        9,
    );
}

#[test]
fn address_arithmetic_is_not_a_raw_access() {
    // `offset` / `is_null` / `eq` / `null_ptr` produce and compare
    // addresses without touching what they point at, so they stay
    // callable from safe code (Rust's `as_ptr` / `offset_from` are
    // safe for the same reason).
    assert_program_result_u64(
        r#"
        fn addresses() -> u64 {
            val p: ptr = __builtin_heap_alloc(16u64)
            val q: ptr = __builtin_ptr_offset(p, 8u64)
            val n: ptr = __builtin_null_ptr()
            var code: u64 = 0u64
            if __builtin_ptr_is_null(n) { code = code + 1u64 }
            if __builtin_ptr_eq(p, q) { code = code + 10u64 }
            __builtin_heap_free(p)
            code
        }

        fn main() -> u64 { addresses() }
        "#,
        1,
    );
}

#[test]
fn a_method_needs_the_declaration_too() {
    let err = test_program(
        r#"
        struct Cell {
            addr: ptr,
        }

        impl Cell {
            fn store(&self, v: u64) -> u64 {
                __builtin_ptr_write(self.addr, 0u64, v)
                v
            }
        }

        fn main() -> u64 { 0u64 }
        "#,
    )
    .expect_err("an impl method is checked like a free function");
    assert!(err.contains("E0024"), "{err}");
    assert!(err.contains("Cell::store"), "{err}");
}

#[test]
fn an_unsafe_method_is_accepted_and_its_caller_stays_safe() {
    assert_program_result_u64(
        r#"
        struct Cell {
            addr: ptr,
        }

        impl Cell {
            fn new() -> Cell {
                Cell { addr: __builtin_heap_alloc(8u64) }
            }

            unsafe fn store(&self, v: u64) -> u64 {
                __builtin_ptr_write(self.addr, 0u64, v)
                v
            }

            unsafe fn load(&self) -> u64 {
                val v: u64 = __builtin_ptr_read::<u64>(self.addr, 0u64)
                v
            }
        }

        fn main() -> u64 {
            val c = Cell::new()
            c.store(5u64)
            c.load()
        }
        "#,
        5,
    );
}

#[test]
fn going_through_ptr_keeps_the_caller_safe() {
    // `Ptr<T>` carries the `unsafe fn` declarations inside the stdlib
    // (`get` / `set` / the bracket forms), so a program that uses the
    // typed window needs none of its own.
    assert_program_result_u64(
        r#"
        fn main() -> u64 {
            val p: Ptr<u64> = Ptr::alloc(4u64)
            p.set(0u64, 11u64)
            p.set(3u64, 31u64)
            val sum: u64 = p.get(0u64) + p.get(3u64)
            __builtin_heap_free(p.as_raw())
            sum
        }
        "#,
        42,
    );
}

#[test]
fn unsafe_is_contextual_and_still_usable_as_a_name() {
    // Only an `unsafe` immediately before `fn` is the modifier — the
    // word is not reserved.
    assert_program_result_u64(
        r#"
        fn main() -> u64 {
            val unsafe: u64 = 3u64
            unsafe + 1u64
        }
        "#,
        4,
    );
}

#[test]
fn a_trait_default_body_carries_its_own_declaration() {
    // A trait signature has no body to check, but a *default* body
    // does — and an impl that omits the method inherits it verbatim.
    // The declaration therefore has to travel with the signature.
    assert_program_result_u64(
        r#"
        trait Peek {
            unsafe fn first(&self) -> u64 {
                val v: u64 = __builtin_ptr_read::<u64>(self.addr, 0u64)
                v
            }
        }

        struct Cell {
            addr: ptr,
        }

        impl Peek for Cell {
        }

        unsafe fn main() -> u64 {
            val p: ptr = __builtin_heap_alloc(8u64)
            __builtin_ptr_write(p, 0u64, 77u64)
            val c = Cell { addr: p }
            c.first()
        }
        "#,
        77,
    );
}

#[test]
fn a_trait_default_body_without_the_declaration_is_refused() {
    let err = test_program(
        r#"
        trait Peek {
            fn first(&self) -> u64 {
                val v: u64 = __builtin_ptr_read::<u64>(self.addr, 0u64)
                v
            }
        }

        struct Cell {
            addr: ptr,
        }

        impl Peek for Cell {
        }

        fn main() -> u64 { 0u64 }
        "#,
    )
    .expect_err("the inherited body is checked in the impl");
    assert!(err.contains("E0024"), "{err}");
}
