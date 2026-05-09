# `if val` / `while val` demo (IF-VAL feature).
#
# Pattern-binding conditional. Toylang uses `val` (not `let`) as its
# immutable-binding keyword, so the construct stays consistent with
# the rest of the language.

fn classify(opt: Option<i64>) -> str {
    if val Option::Some(x) = opt {
        if x > 0i64 {
            "positive"
        } elif x < 0i64 {
            "negative"
        } else {
            "zero"
        }
    } else {
        "none"
    }
}

fn main() -> i64 {
    println(classify(Option::Some(42i64)))
    println(classify(Option::Some(-1i64)))
    println(classify(Option::Some(0i64)))
    println(classify(Option::None))
    0i64
}
