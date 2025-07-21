extern crate rflex;
use std::env;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR environment variable not set");
    let dest = Path::new(&out_dir).join("lexer.rs");
    let path = Path::new("src").join("lexer.l");
    if let Err(e) = rflex::process(path, Some(dest)) {
        for cause in <dyn failure::Fail>::iter_chain(&e) {
            eprintln!("{}: {}", cause.name().unwrap_or("Error"), cause);
        }
        std::process::exit(1);
    }
}
