# NOTE: no `package` line — auto-load derives the module path from the
# file system (`core/std/collections/priority_queue.t -> ["std",
# "collections", "priority_queue"]`).
#
# Stdlib `PriorityQueue<T: Ord>` — a binary min-heap (COLLECTIONS C5,
# `design-docs/COLLECTIONS.md`).
#
# **Smallest first**: `pop` and `peek` answer with the least element by
# `Ord`. `Ord` carries only `lt`, so a max-heap is the same code with
# the comparison turned around; rather than a second type or a stored
# comparator, wrap the element in a struct whose `lt` is reversed —
# that keeps one heap and puts the choice in the element type, where
# `sort` already puts it.
#
# The elements live in a `Vec<T>`, which brings the growth, the bounds
# checks and the `Drop` with it. That is also why this file adds no
# `impl Drop` of its own: the interpreter JIT gives up on a program
# that has a `Drop` impl outside its allow-list, so a stdlib type that
# can avoid declaring one should (see
# `interpreter/src/jit/eligibility/analyze.rs`).
#
# `push` and `pop` are O(log n); `peek`, `size` and `is_empty` are
# O(1). The sift loops are written out inside the two methods rather
# than factored into `sift_up` / `sift_down` helpers: a `&mut self`
# method calling another one on the same receiver is a shape the
# compiled lanes have no need to be asked about here.

struct PriorityQueue<T: Ord> {
    v: Vec<T>,
}

impl<T: Ord> PriorityQueue<T> {
    fn new() -> Self {
        PriorityQueue {
            v: Vec::new(),
        }
    }

    # Add an element and sift it up to its place.
    fn push(&mut self, value: T) {
        self.v.push(value)
        var i: u64 = self.v.size() - 1u64
        while i > 0u64 {
            val parent: u64 = (i - 1u64) / 2u64
            val child: T = self.v.get(i)
            val above: T = self.v.get(parent)
            if !child.lt(above) {
                break
            }
            self.v.set(i, above)
            self.v.set(parent, child)
            i = parent
        }
    }

    # Remove and return the least element, or `None` when empty.
    fn pop(&mut self) -> Option<T> {
        if self.v.is_empty() {
            return Option::None
        }
        val top: T = self.v.get(0u64)
        val last: T = self.v.pop()
        val n: u64 = self.v.size()
        if n > 0u64 {
            self.v.set(0u64, last)
            var i: u64 = 0u64
            loop {
                val left: u64 = i * 2u64 + 1u64
                if left >= n {
                    break
                }
                var least: u64 = left
                val right: u64 = left + 1u64
                if right < n {
                    val rv: T = self.v.get(right)
                    val lv: T = self.v.get(left)
                    if rv.lt(lv) {
                        least = right
                    }
                }
                val cur: T = self.v.get(i)
                val least_val: T = self.v.get(least)
                if !least_val.lt(cur) {
                    break
                }
                self.v.set(i, least_val)
                self.v.set(least, cur)
                i = least
            }
        }
        Option::Some(top)
    }

    # The least element without removing it, or `None` when empty.
    fn peek(&self) -> Option<T> {
        if self.v.is_empty() {
            return Option::None
        }
        val top: T = self.v.get(0u64)
        Option::Some(top)
    }

    fn size(&self) -> u64 {
        self.v.size()
    }

    fn is_empty(&self) -> bool {
        self.v.is_empty()
    }

    fn clear(&mut self) {
        self.v.clear()
    }
}
