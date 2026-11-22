# Issue #137 — independent QA evidence

Product head: `570f6f9f616819e1d44dec5f5387167579c535e4`. QA evidence adds documentation only.

AC1 and AC2 passed independently on 2026-11-08, Linux amd64, Rust 1.98.1.
Both fixture references match CI prepulls exactly; independent index inspection
and tag@digest pulls exited 0. Both indexes contain amd64 and arm64.

Official sources rechecked:

- [Nightscout database compatibility](https://nightscout.github.io/nightscout/database/) specifies MongoDB 5+ for Nightscout 15.0.8+.
- [MongoDB lifecycle](https://www.mongodb.com/legal/support-policy/lifecycles) lists 8.0 support through 2029-10-31.
- Official v15.0.7, v15.0.8 and v15.0.9 release objects retained in the recovery artifacts; exact observed tags used.

## Fresh checks

| Command | Result |
|---|---|
| `cargo test --features integration-tests --locked integration_tests::nightscout -- --test-threads=1` | 4 passed, 0 failed; exit 0; 158.05s |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --all-targets -- -D warnings` | exit 0 |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | exit 0 |
| `cargo test --locked --features "source-llu sink-nightscout sink-mqtt mock-source"` | 208 binary + 9 core; exit 0 |
| `cargo check --all-features --locked` | exit 0 |
| `actionlint -shellcheck= .github/workflows/ci.yml` (v1.7.12) | exit 0 |
| `docker compose -f gluco-hub/tests/fixtures/nightscout-compose.yml config --format json` | exit 0; existing obsolete-version warning |

## Container test output

```text
   Compiling gluco-hub-core v2026.621.0 (/home/claude/projects/gluco-hub-rs/.claude/board-run-clone/.claude/worktrees/issue-137-qa/gluco-hub-core)
   Compiling gluco-hub v2026.621.0 (/home/claude/projects/gluco-hub-rs/.claude/board-run-clone/.claude/worktrees/issue-137-qa/gluco-hub)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 20.52s
     Running unittests src/main.rs (/home/claude/projects/gluco-hub-rs/.claude/super-board/runtime/issue-137/target/debug/deps/gluco_hub-9fd6c889c283b8fc)

running 4 tests
test integration_tests::nightscout::duplicate_pushes_deduplicate_in_mongo ... ok
test integration_tests::nightscout::push_persists_reading_in_real_mongo ... ok
test integration_tests::nightscout::pushing_a_batch_of_three_persists_each_distinct_entry ... ok
test integration_tests::nightscout::wrong_api_secret_surfaces_as_sink_error ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 222 filtered out; finished in 158.05s

     Running unittests src/lib.rs (/home/claude/projects/gluco-hub-rs/.claude/super-board/runtime/issue-137/target/debug/deps/gluco_hub_core-c9789b87fc14faa7)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.00s


exit=0
```

## Runner adaptation and cleanup

Used the retained scoped launcher, which maps each newly created Nightscout
container’s mongo hostname to its actual sibling bridge IP on this runner.
No application/fixture/assertion changes. All four stacks cleaned automatically;
post-run Docker query for this worktree fixture returned no containers.
Normal CI needs no workaround. The interrupted old baseline remains separate
and is not claimed as a passing suite.

```text
308c373c9223: mongo mapped to sibling 172.18.0.2 on project 5f2dab83-ff60-465b-8e18-7e18471faadf
c08be82aa104: mongo mapped to sibling 172.18.0.2 on project 9586b89e-227b-4a98-ad24-27f14d111c86
b97500a3bcc0: mongo mapped to sibling 172.18.0.2 on project e340a33e-f6ac-4c76-8159-1fa4b0d1b84a
292e5b8d4cd3: mongo mapped to sibling 172.18.0.2 on project bbd5d5b7-4cbb-468f-92b3-70e3116b7d7a
command-exit=0; mapping-errors=0
```

## Rerun

Run from the reviewed worktree with the inherited issue runtime. Preserve the
committed output above and capture fresh Reviewer stdout separately.

```bash
python3 "$SB_CARD_RUNTIME/tmp/run-with-compose-hosts.py" "$SB_CARD_RUNTIME/logs/qa-v1/nightscout-hosts.log" cargo test --features integration-tests --locked integration_tests::nightscout -- --test-threads=1
```

Complete command outputs, registry evidence, runtime variables and input hashes:
`.claude/super-board/runtime/issue-137/logs/qa-v1/` in the owning workspace.

## Review and test-gap assessment

Product head: 570f6f9f616819e1d44dec5f5387167579c535e4
Fixed point: origin/staging (merge base 583668ec1f6c0ad5e364dbf0422e39f180c8649a)
Scope: 4 additions / 4 deletions, .github/workflows/ci.yml and gluco-hub/tests/fixtures/nightscout-compose.yml.

## Standards
0 findings. Applied user-provided AGENTS.md, authoritative docs/agents/ci.md,
and PROJECT.md affected-check rules. No application changes, dependencies,
migration, or user-visible behavior requiring CHANGELOG. Digests pin multiarch
indexes; existing startup, timeout, isolation and HTTP/auth settings unchanged.
Fowler smell baseline: no new logic, abstractions, or duplicated business logic.
Fixture and CI intentionally reference identical images.

## Spec
0 findings in diff. AC1 observable: exact fixture/prepull equality, tag@digest
registry availability, amd64/arm64 indexes, official compatibility/lifecycle
sources and Compose/actionlint. AC2 observable: all four real-container tests,
each starting a new isolated Nightscout/MongoDB stack. Execution proof separate.
No scope creep. Existing baseline interrupted outputs are not passing proof.

## Test-gap ledger
| AC | Unit | Component | E2E / operational |
|---|---|---|---|
| AC1 available, compatible, consistent pinned images | n/a: external registry / YAML refs | n/a: image selection has no application branch | fresh index inspect + pull; independent literal equality; Compose config/actionlint; official support sources; AC2 runtime |
| AC2 startup and existing error cases | n/a: requires real pair | gluco-hub/src/integration_tests/common/nightscout_container.rs:80 startup and :111 readiness | gluco-hub/src/integration_tests/nightscout.rs:41 persistence, :67 duplicate, :101 wrong secret, :125 batch |

Edges pinned: one persisted reading (61-63), replay duplicate (92-96), many
readings (146-148), wrong secret witness `nope` (109-120), fresh state per case.
Boundaries/unicode/CSV/money/date/UI/accessibility: n/a to fixed image refs.
Images arm64: manifest availability only; runtime exercised linux/amd64.
No newly changed auth/security/data-loss logic. No High gaps; no new tests
needed for image substitutions, per recovery scope and existing container seam.
Medium gaps not written: wrong-secret test accepts any NS00x, so a transport
error could satisfy it; readiness accepts non-401 responses beyond successful
ones, though subsequent persistence cases require actual writes and reads.
Neither is introduced by this diff; no claim of exact 401 assertion.
Tests exercise real API writes/readback rather than their own mock. Dropping a
write / replay dedup / batch item causes literal count/value assertions to fail.

## Security pass
pr-security-review: 0 findings. Inline per Codex adaptation; no agents launched.
Repository context: API authentication and secret wrapping live outside diff;
fixture uses synthetic credentials and disposable data. Comparative analysis:
only image values change, CI commands contain no untrusted interpolation,
permissions/triggers/network exposure unchanged. No new attacker input,
execution sink, authorization bypass, crypto change, or data exposure.
False-positive pass: test-only fixture hardening, pre-existing exposure,
third-party dependency advisory speculation and non-exploitable workflow
concerns are excluded by skill. No candidate meets >80% exploitability.

## Prior threads and truth check
Complete current issue/PR discussion, review threads and review bodies read.
Three unresolved author notes; no [qa] findings, builder requests or human
requests. Pagination complete. Prior findings: none.
Non-trivial truth trigger absent: 8 changed lines and bug label; no auth, money,
destructive schema or migration changes. Required independent review done.
No project-specific verifier map exists in execution checkout; ordinary AC
checks apply. Screenshot evidence n/a: fixture/CI-only, no UI acceptance.
