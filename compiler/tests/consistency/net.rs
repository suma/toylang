//! NETWORK_IO.md N0: the platform switch, seen from toylang.
//!
//! `toylang_rt` picks epoll or kqueue at compile time with one
//! `#[cfg_attr(path)] mod sys;`, and `net::backend_name()` is the only
//! thing that crosses back. That makes it the observable N0 is
//! accepted on: the four lanes each reach the runtime differently —
//! the tree-walker through its extern registry, the JIT through a
//! symbol-table entry, the AOT binary through the linker — and a
//! switch that resolved differently in any of them would show up here
//! rather than in whatever is built on top later.
//!
//! The *value* is host-dependent, so it is not pinned; that the lanes
//! agree, and that the answer is one of the two backends rather than
//! an empty string from a missing symbol, is.

use super::harness::{assert_consistent, interpreter_value};

#[test]
fn every_lane_reports_the_same_event_backend() {
    let src = r#"
        fn main() -> u64 {
            val name: str = net::backend_name()
            if name == "kqueue" {
                2u64
            } elif name == "epoll" {
                1u64
            } else {
                0u64
            }
        }
    "#;
    // 0 would mean the symbol resolved to something that is neither —
    // an empty string, most likely, which is what a missing runtime
    // entry point looks like from here.
    let name = interpreter_value(src) & 0xff;
    assert!(
        name == 1 || name == 2,
        "backend_name() answered neither `epoll` nor `kqueue`"
    );
    let expected = if cfg!(target_os = "linux") { 1 } else { 2 };
    assert_eq!(name, expected, "the switch selected the other platform's backend");
    assert_consistent(src, "net_backend_name");
}
