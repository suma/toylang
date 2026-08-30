//! DATA-ORIENTED Phase 0 — `soa [T; N]` stack arrays.
//!
//! The contract these pin, from `design-docs/DATA_ORIENTED.md`:
//!
//! * **`soa` is placement, not type identity.** The same program in
//!   its AoS and SoA spellings must produce the same answer, on every
//!   backend — the checker's `is_equivalent` ignores the flag, and
//!   the tree-walker never reads it, so any divergence is a lowering
//!   bug in one of the compiled lanes.
//! * **The single-column shortcut.** `ps[i].f` lowers to one leaf
//!   load instead of materialising the whole element. That is the
//!   feature's point (a loop over one field touches one column), and
//!   it is also new territory for the *AoS* lanes: field chains
//!   rooted at an array element used to be rejected outright
//!   ("field-access chains rooted at a bare identifier"), so the AoS
//!   half of these tests pins functionality, not just layout.
//! * Storage is one homogeneous slot per leaf column, so the IR,
//!   codegen and the IR VM are unchanged — every test here runs the
//!   unmodified `ArrayLoad` / `ArrayStore` machinery.

use super::harness::*;

/// The reference program used by the on/off comparison: one field's
/// loop (the DoD shape), a whole-element read, a re-slice, and a
/// single-column write — everything Phase 0 covers, in one main.
const AOS_SRC: &str = r#"
    struct Particle { x: i64, y: i64, mass: i64 }

    fn main() -> i64 {
        val ps: [Particle; 4] = [
            Particle { x: 1i64, y: 2i64, mass: 10i64 },
            Particle { x: 3i64, y: 4i64, mass: 20i64 },
            Particle { x: 5i64, y: 6i64, mass: 30i64 },
            Particle { x: 7i64, y: 8i64, mass: 40i64 },
        ]
        # the DoD loop: one field, runtime index
        var total: i64 = 0i64
        for i in 0i64..4i64 {
            total = total + ps[i].mass
        }
        # single-column write, then read back through a binding
        ps[2i64].x = 60i64
        val b = ps[2i64].x
        # whole-element read
        val p = ps[1i64]
        # range slice keeps the source layout (no annotation)
        val sub = ps[1i64..3i64]
        total + b + p.x + p.y + sub[0i64].y + sub[1i64].x
    }
"#;

/// `AOS_SRC` with the two array annotations flipped to `soa` — the
/// only difference between the programs, which is the whole claim.
const SOA_SRC: &str = r#"
    struct Particle { x: i64, y: i64, mass: i64 }

    fn main() -> i64 {
        val ps: soa [Particle; 4] = [
            Particle { x: 1i64, y: 2i64, mass: 10i64 },
            Particle { x: 3i64, y: 4i64, mass: 20i64 },
            Particle { x: 5i64, y: 6i64, mass: 30i64 },
            Particle { x: 7i64, y: 8i64, mass: 40i64 },
        ]
        # the DoD loop: one field, runtime index
        var total: i64 = 0i64
        for i in 0i64..4i64 {
            total = total + ps[i].mass
        }
        # single-column write, then read back through a binding
        ps[2i64].x = 60i64
        val b = ps[2i64].x
        # whole-element read
        val p = ps[1i64]
        # range slice keeps the source layout (no annotation)
        val sub = ps[1i64..3i64]
        total + b + p.x + p.y + sub[0i64].y + sub[1i64].x
    }
"#;

#[test]
fn soa_on_and_off_produce_the_same_answer() {
    // 100 + 60 + 3 + 4 + 4 + 60 = 231. Both spellings must land
    // there, and each must be consistent across the tree-walker,
    // the IR VM, the JIT and the AOT binary — the doc's "soa 有無で
    // 答えが変わらない" pin, spelled as a test.
    let aos = interpreter_value(AOS_SRC);
    let soa = interpreter_value(SOA_SRC);
    assert_eq!(aos & 0xff, 231, "AoS spelling: {AOS_SRC}");
    assert_eq!(aos & 0xff, soa & 0xff, "soa changed the answer");
    assert_consistent(AOS_SRC, "soa_on_off_aos");
    assert_consistent(SOA_SRC, "soa_on_off_soa");
}

