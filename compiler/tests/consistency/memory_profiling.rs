//! `--profile=mem`: counters, attribution, layout reports, and the JSON
//! form -- all required to agree across backends.

use std::process::Command;

use compiler::{compile_file, CompilerOptions};
use interpreter::RunOptions;

use super::harness::*;

#[test]
fn neither_heap_reuses_addresses() {
    // DROP-GLUE: the compiled runtime's bump region never reuses a
    // freed address either, so a drop-glue walk that reaches the same
    // boxed node twice reads the block's original contents on the
    // second visit (the free is an idempotent no-op). This used to be
    // a documented interpreter-vs-AOT divergence (the AOT was libc
    // malloc); the glue made reuse observable through crashes, so the
    // AOT heap now mirrors the interpreter's bump allocator.
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            val a: ptr = __builtin_heap_alloc(64u64)
            __builtin_heap_free(a)
            val b: ptr = __builtin_heap_alloc(64u64)
            if __builtin_ptr_eq(a, b) { 1u64 } else { 0u64 }
        }
    "#;
    assert_eq!(
        interpreter_value(src),
        0,
        "the interpreter heap is a bump allocator; if this now reuses, \
         MEMORY_PROFILING's reasoning about address-derived metrics needs revisiting"
    );
    assert_eq!(
        compiler_exit_code(src, "heap_addr_reuse", false),
        0,
        "the AOT bump region must not reuse a freed address either"
    );
}

// --- MEMORY_PROFILING M1: allocation totals across backends ---------
//
// The phase's acceptance criterion. Every counter is defined on the
// sizes and order the program requested, so the backends have to agree
// on them even though their heaps behave differently (see
// `interpreter_heap_does_not_reuse_addresses_but_the_aot_heap_does`).
//
// These compare through the `--all-backends --profile=mem` path, which
// is also what a user runs.


#[test]
fn allocation_totals_agree_for_raw_heap_builtins() {
    memory_profiles_agree(
        r#"
        fn main() -> u64 {
            val a: ptr = __builtin_heap_alloc(64u64)
            val b: ptr = __builtin_heap_alloc(96u64)
            __builtin_heap_free(a)
            val c: ptr = __builtin_heap_realloc(b, 160u64)
            __builtin_heap_free(c)
            0u64
        }
        "#,
        "prof_raw_builtins",
    );
}

#[test]
fn allocation_totals_agree_for_a_growing_vec() {
    // A `Vec` that outgrows its capacity several times exercises the
    // realloc accounting, which is where the definitions bite: this
    // implementation moves the block, and the numbers must not say so.
    memory_profiles_agree(
        r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            var i: u64 = 0u64
            while i < 40u64 {
                v.push(i)
                i = i + 1u64
            }
            v.size()
        }
        "#,
        "prof_vec_growth",
    );
}

/// `String::from_str` on a literal used to diverge: the interpreter
/// materialised the str literal on its heap (one extra allocation),
/// while the compiled backends pointed into `.rodata` and allocated
/// nothing (STR-PTR-LEN). The IR VM now materialises literals
/// counter-free, so the accounting agrees once more — this used to be
/// recorded as a known difference, and its disappearance is the test.
#[test]
fn string_literals_no_longer_allocate_differently_across_backends() {
    memory_profiles_agree(
        "fn main() -> u64 {\n    val s = String::from_str(\"hello world\")\n    s.len()\n}\n",
        "prof_string_from_str",
    );
}

/// STRING-NO-DROP: a `String` frees its buffer when it dies.
///
/// `Vec<T>` owned its allocation from the start; `String` -- the same
/// three fields over the same buffer -- did not, so every string a
/// program built leaked. The number this pins is `live_bytes 0`: the
/// program allocates (`from_str`, then a `to_ascii_upper` that builds
/// a second buffer) and ends owing nothing.
#[test]
fn a_string_frees_its_buffer_when_it_dies() {
    let report = memory_profile_report(
        r#"
        fn main() -> u64 {
            var s: String = String::from_str("hello")
            s.push_str(" world")
            val t: String = s.to_ascii_upper()
            t.len()
        }
        "#,
        "prof_string_drop",
    );
    if report.is_empty() {
        return; // e2e skipped
    }
    assert!(
        report.contains("live_bytes        0"),
        "a String should free its buffer; report was:\n{report}"
    );
}

