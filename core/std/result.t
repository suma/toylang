package std.result

# Stdlib Result<T, E>. Auto-loaded from `<core>/std/result.t` so
# user programs can write
#   `val r: Result<u64, str> = Result::Ok(42u64)`
#   `match r { Result::Ok(v) => ..., Result::Err(e) => ... }`
# without any `import` line.
#
# Like Option, Result is a stack-stored tagged union. It doesn't
# take an `Allocator` — heap responsibility belongs to whichever
# of T or E is allocator-bearing (e.g. `Result<List<u64, A>, str>`
# carries the inner List's allocator transparently).

enum Result<T, E> {
    Ok(T),
    Err(E),
}

impl<T, E> Result<T, E> {
    fn is_ok(self: Self) -> bool {
        match self {
            Result::Ok(_) => true,
            Result::Err(_) => false,
        }
    }

    fn is_err(self: Self) -> bool {
        match self {
            Result::Ok(_) => false,
            Result::Err(_) => true,
        }
    }

    # Extract the Ok value, falling back to `default` on Err. No
    # closures yet, so users supply the default eagerly (Rust's
    # `unwrap_or` shape, not `unwrap_or_else`).
    fn unwrap_or(self: Self, default: T) -> T {
        match self {
            Result::Ok(v) => v,
            Result::Err(_) => default,
        }
    }

    # Panic with `message` on Err, return the Ok value otherwise.
    # Mirrors `Option::expect`.
    fn expect(self: Self, message: str) -> T {
        match self {
            Result::Ok(v) => v,
            Result::Err(_) => panic("Result::expect on Err"),
        }
    }

    # Closures Phase 7 — HOF methods are deferred for `Result<T, E>`.
    # The 2-generic-param enum match unification rejects bodies
    # like `match self { Result::Ok(v) => Result::Ok(v),
    # Result::Err(e) => Result::Err(f(e)) }` because the two
    # arms infer different `Generic(?)` substitutions for the
    # variant the type checker doesn't see literally. The
    # follow-up is tracked in `todo.md` (96残-後半). Use a
    # direct `match` until then.
}
