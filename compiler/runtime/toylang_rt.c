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
#include <string.h>  /* memcpy() for str_alloc / str_concat. */

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
 * Dispatched alloc / realloc / free: routed from the AOT-emitted
 * `__builtin_heap_alloc` / `_realloc` / `_free` after they read
 * `toy_alloc_current()`. The runtime arena / fixed_buffer registry
 * has been retired — the toylang stdlib `Arena` / `FixedBuffer`
 * (`core/std/allocator.t`) reimplements both policies on top of
 * the default allocator. Today every dispatched call routes
 * straight through libc; the `handle` argument is preserved in the
 * IR for forward compatibility but currently ignored.
 */
/* ---- MEMORY_PROFILING M1: allocation counters --------------------
 *
 * Field-for-field the same definitions as
 * `interpreter/src/heap.rs::MemoryStats`, and mirrored again in
 * `compiler/src/jit.rs` for the JIT. Three implementations is one
 * more than anybody wants, but the alternative — linking this
 * translation unit into the compiler binary so the JIT can call it —
 * would collide with the print helpers, which the JIT deliberately
 * reimplements so it can capture stdout. Agreement is therefore
 * enforced by a test (`--all-backends --profile=mem`) rather than by
 * construction; see MEMORY_PROFILING.md.
 *
 * Everything here is off unless TOY_PROFILE_MEM is set, including the
 * size table, so an unprofiled run allocates exactly what it did
 * before.
 */

static int toy_prof_state = -1; /* -1 unresolved, 0 off, 1 on */
/* Report shape, resolved with the state above: 0 text, 1 JSON. Chosen
 * by TOY_PROFILE_MEM=json, which is what `--profile-format=json` sets
 * when it runs a compiled binary. */
static int toy_prof_json;

static uint64_t toy_prof_alloc_count;
static uint64_t toy_prof_free_count;
static uint64_t toy_prof_realloc_count;
static uint64_t toy_prof_cumulative_bytes;
static uint64_t toy_prof_live_bytes;
static uint64_t toy_prof_peak_live_bytes;
static uint64_t toy_prof_peak_at_request;

/* ptr -> size, open addressing with tombstones. Needed because
 * `realloc` has to be accounted as one resize, which means knowing the
 * old size, and libc does not hand it back. Its own storage uses
 * malloc directly so it never appears in the numbers it records. */
typedef struct {
    void *key;
    uint64_t size;
    uint64_t site; /* packed (line << 32) | column, MEMORY_PROFILING M2 */
    int state; /* 0 empty, 1 occupied, 2 tombstone */
} toy_prof_slot;

/* Per-site totals. Linear scan: an allocation site count is in the
 * dozens for realistic programs, and keeping it an array means the
 * report comes out in insertion order deterministically without
 * sorting a hash table. */
#define TOY_PROF_SITES_CAP 256
typedef struct {
    uint64_t site;
    uint64_t alloc_count;
    uint64_t cumulative_bytes;
    uint64_t live_count;
    uint64_t live_bytes;
} toy_prof_site;
static toy_prof_site toy_prof_sites[TOY_PROF_SITES_CAP];
static int toy_prof_site_len;

static toy_prof_site *toy_prof_site_for(uint64_t site) {
    for (int i = 0; i < toy_prof_site_len; i++) {
        if (toy_prof_sites[i].site == site) {
            return &toy_prof_sites[i];
        }
    }
    if (toy_prof_site_len >= TOY_PROF_SITES_CAP) {
        return NULL; /* beyond the cap the per-site view degrades; totals stay exact */
    }
    toy_prof_site *e = &toy_prof_sites[toy_prof_site_len++];
    e->site = site;
    return e;
}

static toy_prof_slot *toy_prof_tab;
static uint64_t toy_prof_tab_cap;
static uint64_t toy_prof_tab_occupied;

static void toy_prof_report(void);
static void toy_prof_report_leaks(void);

static int toy_prof_enabled(void) {
    if (toy_prof_state < 0) {
        const char *v = getenv("TOY_PROFILE_MEM");
        toy_prof_state = (v && v[0] && v[0] != '0') ? 1 : 0;
        toy_prof_json = (toy_prof_state && strcmp(v, "json") == 0) ? 1 : 0;
        if (toy_prof_state) {
            atexit(toy_prof_report);
        }
    }
    return toy_prof_state;
}