/// The same for the element type of a container.
///
/// `Vec::sort` used to bind each element to a local through `get`,
/// which carries drop glue -- so sorting a `Vec<String>` freed the
/// buffers the vector still held, and the next read of one hit a dead
/// allocation. It addresses the buffer directly now, the way
/// `contains` / `index_of` always did. This pins that a sorted vector
/// of strings still reads back, on every lane.
#[test]
fn sorting_a_vec_of_strings_does_not_free_what_it_sorts() {
    memory_profiles_agree(
        r#"
        fn main() -> u64 {
            var v: Vec<String> = Vec::new()
            val a: String = String::from_str("pear")
            v.push(a)
            val b: String = String::from_str("apple")
            v.push(b)
            val c: String = String::from_str("fig")
            v.push(c)
            v.sort()
            val first: &String = v.borrow(0u64)
            val want: String = String::from_str("apple")
            if first == want { 0u64 } else { 1u64 }
        }
        "#,
        "prof_string_vec_sort",
    );
}

// --- MEMORY_PROFILING M2: attribution -------------------------------
//
// The site identifier *is* the allocation's source position, packed as
// `(line << 32) | column`. Every backend reads it from the same
// location pool, so the leak report has to name the same place without
// any shared table of ids to keep in step.

#[test]
fn leaks_are_attributed_to_the_same_source_position_on_every_backend() {
    // `memory_profiles_agree` fails on any disagreement, and the
    // `--all-backends` path compares the leak sections as well as the
    // totals.
    memory_profiles_agree(
        r#"
        fn main() -> u64 {
            val a: ptr = __builtin_heap_alloc(64u64)
            val b: ptr = __builtin_heap_alloc(32u64)
            __builtin_heap_free(a)
            0u64
        }
        "#,
        "prof_leak_sites",
    );
}

#[test]
fn allocations_from_one_site_reached_by_several_callers_aggregate_together() {
    // Attribution is per allocation *site*, not per call path: both
    // calls to `keep` land on the same line and are reported as one
    // site. Recording the granularity so a later phase that adds call
    // paths has something to change deliberately.
    memory_profiles_agree(
        r#"
        fn keep(n: u64) -> ptr {
            __builtin_heap_alloc(n)
        }

        fn main() -> u64 {
            val a: ptr = keep(16u64)
            val b: ptr = keep(24u64)
            val c: ptr = __builtin_heap_alloc(48u64)
            __builtin_heap_free(c)
            0u64
        }
        "#,
        "prof_site_aggregation",
    );
}

// --- MEMORY_PROFILING M3: layout reporting --------------------------
//
// Fragmentation is a property of an allocator's layout, so `trait
// Alloc` reports it and the profiler only collects. The default is
// "not reported", which is not the same as "zero fragmentation" —
// `Global`, `Arena` and `FixedBuffer` all answer that way because none
// of them owns a region: each forwards individual allocations to the
// default allocator and keeps bookkeeping on the side.

#[test]
fn allocators_without_a_region_report_no_layout() {
    let src = r#"
        fn main() -> u64 {
            val a = Arena::new()
            val fb = FixedBuffer::new(1024u64)
            val g = Global::new()
            # Bound first: chained method calls are not lowerable by
            # the AOT MVP.
            val la = a.layout_report()
            val lf = fb.layout_report()
            val lg = g.layout_report()
            var known: u64 = 0u64
            if la.is_known() { known = known + 1u64 }
            if lf.is_known() { known = known + 1u64 }
            if lg.is_known() { known = known + 1u64 }
            known
        }
    "#;
    assert_consistent(src, "layout_opaque");
}

#[test]
fn a_region_owning_allocator_reports_its_layout() {
    // 8 slots of 16 bytes; two live, and freeing the middle one splits
    // the free space into two runs.
    let src = r#"
        fn main() -> u64 {
            var r = SlotRegion::new(16u64, 8u64)
            val a = r.alloc(16u64)
            val b = r.alloc(16u64)
            val c = r.alloc(16u64)
            r.free(b)
            val l = r.layout_report()
            l.managed() + l.live() * 1000u64
                + l.blocks() * 1000000u64 + l.largest() * 10000000u64
        }
    "#;
    // managed 128, live 32, 2 free runs, largest run 5 slots = 80 bytes.
    assert_consistent(src, "layout_region");
}

#[test]
fn fragmentation_is_reported_and_actually_bites() {
    // Freeing every other slot leaves 48 bytes free with no run longer
    // than 16, so a 48-byte request fails. A number that did not
    // predict that would be decoration.
    let src = r#"
        fn main() -> u64 {
            var r = SlotRegion::new(16u64, 6u64)
            val a = r.alloc(16u64)
            val b = r.alloc(16u64)
            val c = r.alloc(16u64)
            val d = r.alloc(16u64)
            val e = r.alloc(16u64)
            val f = r.alloc(16u64)
            r.free(b)
            r.free(d)
            r.free(f)
            val l = r.layout_report()
            val big = r.alloc(48u64)
            var code: u64 = 0u64
            if __builtin_ptr_is_null(big) { code = code + 1u64 }
            code + l.largest() * 10u64 + l.external_fragmentation_permille() * 1000u64
        }
    "#;
    // 1 (the request failed) + 16 * 10 + 666 * 1000.
    assert_consistent(src, "layout_fragmentation");
}

