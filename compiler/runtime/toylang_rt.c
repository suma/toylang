/*
 * Tiny runtime shipped alongside every compiled toylang executable.
 *
 * The compiler's Cranelift codegen emits direct calls into these
 * helpers when it lowers `print` / `println`. We use `printf` /
 * `puts` from libc here so we don't have to wrestle with platform
 * variadic ABI from cranelift's non-variadic Signature on macOS
 * aarch64 (where variadic args go on the stack rather than in
 * registers — calling printf as if it were a fixed-arity function
 * silently produces garbage there).
 *
 * The driver compiles this file with `cc` on each invocation and
 * links the resulting object next to the toylang `.o`. Compilation
 * is in the order of milliseconds.
 */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>  /* exit() for the allocator-stack guard rails. */

void toy_print_i64(int64_t v) {
    printf("%lld", (long long) v);
}

void toy_println_i64(int64_t v) {
    printf("%lld\n", (long long) v);
}

void toy_print_u64(uint64_t v) {
    printf("%llu", (unsigned long long) v);
}

void toy_println_u64(uint64_t v) {
    printf("%llu\n", (unsigned long long) v);
}

/* NUM-W-AOT-pack Phase 2: dedicated narrow-int printers so the
 * codegen call site names the actual width instead of routing
 * through `sextend`/`uextend` + the wide helper. The decimal
 * output is byte-identical to the wide path (printf %lld/%llu of
 * a sign- or zero-extended value lands on the same digits) — the
 * Phase 2 win is purely codegen aesthetics + one fewer cranelift
 * extension instruction per print site. */

void toy_print_i8(int8_t v) {
    printf("%d", (int) v);
}

void toy_println_i8(int8_t v) {
    printf("%d\n", (int) v);
}

void toy_print_u8(uint8_t v) {
    printf("%u", (unsigned) v);
}

void toy_println_u8(uint8_t v) {
    printf("%u\n", (unsigned) v);
}

void toy_print_i16(int16_t v) {
    printf("%d", (int) v);
}

void toy_println_i16(int16_t v) {
    printf("%d\n", (int) v);
}

void toy_print_u16(uint16_t v) {
    printf("%u", (unsigned) v);
}

void toy_println_u16(uint16_t v) {
    printf("%u\n", (unsigned) v);
}

void toy_print_i32(int32_t v) {
    printf("%d", (int) v);
}

void toy_println_i32(int32_t v) {
    printf("%d\n", (int) v);
}

void toy_print_u32(uint32_t v) {
    printf("%u", (unsigned) v);
}

void toy_println_u32(uint32_t v) {
    printf("%u\n", (unsigned) v);
}

/* #121 Phase B-min: active-allocator stack runtime.
 *
 * Allocator handles are u64 sentinel values:
 *   0 — the default global allocator (routes to libc malloc/realloc/free).
 *   non-zero — currently rejected at compile time (arena / fixed_buffer
 *   would land here in a later phase with actual backend implementations).
 *
 * The stack is a single global fixed-size buffer to keep the runtime
 * dependency-free. 64 nesting levels covers any realistic
 * `with allocator = ... { with ... { ... } }` structure; overflow
 * aborts via libc `exit(1)` since the only way to hit it is a codegen
 * bug.
 */
#define TOY_ALLOC_STACK_CAP 64
static uint64_t toy_alloc_stack[TOY_ALLOC_STACK_CAP];
static int toy_alloc_stack_len = 0;

void toy_alloc_push(uint64_t handle) {
    if (toy_alloc_stack_len >= TOY_ALLOC_STACK_CAP) {
        fputs("toylang runtime: allocator stack overflow\n", stderr);
        exit(1);
    }
    toy_alloc_stack[toy_alloc_stack_len++] = handle;
}

void toy_alloc_pop(void) {
    if (toy_alloc_stack_len <= 0) {
        fputs("toylang runtime: allocator stack underflow\n", stderr);
        exit(1);
    }
    toy_alloc_stack_len--;
}

uint64_t toy_alloc_current(void) {
    if (toy_alloc_stack_len <= 0) {
        return 0; /* Default global allocator sentinel. */
    }
    return toy_alloc_stack[toy_alloc_stack_len - 1];
}

/*
 * #121 Phase B-rest Item 1: arena / fixed_buffer allocator backends.
 *
 * Both backends piggy-back on libc malloc/realloc/free for the
 * actual byte storage; the difference is in tracking and free
 * semantics:
 *   - Arena: free() is a no-op. The arena slot keeps a list of
 *     allocations so a future `arena_drop` could release them all,
 *     but explicit `heap_free` calls under an arena allocator are
 *     intentionally ignored.
 *   - FixedBuffer: alloc() rejects (returns NULL) when the
 *     cumulative `used + size` would exceed `capacity`. free()
 *     finds the entry, decrements `used`, and forwards to
 *     libc free.
 *
 * Allocator handles are 1-based indices into a fixed registry;
 * handle 0 is reserved for the default global allocator (libc
 * direct path, unchanged from Phase A).
 */
