import { defineConfig, devices } from '@playwright/test';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  testDir: __dirname,
  testMatch: '**/*.spec.mjs',
  timeout: 30_000,
  retries: 0,
  workers: 1,
  reporter: [
    ['list'],
    ['html', { outputFolder: path.join(__dirname, 'artifacts', 'report'), open: 'never' }],
  ],
  use: {
    // Capture trace on first retry; screenshots on failure
    trace: 'on-first-retry',
    screenshot: 'only-on-failure',
    video: 'off',
    // Viewport large enough to trigger 'wall' context in the UI
    viewport: { width: 1280, height: 800 },
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
});
