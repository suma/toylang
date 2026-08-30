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

// ---------------------------------------------------------------
// The layout itself, as the AOT backend sees it.
//
// Everything above pins that `soa` does *not* change the answer.
// That is only half the claim: a `soa` that silently lowered to the
// interleaved layout would pass every one of those tests. The
// difference `soa` is asked for is a difference in memory, and in the
// AOT lane memory means the cranelift stack frame — `explicit_slot`
// declarations and the `stack_addr` / `imul` chain each access lowers
// to. `aot_clif` renders exactly the text `--emit=clif` writes, which
// is the same codegen the object file comes out of.
//
// These read the frame, not the machine code, so they survive
// register allocation and instruction selection; what they pin is the
// placement decision, which is the feature.
// ---------------------------------------------------------------

/// The slots one `stack_addr` names, in first-seen order — which
/// arrays (or which columns) a stretch of cranelift text touches.
fn slots_touched(clif: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for line in clif.lines() {
        if let Some((_, rest)) = line.split_once("stack_addr.i64 ") {
            let slot = rest.split(|c: char| c == ',' || c.is_whitespace()).next().unwrap_or("");
            if !slot.is_empty() && !seen.iter().any(|s| s == slot) {
                seen.push(slot.to_string());
            }
        }
    }
    seen
}

/// The loop body, as a block of cranelift text.
///
/// A *typed* `imul.i64` is the marker: cranelift prints the type when
/// the operand is a block parameter, which for these programs means
/// the loop's induction variable. The entry block's index arithmetic
/// is all on constants and prints untyped.
fn loop_block(clif_fn: &str) -> String {
    clif_block_containing(clif_fn, "imul.i64")
}

#[test]
fn soa_reshapes_the_aot_stack_frame() {
    // Same program, same answer (pinned above), different memory:
    // AoS gives each array one interleaved slot, SoA gives it one
    // slot per leaf column. `Particle` has three leaves, and the
    // programs hold a 4-element array plus a 2-element range slice.
    let aos = clif_function(&aot_clif(AOS_SRC), "main");
    let soa = clif_function(&aot_clif(SOA_SRC), "main");

    // AoS: 4 * 3 leaves * 8 = 96, and the slice 2 * 3 * 8 = 48.
    assert_eq!(clif_stack_slots(&aos), vec![96, 48], "AoS frame:\n{aos}");
    // SoA: three columns of 4 * 8 = 32, then the slice's three
    // columns of 2 * 8 = 16. The slice carries the source's layout
    // because it is unannotated — `val sub = ps[1..3]`.
    assert_eq!(
        clif_stack_slots(&soa),
        vec![32, 32, 32, 16, 16, 16],
        "SoA frame:\n{soa}"
    );

    // Phase 0 keeps every column at the uniform 8-byte
    // `ARRAY_LEAF_STRIDE`, so the two frames cost the same bytes and
    // only the placement differs. Phase 0.5 (tight pack) is the
    // change that makes the SoA total smaller — this equality is the
    // line that will move then, deliberately.
    let total = |slots: Vec<u32>| slots.iter().sum::<u32>();
    assert_eq!(
        total(clif_stack_slots(&aos)),
        total(clif_stack_slots(&soa)),
        "Phase 0 columns are the same total size as the interleaved slot"
    );
}

