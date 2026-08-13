use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

/// Runtime allocator abstraction.
///
/// Implementations plug into the ambient allocator stack so `with allocator = ...`
/// blocks redirect `__builtin_heap_alloc` and friends to a scoped allocator.
/// Methods take `&self` and rely on interior mutability so multiple stack
/// entries (and `Object::Allocator` values) can share the same underlying
/// state via `Rc`.
pub trait Allocator: fmt::Debug {
    fn alloc(&self, size: usize) -> usize;
    /// Allocate, attributing the block to a source position
    /// (MEMORY_PROFILING M2). Defaults to dropping the attribution so
    /// an allocator that does not track sites needs no change.
    fn alloc_at(&self, size: usize, _site: u64) -> usize {
        self.alloc(size)
    }
    fn free(&self, addr: usize) -> bool;
    fn realloc(&self, addr: usize, new_size: usize) -> usize;
}

/// Default allocator backed by the process-wide `HeapManager`. Every
/// `EvaluationContext` creates one `GlobalAllocator` at initialization and
/// keeps it at the bottom of the allocator stack so code outside any
/// `with` block still has a valid target for heap operations.
#[derive(Debug, Clone)]
pub struct GlobalAllocator {
    inner: Rc<RefCell<HeapManager>>,
}

impl GlobalAllocator {
    pub fn new(inner: Rc<RefCell<HeapManager>>) -> Self {
        Self { inner }
    }
}

impl Allocator for GlobalAllocator {
    fn alloc(&self, size: usize) -> usize {
        self.inner.borrow_mut().alloc(size)
    }

    fn alloc_at(&self, size: usize, site: u64) -> usize {
        self.inner.borrow_mut().alloc_at(size, site)
    }

    fn free(&self, addr: usize) -> bool {
        self.inner.borrow_mut().free(addr)
    }

    fn realloc(&self, addr: usize, new_size: usize) -> usize {
        self.inner.borrow_mut().realloc(addr, new_size)
    }
}

// `ArenaAllocator` / `FixedBufferAllocator` (runtime-side wrappers
// around the shared `HeapManager`) used to live here. The toylang
// stdlib `Arena` / `FixedBuffer` (`core/std/allocator.t`) replaces
// them: tracking + bulk-free + quota enforcement happen in toylang
// code on top of the default allocator. The runtime types and their
// builtins (`__builtin_arena_allocator` / `__builtin_arena_drop` /
// `__builtin_fixed_buffer_allocator` / `__builtin_fixed_buffer_drop`)
// were retired together.

/// Allocation counters (MEMORY_PROFILING M0).
///
/// **Every field is defined on what the program *requested*, never on
/// what the allocator did with the request.** That is the whole point:
/// the interpreter's heap is a bump allocator that never reuses an
/// address, while the AOT path is libc `malloc`, which does — a
/// program can observe the difference today. Any counter derived from
/// addresses or region layout would therefore disagree between
/// backends by construction. Sizes and request order do not.
///
/// Fragmentation is deliberately absent here. It is a property of an
/// allocator's layout, so it belongs to `trait Alloc` and lands in M3;
/// deriving it from these numbers would be inventing it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryStats {
    /// Number of allocation requests. A `realloc` is *not* counted
    /// here even when the implementation services it by allocating —
    /// the program asked for one resize, not for an allocate plus a
    /// free, and an implementation that grows in place must produce
    /// the same number.
    pub alloc_count: u64,
    /// Number of free requests. Freeing a null pointer is a no-op and
    /// is not counted.
    pub free_count: u64,
    /// Number of resize requests, whether the block moved or not.
    pub realloc_count: u64,
    /// Total bytes ever obtained. Never decreases. A `realloc` that
    /// grows contributes the growth (`new - old`); one that shrinks
    /// contributes nothing, because no new bytes were obtained.
    pub cumulative_bytes: u64,
    /// Bytes currently held: obtained and not yet released. A shrinking
    /// `realloc` lowers it.
    pub live_bytes: u64,
    /// Highest value `live_bytes` reached.
    pub peak_live_bytes: u64,
    /// Value of `alloc_count + realloc_count` when `peak_live_bytes`
    /// was last raised — the reproducible stand-in for "when".
    ///
    /// Wall-clock time is deliberately not recorded: a report has to be
    /// byte-identical between runs to be diffable or assertable, and a
    /// timestamp makes that impossible.
    pub peak_at_request: u64,
}

impl MemoryStats {
    /// All-zero value, usable in a `const` context.
    pub const ZERO: MemoryStats = MemoryStats {
        alloc_count: 0,
        free_count: 0,
        realloc_count: 0,
        cumulative_bytes: 0,
        live_bytes: 0,
        peak_live_bytes: 0,
        peak_at_request: 0,
    };

    /// Render the report shared by every backend.
    ///
    /// The three implementations (this one, `toylang_rt.c`, and the JIT
    /// mirror in `compiler/src/jit.rs`) print byte-identical text, so
    /// `--all-backends --profile=mem` can compare them without parsing
    /// anything cleverly. Field names are the struct's own, and no
    /// value is humanised — a rounded "38.2 KB" would make two runs
    /// that differ by a byte look equal.
    pub fn report(&self) -> String {
        let mut out = String::from("memory profile\n");
        for (name, value) in [
            ("alloc_count", self.alloc_count),
            ("free_count", self.free_count),
            ("realloc_count", self.realloc_count),
            ("cumulative_bytes", self.cumulative_bytes),
            ("live_bytes", self.live_bytes),
            ("peak_live_bytes", self.peak_live_bytes),
            ("peak_at_request", self.peak_at_request),
        ] {
            out.push_str(&format!("  {name:<16}  {value}\n"));
        }
        out
    }

