//! The lanes every consistency test runs its source through, the
//! caches that keep re-running them cheap, and the few assertion
//! helpers that more than one section needs.
//!
//! Split out of `consistency.rs` when the sections became their own
//! modules. The contents are unchanged apart from the visibility each
//! item needs to be reachable from a sibling module.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{LazyLock, Mutex};

use compiler::{compile_file, compile_to_jit_main_with_options, CompilerOptions};
use interpreter::object::Object;
use interpreter::{RunOptions, RunOutcome};

// Caches for expensive in-process interpreter results. Each test
// binary is single-process but multi-threaded under libtest; a
// `Mutex` is sufficient because the critical section is tiny
// (HashMap lookup / insert) and the work itself is CPU-bound.
pub(super) static AST_LANES_CACHE: LazyLock<Mutex<HashMap<(String, bool), Option<(u64, Option<i64>)>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
pub(super) static JIT_CACHE: LazyLock<Mutex<HashMap<(String, bool), i32>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Memoizes `needs_core`, which is asked once per lane per source.
pub(super) static NEEDS_CORE_CACHE: LazyLock<Mutex<HashMap<String, bool>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Does `source` need the auto-loaded `core/` modules to type-check?
///
/// This is the *single* place the lite-vs-full decision is made. Each
/// lane used to discover it independently by attempting its own lite
/// run and falling back on failure, which meant a stdlib-using program
/// paid a thrown-away attempt per lane. Deciding once — with the
/// cheapest probe there is, a parse plus a no-core type-check, no
/// lowering and no codegen — and handing the answer to every lane
/// makes that duplicate structurally impossible.
///
/// The answer is memoized per source: `assert_consistent` and the
/// helpers it calls all ask, and sub-tests reuse sources.
pub(super) fn needs_core(source: &str) -> bool {
    if let Some(cached) = NEEDS_CORE_CACHE.lock().unwrap().get(source) {
        return *cached;
    }
    let mut parser = frontend::ParserWithInterner::new(source);
    let answer = match parser.parse_program() {
        Ok(mut program) => {
            let interner = parser.get_string_interner();
            interpreter::check_typing_with_core_modules(
                &mut program,
                interner,
                Some(source),
                Some("test.t"),
                None,
            )
            .is_err()
        }
        // A parse error is not a core question; let the lanes report it
        // against the canonical (with-core) configuration.
        Err(_) => true,
    };
    NEEDS_CORE_CACHE
        .lock()
        .unwrap()
        .insert(source.to_string(), answer);
    answer
}

/// Compile `source` to an in-process JIT program without auto-loading
/// core. Only the lite paths call this, and they have already
/// established that the source needs no stdlib, so there is nothing to
/// fall back to — the previous version probed without core and then
/// retried with it, a retry that could only fire after a caller had
/// already proven the no-core configuration works.
pub(super) fn compile_jit_lite(source: &str) -> Result<compiler::JitProgram, String> {
    let options = CompilerOptions::new(PathBuf::from("<jit>"));
    compile_to_jit_main_with_options(source, &options)
}

/// Repo-relative content-addressed link cache. Pinned outside `/tmp` so
/// the cache survives across `cargo nextest` invocations on a single
/// developer machine — repeat runs hit cached binaries and skip the
/// `cc` invocation that previously dominated post-spawn-removal wall
/// time. The directory is created lazily inside `link_executable`.
pub(super) fn link_cache_dir_for_tests() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/.toy-link-cache"))
}

pub(super) fn skip_e2e() -> bool {
    std::env::var("COMPILER_E2E").map(|v| v == "skip").unwrap_or(false)
}

/// Path to the repo-root `core/` directory containing the auto-loaded
/// stdlib modules. Computed at compile time relative to the compiler
/// crate's `CARGO_MANIFEST_DIR` so tests resolve the same modules
/// the interpreter side picks up via its exe-relative search.
pub(super) fn core_modules_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../core"))
}

pub(super) fn unique_path(stem: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    p.push(format!("toy_consistency_{stem}_{pid}_{nanos}"));
    p
}

/// Run `source` through the interpreter and return the value `main`
/// produced. Numeric programs are reduced to `u64` (with i64 sign-cast
/// folded into u64 via the same `as` semantics the interpreter exposes).
pub(super) fn interpreter_value(source: &str) -> u64 {
    // Most consistency sub-tests are pure user code (no stdlib
    // references), and skipping the type-check pass over `core/std/*.t`
    // shaves a measurable amount off every such test. Both outcomes of
    // the lite attempt are memoized, so the fallback never re-runs it.
    if let Some(v) = interpreter_value_with_core(source, None) {
        return v;
    }
    interpreter_value_with_core(source, Some(core_modules_dir()))
        .expect("interpreter type-check / execute (with core)")
}