// --- MEMORY_PROFILING M4: the report as JSON -------------------------
//
// The phase's acceptance criterion is that the report is byte-identical
// between runs, which is what makes it diffable and assertable. These
// check that, and that the C runtime's hand-written mirror produces the
// same bytes as the shared Rust one — the two are written out
// separately (see `MemoryStats::report_json`), so nothing but a test
// keeps them together.




#[test]
fn the_json_report_is_identical_between_runs() {
    let first = interpreter_json_profile(JSON_PROFILE_PROGRAM);
    let second = interpreter_json_profile(JSON_PROFILE_PROGRAM);
    assert_eq!(
        first, second,
        "the same program produced two different reports; a report that \
         is not reproducible cannot be diffed or asserted on"
    );
    assert_eq!(first, JSON_PROFILE_EXPECTED);
}

#[test]
fn the_aot_json_report_is_byte_identical_to_the_shared_one() {
    if skip_e2e() {
        return;
    }
    let src_path = unique_path("prof_json.t");
    std::fs::write(&src_path, JSON_PROFILE_PROGRAM).expect("write source");
    let exe_path = unique_path("prof_json");
    let mut options = CompilerOptions::new(src_path.clone());
    options.output = Some(exe_path.clone());
    options.link_cache_dir = Some(link_cache_dir_for_tests());
    compile_file(&options).expect("compile");

    let run = || {
        let output = Command::new(&exe_path)
            .env("TOY_PROFILE_MEM", "json")
            .output()
            .expect("spawn binary");
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    let first = run();
    let second = run();
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);

    assert_eq!(first, second, "the compiled runtime's report is not reproducible");
    // The compiled binary names the entry after the input path, which
    // this test's is a temp file — so the comparison lane has to agree
    // on that name rather than on `JSON_PROFILE_EXPECTED`'s `test.t`.
    // The run above already proved reproducibility; this proves the two
    // implementations still render the report byte for byte.
    let shared = interpreter_json_profile_as(JSON_PROFILE_PROGRAM, src_path.to_string_lossy().as_ref());
    assert_eq!(
        first, shared,
        "the C runtime's JSON has drifted from `MemoryStats::report_json`"
    );
}

#[test]
fn nothing_leaked_is_an_empty_array_not_a_missing_section() {
    // The text report omits the leak section entirely when there is
    // nothing to say, which is right for a human and wrong for a
    // consumer: absence would have to be told apart from a producer
    // that predates leak reporting.
    let src = "fn main() -> u64 {\n\
        \x20   val p: ptr = __builtin_heap_alloc(16u64)\n\
        \x20   __builtin_heap_free(p)\n\
        \x20   0u64\n\
        }\n";
    let json = interpreter_json_profile(src);
    assert!(
        json.contains("\"leaks\": [],\n"),
        "expected an empty leaks array, got:\n{json}"
    );
    assert!(json.contains("\"live_bytes\": 0,"), "got:\n{json}");
}

// --- MEMORY_PROFILING M4: reading the counters from the program ------
//
// The point of the phase: `requires` / `ensures` and `test` blocks can
// assert on memory, which only works if the counters answer truthfully
// in an ordinary run. The compiled runtime counts nothing unless asked,
// so lowering emits a `MemStatEnable` at the top of `main` when the
// program reads a counter — these check that it actually took effect,
// because a backend that answered 0 would make every such contract
// pass while checking nothing.

#[test]
fn every_backend_agrees_on_what_the_counters_say() {
    let src = r#"
        fn main() -> u64 {
            val before: u64 = __builtin_live_bytes()
            val p: ptr = __builtin_heap_alloc(64u64)
            val during: u64 = __builtin_live_bytes()
            __builtin_heap_free(p)
            val after: u64 = __builtin_live_bytes()
            val n: u64 = __builtin_alloc_count()
            val peak: u64 = __builtin_peak_live_bytes()
            # before=0, during=64, after=0, n=1, peak=64
            before + during + after * 100u64 + n * 1000u64 + peak * 10000u64
        }
    "#;
    assert_consistent(src, "mem_stat_read");
}

#[test]
fn the_compiled_binary_counts_without_being_asked_to_profile() {
    // The one that would silently rot: `TOY_PROFILE_MEM` is unset here,
    // so the C runtime's counting is off unless `main` turned it on.
    // Written as a direct exit-code check rather than through
    // `assert_consistent` so the failure says "the AOT answered 0"
    // rather than "the backends disagree".
    if skip_e2e() {
        return;
    }
    let src = r#"
        fn main() -> u64 {
            val p: ptr = __builtin_heap_alloc(64u64)
            val live: u64 = __builtin_live_bytes()
            __builtin_heap_free(p)
            live
        }
    "#;
    assert_eq!(
        compiler_exit_code(src, "mem_stat_unprofiled", false),
        64,
        "the compiled binary reported no live bytes; `MemStatEnable` is \
         not reaching `toy_prof_force_counting`"
    );
}