#[test]
fn soa_field_loop_is_a_unit_stride_column_walk_in_the_aot_frame() {
    // The DoD shape: a loop over one field. This is what the layout
    // is *for*, so it gets its own reading of the emitted addressing.
    let src = |soa: &str| {
        format!(
            r#"
        struct Particle {{ x: i64, y: i64, mass: i64 }}

        fn main() -> i64 {{
            val ps: {soa}[Particle; 4] = [
                Particle {{ x: 1i64, y: 2i64, mass: 10i64 }},
                Particle {{ x: 3i64, y: 4i64, mass: 20i64 }},
                Particle {{ x: 5i64, y: 6i64, mass: 30i64 }},
                Particle {{ x: 7i64, y: 8i64, mass: 40i64 }},
            ]
            var total: i64 = 0i64
            for i in 0i64..4i64 {{
                total = total + ps[i].mass
            }}
            total
        }}
    "#
        )
    };
    let aos_src = src("");
    let soa_src = src("soa ");
    assert_eq!(interpreter_value(&aos_src) & 0xff, 100);
    assert_eq!(interpreter_value(&soa_src) & 0xff, 100);

    let aos = clif_function(&aot_clif(&aos_src), "main");
    let soa = clif_function(&aot_clif(&soa_src), "main");

    let aos_loop = loop_block(&aos);
    let soa_loop = loop_block(&soa);

    // AoS: the element index is scaled by the leaf count and offset
    // by the field's leaf before the byte scale — `(i * 3 + 2) * 8`,
    // two multiplies, striding past `x` and `y` on every iteration.
    assert_eq!(
        aos_loop.matches("imul").count(),
        2,
        "AoS loop should scale by leaf count and by stride:\n{aos_loop}"
    );
    assert!(
        aos_loop.contains("iconst.i64 3"),
        "AoS loop should multiply by the 3-leaf element:\n{aos_loop}"
    );

    // SoA: `mass` is its own array, so the address is `i * 8` from
    // that column's base — one multiply, unit stride, and nothing
    // read from the other two columns.
    assert_eq!(
        soa_loop.matches("imul").count(),
        1,
        "SoA loop should scale by the stride only:\n{soa_loop}"
    );
    assert_eq!(
        slots_touched(&soa_loop).len(),
        1,
        "SoA loop should touch exactly the mass column:\n{soa_loop}"
    );
    // And that one column is the third slot — leaves are allocated in
    // declaration order, so `mass` is column 2.
    assert_eq!(slots_touched(&soa_loop), vec!["ss2"], "loop:\n{soa_loop}");
    // The whole function still touches all three, since the literal
    // initialises every field.
    assert_eq!(slots_touched(&soa).len(), 3, "SoA frame:\n{soa}");
    assert_eq!(slots_touched(&aos), vec!["ss0"], "AoS frame:\n{aos}");
}

#[test]
fn narrow_leaf_columns_keep_the_uniform_stride_in_the_aot_frame() {
    // Phase 0.5's canary. Every column is `length * 8` today, whatever
    // the leaf's real width — the same per-leaf cost the interleaved
    // layout has always paid (`ARRAY_LEAF_STRIDE`). When the columns
    // drop to their leaves' widths this test fails with the new sizes,
    // which is the change being made rather than a regression: a
    // `bool` column becomes 3 bytes, `u8` 3, `f32` 12, `u64` 24.
    let src = |soa: &str| {
        format!(
            r#"
        struct Mixed {{ flag: bool, byte: u8, single: f32, wide: u64 }}

        fn main() -> u64 {{
            val ms: {soa}[Mixed; 3] = [
                Mixed {{ flag: true,  byte: 1u8, single: 1.5f32, wide: 10u64 }},
                Mixed {{ flag: false, byte: 2u8, single: 2.5f32, wide: 20u64 }},
                Mixed {{ flag: true,  byte: 3u8, single: 3.5f32, wide: 30u64 }},
            ]
            var bytes: u64 = 0u64
            for i in 0u64..3u64 {{
                bytes = bytes + ms[i].byte as u64
            }}
            bytes
        }}
    "#
        )
    };
    let aos = clif_function(&aot_clif(&src("")), "main");
    let soa = clif_function(&aot_clif(&src("soa ")), "main");

    // 3 elements * 4 leaves * 8 bytes, interleaved into one slot...
    assert_eq!(clif_stack_slots(&aos), vec![96], "AoS frame:\n{aos}");
    // ...or split into four 3 * 8 columns.
    assert_eq!(
        clif_stack_slots(&soa),
        vec![24, 24, 24, 24],
        "SoA frame:\n{soa}"
    );
    // The `byte` loop walks its own column at unit stride even though
    // a `u8` occupies an 8-byte slot in it.
    assert_eq!(slots_touched(&loop_block(&soa)), vec!["ss1"]);
}

#[test]
fn scalar_soa_shares_the_aos_aot_frame() {
    // One leaf is one column, so `soa [u64; N]` has nothing to
    // rearrange: `allocate_array_storage` returns the interleaved
    // slot for it, and the two frames are byte-identical.
    let src = |soa: &str| {
        format!(
            r#"
        fn main() -> u64 {{
            val xs: {soa}[u64; 3] = [4u64, 5u64, 6u64]
            var acc: u64 = 0u64
            for i in 0u64..3u64 {{
                acc = acc + xs[i]
            }}
            acc
        }}
    "#
        )
    };
    let aos = clif_function(&aot_clif(&src("")), "main");
    let soa = clif_function(&aot_clif(&src("soa ")), "main");
    assert_eq!(clif_stack_slots(&aos), vec![24]);
    assert_eq!(clif_stack_slots(&soa), clif_stack_slots(&aos));
    assert_eq!(loop_block(&soa), loop_block(&aos));
}