pub(super) fn interpreter_value_with_core(source: &str, core_dir: Option<PathBuf>) -> Option<u64> {
    ast_lanes(source, core_dir).map(|(value, _ir_vm)| value)
}

/// The two lanes that run straight off a type-checked AST — the
/// tree-walker and the IR VM — sharing one frontend pass.
///
/// They used to parse and type-check `source` independently. In the
/// with-core configuration that meant integrating and type-checking
/// every `core/std/*.t` module twice for the same program, which is the
/// dominant per-test cost once a test leaves the lite path. Neither
/// lane mutates the AST (`execute_program` takes `&File`, and the pools
/// hold no interior mutability), so one checked program feeds both.
///
/// `None` means the program does not type-check or run in this
/// configuration; that is also the lite/full probe every caller uses.
/// The IR VM half is separately `None` when lowering fails or the
/// module leaves the VM's supported subset — the "lane not eligible"
/// signal, not a failure.
///
/// Both outcomes are memoized. Failures used to return early without
/// recording anything, so a source that cannot run in this
/// configuration was re-parsed and re-type-checked on every ask.
pub(super) fn ast_lanes(source: &str, core_dir: Option<PathBuf>) -> Option<(u64, Option<i64>)> {
    let key = (source.to_string(), core_dir.is_some());
    {
        let cache = AST_LANES_CACHE.lock().unwrap();
        if let Some(cached) = cache.get(&key) {
            return *cached;
        }
    }
    let lanes = (|| {
        let mut parser = frontend::ParserWithInterner::new(source);
        let mut program = parser.parse_program().ok()?;
        let interner = parser.get_string_interner();
        interpreter::check_typing_with_core_modules(
            &mut program,
            interner,
            Some(source),
            Some("test.t"),
            core_dir.as_deref(),
        )
        .ok()?;

        // Tree-walker lane. `execute_program` would hand the run to
        // whichever engine is eligible, and the IR VM is eligible for
        // most programs — which made this lane and the IR VM lane
        // below the same engine, and the 4-way agreement three copies
        // of one lowering plus the AOT. MATCH-STRUCT-ARM lived in that
        // blind spot: a struct-producing `if` / `match` in return
        // position returned a zero-filled value and every lane agreed.
        let result = interpreter::execute_program_tree_walking(
            &program,
            interner,
            Some(source),
            Some("test.t"),
        )
        .ok()?;
        let value = match &*result.borrow() {
            Object::UInt64(n) => *n,
            Object::Int64(n) => *n as u64,
            Object::Bool(b) => *b as u64,
            other => panic!("unexpected interpreter result: {other:?}"),
        };

        // IR VM lane, off the same checked AST.
        let ir_vm = (|| {
            let contract_msgs = compiler::ContractMessages::intern(interner);
            let ir_module =
                compiler::lower::lower_program(&program, interner, &contract_msgs, false).ok()?;
            if !interpreter::ir_vm::eligibility::ir_vm_supported(&ir_module) {
                return None;
            }
            // MEMORY_PROFILING M4: this calls the VM directly rather
            // than through `execute_entry`, which is where a run's
            // allocation counters are zeroed. Without this a program
            // that reads `__builtin_alloc_count()` would see the
            // tree-walker lane's allocations too, since both lanes run
            // in this process.
            interpreter::heap::reset_profile();
            interpreter::ir_vm::run_module_with_interner(&ir_module, Some(interner)).ok()
        })();

        Some((value, ir_vm))
    })();
    AST_LANES_CACHE.lock().unwrap().insert(key, lanes);
    lanes
}

/// A parsed + type-checked program, ready for every backend lane.
///
/// The with-core full paths of `assert_consistent` and
/// `assert_stdout_consistent` produce this **once** and hand it to the
/// tree-walker, IR VM, JIT and AOT lanes. The AOT lane used to go
/// through `compile_file` (which re-parses and re-type-checks) and the
/// JIT lane through `run_source` (same), so a with-core test paid the
/// integrate + type-check pass over `core/std/*.t` three times for the
/// same program — once for `ast_lanes`, once for AOT, once for JIT.
/// The AST is never mutated between lanes (in-place type-check
/// rewrites already happened), so one checked program feeds all four,
/// exactly as `ast_lanes` already fed tree-walker + IR VM.
pub(super) struct CheckedProgram<'a> {
    program: frontend::ast::File,
    interner: &'a string_interner::DefaultStringInterner,
    contract_msgs: compiler::ContractMessages,
}

