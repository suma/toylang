# Top-level `const` declarations. Each `const NAME: Type = expression`
# runs once at program startup and is bound as an immutable global. Any
# function body — including `main` — can read it. Initializers may
# reference earlier consts, but forward references are not allowed.

const PI: f64 = 3.14159f64
const TWO_PI: f64 = PI + PI               # references the prior const
const MAX_RETRIES: u64 = 3u64
const GREETING: str = "hello"

fn area(radius: f64) -> f64 {
    PI * radius * radius
}

fn main() -> u64 {
    println(GREETING)
    println(area(5.0f64))                 # 78.53975
    println(TWO_PI)                       # 6.28318
    MAX_RETRIES
}
