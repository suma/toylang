//! Cross-backend agreement over every program in `interpreter/example/`
//! (COMPILER_DEV_LOOP D7).
//!
//! `consistency.rs` proves the backends agree on the programs someone
//! remembered to write a test for. That is the weak point: the `f`
//! lexical-scoping bug was fixed in the type checker and left broken in
//! the tree-walker, and the only reason it surfaced was that a test
//! happened to *run* the program rather than just type check it.
//! Nothing pointed at the other backends.
//!
//! This sweep removes the remembering. Every example that runs cleanly
//! is executed on the interpreter, the JIT and the AOT compiler, and
//! their exit codes and stdout must match. Coverage grows on its own as
//! examples are added.
//!
//! ## The two lists below are a coverage ledger, not a mute button
//!
//! An example is skipped only if it is named in `ERROR_EXAMPLES` (it is
//! *meant* to fail), `AOT_UNSUPPORTED` (the AOT backend cannot build it
//! yet), or `NEEDS_A_PEER` (it waits for something this harness does
//! not provide — see that list). The first two are checked, in both
//! directions:
//!
//!   * a listed example that starts working fails the test, so the list
//!     shrinks as the backends catch up
//!   * an unlisted example that stops working fails the test
//!
//! Without that a skip list quietly becomes a place where regressions
//! go to hide.
//!
//! Set `COMPILER_E2E=skip` to opt out, same as the other e2e suites.

use std::path::{Path, PathBuf};
use std::process::Command;

use compiler::CompilerOptions;

/// Examples that are supposed to fail — they demonstrate a diagnostic.
/// Running them proves nothing about backend agreement.
const ERROR_EXAMPLES: &[&str] = &[
    "array_test.t",
    "bad.t",
    "binary_error_test.t",
    "bool_array_error_test.t",
    "bool_number_mixed_error_test.t",
    "cross_reference_test.t",
    "debug_bad.t",
    "definite_error.t",
    "detailed_error_test.t",
    "error_demo.t",
    "error_test.t",
    "explicit_type_error.t",
    "forward_reference_test.t",
    "generic_option.t",
    "jit_panic_expr_fail.t",
    "jit_panic.t",
    // RECURSIVE-TYPES: mutually recursive struct declarations, now
    // rejected with E0013. They never ran — constructing one aborted
    // the process — so they demonstrate the diagnostic instead.
    "mutual_struct_test.t",
        "null_assignment_test.t",
    "struct_array_error_test.t",
        "struct_cross_ref_test.t",
    "struct_field_error_test.t",
    "type_error_test_new.t",
    "type_error_test.t",
    "undefined_var_demo.t",
    "undefined_var_error_test.t",
    "undefined_var_test.t",
];

/// Examples the AOT backend cannot compile yet. The interpreter and JIT
/// still run them; only the compiler comparison is skipped. Shrinking
/// this list is the point — each entry is a backend gap with a program
/// already written to exercise it.
const AOT_UNSUPPORTED: &[&str] = &[
    // (`allocator_bounded.t` left in MEMORY-ACCESS M2: its bare
    // `__builtin_ptr_read(p, 0u64)` in tail position had no
    // annotation for the AOT lane to take a width from, and
    // `__builtin_ptr_read::<u64>` does not need one. `allocator_list.t`
    // left when the untyped read was removed, for the same reason.
    // `array_type_only.t` left in CONST-ARRAY: a by-value array
    // parameter and an array literal argument both lower now.)
    "const_decls.t",
    "extern_generic_identity.t",
    "float64.t",
    // (`jit_heap.t` left in MEMORY-ACCESS M0: `__builtin_mem_set`
    // was the last builtin in it the AOT lane could not lower.)
    "jit_nested_tuple_fallback.t",
    "panic.t",
    "print_demo.t",
    "tuple_destructure_nested.t",
];

/// Examples that make a backend panic rather than fail cleanly. Each
/// entry is a crash worth fixing; the sweep is what found them.
const KNOWN_CRASHES: &[&str] = &[];

