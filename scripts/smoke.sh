#!/usr/bin/env bash
#
# Smoke-test the gluco-hub HTTP surface end-to-end against the
# in-memory MockSource. Boots the binary with `--features mock-source`,
# hits /healthz, /glucose/latest, /metrics, then signals graceful
# shutdown. Catches "the example doesn't work" before a new operator
# does, without needing a real LibreLink Up account.
#
# Exit codes:
#   0  every endpoint returned the expected status; binary shut down
#      cleanly on SIGTERM.
#   1+ something went wrong; the `set -e` line prints the failing call.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

PORT="${GLUCO_HUB_SMOKE_PORT:-18080}"
BIND="127.0.0.1:${PORT}"
BASE="http://${BIND}"
CONFIG="${REPO_ROOT}/config.example.toml"

# Don't try to launch a real LLU source: ensure no [source.llu] block
# slipped in via env overrides from a prior shell.
unset GLUCO_HUB__SOURCE__LLU__EMAIL || true
unset GLUCO_HUB__SOURCE__LLU__PASSWORD || true
unset GLUCO_HUB__SOURCE__LLU__REGION || true

export GLUCO_HUB__HTTP__BIND="${BIND}"
export GLUCO_HUB_LOG_PRETTY="${GLUCO_HUB_LOG_PRETTY:-1}"
export RUST_LOG="${RUST_LOG:-gluco_hub=info}"

LOG="$(mktemp)"
trap 'rm -f "$LOG"' EXIT

echo "==> building gluco-hub with mock-source…"
cargo build --quiet --features mock-source

BIN="${REPO_ROOT}/target/debug/gluco-hub"
if [ ! -x "$BIN" ]; then
    echo "ERROR: $BIN not found after build" >&2
    exit 1
fi

echo "==> launching $BIN run -c $CONFIG on $BIND"
"$BIN" run -c "$CONFIG" >"$LOG" 2>&1 &
PID=$!

# Always reap the child, even when an assertion below fails.
cleanup() {
    if kill -0 "$PID" 2>/dev/null; then
        kill -TERM "$PID" 2>/dev/null || true
        wait "$PID" 2>/dev/null || true
    fi
    if [ -n "${SMOKE_FAIL:-}" ]; then
        echo "==> binary log on failure:"
        cat "$LOG" >&2
    fi
    rm -f "$LOG"
}
trap 'SMOKE_FAIL=1; cleanup' INT TERM ERR

echo "==> waiting for /healthz…"
for _ in $(seq 1 50); do
    if curl -fsS "$BASE/healthz" >/dev/null 2>&1; then
        break
    fi
    sleep 0.2
done

echo
echo "==> /healthz"
curl -fsS "$BASE/healthz"
echo

echo "==> /glucose/latest (cache empty before first poll → 503 + API001)"
HTTP=$(curl -sS -o /tmp/cgm-smoke-glucose -w '%{http_code}' "$BASE/glucose/latest")
cat /tmp/cgm-smoke-glucose; echo
case "$HTTP" in
    503)
        if ! grep -q '"API001"' /tmp/cgm-smoke-glucose; then
            echo "ERROR: 503 body missing API001 marker" >&2; SMOKE_FAIL=1; exit 1
        fi
        ;;
    200)
        # First mock poll already fired — the body must have an `sgv`
        # field so the smoke proves the cache→API path works too.
        if ! grep -q '"glucose_mgdl"' /tmp/cgm-smoke-glucose; then
            echo "ERROR: 200 body missing glucose_mgdl" >&2; SMOKE_FAIL=1; exit 1
        fi
        ;;
    *)
        echo "ERROR: unexpected /glucose/latest status $HTTP" >&2; SMOKE_FAIL=1; exit 1
        ;;
esac

echo "==> /metrics (Prometheus exposition)"
curl -fsS -D /tmp/cgm-smoke-headers "$BASE/metrics" | head -n 20
grep -qi '^content-type: text/plain' /tmp/cgm-smoke-headers || {
    echo "ERROR: /metrics did not return text/plain" >&2
    SMOKE_FAIL=1; exit 1
}

echo
echo "==> sending SIGTERM and waiting for clean shutdown"
kill -TERM "$PID"
if wait "$PID"; then
    echo "==> binary exited cleanly"
else
    echo "ERROR: binary exited with non-zero status" >&2
    SMOKE_FAIL=1
    exit 1
fi

trap - INT TERM ERR
echo "==> smoke OK"
