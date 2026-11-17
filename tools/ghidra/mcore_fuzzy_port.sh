#!/usr/bin/env bash
# STEP 3b of the M·CORE image-recovery pipeline: the FUZZY layer under mcore_port.sh. Run it
# after the exact signature port(s); it names the still-generic target functions whose masked
# instruction n-grams are mostly contained in one named function of a reference image.
# CLOSE THE GHIDRA GUI on the target first (ApplyPortedNames writes back).
#
# Usage:
#   tools/ghidra/mcore_fuzzy_port.sh <dst_proj> <dst_name> <dst_prog> <label>=<src_proj>:<src_name>:<src_prog> [more sources…]
# e.g. (closest relative FIRST — a tie in score goes to the earlier source)
#   tools/ghidra/mcore_fuzzy_port.sh projects/817.3_e46_m3_zcp_mk60 E46_M3_ZCP_mk60 e46_m3_zcp_mk60_full_dump.cpu.hex \
#       E46_nonM=projects/820.3_E46_non_m_mk60:E46_nonM_mk60:non_m_mk60_full_dump.cpu.hex \
#       M3=projects/E9x_M3_7846816A:MK60_7846816A:7846816A.cpu.hex
#
# DumpMasked.java on every project (read-only snapshots), fuzzy_port.py (6-gram containment,
# floor 0.60, unique + mutual best, comparable size; FUZZY_ARGS adds/overrides options), then
# ApplyPortedNames.java on the target with a plate comment carrying score and source.
# Results in $WORK (fuzzy.tsv applied, fuzzy_tail.txt for hand review). Exit 1 on failure.
set -euo pipefail
[ $# -ge 4 ] || { sed -n '2,19p' "$0"; exit 2; }

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DP="$1"; DN="$2"; DPROG="$3"; shift 3
HEADLESS="${GHIDRA_HOME:-/opt/homebrew/opt/ghidra/libexec}/support/analyzeHeadless"
export JAVA_HOME="${JAVA_HOME:-/Library/Java/JavaVirtualMachines/temurin-25.jdk/Contents/Home}"
WORK="${WORK:-$(mktemp -d "${TMPDIR:-/tmp}/mk60fuzzy.XXXXXX")}"
SCR="$ROOT/tools/ghidra/scripts"

dump() {  # <proj> <name> <prog> <out> ; read-only, on a snapshot copy
  local snap; snap="$(mktemp -d "${TMPDIR:-/tmp}/mk60snap.XXXXXX")"
  cp "$1/$2.gpr" "$snap/"; cp -R "$1/$2.rep" "$snap/"
  local log="$snap/dump.log" rc=0
  "$HEADLESS" "$snap" "$2" -process "$3" -noanalysis -scriptPath "$SCR" \
      -postScript DumpMasked.java "$4" > "$log" 2>&1 || rc=$?
  grep -E "DumpMasked:|ERROR" "$log" | sed -E 's/^INFO  [A-Za-z0-9]+\.java> //; s/ \(GhidraScript\) *$//' || true
  if [ "$rc" -ne 0 ] || [ ! -s "$4" ]; then
    echo "!! DumpMasked failed for $1/$2 ($3): headless exit $rc" >&2
    grep -E 'ERROR|Exception' "$log" | head -10 >&2 || true
    rm -rf "$snap"; exit 1
  fi
  rm -rf "$snap"
}

echo ">> masked instruction streams"
SRC_ARGS=()
for spec in "$@"; do
  label="${spec%%=*}"; rest="${spec#*=}"
  IFS=: read -r sp sn sprog <<< "$rest"
  [ -n "$label" ] && [ -n "$sp" ] && [ -n "$sn" ] && [ -n "$sprog" ] || { echo "!! bad source spec: $spec (want label=proj:name:prog)" >&2; exit 2; }
  dump "$sp" "$sn" "$sprog" "$WORK/$label.masked"
  SRC_ARGS+=(--src "$label=$WORK/$label.masked")
done
dump "$DP" "$DN" "$DPROG" "$WORK/dst.masked"

echo ">> score"
# shellcheck disable=SC2086
python3 -I "$ROOT/tools/fuzzy_port.py" "${SRC_ARGS[@]}" "$WORK/dst.masked" \
    -o "$WORK/fuzzy.tsv" --tail "$WORK/fuzzy_tail.txt" ${FUZZY_ARGS:-}

echo ">> apply to target (writes back -- GUI must be closed)"
APPLY_LOG="$WORK/apply.log"; rc=0
"$HEADLESS" "$DP" "$DN" -process "$DPROG" -noanalysis -scriptPath "$SCR" \
    -postScript ApplyPortedNames.java "$WORK/fuzzy.tsv" > "$APPLY_LOG" 2>&1 || rc=$?
grep -E "ApplyPortedNames:|Unable to lock|ERROR" "$APPLY_LOG" | sed -E 's/^INFO  [A-Za-z0-9]+\.java> //; s/ \(GhidraScript\) *$//' || true
if grep -q 'Unable to lock' "$APPLY_LOG"; then
  echo "!! target project is locked — close the Ghidra GUI and re-run." >&2; exit 1
fi
if [ "$rc" -ne 0 ] || ! grep -q 'ApplyPortedNames:' "$APPLY_LOG"; then
  echo "!! ApplyPortedNames failed (headless exit $rc) — see $APPLY_LOG" >&2; exit 1
fi
echo ">> fuzzy tail for review: $WORK/fuzzy_tail.txt"
