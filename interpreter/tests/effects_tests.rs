// EFFECTS — the effect set behind `never_allocates`, `const fn` and
// contract purity, and the `--effects` listing that reports it.
//
// The three checks have their own test files; what these pin is the
// shared answer they are all masks over:
//
//   * a function that only computes is `pure`, and one that reaches a
//     builtin carries that builtin's effect
//   * effects travel along the call graph, so a caller inherits its
//     callee's without any annotation
//   * an unfollowable call (a closure value, an `extern fn`) is
//     assumed to do everything — the one direction that is safe
//   * a `never_allocates extern` takes back exactly one effect
//   * the receiver's type decides whether `concat` allocates, which
//     is why interpolation is free of `alloc`

use crate::common::core_modules_dir;
use frontend::type_checker::{Effect, EffectSet};

/// The effect set of each declaration in `source`, by name.
fn effects(source: &str) -> Vec<(String, EffectSet)> {
    let core = core_modules_dir();
    let mut options = interpreter::RunOptions::default();
    options.core_modules_dir = Some(core.as_path());
    interpreter::effects_from_source(source, "test.t", &options)
        .expect("type check")
        .into_iter()
        .map(|f| (f.name, f.effects))
        .collect()
}

/// The effect set of one declaration.
fn effects_of(source: &str, name: &str) -> EffectSet {
    effects(source)
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no declaration named {name}"))
        .1
}

#[test]
fn a_function_that_only_computes_is_pure() {
    let source = r#"
fn add(a: u64, b: u64) -> u64 { a + b }
fn main() -> u64 { add(1u64, 2u64) }
"#;
    let set = effects_of(source, "add");
    assert!(set.is_empty(), "expected pure, got {set}");
    assert_eq!(set.to_string(), "pure");
}

#[test]
fn a_builtin_lends_its_effect_to_every_caller() {
    // No annotation anywhere: the effect is read off the call graph,
    // which is the property that keeps the stdlib un-annotated.
    let source = r#"
fn inner(n: u64) -> ptr { __builtin_heap_alloc(n) }
fn middle(n: u64) -> ptr { inner(n) }
fn main() -> u64 {
    val p = middle(8u64)
    0u64
}
"#;
    for name in ["inner", "middle", "main"] {
        assert!(
            effects_of(source, name).contains(Effect::Alloc),
            "{name} should be allocating"
        );
    }
}

#[test]
fn effects_are_reported_separately() {
    let source = r#"
fn noisy(n: u64) -> u64 {
    println("n")
    n
}
fn main() -> u64 { noisy(1u64) }
"#;
    let set = effects_of(source, "noisy");
    assert!(set.contains(Effect::Io), "{set}");
    assert!(!set.contains(Effect::Alloc), "{set}");
}

#[test]
fn panic_is_an_effect_of_its_own() {
    // `const fn` allows it on purpose, so it must be a bit the mask
    // can leave out rather than an absence.
    let source = r#"
fn risky(n: u64) -> u64 {
    if n == 0u64 { panic("zero") } else { n }
}
fn main() -> u64 { risky(1u64) }
"#;
    let set = effects_of(source, "risky");
    assert!(set.contains(Effect::Panic), "{set}");
    assert!(!set.contains(Effect::Io), "{set}");
}

#[test]
fn a_recursive_function_terminates_the_walk() {
    let source = r#"
fn countdown(n: u64) -> u64 {
    if n == 0u64 { 0u64 } else { countdown(n - 1u64) }
}
fn main() -> u64 { countdown(3u64) }
"#;
    assert!(effects_of(source, "countdown").is_empty());
}

#[test]
fn an_unfollowable_call_is_assumed_to_do_everything() {
    // Calling a closure value lands somewhere the walk cannot see.
    let source = r#"
fn apply(n: u64) -> u64 {
    val f = fn(x: u64) -> u64 { x + 1u64 }
    f(n)
}
fn main() -> u64 { apply(1u64) }
"#;
    let set = effects_of(source, "apply");
    for effect in Effect::ALL {
        assert!(set.contains(effect), "expected {}, got {set}", effect.name());
    }
}

#[test]
fn an_extern_declaration_can_take_back_one_effect() {
    let source = r#"
extern fn plain_getchar() -> i32 from "c"
never_allocates extern fn quiet_getchar() -> i32 from "c"

fn loud(n: u64) -> u64 {
    val c = plain_getchar()
    n
}
fn quiet(n: u64) -> u64 {
    val c = quiet_getchar()
    n
}
fn main() -> u64 { 0u64 }
"#;
    let loud = effects_of(source, "loud");
    let quiet = effects_of(source, "quiet");
    assert!(loud.contains(Effect::Alloc), "{loud}");
    // The declaration is a promise about allocation and nothing else,
    // so everything the walk cannot see is still assumed.
    assert!(!quiet.contains(Effect::Alloc), "{quiet}");
    assert!(quiet.contains(Effect::Io), "{quiet}");
}

#[test]
fn interpolation_does_not_allocate_but_a_string_concat_does() {
    // Both desugar to `concat`; the receiver's type is what tells the
    // runtime's `str` implementation from the stdlib's `String`.
    let source = r#"
fn interpolate(n: u64) -> u64 {
    println("n = {n}")
    n
}
fn build(n: u64) -> u64 {
    var s: String = String::from_str("n = ")
    s.push_char('x')
    n
}
fn main() -> u64 { interpolate(1u64) + build(2u64) }
"#;
    assert!(!effects_of(source, "interpolate").contains(Effect::Alloc));
    assert!(effects_of(source, "build").contains(Effect::Alloc));
}

#[test]
fn the_listing_covers_the_entry_file_only() {
    // The stdlib is integrated into the same pools; a listing of it
    // would bury the file the reader asked about.
    let source = r#"
struct Counter { value: u64 }
impl Counter {
    fn bump(&mut self) -> u64 {
        self.value = self.value + 1u64
        self.value
    }
}
fn main() -> u64 {
    var c = Counter { value: 0u64 }
    c.bump()
}
"#;
    let names: Vec<String> = effects(source).into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, vec!["main".to_string(), "Counter::bump".to_string()]);
}
