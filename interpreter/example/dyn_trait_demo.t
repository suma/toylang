trait Animal {
    fn sound(self: Self) -> str
}

struct Dog {}
struct Cat {}

impl Animal for Dog {
    fn sound(self: Self) -> str { "Woof" }
}

impl Animal for Cat {
    fn sound(self: Self) -> str { "Meow" }
}

fn describe(a: &dyn Animal) -> str {
    a.sound()
}

fn main() -> u64 {
    val d = Dog {}
    val c = Cat {}
    val s1 = describe(d)
    val s2 = describe(c)
    println(s1)
    println(s2)
    0u64
}
