# Tuple patterns inside `match`. Each sub-pattern can be a literal,
# a name binding, a wildcard, or another nested tuple. An irrefutable
# tuple pattern (no literal sub-patterns) acts as a wildcard for
# exhaustiveness, so the type checker accepts the match below without
# a separate `_` arm.
#
# classify((0, 0)) = 0; (0, _) = 1; (_, 0) = 2; (x, y) = x * y
# Total = 0 + 1 + 2 + 12 = 15
fn classify(p: (u64, u64)) -> u64 {
    match p {
        (0u64, 0u64) => 0u64,
        (0u64, _) => 1u64,
        (_, 0u64) => 2u64,
        (x, y) => x * y,
    }
}

fn main() -> u64 {
    # Bound rather than passed inline: a tuple literal straight into a
    # call is a separate compiler-MVP gap (#160), unrelated to the
    # patterns this example is about.
    val zeros = (0u64, 0u64)
    val first_zero = (0u64, 5u64)
    val second_zero = (9u64, 0u64)
    val neither = (3u64, 4u64)
    classify(zeros) + classify(first_zero) + classify(second_zero) + classify(neither)
}
