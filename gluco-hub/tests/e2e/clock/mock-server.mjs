/**
 * Mock server for Clock View E2E tests.
 *
 * Serves the real clock.html (unmodified) with window.CLOCK_CONFIG injected
 * exactly like the Rust handler does, plus stub endpoints for clock/state,
 * clock/events (SSE), and clock/history.
 *
 * Endpoints are mounted under BOTH:
 *   - /           (direct access)
 *   - /api/hassio_ingress/TESTTOKEN/   (simulated HA Ingress prefix)
 *
 * Usage:
 *   node mock-server.mjs [port]   (default port: 9744)
 */

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const PORT = parseInt(process.argv[2] ?? '9744', 10);

// Path to the real clock.html — three directories up from tests/e2e/clock/
const CLOCK_HTML_PATH = path.resolve(__dirname, '../../../src/api/clock.html');

// Ingress prefix the Ingress tests mount under
export const INGRESS_PREFIX = '/api/hassio_ingress/TESTTOKEN';

// ---------------------------------------------------------------------------
// Config injected into the HTML, matching the Rust handler format exactly.
// ---------------------------------------------------------------------------
const CLOCK_CONFIG = {
  unit: 'mgdl',
  lo: 70,
  hi: 180,
  patientLabel: null,
  pollMs: 60000,
  eink: false,
  dark: null,
};

function buildConfigScript(overrides = {}) {
  const cfg = { ...CLOCK_CONFIG, ...overrides };
  // Reproduce Rust's injection: window.CLOCK_CONFIG = {unit:"mgdl",lo:70,...}
  // unit is JSON-stringified (produces "mgdl"), booleans/numbers are bare,
  // null is bare null — matching serde_json::to_string and Rust format!().
  const unit = JSON.stringify(cfg.unit);
  const dark = cfg.dark === null ? 'null' : cfg.dark;
  const eink = cfg.eink ? 'true' : 'false';
  return `<script>window.CLOCK_CONFIG = {unit:${unit},lo:${cfg.lo},hi:${cfg.hi},patientLabel:null,pollMs:${cfg.pollMs},eink:${eink},dark:${dark}};</script>`;
}

function serveClockHtml(res, overrides = {}) {
  let html;
  try {
    html = fs.readFileSync(CLOCK_HTML_PATH, 'utf8');
  } catch (err) {
    res.writeHead(500, { 'Content-Type': 'text/plain' });
    res.end(`Cannot read clock.html: ${err.message}`);
    return;
  }

  const inject = buildConfigScript(overrides);
  const injected = html.includes('</head>')
    ? html.replace('</head>', `${inject}</head>`)
    : inject + html;

  res.writeHead(200, {
    'Content-Type': 'text/html; charset=utf-8',
    'Cache-Control': 'no-store',
  });
  res.end(injected);
}

// ---------------------------------------------------------------------------
// Fake data generators
// ---------------------------------------------------------------------------

function makeState() {
  const now = Date.now();
  return {
    value: 120,
    unit: 'mgdl',
    trend: 4,
    trend_label: 'stable',
    delta: 5,
    timestamp_ms: now - 30_000,
    poll_interval_ms: 60000,
    zone: 'in_range',
    patient_label: null,
    lo: 70,
    hi: 180,
  };
}

function makeHistory() {
  const now = Date.now();
  // 20 points at 5-minute intervals going backwards
  const points = [];
  const base = [80, 85, 90, 95, 100, 105, 110, 115, 118, 120,
                122, 125, 120, 115, 110, 118, 122, 124, 120, 118];
  for (let i = 0; i < base.length; i++) {
    points.push({
      ts: now - (base.length - 1 - i) * 5 * 60 * 1000,
      mgdl: base[i],
    });
  }
  return points;
}

