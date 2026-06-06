# Clock View E2E Test Harness

End-to-end tests for `gluco-hub/src/api/clock.html`.
Tests are self-contained under `tests/e2e/clock/` and have no effect on the
Rust build.

## Quick start

```bash
cd gluco-hub/tests/e2e/clock
npm install
npx playwright install --with-deps chromium
npx playwright test
```

## Running just the deterministic unit test (no browser needed)

The `clockUrl()` base-derivation logic — the #1 Ingress-URL risk — is
covered by a standalone Node test that runs without a browser:

```bash
node clockUrl.unit.test.mjs
```

## Running the mock server standalone

```bash
node mock-server.mjs [port]   # default port 9744
# Direct:  http://127.0.0.1:9744/clock
# Ingress: http://127.0.0.1:9744/api/hassio_ingress/TESTTOKEN/clock
```

## Test coverage

| # | Assertion | Requires browser |
|---|-----------|-----------------|
| 1 | Direct load: value renders (120 mg/dL), zone-inrange class, stable trend arrow | yes |
| 1b | SSE update 1: value becomes 45, zone-hypo, trend arrow updates to ↓ | yes |
| 1c | SSE update 2: value becomes 210, zone-high | yes |
| 2 | Sparkline: after tap, SVG polyline has non-empty points, band rect, dot circle | yes |
| 3 | Settings: unit toggle 120 mg/dL → 6.7 mmol/L | yes |
| 3b | localStorage PHI guard: only `gluco_unit` / `gluco_theme` stored, no glucose values or timestamps | yes |
| 4 | E-Ink: `data-preset=eink` set, `data-ctx=eink` set, opacity=1, transition=none, stale-label shown | yes |
| 5 | **Ingress prefix**: all API requests (state, events, history) include `/api/hassio_ingress/TESTTOKEN/` | yes |
| U1–U7 | `clockUrl()` unit tests: 7 cases covering Ingress, trailing-slash, direct, long tokens | **no** (Node only) |

## Ingress prefix assertion (the critical one)

Test 5 loads the page at:

```
http://127.0.0.1:PORT/api/hassio_ingress/TESTTOKEN/clock
```

It captures all network requests and asserts that every API call goes to:

- `.../api/hassio_ingress/TESTTOKEN/clock/state`
- `.../api/hassio_ingress/TESTTOKEN/clock/events`
- `.../api/hassio_ingress/TESTTOKEN/clock/history`

A broken `clockUrl()` would produce `/clock/state` (dropping the token),
which would be a production failure on HA Ingress. This test catches it.

## Artifacts

Screenshots are saved to `artifacts/` on every run:

| File | Captured at |
|------|-------------|
| `01-direct-initial.png` | Initial state load (120 mg/dL, in-range) |
| `02-direct-post-sse-hypo.png` | After SSE reading: 45 mg/dL hypo |
| `03-direct-post-sse-high.png` | After SSE reading: 210 mg/dL high |
| `04-sparkline.png` | Sparkline overlay open |
| `05-settings-mmol.png` | Settings panel, mmol/L selected |
| `06-eink.png` | E-ink mode layout |
| `07-ingress.png` | Ingress-prefix load |

## CI integration

Add to your CI pipeline after Rust tests:

```yaml
- name: Run Clock View E2E tests
  working-directory: gluco-hub/tests/e2e/clock
  run: |
    npm ci
    npx playwright install --with-deps chromium
    npx playwright test
```

## Known limitations / fallback status

The unit tests (`clockUrl.unit.test.mjs`) run without a browser and cover the
#1 Ingress-URL risk with 7 deterministic assertions. They run on any Node.js
environment.

The Playwright browser tests require Chromium (ARM64 Linux is supported by
Playwright's own bundled Chromium; it is NOT supported by Puppeteer's bundled
Chrome). If `npx playwright install chromium` fails in your environment, only
the unit tests will run, and the browser assertions are pending.
