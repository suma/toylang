//! MEMORY-ACCESS M1: `__builtin_ptr_read::<T>(p, off)`.
//!
//! The context-typed `__builtin_ptr_read(p, off)` takes its width from
//! the annotation of the binding it must sit in. That makes the read a
//! statement rather than an expression -- the compiled lanes only
//! accept it as `val NAME: TYPE = ...`, enforced by a syntactic special
//! case in `let_lowering.rs` -- and leaves each lane to answer on its
//! own when no annotation is in reach. Naming the type at the call
//! settles both: the width is in the operation, where the IR has always
//! carried it (`InstKind::PtrRead { elem_ty }`).
//!
//! See design-docs/MEMORY_ACCESS.md; the legacy form still works and is
//! what `core/std/*.t` uses until M2 migrates it.

use super::harness::*;

/// The read is an expression. This program is the one from the design
/// doc's 実測 2: the tree-walker ran it, and both compiled lanes
/// refused it with "could not infer source scalar type for `as` cast"
/// because the read had no annotation to take a width from.
#[test]
fn a_typed_read_needs_no_annotation_around_it() {
    let src = r#"
        unsafe fn main() -> u64 {
            val p: ptr = __builtin_heap_alloc(16u64)
            __builtin_ptr_write(p, 0u64, 0x41u8)
            __builtin_ptr_write(p, 1u64, 0x42u8)
            val sum: u64 = (__builtin_ptr_read::<u8>(p, 0u64) as u64)
                         + (__builtin_ptr_read::<u8>(p, 1u64) as u64)
            __builtin_heap_free(p)
            sum
        }
    "#;
    assert_eq!(interpreter_value(src), 131);
    assert_consistent(src, "typed_ptr_read_expr");
}

/// Reading a range as a wider type than it was written with is the
/// program from 実測 1, where the three lanes answered 65 / 16961 /
/// 25769820737 and nothing said a word. Written with the width at the
/// call it is still a reinterpretation -- nothing checks that the bytes
/// were *written* as a `u64` -- but every lane now reads the same eight
/// bytes, little-endian.
#[test]
fn a_wider_read_sees_the_bytes_every_lane_sees() {
    let src = r#"
        unsafe fn main() -> u64 {
            val p: ptr = __builtin_heap_alloc(8u64)
            var i: u64 = 0u64
            while i < 8u64 {
                __builtin_ptr_write(p, i, 0u8)
                i = i + 1u64
            }
            __builtin_ptr_write(p, 0u64, 0x41u8)
            __builtin_ptr_write(p, 1u64, 0x42u8)
            val wide: u64 = __builtin_ptr_read::<u64>(p, 0u64)
            __builtin_heap_free(p)
            # 0x4241 — little-endian, the AOT lane's answer all along
            wide
        }
    "#;
    assert_eq!(interpreter_value(src), 16961);
    assert_consistent(src, "typed_ptr_read_wide");
}

/// Every scalar width, including the ones the legacy form reaches only
/// through an annotation. `f64` is here because the tree-walker's
/// byte-level fallback had no float arm at all.
#[test]
fn every_scalar_width_round_trips() {
    let src = r#"
        unsafe fn main() -> u64 {
            val p: ptr = __builtin_heap_alloc(64u64)
            __builtin_ptr_write(p, 0u64, 200u8)
            __builtin_ptr_write(p, 8u64, 40000u16)
            __builtin_ptr_write(p, 16u64, 70000u32)
            __builtin_ptr_write(p, 24u64, 5000000000u64)
            __builtin_ptr_write(p, 32u64, -7i32)
            __builtin_ptr_write(p, 40u64, 2.5f64)
            __builtin_ptr_write(p, 48u64, true)
            val a: u64 = __builtin_ptr_read::<u8>(p, 0u64) as u64
            val b: u64 = __builtin_ptr_read::<u16>(p, 8u64) as u64
            val c: u64 = __builtin_ptr_read::<u32>(p, 16u64) as u64
            val d: u64 = __builtin_ptr_read::<u64>(p, 24u64)
            val e: i64 = __builtin_ptr_read::<i32>(p, 32u64) as i64
            val f: f64 = __builtin_ptr_read::<f64>(p, 40u64)
            val g: bool = __builtin_ptr_read::<bool>(p, 48u64)
            __builtin_heap_free(p)
            val floats: u64 = if f == 2.5f64 { 1u64 } else { 0u64 }
            val flags: u64 = if g { 2u64 } else { 0u64 }
            val signed: u64 = if e == -7i64 { 4u64 } else { 0u64 }
            # 200 + 40000 + 70000 + 5000000000 stays under u64
            a + b + c + d + floats + flags + signed
        }
    "#;
    assert_eq!(interpreter_value(src), 5000110207);
    assert_consistent(src, "typed_ptr_read_widths");
}

