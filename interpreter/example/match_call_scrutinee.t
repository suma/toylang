# AOT-MATCH-SCRUTINEE-EXPAND: a `match` whose scrutinee is a call to a
# free function returning an enum. This is what `while val` desugars
# into when the producer is a plain function rather than a method.
enum Step { Go(i64), Stop }

fn step(n: i64) -> Step {
    if n < 4i64 { Step::Go(n + 1i64) } else { Step::Stop }
}

fn find(n: i64) -> Option<i64> {
    if n > 0i64 { Option::Some(n * 2i64) } else { Option::None }
}

fn main() -> i64 {
    var total: i64 = 0i64
    var i: i64 = 0i64
    while val Step::Go(next) = step(i) {
        total = total + next
        i = next
    }
    val doubled: i64 = match find(total) {
        Option::Some(v) => v,
        Option::None => 0i64,
    }
    doubled
}