    /// The leak section, or an empty string when nothing leaked.
    ///
    /// Byte-identical to what `toy_prof_report_leaks` in the C runtime
    /// prints, so the three implementations stay comparable verbatim.
    /// Sites are emitted in source order; a hash order would make the
    /// report differ between runs of the same program.
    pub fn leak_report(sites: &[(u64, SiteStats)]) -> String {
        let leaked: Vec<&(u64, SiteStats)> =
            sites.iter().filter(|(_, s)| s.live_count > 0).collect();
        if leaked.is_empty() {
            return String::new();
        }
        let count: u64 = leaked.iter().map(|(_, s)| s.live_count).sum();
        let bytes: u64 = leaked.iter().map(|(_, s)| s.live_bytes).sum();
        let mut out = format!(
            "leaks ({} sites, {count} allocations, {bytes} bytes)\n",
            leaked.len()
        );
        for (site, s) in leaked {
            out.push_str(&format!(
                "  {}:{}  {} allocations  {} bytes\n",
                site >> 32,
                site & 0xffff_ffff,
                s.live_count,
                s.live_bytes
            ));
        }
        out
    }

    /// The whole report as JSON (MEMORY_PROFILING M4).
    ///
    /// Hand-written rather than derived through `serde`, for the same
    /// reason [`Self::report`] is: `toylang_rt.c` has to emit the same
    /// bytes with `fprintf`, and a mirror is only checkable when both
    /// sides are written out. Nothing here is a string, so there is no
    /// escaping to get subtly different between the two.
    ///
    /// `leaks` is always present, `[]` when nothing leaked — the text
    /// report omits the section entirely, which is right for a human
    /// skimming stderr and wrong for a consumer that would then have to
    /// tell "no leaks" from "this producer predates leak reporting".
    pub fn report_json(&self, sites: &[(u64, SiteStats)], layouts: &[AllocatorLayoutReport]) -> String {
        let mut out = String::from("{\n  \"memory_profile\": {\n");
        let fields = [
            ("alloc_count", self.alloc_count),
            ("free_count", self.free_count),
            ("realloc_count", self.realloc_count),
            ("cumulative_bytes", self.cumulative_bytes),
            ("live_bytes", self.live_bytes),
            ("peak_live_bytes", self.peak_live_bytes),
            ("peak_at_request", self.peak_at_request),
        ];
        for (i, (name, value)) in fields.iter().enumerate() {
            let comma = if i + 1 == fields.len() { "" } else { "," };
            out.push_str(&format!("    \"{name}\": {value}{comma}\n"));
        }
        out.push_str("  },\n");

        let leaked: Vec<&(u64, SiteStats)> =
            sites.iter().filter(|(_, s)| s.live_count > 0).collect();
        if leaked.is_empty() {
            out.push_str("  \"leaks\": [],\n");
        } else {
            out.push_str("  \"leaks\": [\n");
            for (i, (site, s)) in leaked.iter().enumerate() {
                let comma = if i + 1 == leaked.len() { "" } else { "," };
                out.push_str(&format!(
                    "    {{\n      \"line\": {},\n      \"column\": {},\n      \"allocations\": {},\n      \"bytes\": {}\n    }}{comma}\n",
                    site >> 32,
                    site & 0xffff_ffff,
                    s.live_count,
                    s.live_bytes
                ));
            }
            out.push_str("  ],\n");
        }
        // The layouts section is always present, so "leaks" above always
        // takes a trailing comma.
        out.push_str(&allocator_layout_report_json(layouts));
        out.push_str("}\n");
        out
    }

    /// One counter by name, for the `__builtin_*` readers
    /// (MEMORY_PROFILING M4).
    ///
    /// The single definition of which builtin maps to which field:
    /// the tree-walker, the IR VM and the compiler-side JIT all go
    /// through here, so only the C runtime restates it.
    pub fn field(&self, stat: frontend::ast::MemStat) -> u64 {
        use frontend::ast::MemStat;
        match stat {
            MemStat::AllocCount => self.alloc_count,
            MemStat::FreeCount => self.free_count,
            MemStat::ReallocCount => self.realloc_count,
            MemStat::CumulativeBytes => self.cumulative_bytes,
            MemStat::LiveBytes => self.live_bytes,
            MemStat::PeakLiveBytes => self.peak_live_bytes,
        }
    }

    /// Requests that obtained memory, in program order. Used as the
    /// reproducible time axis.
    fn request_seq(&self) -> u64 {
        self.alloc_count + self.realloc_count
    }

    /// Record `bytes` newly obtained and refresh the peak.
    ///
    /// Public so the JIT mirror in `compiler/src/jit.rs` reuses the
    /// arithmetic instead of restating it — the counting *sites* differ
    /// per backend, the accounting must not.
    pub fn record_obtained(&mut self, bytes: u64) {
        self.cumulative_bytes += bytes;
        self.live_bytes += bytes;
        if self.live_bytes > self.peak_live_bytes {
            self.peak_live_bytes = self.live_bytes;
            self.peak_at_request = self.request_seq();
        }
    }