static uint64_t toy_prof_hash(void *p) {
    uint64_t x = (uint64_t) (uintptr_t) p;
    x >>= 4; /* malloc alignment: the low bits carry no information */
    x *= 0x9E3779B97F4A7C15ull;
    return x ^ (x >> 29);
}

static void toy_prof_tab_grow(void);

static void toy_prof_put(void *p, uint64_t size, uint64_t site) {
    if (toy_prof_tab_cap == 0 || (toy_prof_tab_occupied + 1) * 4 >= toy_prof_tab_cap * 3) {
        toy_prof_tab_grow();
    }
    uint64_t mask = toy_prof_tab_cap - 1;
    uint64_t i = toy_prof_hash(p) & mask;
    while (toy_prof_tab[i].state == 1 && toy_prof_tab[i].key != p) {
        i = (i + 1) & mask;
    }
    if (toy_prof_tab[i].state != 1) {
        toy_prof_tab_occupied++;
    }
    toy_prof_tab[i].key = p;
    toy_prof_tab[i].size = size;
    toy_prof_tab[i].site = site;
    toy_prof_tab[i].state = 1;
}

/* Remove `p` and return the size it held, or 0 if it was not tracked
 * (a double free, or a pointer this runtime never handed out). */
static uint64_t toy_prof_take_site;

static uint64_t toy_prof_take(void *p) {
    toy_prof_take_site = 0;
    if (toy_prof_tab_cap == 0) {
        return 0;
    }
    uint64_t mask = toy_prof_tab_cap - 1;
    uint64_t i = toy_prof_hash(p) & mask;
    while (toy_prof_tab[i].state != 0) {
        if (toy_prof_tab[i].state == 1 && toy_prof_tab[i].key == p) {
            uint64_t size = toy_prof_tab[i].size;
            toy_prof_take_site = toy_prof_tab[i].site;
            toy_prof_tab[i].state = 2;
            toy_prof_tab_occupied--;
            return size;
        }
        i = (i + 1) & mask;
    }
    return 0;
}

static void toy_prof_tab_grow(void) {
    uint64_t old_cap = toy_prof_tab_cap;
    toy_prof_slot *old = toy_prof_tab;
    uint64_t new_cap = old_cap ? old_cap * 2 : 256;
    toy_prof_slot *fresh = (toy_prof_slot *) calloc((size_t) new_cap, sizeof(toy_prof_slot));
    if (!fresh) {
        return; /* out of memory while profiling: keep running, lose accuracy */
    }
    toy_prof_tab = fresh;
    toy_prof_tab_cap = new_cap;
    toy_prof_tab_occupied = 0;
    for (uint64_t i = 0; i < old_cap; i++) {
        if (old[i].state == 1) {
            toy_prof_put(old[i].key, old[i].size, old[i].site);
        }
    }
    free(old);
}

static void toy_prof_obtained(uint64_t bytes) {
    toy_prof_cumulative_bytes += bytes;
    toy_prof_live_bytes += bytes;
    if (toy_prof_live_bytes > toy_prof_peak_live_bytes) {
        toy_prof_peak_live_bytes = toy_prof_live_bytes;
        toy_prof_peak_at_request = toy_prof_alloc_count + toy_prof_realloc_count;
    }
}

static void toy_prof_released(uint64_t bytes) {
    if (bytes > toy_prof_live_bytes) {
        toy_prof_live_bytes = 0; /* saturating, as in the interpreter */
    } else {
        toy_prof_live_bytes -= bytes;
    }
}

/* Written to stderr so a profiled run's stdout stays exactly what the
 * program printed. Field names match `MemoryStats` so the three
 * implementations can be compared verbatim. */
/* Sites that still hold memory at exit, in source order so the report
 * is diffable. */