/// Examples that wait for something this harness does not provide.
///
/// Unlike the two lists above, this one is not a ledger of work to be
/// done — nothing about a backend would make these runnable here. A
/// server example blocks until a peer connects or its idle budget runs
/// out; running it would add that budget to every lane and still prove
/// only that nobody called.
///
/// The API is not left uncovered by the skip:
/// `compiler/tests/consistency/net.rs` exercises the same `bind` /
/// `accept` / `Poller` surface across four lanes, with the client in
/// the same process so it is deterministic. What lives here is the
/// documentation half — a server shaped the way one is actually
/// written, which needs a person and a terminal.
const NEEDS_A_PEER: &[&str] = &["net_echo_server.t"];

fn skip_e2e() -> bool {
    std::env::var("COMPILER_E2E").map(|v| v == "skip").unwrap_or(false)
}

fn examples_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../interpreter/example"))
}

fn core_modules_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../core"))
}

fn link_cache_dir_for_tests() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/.toy-link-cache"))
}

fn unique_path(stem: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    p.push(format!("toy_example_{stem}_{}_{nanos}", std::process::id()));
    p
}

/// Every example, sorted so the shards below are stable.
fn all_examples() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(examples_dir())
        .expect("read examples dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "t"))
        .collect();
    paths.sort();
    paths
}

fn file_name(path: &Path) -> String {
    path.file_name().unwrap().to_string_lossy().into_owned()
}

struct Run {
    /// `None` when `main` returned something that is not a number, in
    /// which case the interpreter has no exit code to report and a
    /// comparison against the native binary's would be meaningless —
    /// several examples return `bool` or `str`, and the compiled binary
    /// exits with whatever those happen to truncate to.
    exit_code: Option<i32>,
    stdout: String,
}

/// Compare two runs, ignoring the exit code when the reference side has
/// none to give.
fn disagrees(reference: &Run, other: &Run) -> bool {
    if reference.stdout != other.stdout {
        return true;
    }
    match (reference.exit_code, other.exit_code) {
        (Some(a), Some(b)) => a & 0xff != b & 0xff,
        _ => false,
    }
}

/// A parsed + type-checked program, shared by every backend lane.
///
/// The tree-walker, the interpreter's JIT and the AOT compiler all run
/// off this one checked program. The AOT lane used to go through
/// `compile_file`, which re-parses and re-type-checks — for an example
/// that meant the core-module integrate + type-check pass ran a third
/// time on identical input. `compile_checked_program` lowers the same
/// checked AST the two in-process engines already executed.
struct CheckedProgram<'a> {
    program: frontend::ast::File,
    interner: &'a string_interner::DefaultStringInterner,
    contract_msgs: compiler::ContractMessages,
}

/// Parse + type-check the example once, borrowing the caller's parser
/// so the interner survives for every lane. `Err` carries the
/// diagnostic so the caller can tell "this program is meant to fail"
/// from "this program regressed".
///
/// The frontend is not what this sweep compares, so it runs once and
/// every engine executes the result. What that gives up is the
/// guarantee of going through the same entry point the binary uses;
/// the steps below are that entry point's, minus the diagnostic
/// rendering.
fn check_checked<'a>(
    source: &str,
    name: &str,
    parser: &'a mut frontend::ParserWithInterner,
) -> Result<CheckedProgram<'a>, String> {
    let core = core_modules_dir();
    parser.set_source_file(name);
    let mut program = parser
        .parse_program()
        .map_err(|e| format!("Parse error: {e:?}"))?;
    interpreter::check_typing_with_core_modules(
        &mut program,
        parser.get_string_interner(),
        Some(source),
        Some(name),
        std::slice::from_ref(&core),
    )
    .map_err(|errors| format!("Type check errors: {errors:?}"))?;
    let contract_msgs = compiler::ContractMessages::intern(parser.get_string_interner());
    Ok(CheckedProgram {
        program,
        interner: &*parser.get_string_interner(),
        contract_msgs,
    })
}

/// Execute an already-checked program with the JIT forced on or off.
fn execute_once(
    checked: &CheckedProgram,
    source: &str,
    name: &str,
    jit: bool,
) -> Result<Run, String> {
    let (result, stdout) = interpreter::output::with_capture(|| {
        interpreter::jit::with_jit_override(jit, || {
            interpreter::execute_program(&checked.program, checked.interner, Some(source), Some(name))
        })
    });
    let value = result?;
    let exit_code = match &*value.borrow() {
        interpreter::object::Object::Int64(v) => Some(*v as i32),
        interpreter::object::Object::UInt64(v) => Some(*v as i32),
        _ => None,
    };
    Ok(Run { exit_code, stdout })
}