    /// Record `bytes` released. Saturating because a double free or a
    /// free of an untracked address must not wrap the counter into a
    /// nonsense number; the accounting stays monotone even when the
    /// program misbehaves.
    pub fn record_released(&mut self, bytes: u64) {
        self.live_bytes = self.live_bytes.saturating_sub(bytes);
    }
}

thread_local! {
    /// Process-wide (per-thread) allocation totals.
    ///
    /// A run does not have one heap: the tree-walker allocates through
    /// the `HeapManager` its `EvaluationContext` owns, while the JIT
    /// path installs a second one in `RuntimeState`. Reporting "the
    /// heap's" numbers would therefore mean guessing which heap ran the
    /// program. Every `HeapManager` folds into this instead, so the
    /// total is right whichever path executed — and stays right if a
    /// run uses both.
    ///
    /// Read by `--profile=mem` (MEMORY_PROFILING M1). M4 replaces the
    /// read-after-the-fact shape with builtins that can be called
    /// mid-run.
    static PROFILE: std::cell::Cell<MemoryStats> = const {
        std::cell::Cell::new(MemoryStats::ZERO)
    };
}

/// What one allocation site did (MEMORY_PROFILING M2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SiteStats {
    pub alloc_count: u64,
    pub cumulative_bytes: u64,
    /// Allocations from this site that were never freed, and their
    /// bytes — the leak report.
    pub live_count: u64,
    pub live_bytes: u64,
}

/// One allocator's final layout, as registered by
/// `__builtin_record_allocator_layout` (MEMORY_PROFILING M3 residual).
///
/// A region-owning allocator reports its layout so `--profile=mem` can
/// fold fragmentation into the report. The runtime profiler cannot
/// reach back into a toylang object once the run has ended, so the
/// allocator pushes its numbers here — the stdlib `SlotRegion` does it
/// from `Drop`, which fires just before `main` returns and is what
/// makes the report automatic rather than opt-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllocatorLayoutReport {
    pub name: String,
    pub managed_bytes: u64,
    pub live_bytes: u64,
    pub free_blocks: u64,
    pub largest_free: u64,
}

impl AllocatorLayoutReport {
    /// External fragmentation as a permille — the free bytes that are
    /// *not* in the largest contiguous run, as a fraction of all free
    /// bytes. Identical to the stdlib `AllocLayout::external_fragmentation_permille`;
    /// pure integer arithmetic so the number is exact and byte-identical
    /// across backends. Zero when nothing is free (an allocator with no
    /// free space is full, not fragmented).
    pub fn external_fragmentation_permille(&self) -> u64 {
        if self.managed_bytes <= self.live_bytes {
            return 0;
        }
        let free_total = self.managed_bytes - self.live_bytes;
        if free_total == 0 {
            return 0;
        }
        let scattered = free_total - self.largest_free;
        scattered * 1000 / free_total
    }
}

thread_local! {
    /// Per-thread registered allocator layouts, in registration order
    /// (which is deterministic — it follows the program's `Drop`
    /// order). Separate from the counters so a profile run and an
    /// ordinary run can share the collection point.
    static PROFILE_ALLOCATORS: RefCell<Vec<AllocatorLayoutReport>> =
        const { RefCell::new(Vec::new()) };
}

/// Register an allocator's layout (MEMORY_PROFILING M3 residual).
pub fn record_allocator_layout(
    name: &str,
    managed: u64,
    live: u64,
    free_blocks: u64,
    largest: u64,
) {
    PROFILE_ALLOCATORS.with(|v| {
        v.borrow_mut().push(AllocatorLayoutReport {
            name: name.to_string(),
            managed_bytes: managed,
            live_bytes: live,
            free_blocks,
            largest_free: largest,
        });
    });
}

/// The registered allocator layouts since the last [`reset_profile`].
pub fn allocator_layouts() -> Vec<AllocatorLayoutReport> {
    PROFILE_ALLOCATORS.with(|v| v.borrow().clone())
}

/// Render the allocator-layout section of the text report, or an empty
/// string when no region-owning allocator registered one.
///
/// Byte-identical to `toy_prof_report_layouts` in the C runtime and the
/// JIT mirror, so `--all-backends --profile=mem` can compare it
/// verbatim. Entries are in registration order (the program's `Drop`
/// order), which is deterministic.
pub fn allocator_layout_report_text(layouts: &[AllocatorLayoutReport]) -> String {
    if layouts.is_empty() {
        return String::new();
    }
    let mut out = String::from("allocator layouts\n");
    for l in layouts {
        out.push_str(&format!(
            "  {}  managed {}  live {}  free_blocks {}  largest_free {}  external_fragmentation {} permille\n",
            l.name,
            l.managed_bytes,
            l.live_bytes,
            l.free_blocks,
            l.largest_free,
            l.external_fragmentation_permille()
        ));
    }
    out
}

