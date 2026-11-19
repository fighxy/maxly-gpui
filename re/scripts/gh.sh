#!/usr/bin/env bash
# Run a MaxRs Ghidra script against an already analyzed program in the persistent project.
# Usage: gh.sh <program, e.g. core.dll> <Script.java> [script args...]
# Needs GHIDRA_INSTALL_DIR (Ghidra 12.1.x); JAVA_HOME defaults to %USERPROFILE%\.local\jdk-21.
# Log goes to re/out/gh_<script>.log; the project is opened read-only (-readOnly).
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd -W)
program=$1; script=$2; shift 2
: "${GHIDRA_INSTALL_DIR:?set GHIDRA_INSTALL_DIR to the Ghidra 12.1.x folder}"
export GHIDRA_HEADLESS_MAXMEM=${GHIDRA_HEADLESS_MAXMEM:-12G}
export JAVA_HOME=${JAVA_HOME:-$USERPROFILE\\.local\\jdk-21}
log="$root/re/out/gh_${script%.java}.log"
cmd //c "$GHIDRA_INSTALL_DIR\\support\\analyzeHeadless.bat" \
  "$root/re/ghidra" max -process "$program" -noanalysis -readOnly \
  -scriptPath "$root/re/ghidra_scripts" -postScript "$script" "$@" > "$log" 2>&1
grep -E 'matches=|Decompile wrote|MaxRs |ExtractSchemas|ERROR|Exception' "$log" | grep -v 'INFO  Using' | tail -15 || true
