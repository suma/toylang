trait Greet {
    fn name(self: Self) -> str
    fn greet(self: Self) -> str {
        self.name()
    }
}

struct Dog {
    label: str,
}

impl Greet for Dog {
    fn name(self: Self) -> str {
        self.label
    }
}

fn main() -> str {
    val d = Dog { label: "Rex" }
    val msg = d.greet()
    println(msg)
    msg
}
