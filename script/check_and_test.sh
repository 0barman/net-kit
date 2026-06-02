#!/usr/bin/env bash
#
# check_and_test.sh
#
# 1. Scan the product code under src/ for dangerous, crash-prone constructs
#    (.unwrap(), .expect(...), panic!, unreachable!, todo!, unimplemented!).
#    If ANY are found, stop immediately and report every location.
# 2. Only if the scan is clean, run the full test suite. If ANY test fails,
#    stop and report.
#
# Usage:
#   bash script/check_and_test.sh
#
# Exit codes:
#   0  -> scan clean AND all tests passed
#   1  -> dangerous code found in src/ (tests were NOT run)
#   2  -> a test case failed
#   3  -> environment/setup error (e.g. cargo not found, wrong directory)

set -uo pipefail

# ---------------------------------------------------------------------------
# Resolve the crate root (the parent of this script's directory) so the script
# works regardless of the caller's current working directory.
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
SRC_DIR="${CRATE_ROOT}/src"

# Colors (fall back to plain text when not a TTY).
if [ -t 1 ]; then
  RED="$(printf '\033[31m')"; GREEN="$(printf '\033[32m')"
  YELLOW="$(printf '\033[33m')"; BOLD="$(printf '\033[1m')"; RESET="$(printf '\033[0m')"
else
  RED=""; GREEN=""; YELLOW=""; BOLD=""; RESET=""
fi

notify() { printf '%s\n' "$*"; }
fail()   { printf '%s%s%s\n' "${RED}${BOLD}" "$*" "${RESET}"; }
ok()     { printf '%s%s%s\n' "${GREEN}${BOLD}" "$*" "${RESET}"; }

# ---------------------------------------------------------------------------
# Sanity checks.
# ---------------------------------------------------------------------------
if ! command -v cargo >/dev/null 2>&1; then
  fail "ERROR: 'cargo' was not found on PATH. Cannot run tests."
  exit 3
fi
if [ ! -d "${SRC_DIR}" ]; then
  fail "ERROR: source directory not found: ${SRC_DIR}"
  exit 3
fi

# ---------------------------------------------------------------------------
# Step 1: scan product code (src/) for dangerous constructs.
#
# We deliberately scan ONLY src/ (product code). Tests, examples and benches
# legitimately use unwrap/expect/panic and are out of scope here.
#
# Each pattern is matched on lines AFTER stripping // line comments and #![...]
# style attributes are left intact (they cannot contain these calls). Block
# comments are not stripped; in this codebase dangerous tokens never appear in
# block comments, and missing one would only over-report (safe direction).
# ---------------------------------------------------------------------------

# Patterns to flag. Each entry is "label|regex".
PATTERNS=(
  "unwrap()|\.unwrap\(\)"
  "unwrap_err()|\.unwrap_err\(\)"
  "expect(...)|\.expect\("
  "panic!|panic!"
  "unreachable!|unreachable!"
  "todo!|todo!"
  "unimplemented!|unimplemented!"
)

notify "${BOLD}== Step 1/2: scanning src/ for dangerous code ==${RESET}"
notify "scan root: ${SRC_DIR}"
notify ""

FINDINGS_FILE="$(mktemp)"
trap 'rm -f "${FINDINGS_FILE}"' EXIT

total_findings=0

# Collect every .rs file under src/.
while IFS= read -r -d '' file; do
  # Read with line numbers, strip trailing // comments, then test each pattern.
  # We process line-by-line so we can drop comment text before matching.
  lineno=0
  while IFS= read -r raw || [ -n "$raw" ]; do
    lineno=$((lineno + 1))

    # Strip a // line comment (best-effort: removes from the first // to EOL).
    # This avoids flagging dangerous words that appear only inside comments.
    code="${raw%%//*}"

    # Skip if nothing remains after removing the comment.
    [ -z "${code//[[:space:]]/}" ] && continue

    for entry in "${PATTERNS[@]}"; do
      label="${entry%%|*}"
      regex="${entry#*|}"
      if printf '%s' "$code" | grep -Eq "$regex"; then
        rel="${file#"${CRATE_ROOT}/"}"
        printf '%s:%s: [%s] %s\n' "$rel" "$lineno" "$label" "$(printf '%s' "$raw" | sed 's/^[[:space:]]*//')" >>"${FINDINGS_FILE}"
        total_findings=$((total_findings + 1))
      fi
    done
  done <"$file"
done < <(find "${SRC_DIR}" -type f -name '*.rs' -print0)

if [ "${total_findings}" -gt 0 ]; then
  fail "DANGEROUS CODE FOUND in src/ (${total_findings} occurrence(s)). Tests were NOT run."
  notify ""
  notify "${YELLOW}The following crash-prone constructs must be removed or made safe:${RESET}"
  notify ""
  # Print sorted by file:line for readability.
  sort -t: -k1,1 -k2,2n "${FINDINGS_FILE}"
  notify ""
  fail "STOPPING. Please fix the locations above, then re-run this script."
  exit 1
fi

ok "Scan clean: no dangerous constructs found in src/."
notify ""

# ---------------------------------------------------------------------------
# Step 2: run the full test suite. Stop on the first failing test case.
# ---------------------------------------------------------------------------
notify "${BOLD}== Step 2/2: running the test suite (cargo test) ==${RESET}"
notify ""

# --no-fail-fast is intentionally NOT used: we want to surface a failure and
# stop, matching the requested behavior. cargo test returns non-zero if any
# test fails.
( cd "${CRATE_ROOT}" && cargo test )
test_status=$?

notify ""
if [ "${test_status}" -ne 0 ]; then
  fail "TEST FAILURE: at least one test case failed (cargo exit code ${test_status})."
  fail "STOPPING. See the cargo output above for the failing test(s)."
  exit 2
fi

ok "All tests passed and no dangerous code was found. ✔"
exit 0