static void toy_prof_report_leaks(void) {
    uint64_t sites = 0, count = 0, bytes = 0;
    for (int i = 0; i < toy_prof_site_len; i++) {
        if (toy_prof_sites[i].live_count > 0) {
            sites++;
            count += toy_prof_sites[i].live_count;
            bytes += toy_prof_sites[i].live_bytes;
        }
    }
    if (sites == 0) {
        return;
    }
    fprintf(stderr, "leaks (%llu sites, %llu allocations, %llu bytes)\n",
            (unsigned long long) sites, (unsigned long long) count,
            (unsigned long long) bytes);
    /* Selection sort by packed position: the table is tiny and this
     * avoids depending on insertion order, which differs from the
     * interpreter's. */
    for (int a = 0; a < toy_prof_site_len; a++) {
        int best = -1;
        for (int i = 0; i < toy_prof_site_len; i++) {
            if (toy_prof_sites[i].live_count == 0) continue;
            if (toy_prof_sites[i].site == UINT64_MAX) continue;
            if (best < 0 || toy_prof_sites[i].site < toy_prof_sites[best].site) best = i;
        }
        if (best < 0) break;
        fprintf(stderr, "  %llu:%llu  %llu allocations  %llu bytes\n",
                (unsigned long long) (toy_prof_sites[best].site >> 32),
                (unsigned long long) (toy_prof_sites[best].site & 0xffffffffu),
                (unsigned long long) toy_prof_sites[best].live_count,
                (unsigned long long) toy_prof_sites[best].live_bytes);
        toy_prof_sites[best].site = UINT64_MAX; /* consumed */
    }
}

/* MEMORY_PROFILING M4. Byte-identical to `MemoryStats::report_json`;
 * both are written out by hand so the mirror is checkable, and the
 * check is `--all-backends --profile=mem --profile-format=json`.
 *
 * Leak entries come out in source order, using the same consume-the-
 * minimum scan as the text report, so the two orderings cannot drift
 * apart. */
static void toy_prof_report_json(void) {
    fprintf(stderr, "{\n  \"memory_profile\": {\n");
    fprintf(stderr, "    \"alloc_count\": %llu,\n", (unsigned long long) toy_prof_alloc_count);
    fprintf(stderr, "    \"free_count\": %llu,\n", (unsigned long long) toy_prof_free_count);
    fprintf(stderr, "    \"realloc_count\": %llu,\n", (unsigned long long) toy_prof_realloc_count);
    fprintf(stderr, "    \"cumulative_bytes\": %llu,\n", (unsigned long long) toy_prof_cumulative_bytes);
    fprintf(stderr, "    \"live_bytes\": %llu,\n", (unsigned long long) toy_prof_live_bytes);
    fprintf(stderr, "    \"peak_live_bytes\": %llu,\n", (unsigned long long) toy_prof_peak_live_bytes);
    fprintf(stderr, "    \"peak_at_request\": %llu\n", (unsigned long long) toy_prof_peak_at_request);
    fprintf(stderr, "  },\n");

    uint64_t leaking = 0;
    for (int i = 0; i < toy_prof_site_len; i++) {
        if (toy_prof_sites[i].live_count > 0) leaking++;
    }
    if (leaking == 0) {
        fprintf(stderr, "  \"leaks\": []\n}\n");
        return;
    }
    fprintf(stderr, "  \"leaks\": [\n");
    for (uint64_t emitted = 0; emitted < leaking; emitted++) {
        int best = -1;
        for (int i = 0; i < toy_prof_site_len; i++) {
            if (toy_prof_sites[i].live_count == 0) continue;
            if (toy_prof_sites[i].site == UINT64_MAX) continue;
            if (best < 0 || toy_prof_sites[i].site < toy_prof_sites[best].site) best = i;
        }
        if (best < 0) break;
        fprintf(stderr,
                "    {\n      \"line\": %llu,\n      \"column\": %llu,\n"
                "      \"allocations\": %llu,\n      \"bytes\": %llu\n    }%s\n",
                (unsigned long long) (toy_prof_sites[best].site >> 32),
                (unsigned long long) (toy_prof_sites[best].site & 0xffffffffu),
                (unsigned long long) toy_prof_sites[best].live_count,
                (unsigned long long) toy_prof_sites[best].live_bytes,
                (emitted + 1 == leaking) ? "" : ",");
        toy_prof_sites[best].site = UINT64_MAX; /* consumed */
    }
    fprintf(stderr, "  ]\n}\n");
}

