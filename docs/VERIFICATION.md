# Verification Runbook — Ingress, MQTT `_patients`, and Clock View

How to verify the Sprint A–C + polished Clock View feature set end to end.
The automated layers run in CI; the three numbered steps below close the gaps
that need a real environment (Docker socket, a browser, a live Home Assistant).

Related PRs: gluco-hub-rs `#33` (PollStatus + `/api/v1/status`), `#34`
(`_patients`), `#35` (Clock View), `#36` (polished UI + `/clock/history`);
ha-libre-glucose-mqtt branch `feat/ingress-clock-view` (Ingress, `panel_admin`,
docs).

## Automated coverage (runs in CI, no setup)

```bash
cargo clippy --workspace --all-features --all-targets -- -D warnings   # must be clean
cargo clippy --workspace -- -D warnings                                 # default features
cargo test  --workspace --all-features                                  # see step 1 caveat
```

What this covers without any external service: zone logic, `PatientSummary`
serialization + abbreviated `display_name`, `PollStatus` 503/200 + `no-store`,
the readings ring buffer + `/clock/history` shape, the SSE `reading` field set,
the e-ink throttle, `/clock` HTML config injection, and a wiremock LLU →
retained `_patients` publish (in-process MQTT stub broker — no Docker).

## 1. Docker testcontainer suite (needs a Docker host)

`cargo test --workspace --all-features` reports **12 failures in a sandbox
without Docker** — the `integration_tests::{mqtt,multi_sink,nightscout}` cases
abort at container startup (`SocketNotFoundError(/run/user/.../docker.sock)`).
They are environmental, not code defects. On a host with a reachable Docker
socket they spin up real Mosquitto / Nightscout / MongoDB and go green:

```bash
docker info            # confirm the socket is reachable
cargo test --workspace --all-features integration_tests
```

This is the real-broker confirmation of the MQTT publish path (glucose,
`_health`, `_stats`, `_patients`, HA discovery).

## 2. Clock View frontend E2E (needs a browser)

Harness lives at `gluco-hub/tests/e2e/clock/` and tests the *embedded*
`src/api/clock.html` against a mock backend.

```bash
cd gluco-hub/tests/e2e/clock
node clockUrl.unit.test.mjs                       # 9 tests, NO browser — always run this
npm install
npx playwright install --with-deps chromium       # installs Chromium + system libs
npx playwright test                                # 5 browser tests
```

- `clockUrl.unit.test.mjs` (no browser) covers the **#1 risk**: under an Ingress
  path prefix `/api/hassio_ingress/<token>/clock`, the JS must resolve requests
  to `…/clock/state|events|history` (prefix preserved, never a bare `/clock/...`
  or sibling `/state`). These pass in CI today.
- The Playwright suite adds live rendering, SSE DOM updates, the history
  sparkline, the unit/theme settings panel, e-ink mode, and a network-level
  assertion of the same Ingress-prefix behavior. It needs a real Chromium
  (the headless sandbox lacks `libnspr4`/`libnss3`).

PHI guard (asserted in the spec): `localStorage` holds only `gluco_unit` /
`gluco_theme` — never glucose values, history, patient names, or timestamps.

## 3. Live end-to-end in Home Assistant (the real confirmation)

The only test that exercises live LibreLink Up → MQTT → HA panel, and the only
place the Ingress proxy behavior is real. See plan `:develop`-image-in-HA.

1. Build/publish the `:develop` image and add the add-on locally in HA.
2. Install the **Mosquitto broker** add-on; configure the MQTT integration.
3. Set LibreLink Up credentials + `topic_prefix`; start the add-on.
4. Verify:
   - `sensor.gluco_hub_<client_id>_glucose` appears (MQTT auto-discovery).
   - Retained `<topic_prefix>/_patients` carries `[{id, display_name,
     is_active}]` with abbreviated names (MQTT *Listen to topic*).
   - `GET /api/v1/status` shows `last_poll_attempt_at` vs
     `last_successful_reading_at`.
   - The sidebar **Glucose Bridge** panel (admin-only via `panel_admin`) opens
     the Clock View; the value updates live over SSE through the Ingress proxy;
     tapping shows the history sparkline. This is the ultimate proof that
     `clockUrl()` resolves correctly under the real ingress prefix.

## PHI checklist (must hold everywhere)

- `Cache-Control: no-store` on every glucose/patient response (`/api/v1/status`,
  `/clock`, `/clock/state`, `/clock/events`, `/clock/history`).
- Patient names abbreviated only (`Anna M.`), never full surnames or birthdates.
- No glucose values in `localStorage`.
- Ingress sidebar panel restricted to HA admins (`panel_admin: true`).
