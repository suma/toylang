# DATA-ORIENTED Phase 2 — `soa Vec<T>`, the heap sibling of
# `soa [T; N]` (see `soa.t` for the stack form).
#
# `soa Vec<Particle>` is sugar for the stdlib `SoaVec<Particle>`: one
# allocation split into one column per field, so every `mass` sits
# next to every other `mass` instead of being 16 bytes apart. The call
# surface is `Vec`'s, so switching a program between the two layouts
# is the annotation plus the constructor's name — nothing in the loops
# below changes.

struct Particle {
    x: i64,
    y: i64,
    mass: i64,
}

fn main() -> i64 {
    var ps: soa Vec<Particle> = SoaVec::new()

    # Grows 0 -> 4 -> 8: a grow cannot resize in place here (every
    # column but the first moves), so the elements are re-placed one
    # at a time at the new capacity's offsets.
    var i: i64 = 0i64
    while i < 6i64 {
        ps.push(Particle { x: i, y: i * 2i64, mass: 10i64 + i })
        i = i + 1i64
    }

    println("size={ps.size()} capacity={ps.capacity()}")

    # The DoD loop: one field over every element.
    var total_mass: i64 = 0i64
    for p in ps.iter() {
        total_mass = total_mass + p.mass
    }
    println("total mass = {total_mass}")

    # Random access reads the whole element back out of its columns.
    val third: Particle = ps.get(2u64)
    println("ps[2] = ({third.x}, {third.y}) mass {third.mass}")

    ps.set(0u64, Particle { x: 100i64, y: 200i64, mass: 1i64 })
    val first: Particle = ps.get(0u64)
    println("after set, ps[0].x = {first.x}")

    val popped: Particle = ps.pop()
    println("popped mass {popped.mass}, {ps.size()} left")

    total_mass + first.x + popped.mass
}
