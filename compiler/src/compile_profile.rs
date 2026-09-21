//! COMPILE-PROFILE: render what `frontend::compile_profile` recorded
//! during one AOT compile (`compiler --profile=compile`).
//!
//! The recorder holds the phases, files, counters and hotspots; this
//! adds the conditions the compile ran under (section A of
//! `design-docs/COMPILE_PROFILE.md`) and draws the text table or the
//! JSON document. Both go to stderr: under `--format=json` stdout
//! already carries the build's own result document.

use std::time::Duration;

use frontend::compile_profile::{Hot, Origin, Phase, Profile};
use serde_json::{json, Value};

use crate::CompilerOptions;

/// Rows shown per hotspot table and in the text file list. The JSON
/// file list is complete; its hotspot tables are cut the same way.
const TOP: usize = 10;

/// The conditions a compile ran under. Two profiles are comparable
/// only when these agree.
struct Conditions {
    input: String,
    emit: &'static str,
    release: bool,
    /// How the compiler binary itself was built. A debug build of the
    /// compiler is several times slower, which swamps everything else.
    compiler_build: &'static str,
    opt_level: &'static str,
    ast_cache_dir: Option<String>,
    link_cache_dir: Option<String>,
    usage: Usage,
}

/// Process-wide resource use, from `getrusage`.
#[derive(Default)]
struct Usage {
    user: Duration,
    sys: Duration,
    peak_rss_bytes: u64,
}

fn usage() -> Usage {
    // SAFETY: `getrusage` fills the struct it is handed and reads
    // nothing else; a zeroed `rusage` is a valid value to pass.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) } != 0 {
        return Usage::default();
    }
    let tv = |t: libc::timeval| {
        Duration::from_secs(t.tv_sec as u64) + Duration::from_micros(t.tv_usec as u64)
    };
    // macOS reports the peak in bytes, Linux in kilobytes.
    let rss_unit = if cfg!(target_os = "macos") { 1 } else { 1024 };
    Usage {
        user: tv(ru.ru_utime),
        sys: tv(ru.ru_stime),
        peak_rss_bytes: ru.ru_maxrss as u64 * rss_unit,
    }
}

fn conditions(options: &CompilerOptions) -> Conditions {
    let ast_cache_dir = std::env::var("TOY_CACHE_DISABLE")
        .map_or(true, |v| v.is_empty())
        .then(|| frontend::cache::default_cache_dir().display().to_string());
    Conditions {
        input: options.input.display().to_string(),
        emit: match options.emit {
            crate::EmitKind::Executable => "exe",
            crate::EmitKind::Object => "obj",
            crate::EmitKind::Ir => "ir",
            crate::EmitKind::Clif => "clif",
        },
        release: options.release,
        compiler_build: if cfg!(debug_assertions) { "debug" } else { "release" },
        opt_level: crate::codegen::cranelift_opt_level(),
        ast_cache_dir,
        link_cache_dir: crate::driver::link_cache_dir(options.link_cache_dir.as_deref())
            .map(|d| d.display().to_string()),
        usage: usage(),
    }
}

/// Render `profile` as text or as one JSON document.
pub fn render(profile: &Profile, options: &CompilerOptions, json: bool) -> String {
    let conditions = conditions(options);
    if json {
        let value = to_json(profile, &conditions);
        let mut out = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
        out.push('\n');
        out
    } else {
        to_text(profile, &conditions)
    }
}

fn ms(d: Duration) -> f64 {
    // Three decimals: microseconds are the finest the recorder means.
    (d.as_secs_f64() * 1e6).round() / 1e3
}

/// A phase's time not covered by its children: work nobody named, or
/// a gap between measured steps.
fn self_time(phase: &Phase) -> Duration {
    let children: Duration = phase.children.iter().map(|c| c.wall).sum();
    phase.wall.saturating_sub(children)
}

/// Files, bytes and lines, entry against everything else.
fn totals(profile: &Profile) -> [(u64, u64, u64); 2] {
    let mut out = [(0, 0, 0); 2];
    for f in &profile.files {
        let slot = &mut out[usize::from(f.origin != Origin::Entry)];
        slot.0 += 1;
        slot.1 += f.bytes;
        slot.2 += f.lines;
    }
    out
}

fn top(hot: &[Hot]) -> Vec<&Hot> {
    let mut rows: Vec<&Hot> = hot.iter().collect();
    rows.sort_by(|a, b| b.wall.cmp(&a.wall).then_with(|| a.name.cmp(&b.name)));
    rows.truncate(TOP);
    rows
}

