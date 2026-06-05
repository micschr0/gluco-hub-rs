/**
 * Factory function that starts the mock server on a given port.
 * Extracted from mock-server.mjs so Playwright workers can spin up their own
 * instance without spawning a subprocess.
 *
 * Returns { server, port } where port is the actual bound port (which may
 * differ from the requested port if a random port was requested with 0).
 */

import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const CLOCK_HTML_PATH = path.resolve(__dirname, '../../../src/api/clock.html');

export const INGRESS_PREFIX = '/api/hassio_ingress/TESTTOKEN';

const DEFAULT_CONFIG = {
  unit: 'mgdl',
  lo: 70,
  hi: 180,
  patientLabel: null,
  pollMs: 60000,
  eink: false,
  dark: null,
};

function buildConfigScript(overrides = {}) {
  const cfg = { ...DEFAULT_CONFIG, ...overrides };
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

function makeState() {
  return {
    value: 120,
    unit: 'mgdl',
    trend: 4,
    trend_label: 'stable',
    delta: 5,
    timestamp_ms: Date.now() - 30_000,
    poll_interval_ms: 60000,
    zone: 'in_range',
    patient_label: null,
    lo: 70,
    hi: 180,
  };
}

function makeHistory() {
  const now = Date.now();
  const base = [80, 85, 90, 95, 100, 105, 110, 115, 118, 120,
                122, 125, 120, 115, 110, 118, 122, 124, 120, 118];
  return base.map((mgdl, i) => ({
    ts: now - (base.length - 1 - i) * 5 * 60 * 1000,
    mgdl,
  }));
}

function startSseStream(res) {
  res.writeHead(200, {
    'Content-Type': 'text/event-stream',
    'Cache-Control': 'no-store',
    'X-Accel-Buffering': 'no',
    'Connection': 'keep-alive',
  });

  const send = (event, data) => {
    if (!res.writableEnded) {
      res.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
    }
  };

  // Immediate keepalive comment so the browser knows the stream is open
  res.write(': keepalive\n\n');

  // First SSE reading: hypo (45 mg/dL, SingleDown)
  const t1 = setTimeout(() => {
    send('reading', {
      value: 45,
      unit: 'mgdl',
      trend: 2,
      trend_label: 'falling',
      delta: -10,
      timestamp_ms: Date.now(),
      zone: 'hypo',
    });
  }, 600);

  // Second SSE reading: high (210 mg/dL, FortyFiveUp)
  const t2 = setTimeout(() => {
    send('reading', {
      value: 210,
      unit: 'mgdl',
      trend: 5,
      trend_label: 'rising',
      delta: 15,
      timestamp_ms: Date.now(),
      zone: 'high',
    });
  }, 1800);

  const ka = setInterval(() => {
    if (!res.writableEnded) res.write(': keepalive\n\n');
  }, 15_000);

  res.on('close', () => {
    clearTimeout(t1);
    clearTimeout(t2);
    clearInterval(ka);
  });
}

function parseQueryOverrides(searchString) {
  const params = new URLSearchParams(searchString);
  const overrides = {};
  if (params.has('lo')) overrides.lo = Number(params.get('lo'));
  if (params.has('hi')) overrides.hi = Number(params.get('hi'));
  if (params.has('eink')) overrides.eink = true;
  if (params.has('unit')) overrides.unit = params.get('unit');
  return overrides;
}

function handleClockRequest(relPath, req, res, queryOverrides = {}) {
  const clean = relPath.split('?')[0];
  if (clean === '/clock' || clean === '/clock/') {
    serveClockHtml(res, queryOverrides);
  } else if (clean === '/clock/state') {
    res.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
    res.end(JSON.stringify(makeState()));
  } else if (clean === '/clock/events') {
    startSseStream(res);
  } else if (clean === '/clock/history') {
    res.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
    res.end(JSON.stringify(makeHistory()));
  } else {
    res.writeHead(404, { 'Content-Type': 'text/plain' });
    res.end(`Not found: ${clean}`);
  }
}

/**
 * Start the mock server.
 * @param {number} port - Port to listen on (use 0 for random).
 * @returns {Promise<{server: http.Server, port: number}>}
 */
export function createMockServer(port = 0) {
  return new Promise((resolve, reject) => {
    const server = http.createServer((req, res) => {
      const urlObj = new URL(req.url, `http://localhost`);
      const pathname = urlObj.pathname;
      const queryOverrides = parseQueryOverrides(urlObj.search.slice(1));

      if (pathname.startsWith(INGRESS_PREFIX)) {
        const relPath = pathname.slice(INGRESS_PREFIX.length) || '/';
        handleClockRequest(relPath, req, res, queryOverrides);
      } else {
        handleClockRequest(pathname, req, res, queryOverrides);
      }
    });

    server.on('error', reject);

    server.listen(port, '127.0.0.1', () => {
      const addr = server.address();
      resolve({ server, port: addr.port });
    });
  });
}
