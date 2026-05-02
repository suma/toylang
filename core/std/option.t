package std.option

# Stdlib Option<T>. Auto-loaded from `<core>/std/option.t` so user
# programs can write `val o: Option<i64> = Option::Some(5i64)` and
# `match o { Option::Some(v) => ..., Option::None => ... }` without
# any `import` line.
#
# Option is a stack-stored tagged union (1-byte tag + payload). It
# doesn't take an `Allocator` type parameter — heap responsibility
# belongs to the contained T. `Option<List<u64, Arena>>` works
# unchanged: the inner List holds its allocator, the Option is just
# the discriminant.

enum Option<T> {
    None,
    Some(T),
}

impl<T> Option<T> {
    # Discriminant probes — useful in `if` chains where pattern
    # matching would be overkill.
    fn is_some(self: Self) -> bool {
        match self {
            Option::Some(_) => true,
            Option::None => false,
        }
    }

    fn is_none(self: Self) -> bool {
        match self {
            Option::Some(_) => false,
            Option::None => true,
        }
    }

    # Extract the contained value, falling back to `default` on None.
    # No closures yet, so users supply the default eagerly.
    fn unwrap_or(self: Self, default: T) -> T {
        match self {
            Option::Some(v) => v,
            Option::None => default,
        }
    }

    # Extract the contained value or panic on None. Mirrors Rust's
    # `Option::expect` shape (message is a static string literal).
    fn expect(self: Self, message: str) -> T {
        match self {
            Option::Some(v) => v,
            Option::None => panic("Option::expect on None"),
        }
    }
}