/// Parse + type-check `source` once, borrowing the caller's parser so
/// the interner stays alive for every lane. `None` when the source does
/// not parse or type-check in this configuration.
pub(super) fn checked_program<'a>(
    source: &str,
    parser: &'a mut frontend::ParserWithInterner,
    core_dir: Option<&std::path::Path>,
) -> Option<CheckedProgram<'a>> {
    let mut program = parser.parse_program().ok()?;
    interpreter::check_typing_with_core_modules(
        &mut program,
        parser.get_string_interner(),
        Some(source),
        Some("test.t"),
        core_dir,
    )
    .ok()?;
    let contract_msgs = compiler::ContractMessages::intern(parser.get_string_interner());
    Some(CheckedProgram {
        program,
        interner: &*parser.get_string_interner(),
        contract_msgs,
    })
}

pub(super) fn checked_interpreter_value(checked: &CheckedProgram, source: &str) -> u64 {
    // Tree-walker, not "whichever engine is eligible" — see the note
    // in `ast_lanes`.
    let result = interpreter::execute_program_tree_walking(
        &checked.program,
        checked.interner,
        Some(source),
        Some("test.t"),
    )
    .expect("interpreter execute (checked program)");
    match &*result.borrow() {
        Object::UInt64(n) => *n,
        Object::Int64(n) => *n as u64,
        Object::Bool(b) => *b as u64,
        other => panic!("unexpected interpreter result: {other:?}"),
    }
}

/// IR VM lane off a shared checked program. `None` when lowering fails
/// or the module leaves the VM's supported subset — the "lane not
/// eligible" signal, not a failure.
pub(super) fn checked_ir_vm_value(checked: &CheckedProgram) -> Option<i64> {
    let ir_module = compiler::lower::lower_program(
        &checked.program,
        checked.interner,
        &checked.contract_msgs,
        false,
    )
    .ok()?;
    if !interpreter::ir_vm::eligibility::ir_vm_supported(&ir_module) {
        return None;
    }
    // MEMORY_PROFILING M4: same rationale as `ast_lanes` — without the
    // reset, `__builtin_alloc_count()` would see the tree-walker lane's
    // allocations too.
    interpreter::heap::reset_profile();
    interpreter::ir_vm::run_module_with_interner(&ir_module, Some(checked.interner)).ok()
}

/// JIT lane off a shared checked program. In-process, so it needs the
/// same `jit_available` guard as `jit_exit_code` — a build without the
/// `jit` feature would silently compare the tree-walker against itself.
pub(super) fn checked_jit_exit_code(checked: &CheckedProgram, source: &str) -> i32 {
    assert!(
        interpreter::jit_available(),
        "the interpreter was built without its `jit` feature, so this would compare the \
         tree-walker against itself"
    );
    let result = interpreter::jit::with_jit_override(true, || {
        interpreter::execute_program(
            &checked.program,
            checked.interner,
            Some(source),
            Some("test.t"),
        )
    })
    .expect("interpreter execute (checked program, jit)");
    let code = match &*result.borrow() {
        Object::Int64(v) => Some(*v as i32),
        Object::UInt64(v) => Some(*v as i32),
        _ => None,
    };
    code.map(|c| c & 0xff).unwrap_or(0)
}

