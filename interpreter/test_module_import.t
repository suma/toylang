package main
import hello

fn main() -> u64 {
    val a = hello.add(10u64, 20u64)
    val b = hello.multiply(3u64, 4u64)
    val c = hello.get_magic_number()
    a + b + c
}