static void toy_prof_report(void) {
    if (toy_prof_json) {
        toy_prof_report_json();
        return;
    }
    fprintf(stderr, "memory profile\n");
    fprintf(stderr, "  alloc_count       %llu\n", (unsigned long long) toy_prof_alloc_count);
    fprintf(stderr, "  free_count        %llu\n", (unsigned long long) toy_prof_free_count);
    fprintf(stderr, "  realloc_count     %llu\n", (unsigned long long) toy_prof_realloc_count);
    fprintf(stderr, "  cumulative_bytes  %llu\n", (unsigned long long) toy_prof_cumulative_bytes);
    fprintf(stderr, "  live_bytes        %llu\n", (unsigned long long) toy_prof_live_bytes);
    fprintf(stderr, "  peak_live_bytes   %llu\n", (unsigned long long) toy_prof_peak_live_bytes);
    fprintf(stderr, "  peak_at_request   %llu\n", (unsigned long long) toy_prof_peak_at_request);
    toy_prof_report_leaks();
}

void *toy_dispatched_alloc(uint64_t handle, uint64_t size, uint64_t site) {
    (void)handle;
    /* A zero-size request yields the null pointer and is not counted,
     * matching the interpreter. libc would hand back a unique
     * non-null pointer here, which would then differ. */
    if (size == 0) {
        return NULL;
    }
    void *p = malloc((size_t)size);
    if (p && toy_prof_enabled()) {
        toy_prof_alloc_count++;
        toy_prof_obtained(size);
        toy_prof_put(p, size, site);
        toy_prof_site *e = toy_prof_site_for(site);
        if (e) {
            e->alloc_count++;
            e->cumulative_bytes += size;
            e->live_count++;
            e->live_bytes += size;
        }
    }
    return p;
}

void toy_dispatched_free(uint64_t handle, void *p) {
    (void)handle;
    if (!p) {
        return; /* freeing null is a no-op and is not counted */
    }
    if (toy_prof_enabled()) {
        uint64_t size = toy_prof_take(p);
        if (size > 0) {
            toy_prof_free_count++;
            toy_prof_released(size);
            toy_prof_site *e = toy_prof_site_for(toy_prof_take_site);
            if (e) {
                if (e->live_count) e->live_count--;
                e->live_bytes = (e->live_bytes > size) ? e->live_bytes - size : 0;
            }
        }
    }
    free(p);
}

