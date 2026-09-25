# NOTE: no `package` line — auto-load derives the module path from the
# file system (`core/std/collections/deque.t -> ["std", "collections",
# "deque"]`).
#
# Stdlib `Deque<T>` — a double-ended queue over a ring buffer
# (COLLECTIONS C4, `design-docs/COLLECTIONS.md`).
#
# Not built on `Vec<T>`: a vector has no cheap front removal, so
# wrapping one would give a queue whose `pop_front` is O(n) — the whole
# point of the type. The buffer here is the same `__builtin_heap_*`
# allocation a `Vec` uses; what differs is that `head` says where the
# elements start and the indices wrap.
#
# Growth doubles the buffer and, when the elements wrapped past the old
# end, moves just the wrapped prefix up behind them. `head` does not
# move, so nothing else has to be rewritten.

struct Deque<T> {
    data: ptr,
    # index of the front element; meaningless while `len` is 0.
    head: u64,
    len: u64,
    cap: u64,
}

impl<T> Deque<T> {
    fn new() -> Self {
        Deque {
            data: __builtin_heap_alloc(0u64),
            head: 0u64,
            len: 0u64,
            cap: 0u64,
        }
    }

    # Make room for one more element. Doubling, so `n` pushes cost
    # amortised O(1) at either end.
    fn reserve_one(&mut self) {
        if self.len < self.cap {
            return
        }
        val oldcap: u64 = self.cap
        var newcap: u64 = 4u64
        if oldcap > 0u64 {
            newcap = oldcap * 2u64
        }
        self.data = __builtin_heap_realloc(self.data, newcap * __builtin_sizeof::<T>())
        self.cap = newcap
        val p: Ptr<T> = Ptr { addr: self.data }
        # The elements wrapped: [head, oldcap) then [0, wrapped). The
        # second run now belongs directly after the first, at oldcap.
        if oldcap > 0u64 && self.head + self.len > oldcap {
            val wrapped: u64 = self.head + self.len - oldcap
            var i: u64 = 0u64
            while i < wrapped {
                val v: T = p.get(i)
                p.set(oldcap + i, v)
                i = i + 1u64
            }
        }
    }

    fn push_back(&mut self, value: T) {
        self.reserve_one()
        val slot: u64 = (self.head + self.len) % self.cap
        val p: Ptr<T> = Ptr { addr: self.data }
        p.set(slot, value)
        self.len = self.len + 1u64
    }

    fn push_front(&mut self, value: T) {
        self.reserve_one()
        # `head + cap - 1` rather than `head - 1`: u64 subtraction traps
        # rather than wrapping, and `head` is 0 half the time.
        self.head = (self.head + self.cap - 1u64) % self.cap
        val p: Ptr<T> = Ptr { addr: self.data }
        p.set(self.head, value)
        self.len = self.len + 1u64
    }

    # Remove and return the front element. Panics when empty, like
    # `Vec::pop` — the alternative is reading whatever sits at the head
    # slot and underflowing `len`.
    fn pop_front(&mut self) -> T {
        if self.len == 0u64 { panic("Deque::pop_front on an empty Deque") }
        val p: Ptr<T> = Ptr { addr: self.data }
        val v: T = p.get(self.head)
        self.head = (self.head + 1u64) % self.cap
        self.len = self.len - 1u64
        v
    }

    fn pop_back(&mut self) -> T {
        if self.len == 0u64 { panic("Deque::pop_back on an empty Deque") }
        val slot: u64 = (self.head + self.len - 1u64) % self.cap
        val p: Ptr<T> = Ptr { addr: self.data }
        val v: T = p.get(slot)
        self.len = self.len - 1u64
        v
    }

    # Read by logical position: 0 is the front, `size() - 1` the back.
    fn get(&self, index: u64) -> T {
        if index >= self.len { panic("Deque::get index out of bounds") }
        val slot: u64 = (self.head + index) % self.cap
        val p: Ptr<T> = Ptr { addr: self.data }
        val v: T = p.get(slot)
        v
    }

    fn set(&mut self, index: u64, value: T) {
        if index >= self.len { panic("Deque::set index out of bounds") }
        val slot: u64 = (self.head + index) % self.cap
        val p: Ptr<T> = Ptr { addr: self.data }
        p.set(slot, value)
    }

    fn size(&self) -> u64 {
        self.len
    }

    fn capacity(&self) -> u64 {
        self.cap
    }

    fn is_empty(&self) -> bool {
        self.len == 0u64
    }

    # Forget the elements, keep the buffer (`Vec::clear`'s bargain).
    fn clear(&mut self) {
        self.len = 0u64
        self.head = 0u64
    }
}

# DROP-GLUE: same shape as `Vec`'s — the elements are glued by the
# backend before this runs, then the buffer goes.
impl<T> Drop for Deque<T> {
    fn drop(&mut self) {
        __builtin_heap_free(self.data)
    }
}

# Iterator protocol (STDLIB-ITER): front to back.
#
# `head` and `cap` share a field. The iterator's `next` is a
# `&mut self` method, which returns one register per receiver leaf plus
# its result, against the backends' budget of eight.
struct DequeIter<T> {
    data: ptr,
    # head in the high 32 bits, capacity in the low.
    head_cap: u64,
    count: u64,
    index: u64,
}

impl<T> Deque<T> {
    fn iter(&self) -> DequeIter<T> {
        DequeIter {
            data: self.data,
            head_cap: (self.head << 32u64) | self.cap,
            count: self.len,
            index: 0u64,
        }
    }
}

impl<T> Iterator<T> for DequeIter<T> {
    fn next(&mut self) -> Option<T> {
        val p: Ptr<T> = Ptr { addr: self.data }
        if self.index >= self.count {
            Option::None
        } else {
            val head: u64 = self.head_cap >> 32u64
            val cap: u64 = self.head_cap & 0xFFFFFFFFu64
            val slot: u64 = (head + self.index) % cap
            self.index = self.index + 1u64
            val v: T = p.get(slot)
            Option::Some(v)
        }
    }
}