/// Compile and run. `None` when the AOT backend cannot build it.
fn run_compiled(checked: &CheckedProgram, stem: &str) -> Option<Run> {
    let exe_path = unique_path(stem);
    let mut options = CompilerOptions::new(PathBuf::from("<checked>"));
    options.output = Some(exe_path.clone());
    options.core_modules_dirs = vec![core_modules_dir()];
    options.link_cache_dir = Some(link_cache_dir_for_tests());
    if compiler::compile_checked_program(
        &checked.program,
        checked.interner,
        &checked.contract_msgs,
        &options,
    )
    .is_err()
    {
        return None;
    }
    let output = Command::new(&exe_path).output().expect("spawn compiled binary");
    let _ = std::fs::remove_file(&exe_path);
    Some(Run {
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
    })
}

/// Check one example, isolating panics.
///
/// The backends can panic outright on some programs (a cranelift block
/// left unsealed, a symbol missing from the interner). Letting that
/// escape would abort the whole shard and hide every example after it —
/// the sweep would report one problem per run instead of all of them.
fn check_example_isolated(path: &Path) -> Result<(), String> {
    let name = file_name(path);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check_example(path)));
    match outcome {
        Ok(result) => result,
        Err(_) if KNOWN_CRASHES.contains(&name.as_str()) => Ok(()),
        Err(_) => Err(format!(
            "`{name}` panicked inside a backend (see the panic message above) — fix it, \
             or add it to KNOWN_CRASHES with the reason"
        )),
    }
}

/// Check one example. Returns a description of any disagreement.
fn check_example(path: &Path) -> Result<(), String> {
    let name = file_name(path);
    if NEEDS_A_PEER.contains(&name.as_str()) {
        return Ok(());
    }
    let source = std::fs::read_to_string(path).expect("read example");
    let expects_error = ERROR_EXAMPLES.contains(&name.as_str());

    let mut parser = frontend::ParserWithInterner::new(&source);
    let checked = match check_checked(&source, &name, &mut parser) {
        Ok(checked) => checked,
        Err(diagnostic) => {
            return if expects_error {
                Ok(())
            } else {
                Err(format!("`{name}` failed on the interpreter: {diagnostic}"))
            };
        }
    };

    let interp = match execute_once(&checked, &source, &name, false) {
        Ok(run) => run,
        Err(diagnostic) => {
            return if expects_error {
                Ok(())
            } else {
                Err(format!("`{name}` failed on the interpreter: {diagnostic}"))
            };
        }
    };
    let jit = match execute_once(&checked, &source, &name, true) {
        Ok(run) => run,
        Err(diagnostic) => {
            return if expects_error {
                Ok(())
            } else {
                Err(format!("`{name}` failed on the JIT: {diagnostic}"))
            };
        }
    };
    if expects_error {
        return Err(format!(
            "`{name}` is listed in ERROR_EXAMPLES but now runs cleanly — \
             remove it from the list so it is checked across backends"
        ));
    }

    if disagrees(&interp, &jit) {
        return Err(format!(
            "`{name}`: JIT disagrees with the interpreter\n  \
             interpreter: exit={:?} stdout={:?}\n  jit:         exit={:?} stdout={:?}",
            interp.exit_code, interp.stdout, jit.exit_code, jit.stdout
        ));
    }

    let aot_expected_unsupported = AOT_UNSUPPORTED.contains(&name.as_str());
    let stem = name.trim_end_matches(".t");
    match run_compiled(&checked, stem) {
        None => {
            if aot_expected_unsupported {
                Ok(())
            } else {
                Err(format!(
                    "`{name}` no longer compiles with the AOT backend — fix it, or add it \
                     to AOT_UNSUPPORTED with the reason"
                ))
            }
        }
        Some(compiled) => {
            if aot_expected_unsupported {
                return Err(format!(
                    "`{name}` is listed in AOT_UNSUPPORTED but now compiles — \
                     remove it from the list so it is checked"
                ));
            }
            if disagrees(&interp, &compiled) {
                return Err(format!(
                    "`{name}`: AOT disagrees with the interpreter\n  \
                     interpreter: exit={:?} stdout={:?}\n  compiled:    exit={:?} stdout={:?}",
                    interp.exit_code, interp.stdout, compiled.exit_code, compiled.stdout
                ));
            }
            Ok(())
        }
    }
}

