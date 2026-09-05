#!/bin/sh
# Build the module root that `--core-modules` is pointed at.
#
# toylang finds modules by walking one directory, so the root has to
# contain both the standard library and this program's modules. Both
# are symlinks, so editing `src/*.t` needs no build step and adding a
# module needs no re-run of this script.
#
# `main.t` sits beside this script, *outside* `src/`, and that is on
# purpose: the entry point is handed to the compiler as the program,
# and a file that is also inside the module root gets auto-loaded a
# second time. The duplicate is type-checked without the file's own
# top-level `const` declarations, which fails as
# "Identifier 'BUF_BYTES' not found". One file, one role.
#
# The root goes under `build/`, which is not tracked: it holds nothing
# but symlinks, and where they point depends on where this checkout
# lives. This script is the source; `build/` is its output.
set -e
here=$(cd "$(dirname "$0")" && pwd)
root="$here/build/root"
core=$(cd "$here/../../core" && pwd)

rm -rf "$root"
mkdir -p "$root"
ln -s "$core/std" "$root/std"
ln -s "$here/src" "$root/logsearch"
echo "module root: $root"
ls -l "$root"