#define TOY_ALLOC_REGISTRY_CAP 64
typedef enum { TOY_ALLOC_KIND_ARENA = 1, TOY_ALLOC_KIND_FIXED_BUFFER = 2 } toy_alloc_kind_t;
typedef struct {
    toy_alloc_kind_t kind;
    void **addrs;       /* tracked allocation pointers */
    uint64_t *sizes;    /* parallel size array */
    size_t count;
    size_t cap_array;
    uint64_t capacity;  /* FixedBuffer only */
    uint64_t used;      /* FixedBuffer only */
} toy_alloc_slot_t;

static toy_alloc_slot_t toy_alloc_registry[TOY_ALLOC_REGISTRY_CAP];
static int toy_alloc_registry_len = 0;

static toy_alloc_slot_t *toy_alloc_slot_lookup(uint64_t handle) {
    if (handle == 0) return NULL;
    int idx = (int)(handle - 1);
    if (idx < 0 || idx >= toy_alloc_registry_len) {
        fputs("toylang runtime: invalid allocator handle\n", stderr);
        exit(1);
    }
    return &toy_alloc_registry[idx];
}

static void toy_alloc_slot_track(toy_alloc_slot_t *slot, void *p, uint64_t size) {
    if (slot->count == slot->cap_array) {
        size_t new_cap = slot->cap_array == 0 ? 4 : slot->cap_array * 2;
        slot->addrs = (void **)realloc(slot->addrs, new_cap * sizeof(void *));
        slot->sizes = (uint64_t *)realloc(slot->sizes, new_cap * sizeof(uint64_t));
        slot->cap_array = new_cap;
    }
    slot->addrs[slot->count] = p;
    slot->sizes[slot->count] = size;
    slot->count++;
}

uint64_t toy_arena_new(void) {
    if (toy_alloc_registry_len >= TOY_ALLOC_REGISTRY_CAP) {
        fputs("toylang runtime: allocator registry overflow\n", stderr);
        exit(1);
    }
    int idx = toy_alloc_registry_len++;
    toy_alloc_registry[idx].kind = TOY_ALLOC_KIND_ARENA;
    toy_alloc_registry[idx].addrs = NULL;
    toy_alloc_registry[idx].sizes = NULL;
    toy_alloc_registry[idx].count = 0;
    toy_alloc_registry[idx].cap_array = 0;
    toy_alloc_registry[idx].capacity = 0;
    toy_alloc_registry[idx].used = 0;
    return (uint64_t)(idx + 1);
}

uint64_t toy_fixed_buffer_new(uint64_t capacity) {
    if (toy_alloc_registry_len >= TOY_ALLOC_REGISTRY_CAP) {
        fputs("toylang runtime: allocator registry overflow\n", stderr);
        exit(1);
    }
    int idx = toy_alloc_registry_len++;
    toy_alloc_registry[idx].kind = TOY_ALLOC_KIND_FIXED_BUFFER;
    toy_alloc_registry[idx].addrs = NULL;
    toy_alloc_registry[idx].sizes = NULL;
    toy_alloc_registry[idx].count = 0;
    toy_alloc_registry[idx].cap_array = 0;
    toy_alloc_registry[idx].capacity = capacity;
    toy_alloc_registry[idx].used = 0;
    return (uint64_t)(idx + 1);
}

/*
 * Dispatched alloc/realloc/free: routed from the AOT-emitted
 * `__builtin_heap_alloc` / `_realloc` / `_free` after they read
 * `toy_alloc_current()`. When handle == 0 we hit the default
 * libc path; otherwise arena/fixed_buffer semantics apply.
 */
void *toy_dispatched_alloc(uint64_t handle, uint64_t size) {
    if (handle == 0) {
        return malloc((size_t)size);
    }
    toy_alloc_slot_t *slot = toy_alloc_slot_lookup(handle);
    if (slot->kind == TOY_ALLOC_KIND_FIXED_BUFFER) {
        if (slot->used + size > slot->capacity) {
            return NULL;
        }
    }
    void *p = malloc((size_t)size);
    if (!p) return NULL;
    toy_alloc_slot_track(slot, p, size);
    if (slot->kind == TOY_ALLOC_KIND_FIXED_BUFFER) {
        slot->used += size;
    }
    return p;
}

void toy_dispatched_free(uint64_t handle, void *p) {
    if (handle == 0) {
        free(p);
        return;
    }
    toy_alloc_slot_t *slot = toy_alloc_slot_lookup(handle);
    if (slot->kind == TOY_ALLOC_KIND_ARENA) {
        /* Arena `free` is a no-op; storage lives until arena drop
         * (out of scope for this phase). */
        return;
    }
    /* FixedBuffer: find and remove the tracked entry, return its
     * size to the quota. */
    for (size_t i = 0; i < slot->count; i++) {
        if (slot->addrs[i] == p) {
            slot->used -= slot->sizes[i];
            slot->addrs[i] = slot->addrs[slot->count - 1];
            slot->sizes[i] = slot->sizes[slot->count - 1];
            slot->count--;
            free(p);
            return;
        }
    }
    /* Untracked pointer — defensively forward to libc free. */
    free(p);
}

