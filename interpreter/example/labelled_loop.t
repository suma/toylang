# Labelled loop demo (LABEL feature).
#
# `@label:` decorates a loop with a name; `break @label` /
# `continue @label` then targets that loop directly. Useful for
# escaping nested loops without flag variables.

fn search() -> i64 {
    @outer: for i in 0i64 to 10i64 {
        for j in 0i64 to 10i64 {
            if i * j == 42i64 {
                return i * 100i64 + j
            }
            if j > 5i64 { continue @outer }
        }
    }
    -1i64
}

fn main() -> i64 {
    val result: i64 = search()
    println("found = {result}")
    result
}