#[test]
fn an_unannotated_range_slice_inherits_the_source_layout_in_the_aot_frame() {
    // `val sub = ps[1..3]` keeps the source's placement; an
    // annotation overrides it in either direction. Only the frame can
    // tell these apart — every spelling computes the same answer,
    // which is why the behavioural test above passed while the
    // inheritance was in fact broken: an unannotated `val` carries
    // `TypeDecl::Unknown`, not `None`, and asking that whether it is
    // `soa` answered "no" and re-laid the slice out as AoS.
    let src = |annotation: &str| {
        format!(
            r#"
        struct Point {{ x: i64, y: i64 }}

        fn main() -> i64 {{
            val ps: soa [Point; 4] = [
                Point {{ x: 1i64, y: 2i64 }},
                Point {{ x: 3i64, y: 4i64 }},
                Point {{ x: 5i64, y: 6i64 }},
                Point {{ x: 7i64, y: 8i64 }},
            ]
            val sub{annotation} = ps[1i64..3i64]
            sub[0i64].x + sub[1i64].y
        }}
    "#
        )
    };
    // Source columns are 4 * 8 = 32 each; an inherited SoA slice is
    // two columns of 2 * 8 = 16, an AoS slice one slot of 2 * 2 * 8.
    let frame = |annotation: &str| clif_stack_slots(&clif_function(&aot_clif(&src(annotation)), "main"));
    assert_eq!(frame(""), vec![32, 32, 16, 16], "no annotation: inherit SoA");
    assert_eq!(frame(": soa [Point; 2]"), vec![32, 32, 16, 16], "explicit soa");
    assert_eq!(frame(": [Point; 2]"), vec![32, 32, 32], "explicit AoS re-layout");

    // All three spellings still agree on the answer (3 + 6 = 9).
    for annotation in ["", ": soa [Point; 2]", ": [Point; 2]"] {
        assert_eq!(interpreter_value(&src(annotation)) & 0xff, 9);
    }
    assert_consistent(&src(""), "soa_slice_inherits_layout");
}

// ---------------------------------------------------------------
// DATA-ORIENTED Phase 2 — `soa Vec<T>` / `SoaVec<T>`.
//
// The heap form cannot be a flag on `Vec<T>` the way the stack form
// is a flag on `[T; N]`: a heap buffer's layout is observable (what a
// grow has to move, what `as_ptr` would point at, what `retains(N)`
// reports), so `soa Vec<T>` is sugar for a *separate* stdlib type,
// rewritten in the parser. What these pin, therefore, is a pair of
// claims:
//
//   * the two types answer identically — same values, same allocation
//     totals, same drops — so a program can be moved between them;
//   * and the memory really is different, which for a heap buffer
//     shows up in the emitted address arithmetic rather than in a
//     stack frame.
// ---------------------------------------------------------------

/// The same program in both layouts. `{vec}` is the type's name and
/// `{new}` its constructor, which is the whole difference.
fn vec_program(ty: &str, new: &str) -> String {
    format!(
        r#"
        struct Particle {{ x: i64, y: i64, mass: i64 }}

        fn main() -> i64 {{
            var ps: {ty} = {new}
            var i: i64 = 0i64
            while i < 6i64 {{
                ps.push(Particle {{ x: i, y: i * 2i64, mass: 10i64 + i }})
                i = i + 1i64
            }}
            # one field over every element — the shape the layout is for
            var total: i64 = 0i64
            for p in ps.iter() {{
                total = total + p.mass
            }}
            # random access reads the whole element back
            val third: Particle = ps.get(2i64 as u64)
            ps.set(0u64, Particle {{ x: 100i64, y: 200i64, mass: 1i64 }})
            val first: Particle = ps.get(0u64)
            val popped: Particle = ps.pop()
            total + third.y + first.x + popped.mass + ps.size() as i64
        }}
    "#
    )
}