/// Render the `"layouts"` JSON fragment (MEMORY_PROFILING M3 residual),
/// always present so a consumer can tell "no allocators registered a
/// layout" from "this producer predates layout reporting". Written out
/// by hand for the same reason as [`MemoryStats::report_json`]: the C
/// runtime emits the same bytes with `fprintf`, and only a test keeps
/// the two in step.
///
/// The name is emitted verbatim; the stdlib registers fixed identifier
/// names (`SlotRegion`), which never need escaping.
pub fn allocator_layout_report_json(layouts: &[AllocatorLayoutReport]) -> String {
    if layouts.is_empty() {
        return "  \"layouts\": []\n".to_string();
    }
    let mut out = String::from("  \"layouts\": [\n");
    for (i, l) in layouts.iter().enumerate() {
        let comma = if i + 1 == layouts.len() { "" } else { "," };
        out.push_str(&format!(
            "    {{\n      \"name\": \"{}\",\n      \"managed\": {},\n      \"live\": {},\n      \"free_blocks\": {},\n      \"largest_free\": {},\n      \"external_fragmentation_permille\": {}\n    }}{comma}\n",
            l.name,
            l.managed_bytes,
            l.live_bytes,
            l.free_blocks,
            l.largest_free,
            l.external_fragmentation_permille()
        ));
    }
    out.push_str("  ]\n");
    out
}

thread_local! {
    /// Per-site totals, keyed by the packed `(line << 32) | column` the
    /// allocation site carries. Separate from `PROFILE` because
    /// `MemoryStats` is `Copy` and a map is not.
    static PROFILE_SITES: RefCell<std::collections::BTreeMap<u64, SiteStats>> =
        const { RefCell::new(std::collections::BTreeMap::new()) };
}

/// Per-site totals since the last [`reset_profile`], ordered by source
/// position. A `BTreeMap` rather than a hash map so the report is
/// emitted in a stable order — a hash-ordered report would not be
/// diffable, which is the property the whole design rests on.
pub fn profile_sites() -> Vec<(u64, SiteStats)> {
    PROFILE_SITES.with(|m| m.borrow().iter().map(|(k, v)| (*k, *v)).collect())
}

/// Clear the per-thread totals. The profiling CLI calls this before a
/// run so the numbers describe that run alone.
pub fn reset_profile() {
    PROFILE.with(|p| p.set(MemoryStats::ZERO));
    PROFILE_SITES.with(|m| m.borrow_mut().clear());
    PROFILE_ALLOCATORS.with(|v| v.borrow_mut().clear());
}

/// The per-thread totals accumulated since the last [`reset_profile`].
pub fn profile() -> MemoryStats {
    PROFILE.with(|p| p.get())
}

/// Everything [`profile`] and [`profile_sites`] would report, captured
/// so an abandoned execution attempt can be rolled back.
#[derive(Debug, Clone)]
pub struct ProfileSnapshot {
    totals: MemoryStats,
    sites: std::collections::BTreeMap<u64, SiteStats>,
    allocators: Vec<AllocatorLayoutReport>,
}

/// Capture the counters before an execution attempt that might be
/// abandoned (MEMORY_PROFILING M4).
///
/// A run tries the JIT, then the IR VM, then the tree-walker, and an
/// engine that fails partway has already allocated. Those allocations
/// belong to no run: the answer the user gets comes from whichever
/// engine finished. Without this, a program that panics under the IR
/// VM and is re-run by the tree-walker reports every allocation twice,
/// and `__builtin_live_bytes()` returns a number that never described
/// any state the program was in.
///
/// The allocator-layout registry is rolled back too: a `Drop` that
/// fired under the abandoned engine would otherwise register the same
/// allocator twice (once per engine).
pub fn snapshot_profile() -> ProfileSnapshot {
    ProfileSnapshot {
        totals: PROFILE.with(|p| p.get()),
        sites: PROFILE_SITES.with(|m| m.borrow().clone()),
        allocators: PROFILE_ALLOCATORS.with(|v| v.borrow().clone()),
    }
}

/// Roll the counters back to `snapshot`, discarding whatever the
/// abandoned attempt recorded.
pub fn restore_profile(snapshot: ProfileSnapshot) {
    PROFILE.with(|p| p.set(snapshot.totals));
    PROFILE_SITES.with(|m| *m.borrow_mut() = snapshot.sites);
    PROFILE_ALLOCATORS.with(|v| *v.borrow_mut() = snapshot.allocators);
}

/// Simple heap memory manager for pointer operations
#[derive(Debug)]
pub struct HeapManager {
    memory: Vec<u8>,
    allocations: HashMap<usize, (usize, u64)>, // address -> (size, site)
    next_addr: usize,
    // Typed-slot storage keyed by (base address, byte offset). When a write
    // stores a non-u64 value (bool, i64, user struct, enum variant, ...)
    // the evaluator records the `RcObject` here so a matching `ptr_read`
    // can return it verbatim without round-tripping through the byte buffer.
    // u64 writes also update the byte buffer for backward compatibility with
    // byte-level reads, but still deposit the Rc here to keep a single source
    // of truth.
    typed_slots: HashMap<(usize, usize), crate::object::RcObject>,
    /// MEMORY_PROFILING M0. Updated by the public `alloc` / `free` /
    /// `realloc` entry points only — never by the internal calls
    /// `realloc` makes to service a move, which would count one resize
    /// as an allocate plus a free and make the numbers depend on the
    /// implementation strategy.
    stats: MemoryStats,
}

impl HeapManager {
    pub fn new() -> Self {
        Self {
            memory: Vec::new(),
            allocations: HashMap::new(),
            next_addr: 1, // 0 is reserved for null pointer
            typed_slots: HashMap::new(),
            stats: MemoryStats::default(),
        }
    }

