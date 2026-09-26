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

use compiler::all_backends::ProfileMode;
use compiler::{compile_file, CompilerOptions, EmitKind};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mut options, Mode { all_backends, profile, compile_profile, json, heap_check }) = match parse_args(&args) {
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
    // The compiled lanes name the entry after this too, not after the
    // spill file a pipe was written to.
    options.display_name = Some(display_name.clone());

    if all_backends {
        return ExitCode::from(
            u8::try_from(compiler::all_backends::run_with(
                &options,
                &source,
                &display_name,
                profile,
                heap_check,
                json,
            ))
            .unwrap_or(1),
        );
    }

    // COMPILE-PROFILE: time is measured from here, so reading the input
    // above (done again inside `compile_file`) is not counted twice.
    if compile_profile {
        frontend::compile_profile::enable();
    }
    let result = compile_file(&options);
    if let Some(recorded) = frontend::compile_profile::finish() {
        // A failed compile still reports: which phase it got to, and
        // how long that took, is part of the answer.
        eprint!("{}", compiler::compile_profile::render(&recorded, &options, json));
    }
    match result {
        Ok(()) => {
            if json {
                let report = serde_json::json!({
                    "input": display_name,
                    "emit": emit_name(options.emit),
                    "output": compiler::output_path(&options).display().to_string(),
                });
                println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("compile error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// What the invocation does besides configuring the build. Not
/// `CompilerOptions` fields: `--all-backends` selects a different action
/// entirely (run everywhere and compare), and the report shape is the
/// CLI's business, not the library's.
struct Mode {
    all_backends: bool,
    profile: ProfileMode,
    /// `--profile=compile`: time the compile's phases (COMPILE-PROFILE).
    compile_profile: bool,
    /// `--format=json`: the result (what was built, or the backends'
    /// verdict) as one JSON document on stdout. The same flag also
    /// shapes the diagnostics and the memory report.
    json: bool,
    /// HEAP-CHECK H0: `--heap-check=report` under `--all-backends`.
    heap_check: bool,
}

fn emit_name(emit: EmitKind) -> &'static str {
    match emit {
        EmitKind::Executable => "exe",
        EmitKind::Object => "obj",
        EmitKind::Ir => "ir",
        EmitKind::Clif => "clif",
    }
}

fn parse_args(args: &[String]) -> Result<(CompilerOptions, Mode), String> {
    if args.is_empty() {
        return Err("no input file".to_string());
    }
    let mut all_backends = false;
    let mut profile_mem = false;
    let mut compile_profile = false;
    let mut input: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut emit = EmitKind::Executable;
    let mut verbose = false;
    let mut release = false;
    let mut test_mode = false;
    let mut core_modules_dirs: Vec<PathBuf> = Vec::new();
    let mut json = false;
    let mut heap_check = false;
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
            // TEST-TOOL T1: build an entry that runs the `test`
            // blocks instead of `main`.
            "--test" => test_mode = true,
            "--all-backends" => all_backends = true,
            s if s.starts_with("--heap-check=") => match &s["--heap-check=".len()..] {
                "report" => heap_check = true,
                "poison" => {
                    return Err(
                        "--heap-check=poison checks the interpreter's heap only for now: run \
                         `interpreter --heap-check=poison` (the compiled lanes come with \
                         design-docs/HEAP_CHECK.md H2)"
                            .to_string(),
                    )
                }
                "reuse" => {
                    return Err(
                        "--heap-check=reuse is not available yet (design-docs/HEAP_CHECK.md, H3)"
                            .to_string(),
                    )
                }
                other => return Err(format!("--heap-check expects `report`, got `{other}`")),
            },
            // Repeatable, and a comma list: `--profile=mem,compile`.
            s if s.starts_with("--profile=") => {
                for what in s["--profile=".len()..].split(',') {
                    match what {
                        "mem" => profile_mem = true,
                        "compile" => compile_profile = true,
                        other => {
                            return Err(format!("--profile expects `mem` or `compile`, got `{other}`"))
                        }
                    }
                }
            }
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
                core_modules_dirs.push(PathBuf::from(v));
            }
            s if s.starts_with("--diagnostics") || s.starts_with("--profile-format") => {
                return Err(folded_into_format(s));
            }
            s if s.starts_with("--format=") => {
                json = parse_format(&s["--format=".len()..])?;
            }
            "--format" => {
                i += 1;
                let v = args.get(i).ok_or_else(|| "--format needs `text` or `json`".to_string())?;
                json = parse_format(v)?;
            }
            s if s.starts_with("--core-modules=") => {
                core_modules_dirs.push(PathBuf::from(&s["--core-modules=".len()..]));
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
    options.core_modules_dirs = core_modules_dirs;
    options.test_mode = test_mode;
    // One flag shapes everything the tool itself prints: the result,
    // the diagnostics and the memory report.
    options.diagnostics_json = json;
    let profile = match (profile_mem, json) {
        (false, _) => ProfileMode::Off,
        (true, false) => ProfileMode::Text,
        (true, true) => ProfileMode::Json,
    };
    // `--all-backends` runs the program; there is no single compile
    // whose phases the profile would describe.
    if compile_profile && all_backends {
        return Err("--profile=compile times one AOT build; drop --all-backends".to_string());
    }
    // Report mode lives in the runtime: a binary reads it from its
    // environment, so there is nothing to build in.
    if heap_check && !all_backends {
        return Err(
            "--heap-check=report is read at run time: build normally and run the binary \
             with TOY_HEAP_CHECK=report (or add --all-backends)"
                .to_string(),
        );
    }
    Ok((options, Mode { all_backends, profile, compile_profile, json, heap_check }))
}

fn parse_format(value: &str) -> Result<bool, String> {
    match value {
        "json" => Ok(true),
        "text" => Ok(false),
        other => Err(format!("--format expects `text` or `json`, got `{other}`")),
    }
}

/// `--diagnostics` and `--profile-format` were separate spellings of
/// the same choice; they are now `--format`. Named rather than reported
/// as unknown, so a command copied from an old note says what to type.
fn folded_into_format(flag: &str) -> String {
    let name = flag.split('=').next().unwrap_or(flag);
    format!("{name} was folded into --format; use --format=json (or --format=text)")
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
        "usage: compiler <input.t> [-o <output>] [--emit exe|obj|ir|clif] [--release] [--format=text|json] [-v]"
    );
    eprintln!(
        "       compiler <input.t> --all-backends   # run on interpreter / JIT / AOT, report disagreements"
    );
    eprintln!(
        "       compiler <input.t> --all-backends --profile=mem [--format=text|json]  # also compare allocation totals"
    );
    eprintln!("       --format=json prints the result (what was built, or the backends' verdict) as JSON on stdout,");
    eprintln!("       and the diagnostics and the memory report as JSON on stderr");
    eprintln!(
        "       compiler <input.t> --profile=compile [--format=text|json]  # time each compile phase (stderr)"
    );
    eprintln!("       use `-` as <input.t> to read the program from stdin");
}
