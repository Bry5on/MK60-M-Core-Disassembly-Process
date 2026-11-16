#!/usr/bin/env bash
# Verify + quantify a recovered project in ONE read-only run. Works with the Ghidra GUI open:
# the scanners run against a snapshot copy of the project (nothing is written back).
#
# Usage:
#   tools/ghidra/mcore_verify.sh <proj_dir> <name> <cpu_hex_path>
#
# Bounds come from <proj_dir>/<prog>.bounds (written by mcore_recover.sh) or, failing that,
# from find_boundary.py. codeHI is the prologue pin (recovery-window ceiling; tables are
# labelled from there), dataLO the end of the last real function (the data section proper).
# Runs CountErr, ScanMisdisasm, ImageMap, RegionStats (the region below codeLO that the
# pipeline leaves alone), UndefPrologues, PtrTargets and DataTableInventory, then prints a
# summary. Set FULL=1 to also print every DataTableInventory row (one per code-referenced
# table base).
#
# Exit status: 0 = healthy (0 error bookmarks, 0 mis-disassembled tables, 0 code bytes in
# the data section, 0 referenced code runs owned by no function, 0 prologues left in
# non-code bytes, 0 call / table-pointer targets that are not code, 0 pool slots inside code);
# 4 = one of those is non-zero (details printed).
set -euo pipefail
[ $# -ge 3 ] || { sed -n '2,14p' "$0"; exit 2; }

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PROJ_DIR="$1"; NAME="$2"; HEX="$3"; PROG="$(basename "$HEX")"
HEADLESS="${GHIDRA_HOME:-/opt/homebrew/opt/ghidra/libexec}/support/analyzeHeadless"
export JAVA_HOME="${JAVA_HOME:-/Library/Java/JavaVirtualMachines/temurin-25.jdk/Contents/Home}"
SIDECAR="$PROJ_DIR/$PROG.bounds"

if [ -f "$SIDECAR" ]; then
  FLASH_LO="$(sed -n 's/^FLASH_LO=\([0-9a-fA-F]*\).*/\1/p' "$SIDECAR")"
  CODE_LO="$(sed -n 's/^CODE_LO=\([0-9a-fA-F]*\).*/\1/p' "$SIDECAR")"
  CODE_HI="$(sed -n 's/^CODE_HI=\([0-9a-fA-F]*\).*/\1/p' "$SIDECAR")"
  DATA_LO="$(sed -n 's/^DATA_LO=\([0-9a-fA-F]*\).*/\1/p' "$SIDECAR")"
  DATA_HI="$(sed -n 's/^DATA_HI=\([0-9a-fA-F]*\).*/\1/p' "$SIDECAR")"
  [ -n "$CODE_HI" ] || CODE_HI="$DATA_LO"     # pre-island sidecar: DATA_LO was the prologue pin
  echo ">> bounds from $SIDECAR: codeLO=$CODE_LO codeHI(pin)=$CODE_HI dataLO=$DATA_LO dataHI=$DATA_HI"
else
  BND="$(python3 -I "$ROOT/tools/find_boundary.py" "$HEX" | grep '^BOUNDS' || true)"
  pick(){ echo "$BND" | sed -n "s/.*$1=0x\([0-9a-fA-F]*\).*/\1/p"; }
  FLASH_LO="$(pick flashLO)"; CODE_LO="$(pick codeLO)"; DATA_LO="$(pick codeHI)"; DATA_HI="$(pick dataHI)"; CODE_HI="$DATA_LO"
  echo ">> no $SIDECAR (mcore_recover.sh not run yet?); using find_boundary estimate: codeLO=$CODE_LO codeHI=$DATA_LO dataHI=$DATA_HI"
fi
[ -n "$CODE_LO" ] && [ -n "$DATA_LO" ] && [ -n "$DATA_HI" ] || { echo "!! could not determine bounds" >&2; exit 1; }
FLASH_LO="${FLASH_LO:-40000}"     # image base (VBR); pre-FLASH_LO sidecars are E5 images
FLASH_LO6="$(printf '%06x' "0x$FLASH_LO")"

SNAP="$(mktemp -d "${TMPDIR:-/tmp}/mk60snap.XXXXXX")"
trap 'rm -rf "$SNAP"' EXIT
cp "$PROJ_DIR/$NAME.gpr" "$SNAP/"
cp -R "$PROJ_DIR/$NAME.rep" "$SNAP/"

LOG="$(mktemp "$SNAP/verify.XXXXXX")"
rc=0
"$HEADLESS" "$SNAP" "$NAME" \
    -process "$PROG" -noanalysis \
    -scriptPath "$ROOT/tools/ghidra/scripts" \
    -postScript CountErr.java \
    -postScript ScanMisdisasm.java "$CODE_LO" "$DATA_LO" \
    -postScript ImageMap.java \
    -postScript RegionStats.java "$FLASH_LO" "$CODE_LO" \
    -postScript RegionStats.java "$CODE_LO" "$DATA_LO" \
    -postScript RegionStats.java "$DATA_LO" "$DATA_HI" \
    -postScript UndefPrologues.java "$CODE_LO" "$DATA_LO" \
    -postScript PtrTargets.java "$CODE_LO" "$DATA_HI" "$CODE_HI" \
    -postScript DataTableInventory.java "$CODE_HI" "$DATA_HI" > "$LOG" 2>&1 || rc=$?

out(){ sed -nE "s/^INFO  $1\.java> (.*) \(GhidraScript\) *$/\1/p" "$LOG"; }

if [ "$rc" -ne 0 ] || ! grep -q 'Error bookmarks=' "$LOG"; then
  echo "!! verify run failed (headless exit $rc)" >&2; grep -E 'ERROR|Exception' "$LOG" | head -20 >&2 || true; exit 1
fi

echo; echo "=== ImageMap ==="; out ImageMap
echo; echo "=== ScanMisdisasm (code region $CODE_LO..$DATA_LO) ==="; out ScanMisdisasm
echo; echo "=== RegionStats ==="; out RegionStats | grep -E '^RegionStats'
echo; echo "=== UndefPrologues (code region $CODE_LO..$DATA_LO) ==="; out UndefPrologues
echo; echo "=== PtrTargets (call targets, image $CODE_LO..$DATA_HI) ==="; out PtrTargets
if [ "${FULL:-}" = 1 ]; then echo; echo "=== DataTableInventory ==="; out DataTableInventory; fi

ERRS="$(out CountErr | sed -n 's/.*Error bookmarks=\([0-9]*\).*/\1/p')"
FNS="$(out CountErr | sed -n 's/.*functions=\([0-9]*\).*/\1/p')"
MISRUNS="$(out ScanMisdisasm | sed -n 's/.*runs=\([0-9]*\).*/\1/p')"
BASES="$(out DataTableInventory | sed -n 's/^TOTAL_BASES\t\([0-9]*\).*/\1/p')"
DATACODE="$(out RegionStats | grep "^RegionStats 0*${DATA_LO}-" | sed -n 's/.*instrBytes=\([0-9]*\).*/\1/p')"
LOWFNS="$(out RegionStats | grep "^RegionStats ${FLASH_LO6}-" | sed -n 's/.*functions=\([0-9]*\).*/\1/p')"
CODELINE="$(out RegionStats | grep "^RegionStats 0*${CODE_LO}-")"
ORPHAN="$(echo "$CODELINE" | sed -n 's/.*orphanInstrBytes=\([0-9]*\).*/\1/p')"
ORPHANREFD="$(echo "$CODELINE" | sed -n 's/.*orphanRefdRuns=\([0-9]*\).*/\1/p')"
CODEBYTES="$(echo "$CODELINE" | sed -n 's/.*instrBytes=\([0-9]*\).*/\1/p')"
UNDEFPRO="$(out UndefPrologues | sed -n 's/.*prologuesNotCode=\([0-9]*\).*/\1/p')"
PTRLINE="$(out PtrTargets | grep '^PtrTargets:' | tail -1)"
BADCALLS="$(echo "$PTRLINE" | sed -n 's/.*callTargetsNotCode=\([0-9]*\).*/\1/p')"
BADTAIL="$(echo "$PTRLINE" | sed -n 's/.*tailJumpTargetsNotCode=\([0-9]*\).*/\1/p')"
SUSPECT="$(echo "$PTRLINE" | sed -n 's/.*callTargetsSuspect=\([0-9]*\).*/\1/p')"
SLOTCODE="$(echo "$PTRLINE" | sed -n 's/.*poolSlotsInCode=\([0-9]*\).*/\1/p')"
TABLEBAD="$(echo "$PTRLINE" | sed -n 's/.*tablePtrTargetsNotCode=\([0-9]*\).*/\1/p')"
TABLEWORDS="$(echo "$PTRLINE" | sed -n 's/.*tablePtrWords=\([0-9]*\).*/\1/p')"

echo
echo "=== SUMMARY ($NAME / $PROG) ==="
printf '%-42s %s   (want 0)\n'  "error bookmarks"                         "${ERRS:-?}"
printf '%-42s %s   (want 0)\n'  "mis-disassembled pointer-table runs"     "${MISRUNS:-?}"
printf '%-42s %s   (want 0)\n'  "code bytes in data section $DATA_LO..$DATA_HI" "${DATACODE:-?}"
[ "$CODE_HI" = "$DATA_LO" ] || printf '%-42s %s\n' "call-target islands above prologue pin" "$CODE_HI..$DATA_LO"
printf '%-42s %s   (want low thousands)\n' "functions"                    "${FNS:-?}"
printf '%-42s %s of %s   (unreferenced speculative decodes; informational)\n' "code bytes owned by no function" "${ORPHAN:-?}" "${CODEBYTES:-?}"
printf '%-42s %s   (want 0)\n'  "orphan code runs referenced by data/functions" "${ORPHANREFD:-?}"
printf '%-42s %s   (want 0; functions never disassembled)\n' "prologues in non-code bytes" "${UNDEFPRO:-?}"
printf '%-42s %s   (want 0; called/tail-jumped addresses that are raw bytes)\n' "call targets not code" "${BADCALLS:-?}+${BADTAIL:-?}"
printf '%-42s %s   (call sites whose target is typed data / dirty decode: junk decodes; informational)\n' "suspect call sites" "${SUSPECT:-?}"
printf '%-42s %s   (want 0; a literal pool decoded as code)\n' "pool slots inside instructions" "${SLOTCODE:-?}"
printf '%-42s %s of %s   (want 0; untyped-table code pointers at function-like raw bytes)\n' "table pointers to undisassembled functions" "${TABLEBAD:-?}" "${TABLEWORDS:-?}"
printf '%-42s %s   (from the prologue pin $CODE_HI up)\n' "code-referenced table bases" "${BASES:-?}"
printf '%-42s %s   (auto-analysis noise below codeLO; left alone by design)\n' "functions in $FLASH_LO..$CODE_LO" "${LOWFNS:-?}"

status=0
[ "${ERRS:-1}" -eq 0 ] || status=4
[ "${MISRUNS:-1}" -eq 0 ] || status=4
[ "${DATACODE:-1}" -eq 0 ] || status=4
[ "${ORPHANREFD:-1}" -eq 0 ] || status=4
[ "${UNDEFPRO:-1}" -eq 0 ] || status=4
[ "${BADCALLS:-1}" -eq 0 ] || status=4
[ "${BADTAIL:-1}" -eq 0 ] || status=4
[ "${SLOTCODE:-1}" -eq 0 ] || status=4
[ "${TABLEBAD:-1}" -eq 0 ] || status=4
[ "$status" -eq 0 ] && echo ">> HEALTHY" || echo "!! NOT CLEAN — re-run mcore_recover.sh (it resumes and converges)" >&2
exit "$status"