#[test]
fn a_program_that_reads_no_counter_does_not_ask_for_counting() {
    // The other half of the bargain: an unprofiled run has to allocate
    // exactly what it did before the profiler existed, so the enable
    // call appears only when something reads a counter.
    let with_read = "fn main() -> u64 { __builtin_live_bytes() }\n";
    let without = "fn main() -> u64 {\n    val p: ptr = __builtin_heap_alloc(8u64)\n    __builtin_heap_free(p)\n    0u64\n}\n";
    assert!(
        lowered_ir(with_read).contains("mem_stat_enable"),
        "a program that reads a counter must enable counting"
    );
    assert!(
        !lowered_ir(without).contains("mem_stat_enable"),
        "a program that reads no counter must not pay for counting"
    );
}

// --- MEMORY_PROFILING M3 residual: allocator layout in the report ----
//
// A region-owning allocator registers its final layout from `Drop`, and
// `--profile=mem` folds it into the report. The numbers are hand-checked
// here, and the section is byte-identical across the interpreter and the
// C runtime (the counter totals deliberately are *not* compared: the
// `"SlotRegion"` str literal is materialised on the interpreter's heap
// but lives in `.rodata` for the compiler — the same known difference as
// `string_literals_allocate_on_the_interpreter_but_not_when_compiled`).




#[test]
fn allocator_layouts_are_reported_in_the_memory_profile() {
    let report = interpreter_layout_report(LAYOUT_PROFILE_PROGRAM);
    assert_eq!(
        report, LAYOUT_PROFILE_EXPECTED,
        "the layout section drifted from the hand-computed numbers"
    );
}

#[test]
fn the_aot_layout_report_is_byte_identical_to_the_shared_one() {
    if skip_e2e() {
        return;
    }
    let src_path = unique_path("prof_layout.t");
    std::fs::write(&src_path, LAYOUT_PROFILE_PROGRAM).expect("write source");
    let exe_path = unique_path("prof_layout");
    let mut options = CompilerOptions::new(src_path.clone());
    options.output = Some(exe_path.clone());
    options.core_modules_dirs = vec![core_modules_dir()];
    options.link_cache_dir = Some(link_cache_dir_for_tests());
    compile_file(&options).expect("compile");

    let output = Command::new(&exe_path)
        .env("TOY_PROFILE_MEM", "1")
        .output()
        .expect("spawn binary");
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let aot_layouts = stderr
        .split("allocator layouts\n")
        .nth(1)
        .unwrap_or_default();
    let aot_layouts = format!("allocator layouts\n{aot_layouts}");

    assert_eq!(
        aot_layouts, LAYOUT_PROFILE_EXPECTED,
        "the C runtime's layout report has drifted from the shared one"
    );
    assert_eq!(
        interpreter_layout_report(LAYOUT_PROFILE_PROGRAM),
        aot_layouts,
        "the interpreter and the C runtime disagree on the layout report"
    );
}

#[test]
fn allocators_without_a_region_still_report_no_layout() {
    // `Arena` / `FixedBuffer` / `Global` forward everything to the
    // default allocator and own no region, so none of them implements
    // a `Drop` that registers a layout — the report is simply absent.
    let src = "fn main() -> u64 {\n\
        \x20   val a = Arena::new()\n\
        \x20   val fb = FixedBuffer::new(1024u64)\n\
        \x20   a.bytes_used() + fb.used()\n\
        }\n";
    let report = interpreter_layout_report(src);
    assert_eq!(
        report, "",
        "allocators without a region must not register a layout, got:\n{report}"
    );
}

// --- Pointer arithmetic: interior pointers (MEMORY-PROFILING M3 residual) ---
//
// `__builtin_ptr_offset(base, offset)` makes a pointer into the middle of
// an allocation. That is the primitive an offset-based free-list / region
// allocator is built on: allocate one block, hand out sub-blocks.

