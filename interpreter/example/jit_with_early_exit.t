# Exercises early-exit (`return` / `break` / `continue`) inside a JIT-
# compiled `with allocator = …` body. The codegen must emit the
# matching `pop` for every active `with` frame before each exit so the
# runtime allocator stack is balanced.
#
# Expected: 9 + 5 + 25 = 39 → exit 39.
fn early_return_in_with(arena: Allocator) -> u64 {
    with allocator = arena {
        # Tear down the with frame on the early return path.
        return 9u64
    }
    0u64
}

fn break_in_with_loop(arena: Allocator) -> u64 {
    var i = 0u64
    var hits = 0u64
    while i < 100u64 {
        with allocator = arena {
            if i == 5u64 {
                # `break` jumps out of the loop; the `with` frame
                # opened just above must pop first.
                break
            }
            hits = hits + 1u64
        }
        i = i + 1u64
    }
    hits  # 5 iterations completed (i=0..4)
}

fn continue_in_with_loop(arena: Allocator) -> u64 {
    var i = 0u64
    var sum = 0u64
    while i < 10u64 {
        i = i + 1u64
        with allocator = arena {
            if i % 2u64 == 0u64 {
                # `continue` jumps to the loop header; the `with`
                # frame must pop first.
                continue
            }
            sum = sum + i
        }
    }
    sum  # 1+3+5+7+9 = 25
}

fn main() -> u64 {
    val arena = __builtin_arena_allocator()
    early_return_in_with(arena) + break_in_with_loop(arena) + continue_in_with_loop(arena)
}
