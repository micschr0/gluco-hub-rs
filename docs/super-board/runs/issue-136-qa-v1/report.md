# Issue 136 QA v1

Verified 2026-11-08 against implementation head `941a796efcb58acd60b8a6c57b467ff09a6c1784`.
PR: https://github.com/micschr0/gluco-hub-rs/pull/145 (base `staging`).
Evidence-only commit follows this tested implementation; no product or test changes.

## Acceptance proof and test plan

| AC | Unit | Component/integration | E2E / observable check | Result |
|---|---|---|---|---|
| Supported Node LTS, compatible Playwright, locked installation | n/a: toolchain selection | `package.json:13,16`, `.node-version:1`, README:87; fresh `npm ci` in `browser.log` | Vendor docs below plus real install under Node 26.13.0 / npm 11.20.0 | Pass |
| CI installs matching Chromium, runs existing behavior successfully | `clockUrl.unit.test.mjs:57` onwards: 9 checks | `.github/workflows/ci.yml:320`: live clock-browser job | `clock.spec.mjs:82,144,185,259,304`: 5 browser tests; matching CLI install; CI success | Pass |

Paths in the ledger are relative to `gluco-hub/tests/e2e/clock/`, except the workflow.
Unit columns were assessed first, then component/integration, then E2E.
The first AC is verified through the actual install boundary, not a configuration-mirroring test.
The second AC is verified through the actual CI job and browser suite.

Local tests: `cd gluco-hub/tests/e2e/clock && npm ci && npm run test:unit && npx playwright install chromium && npm test -- --project=chromium`
Additional required check: `cargo clippy --all-targets -- -D warnings` (exit 0, `clippy.log`).
Browser command exit 0; 9 unit checks and 5 Chromium tests passed (`browser.log`).
Launcher Cargo/runtime environment retained; npm cache and browser path use the issue runtime.
Real clock HTML is served by the local mock HTTP server; LibreLink Up, Nightscout and production services are not exercised.
Server lifecycle is owned by the suite and completed without cleanup failure.

## Revalidated sources and CI

- [Node releases](https://nodejs.org/en/about/previous-releases): Node 26 is listed as LTS.
- [Playwright requirements](https://playwright.dev/docs/intro#system-requirements): latest Node 26.x and Ubuntu 24.04 are supported.
- [Playwright 1.65 release notes](https://playwright.dev/docs/release-notes#version-165): selected supported release exists.
- [Playwright browsers](https://playwright.dev/docs/browsers): use the installed CLI to obtain matching binaries.
- [Clock job 128696182335](https://github.com/micschr0/gluco-hub-rs/actions/runs/42469330402/job/128696182335): SUCCESS on implementation head; independently read metadata and full job log. Node 26.13.0, npm ci, matching Chromium install with OS dependencies, 9 unit checks, 5 browser tests.
- Full CI output retained at workspace runtime `issue-136/qa-ci-clock.log`; local generated reports and screenshots retained in `qa-artifacts/` and `qa-test-results/`.

Screenshot evidence does not apply: the ACs change contributor toolchain/CI behavior, with no UI behavior or layout change. Logs and the actual CI run prove these ACs.

## Test-gap and edge-case check

No High gaps: both ACs have direct observable install/job checks and browser behavior remains asserted.
Boundary checks: Node supported range is >=26 <27; lock selects Playwright 1.65.0; CLI and browser are installed together. Wrong engine rejection is not promised (npm engines is advisory).
Error paths: shell steps propagate failed installs/tests; job timeout bounds hangs; artifact upload runs on failure. No newly introduced catches or application error branches.
Replay/idempotency: npm ci installs the lock graph; CI concurrency cancels superseded PR runs; server closes after tests.
Auth, money, input validation, volume, timezone and accessibility changes: n/a to this infrastructure diff.
Test-the-tests: the changed e-ink assertion reads real computed CSS and checks literal zero duration, avoiding shorthand serialization. It would detect a nonzero duration; no recomputed expected value or own-mock assertion was introduced.

Test gaps (not written): Medium, pre-existing URL unit tests copy production URL logic rather than import it; browser ingress requests independently cover the critical path. Medium, settings test forces the panel open rather than exercising long press. No new input branch requires either gap to be addressed in this issue.
Optional verifier map: absent in the issue checkout; ordinary AC checks used.

## Independent review (inline Codex adaptation)

Standards: 0 findings. Focused harness/CI scope, compatible dependency range with lockfile, SHA-pinned Actions, read-only token, blocked egress; no user-visible behavior requiring a changelog.
Spec: 0 findings. Both ACs satisfied by independent local execution and the live CI job. The longhand assertion preserves the intended no-transition behavior.

pr-security-review: 0 findings. Reviewed all six changed files against staging. CI uses pull_request, read-only contents permission and constant shell commands; no untrusted interpolation, privileged trigger, production secret or new application input path. Security false-positive exclusions applied inline; no candidate met the concrete exploitability threshold of 8/10.

Code-grounder: verified all six files, manifest/runtime/lock agreement, workflow command order, live job and actual assertions. No issue; confidence 98/100.
Historian: inspected the three issue commits and blame for the changed assertion; it preserves the original e-ink intent. The last commit strengthens runner egress; no conflicting ADR or reverted requirement found in the issue scope. No issue; confidence 95/100.
Adversarial aggregate: min(98,95)=95, above configured threshold 70. These are QA checks; Reviewer still owns its independent review and merge gate.

Remote discussion: all seven review threads are author notes, with no replies or lane findings. No unresolved [qa] threads. No prior Reviewer findings to recheck.