#[test]
fn soa_vec_answers_exactly_as_vec_does() {
    // 75 + 4 + 100 + 15 + 5 = 199. Both types must land there, and
    // each must agree across the tree-walker, the IR VM, the JIT and
    // the AOT binary. The loop grows the vec twice (0 -> 4 -> 8), so
    // this also covers the re-placement a column split needs instead
    // of a realloc.
    let aos = vec_program("Vec<Particle>", "Vec::new()");
    let soa = vec_program("soa Vec<Particle>", "SoaVec::new()");
    assert_eq!(interpreter_value(&aos) & 0xff, 199, "AoS: {aos}");
    assert_eq!(interpreter_value(&soa) & 0xff, 199, "SoA: {soa}");
    assert_consistent(&aos, "vec_particles_aos");
    assert_consistent(&soa, "vec_particles_soa");
}

#[test]
fn the_soa_vec_spelling_is_the_stdlib_type() {
    // `soa Vec<T>` is rewritten to `SoaVec<T>` in the parser, so the
    // two spellings are the *same* type, not two types that behave
    // alike: what gets lowered are `SoaVec`'s monomorphised methods,
    // and a value written one way passes where the other is declared.
    let sugar = vec_program("soa Vec<Particle>", "SoaVec::new()");
    let ir = lowered_ir(&sugar);
    assert!(
        ir.contains("toy_SoaVec__push__Struct") && ir.contains("toy_SoaVec__get__Struct"),
        "the sugar should lower SoaVec's methods:\n{ir}"
    );

    let crosses = r#"
        struct P { x: i64, y: i64 }

        fn total(ps: SoaVec<P>) -> i64 {
            var acc: i64 = 0i64
            for p in ps.iter() { acc = acc + p.x + p.y }
            acc
        }

        fn main() -> i64 {
            var ps: soa Vec<P> = SoaVec::new()
            ps.push(P { x: 1i64, y: 2i64 })
            ps.push(P { x: 3i64, y: 4i64 })
            total(ps)
        }
    "#;
    assert_eq!(interpreter_value(crosses) & 0xff, 10);

    // And the layouts stay apart where the stack form's do not: a
    // `soa Vec<T>` is not a `Vec<T>`, because for a heap container the
    // layout *is* observable. This is the deliberate asymmetry with
    // `soa [T; N]`, which is the same type as `[T; N]`.
    let mismatched = r#"
        struct P { x: i64, y: i64 }

        fn total(ps: Vec<P>) -> i64 { ps.size() as i64 }

        fn main() -> i64 {
            var ps: soa Vec<P> = SoaVec::new()
            ps.push(P { x: 1i64, y: 2i64 })
            total(ps)
        }
    "#;
    let errors = type_check_errors(mismatched);
    assert!(
        errors.iter().any(|e| e.contains("total")),
        "expected the call to be rejected, got: {errors:?}"
    );
}

#[test]
fn soa_vec_addresses_columns_where_vec_strides_elements() {
    // The layout claim itself, read off what the compiled lanes emit.
    // `Particle` is three 8-byte leaves, so:
    //
    //   Vec::get     one `mul` (index * elem_size), then the leaves
    //                at +0 / +8 / +16 from it — one element's worth
    //                of memory touched, three fields apart.
    //   SoaVec::get  a `mul` per leaf (index * 8, its own stride) and
    //                a `mul` per non-zero column base (8 * cap,
    //                16 * cap) — three columns, each addressed on its
    //                own, which is what lets a caller walk one of
    //                them without the others.
    let aos = lowered_function_starting_with(
        &lowered_ir(&vec_program("Vec<Particle>", "Vec::new()")),
        "Vec__get__Struct",
    );
    let soa = lowered_function_starting_with(
        &lowered_ir(&vec_program("soa Vec<Particle>", "SoaVec::new()")),
        "SoaVec__get__Struct",
    );

    assert_eq!(aos.matches(" = mul ").count(), 1, "Vec::get:\n{aos}");
    assert_eq!(aos.matches(" = add ").count(), 2, "Vec::get:\n{aos}");
    // The interleaved leaf offsets are constants added to the one
    // element base.
    assert!(aos.contains("const 8u64") && aos.contains("const 16u64"), "Vec::get:\n{aos}");

    // Three index scalings plus two column bases.
    assert_eq!(soa.matches(" = mul ").count(), 5, "SoaVec::get:\n{soa}");
    assert_eq!(soa.matches(" = add ").count(), 2, "SoaVec::get:\n{soa}");
    // Both read three leaves; only where they read them differs.
    assert_eq!(aos.matches("ptr_read").count(), 3, "Vec::get:\n{aos}");
    assert_eq!(soa.matches("ptr_read").count(), 3, "SoaVec::get:\n{soa}");
}