/// AOT lane off a shared checked program: link a fresh executable from
/// the already-lowered code and run it. Returns `(exit_code, stdout)`;
/// stdout is always captured so exit-code callers pay nothing extra.
pub(super) fn checked_compiler_run(checked: &CheckedProgram, stem: &str) -> (i32, String) {
    let exe_path = unique_path(stem);
    let mut options = CompilerOptions::new(PathBuf::from("<checked>"));
    options.output = Some(exe_path.clone());
    options.core_modules_dir = Some(core_modules_dir());
    options.link_cache_dir = Some(link_cache_dir_for_tests());
    compiler::compile_checked_program(
        &checked.program,
        checked.interner,
        &checked.contract_msgs,
        &options,
    )
    .expect("compile_checked_program failed");
    let output = Command::new(&exe_path).output().expect("spawn binary");
    let _ = std::fs::remove_file(&exe_path);
    (
        output.status.code().expect("exit code"),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

pub(super) fn checked_interpreter_stdout(checked: &CheckedProgram, source: &str) -> String {
    let (result, captured) = interpreter::output::with_capture(|| {
        interpreter::execute_program_tree_walking(
            &checked.program,
            checked.interner,
            Some(source),
            Some("test.t"),
        )
    });
    result.expect("interpreter execute (checked program)");
    captured
}

pub(super) fn checked_jit_stdout(checked: &CheckedProgram, source: &str) -> String {
    assert!(
        interpreter::jit_available(),
        "the interpreter was built without its `jit` feature, so this would compare the \
         tree-walker against itself"
    );
    let (result, captured) = interpreter::output::with_capture(|| {
        interpreter::jit::with_jit_override(true, || {
            interpreter::execute_program(
                &checked.program,
                checked.interner,
                Some(source),
                Some("test.t"),
            )
        })
    });
    result.expect("interpreter execute (checked program, jit)");
    captured
}

/// Run `source` through the in-process interpreter with the JIT path
/// forced on. Returns the exit code `main` would have produced, already
/// `& 0xff`. When `with_core` is false the call skips auto-loading the
/// ~11 stdlib modules (~150 ms saved per call in debug builds). Callers
/// should pass `with_core=false` only after the in-process tree-walker
/// check has confirmed the program compiles without the core modules.
///
/// Replaces the previous spawn of `target/debug/interpreter` with
/// `INTERPRETER_JIT=1`. `interpreter::jit::with_jit_override` provides
/// the per-call JIT toggle (no env-var race under threaded test
/// execution).
pub(super) fn jit_exit_code(source: &str, _stem: &str, with_core: bool) -> i32 {
    // `RunOptions::jit` is a no-op when the interpreter was built
    // without its `jit` feature, which would turn every call here into
    // a plain tree-walker run and make this whole column agree with the
    // interpreter column by construction. The dev-dependency in
    // `compiler/Cargo.toml` asks for the feature; this is the check
    // that it is actually there.
    assert!(
        interpreter::jit_available(),
        "the interpreter was built without its `jit` feature, so this would compare the \
         tree-walker against itself"
    );
    let key = (source.to_string(), with_core);
    {
        let cache = JIT_CACHE.lock().unwrap();
        if let Some(cached) = cache.get(&key) {
            return *cached;
        }
    }
    let core_dir = if with_core { Some(core_modules_dir()) } else { None };
    let mut options = RunOptions::default();
    options.jit = true;
    options.core_modules_dir = core_dir.as_deref();
    let result = match interpreter::run_source(source, "test.t", &options) {
        Ok(RunOutcome { exit_code: Some(code) }) => code & 0xff,
        Ok(RunOutcome { exit_code: None }) => 0,
        Err(diag) => panic!("interpreter run_source (jit) failed: {diag}"),
    };
    let mut cache = JIT_CACHE.lock().unwrap();
    cache.insert(key, result);
    result
}

/// Compile `source` into a fresh executable, run it, and return the
/// observed exit code (already `& 0xff` from the OS). `with_core`
/// follows the same convention as `jit_exit_code`.
pub(super) fn compiler_exit_code(source: &str, stem: &str, with_core: bool) -> i32 {
    try_compiler_exit_code(source, stem, with_core)
        .expect("compile_file: compile failed")
}

/// Try-compile variant for the lazy-core fast path. Returns `None`
/// if `compile_file` fails (typically a type-check error from a
/// stdlib symbol the no-core path can't resolve), letting the
/// caller fall back to the full core-aware path without panicking.
pub(super) fn try_compiler_exit_code(source: &str, stem: &str, with_core: bool) -> Option<i32> {
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let exe_path = unique_path(stem);
    let mut options = CompilerOptions::new(src_path.clone());
    options.output = Some(exe_path.clone());
    options.core_modules_dir = if with_core { Some(core_modules_dir()) } else { None };
    options.link_cache_dir = Some(link_cache_dir_for_tests());
    let compile_ok = compile_file(&options).is_ok();
    let result = if compile_ok {
        let status = Command::new(&exe_path).status().expect("spawn binary");
        Some(status.code().expect("exit code"))
    } else {
        None
    };
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);
    result
}

/// The IR VM lane's view of the shared frontend pass. `None` when the
/// program does not type-check, when lowering fails, or when the
/// lowered IR leaves the Phase 1 scalar subset.
pub(super) fn ir_vm_exit_code(source: &str, with_core: bool) -> Option<i64> {
    let core_dir = with_core.then(core_modules_dir);
    ast_lanes(source, core_dir).and_then(|(_value, ir_vm)| ir_vm)
}

/// Assert that the interpreter result, the JIT-compiled binary's exit
/// code, and the AOT-compiled binary's exit code all agree, with `&
/// 0xff` shell truncation applied uniformly so test programs need not
/// stay below 256 to pass. Any divergence pinpoints which pair drifted.
///
/// `needs_core` decides once, up front, whether the source needs the
/// auto-loaded stdlib; every lane then runs exactly once in that
/// configuration. Pure user code (most sub-tests) skips the type-check
/// pass over `core/std/*.t` in all four lanes. The lite lanes are still
/// allowed to fail or disagree — if they do we redo the comparison
/// with core, so the diagnostic the user sees comes from the
/// configuration that matches the production binaries.
///
/// Phase 1+: when the IR VM lane is eligible, a 4-way agreement
/// (interpreter / compiler / JIT / IR VM) is required on the fast path.
pub(super) fn assert_consistent(source: &str, stem: &str) {
    if skip_e2e() {
        return;
    }
    // Fast path: in-process drivers, no stdlib auto-load. This also
    // skips the JIT spawn (~1-2 s of interpreter binary startup).
    // The no-core interpreter run *is* the lite/full probe: it is the
    // cheapest lane (no lowering, no codegen, no link) and the `&&`
    // chain short-circuits on it, so a stdlib-using source never
    // reaches the lanes that would only throw their work away. Its
    // result — success and failure alike — is memoized, so the
    // canonical path below re-asks for free.
    if let Some(interp) = interpreter_value_with_core(source, None)
        && let Some(compiled) = try_compiler_exit_code(source, stem, false)
        && let Ok(jit_prog) = compile_jit_lite(source)
    {
        let jit = jit_prog.run();
        let compiled = compiled as u64;
        let mut all_match = interp & 0xff == compiled & 0xff && interp & 0xff == jit & 0xff;
        if let Some(ir_vm) = ir_vm_exit_code(source, false) {
            all_match &= interp & 0xff == (ir_vm as u64) & 0xff;
        }
        if all_match {
            return;
        }
    }
    let core = core_modules_dir();
    let mut parser = frontend::ParserWithInterner::new(source);
    let checked = checked_program(source, &mut parser, Some(core.as_path()))
        .expect("interpreter type-check / execute (with core)");
    let interp = checked_interpreter_value(&checked, source);
    let (compiled, _) = checked_compiler_run(&checked, stem);
    let compiled = compiled as u64;
    let jit = checked_jit_exit_code(&checked, source) as u64;
    assert_eq!(
        interp & 0xff,
        compiled & 0xff,
        "interpreter={interp} compiler={compiled} for source:\n{source}",
    );
    assert_eq!(
        interp & 0xff,
        jit & 0xff,
        "interpreter={interp} jit={jit} for source:\n{source}",
    );
    if let Some(ir_vm) = checked_ir_vm_value(&checked) {
        assert_eq!(
            interp & 0xff,
            (ir_vm as u64) & 0xff,
            "interpreter={interp} ir_vm={ir_vm} for source:\n{source}",
        );
    }
}

/// Like [`assert_consistent`], for a program the **tree-walker cannot
/// run**: the compiled lanes and the IR VM must agree, and the
/// tree-walker is left out with its reason spelled at the call site.
///
/// Reach for this only when the tree-walker's gap is the thing being
/// documented. It is deliberately two-directional, like the skip lists
/// in `example_consistency.rs`: the assertion below fails once the
/// tree-walker learns the shape, so the opt-out cannot outlive the gap
/// it describes.
pub(super) fn assert_consistent_without_tree_walker(source: &str, stem: &str, why: &str) {
    if skip_e2e() {
        return;
    }
    let core = core_modules_dir();
    let mut parser = frontend::ParserWithInterner::new(source);
    let checked = checked_program(source, &mut parser, Some(core.as_path()))
        .expect("interpreter type-check (with core)");
    let tree_walker = interpreter::execute_program_tree_walking(
        &checked.program,
        checked.interner,
        Some(source),
        Some("test.t"),
    );
    assert!(
        tree_walker.is_err(),
        "the tree-walker now runs this program ({why}) — drop the opt-out          and use `assert_consistent` for source:\n{source}",
    );
    let (compiled, _) = checked_compiler_run(&checked, stem);
    let compiled = compiled as u64;
    let jit = checked_jit_exit_code(&checked, source) as u64;
    assert_eq!(
        compiled & 0xff,
        jit & 0xff,
        "compiler={compiled} jit={jit} for source:\n{source}",
    );
    if let Some(ir_vm) = checked_ir_vm_value(&checked) {
        assert_eq!(
            compiled & 0xff,
            (ir_vm as u64) & 0xff,
            "compiler={compiled} ir_vm={ir_vm} for source:\n{source}",
        );
    }
}

/// Run `source` through the in-process interpreter (tree-walker) and
/// return everything its `print` / `println` builtins emit.
/// `interpreter::output::with_capture` installs a thread-local sink
/// that the print sites write to, so this is safe under libtest's
/// threaded execution (each test thread gets its own buffer).
///
/// `with_core` — same convention as `jit_exit_code`.
pub(super) fn interpreter_stdout(source: &str, _stem: &str, with_core: bool) -> String {
    let core_dir = if with_core { Some(core_modules_dir()) } else { None };
    let mut options = RunOptions::default();
    options.core_modules_dir = core_dir.as_deref();
    let (result, captured) = interpreter::output::with_capture(|| {
        interpreter::run_source(source, "test.t", &options)
    });
    if let Err(diag) = result {
        panic!("interpreter run_source (no jit) failed: {diag}");
    }
    captured
}

/// Compile to a binary, run it, and capture stdout. Mirrors
/// `compiler_exit_code` but keeps the bytes instead of the exit code.
/// `with_core` follows the same convention; returns `None` when
/// `compile_file` fails (lets the lite-path caller fall back).
pub(super) fn try_compiler_stdout(source: &str, stem: &str, with_core: bool) -> Option<String> {
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let exe_path = unique_path(stem);
    let mut options = CompilerOptions::new(src_path.clone());
    options.output = Some(exe_path.clone());
    options.core_modules_dir = if with_core { Some(core_modules_dir()) } else { None };
    options.link_cache_dir = Some(link_cache_dir_for_tests());
    let result = if compile_file(&options).is_ok() {
        let out = Command::new(&exe_path).output().expect("spawn binary");
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    };
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);
    result
}