// SSE event sequence: first a hypo reading (value 45), then a high reading (210)
// Emitted with ~600ms delay to give Playwright time to set up the listener.
function startSseStream(res) {
  res.writeHead(200, {
    'Content-Type': 'text/event-stream',
    'Cache-Control': 'no-store',
    'X-Accel-Buffering': 'no',
    'Connection': 'keep-alive',
  });

  const send = (event, data) => {
    res.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
  };

  // Send a keepalive comment immediately so the client knows the stream opened
  res.write(': keepalive\n\n');

  // First reading: hypo (value 45, zone hypo, trend falling)
  const t1 = setTimeout(() => {
    send('reading', {
      value: 45,
      unit: 'mgdl',
      trend: 2,            // SingleDown
      trend_label: 'falling',
      delta: -10,
      timestamp_ms: Date.now(),
      zone: 'hypo',
    });
  }, 600);

  // Second reading: high (value 210)
  const t2 = setTimeout(() => {
    send('reading', {
      value: 210,
      unit: 'mgdl',
      trend: 5,            // FortyFiveUp
      trend_label: 'rising',
      delta: 15,
      timestamp_ms: Date.now(),
      zone: 'high',
    });
  }, 1800);

  // Keep the stream alive with a keepalive event every 15s
  const ka = setInterval(() => {
    if (!res.writableEnded) {
      res.write(': keepalive\n\n');
    }
  }, 15_000);

  res.on('close', () => {
    clearTimeout(t1);
    clearTimeout(t2);
    clearInterval(ka);
  });
}

// ---------------------------------------------------------------------------
// Request router
// ---------------------------------------------------------------------------

/**
 * Handle a request given its path relative to the mount prefix.
 * `relPath` is the part after the prefix, starting with '/'.
 */
function handleClockRequest(relPath, req, res, queryOverrides = {}) {
  // Strip query string from relPath (already parsed out)
  if (relPath === '/clock' || relPath === '/clock/') {
    serveClockHtml(res, queryOverrides);
  } else if (relPath === '/clock/state') {
    const body = JSON.stringify(makeState());
    res.writeHead(200, {
      'Content-Type': 'application/json',
      'Cache-Control': 'no-store',
    });
    res.end(body);
  } else if (relPath === '/clock/events') {
    startSseStream(res);
  } else if (relPath === '/clock/history') {
    const body = JSON.stringify(makeHistory());
    res.writeHead(200, {
      'Content-Type': 'application/json',
      'Cache-Control': 'no-store',
    });
    res.end(body);
  } else {
    res.writeHead(404, { 'Content-Type': 'text/plain' });
    res.end(`Not found: ${relPath}`);
  }
}

function parseQueryOverrides(rawQuery) {
  const params = new URLSearchParams(rawQuery ?? '');
  const overrides = {};
  if (params.has('lo')) overrides.lo = Number(params.get('lo'));
  if (params.has('hi')) overrides.hi = Number(params.get('hi'));
  if (params.has('eink')) overrides.eink = true;
  if (params.has('unit')) overrides.unit = params.get('unit');
  return overrides;
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url, `http://localhost:${PORT}`);
  const pathname = url.pathname;
  const queryOverrides = parseQueryOverrides(url.search.slice(1));

  // Ingress prefix route
  if (pathname.startsWith(INGRESS_PREFIX)) {
    const relPath = pathname.slice(INGRESS_PREFIX.length) || '/';
    handleClockRequest(relPath, req, res, queryOverrides);
    return;
  }

  // Direct (root-mounted) route
  handleClockRequest(pathname, req, res, queryOverrides);
});

server.listen(PORT, '127.0.0.1', () => {
  console.log(`mock-server listening on http://127.0.0.1:${PORT}`);
  console.log(`  Direct:  http://127.0.0.1:${PORT}/clock`);
  console.log(`  Ingress: http://127.0.0.1:${PORT}${INGRESS_PREFIX}/clock`);
});

server.on('error', (err) => {
  console.error('mock-server error:', err.message);
  process.exit(1);
});

export { PORT, server };
