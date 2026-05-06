# Phase JE-2b/c end-to-end: a non-generic enum with a uniform
# tuple-variant payload now compiles via the JIT. The constructor
# emits (tag, payload), match scrutinee dispatches on tag and the
# `Status::Ok(x)` arm binds x to the payload Variable. Both
# interpreter and JIT must return exit 42.

enum Status {
    Ok(i64),
    Bad,
}

fn main() -> i64 {
    val s: Status = Status::Ok(40i64)
    val b: Status = Status::Bad
    val a: i64 = match s {
        Status::Ok(x) => x,
        Status::Bad => 0i64,
    }
    val c: i64 = match b {
        Status::Ok(_) => 99i64,
        Status::Bad => 2i64,
    }
    a + c
}