/// Same shape as `assert_consistent`, but compares stdout instead of
/// exit codes. Catches divergences in `print` / `println` formatting
/// across the three backends (the original motivation: the
/// interpreter-vs-compiler generic-instance type-args mismatch that
/// Phase "interpreter generic print" tracked down).
///
/// Lite path: same `needs_core` decision as `assert_consistent`. When
/// the source is pure user code all three backends skip the stdlib
/// auto-load; when it is not, none of them attempts a lite run that
/// would only be thrown away. The lite lanes may still fail or
/// disagree, in which case we fall back to the canonical with-core
/// path. The lite path's AOT compile uses `try_compiler_stdout`
/// (returns None on failure) so that fallback does not panic.
pub(super) fn assert_stdout_consistent(source: &str, stem: &str) {
    if skip_e2e() {
        return;
    }
    // Lite path: compiler in-process JIT (`compile_to_jit_main` +
    // `run_capturing_stdout`) replaces the JIT binary spawn,
    // alongside the existing in-process AOT compile. The
    // interpreter-side stdout still spawns because there's no
    // in-process stdout-capturing API for it yet (and the
    // interpreter spawn is comparatively cheap once stdlib
    // auto-load is skipped).
    if !needs_core(source)
        && let Some(compiled) = try_compiler_stdout(source, &format!("{stem}_aot_lite"), false)
        && let Ok(jit_prog) = compile_jit_lite(source) {
            let interp = interpreter_stdout(source, &format!("{stem}_interp_lite"), false);
            let (_exit, jit) = jit_prog.run_capturing_stdout();
            if interp == compiled && interp == jit {
                return;
            }
            // Mismatch on lite path falls through; the canonical path
            // produces the diagnostic the user sees.
        }
    let core = core_modules_dir();
    let mut parser = frontend::ParserWithInterner::new(source);
    let checked = checked_program(source, &mut parser, Some(core.as_path()))
        .expect("interpreter type-check / execute (with core)");
    let interp = checked_interpreter_stdout(&checked, source);
    let (_, compiled) = checked_compiler_run(&checked, &format!("{stem}_aot"));
    let jit = checked_jit_stdout(&checked, source);
    assert_eq!(
        interp, compiled,
        "interpreter vs compiler stdout mismatch for source:\n{source}\n--interp--\n{interp}\n--compiler--\n{compiled}",
    );
    assert_eq!(
        interp, jit,
        "interpreter vs jit stdout mismatch for source:\n{source}\n--interp--\n{interp}\n--jit--\n{jit}",
    );
}