void *toy_dispatched_realloc(uint64_t handle, void *p, uint64_t new_size) {
    if (!p) {
        return toy_dispatched_alloc(handle, new_size, 0);
    }
    if (new_size == 0) {
        toy_dispatched_free(handle, p);
        return NULL;
    }
    (void)handle;
    if (!toy_prof_enabled()) {
        return realloc(p, (size_t)new_size);
    }
    /* One resize request, accounted by the size change the program
     * asked for — never as an allocate plus a free, so that an
     * allocator growing the block in place reports the same numbers. */
    uint64_t old_size = toy_prof_take(p);
    uint64_t site = toy_prof_take_site;
    toy_prof_realloc_count++;
    if (new_size > old_size) {
        toy_prof_obtained(new_size - old_size);
    } else {
        toy_prof_released(old_size - new_size);
    }
    /* A resize keeps the site its block already had, so a leak still
     * points at where the memory came from. */
    toy_prof_site *e = toy_prof_site_for(site);
    if (e) {
        if (new_size > old_size) {
            e->cumulative_bytes += new_size - old_size;
            e->live_bytes += new_size - old_size;
        } else {
            uint64_t shrank = old_size - new_size;
            e->live_bytes = (e->live_bytes > shrank) ? e->live_bytes - shrank : 0;
        }
    }
    void *np = realloc(p, (size_t)new_size);
    toy_prof_put(np ? np : p, new_size, site);
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

/* ---- Heap-allocated str helpers (string interpolation Phase 2) ----
 *
 * AOT `str` runtime layout, per `compiler/src/codegen/lower_inst.rs`
 * `ConstStr` / `Print`:
 *
 *     [bytes...][NUL][u64 len LE]
 *      ^                ^
 *      byte_start       (str runtime value points here)
 *
 * Heap-allocated strings (produced by `__builtin_to_string` and
 * `.concat()`) follow the exact same layout so every consumer of
 * `str` (print / println / strlen / interpolation chain) is
 * pointer-uniform: a `str` value always points at its u64 len
 * field; `byte_start = s - len - 1`.
 *
 * Allocation goes through libc malloc directly rather than the
 * active toylang allocator stack — interpolation strings are
 * typically short-lived and routing them through the user-facing
 * allocator could surprise programs that swap in a quota-limited
 * fixed_buffer for a different purpose. `free` is the caller's
 * responsibility (currently a no-op; relies on process exit).
 */

/* Lay out a fresh heap str. `bytes` may be NULL when len is 0. */
static const char *toy_str_alloc(const char *bytes, uint64_t len) {
    char *base = (char *) malloc(len + 1u + 8u);
    if (!base) {
        fputs("toy_str_alloc: out of memory\n", stderr);
        exit(1);
    }
    if (len > 0 && bytes != NULL) {
        memcpy(base, bytes, (size_t) len);
    }
    base[len] = '\0';
    /* Length stored little-endian (host order = LE on every
     * cranelift target the compiler currently supports). */
    *(uint64_t *) (base + len + 1u) = len;
    return (const char *) (base + len + 1u);
}

/* Concatenate two toylang str values. Both arguments and the
 * result follow the runtime layout described above. */
const char *toy_str_concat(const char *a, const char *b) {
    uint64_t la = *(const uint64_t *) a;
    uint64_t lb = *(const uint64_t *) b;
    const char *a_bytes = a - la - 1u;
    const char *b_bytes = b - lb - 1u;
    uint64_t total = la + lb;
    char *base = (char *) malloc(total + 1u + 8u);
    if (!base) {
        fputs("toy_str_concat: out of memory\n", stderr);
        exit(1);
    }
    if (la > 0) memcpy(base, a_bytes, (size_t) la);
    if (lb > 0) memcpy(base + la, b_bytes, (size_t) lb);
    base[total] = '\0';
    *(uint64_t *) (base + total + 1u) = total;
    return (const char *) (base + total + 1u);
}

/* `__builtin_to_string(value)` lowering — one entry point per
 * primitive type. Each formats with the same conventions
 * `Object::to_display_string` uses in the interpreter so
 * interpreter / AOT stay byte-identical for string-interpolation
 * output. */

const char *toy_to_string_i64(int64_t v) {
    char buf[32];
    int n = snprintf(buf, sizeof(buf), "%lld", (long long) v);
    if (n < 0) n = 0;
    return toy_str_alloc(buf, (uint64_t) n);
}

const char *toy_to_string_u64(uint64_t v) {
    char buf[32];
    int n = snprintf(buf, sizeof(buf), "%llu", (unsigned long long) v);
    if (n < 0) n = 0;
    return toy_str_alloc(buf, (uint64_t) n);
}

const char *toy_to_string_f64(double v) {
    char buf[64];
    int n;
    /* Mirror `emit_f64`'s integral-padding rule so f64 output
     * matches print / println. */
    if (v == (double) (long long) v) {
        n = snprintf(buf, sizeof(buf), "%.1f", v);
    } else {
        n = snprintf(buf, sizeof(buf), "%g", v);
    }
    if (n < 0) n = 0;
    return toy_str_alloc(buf, (uint64_t) n);
}

const char *toy_to_string_bool(uint8_t v) {
    return v ? toy_str_alloc("true", 4) : toy_str_alloc("false", 5);
}

/* str -> str: identity. The desugaring lifts every `{expr}` segment
 * through `__builtin_to_string`, even when `expr` is already `str`,
 * so the codegen call site can stay type-uniform. Returning the
 * original handle avoids a redundant heap copy. */
const char *toy_to_string_str(const char *s) {
    return s;
}

/* Narrow integer to_string variants. Each promotes through the
 * existing snprintf format specifier of the matching width. */
const char *toy_to_string_i8(int8_t v) {
    return toy_to_string_i64((int64_t) v);
}
const char *toy_to_string_u8(uint8_t v) {
    return toy_to_string_u64((uint64_t) v);
}
const char *toy_to_string_i16(int16_t v) {
    return toy_to_string_i64((int64_t) v);
}
const char *toy_to_string_u16(uint16_t v) {
    return toy_to_string_u64((uint64_t) v);
}
const char *toy_to_string_i32(int32_t v) {
    return toy_to_string_i64((int64_t) v);
}
const char *toy_to_string_u32(uint32_t v) {
    return toy_to_string_u64((uint64_t) v);
}
