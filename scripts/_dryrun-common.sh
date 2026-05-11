# shellcheck shell=bash
#
# Sourceable helpers shared by smoke / llu-dryrun / ns-dryrun /
# full-dryrun. Sets repo cwd, configures default tracing env. Source
# from each script with:
#
#   source "$(dirname "${BASH_SOURCE[0]}")/_dryrun-common.sh"
#
# Idempotent — safe to source multiple times in chained-script flows.

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export RUST_LOG="${RUST_LOG:-gluco_hub=info}"
export GLUCO_HUB_LOG_PRETTY="${GLUCO_HUB_LOG_PRETTY:-1}"