/// Parse + type-check `source` with the core modules, returning the
/// rendered diagnostics on failure.
pub(super) fn type_check_errors(source: &str) -> Vec<String> {
    let mut session = compiler_core::CompilerSession::new();
    let mut program = session
        .parse_program(source)
        .expect("parse");
    interpreter::check_typing_with_core_modules(
        &mut program,
        session.string_interner_mut(),
        Some(source),
        Some("test.t"),
        Some(core_modules_dir().as_path()),
    )
    .expect_err("expected a type-check error")
}

pub(super) const ITER_COUNTER_PRELUDE: &str = r#"
    struct Counter { current: i64, end: i64 }
    impl Counter {
        fn new(end: i64) -> Self {
            Counter { current: 0i64, end: end }
        }
        fn next(&mut self) -> Option<i64> {
            if self.current >= self.end {
                Option::None
            } else {
                val v = self.current
                self.current = self.current + 1i64
                Option::Some(v)
            }
        }
    }
"#;

pub(super) fn memory_profiles_agree(source: &str, stem: &str) {
    if skip_e2e() {
        return;
    }
    let dir = unique_path(stem);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let src_path = dir.join("p.t");
    std::fs::write(&src_path, source).expect("write source");
    let output = Command::new(env!("CARGO_BIN_EXE_compiler"))
        .arg(&src_path)
        .arg("--all-backends")
        .arg("--profile=mem")
        .arg("--core-modules")
        .arg(core_modules_dir())
        .output()
        .expect("spawn compiler");
    let _ = std::fs::remove_dir_all(&dir);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "backends disagreed on allocation totals:\n{stderr}"
    );
    assert!(
        stderr.contains("alloc_count"),
        "no profile was produced:\n{stderr}"
    );
}