/// A compound `T` reads one leaf at a time into the binding's leaf
/// locals, the way the annotation form's `AOT-COMPOUND-PTR-RW` path
/// already did — the type argument just replaces the annotation.
#[test]
fn a_struct_read_takes_its_shape_from_the_type_argument() {
    let src = r#"
        struct Pair { a: u64, b: u64 }

        unsafe fn main() -> u64 {
            val p: ptr = __builtin_heap_alloc(16u64)
            # Bound first: a compound literal in an argument position
            # is a separate compiled-lane gap, not this one's.
            val v: Pair = Pair { a: 3u64, b: 4u64 }
            __builtin_ptr_write(p, 0u64, v)
            val back: Pair = __builtin_ptr_read::<Pair>(p, 0u64)
            __builtin_heap_free(p)
            back.a * 10u64 + back.b
        }
    "#;
    assert_eq!(interpreter_value(src), 34);
    assert_consistent(src, "typed_ptr_read_struct");
}

/// Inside a generic body the type argument is the parameter, resolved
/// through the same substitution `__builtin_sizeof::<T>()` uses.
#[test]
fn a_generic_parameter_is_a_usable_type_argument() {
    let src = r#"
        struct Cell<T> { addr: ptr }

        impl<T> Cell<T> {
            unsafe fn make(value: T) -> Self {
                val p: ptr = __builtin_heap_alloc(__builtin_sizeof::<T>())
                __builtin_ptr_write(p, 0u64, value)
                Cell { addr: p }
            }

            unsafe fn get(&self) -> T {
                __builtin_ptr_read::<T>(self.addr, 0u64)
            }
        }

        unsafe fn main() -> u64 {
            val c: Cell<u64> = Cell::make(41u64)
            c.get() + 1u64
        }
    "#;
    assert_eq!(interpreter_value(src), 42);
    assert_consistent(src, "typed_ptr_read_generic");
}

/// MEMORY-ACCESS M5: the stdlib `Ptr<T>`'s `get` / `set` lower to the
/// read and write their bodies perform, not to calls -- the compiled
/// lanes have no inliner, and the call doubled an element loop's time.
/// Every scalar width reads back what it wrote, and `main` calls
/// neither method.
#[test]
fn stdlib_ptr_access_lowers_to_the_read_itself() {
    let src = r#"
        fn main() -> u64 {
            val b: Ptr<bool> = Ptr::alloc(2u64)
            val u: Ptr<u8> = Ptr::alloc(3u64)
            val h: Ptr<i16> = Ptr::alloc(2u64)
            val f: Ptr<f32> = Ptr::alloc(2u64)
            val w: Ptr<u64> = Ptr::alloc(2u64)
            b.set(1u64, true)
            u.set(2u64, 200u8)
            h.set(1u64, -7i16)
            f.set(1u64, 2.5f32)
            w.set(1u64, 9u64)
            w[0u64] = 4u64
            val b1 = b.get(1u64)
            val u2 = u.get(2u64)
            val h1 = h.get(1u64)
            val f1 = f.get(1u64)
            val w0 = w[0u64]
            val w1 = w.get(1u64)
            println("{b1} {u2} {h1} {f1} {w0} {w1}")
            0u64
        }
    "#;
    assert_renders(src, "ptr_intrinsic_widths", "true 200 -7 2.5 4 9\n");
    // `main` reads and writes directly. (A method body can still be
    // instantiated when the type of `val w1 = w.get(..)` is worked out;
    // nothing calls it.)
    let ir = lowered_ir(src);
    let start = ir.find("function main(").expect("main in the IR");
    let main = &ir[start..start + ir[start..].find("\n}").expect("end of main")];
    // Five calls, one per `Ptr::alloc`; every access is inline.
    assert_eq!(main.matches("call").count(), 5, "{main}");
    assert_eq!(main.matches("ptr_write").count(), 6, "{main}");
    assert_eq!(main.matches("ptr_read").count(), 6, "{main}");
}

/// Only the stdlib's `Ptr` is the intrinsic. A program's own `Ptr` with
/// a `get` of its own keeps its body.
#[test]
fn a_user_ptr_keeps_its_own_get() {
    let src = r#"
        struct Ptr<T> { addr: ptr }
        impl<T> Ptr<T> {
            fn get(&self, i: u64) -> u64 { i + 100u64 }
        }
        fn main() -> u64 {
            val p: Ptr<u64> = Ptr { addr: __builtin_null_ptr() }
            val v = p.get(5u64)
            println(v)
            0u64
        }
    "#;
    assert_renders(src, "user_ptr_get", "105\n");
}
