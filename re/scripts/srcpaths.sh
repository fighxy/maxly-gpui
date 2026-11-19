#!/usr/bin/env bash
# Extract build-machine source paths embedded in MAX binaries (assert/log strings).
# Usage: srcpaths.sh <bin-dir> <out-dir> <file>...
set -euo pipefail
bin_dir=$1; out_dir=$2; shift 2
pat='C:[\\/]gitlab[\\/]builds[\\/][A-Za-z0-9]+[\\/]0[\\/]client[\\/][A-Za-z0-9_./\\-]+'
for f in "$@"; do
  rg -a -o -N "$pat" "$bin_dir/$f" \
    | tr '\\' '/' \
    | sed -E 's#^C:/gitlab/builds/[A-Za-z0-9]+/0/client/##' \
    | sort -u > "$out_dir/srcpaths_$f.txt" || true
  echo "$f: $(wc -l < "$out_dir/srcpaths_$f.txt")"
done