/// A program with a predictable allocation history: raw builtins only,
/// no stdlib, one 32-byte block deliberately left live.
pub(super) const JSON_PROFILE_PROGRAM: &str = "fn keep() -> u64 {\n\
    \x20   val p: ptr = __builtin_heap_alloc(32u64)\n\
    \x20   __builtin_ptr_write(p, 0u64, 7u64)\n\
    \x20   val v: u64 = __builtin_ptr_read(p, 0u64)\n\
    \x20   v\n\
    }\n\
    \n\
    fn main() -> u64 {\n\
    \x20   val a: ptr = __builtin_heap_alloc(64u64)\n\
    \x20   __builtin_heap_free(a)\n\
    \x20   keep()\n\
    }\n";

/// Exactly what both implementations must print for the program above.
/// Spelled out rather than derived, so a change to either one has to be
/// made on purpose.
pub(super) const JSON_PROFILE_EXPECTED: &str = r#"{
  "memory_profile": {
    "alloc_count": 2,
    "free_count": 1,
    "realloc_count": 0,
    "cumulative_bytes": 96,
    "live_bytes": 32,
    "peak_live_bytes": 64,
    "peak_at_request": 1
  },
  "leaks": [
    {
      "line": 2,
      "column": 18,
      "allocations": 1,
      "bytes": 32
    }
  ],
  "layouts": []
}
"#;

/// Run `source` on the tree-walker and render the JSON report, the way
/// `interpreter --profile=mem --profile-format=json` does.
pub(super) fn interpreter_json_profile(source: &str) -> String {
    interpreter::heap::reset_profile();
    let options = RunOptions::default();
    interpreter::run_source(source, "test.t", &options).expect("interpreter run");
    interpreter::heap::profile().report_json(
        &interpreter::heap::profile_sites(),
        &interpreter::heap::allocator_layouts(),
    )
}

/// 6 slots of 16 bytes, two live, and the middle one freed so the free
/// space splits into two runs (1 slot + 3 slots).
pub(super) const LAYOUT_PROFILE_PROGRAM: &str = "fn main() -> u64 {\n\
    \x20   var r = SlotRegion::new(16u64, 6u64)\n\
    \x20   val a = r.alloc(16u64)\n\
    \x20   val b = r.alloc(16u64)\n\
    \x20   val c = r.alloc(16u64)\n\
    \x20   r.free(b)\n\
    \x20   0u64\n\
    }\n";

/// Exactly what the layout section must be: managed 6*16, live 2*16,
/// two free runs, the largest 3 slots = 48 bytes, fragmentation
/// (64 - 48) * 1000 / 64 = 250 permille.
pub(super) const LAYOUT_PROFILE_EXPECTED: &str = "allocator layouts\n\
    \x20 SlotRegion  managed 96  live 32  free_blocks 2  largest_free 48  external_fragmentation 250 permille\n";

pub(super) fn interpreter_layout_report(source: &str) -> String {
    interpreter::heap::reset_profile();
    let core = core_modules_dir();
    let mut options = RunOptions::default();
    options.core_modules_dir = Some(core.as_path());
    interpreter::run_source(source, "test.t", &options).expect("interpreter run");
    interpreter::heap::allocator_layout_report_text(&interpreter::heap::allocator_layouts())
}

