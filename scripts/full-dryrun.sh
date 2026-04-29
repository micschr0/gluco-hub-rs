#!/usr/bin/env bash
#
# Pre-flight check before the first real run. Composes the three
# probes (smoke, llu-dryrun, ns-dryrun) into one fail-fast pipeline so
# the operator gets a single OK / NOT-OK answer.
#
# Stages:
#   1. smoke      — credentials-free; HTTP API + cache + graceful shutdown.
#   2. llu-dryrun — live LLU one-shot probe (skipped without credentials).
#   3. ns-dryrun  — live NS read-only probe (skipped without credentials).
#
# Required env: none. Without LLU/NS credentials each live stage is
# logged as `[SKIP]` and the overall status remains green — useful for
# CI smoke runs where live credentials aren't available.
#
# Optional env (live stages):
#   LLU_EMAIL, LLU_PASSWORD, LLU_REGION, LLU_VERSION, LLU_PATIENT_ID
#   NS_BASE_URL, NS_API_SECRET (or NS_API_SECRET_FILE)
#   SKIP_LLU=1   — explicitly skip the LLU stage even if creds are set.
#   SKIP_NS=1    — explicitly skip the NS  stage even if creds are set.
#
# Exit codes:
#   0   every executed stage passed
#   1   smoke failed
#   2   llu-dryrun failed (its own exit code is on stderr)
#   3   ns-dryrun failed
#   4   usage (e.g. missing scripts on disk)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

for s in smoke.sh llu-dryrun.sh ns-dryrun.sh; do
    if [[ ! -x "scripts/$s" ]]; then
        echo "ERROR: scripts/$s not executable" >&2
        exit 4
    fi
done

stage() { echo; echo "==[$1] $2"; }

LLU_RESULT="skip"
NS_RESULT="skip"

# Stage 1 — smoke. Always runs; failure is fatal.
stage "1/3" "smoke (credentials-free pipeline check)"
if ! bash scripts/smoke.sh; then
    echo "==[FAIL] smoke" >&2
    echo "full-dryrun: FAIL (smoke=fail)" >&2
    exit 1
fi

# Stage 2 — llu-dryrun. Skip when creds absent or SKIP_LLU=1.
stage "2/3" "llu-dryrun (live LLU probe)"
if [[ "${SKIP_LLU:-0}" == "1" ]]; then
    echo "[SKIP] SKIP_LLU=1"
elif [[ -z "${LLU_EMAIL:-}" || -z "${LLU_PASSWORD:-}" ]]; then
    echo "[SKIP] LLU_EMAIL or LLU_PASSWORD unset"
else
    if bash scripts/llu-dryrun.sh; then
        LLU_RESULT="ok"
    else
        rc=$?
        echo "==[FAIL] llu-dryrun (exit $rc)" >&2
        echo "full-dryrun: FAIL (smoke=ok llu=fail)" >&2
        exit 2
    fi
fi

# Stage 3 — ns-dryrun. Skip when creds absent or SKIP_NS=1.
stage "3/3" "ns-dryrun (live NS read-only probe)"
if [[ "${SKIP_NS:-0}" == "1" ]]; then
    echo "[SKIP] SKIP_NS=1"
elif [[ -z "${NS_BASE_URL:-}" ]]; then
    echo "[SKIP] NS_BASE_URL unset"
elif [[ -z "${NS_API_SECRET:-}" && -z "${NS_API_SECRET_FILE:-}" ]]; then
    echo "[SKIP] neither NS_API_SECRET nor NS_API_SECRET_FILE set"
else
    if bash scripts/ns-dryrun.sh; then
        NS_RESULT="ok"
    else
        rc=$?
        echo "==[FAIL] ns-dryrun (exit $rc)" >&2
        echo "full-dryrun: FAIL (smoke=ok llu=$LLU_RESULT ns=fail)" >&2
        exit 3
    fi
fi

echo
echo "full-dryrun: OK (smoke=ok llu=$LLU_RESULT ns=$NS_RESULT)"
