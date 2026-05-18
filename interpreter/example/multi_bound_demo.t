trait Greet {
    fn greet(self: Self) -> str
}
trait Named {
    fn name(self: Self) -> str
}

struct Dog {
    label: str,
}

impl Greet for Dog {
    fn greet(self: Self) -> str { "Woof!" }
}

impl Named for Dog {
    fn name(self: Self) -> str { self.label }
}

fn describe<T: Greet + Named>(x: T) -> str {
    val g = x.greet()
    val n = x.name()
    n
}

fn main() -> str {
    val d = Dog { label: "Rex" }
    val s = describe(d)
    println(s)
    s
}