#[test]
fn single_column_shortcut_reads_and_writes() {
    // `ps[i].f` under a runtime index, both directions, plus the
    // negative-constant-index form (`ps[-1i64].x` is the last
    // element) that the shared bounds-guard path must keep working.
    let src = r#"
        struct Cell { tag: u64, value: u64 }

        fn main() -> u64 {
            val cs: soa [Cell; 3] = [
                Cell { tag: 1u64, value: 10u64 },
                Cell { tag: 2u64, value: 20u64 },
                Cell { tag: 3u64, value: 30u64 },
            ]
            var sum: u64 = 0u64
            for i in 0u64..3u64 {
                sum = sum + cs[i].value
            }
            cs[1u64].value = 99u64
            val last = cs[-1i64].tag
            sum + cs[1u64].value + last
        }
    "#;
    // 60 + 99 + 3 = 162.
    assert_eq!(interpreter_value(src) & 0xff, 162);
    assert_consistent(src, "soa_single_column_shortcut");
}

#[test]
fn field_chains_below_an_array_element() {
    // `ps[i].pos.x` (a chain through a nested struct) and `ts[i].0`
    // (a tuple element) resolve to one leaf each. The compound
    // stopover (`ps[i].pos` as a value) is deliberately not here —
    // it needs the pending-compound channel and stays on the
    // element-binding form.
    let src = r#"
        struct Pos { x: i64, y: i64 }
        struct Body { pos: Pos, id: i64 }

        fn main() -> i64 {
            val ps: soa [Body; 2] = [
                Body { pos: Pos { x: 1i64, y: 2i64 }, id: 10i64 },
                Body { pos: Pos { x: 3i64, y: 4i64 }, id: 20i64 },
            ]
            val ts: soa [(i64, i64); 2] = [(5i64, 6i64), (7i64, 8i64)]
            ps[1i64].pos.x + ps[0i64].id + ts[1i64].0 + ts[0i64].1
        }
    "#;
    // 3 + 10 + 7 + 6 = 26.
    assert_eq!(interpreter_value(src) & 0xff, 26);
    assert_consistent(src, "soa_field_chains");
}

#[test]
fn range_slice_preserves_or_relayouts() {
    // No annotation → the slice keeps the source's placement; an
    // annotation decides otherwise, in either direction. The
    // per-leaf copy is the same code either way (a cross-layout
    // copy materialises through leaf slots), so all four
    // combinations below must agree.
    let src = r#"
        struct Point { x: i64, y: i64 }

        fn main() -> i64 {
            val aos: [Point; 4] = [
                Point { x: 1i64, y: 2i64 },
                Point { x: 3i64, y: 4i64 },
                Point { x: 5i64, y: 6i64 },
                Point { x: 7i64, y: 8i64 },
            ]
            val soa: soa [Point; 4] = [
                Point { x: 1i64, y: 2i64 },
                Point { x: 3i64, y: 4i64 },
                Point { x: 5i64, y: 6i64 },
                Point { x: 7i64, y: 8i64 },
            ]
            # no annotation: keep the source layout
            val keep_aos = aos[1i64..3i64]
            val keep_soa = soa[1i64..3i64]
            # annotation: re-layout in either direction
            val to_soa: soa [Point; 2] = aos[1i64..3i64]
            val to_aos: [Point; 2] = soa[1i64..3i64]
            keep_aos[0i64].x + keep_soa[1i64].y
                + to_soa[0i64].x + to_aos[1i64].y
        }
    "#;
    // 3 + 6 + 3 + 6 = 18.
    assert_eq!(interpreter_value(src) & 0xff, 18);
    assert_consistent(src, "soa_range_slice_layouts");
}

#[test]
fn scalar_soa_degenerates_to_the_aos_slot() {
    // One leaf is one column, so `soa [u64; N]` lowers identically to
    // `[u64; N]` — the spelling is accepted and means nothing extra.
    let src = r#"
        fn main() -> u64 {
            val xs: soa [u64; 3] = [4u64, 5u64, 6u64]
            var acc: u64 = 0u64
            for i in 0u64..3u64 {
                acc = acc + xs[i]
            }
            xs[1u64] = 50u64
            acc + xs[1u64]
        }
    "#;
    // 15 + 50 = 65.
    assert_eq!(interpreter_value(src) & 0xff, 65);
    assert_consistent(src, "soa_scalar_degenerate");
}

