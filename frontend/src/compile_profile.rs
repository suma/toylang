//! COMPILE-PROFILE: where a compile spends its time, and on how much.
//!
//! One recorder per process, off by default. The phases of an AOT
//! compile live in four crates (`frontend`, `interpreter`,
//! `compiler_lower`, `compiler`), and this is the lowest one they all
//! depend on, so the recorder lives here and holds data only — the
//! text / JSON rendering is the `compiler` binary's business.
//!
//! Disabled, every entry point costs one relaxed atomic load. Enabled,
//! each call takes a mutex: the codegen workers report from rayon
//! threads, so a thread-local would lose their records.
//!
//! Phases nest (a guard closes its phase on drop) and are only opened
//! on the thread that called [`enable`]; a phase opened elsewhere would
//! interleave with the owner's stack. Counters, source files and
//! hotspots may be recorded from any thread.
//!
//! See `design-docs/COMPILE_PROFILE.md` for what is recorded and why.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread::ThreadId;
use std::time::{Duration, Instant};

static ENABLED: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<Option<State>> = Mutex::new(None);

/// One timed step of the compile. `start` is measured from [`enable`].
#[derive(Debug, Clone)]
pub struct Phase {
    pub name: &'static str,
    pub start: Duration,
    pub wall: Duration,
    pub children: Vec<Phase>,
}

/// Where a source file came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The file named on the command line.
    Entry,
    /// The prelude compiled into the binary.
    Prelude,
    /// A module under the first module root — the stdlib, unless the
    /// caller replaced the roots.
    Stdlib,
    /// A module under a later root (a package's `src/`).
    Package,
}

impl Origin {
    /// The origin of a module integrated from the root at `rank`.
    pub fn of_root(rank: u32) -> Self {
        if rank == 0 { Origin::Stdlib } else { Origin::Package }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Entry => "entry",
            Origin::Prelude => "prelude",
            Origin::Stdlib => "stdlib",
            Origin::Package => "package",
        }
    }
}

/// Whether a parse was served from the on-disk AST cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheUse {
    Hit,
    Miss,
    /// The cache was disabled, or does not apply (the entry file).
    Off,
}

impl CacheUse {
    pub fn as_str(self) -> &'static str {
        match self {
            CacheUse::Hit => "hit",
            CacheUse::Miss => "miss",
            CacheUse::Off => "off",
        }
    }
}

/// One source file the compile read.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: String,
    pub origin: Origin,
    pub bytes: u64,
    pub lines: u64,
    pub ast_cache: CacheUse,
    /// Parsing it, or loading its AST from the cache. For the stdlib
    /// this ran on a worker thread, so the files' times overlap.
    pub parse: Duration,
    /// Copying its AST into the program (symbol remap, cache save).
    pub integrate: Duration,
}

/// One function (or impl block) that took long in a phase.
#[derive(Debug, Clone)]
pub struct Hot {
    pub name: String,
    pub wall: Duration,
    /// Lower / codegen: the size of its IR.
    pub ir_insts: Option<u64>,
    /// Codegen only: the machine code it became.
    pub code_bytes: Option<u64>,
}

/// Which hotspot table a record belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotTable {
    Typecheck,
    Lower,
    Codegen,
}

/// Everything recorded between [`enable`] and [`finish`].
#[derive(Debug, Clone)]
pub struct Profile {
    pub wall: Duration,
    pub phases: Vec<Phase>,
    pub counters: BTreeMap<&'static str, u64>,
    pub files: Vec<SourceFile>,
    pub hot_typecheck: Vec<Hot>,
    pub hot_lower: Vec<Hot>,
    pub hot_codegen: Vec<Hot>,
}

struct State {
    epoch: Instant,
    owner: ThreadId,
    /// Phases opened and not yet closed, innermost last.
    open: Vec<Phase>,
    closed: Vec<Phase>,
    counters: BTreeMap<&'static str, u64>,
    files: Vec<SourceFile>,
    hot_typecheck: Vec<Hot>,
    hot_lower: Vec<Hot>,
    hot_codegen: Vec<Hot>,
    /// Open [`quiet`] guards. While non-zero, counters and hotspots are
    /// dropped; phases are still recorded.
    quiet: u32,
}

/// Start recording. Time is measured from here.
pub fn enable() {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    *state = Some(State {
        epoch: Instant::now(),
        owner: std::thread::current().id(),
        open: Vec::new(),
        closed: Vec::new(),
        counters: BTreeMap::new(),
        files: Vec::new(),
        hot_typecheck: Vec::new(),
        hot_lower: Vec::new(),
        hot_codegen: Vec::new(),
        quiet: 0,
    });
    ENABLED.store(true, Ordering::Release);
}

#[inline]
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Stop recording and hand back what was recorded. Phases still open
/// are closed at this instant.
pub fn finish() -> Option<Profile> {
    ENABLED.store(false, Ordering::Release);
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let mut state = guard.take()?;
    let now = state.epoch.elapsed();
    while let Some(mut phase) = state.open.pop() {
        phase.wall = now.saturating_sub(phase.start);
        attach(&mut state, phase);
    }
    Some(Profile {
        wall: now,
        phases: state.closed,
        counters: state.counters,
        files: state.files,
        hot_typecheck: state.hot_typecheck,
        hot_lower: state.hot_lower,
        hot_codegen: state.hot_codegen,
    })
}

fn with_state(f: impl FnOnce(&mut State)) {
    if !is_enabled() {
        return;
    }
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = guard.as_mut() {
        f(state);
    }
}

fn attach(state: &mut State, phase: Phase) {
    match state.open.last_mut() {
        Some(parent) => parent.children.push(phase),
        None => state.closed.push(phase),
    }
}

