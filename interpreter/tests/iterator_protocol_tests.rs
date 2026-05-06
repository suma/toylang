// Iterator protocol tests — `for x in EXPR { body }`.
//
// `EXPR` is any value whose type provides `fn next(&mut self) -> Option<T>`
// (structural / duck-typed; the parser desugars at parse time so no
// `trait Iterator<T>` declaration is required — generic-trait support
// is on the deferred list, see `core/std/iter.t`).
//
// The desugaring (in `frontend/src/parser/stmt.rs::desugar_for_in_iterator`):
//
//     for x in EXPR { body }
//   ⇒ {
//         var __iter_for_<n> = EXPR
//         while true {
//             match __iter_for_<n>.next() {
//                 Option::Some(x) => { body; continue },
//                 Option::None    => { break },
//             }
//         }
//     }
//
// Bare integer ranges (`for i in 0..N` / `for i in 0 to N`) keep
// their dedicated `Stmt::For` integer fast path and don't flow
// through this protocol.

mod common;

use common::{assert_program_result_i64, assert_program_result_u64};

const COUNTER_PRELUDE: &str = "
struct Counter {
    current: i64,
    end: i64,
}
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
";

fn program_with_counter(main_body: &str) -> String {
    format!("{COUNTER_PRELUDE}\nfn main() -> i64 {{\n{main_body}\n}}\n")
}

#[test]
fn iterator_basic_sums_to_ten() {
    // 0+1+2+3+4 = 10
    assert_program_result_i64(
        &program_with_counter(
            "    var sum = 0i64
    var iter = Counter::new(5i64)
    for x in iter { sum = sum + x }
    sum",
        ),
        10,
    );
}

#[test]
fn iterator_break_terminates_early() {
    // first five values yielded, then break — sum 0+1+2+3+4 = 10
    assert_program_result_i64(
        &program_with_counter(
            "    var sum = 0i64
    var iter = Counter::new(100i64)
    for x in iter {
        if x >= 5i64 { break }
        sum = sum + x
    }
    sum",
        ),
        10,
    );
}

#[test]
fn iterator_continue_skips_iteration() {
    // 0..10 keeping evens: 0+2+4+6+8 = 20
    assert_program_result_i64(
        &program_with_counter(
            "    var sum = 0i64
    var iter = Counter::new(10i64)
    for x in iter {
        if x % 2i64 == 1i64 { continue }
        sum = sum + x
    }
    sum",
        ),
        20,
    );
}

#[test]
fn iterator_return_propagates_from_for_body() {
    assert_program_result_i64(
        &format!(
            "{COUNTER_PRELUDE}
fn first_ge(threshold: i64) -> i64 {{
    var iter = Counter::new(100i64)
    for x in iter {{
        if x >= threshold {{ return x }}
    }}
    -1i64
}}
fn main() -> i64 {{
    first_ge(7i64)
}}
"
        ),
        7,
    );
}

#[test]
fn iterator_nested_two_loops() {
    // sum_{i,j in 0..3} i*j
    //  = (0+0+0)+(0+1+2)+(0+2+4) = 9
    assert_program_result_i64(
        &program_with_counter(
            "    var total = 0i64
    var outer = Counter::new(3i64)
    for i in outer {
        var inner = Counter::new(3i64)
        for j in inner { total = total + i * j }
    }
    total",
        ),
        9,
    );
}

#[test]
fn iterator_zero_iterations_when_immediately_none() {
    assert_program_result_i64(
        &program_with_counter(
            "    var sum = 0i64
    var iter = Counter::new(0i64)
    for x in iter { sum = sum + x + 1i64 }
    sum",
        ),
        0,
    );
}

#[test]
fn integer_range_fast_path_still_works() {
    // Regression: the existing `for i in 0..N` and `for i in 0 to N`
    // forms must continue to use the dedicated `Stmt::For` integer
    // path (no iterator desugaring).
    assert_program_result_u64(
        "fn main() -> u64 {
            var sum = 0u64
            for i in 0u64..5u64 { sum = sum + i }
            sum
        }",
        10,
    );
    assert_program_result_u64(
        "fn main() -> u64 {
            var sum = 0u64
            for i in 0u64 to 5u64 { sum = sum + i }
            sum
        }",
        10,
    );
}
