/*
 * DATA-ORIENTED Phase 0: `soa [T; N]` — the prefix layout modifier.
 *
 * `soa` chooses the *placement* of a fixed-size array, not its type:
 * the columns (x | y | mass) are stored contiguously instead of the
 * elements. Layout is not type identity — the checker treats
 * `soa [Particle; N]` and `[Particle; N]` as the same type, so the
 * modifier can be added or removed to measure, with no other edit.
 *
 * The payoff shape is the field loop: `ps[i].mass` lowers to one
 * load from the mass column, so a loop over one field touches one
 * column instead of every leaf of every element. Writes
 * (`ps[i].x = ...`), whole-element reads (`val p = ps[i]`) and range
 * slices work the same as the AoS spelling.
 */

struct Particle { x: f64, y: f64, mass: f64 }

fn main() -> u64 {
    val ps: soa [Particle; 4] = [
        Particle { x: 0.0f64, y: 0.0f64, mass: 1.0f64 },
        Particle { x: 1.0f64, y: 2.0f64, mass: 2.0f64 },
        Particle { x: 2.0f64, y: 4.0f64, mass: 3.0f64 },
        Particle { x: 3.0f64, y: 6.0f64, mass: 4.0f64 },
    ]

    # The DoD loop: total mass reads only the mass column.
    var total: f64 = 0.0f64
    for i in 0i64..4i64 {
        total = total + ps[i].mass
    }
    println(total)                          # 10.0

    # Single-column writes: advance every particle by its mass.
    for i in 0i64..4i64 {
        ps[i].y = ps[i].y + ps[i].mass
    }

    # Whole-element read: works exactly like the AoS spelling.
    val p = ps[3i64]
    println(p.y)                            # 10.0

    # Range slice: no annotation keeps the source layout.
    val heavy = ps[2i64..4i64]
    var heavy_mass: f64 = 0.0f64
    for i in 0i64..2i64 {
        heavy_mass = heavy_mass + heavy[i].mass
    }
    println(heavy_mass)                     # 7.0

    # An exit code under 256 so every backend can report it alike.
    total as u64 + p.y as u64 + heavy_mass as u64
}