#[test]
fn interior_pointers_read_and_write_independently() {
    // One 64-byte block split into two 32-byte cells. Writes through the
    // two interior pointers must land in disjoint regions, and a write
    // through one must be visible at the same offset of the base block.
    let src = r#"
        unsafe fn main() -> u64 {
            val block: ptr = __builtin_heap_alloc(64u64)
            val cell0: ptr = __builtin_ptr_offset(block, 0u64)
            val cell1: ptr = __builtin_ptr_offset(block, 32u64)
            __builtin_ptr_write(cell0, 0u64, 111u64)
            __builtin_ptr_write(cell1, 0u64, 222u64)
            val a: u64 = __builtin_ptr_read::<u64>(cell0, 0u64)
            val b: u64 = __builtin_ptr_read::<u64>(cell1, 0u64)
            # The interior pointer of cell1 aliases base + 32.
            val c: u64 = __builtin_ptr_read::<u64>(block, 32u64)
            __builtin_heap_free(block)
            a + b + c
        }
    "#;
    // 111 + 222 + 222.
    assert_consistent(src, "ptr_offset_cells");
}

#[test]
fn interior_pointers_compose() {
    // Offset from an interior pointer reaches the same address as the
    // equivalent offset from the base — the value is plain addition.
    let src = r#"
        unsafe fn main() -> u64 {
            val block: ptr = __builtin_heap_alloc(64u64)
            val half: ptr = __builtin_ptr_offset(block, 32u64)
            val quarter: ptr = __builtin_ptr_offset(half, 16u64)
            __builtin_ptr_write(quarter, 0u64, 99u64)
            val v: u64 = __builtin_ptr_read::<u64>(block, 48u64)
            v
        }
    "#;
    assert_consistent(src, "ptr_offset_compose");
}

#[test]
fn a_struct_returned_from_its_constructor_is_not_dropped() {
    // The constructor's local `var r` must be moved out, not dropped:
    // dropping it would free `r.ptrs`, and the caller's binding drop
    // would then free the same pointer a second time (use-after-free
    // that crashed the AOT). A clean single free on every backend is
    // the pass condition.
    let src = r#"
        struct Region { ptrs: ptr }

        impl Region {
            fn new() -> Self {
                var r = Region { ptrs: __builtin_null_ptr() }
                with allocator = __builtin_default_allocator() {
                    r.ptrs = __builtin_heap_alloc(16u64)
                }
                r
            }
        }

        impl Drop for Region {
            fn drop(&mut self) {
                __builtin_heap_free(self.ptrs)
            }
        }

        fn main() -> u64 {
            val r = Region::new()
            42u64
        }
    "#;
    assert_consistent(src, "drop_returned_binding");
}





#[test]
fn an_abandoned_execution_attempt_is_not_counted_against_the_next_one() {
    // A run tries the IR VM before the tree-walker, and an engine that
    // fails partway has already allocated. Rolling the counters back at
    // the fallback is what keeps a memory contract pointing at the
    // function that broke it.
    //
    // Here `tidy` frees what it takes and `hoggy` does not. Under the
    // IR VM, `hoggy` violates its bound and the run restarts on the
    // tree-walker — with `hoggy`'s 4096 bytes still counted as live,
    // `tidy` was the first to fail on the way through, and the message
    // named the one function that was behaving.
    let src = "fn tidy(n: u64) -> u64\n\
        \x20   ensures __builtin_live_bytes() <= 128u64\n\
        {\n\
        \x20   val p: ptr = __builtin_heap_alloc(n)\n\
        \x20   __builtin_heap_free(p)\n\
        \x20   n\n\
        }\n\
        \n\
        fn hoggy(n: u64) -> u64\n\
        \x20   ensures __builtin_live_bytes() <= 128u64\n\
        {\n\
        \x20   val p: ptr = __builtin_heap_alloc(n)\n\
        \x20   n\n\
        }\n\
        \n\
        fn main() -> u64 {\n\
        \x20   val a: u64 = tidy(64u64)\n\
        \x20   val b: u64 = hoggy(4096u64)\n\
        \x20   a + b\n\
        }\n";
    let options = RunOptions::default();
    let err = interpreter::run_source(src, "test.t", &options)
        .expect_err("hoggy leaks 4096 bytes against a 128-byte bound");
    assert!(
        err.contains("function `hoggy`"),
        "the violation should name the function that leaked, got: {err}"
    );
}

// --- `Display` (core/std/fmt.t) ---------------------------------
//
// A type with a `to_str(&self) -> str` method controls what `print` /
// `println` write and what string interpolation splices in. The type
// checker rewrites the argument of those builtins to call it, so every
// backend sees an ordinary method call.
//
// Each test pins the text as well as cross-backend agreement.
// `assert_stdout_consistent` alone would not: with the dispatch turned
// off, every backend renders structurally and they still agree with
// each other, so the test would pass while the feature did nothing.

// ERROR_MODEL E5: an allocation that fails is noticed.
//
// Before this, `Vec` / `String` / `Box` did not look at what the
// allocator handed back, so a failed request became a write through
// address 0. Nothing could be tested about it either: the interpreter's
// heap went through Rust's allocator, which *aborts* on failure, so the
// four lanes could not even agree that an allocation can fail.

