/*
 * A linked list with `Option<Ptr<Node>>` (POINTER P5).
 *
 * `Ptr<T>` is non-null by construction — `alloc` never hands back a
 * null address — so "there is no next node" lives one level up, in
 * the type: `Option<Ptr<Node>>`. The raw-`ptr` idiom
 * (`interpreter/example/linked_list_ptr.t`) needs a `next: ptr`
 * field *plus* a `has_next: bool` field that must be kept in sync;
 * here the match arm is the truth:
 *
 *     struct Node {
 *         v: i64,
 *         next: Option<Ptr<Node>>,
 *     }
 *
 *     match n.next {
 *         Option::None => 0i64,
 *         Option::Some(p) => { ... }
 *     }
 *
 * The trade is size: the enum layout is a u64 tag plus every
 * variant's payload, and no slot is shared (no niche optimisation),
 * so `Option<Ptr<Node>>` is 16 bytes, not 8.
 */

struct Node {
    v: i64,
    next: Option<Ptr<Node>>,
}

# Sum the list. `Option<Ptr<Node>>` travels by value; `Option::None`
# is the end.
fn total(n: Option<Ptr<Node>>) -> i64 {
    match n {
        Option::None => 0i64,
        Option::Some(p) => {
            val node: Node = p.get(0u64)
            node.v + total(node.next)
        }
    }
}

# Cons a value onto a list, returning the new head.
fn cons(v: i64, rest: Option<Ptr<Node>>) -> Ptr<Node> {
    val p: Ptr<Node> = Ptr::alloc(1u64)
    p.set(0u64, Node { v: v, next: rest })
    p
}

fn main() -> u64 {
    # Build 3 -> 2 -> 1 (cons onto an empty list). Enum constructions
    # that appear as values bind through `val` first (the usual
    # compiled-lane convention).
    val empty: Option<Ptr<Node>> = Option::None
    val n1: Ptr<Node> = cons(1i64, empty)
    val some_n1: Option<Ptr<Node>> = Option::Some(n1)
    val n2: Ptr<Node> = cons(2i64, some_n1)
    val some_n2: Option<Ptr<Node>> = Option::Some(n2)
    val head: Ptr<Node> = cons(3i64, some_n2)

    val some_head: Option<Ptr<Node>> = Option::Some(head)
    println(total(some_head))   # 6
    println(total(empty))       # 0

    # Absence is a value of the same type — a missing field and a
    # present one are distinguished by the match, not by a flag.
    val node: Node = head.get(0u64)
    val has_next: u64 = match node.next {
        Option::None => 0u64,
        Option::Some(_) => 1u64,
    }
    println(has_next)           # 1

    total(some_head) as u64     # 6
}
