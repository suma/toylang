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
    let options = match parse_args(&args) {
        Ok(o) => o,
        Err(msg) => {
            eprintln!("{msg}");
            print_usage();
            return ExitCode::from(2);
        }
    };

    match compile_file(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("compile error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: &[String]) -> Result<CompilerOptions, String> {
    if args.is_empty() {
        return Err("no input file".to_string());
    }
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut emit = EmitKind::Executable;
    let mut verbose = false;
    let mut release = false;
    let mut core_modules_dir: Option<PathBuf> = None;
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
            s if s.starts_with("--core-modules=") => {
                core_modules_dir = Some(PathBuf::from(&s["--core-modules=".len()..]));
            }
            s if s.starts_with('-') => {
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
    Ok(CompilerOptions {
        input,
        output,
        emit,
        verbose,
        release,
        core_modules_dir,
    })
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
        "usage: compiler <input.t> [-o <output>] [--emit exe|obj|ir|clif] [--release] [-v]"
    );
}