/// How many parallel pieces the sweep is cut into.
///
/// Sizing is a scheduling decision, not a coverage one — `check_shard`
/// partitions by `index % SHARDS`, so every example runs for any value.
/// The sweep is the most expensive thing in the suite, so its shard
/// width sets the run's critical path: at 4 each shard took ~3.8s while
/// the next-slowest test in the whole workspace was under 1s. 12 brings
/// a shard down to about that, which is as far as widening helps.
const SHARDS: usize = 12;

/// Shard the sweep so nextest runs the parts in parallel; one serial
/// pass over ~140 programs takes about `SHARDS` times as long.
fn check_shard(shard: usize, shards: usize) {
    if skip_e2e() {
        return;
    }
    // Without the feature, `RunOptions::jit = true` is silently ignored
    // and the JIT column below is a second tree-walker run — a third of
    // the sweep comparing a backend against itself and always agreeing.
    // See the `interpreter` dev-dependency in `compiler/Cargo.toml`.
    assert!(
        interpreter::jit_available(),
        "the interpreter was built without its `jit` feature, so the JIT column of this \
         sweep would silently be a second tree-walker run"
    );
    let failures: Vec<String> = all_examples()
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % shards == shard)
        .filter_map(|(_, path)| check_example_isolated(&path).err())
        .collect();
    assert!(
        failures.is_empty(),
        "cross-backend disagreement in {} example(s):\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// One `#[test]` per shard — nextest schedules tests, not threads, so
/// the sweep only runs in parallel if it is spelled out as separate
/// test functions. Written with a macro so the count stays in one
/// place (`SHARDS`) instead of drifting between the list and the
/// divisor.
macro_rules! example_shards {
    ($($name:ident => $index:expr),+ $(,)?) => {
        $(
            #[test]
            fn $name() {
                check_shard($index, SHARDS);
            }
        )+

        /// A missing shard function is invisible otherwise: the sweep
        /// would still pass, having quietly stopped checking every
        /// `SHARDS`-th example. Assert the declared indices are exactly
        /// `0..SHARDS`.
        #[test]
        fn the_shards_cover_every_example() {
            let mut declared = [$($index),+];
            declared.sort_unstable();
            let expected: Vec<usize> = (0..SHARDS).collect();
            assert_eq!(
                declared.as_slice(),
                expected.as_slice(),
                "shard functions must declare each index in 0..SHARDS exactly once",
            );
        }
    };
}

example_shards! {
    examples_agree_across_backends_shard_0 => 0,
    examples_agree_across_backends_shard_1 => 1,
    examples_agree_across_backends_shard_2 => 2,
    examples_agree_across_backends_shard_3 => 3,
    examples_agree_across_backends_shard_4 => 4,
    examples_agree_across_backends_shard_5 => 5,
    examples_agree_across_backends_shard_6 => 6,
    examples_agree_across_backends_shard_7 => 7,
    examples_agree_across_backends_shard_8 => 8,
    examples_agree_across_backends_shard_9 => 9,
    examples_agree_across_backends_shard_10 => 10,
    examples_agree_across_backends_shard_11 => 11,
}

/// The lists name real files. A rename that leaves a stale entry would
/// otherwise silently exempt nothing at all, which looks like coverage.
#[test]
fn skip_lists_name_existing_examples() {
    let present: Vec<String> = all_examples().iter().map(|p| file_name(p)).collect();
    let mut missing: Vec<&str> = Vec::new();
    for name in ERROR_EXAMPLES.iter().chain(AOT_UNSUPPORTED.iter()) {
        if !present.iter().any(|p| p == name) {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "skip lists name examples that no longer exist: {missing:?}"
    );
}
