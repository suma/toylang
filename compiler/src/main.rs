//! CLI front-end for the AOT compiler.
//!
//! Usage:
//!   compiler <input.t> [-o <output>] [--emit ir|obj|exe] [-v]
//!
//! Default `--emit` is `exe`. `--emit=ir` writes Cranelift IR text;
//! `--emit=obj` writes the unlinked object file. The `-o` flag is the
//! path of the produced artefact regardless of `--emit`.

use std::path::PathBuf;
use std::process::ExitCode;

use compiler::{compile_file, CompilerOptions, EmitKind};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut options, all_backends) = match parse_args(&args) {
        Ok(o) => o,
        Err(msg) => {
            eprintln!("{msg}");
            print_usage();
            return ExitCode::from(2);
        }
    };

    // D6: `-` reads the program from stdin. Resolved here rather than
    // inside the driver because the AOT step needs a real path, so a
    // stdin program has to be spilled to one, and the spill's lifetime
    // must cover the whole invocation.
    let display_name = options.input.display().to_string();
    let (source, input_path, _spill) = match compiler::read_input(&options.input) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Failed to read {display_name}: {e}");
            return ExitCode::FAILURE;
        }
    };
    options.input = input_path;
    let display_name = if display_name == "-" { "<stdin>".to_string() } else { display_name };

    if all_backends {
        return ExitCode::from(
            u8::try_from(compiler::all_backends::run(&options, &source, &display_name))
                .unwrap_or(1),
        );
    }

    match compile_file(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("compile error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Returns the build options plus whether `--all-backends` was asked
/// for. It is not a `CompilerOptions` field because it selects a
/// different action entirely (run everywhere and compare) rather than
/// configuring the build.
fn parse_args(args: &[String]) -> Result<(CompilerOptions, bool), String> {
    if args.is_empty() {
        return Err("no input file".to_string());
    }
    let mut all_backends = false;
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut emit = EmitKind::Executable;
    let mut verbose = false;
    let mut release = false;
    let mut core_modules_dir: Option<PathBuf> = None;
    let mut diagnostics_json = false;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "-v" | "--verbose" => verbose = true,
            "--release" => release = true,
            "--all-backends" => all_backends = true,
            "-o" => {
                i += 1;
                let v = args.get(i).ok_or_else(|| "-o needs an argument".to_string())?;
                output = Some(PathBuf::from(v));
            }
            s if s.starts_with("--emit=") => {
                emit = parse_emit(&s["--emit=".len()..])?;
            }
            "--emit" => {
                i += 1;
                let v = args.get(i).ok_or_else(|| "--emit needs an argument".to_string())?;
                emit = parse_emit(v)?;
            }
            "--core-modules" => {
                i += 1;
                let v = args
                    .get(i)
                    .ok_or_else(|| "--core-modules needs a path argument".to_string())?;
                core_modules_dir = Some(PathBuf::from(v));
            }
            s if s.starts_with("--diagnostics=") => {
                match &s["--diagnostics=".len()..] {
                    "json" => diagnostics_json = true,
                    "text" => diagnostics_json = false,
                    other => return Err(format!("--diagnostics expects `text` or `json`, got `{other}`")),
                }
            }
            s if s.starts_with("--core-modules=") => {
                core_modules_dir = Some(PathBuf::from(&s["--core-modules=".len()..]));
            }
            // A bare `-` is the input, not a flag (D6: read stdin).
            s if s.starts_with('-') && s != "-" => {
                return Err(format!("unknown flag: {s}"));
            }
            _ => {
                if input.is_some() {
                    return Err(format!("more than one input file: {a}"));
                }
                input = Some(PathBuf::from(a));
            }
        }
        i += 1;
    }
    let input = input.ok_or_else(|| "no input file".to_string())?;
    let mut options = CompilerOptions::new(input);
    options.output = output;
    options.emit = emit;
    options.verbose = verbose;
    options.release = release;
    options.core_modules_dir = core_modules_dir;
    options.diagnostics_json = diagnostics_json;
    Ok((options, all_backends))
}

fn parse_emit(s: &str) -> Result<EmitKind, String> {
    match s {
        "exe" | "executable" => Ok(EmitKind::Executable),
        "obj" | "object" => Ok(EmitKind::Object),
        "ir" => Ok(EmitKind::Ir),
        "clif" => Ok(EmitKind::Clif),
        other => Err(format!("unknown --emit kind: {other}")),
    }
}

fn print_usage() {
    eprintln!(
        "usage: compiler <input.t> [-o <output>] [--emit exe|obj|ir|clif] [--release] [--diagnostics=text|json] [-v]"
    );
    eprintln!(
        "       compiler <input.t> --all-backends   # run on interpreter / JIT / AOT, report disagreements"
    );
    eprintln!("       use `-` as <input.t> to read the program from stdin");
}
