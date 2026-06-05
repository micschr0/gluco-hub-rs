/**
 * Deterministic Node.js unit test for the clockUrl() / CLOCK_BASE logic
 * extracted verbatim from clock.html.
 *
 * This test covers the #1 project risk (HA Ingress URL resolution) without
 * requiring a real browser. It runs via:
 *   node clockUrl.unit.test.mjs
 *
 * All assertions use the Node assert module — zero dependencies.
 */

import assert from 'node:assert/strict';

// ---------------------------------------------------------------------------
// Replicate the EXACT logic from clock.html (do NOT modify to match tests).
// If this breaks, the source UI may have a bug.
// ---------------------------------------------------------------------------

function makeCLOCK_BASE(pathname) {
  let p = pathname.replace(/\/clock\/?$/, '');
  if (!p.endsWith('/')) p += '/';
  return p;
}

function clockUrl(CLOCK_BASE, origin, subPath) {
  return new URL(CLOCK_BASE + subPath, origin).toString();
}

// ---------------------------------------------------------------------------
// Test cases
// ---------------------------------------------------------------------------

let passed = 0;
let failed = 0;

function runCase(label, fn) {
  try {
    fn();
    console.log(`  PASS  ${label}`);
    passed++;
  } catch (err) {
    console.error(`  FAIL  ${label}`);
    console.error(`        ${err.message}`);
    failed++;
  }
}

const ORIGIN = 'http://127.0.0.1:9744';

// --- Case 1: Ingress prefix path (the critical production case) ---
runCase(
  'Ingress prefix /api/hassio_ingress/TESTTOKEN/clock -> state URL preserves prefix',
  () => {
    const base = makeCLOCK_BASE('/api/hassio_ingress/TESTTOKEN/clock');
    assert.equal(base, '/api/hassio_ingress/TESTTOKEN/');
    const url = clockUrl(base, ORIGIN, 'clock/state');
    assert.equal(url, `${ORIGIN}/api/hassio_ingress/TESTTOKEN/clock/state`);
  }
);

runCase(
  'Ingress prefix /api/hassio_ingress/TESTTOKEN/clock -> events URL preserves prefix',
  () => {
    const base = makeCLOCK_BASE('/api/hassio_ingress/TESTTOKEN/clock');
    const url = clockUrl(base, ORIGIN, 'clock/events');
    assert.equal(url, `${ORIGIN}/api/hassio_ingress/TESTTOKEN/clock/events`);
  }
);

runCase(
  'Ingress prefix /api/hassio_ingress/TESTTOKEN/clock -> history URL preserves prefix',
  () => {
    const base = makeCLOCK_BASE('/api/hassio_ingress/TESTTOKEN/clock');
    const url = clockUrl(base, ORIGIN, 'clock/history');
    assert.equal(url, `${ORIGIN}/api/hassio_ingress/TESTTOKEN/clock/history`);
  }
);

// --- Case 2: Ingress prefix with trailing slash ---
runCase(
  'Ingress prefix with trailing slash /api/hassio_ingress/TESTTOKEN/clock/ -> state',
  () => {
    const base = makeCLOCK_BASE('/api/hassio_ingress/TESTTOKEN/clock/');
    assert.equal(base, '/api/hassio_ingress/TESTTOKEN/');
    const url = clockUrl(base, ORIGIN, 'clock/state');
    assert.equal(url, `${ORIGIN}/api/hassio_ingress/TESTTOKEN/clock/state`);
  }
);

// --- Case 3: Direct root /clock (no ingress prefix) ---
runCase(
  'Direct /clock -> base is /',
  () => {
    const base = makeCLOCK_BASE('/clock');
    assert.equal(base, '/');
  }
);

runCase(
  'Direct /clock -> state is /clock/state',
  () => {
    const base = makeCLOCK_BASE('/clock');
    const url = clockUrl(base, ORIGIN, 'clock/state');
    assert.equal(url, `${ORIGIN}/clock/state`);
  }
);

runCase(
  'Direct /clock/ -> state is /clock/state',
  () => {
    const base = makeCLOCK_BASE('/clock/');
    const url = clockUrl(base, ORIGIN, 'clock/state');
    assert.equal(url, `${ORIGIN}/clock/state`);
  }
);

// --- Case 4: A deeper ingress token (variable-length tokens) ---
runCase(
  'Longer token /api/hassio_ingress/abc123def456/clock -> prefix preserved',
  () => {
    const base = makeCLOCK_BASE('/api/hassio_ingress/abc123def456/clock');
    assert.equal(base, '/api/hassio_ingress/abc123def456/');
    const url = clockUrl(base, ORIGIN, 'clock/state');
    assert.equal(url, `${ORIGIN}/api/hassio_ingress/abc123def456/clock/state`);
  }
);

// --- Case 5: Bare /clock/state should NEVER be produced from Ingress path ---
runCase(
  'Ingress path MUST NOT produce bare /clock/state (regression)',
  () => {
    const base = makeCLOCK_BASE('/api/hassio_ingress/TESTTOKEN/clock');
    const url = clockUrl(base, ORIGIN, 'clock/state');
    // The URL must NOT be just /clock/state at root (dropped prefix)
    assert.notEqual(url, `${ORIGIN}/clock/state`);
  }
);

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

console.log('');
console.log(`clockUrl unit tests: ${passed} passed, ${failed} failed`);

if (failed > 0) {
  process.exit(1);
}