    /// Allocation counters for this heap.
    pub fn stats(&self) -> MemoryStats {
        self.stats
    }

    /// Record a typed slot so a later `typed_read` can return the exact Rc.
    pub fn typed_write(&mut self, addr: usize, offset: usize, value: crate::object::RcObject) {
        if addr != 0 {
            self.typed_slots.insert((addr, offset), value);
        }
    }

    /// Look up a previously-stored typed value, if any.
    pub fn typed_read(&self, addr: usize, offset: usize) -> Option<crate::object::RcObject> {
        self.typed_slots.get(&(addr, offset)).cloned()
    }
    
    /// Allocate memory and return address
    pub fn alloc(&mut self, size: usize) -> usize {
        self.alloc_at(size, 0)
    }

    /// Allocate, attributing the block to `site` (MEMORY_PROFILING M2's
    /// packed `(line << 32) | column`). `alloc` is this with an unknown
    /// site, kept so existing callers and tests read unchanged.
    pub fn alloc_at(&mut self, size: usize, site: u64) -> usize {
        let addr = self.alloc_uncounted_at(size, site);
        if addr != 0 {
            self.stats.alloc_count += 1;
            self.stats.record_obtained(size as u64);
            PROFILE.with(|p| {
                let mut g = p.get();
                g.alloc_count += 1;
                g.record_obtained(size as u64);
                p.set(g);
            });
            PROFILE_SITES.with(|m| {
                let mut m = m.borrow_mut();
                let e = m.entry(site).or_default();
                e.alloc_count += 1;
                e.cumulative_bytes += size as u64;
                e.live_count += 1;
                e.live_bytes += size as u64;
            });
        }
        addr
    }

    /// The allocation itself, without touching the counters.
    ///
    /// `realloc` services a move through this so one resize request
    /// stays one counted event. Note there is no free list and no
    /// reuse: `next_addr` only ever moves forward. That is the current
    /// behaviour, recorded rather than endorsed —
    /// `interpreter_heap_does_not_reuse_addresses` pins it.
    fn alloc_uncounted_at(&mut self, size: usize, site: u64) -> usize {
        if size == 0 {
            return 0; // null pointer for zero-size allocations
        }

        let addr = self.next_addr;
        self.memory.resize(self.memory.len() + size, 0);
        self.allocations.insert(addr, (size, site));
        self.next_addr += size;
        addr
    }

    /// Free memory at address
    pub fn free(&mut self, addr: usize) -> bool {
        if addr == 0 {
            return true; // freeing null pointer is a no-op
        }
        match self.free_uncounted(addr) {
            Some((size, site)) => {
                self.stats.free_count += 1;
                self.stats.record_released(size as u64);
                PROFILE.with(|p| {
                    let mut g = p.get();
                    g.free_count += 1;
                    g.record_released(size as u64);
                    p.set(g);
                });
                PROFILE_SITES.with(|m| {
                    let mut m = m.borrow_mut();
                    let e = m.entry(site).or_default();
                    e.live_count = e.live_count.saturating_sub(1);
                    e.live_bytes = e.live_bytes.saturating_sub(size as u64);
                });
                true
            }
            None => false,
        }
    }

    /// Drop the tracking entry, returning the size it held. Counter-free
    /// for the same reason as `alloc_uncounted`.
    fn free_uncounted(&mut self, addr: usize) -> Option<(usize, u64)> {
        self.allocations.remove(&addr)
    }
    
    /// Reallocate memory
    pub fn realloc(&mut self, addr: usize, new_size: usize) -> usize {
        if addr == 0 {
            // Reallocating null pointer is equivalent to alloc
            return self.alloc(new_size);
        }
        
        if new_size == 0 {
            // Reallocating to zero size is equivalent to free
            self.free(addr);
            return 0;
        }
        
        if let Some((old_size, site)) = self.allocations.get(&addr).copied() {
            // MEMORY_PROFILING M0: one resize request, counted once and
            // in terms of the size change the program asked for. The
            // move below is this implementation's way of servicing it —
            // an allocator that grew the block in place would have to
            // report the same numbers, so the internal calls are the
            // uncounted ones.
            self.stats.realloc_count += 1;
            if new_size > old_size {
                self.stats.record_obtained((new_size - old_size) as u64);
            } else {
                self.stats.record_released((old_size - new_size) as u64);
            }
            PROFILE.with(|p| {
                let mut g = p.get();
                g.realloc_count += 1;
                if new_size > old_size {
                    g.record_obtained((new_size - old_size) as u64);
                } else {
                    g.record_released((old_size - new_size) as u64);
                }
                p.set(g);
            });
            // MEMORY_PROFILING M2: a resize keeps the site its block
            // already had — it is the same logical allocation, so a leak
            // still points at where the memory came from rather than at
            // the last place it was grown.
            PROFILE_SITES.with(|m| {
                let mut m = m.borrow_mut();
                let e = m.entry(site).or_default();
                if new_size > old_size {
                    let grew = (new_size - old_size) as u64;
                    e.cumulative_bytes += grew;
                    e.live_bytes += grew;
                } else {
                    e.live_bytes = e.live_bytes.saturating_sub((old_size - new_size) as u64);
                }
            });
            // Allocate new memory
            let new_addr = self.alloc_uncounted_at(new_size, site);

            // Copy old data to new location
            let copy_size = old_size.min(new_size);
            // First get the source data to avoid borrowing conflicts
            if let Some(src) = self.get_memory_slice(addr, copy_size) {
                let temp_data: Vec<u8> = src.to_vec();
                if let Some(dest) = self.get_memory_slice_mut(new_addr, copy_size) {
                    dest.copy_from_slice(&temp_data);
                }
            }

            // Relocate typed slots from the old address to the new one so
            // values stashed under the previous base keep matching ptr_read
            // after realloc.
            let moved: Vec<(usize, crate::object::RcObject)> = self.typed_slots
                .iter()
                .filter_map(|((a, off), v)| {
                    if *a == addr && *off < copy_size {
                        Some((*off, v.clone()))
                    } else {
                        None
                    }
                })
                .collect();
            self.typed_slots.retain(|(a, _), _| *a != addr);
            for (off, v) in moved {
                self.typed_slots.insert((new_addr, off), v);
            }

            // Free old memory
            self.free_uncounted(addr);

            new_addr
        } else {
            0 // Invalid address
        }
    }
    