/*
 * #121 Phase B-rest Item 2 follow-up: explicit arena drop.
 * Releases every allocation tracked by the arena slot and clears
 * the tracking arrays. The slot itself stays in the registry
 * (handles are stable u64 indices), but `used` is reset to 0
 * conceptually — the tracked vectors are emptied. Calling
 * `toy_arena_drop` on a fixed_buffer slot or the default
 * sentinel is a no-op (defensive — releasing fixed-buffer
 * allocations would silently invalidate ptrs the user might
 * still hold).
 */
void toy_arena_drop(uint64_t handle) {
    if (handle == 0) return;
    toy_alloc_slot_t *slot = toy_alloc_slot_lookup(handle);
    if (slot->kind != TOY_ALLOC_KIND_ARENA) return;
    for (size_t i = 0; i < slot->count; i++) {
        free(slot->addrs[i]);
    }
    free(slot->addrs);
    free(slot->sizes);
    slot->addrs = NULL;
    slot->sizes = NULL;
    slot->count = 0;
    slot->cap_array = 0;
}

/*
 * Phase 5 (FixedBuffer auto-cleanup): symmetric to toy_arena_drop
 * but specific to fixed_buffer slots. Releases every tracked
 * allocation, frees the bookkeeping arrays, and resets the quota.
 * No-op for non-fixed_buffer slots (default sentinel / arena) so
 * callers don't accidentally invalidate arena pointers via the
 * wrong builtin. Used by the temporary-form `with allocator =
 * FixedBuffer::new(cap) { ... }` auto-cleanup wiring (the lower
 * layer emits `AllocFixedBufferDrop` at scope exit).
 */
void toy_fixed_buffer_drop(uint64_t handle) {
    if (handle == 0) return;
    toy_alloc_slot_t *slot = toy_alloc_slot_lookup(handle);
    if (slot->kind != TOY_ALLOC_KIND_FIXED_BUFFER) return;
    for (size_t i = 0; i < slot->count; i++) {
        free(slot->addrs[i]);
    }
    free(slot->addrs);
    free(slot->sizes);
    slot->addrs = NULL;
    slot->sizes = NULL;
    slot->count = 0;
    slot->cap_array = 0;
    slot->used = 0;
}

void *toy_dispatched_realloc(uint64_t handle, void *p, uint64_t new_size) {
    if (handle == 0) {
        return realloc(p, (size_t)new_size);
    }
    toy_alloc_slot_t *slot = toy_alloc_slot_lookup(handle);
    if (slot->kind == TOY_ALLOC_KIND_FIXED_BUFFER) {
        uint64_t old_size = 0;
        size_t found = (size_t)-1;
        for (size_t i = 0; i < slot->count; i++) {
            if (slot->addrs[i] == p) {
                old_size = slot->sizes[i];
                found = i;
                break;
            }
        }
        /* Quota check: net delta is new_size - old_size. */
        if (slot->used + new_size < slot->used + old_size) {
            /* shouldn't underflow but be defensive */
        }
        uint64_t projected = slot->used - old_size + new_size;
        if (projected > slot->capacity) {
            return NULL;
        }
        void *np = realloc(p, (size_t)new_size);
        if (!np) return NULL;
        if (found != (size_t)-1) {
            slot->addrs[found] = np;
            slot->sizes[found] = new_size;
            slot->used = projected;
        } else {
            toy_alloc_slot_track(slot, np, new_size);
            slot->used += new_size;
        }
        return np;
    }
    /* Arena: realloc through libc, update tracking. */
    void *np = realloc(p, (size_t)new_size);
    if (!np) return NULL;
    for (size_t i = 0; i < slot->count; i++) {
        if (slot->addrs[i] == p) {
            slot->addrs[i] = np;
            slot->sizes[i] = new_size;
            return np;
        }
    }
    toy_alloc_slot_track(slot, np, new_size);
    return np;
}

void toy_print_bool(uint8_t v) {
    /* Match the interpreter's display: lowercase `true`/`false`. */
    fputs(v ? "true" : "false", stdout);
}

void toy_println_bool(uint8_t v) {
    puts(v ? "true" : "false");
}

void toy_print_str(const char *s) {
    fputs(s, stdout);
}

void toy_println_str(const char *s) {
    puts(s);
}

/* `%g` matches the interpreter's f64 display for typical values; the
 * interpreter forces a decimal point for whole-number f64s, so we use
 * `%.1f` style formatting when the value is integral. printf's `%g`
 * drops the trailing `.0`, which would mismatch — pad with a check. */
static void emit_f64(double v, int newline) {
    if (v == (double) (long long) v) {
        printf("%.1f", v);
    } else {
        printf("%g", v);
    }
    if (newline) {
        putchar('\n');
    }
}

void toy_print_f64(double v) {
    emit_f64(v, 0);
}

void toy_println_f64(double v) {
    emit_f64(v, 1);
}
