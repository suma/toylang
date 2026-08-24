/*
 * NEWTYPE: tuple structs (`struct Meters(i64)`).
 *
 * A positional struct wraps a value without inventing a field name, so
 * a unit or an id gets its own type instead of riding along as a bare
 * i64. The compiler treats it as an ordinary struct whose fields are
 * named by position, which is why `impl` blocks, `&self` methods,
 * generics and printing all behave exactly as they do for a struct
 * with named fields.
 */

# A unit type. `Meters` and `Seconds` are distinct types, so a function
# that wants one cannot silently be handed the other.
struct Meters(i64)
struct Seconds(i64)

# Several fields are positional too, reached as `.0` / `.1`.
struct Sample(i64, str)

# Generic, like any other struct.
struct Wrap<T>(T)

impl Meters {
    fn scale(&self, k: i64) -> Meters {
        Meters(self.0 * k)
    }
}

fn speed(distance: Meters, elapsed: Seconds) -> i64 {
    distance.0 / elapsed.0
}

fn main() -> i64 {
    val distance = Meters(120i64)
    val elapsed = Seconds(4i64)
    println(speed(distance, elapsed))

    # `.0` reads the wrapped value; the struct itself prints in the
    # form it was written.
    println(distance.0)
    println(distance)

    val doubled = distance.scale(2i64)
    println(doubled)

    val s = Sample(7i64, "seven")
    println(s.0)
    println(s.1)

    # A generic tuple struct needs the same annotation a generic
    # named struct does when the backends have to pick an instance.
    val w: Wrap<i64> = Wrap(9i64)
    println(w.0)

    # Positional patterns destructure it. `..` ignores the rest, just
    # as it does in `Point { x, .. }`.
    val label = match s {
        Sample(0i64, name) => name,
        Sample(_, name) => name,
    }
    println(label)
    val head = match s { Sample(n, ..) => n }
    println(head)

    speed(distance, elapsed) + doubled.0 + w.0 + head
}
