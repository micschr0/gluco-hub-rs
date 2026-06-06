/**
 * E2E tests for the Clock View UI (clock.html).
 *
 * Each test group starts the mock server (or reuses the shared instance),
 * loads the page via Playwright, and asserts DOM state + network behavior.
 *
 * Run:
 *   npx playwright test clock.spec.mjs
 *
 * Artifacts (screenshots) land in ./artifacts/.
 */

import { test, expect } from '@playwright/test';
import http from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer } from 'node:http';
import fs from 'node:fs';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

// ---------------------------------------------------------------------------
// Shared mock server lifecycle
// ---------------------------------------------------------------------------

// We start one server per worker process; port is fixed.
// If multiple workers run, they each get their own port via the env var set
// in globalSetup (or we use a fixed offset).
const BASE_PORT = 9744;

const ARTIFACTS_DIR = path.join(__dirname, 'artifacts');
if (!fs.existsSync(ARTIFACTS_DIR)) {
  fs.mkdirSync(ARTIFACTS_DIR, { recursive: true });
}

// Inline the mock server logic here so the spec is self-contained and
// playwright workers can spin it up without a subprocess.
import { createMockServer } from './mock-server-factory.mjs';

let _server = null;
let _port = null;

function getServerPort() {
  return _port ?? BASE_PORT;
}

test.beforeAll(async () => {
  const result = await createMockServer(BASE_PORT);
  _server = result.server;
  _port = result.port;
});

