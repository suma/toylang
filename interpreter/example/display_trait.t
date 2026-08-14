/*
 * `Display` — letting a type decide how it looks.
 *
 * A `to_str(&self) -> str` method controls what `print` / `println`
 * write and what string interpolation splices in. Without one, a
 * struct or enum renders structurally, which is useful while debugging
 * and rarely what belongs in output a person reads.
 *
 *   cargo run -q -p interpreter -- interpreter/example/display_trait.t
 *
 * Kept to ASCII on purpose: a non-ASCII source literal is currently
 * mangled on the way through the lexer (a 3-byte `\u{2660}` measures 6),
 * which is a separate bug and not something this example should be
 * demonstrating.
 */

struct Point { x: i64, y: i64 }

impl Display for Point {
    fn to_str(&self) -> str { "({self.x}, {self.y})" }
}

enum Suit { Hearts, Spades }

impl Display for Suit {
    fn to_str(&self) -> str {
        match self {
            Suit::Hearts => "hearts",
            Suit::Spades => "spades",
        }
    }
}

# No `impl Display` — renders structurally, as before.
struct Raw { a: i64, b: bool }

# Dispatch is on the method, not on a registered `impl Display for`,
# the same way `==` finds `eq`. An inherent one works too.
struct Temperature { celsius: i64 }

impl Temperature {
    fn to_str(&self) -> str { "{self.celsius}C" }
}

fn main() -> u64 {
    val p = Point { x: 3i64, y: -4i64 }
    println(p)
    println("origin to {p}")

    val s = Suit::Spades
    println("dealt {s}")

    val r = Raw { a: 1i64, b: true }
    println(r)

    val t = Temperature { celsius: 21i64 }
    println("it is {t} inside")

    # `String` renders as its text. Before `impl Display for String`
    # this printed the struct's fields, pointer and all.
    val name = String::from_str("ada")
    println("hello, {name}")

    0u64
}
