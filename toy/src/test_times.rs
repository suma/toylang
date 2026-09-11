//! What the last run cost, so this one can schedule (TEST_PARALLEL.md
//! D2 / P4).
//!
//! Jobs are wildly uneven — one `poc/logsearch` test runs for seconds
//! while its neighbours take a millisecond — and with a shared cursor
//! the order they are handed out in decides the wall time: start the
//! long one last and everyone waits for it alone. Nothing in the
//! source says which tests are slow, so the runner measures and
//! remembers.
//!
//! Three kinds of record, because they answer two different questions:
//!
//! - `test` — one block's own time. Orders the IR VM lane's jobs, and
//!   says whether splitting a file into per-test jobs can pay.
//! - `driver` — what one compiled driver cost (compile, link, run).
//!   Orders the compiled lane, where a job is a binary rather than a
//!   test.
//!
//! Records carry the lane they were measured on: the same block costs
//! milliseconds on a compiled driver and seconds on the IR VM, so
//! mixing them would make every estimate a guess about which lane the
//! number came from.
//!
//! The file is a cache, not an input. A missing, stale, or unreadable
//! one costs a worse schedule and nothing else, which is why every
//! failure here is silent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Which lane a measurement came from.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lane {
    Aot,
    Vm,
}

impl Lane {
    fn tag(self) -> &'static str {
        match self {
            Lane::Aot => "aot",
            Lane::Vm => "vm",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "aot" => Some(Lane::Aot),
            "vm" => Some(Lane::Vm),
            _ => None,
        }
    }
}

/// A test, identified the way the report identifies it: by where it is
/// written. A name alone is not enough (two modules may both have a
/// "roundtrip"), and a position alone does not survive an edit above
/// it any better than the name does.
type TestKey = (String, u32, String);

#[derive(Default)]
pub struct History {
    tests: HashMap<(Lane, String, u32, String), Duration>,
    drivers: HashMap<String, Duration>,
}

impl History {
    pub fn test(&self, lane: Lane, key: &TestKey) -> Option<Duration> {
        self.tests
            .get(&(lane, key.0.clone(), key.1, key.2.clone()))
            .copied()
    }

    pub fn driver(&self, file: &str) -> Option<Duration> {
        self.drivers.get(file).copied()
    }

    pub fn record_test(&mut self, lane: Lane, key: TestKey, took: Duration) {
        self.tests.insert((lane, key.0, key.1, key.2), took);
    }

    pub fn record_driver(&mut self, file: String, took: Duration) {
        self.drivers.insert(file, took);
    }

    pub fn is_empty(&self) -> bool {
        self.tests.is_empty() && self.drivers.is_empty()
    }

    /// Fold `other`'s measurements in, letting it win.
    ///
    /// A filtered run measures a handful of tests; without the merge
    /// it would forget every test it did not run, and the next full
    /// run would schedule blind.
    pub fn absorb(&mut self, other: History) {
        self.tests.extend(other.tests);
        self.drivers.extend(other.drivers);
    }
}

pub fn path_for(profile_dir: &Path) -> PathBuf {
    profile_dir.join(".testtimes")
}

/// Read what the last run measured. Any problem is a miss.
pub fn load(path: &Path) -> History {
    let mut history = History::default();
    let Ok(text) = std::fs::read_to_string(path) else {
        return history;
    };
    for line in text.lines() {
        // kind \t lane \t micros \t file \t line \t name
        // The name is last so that a name containing a tab still
        // parses: everything after the fifth field belongs to it.
        let mut parts = line.splitn(6, '\t');
        let (Some(kind), Some(lane), Some(micros), Some(file)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Ok(micros) = micros.parse::<u64>() else {
            continue;
        };
        let took = Duration::from_micros(micros);
        match kind {
            "driver" => {
                history.drivers.insert(file.to_string(), took);
            }
            "test" => {
                let (Some(lane), Some(Ok(line_no)), Some(name)) = (
                    Lane::parse(lane),
                    parts.next().map(str::parse::<u32>),
                    parts.next(),
                ) else {
                    continue;
                };
                history
                    .tests
                    .insert((lane, file.to_string(), line_no, name.to_string()), took);
            }
            _ => {}
        }
    }
    history
}

/// Write `history`, staging to a temp file and renaming.
///
/// Two `toy test` runs in the same package can overlap (a watch loop,
/// two terminals), and a half-written file would be read as a set of
/// missing measurements — harmless, but the rename costs nothing.
/// Failure is ignored: this is a cache.
pub fn save(path: &Path, history: &History) {
    let mut lines: Vec<String> = Vec::new();
    for (file, took) in &history.drivers {
        lines.push(format!("driver\t-\t{}\t{file}", took.as_micros()));
    }
    for ((lane, file, line, name), took) in &history.tests {
        // A name with a newline in it would split the record; a name
        // is a string literal in the source and may hold one.
        let name = name.replace(['\n', '\r'], " ");
        lines.push(format!(
            "test\t{}\t{}\t{file}\t{line}\t{name}",
            lane.tag(),
            took.as_micros()
        ));
    }
    // Sorted so the file is stable between runs that measured the same
    // thing: a diff of it should show what changed, not what a hash
    // map felt like today.
    lines.sort();
    let body = lines.join("\n") + "\n";

    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = dir.join(format!(".testtimes.{}.tmp", std::process::id()));
    if std::fs::write(&tmp, body).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}
