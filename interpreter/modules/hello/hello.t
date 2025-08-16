package hello

pub fn add(a: u64, b: u64) -> u64 {
    a + b
}

pub fn multiply(a: u64, b: u64) -> u64 {
    a * b
}

fn private_helper() -> u64 {
    42u64
}

pub fn get_magic_number() -> u64 {
    private_helper()
}