#[test]
fn a_scalar_element_has_nothing_to_split() {
    // One leaf is one column, so `SoaVec<u64>` addresses
    // `index * 8` — byte-for-byte what `Vec<u64>` does. The type
    // stays distinct (it is a different nominal type), but the memory
    // is the same, exactly as `soa [u64; N]` degenerates to the AoS
    // slot in Phase 0.
    let src = |ty: &str, new: &str| {
        format!(
            r#"
        fn main() -> u64 {{
            var xs: {ty} = {new}
            var i: u64 = 0u64
            while i < 6u64 {{
                xs.push(i * 2u64)
                i = i + 1u64
            }}
            var acc: u64 = 0u64
            for x in xs.iter() {{ acc = acc + x }}
            acc + xs.get(3u64) + xs.capacity()
        }}
    "#
        )
    };
    let aos = src("Vec<u64>", "Vec::new()");
    let soa = src("soa Vec<u64>", "SoaVec::new()");
    // 30 + 6 + 8 = 44.
    assert_eq!(interpreter_value(&aos) & 0xff, 44);
    assert_eq!(interpreter_value(&soa) & 0xff, 44);
    assert_consistent(&soa, "soa_vec_scalar");

    let aos_get = lowered_function_starting_with(&lowered_ir(&aos), "Vec__get__U64");
    let soa_get = lowered_function_starting_with(&lowered_ir(&soa), "SoaVec__get__U64");
    assert_eq!(aos_get.matches("ptr_read").count(), 1);
    assert_eq!(soa_get.matches("ptr_read").count(), 1);
    assert_eq!(soa_get.matches(" = mul ").count(), 1, "SoaVec::get:\n{soa_get}");
    assert_eq!(soa_get.matches(" = add ").count(), 0, "SoaVec::get:\n{soa_get}");
}

#[test]
fn soa_vec_allocates_exactly_what_vec_allocates() {
    // Columns are tight from the start — each strides by its leaf's
    // own width, and their total is `__builtin_sizeof::<T>()` — so a
    // `SoaVec` of `cap` elements is the same number of bytes as the
    // `Vec`. (This is where the stack form still differs: Phase 0
    // pads every column to 8 bytes and Phase 0.5 is the change that
    // closes it.) The grow path allocates a fresh buffer and frees
    // the old one where `Vec` reallocs, which the counters do record
    // — as one alloc plus one free instead of one realloc.
    memory_profiles_agree(
        &vec_program("soa Vec<Particle>", "SoaVec::new()"),
        "prof_soa_vec_growth",
    );
}

#[test]
fn soa_vec_drop_glue_releases_owning_elements() {
    // A `SoaVec<Box<i64>>` owns its boxes: the backend's glue walks
    // the columns (`drop_glue.rs` shares the walk with `Vec`, which
    // has the same four fields) and frees each element before the
    // buffer. Without the column-aware walk the boxes would leak
    // silently — the values would still be right.
    let src = r#"
        fn main() -> i64 {
            var bs: soa Vec<Box<i64>> = SoaVec::new()
            val b1: Box<i64> = Box::new(1i64)
            val b2: Box<i64> = Box::new(2i64)
            val b3: Box<i64> = Box::new(3i64)
            bs.push(b1)
            bs.push(b2)
            bs.push(b3)
            var total: i64 = 0i64
            var i: u64 = 0u64
            while i < bs.size() {
                val b: Box<i64> = bs.get(i)
                total = total + b.get()
                i = i + 1u64
            }
            total
        }
    "#;
    assert_eq!(interpreter_value(src) & 0xff, 6);
    assert_consistent(src, "soa_vec_boxes");
    // Every box and the buffer are freed: nothing outlives `main`,
    // on any of the lanes (the report is the one all three agreed on).
    let report = memory_profile_report(src, "prof_soa_vec_boxes");
    assert!(
        report.contains("live_bytes        0"),
        "something leaked:\n{report}"
    );
}
