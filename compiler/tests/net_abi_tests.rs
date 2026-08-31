//! NETWORK_IO.md N0: the platform constants are checked, not trusted.
//!
//! `toylang_rt` is `no_std` and dependency-free, so every socket and
//! event-notification constant in `sys_epoll.rs` / `sys_kqueue.rs` is
//! transcribed from the system headers by hand. That is the worst
//! failure shape available: a wrong number compiles cleanly, links
//! cleanly, and misbehaves at run time somewhere far from the
//! transcription.
//!
//! So a C probe prints what the headers actually say and this compares
//! it against the values the runtime was built with. It is the whole
//! acceptance criterion for N0 — nothing is connected yet, and this
//! still earns its place, because everything after it stands on these
//! numbers.
//!
//! It checks whichever backend is live, so a macOS run verifies the
//! kqueue file and a Linux run the epoll one. There is no way to check
//! the other half from here; a Linux CI run is the first real test of
//! `sys_epoll.rs`.

use std::collections::HashMap;
use std::process::Command;

const PROBE_C: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/net/abi_probe.c");

fn skip_e2e() -> bool {
    std::env::var("COMPILER_E2E").map(|v| v == "skip").unwrap_or(false)
}

/// Compile and run the probe, returning its `NAME value` lines.
fn header_values() -> HashMap<String, i64> {
    let dir = std::env::temp_dir().join(format!("toy_net_abi_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create probe dir");
    let exe = dir.join("abi_probe");
    let status = Command::new("cc")
        .arg("-o")
        .arg(&exe)
        .arg(PROBE_C)
        .status()
        .expect("spawn cc for the ABI probe");
    assert!(status.success(), "cc for the ABI probe failed");
    let out = Command::new(&exe).output().expect("run the ABI probe");
    assert!(out.status.success(), "the ABI probe exited non-zero");
    let text = String::from_utf8(out.stdout).expect("probe output is ASCII");
    let mut map = HashMap::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(value)) = (parts.next(), parts.next()) else {
            panic!("unparsable probe line: {line:?}");
        };
        let value: i64 = value.parse().unwrap_or_else(|_| panic!("bad value in {line:?}"));
        map.insert(name.to_string(), value);
    }
    let _ = std::fs::remove_dir_all(&dir);
    map
}

/// Every constant the runtime carries agrees with the header it was
/// copied from.
#[test]
fn the_transcribed_platform_constants_match_the_system_headers() {
    if skip_e2e() {
        return;
    }
    let truth = header_values();
    let mut checked = 0usize;
    for (name, ours) in toylang_rt::net_abi_values()
        .into_iter()
        .chain(toylang_rt::net_abi_backend_values())
    {
        let theirs = truth
            .get(name)
            .unwrap_or_else(|| panic!("the probe does not print `{name}`; add a row for it"));
        assert_eq!(
            *theirs, ours,
            "`{name}`: the header says {theirs}, `sys_*.rs` says {ours}"
        );
        checked += 1;
    }
    // A probe that printed nothing, or a value list that lost its
    // rows, would otherwise pass this test silently.
    assert!(checked >= 30, "only {checked} constants were checked");
}

/// The switch resolved to a backend that matches the host.
#[test]
fn the_compiled_backend_matches_the_platform() {
    let name = toylang_rt::net_backend_name();
    let expected = if cfg!(target_os = "linux") { "epoll" } else { "kqueue" };
    assert_eq!(
        name, expected,
        "the `mod sys` switch selected `{name}` on a host that wants `{expected}`"
    );
}
