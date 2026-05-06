# Phase JE-2d end-to-end: a non-generic enum with uniform tuple-
# variant payload now flows across function boundaries through the
# JIT. Constructor / match / arg / return all expand to (tag,
# payload) cranelift values; both interpreter and JIT must return
# exit 42.

enum Status {
    Ok(i64),
    Bad,
}

fn unwrap_or(s: Status, d: i64) -> i64 {
    match s {
        Status::Ok(x) => x,
        Status::Bad => d,
    }
}

fn double_status(s: Status) -> Status {
    match s {
        Status::Ok(x) => Status::Ok(x + x),
        Status::Bad => Status::Bad,
    }
}

fn main() -> i64 {
    val s: Status = Status::Ok(20i64)
    val b: Status = Status::Bad
    val d: Status = double_status(s)
    unwrap_or(d, 0i64) + unwrap_or(b, 2i64)
}
