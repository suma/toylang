# Always-loaded prelude. Embedded in the binary via `include_str!`
# from `interpreter::lib`, integrated before any user code so that
# language-level conveniences are available even when the
# core-modules directory hasn't been configured.
#
# Currently empty. The numeric extension traits (`Abs` / `Sqrt`
# for `i64` / `f64`) used to live here as a transitional bridge
# during Step E of the extension-trait migration; they now live
# in `core/std/i64.t` and `core/std/f64.t` and reach user
# programs through the auto-load path. Programs that opt out of
# auto-load (`TOYLANG_CORE_MODULES=` or
# `--core-modules ""` for tests/CI) lose access to those methods —
# call the matching `__extern_*` symbol directly when that
# matters.
#
# Add new entries here only when something genuinely cannot live
# in `core/`: language built-ins that the parser / type-checker
# embed by name, or constants required to bootstrap the runtime.