#[test]
fn an_allocation_that_cannot_be_served_is_reported_not_written_through() {
    // 2^45 elements of 8 bytes = 256 TB. No allocator serves it, and
    // asking is cheap -- nothing is touched.
    let src = r#"
        fn main() -> u64 {
            val big: u64 = 1u64 << 45u64
            val r: Result<Vec<u64>, AllocError> = Vec::try_with_capacity(big)
            match r {
                Result::Ok(_) => { println("unexpectedly succeeded") }
                Result::Err(e) => { println(e) }
            }
            # A different fix, so a different variant: no budget makes
            # a byte count that does not fit in u64 possible.
            val r2: Result<Vec<u64>, AllocError> =
                Vec::try_with_capacity(18446744073709551615u64)
            match r2 {
                Result::Ok(_) => { println("unexpectedly succeeded") }
                Result::Err(e) => { println(e) }
            }
            0u64
        }
    "#;
    assert_stdout_consistent(src, "alloc_failure_reported");
}

#[test]
fn an_empty_container_is_not_an_allocation_failure() {
    // The regression this guards is total: `heap_alloc(0)` returns null
    // by contract, so reading every null as failure would make every
    // `Vec::new()` and every empty `String` panic.
    let src = r#"
        fn main() -> u64 {
            val v: Vec<u64> = Vec::new()
            val w: Vec<u64> = Vec::with_capacity(0u64)
            val s = String::new()
            val t = String::from_str("")
            v.size() + w.size() + s.size() + t.size()
        }
    "#;
    assert_consistent(src, "empty_container_not_a_failure");
}

#[test]
fn a_refused_reservation_leaves_the_vector_usable_and_leaks_nothing() {
    // `realloc` leaves the original block alone when it fails, so the
    // check has to happen before `self.data` is assigned -- otherwise
    // the old pointer is lost (a leak) and every later element is
    // written to address 0. The proof is that the vector still works
    // afterwards and the run ends with nothing outstanding.
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(1u64)
            v.push(2u64)
            val big: u64 = 1u64 << 45u64
            val refused = v.try_reserve(big)
            match refused {
                Result::Ok(_) => { println("unexpectedly succeeded") }
                Result::Err(e) => { println(e) }
            }
            # Still intact: the elements are there and it still grows.
            v.push(3u64)
            println(v.get(0u64) + v.get(1u64) + v.get(2u64))
            0u64
        }
    "#;
    let report = memory_profile_report(src, "refused_reservation_no_leak");
    if report.is_empty() {
        return;
    }
    let live = report
        .lines()
        .find_map(|l| l.trim().strip_prefix("live_bytes"))
        .map(|v| v.trim().to_string());
    assert_eq!(
        live.as_deref(),
        Some("0"),
        "a refused reservation lost the original buffer:\n{report}"
    );
}

/// HEAP-CHECK H0: the tree-walker counts double frees in the same words
/// as the compiled lanes (`all_backends_cli.rs` pins those), including a
/// block a resize moved. It is the lane the harness treats as the
/// oracle, and `--all-backends` does not run it.
#[test]
fn the_tree_walker_reports_double_frees_like_the_other_lanes() {
    let src = "\
fn main() -> u64 {
    val p: ptr = __builtin_heap_alloc(16u64)
    __builtin_heap_free(p)
    __builtin_heap_free(p)
    val q: ptr = __builtin_heap_alloc(8u64)
    val r: ptr = __builtin_heap_realloc(q, 64u64)
    __builtin_heap_free(q)
    __builtin_heap_free(r)
    0u64
}
";
    assert_eq!(
        tree_walker_heap_check_report(src),
        "heap check: 2 double frees (2 distinct)\n\
         \x20 x1  in main: allocated at test.t:2:18, freed at test.t:3:5, freed again at test.t:4:5\n\
         \x20 x1  in main: allocated at test.t:5:18, moved by a resize, freed again at test.t:7:5\n"
    );
}

/// DOUBLE-DROP-LANE-DIVERGENCE: every lane frees each node of the Box
/// list once. The lanes that share `compiler_lower` used to free six of
/// them twice: `sum(l: &List)` matched its borrowed list, and each arm
/// took a drop target for the `Box` payload, freeing the caller's nodes
/// -- which the caller freed again. A `&T` parameter's and a `&self`
/// receiver's leaves are the caller's now, as a `borrow()` result's
/// already were.
#[test]
fn the_tree_walker_frees_each_box_once() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../interpreter/example/box_linked_list.t"
    ))
    .expect("read example");
    assert_eq!(tree_walker_heap_check_report(&src), "heap check: 0 double frees (0 distinct)\n");
}