/// Compile `source` and run it, returning `(exit code, stderr)`.
///
/// `try_compiler_exit_code` throws the output away, which is enough
/// for "did it stop" but not for "did it say the same thing" —
/// ALLOC-CONTRACT-SUGAR needs the latter, because the sentence exists
/// twice: once in `compiler_ir` for the interpreting engines and once
/// in the dependency-free `toylang_rt` for compiled binaries.
pub(super) fn compiled_run_output(source: &str, stem: &str) -> Option<(i32, String)> {
    let src_path = unique_path(&format!("{stem}.t"));
    std::fs::write(&src_path, source).expect("write source");
    let exe_path = unique_path(stem);
    let mut options = CompilerOptions::new(src_path.clone());
    options.output = Some(exe_path.clone());
    options.core_modules_dir = Some(core_modules_dir());
    options.link_cache_dir = Some(link_cache_dir_for_tests());
    let result = if compile_file(&options).is_ok() {
        let out = Command::new(&exe_path).output().expect("spawn binary");
        Some((
            out.status.code().expect("exit code"),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    } else {
        None
    };
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&exe_path);
    result
}

/// The IR `lower_program` produces for `source`, rendered.
pub(super) fn lowered_ir(source: &str) -> String {
    lowered_ir_with(source, false)
}

/// As `lowered_ir`, with the `release` flag `lower_program` takes —
/// which decides whether contract clauses are emitted at all, and so
/// (CONTRACT-ELISION) whether they may stand in for a guard.
pub(super) fn lowered_ir_with(source: &str, release: bool) -> String {
    let mut parser = frontend::ParserWithInterner::new(source);
    let mut program = parser.parse_program().expect("parse");
    let interner = parser.get_string_interner();
    interpreter::check_typing_with_core_modules(
        &mut program,
        interner,
        Some(source),
        Some("test.t"),
        None,
    )
    .expect("type check");
    let contract_msgs = compiler_lower::ContractMessages::intern(interner);
    let module = compiler_lower::lower_program(&program, interner, &contract_msgs, release)
        .expect("lower");
    format!("{module}")
}

/// The rendered body of one lowered function, so a test can count
/// instructions inside it without matching the rest of the module.
pub(super) fn lowered_function(ir: &str, name: &str) -> String {
    let head = format!("local function toy_{name}(");
    let start = ir
        .find(&head)
        .unwrap_or_else(|| panic!("no function `{name}` in:\n{ir}"));
    let rest = &ir[start..];
    let end = rest.find("\n}").expect("function end") + 2;
    rest[..end].to_string()
}

/// Assert that `source` prints exactly `expected`, and that all three
/// backends agree on it.
pub(super) fn assert_renders(source: &str, stem: &str, expected: &str) {
    if skip_e2e() {
        return;
    }
    assert_stdout_consistent(source, stem);
    assert_eq!(
        interpreter_stdout(source, stem, true),
        expected,
        "rendered output changed"
    );
}

/// Run `source` on the interpreter's own JIT and assert two things: that
/// the JIT actually compiled it, and that its stdout matches the
/// tree-walker's.
///
/// The second half is what `assert_stdout_consistent` already does. The
/// first half is what it cannot do. That helper's lite path returns as
/// soon as the tree-walker, the AOT binary and the *compiler*-side JIT
/// agree, so the interpreter's JIT column is only reached on the full
/// path -- and even there, a program the JIT declines silently falls
/// back to the tree-walker, which makes the comparison
/// tree-walker-versus-tree-walker and passes for the wrong reason.
///
/// Asserting on the verbose "JIT compiled:" line closes that: if
/// eligibility starts rejecting the program, the test fails instead of
/// quietly stopping to test anything.
pub(super) fn assert_jit_compiled_and_matches(source: &str, stem: &str) {
    if skip_e2e() {
        return;
    }
    let core = core_modules_dir();
    let mut options = RunOptions::default();
    options.jit = true;
    options.core_modules_dir = Some(core.as_path());

    let (result, jit_stdout, stderr) =
        interpreter::output::with_stdout_stderr_capture(|| {
            interpreter::jit::with_jit_verbose_override(true, || {
                interpreter::run_source(source, "test.t", &options)
            })
        });
    result.unwrap_or_else(|e| panic!("interpreter JIT run for `{stem}`: {e:?}"));

    assert!(
        stderr.contains("JIT compiled:"),
        "`{stem}` never reached the JIT, so this test compared the \
         tree-walker with itself. stderr:\n{stderr}",
    );

    let interp = interpreter_stdout(source, stem, true);
    assert_eq!(
        interp, jit_stdout,
        "interpreter vs its own JIT stdout mismatch for `{stem}`:\n{source}",
    );
}