/// Closes its phase when dropped.
#[must_use = "the phase closes when this guard is dropped"]
pub struct PhaseGuard {
    active: bool,
}

impl Drop for PhaseGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        with_state(|state| {
            if let Some(mut phase) = state.open.pop() {
                phase.wall = state.epoch.elapsed().saturating_sub(phase.start);
                attach(state, phase);
            }
        });
    }
}

/// Open a phase nested in the innermost open one.
pub fn phase(name: &'static str) -> PhaseGuard {
    let mut active = false;
    with_state(|state| {
        if std::thread::current().id() != state.owner {
            return;
        }
        let start = state.epoch.elapsed();
        state.open.push(Phase { name, start, wall: Duration::ZERO, children: Vec::new() });
        active = true;
    });
    PhaseGuard { active }
}

/// Ends a [`quiet`] region when dropped.
#[must_use = "the quiet region ends when this guard is dropped"]
pub struct QuietGuard {
    active: bool,
}

impl Drop for QuietGuard {
    fn drop(&mut self) {
        if self.active {
            with_state(|state| state.quiet = state.quiet.saturating_sub(1));
        }
    }
}

/// Run a pass a second time without it counting as the first.
///
/// The const fold lowers the whole program to run it on the IR VM;
/// without this its bodies land in the `lower` hotspot table beside
/// the real lowering, and a function lowered twice is listed twice.
/// The time still shows, under whatever phase encloses the region.
pub fn quiet() -> QuietGuard {
    let mut active = false;
    with_state(|state| {
        state.quiet += 1;
        active = true;
    });
    QuietGuard { active }
}

/// Add `n` to a counter.
pub fn count(key: &'static str, n: u64) {
    with_state(|state| {
        if state.quiet == 0 {
            *state.counters.entry(key).or_insert(0) += n;
        }
    });
}

/// A start time for [`hot`] / [`file_parsed`], or `None` when not
/// recording — so a hot loop does not read the clock for nothing.
#[inline]
pub fn timer() -> Option<Instant> {
    is_enabled().then(Instant::now)
}

/// Record the time since `started` against a function.
pub fn hot(table: HotTable, started: Option<Instant>, name: impl FnOnce() -> String) {
    let Some(started) = started else { return };
    let wall = started.elapsed();
    hot_record(table, Hot { name: name(), wall, ir_insts: None, code_bytes: None });
}

/// Record a fully built hotspot entry.
pub fn hot_record(table: HotTable, record: Hot) {
    with_state(|state| {
        if state.quiet > 0 {
            return;
        }
        match table {
            HotTable::Typecheck => state.hot_typecheck.push(record),
            HotTable::Lower => state.hot_lower.push(record),
            HotTable::Codegen => state.hot_codegen.push(record),
        }
    });
}

/// Record that `path` was read and parsed (or loaded from the cache).
pub fn file_parsed(
    path: &str,
    origin: Origin,
    source: &str,
    ast_cache: CacheUse,
    started: Option<Instant>,
) {
    let Some(started) = started else { return };
    let parse = started.elapsed();
    with_state(|state| {
        let record = file_entry(state, path, origin);
        record.bytes = source.len() as u64;
        record.lines = source.lines().count() as u64;
        record.ast_cache = ast_cache;
        record.parse = parse;
    });
}

/// Record the time since `started` as `path`'s integration.
pub fn file_integrated(path: &str, origin: Origin, started: Option<Instant>) {
    let Some(started) = started else { return };
    let integrate = started.elapsed();
    with_state(|state| file_entry(state, path, origin).integrate += integrate);
}

fn file_entry<'a>(state: &'a mut State, path: &str, origin: Origin) -> &'a mut SourceFile {
    let index = match state.files.iter().position(|f| f.path == path) {
        Some(i) => i,
        None => {
            state.files.push(SourceFile {
                path: path.to_string(),
                origin,
                bytes: 0,
                lines: 0,
                ast_cache: CacheUse::Off,
                parse: Duration::ZERO,
                integrate: Duration::ZERO,
            });
            state.files.len() - 1
        }
    };
    &mut state.files[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test: the recorder is process-global, and the test harness
    // runs tests on several threads.
    #[test]
    fn phases_nest_and_records_land() {
        let _ = phase("ignored while disabled");
        enable();
        {
            let _outer = phase("outer");
            {
                let _inner = phase("inner");
                count("things", 2);
                count("things", 3);
            }
            file_parsed("a.t", Origin::Entry, "x\ny\n", CacheUse::Off, timer());
            file_integrated("a.t", Origin::Entry, timer());
            hot(HotTable::Codegen, timer(), || "main".to_string());
            {
                // A second run of a pass keeps its phase, not its records.
                let _quiet = quiet();
                let _again = phase("again");
                count("things", 100);
                hot(HotTable::Codegen, timer(), || "main again".to_string());
            }
            // A phase from another thread is dropped, not mis-nested.
            std::thread::spawn(|| drop(phase("elsewhere"))).join().unwrap();
        }
        let profile = finish().expect("recorded");
        assert!(!is_enabled());
        assert_eq!(profile.phases.len(), 1);
        assert_eq!(profile.phases[0].name, "outer");
        let children: Vec<_> = profile.phases[0].children.iter().map(|p| p.name).collect();
        assert_eq!(children, ["inner", "again"]);
        assert_eq!(profile.counters["things"], 5);
        assert_eq!(profile.files.len(), 1);
        assert_eq!((profile.files[0].bytes, profile.files[0].lines), (4, 2));
        assert_eq!(profile.hot_codegen.len(), 1);
        assert_eq!(profile.hot_codegen[0].name, "main");
    }
}