/// DOUBLE-DROP-LANE-DIVERGENCE: `Vec<String>::sort` reads elements
/// through `Ptr::get` (`val key: T = p.get(i)`), an alias of the slot.
/// The tree-walker treated only the raw `__builtin_ptr_read` as one, so
/// it dropped every string `sort` touched and the vector freed them
/// again (5 double frees in `std_ord_sort.t`); the compiled lanes never
/// did.
#[test]
fn sorting_strings_frees_each_once_on_the_tree_walker() {
    let src = r#"
        fn main() -> u64 {
            var words: Vec<String> = Vec::new()
            words.push(String::from_str("pear"))
            words.push(String::from_str("apple"))
            words.push(String::from_str("fig"))
            words.sort()
            val first: &String = words.borrow(0u64)
            first.len()
        }
    "#;
    assert_eq!(tree_walker_heap_check_report(src), "heap check: 0 double frees (0 distinct)\n");
    assert_eq!(interpreter_value(src), 5);
    assert_consistent(src, "sort_strings_once");
}

/// DOUBLE-DROP-LANE-DIVERGENCE: the tree-walker's column window
/// (`vs.mass` over a `soa Vec`) holds its source in place of an
/// address. Dropping the window dropped the `SoaVec` it viewed, which
/// then dropped itself again (`soa_column.t`, found by HEAP-CHECK).
#[test]
fn a_column_window_owns_nothing_on_the_tree_walker() {
    let src = r#"
        struct P { x: u64, mass: u64 }
        fn total(ms: Column<u64>) -> u64 {
            var t: u64 = 0u64
            var i: u64 = 0u64
            while i < ms.len() {
                t = t + ms.get(i)
                i = i + 1u64
            }
            t
        }
        fn main() -> u64 {
            var vs: soa Vec<P> = SoaVec::new()
            vs.push(P { x: 1u64, mass: 3u64 })
            vs.push(P { x: 2u64, mass: 5u64 })
            val ms = vs.mass
            total(ms)
        }
    "#;
    assert_eq!(tree_walker_heap_check_report(src), "heap check: 0 double frees (0 distinct)\n");
    assert_eq!(interpreter_value(src), 8);
    assert_consistent(src, "column_window_owns_nothing");
}

/// DOUBLE-DROP-LANE-DIVERGENCE: `val v = f()?` over a compound payload.
/// The desugar `{ val t = f()  match t { Ok(p) => p, Err(e) => .. } }`
/// hands the payload into `v` on the `Ok` path, but `t` kept its drop
/// flag set, so the lanes that share the lowering freed the vector twice
/// (`try_compound.t`, found by HEAP-CHECK). The block's tail is now a
/// transfer out of `t` into the binding.
#[test]
fn a_question_mark_hands_its_payload_over_once() {
    let src = r#"
        fn digits(n: u64) -> Result<Vec<u64>, str> {
            if n == 0u64 { return Result::Err("no digits") }
            var out: Vec<u64> = Vec::new()
            out.push(n)
            Result::Ok(out)
        }
        fn digit_sum(n: u64) -> Result<u64, str> {
            val v = digits(n)?
            Result::Ok(v.size())
        }
        fn main() -> u64 {
            val a = digit_sum(5u64)
            val b = digit_sum(0u64)
            val x = match a { Result::Ok(x) => x, Result::Err(_) => 100u64 }
            val y = match b { Result::Ok(y) => y, Result::Err(_) => 10u64 }
            x + y
        }
    "#;
    assert_eq!(tree_walker_heap_check_report(src), "heap check: 0 double frees (0 distinct)\n");
    assert_eq!(interpreter_value(src), 11);
    assert_consistent(src, "question_mark_payload_once");
    memory_profiles_agree(src, "question_mark_payload_once");
}

/// DOUBLE-DROP-LANE-DIVERGENCE: a method that takes its receiver by
/// value (`fn finish(self: Self) -> String { self.out }`) consumes it.
/// The move check treated every receiver as a read, so after
/// `val part = w.finish()` both `part` and `w` owned the buffer and
/// every lane freed it twice (`json_config.t` via `JsonWriter`, found by
/// HEAP-CHECK).
#[test]
fn a_consuming_method_takes_its_receiver() {
    let src = r#"
        struct Writer { out: String, n: u64 }
        impl Writer {
            fn new() -> Self { Writer { out: String::new(), n: 0u64 } }
            fn put(&mut self, s: str) { self.out.push_str(s)  self.n = self.n + 1u64 }
            fn finish(self: Self) -> String { self.out }
        }
        fn main() -> u64 {
            var w: Writer = Writer::new()
            w.put("ab")
            w.put("cde")
            val part: String = w.finish()
            part.len()
        }
    "#;
    assert_eq!(tree_walker_heap_check_report(src), "heap check: 0 double frees (0 distinct)\n");
    assert_eq!(interpreter_value(src), 5);
    assert_consistent(src, "consuming_method_receiver");
    memory_profiles_agree(src, "consuming_method_receiver");
}