#[test]
fn narrow_and_bool_leaves_keep_their_values() {
    // Phase 0 stores every column at the uniform 8-byte stride, so a
    // `u8` / `f32` / `bool` leaf occupies an 8-byte slot the way the
    // interleaved layout's leaves always have. Values — not bytes —
    // are what the engines agree on, and narrow leaves round-trip
    // through the column slots exactly as through the interleaved
    // ones.
    let src = r#"
        struct Mixed { flag: bool, byte: u8, single: f32, wide: u64 }

        fn main() -> u64 {
            val ms: soa [Mixed; 3] = [
                Mixed { flag: true,  byte: 1u8,  single: 1.5f32, wide: 10u64 },
                Mixed { flag: false, byte: 2u8,  single: 2.5f32, wide: 20u64 },
                Mixed { flag: true,  byte: 3u8,  single: 3.5f32, wide: 30u64 },
            ]
            var flags: u64 = 0u64
            var bytes: u64 = 0u64
            for i in 0u64..3u64 {
                if ms[i].flag { flags = flags + 1u64 }
                bytes = bytes + ms[i].byte as u64
            }
            ms[1u64].byte = 9u8
            # 1.5 + 3.5 = 5.0 exactly in f32, so the compare is exact
            val singles = if ms[0u64].single + ms[2u64].single == 5.0f32 { 100u64 } else { 0u64 }
            flags + bytes + ms[1u64].byte as u64 + singles + ms[2u64].wide
        }
    "#;
    // 2 + 6 + 9 + 100 + 30 = 147.
    assert_eq!(interpreter_value(src) & 0xff, 147);
    assert_consistent(src, "soa_narrow_leaves");
}

#[test]
fn aos_field_chains_below_an_element_also_work() {
    // The shortcut is not SoA-only: `aos[i].f` resolves to the one
    // interleaved leaf load (`i * leaf_count + leaf`) instead of the
    // historical rejection. Same program, AoS spelling — this pins
    // the newly-supported AoS side on its own, so a regression there
    // cannot hide behind the soa tests.
    let src = r#"
        struct Point { x: i64, y: i64 }

        fn main() -> i64 {
            val ps: [Point; 3] = [
                Point { x: 1i64, y: 2i64 },
                Point { x: 3i64, y: 4i64 },
                Point { x: 5i64, y: 6i64 },
            ]
            var acc: i64 = 0i64
            for i in 0i64..3i64 {
                acc = acc + ps[i].y
            }
            ps[0i64].x = 50i64
            acc + ps[0i64].x
        }
    "#;
    // 12 + 50 = 62.
    assert_eq!(interpreter_value(src) & 0xff, 62);
    assert_consistent(src, "aos_field_chains");
}

#[test]
fn annotated_struct_element_arrays_type_check() {
    // `val ps: [Point; N] = [...]` used to be rejected outright
    // ("Cannot mix struct type Identifier with Struct(name, []) in
    // array") — the literal's element type is spelled `Struct` while
    // the annotation's is `Identifier`, and the element check did
    // not unify the two spellings. `soa` needs the annotated form,
    // so this pins the fix for both spellings.
    let src = r#"
        struct Point { x: i64, y: i64 }

        fn main() -> i64 {
            val aos: [Point; 2] = [Point { x: 1i64, y: 2i64 }, Point { x: 3i64, y: 4i64 }]
            val soa: soa [Point; 2] = [Point { x: 5i64, y: 6i64 }, Point { x: 7i64, y: 8i64 }]
            aos[0i64].x + aos[1i64].y + soa[0i64].x + soa[1i64].y
        }
    "#;
    // 1 + 4 + 5 + 8 = 18.
    assert_eq!(interpreter_value(src) & 0xff, 18);
    assert_consistent(src, "annotated_struct_element_array");
}