    /// Read u64 from memory at address + offset
    pub fn read_u64(&self, addr: usize, offset: usize) -> Option<u64> {
        if addr == 0 {
            return None; // null pointer access
        }
        
        let (size, _) = self.allocations.get(&addr)?;
        if offset + 8 > *size {
            return None; // out of bounds
        }
        
        let memory_offset = self.addr_to_memory_offset(addr)?;
        let slice = &self.memory[memory_offset + offset..memory_offset + offset + 8];
        Some(u64::from_le_bytes(slice.try_into().ok()?))
    }
    
    /// Read `width` (1/2/4/8) bytes little-endian from the raw byte buffer
    /// at `addr + offset`, zero-extended into a u64. Returns `None` on null
    /// or out-of-bounds. Used as a fallback when no typed slot exists (e.g.
    /// bytes written via `copy_memory` into a fresh destination buffer).
    pub fn read_scalar_bytes(&self, addr: usize, offset: usize, width: usize) -> Option<u64> {
        if addr == 0 || width == 0 {
            return None;
        }
        let slice = self.get_memory_slice(addr, offset + width)?;
        let bytes = &slice[offset..offset + width];
        let mut buf = [0u8; 8];
        buf[..width].copy_from_slice(bytes);
        Some(u64::from_le_bytes(buf))
    }

    /// Raw, base-agnostic byte read. Unlike `read_scalar_bytes`, `addr`
    /// need not be an allocation base — any interior address works because
    /// the byte buffer is contiguous and 1-based (`memory_offset = addr-1`).
    /// Used by the `str` runtime layout, whose value points at the trailing
    /// `u64 len` field (interior to its allocation).
    pub fn read_bytes_raw(&self, addr: usize, len: usize) -> Option<Vec<u8>> {
        if addr == 0 {
            return None;
        }
        let off = addr - 1;
        // Checked add: a misread length (e.g. interpreting a scalar as a str
        // handle) must not overflow-panic — fail the bounds check instead.
        match off.checked_add(len) {
            Some(end) if end <= self.memory.len() => Some(self.memory[off..end].to_vec()),
            _ => None,
        }
    }

    /// Raw, base-agnostic u64 read (little-endian) from an interior address.
    pub fn read_u64_raw(&self, addr: usize) -> Option<u64> {
        let bytes = self.read_bytes_raw(addr, 8)?;
        Some(u64::from_le_bytes(bytes.try_into().ok()?))
    }

    /// Raw, base-agnostic byte write. `addr` may be interior; the caller is
    /// responsible for the region being within an allocation it owns.
    pub fn write_bytes_raw(&mut self, addr: usize, bytes: &[u8]) -> bool {
        if addr == 0 {
            return false;
        }
        let off = addr - 1;
        if off + bytes.len() > self.memory.len() {
            return false;
        }
        self.memory[off..off + bytes.len()].copy_from_slice(bytes);
        true
    }

    /// Write u64 to memory at address + offset
    pub fn write_u64(&mut self, addr: usize, offset: usize, value: u64) -> bool {
        if addr == 0 {
            return false; // null pointer access
        }
        
        let (size, _) = match self.allocations.get(&addr) {
            Some(s) => *s,
            None => return false,
        };
        
        if offset + 8 > size {
            return false; // out of bounds
        }
        
        if let Some(memory_offset) = self.addr_to_memory_offset(addr) {
            let bytes = value.to_le_bytes();
            self.memory[memory_offset + offset..memory_offset + offset + 8]
                .copy_from_slice(&bytes);
            true
        } else {
            false
        }
    }
    
