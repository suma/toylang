# Iterator protocol demo. `for x in EXPR { body }` works against
# any struct that exposes `fn next(&mut self) -> Option<T>` —
# structural / duck-typed, no `trait Iterator<T>` declaration is
# required (generic traits aren't supported yet).
#
# The parser desugars at parse time:
#
#     for x in EXPR { body }
#   ⇒ {
#         var __iter = EXPR
#         while true {
#             match __iter.next() {
#                 Option::Some(x) => { body; continue },
#                 Option::None    => { break },
#             }
#         }
#     }
#
# Range-based for-loops (`for i in 0..N` / `for i in 0 to N`)
# keep the existing integer fast path and don't flow through
# this protocol.

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

fn main() -> i64 {
    var sum = 0i64
    var iter = Counter::new(5i64)
    for x in iter {
        sum = sum + x
    }
    # 0+1+2+3+4 = 10
    sum
}
