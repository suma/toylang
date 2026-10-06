# Stdlib `Task<T>` — the result of a `spawn { ... }` block
# (CONCURRENCY B, `design-docs/CONCURRENCY.md` section 7).
#
#     val t: Task<Result<u64, IoError>> = spawn {
#         io::write_file(path, bytes)
#     }
#     # ... keep serving ...
#     if t.is_done() {
#         val r = t.join()
#     }
#
# The block's owned captures move into it; it hands back one `T`.
#
# ## Sequential lanes
#
# The tree-walker and the IR VM run the block to completion where it is
# written, so a task they hand out is always done. A child finishing
# before its parent looks is one of the orders real threads can take,
# which is why every lane gives the same answer
# (CONCURRENCY.md section 7, point 2).
#
# ## Representation
#
# The result lives in a `Vec<T>` of length one rather than behind a
# pointer of its own: `join` moves it out with `pop`, and a task that
# dies unjoined drops it through the vector's drop glue, so neither
# path needs the backends to know about `Task`.
#
# `handle` names the thread running the block, or is 0 when the block
# has already run (always, on a sequential lane).
#
# ## API
#
#   - `t.is_done() -> bool` — whether `join` would return at once
#   - `t.join() -> T` — wait for the block and take its value (once;
#     a second `join` panics)
#   - dropping an unjoined task waits for it and drops the value
#     (no thread outlives its task; section 7, point 4)

extern fn __extern_task_wait(handle: u64) from "toylang_rt" as "toy_task_wait"
extern fn __extern_task_done(handle: u64) -> u64 from "toylang_rt" as "toy_task_done"
extern fn __extern_task_release(handle: u64) from "toylang_rt" as "toy_task_release"

struct Task<T> {
    result: Vec<T>,
    handle: u64,
}

impl<T> Task<T> {
    # What `spawn { body }` becomes on a lane that runs the body in
    # place: the body's value, already done. Not for calling directly.
    fn __ready(value: T) -> Self {
        var result: Vec<T> = Vec::with_capacity(1u64)
        result.push(value)
        Task { result: result, handle: 0u64 }
    }

    fn is_done(&self) -> bool {
        __extern_task_done(self.handle) == 1u64
    }

    # Wait for the body and take its value. The task itself lives on
    # until its scope ends, when its drop releases the handle; a second
    # `join` finds the slot empty and panics.
    fn join(&mut self) -> T {
        __extern_task_wait(self.handle)
        if self.result.size() == 0u64 {
            panic("Task::join: the task was already joined")
        }
        val v: T = self.result.pop()
        v
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        __extern_task_release(self.handle)
    }
}
