#!/usr/bin/env bash
#
# Live one-shot probe against the real LibreLink Up API. Use this
# BEFORE wiring Nightscout — it confirms credentials + region +
# version actually work without involving the sink path.
#
# Required env:
#   LLU_EMAIL       — account email
#   LLU_PASSWORD    — plaintext password (never written to disk)
#   LLU_REGION      — uppercase region code (EU, US, DE, …); default EU
#
# Optional env:
#   LLU_VERSION     — pin a specific app-version header (e.g. 4.17.0)
#                     when LibreView rejects the binary's default
#   LLU_PATIENT_ID  — pick a specific patient when multiple are linked
#
# Exit codes (matches src/main.rs::DRYRUN_EXIT_TABLE):
#   0  ok
#   1  unclassified failure
#   2  config / env (missing var or bad region/email)
#   3  invalid credentials
#   4  status / protocol / version mismatch
#   5  transport / network / WAF rejection

set -euo pipefail

if [[ -z "${LLU_EMAIL:-}" || -z "${LLU_PASSWORD:-}" ]]; then
    echo "ERROR: LLU_EMAIL and LLU_PASSWORD must be set" >&2
    exit 2
fi
LLU_REGION="${LLU_REGION:-EU}"

# shellcheck source=scripts/_dryrun-common.sh
source "$(dirname "${BASH_SOURCE[0]}")/_dryrun-common.sh"

# Translate operator-friendly env vars into the GLUCO_HUB__* names the
# config crate understands (double-underscore = section separator).
export GLUCO_HUB__SOURCE__LLU__EMAIL="${LLU_EMAIL}"
export GLUCO_HUB__SOURCE__LLU__PASSWORD="${LLU_PASSWORD}"
export GLUCO_HUB__SOURCE__LLU__REGION="${LLU_REGION}"
if [[ -n "${LLU_VERSION:-}" ]]; then
    export GLUCO_HUB__SOURCE__LLU__VERSION="${LLU_VERSION}"
fi
if [[ -n "${LLU_PATIENT_ID:-}" ]]; then
    export GLUCO_HUB__SOURCE__LLU__PATIENT_ID="${LLU_PATIENT_ID}"
fi

cargo run --quiet --features source-llu -- dryrun
