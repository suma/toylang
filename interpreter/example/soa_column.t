# DATA-ORIENTED Phase 1 — column windows.
#
# Phase 0 made "loop over one field" cheap; a window makes that field
# *passable*. `ps.mass` is a `Column<f64>` (`core/std/column.t`): the
# address the masses start at, how many there are, and how far apart
# they sit. The stride is why one type covers both layouts — the
# values are adjacent under `soa` and one element apart without it —
# so `total_mass` below is written once and measured twice.

struct Particle {
    x: f64,
    y: f64,
    mass: f64,
}

# The function that could not be written before Phase 1: it reads the
# masses and nothing else, whatever the array's layout is.
fn total_mass(ms: Column<f64>) -> f64 {
    var total: f64 = 0.0f64
    var i: u64 = 0u64
    while i < ms.len() {
        total = total + ms.get(i)
        i = i + 1u64
    }
    total
}

fn main() -> i64 {
    val soa_ps: soa [Particle; 4] = [
        Particle { x: 0.0f64, y: 0.0f64, mass: 1.0f64 },
        Particle { x: 1.0f64, y: 2.0f64, mass: 2.0f64 },
        Particle { x: 3.0f64, y: 4.0f64, mass: 4.0f64 },
        Particle { x: 5.0f64, y: 6.0f64, mass: 8.0f64 },
    ]
    val aos_ps: [Particle; 4] = [
        Particle { x: 0.0f64, y: 0.0f64, mass: 1.0f64 },
        Particle { x: 1.0f64, y: 2.0f64, mass: 2.0f64 },
        Particle { x: 3.0f64, y: 4.0f64, mass: 4.0f64 },
        Particle { x: 5.0f64, y: 6.0f64, mass: 8.0f64 },
    ]

    # Same call, same answer, different placement behind it.
    val soa_ms = soa_ps.mass
    val aos_ms = aos_ps.mass
    println("soa total = {total_mass(soa_ms)}")
    println("aos total = {total_mass(aos_ms)}")

    # A window is a view: writing through it changes the array.
    var ps: soa [Particle; 3] = [
        Particle { x: 0.0f64, y: 0.0f64, mass: 1.0f64 },
        Particle { x: 1.0f64, y: 1.0f64, mass: 1.0f64 },
        Particle { x: 2.0f64, y: 2.0f64, mass: 1.0f64 },
    ]
    var ms = ps.mass
    ms.set(1u64, 10.0f64)
    println("ps[1].mass is now {ps[1u64].mass}")

    # The heap form windows a `soa Vec<T>`'s column the same way, over
    # the live elements rather than the capacity.
    var vs: soa Vec<Particle> = SoaVec::new()
    vs.push(Particle { x: 0.0f64, y: 0.0f64, mass: 3.0f64 })
    vs.push(Particle { x: 1.0f64, y: 1.0f64, mass: 5.0f64 })
    val heap_ms = vs.mass
    println("vec masses: {heap_ms.len()} of them, totalling {total_mass(heap_ms)}")

    0i64
}