fn to_json(profile: &Profile, c: &Conditions) -> Value {
    fn phase(p: &Phase) -> Value {
        let mut v = json!({
            "name": p.name,
            "start_ms": ms(p.start),
            "wall_ms": ms(p.wall),
        });
        if !p.children.is_empty() {
            v["self_ms"] = json!(ms(self_time(p)));
            v["children"] = p.children.iter().map(phase).collect();
        }
        v
    }
    fn hot(h: &Hot) -> Value {
        let mut v = json!({ "name": h.name, "ms": ms(h.wall) });
        if let Some(n) = h.ir_insts {
            v["ir_insts"] = json!(n);
        }
        if let Some(n) = h.code_bytes {
            v["code_bytes"] = json!(n);
        }
        v
    }
    let [entry, other] = totals(profile);
    let count = |(files, bytes, lines): (u64, u64, u64)| {
        json!({ "files": files, "bytes": bytes, "lines": lines })
    };
    json!({
        "input": c.input,
        "emit": c.emit,
        "release": c.release,
        "compiler_build": c.compiler_build,
        "cranelift_opt_level": c.opt_level,
        "ast_cache_dir": c.ast_cache_dir,
        "link_cache_dir": c.link_cache_dir,
        "total_ms": ms(profile.wall),
        "user_ms": ms(c.usage.user),
        "sys_ms": ms(c.usage.sys),
        "peak_rss_bytes": c.usage.peak_rss_bytes,
        "phases": profile.phases.iter().map(phase).collect::<Vec<_>>(),
        "sources": {
            "entry": count(entry),
            "modules": count(other),
            "files": profile.files.iter().map(|f| json!({
                "path": f.path,
                "origin": f.origin.as_str(),
                "bytes": f.bytes,
                "lines": f.lines,
                "ast_cache": f.ast_cache.as_str(),
                "parse_ms": ms(f.parse),
                "integrate_ms": ms(f.integrate),
            })).collect::<Vec<_>>(),
        },
        "counters": profile.counters,
        "hot": {
            "typecheck": top(&profile.hot_typecheck).into_iter().map(hot).collect::<Vec<_>>(),
            "lower": top(&profile.hot_lower).into_iter().map(hot).collect::<Vec<_>>(),
            "codegen": top(&profile.hot_codegen).into_iter().map(hot).collect::<Vec<_>>(),
        },
    })
}

fn to_text(profile: &Profile, c: &Conditions) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let total = profile.wall;
    let pct = |d: Duration| {
        if total.is_zero() { 0.0 } else { d.as_secs_f64() / total.as_secs_f64() * 100.0 }
    };
    let _ = writeln!(
        out,
        "compile profile: {} (emit {}, contracts {}, cranelift opt {}, {} build of the compiler)",
        c.input,
        c.emit,
        if c.release { "off (--release)" } else { "on" },
        c.opt_level,
        c.compiler_build,
    );
    let _ = writeln!(
        out,
        "  ast cache {}, link cache {}",
        c.ast_cache_dir.as_deref().unwrap_or("off"),
        c.link_cache_dir.as_deref().unwrap_or("off"),
    );
    let _ = writeln!(
        out,
        "  total {:.1} ms wall, {:.1} ms user, {:.1} ms sys, peak RSS {:.1} MB",
        ms(total),
        ms(c.usage.user),
        ms(c.usage.sys),
        c.usage.peak_rss_bytes as f64 / (1024.0 * 1024.0),
    );

    let _ = writeln!(out, "\n{:<34} {:>9} {:>9} {:>6} {:>9}", "phase", "start ms", "wall ms", "%", "self ms");
    fn walk(out: &mut String, p: &Phase, depth: usize, pct: &dyn Fn(Duration) -> f64) {
        let name = format!("{}{}", "  ".repeat(depth), p.name);
        let self_col = if p.children.is_empty() {
            String::new()
        } else {
            format!("{:.3}", ms(self_time(p)))
        };
        let _ = writeln!(
            out,
            "{name:<34} {:>9.3} {:>9.3} {:>5.1}% {self_col:>9}",
            ms(p.start),
            ms(p.wall),
            pct(p.wall),
        );
        for child in &p.children {
            walk(out, child, depth + 1, pct);
        }
    }
    for p in &profile.phases {
        walk(&mut out, p, 0, &pct);
    }
    let measured: Duration = profile.phases.iter().map(|p| p.wall).sum();
    let _ = writeln!(
        out,
        "{:<34} {:>9} {:>9.3} {:>5.1}%",
        "(outside any phase)",
        "",
        ms(total.saturating_sub(measured)),
        pct(total.saturating_sub(measured)),
    );

    let [entry, other] = totals(profile);
    let _ = writeln!(
        out,
        "\nsources: entry {} file(s) {} bytes {} lines; modules {} file(s) {} bytes {} lines",
        entry.0, entry.1, entry.2, other.0, other.1, other.2
    );
    let mut files: Vec<_> = profile.files.iter().collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.parse + f.integrate));
    let _ = writeln!(
        out,
        "{:<8} {:<5} {:>9} {:>9} {:>8} {:>6}  path  (top {TOP} by parse + integrate)",
        "origin", "cache", "parse ms", "integ ms", "bytes", "lines"
    );
    for f in files.iter().take(TOP) {
        let _ = writeln!(
            out,
            "{:<8} {:<5} {:>9.3} {:>9.3} {:>8} {:>6}  {}",
            f.origin.as_str(),
            f.ast_cache.as_str(),
            ms(f.parse),
            ms(f.integrate),
            f.bytes,
            f.lines,
            f.path
        );
    }

    let _ = writeln!(out, "\ncounters");
    for (key, value) in &profile.counters {
        let _ = writeln!(out, "  {key:<32} {value}");
    }

    let tables = [
        ("typecheck", &profile.hot_typecheck),
        ("lower", &profile.hot_lower),
        ("codegen", &profile.hot_codegen),
    ];
    for (title, hot) in tables {
        if hot.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\nhot: {title} (top {TOP} of {})", hot.len());
        for h in top(hot) {
            let mut extra = String::new();
            if let Some(n) = h.ir_insts {
                let _ = write!(extra, "  {n} IR insts");
            }
            if let Some(n) = h.code_bytes {
                let _ = write!(extra, "  {n} bytes");
            }
            let _ = writeln!(out, "  {:>9.3} ms  {}{extra}", ms(h.wall), h.name);
        }
    }
    out
}
