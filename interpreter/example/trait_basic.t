trait Greet {
    fn greet(self: Self) -> str
}

struct Dog {
    name: str
}

struct Cat {
    name: str
}

impl Greet for Dog {
    fn greet(self: Self) -> str {
        "Woof!"
    }
}

impl Greet for Cat {
    fn greet(self: Self) -> str {
        "Meow!"
    }
}

fn announce<T: Greet>(animal: T) -> str {
    animal.greet()
}

fn main() -> u64 {
    val d = Dog { name: "Rex" }
    val c = Cat { name: "Whiskers" }
    println(announce(d))
    println(announce(c))
    0u64
}