test.afterAll(async () => {
  await new Promise((resolve) => {
    if (_server) _server.close(resolve);
    else resolve();
  });
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const INGRESS_PREFIX = '/api/hassio_ingress/TESTTOKEN';

function clockUrl(port, path = '', query = '') {
  return `http://127.0.0.1:${port}/clock${path}${query ? '?' + query : ''}`;
}

function ingressUrl(port, path = '', query = '') {
  return `http://127.0.0.1:${port}${INGRESS_PREFIX}/clock${path}${query ? '?' + query : ''}`;
}

async function waitForValueNotDashes(page) {
  await expect(page.locator('#sgv')).not.toHaveText('--', { timeout: 5000 });
}

// ---------------------------------------------------------------------------
// Test 1: Direct load — initial state + zone + SSE update
// ---------------------------------------------------------------------------

test('direct load: value renders with correct zone, SSE update changes value and zone', async ({ page }) => {
  const port = getServerPort();
  const url = clockUrl(port, '', 'lo=70&hi=180&unit=mgdl');

  // Collect all network requests for later inspection
  const requests = [];
  page.on('request', (req) => requests.push(req.url()));

  await page.goto(url);

  // Wait for the initial state fetch to populate the DOM
  await waitForValueNotDashes(page);

  // --- Assertion 1a: value renders (120 from mock state) ---
  const sgv = page.locator('#sgv');
  await expect(sgv).toHaveText('120', { timeout: 5000 });

  // --- Assertion 1b: zone class correct for 120 mg/dL (in_range) ---
  // zone-inrange means dark background + green value text
  await expect(page.locator('body')).toHaveClass(/zone-inrange/, { timeout: 3000 });

  // --- Assertion 1c: trend arrow present (not '--') ---
  const arrow = page.locator('#trend-arrow');
  await expect(arrow).not.toHaveText('--', { timeout: 3000 });
  // Stable trend is '→' (arrow num 4)
  await expect(arrow).toHaveText('→');

  // --- Screenshot: initial state ---
  await page.screenshot({
    path: path.join(ARTIFACTS_DIR, '01-direct-initial.png'),
    fullPage: true,
  });

  // --- Assertion 1d: SSE first reading — value=45, zone=hypo ---
  // The mock emits this at 600ms after connect
  await expect(sgv).toHaveText('45', { timeout: 4000 });
  await expect(page.locator('body')).toHaveClass(/zone-hypo/, { timeout: 3000 });

  // --- Assertion 1e: trend arrow updated (SingleDown = '↓') ---
  await expect(arrow).toHaveText('↓', { timeout: 3000 });

  // --- Screenshot: post-SSE hypo state ---
  await page.screenshot({
    path: path.join(ARTIFACTS_DIR, '02-direct-post-sse-hypo.png'),
    fullPage: true,
  });

  // --- Assertion 1f: SSE second reading — value=210, zone=high ---
  await expect(sgv).toHaveText('210', { timeout: 4000 });
  await expect(page.locator('body')).toHaveClass(/zone-high/, { timeout: 3000 });

  // --- Screenshot: post-SSE high state ---
  await page.screenshot({
    path: path.join(ARTIFACTS_DIR, '03-direct-post-sse-high.png'),
    fullPage: true,
  });
});

// ---------------------------------------------------------------------------
// Test 2: Sparkline — history SVG contains drawn geometry
// ---------------------------------------------------------------------------

test('sparkline: history SVG has drawn geometry after tap', async ({ page }) => {
  const port = getServerPort();
  await page.goto(clockUrl(port, '', 'lo=70&hi=180&unit=mgdl'));
  await waitForValueNotDashes(page);

  // Open detail overlay via a click on the body (mouse fallback in the UI)
  // Wait a moment for history fetch to complete first
  await page.waitForTimeout(500);
  await page.click('body', { position: { x: 200, y: 200 } });

  // Wait for the detail overlay to become visible
  await expect(page.locator('#detail-overlay')).toHaveClass(/visible/, { timeout: 3000 });

  const svg = page.locator('#sparkline-svg');

  // --- Assertion 2a: sparkline SVG has a polyline with points ---
  const polyline = svg.locator('polyline');
  await expect(polyline).toBeVisible({ timeout: 3000 });
  const pointsAttr = await polyline.getAttribute('points');
  expect(pointsAttr).not.toBeNull();
  expect(pointsAttr.trim().length).toBeGreaterThan(10);

  // --- Assertion 2b: sparkline has the in-range band rect ---
  const band = svg.locator('rect').first();
  await expect(band).toBeVisible();

  // --- Assertion 2c: sparkline has the current-value dot (circle) ---
  const dot = svg.locator('circle');
  await expect(dot).toBeVisible();

  // Screenshot
  await page.screenshot({
    path: path.join(ARTIFACTS_DIR, '04-sparkline.png'),
    fullPage: true,
  });
});

// ---------------------------------------------------------------------------
// Test 3: Settings — unit toggle + localStorage PHI guard
// ---------------------------------------------------------------------------

test('settings: unit toggle converts 120 to 6.7 mmol/L; localStorage has no PHI', async ({ page }) => {
  const port = getServerPort();
  await page.goto(clockUrl(port, '', 'lo=70&hi=180&unit=mgdl'));
  await waitForValueNotDashes(page);

  // Ensure value is 120 before toggling
  await expect(page.locator('#sgv')).toHaveText('120', { timeout: 4000 });

  // Open settings via long-press simulation — the UI uses 600ms for non-kiosk
  // Simulate it by directly calling the exposed openSettings function
  // (the settings panel is opened by JS; we trigger it via page.evaluate)
  await page.evaluate(() => {
    // The UI registers openSettings triggered by long-press; we expose
    // window.openSettings as it's defined inside the IIFE. Instead call
    // the btn-mmol directly after forcing settings to open programmatically.
    // The settings panel has onclick attributes we can call.
    document.getElementById('settings-panel').classList.add('visible');
  });

  await expect(page.locator('#settings-panel')).toHaveClass(/visible/, { timeout: 2000 });

  // --- Assertion 3a: click mmol/L button ---
  await page.click('#btn-mmol');

  // --- Assertion 3b: value converts from 120 mg/dL -> 6.7 mmol/L ---
  // 120 / 18.018 = 6.66... -> toFixed(1) = "6.7"
  await expect(page.locator('#sgv')).toHaveText('6.7', { timeout: 3000 });

  // --- Assertion 3c: unit label updates ---
  await expect(page.locator('#glucose-unit')).toHaveText('mmol/L');

  // --- Assertion 3d: localStorage PHI guard ---
  // Only gluco_unit and gluco_theme should be stored — no glucose values,
  // no history, no patient names, no timestamps.
  const lsSnapshot = await page.evaluate(() => {
    const result = {};
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      result[key] = localStorage.getItem(key);
    }
    return result;
  });

  const lsKeys = Object.keys(lsSnapshot);
  console.log('localStorage keys found:', lsKeys);
  console.log('localStorage values:', JSON.stringify(lsSnapshot));

  // Only these two keys are permitted
  const permittedKeys = new Set(['gluco_unit', 'gluco_theme']);
  const forbiddenKeys = lsKeys.filter((k) => !permittedKeys.has(k));
  expect(forbiddenKeys).toEqual([]);

  // Assert the unit was persisted correctly
  expect(lsSnapshot['gluco_unit']).toBe('mmol');

  // Assert values do NOT contain glucose readings or PHI patterns
  for (const [key, value] of Object.entries(lsSnapshot)) {
    // No numeric glucose value stored (would be a number string like "120")
    expect(value).not.toMatch(/^\d{2,3}(\.\d+)?$/, { message: `key "${key}" looks like a glucose value: ${value}` });
    // No timestamp-looking value (epoch ms would be 13 digits)
    expect(value).not.toMatch(/^\d{13}/, { message: `key "${key}" looks like a timestamp: ${value}` });
  }

  // Screenshot
  await page.screenshot({
    path: path.join(ARTIFACTS_DIR, '05-settings-mmol.png'),
    fullPage: true,
  });
});

// ---------------------------------------------------------------------------
// Test 4: E-Ink mode — no opacity/pulse decay, stale-label path active
// ---------------------------------------------------------------------------

test('eink: data-preset=eink set, opacity stays 1, stale-label element present', async ({ page }) => {
  const port = getServerPort();
  await page.goto(clockUrl(port, '', 'eink=1&lo=70&hi=180'));
  await waitForValueNotDashes(page);

  // --- Assertion 4a: html element has data-preset="eink" ---
  const htmlEl = page.locator('html');
  await expect(htmlEl).toHaveAttribute('data-preset', 'eink', { timeout: 3000 });

  // --- Assertion 4b: body has data-ctx="eink" ---
  await expect(page.locator('body')).toHaveAttribute('data-ctx', 'eink');

  // --- Assertion 4c: glucose-value has opacity forced to 1 (no decay) ---
  // In eink mode the CSS rule is: html[data-preset="eink"] .glucose-value { opacity: 1 !important; }
  // We verify the computed style opacity is "1" — rAF decay does not run in eink mode
  const opacity = await page.locator('.glucose-value').evaluate((el) =>
    window.getComputedStyle(el).opacity
  );
  expect(parseFloat(opacity)).toBeCloseTo(1, 1);

  // --- Assertion 4d: no CSS transition on glucose-value (eink disables transitions) ---
  const transition = await page.locator('.glucose-value').evaluate((el) =>
    window.getComputedStyle(el).transition
  );
  // In eink mode transition is "none" (all 0s)
  expect(transition).toMatch(/0s/);

  // --- Assertion 4e: .stale-label element exists in DOM (it's shown in eink ctx) ---
  const staleLabel = page.locator('#stale-label');
  await expect(staleLabel).toBeAttached();
  // It's visible in eink context (display: inline via CSS body[data-ctx="eink"] .stale-label)
  // but may be empty string until 2x pollMs have elapsed — just assert it's in DOM.
  const staleDisplay = await staleLabel.evaluate((el) =>
    window.getComputedStyle(el).display
  );
  expect(staleDisplay).not.toBe('none');

  // Screenshot
  await page.screenshot({
    path: path.join(ARTIFACTS_DIR, '06-eink.png'),
    fullPage: true,
  });
});

// ---------------------------------------------------------------------------
// Test 5: Ingress prefix — URL math is preserved (the #1 risk)
// ---------------------------------------------------------------------------

test('ingress prefix: all clock API requests preserve /api/hassio_ingress/TESTTOKEN/ prefix', async ({ page }) => {
  const port = getServerPort();
  const url = ingressUrl(port);

  const capturedRequests = [];
  page.on('request', (req) => {
    const u = req.url();
    // Capture only requests that are API calls (not the HTML page itself)
    if (u.includes('/clock/state') || u.includes('/clock/events') || u.includes('/clock/history')) {
      capturedRequests.push(u);
    }
  });

  await page.goto(url);

  // Wait for initial value from state endpoint
  await waitForValueNotDashes(page);

  // Give the history fetch time to complete
  await page.waitForTimeout(800);

  console.log('Captured API requests:', capturedRequests);

  // --- Assertion 5a: state request preserves ingress prefix ---
  const stateReqs = capturedRequests.filter((u) => u.includes('/clock/state'));
  expect(stateReqs.length).toBeGreaterThan(0);
  for (const r of stateReqs) {
    expect(r).toContain('/api/hassio_ingress/TESTTOKEN/clock/state');
    // Must NOT be bare /clock/state (dropping the token)
    expect(r).not.toMatch(/^http:\/\/127\.0\.0\.1:\d+\/clock\/state/);
  }

  // --- Assertion 5b: events (SSE) request preserves ingress prefix ---
  const eventsReqs = capturedRequests.filter((u) => u.includes('/clock/events'));
  expect(eventsReqs.length).toBeGreaterThan(0);
  for (const r of eventsReqs) {
    expect(r).toContain('/api/hassio_ingress/TESTTOKEN/clock/events');
    expect(r).not.toMatch(/^http:\/\/127\.0\.0\.1:\d+\/clock\/events/);
  }

  // --- Assertion 5c: history request preserves ingress prefix ---
  const historyReqs = capturedRequests.filter((u) => u.includes('/clock/history'));
  expect(historyReqs.length).toBeGreaterThan(0);
  for (const r of historyReqs) {
    expect(r).toContain('/api/hassio_ingress/TESTTOKEN/clock/history');
    expect(r).not.toMatch(/^http:\/\/127\.0\.0\.1:\d+\/clock\/history/);
  }

  // --- Assertion 5d: paths are /clock/<x> not bare /<x> ---
  for (const r of capturedRequests) {
    const urlObj = new URL(r);
    // Path must start with the ingress prefix
    expect(urlObj.pathname).toMatch(
      new RegExp(`^/api/hassio_ingress/TESTTOKEN/clock/`)
    );
  }

  // Screenshot from ingress URL
  await page.screenshot({
    path: path.join(ARTIFACTS_DIR, '07-ingress.png'),
    fullPage: true,
  });

  // Log the actual URLs for the report
  console.log('\n=== Ingress request URLs observed ===');
  for (const r of capturedRequests) {
    console.log(' ', r);
  }
});