    /// Copy memory from src to dest. Walks both the raw byte buffer
    /// (the classic mem_copy semantic) **and** the typed_slots map
    /// (so values stashed by `__builtin_str_to_ptr` /
    /// `__builtin_ptr_write` survive a `mem_copy` into a fresh
    /// destination buffer). The typed copy is offset-relative —
    /// every entry under `(src_addr, off)` for `off < size` is
    /// re-keyed under `(dest_addr, off)` so per-byte / per-element
    /// reads at the destination see the same values as the source.
    pub fn copy_memory(&mut self, src_addr: usize, dest_addr: usize, size: usize) -> bool {
        if size == 0 {
            // Zero-byte copy is a no-op success — including when
            // either pointer is null. Matches the AOT path's call
            // into libc memcpy(3), which is also a no-op for n==0,
            // and unblocks the `Vec::from_str("")` /
            // `heap_alloc(0)` + `heap_realloc(p, 0)` chain in
            // `core/std/collections/vec.t::from_str`.
            return true;
        }
        if src_addr == 0 || dest_addr == 0 {
            return false; // null pointer access
        }

        let mut copied_any = false;

        // Raw byte buffer copy — covers AOT-style buffers built by
        // `heap_alloc` + raw `ptr_write`.
        if let Some(src_slice) = self.get_memory_slice(src_addr, size) {
            let temp_data: Vec<u8> = src_slice.to_vec();
            if let Some(dest_slice) = self.get_memory_slice_mut(dest_addr, size) {
                dest_slice.copy_from_slice(&temp_data);
                copied_any = true;
            }
        }

        // typed_slots range copy — covers buffers populated by
        // `__builtin_str_to_ptr` (writes one `Object::U8` per byte)
        // or by `__builtin_ptr_write(p, off, value)` for non-u64
        // typed values. Without this, the AOT-style `mem_copy(src,
        // dest, n)` over a `s.as_ptr()` source returns no data on
        // the interpreter (the bytes only live in typed_slots).
        let snapshot: Vec<(usize, crate::object::RcObject)> = self
            .typed_slots
            .iter()
            .filter_map(|((a, off), v)| {
                if *a == src_addr && *off < size {
                    Some((*off, v.clone()))
                } else {
                    None
                }
            })
            .collect();
        for (off, value) in snapshot {
            self.typed_slots.insert((dest_addr, off), value);
            copied_any = true;
        }

        copied_any
    }
    
    /// Move memory from src to dest (handles overlapping regions)
    pub fn move_memory(&mut self, src_addr: usize, dest_addr: usize, size: usize) -> bool {
        if size == 0 {
            return true; // no-op success — mirrors libc memmove(3)
        }
        if src_addr == 0 || dest_addr == 0 {
            return false; // null pointer access
        }
        
        // For simplicity, we'll copy the data to a temporary buffer first
        if let Some(src_slice) = self.get_memory_slice(src_addr, size) {
            let temp_data: Vec<u8> = src_slice.to_vec();
            if let Some(dest_slice) = self.get_memory_slice_mut(dest_addr, size) {
                dest_slice.copy_from_slice(&temp_data);
                return true;
            }
        }
        false
    }
    
    /// Set memory region to a specific byte value
    pub fn set_memory(&mut self, addr: usize, value: u8, size: usize) -> bool {
        if size == 0 {
            return true; // no-op success — mirrors libc memset(3)
        }
        if addr == 0 {
            return false; // null pointer access
        }
        
        if let Some(slice) = self.get_memory_slice_mut(addr, size) {
            slice.fill(value);
            true
        } else {
            false
        }
    }
    
    /// Check if address is valid
    pub fn is_valid_address(&self, addr: usize) -> bool {
        addr == 0 || self.allocations.contains_key(&addr)
    }
    
    // Helper methods
    
    fn addr_to_memory_offset(&self, addr: usize) -> Option<usize> {
        // Simple linear mapping for now
        // In a real implementation, this would be more complex
        if self.allocations.contains_key(&addr) {
            Some(addr - 1) // subtract 1 because addresses start at 1
        } else {
            None
        }
    }
    
    fn get_memory_slice(&self, addr: usize, size: usize) -> Option<&[u8]> {
        let (alloc_size, _) = self.allocations.get(&addr)?;
        if size > *alloc_size {
            return None;
        }
        
        let memory_offset = self.addr_to_memory_offset(addr)?;
        if memory_offset + size <= self.memory.len() {
            Some(&self.memory[memory_offset..memory_offset + size])
        } else {
            None
        }
    }
    
    fn get_memory_slice_mut(&mut self, addr: usize, size: usize) -> Option<&mut [u8]> {
        let (alloc_size, _) = self.allocations.get(&addr).copied()?;
        if size > alloc_size {
            return None;
        }
        
        let memory_offset = self.addr_to_memory_offset(addr)?;
        if memory_offset + size <= self.memory.len() {
            Some(&mut self.memory[memory_offset..memory_offset + size])
        } else {
            None
        }
    }
}

impl Default for HeapManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heap_alloc_free() {
        let mut heap = HeapManager::new();
        
        // Allocate memory
        let addr = heap.alloc(64);
        assert_ne!(addr, 0);
        assert!(heap.is_valid_address(addr));
        
        // Free memory
        assert!(heap.free(addr));
        