/// The consuming call is decided per receiver type: another type's
/// `finish(&mut self)` does not keep `JsonWriter`-style `finish(self:
/// Self)` from consuming its receiver. With the name-only rule the
/// shared name made every `finish` a read, and poc/logsearch's
/// `mount::write_meta` still freed the writer's buffer twice.
#[test]
fn a_consuming_method_is_found_by_the_receivers_type() {
    let src = r#"
        struct Writer { out: String }
        impl Writer {
            fn finish(self: Self) -> String { self.out }
        }
        struct Counter { n: u64 }
        impl Counter {
            fn finish(&mut self) -> u64 { self.n = self.n + 1u64  self.n }
        }
        fn main() -> u64 {
            var c = Counter { n: 0u64 }
            val w = Writer { out: String::from_str("abc") }
            val s = w.finish()
            val k = c.finish()
            s.len() + k + c.finish()
        }
    "#;
    assert_eq!(tree_walker_heap_check_report(src), "heap check: 0 double frees (0 distinct)\n");
    assert_eq!(interpreter_value(src), 6);
    assert_consistent(src, "consuming_method_by_type");
    memory_profiles_agree(src, "consuming_method_by_type");
}

/// DOUBLE-DROP-LANE-DIVERGENCE: `val b = self.bytes` in a `&self`
/// method, and `val b = s.bytes` over a local, name part of a value
/// another binding owns -- a compound `val` never copies. The move
/// check knew only plain names as aliases (and did not declare an
/// implicit `&self` at all), so on the tree-walker `b` dropped the
/// vector and its owner dropped it again (`crypto_sha256.t`'s
/// `Sum::to_hex`, found by HEAP-CHECK).
#[test]
fn a_field_bound_by_val_is_an_alias_not_an_owner() {
    let src = r#"
        struct Sum { bytes: Vec<u8> }
        impl Sum {
            fn peek(&self) -> u64 {
                val b = self.bytes
                b.size()
            }
        }
        fn main() -> u64 {
            var v: Vec<u8> = Vec::new()
            v.push(1u8)
            v.push(2u8)
            val s = Sum { bytes: v }
            val n = s.peek() + s.peek()
            val b = s.bytes
            n * 10u64 + b.size()
        }
    "#;
    assert_eq!(tree_walker_heap_check_report(src), "heap check: 0 double frees (0 distinct)\n");
    assert_eq!(interpreter_value(src), 42);
    assert_consistent(src, "field_alias_not_owner");
    memory_profiles_agree(src, "field_alias_not_owner");
}

/// HEAP-CHECK H1: in poison mode a read of a freed block stops the run
/// and names where the block was allocated and freed, on the
/// tree-walker and the IR VM alike. Without the mode the read quietly
/// returns the old value -- the heap never reuses an address.
#[test]
fn poison_mode_stops_a_read_of_a_freed_block() {
    let src = "\
unsafe fn main() -> u64 {
    val p: ptr = __builtin_heap_alloc(16u64)
    __builtin_ptr_write(p, 0u64, 42u64)
    __builtin_heap_free(p)
    val x = __builtin_ptr_read::<u64>(p, 0u64)
    x
}
";
    let (tree, vm) = heap_poison_errors(src);
    let want = "heap check: read of 8 bytes at offset 0 of a 16-byte block that was already freed \
                (allocated at test.t:2:18, freed at test.t:4:5)";
    assert!(tree.contains(want), "tree-walker: {tree}");
    assert!(vm.contains(want), "IR VM: {vm}");
    assert_eq!(interpreter_value(src), 42);
}

/// HEAP-CHECK H1: a window held across a `push` that reallocates -- the
/// hazard `core/std/span.t` names as unchecked -- reads the vector's
/// old buffer, and poison mode says so: the block was moved by a resize.
#[test]
fn poison_mode_stops_a_window_held_across_a_resize() {
    let src = r#"
        fn main() -> u64 {
            var v: Vec<u64> = Vec::new()
            v.push(1u64)
            val w = v.as_span()
            val s: Span<u64> = w ?? panic("empty")
            var i: u64 = 0u64
            while i < 10u64 {
                v.push(i)
                i = i + 1u64
            }
            s.get(0u64)
        }
    "#;
    let (tree, vm) = heap_poison_errors(src);
    for (lane, err) in [("tree-walker", &tree), ("IR VM", &vm)] {
        assert!(err.contains("heap check: read of 8 bytes at offset 0"), "{lane}: {err}");
        assert!(err.contains("moved by a resize"), "{lane}: {err}");
    }
}
