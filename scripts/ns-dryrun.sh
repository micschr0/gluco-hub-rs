#!/usr/bin/env bash
#
# Live, read-only probe against the configured Nightscout instance.
# Hits `GET /api/v3/entries?count=1` ONLY — never POSTs an entry.
# Use AFTER `scripts/llu-dryrun.sh` and BEFORE running the bridge for
# real, to confirm the api-secret hashes correctly and the NS URL +
# TLS path actually work.
#
# Required env:
#   NS_BASE_URL      — full HTTPS URL (e.g. https://ns.example.com)
#   NS_API_SECRET    — raw API secret (NOT pre-hashed)
#
# Optional:
#   NS_API_SECRET_FILE  — read the secret from this file instead, so
#                         it never appears in shell history. Wins over
#                         NS_API_SECRET if both are set.
#
# Exit codes (matches src/main.rs::classify_nsdryrun_exit):
#   0  ok
#   1  unclassified failure
#   2  config / env / invalid base URL
#   3  transport / network
#   4  401 / 403 auth
#   5  unexpected status / retryable

set -euo pipefail

if [[ -z "${NS_BASE_URL:-}" ]]; then
    echo "ERROR: NS_BASE_URL must be set" >&2
    exit 2
fi
if [[ -n "${NS_API_SECRET_FILE:-}" ]]; then
    if [[ ! -r "$NS_API_SECRET_FILE" ]]; then
        echo "ERROR: NS_API_SECRET_FILE not readable: $NS_API_SECRET_FILE" >&2
        exit 2
    fi
    NS_API_SECRET="$(cat "$NS_API_SECRET_FILE")"
fi
if [[ -z "${NS_API_SECRET:-}" ]]; then
    echo "ERROR: NS_API_SECRET (or NS_API_SECRET_FILE) must be set" >&2
    exit 2
fi
export NS_API_SECRET

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Translate operator-friendly env vars into the names the binary's
# config crate already understands.
export CGM_BRIDGE__SINK__NIGHTSCOUT__BASE_URL="${NS_BASE_URL}"
export CGM_BRIDGE__SINK__NIGHTSCOUT__API_SECRET_ENV="NS_API_SECRET"

# Tracing on stderr; the JSON summary is the sole stdout content.
export RUST_LOG="${RUST_LOG:-cgm_bridge=info}"
export CGM_BRIDGE_LOG_PRETTY="${CGM_BRIDGE_LOG_PRETTY:-1}"

cargo run --quiet --features sink-nightscout -- ns-dryrun