        // Free null pointer should succeed
        assert!(heap.free(0));
    }
    
    #[test]
    fn test_heap_read_write() {
        let mut heap = HeapManager::new();
        
        let addr = heap.alloc(64);
        assert_ne!(addr, 0);
        
        // Write and read u64
        assert!(heap.write_u64(addr, 0, 0x1234567890abcdef));
        assert_eq!(heap.read_u64(addr, 0), Some(0x1234567890abcdef));
        
        // Out of bounds access should fail
        assert_eq!(heap.read_u64(addr, 64), None);
        assert!(!heap.write_u64(addr, 64, 0));
        
        // Null pointer access should fail
        assert_eq!(heap.read_u64(0, 0), None);
        assert!(!heap.write_u64(0, 0, 0));
    }
    
    #[test]
    fn test_heap_copy_move_set() {
        let mut heap = HeapManager::new();
        
        let src_addr = heap.alloc(64);
        let dest_addr = heap.alloc(64);
        
        // Write some data to source
        assert!(heap.write_u64(src_addr, 0, 0x1111111111111111));
        assert!(heap.write_u64(src_addr, 8, 0x2222222222222222));
        
        // Copy memory
        assert!(heap.copy_memory(src_addr, dest_addr, 16));
        assert_eq!(heap.read_u64(dest_addr, 0), Some(0x1111111111111111));
        assert_eq!(heap.read_u64(dest_addr, 8), Some(0x2222222222222222));
        
        // Set memory
        assert!(heap.set_memory(dest_addr, 0xff, 16));
        assert_eq!(heap.read_u64(dest_addr, 0), Some(0xffffffffffffffff));
        assert_eq!(heap.read_u64(dest_addr, 8), Some(0xffffffffffffffff));
    }

    #[test]
    fn test_global_allocator_delegates_to_heap_manager() {
        let heap = Rc::new(RefCell::new(HeapManager::new()));
        let allocator = GlobalAllocator::new(heap.clone());

        let addr = allocator.alloc(32);
        assert_ne!(addr, 0);
        assert!(heap.borrow().is_valid_address(addr));

        assert!(allocator.free(addr));
        assert!(!heap.borrow().is_valid_address(addr));
    }

    // Arena / FixedBuffer runtime tests removed when the runtime
    // arena/fixed_buffer types were retired. Equivalent contracts
    // are now covered end-to-end by the consistency suite against
    // the toylang stdlib `Arena` / `FixedBuffer` (`compiler/tests/
    // consistency.rs::aot_arena_bytes_used_and_reset` etc.).
}
#[cfg(test)]
mod memory_stats_tests {
    use super::*;

    /// The counters are defined on what the program asked for, so they
    /// are checked against hand-computed numbers rather than against
    /// whatever the implementation happened to do.
    #[test]
    fn alloc_and_free_account_exactly() {
        let mut heap = HeapManager::new();
        let a = heap.alloc(64);
        let b = heap.alloc(32);
        assert_eq!(heap.stats().live_bytes, 96);
        assert_eq!(heap.stats().peak_live_bytes, 96);

        heap.free(a);
        let s = heap.stats();
        assert_eq!(s.alloc_count, 2);
        assert_eq!(s.free_count, 1);
        assert_eq!(s.live_bytes, 32);
        assert_eq!(s.cumulative_bytes, 96, "cumulative never decreases");
        assert_eq!(s.peak_live_bytes, 96, "peak survives the free");

        heap.free(b);
        assert_eq!(heap.stats().live_bytes, 0);
    }

    #[test]
    fn realloc_is_one_request_not_an_alloc_plus_a_free() {
        // This implementation services a growing realloc by moving the
        // block. An allocator that grew it in place has to report the
        // same numbers, so the counts must not leak the strategy.
        let mut heap = HeapManager::new();
        let p = heap.alloc(16);
        heap.realloc(p, 64);

        let s = heap.stats();
        assert_eq!(s.alloc_count, 1, "the move must not count as an allocation");
        assert_eq!(s.free_count, 0, "the move must not count as a free");
        assert_eq!(s.realloc_count, 1);
        assert_eq!(s.live_bytes, 64);
        assert_eq!(s.cumulative_bytes, 64, "16 obtained, then 48 more");
    }

    #[test]
    fn shrinking_realloc_lowers_live_but_not_cumulative() {
        let mut heap = HeapManager::new();
        let p = heap.alloc(64);
        heap.realloc(p, 16);

        let s = heap.stats();
        assert_eq!(s.live_bytes, 16);
        assert_eq!(s.cumulative_bytes, 64, "shrinking obtains nothing");
        assert_eq!(s.peak_live_bytes, 64);
    }

    #[test]
    fn peak_records_the_request_it_happened_at() {
        let mut heap = HeapManager::new();
        heap.alloc(10); // request 1
        let big = heap.alloc(100); // request 2 — peak here, 110 live
        heap.free(big);
        heap.alloc(5); // request 3

        let s = heap.stats();
        assert_eq!(s.peak_live_bytes, 110);
        assert_eq!(s.peak_at_request, 2);
        assert_eq!(s.live_bytes, 15);
    }

    #[test]
    fn zero_size_and_null_are_not_counted() {
        let mut heap = HeapManager::new();
        assert_eq!(heap.alloc(0), 0, "zero-size allocation yields the null pointer");
        heap.free(0);
        let s = heap.stats();
        assert_eq!(s.alloc_count, 0);
        assert_eq!(s.free_count, 0);
        assert_eq!(s.live_bytes, 0);
    }

    #[test]
    fn a_double_free_cannot_drive_live_bytes_negative() {
        // Saturating accounting: a misbehaving program produces wrong
        // numbers, not nonsensical ones.
        let mut heap = HeapManager::new();
        let p = heap.alloc(32);
        heap.free(p);
        heap.free(p);
        let s = heap.stats();
        assert_eq!(s.live_bytes, 0);
        assert_eq!(s.free_count, 1, "the second free tracked nothing");
    }
}
