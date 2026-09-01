# A binary tree, built in one expression.
#
# The recursion is carried by `Box` for the same reason
# `box_linked_list.t` needs it: `Node(Tree, i64, Tree)` has no finite
# layout ([E0013]), while a `Box<Tree>` field is a pointer.
#
# What is new here is the shape of the construction. An enum value
# never flows through SSA as one value — it lives in a tag local plus
# per-variant payload locals — so until ENUM-VARIANT-ARG /
# ENUM-ARG-NEST, every enum that was not the right-hand side of a
# `val` had nowhere to put its leaves. Building this tree took one
# binding per node and per leaf. Now the construction can go straight
# into an argument (`node(leaf(), ...)`) or into another enum's
# payload (`Tree::Node(Box::new(l), v, Box::new(r))`).
#
# `l.get()` is still bound with `val` first: a compound-returning
# method in expression position is the one part of this that has not
# landed (todo.md, COMPOUND-BLOCK-RHS residual).
#
# Run: cargo run -q -p interpreter -- example/box_binary_tree.t
# Expected exit code: 10 (1 + 2 + 3 + 4)
enum Tree {
    Leaf,
    Node(Box<Tree>, i64, Box<Tree>),
}

fn leaf() -> Tree {
    Tree::Leaf
}

fn node(l: Tree, v: i64, r: Tree) -> Tree {
    Tree::Node(Box::new(l), v, Box::new(r))
}

fn sum(t: Tree) -> i64 {
    match t {
        Tree::Leaf => 0i64,
        Tree::Node(l, v, r) => {
            val left: Tree = l.get()
            val right: Tree = r.get()
            sum(left) + v + sum(right)
        }
    }
}

fn main() -> i64 {
    #        2
    #       / \
    #      1   4
    #         /
    #        3
    val t = node(
        node(leaf(), 1i64, leaf()),
        2i64,
        node(node(leaf(), 3i64, leaf()), 4i64, leaf()),
    )
    sum(t)
}
