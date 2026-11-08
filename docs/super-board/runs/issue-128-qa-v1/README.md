# Issue 128 — independent QA, 2026-11-08

PASS: all three ACs, pr-security-review 0 findings. Tested clean head
`ec167afe0805fdceafbddc42a914edb4bc331b4c`; this commit adds evidence only.
The security-label human merge gate remains required after Review.

## Results

| Check | Result | Complete evidence in archive |
|---|---|---|
| Five locked feature/edge graphs | Active WebPKI 0.103.16; legacy 0.102.8 only with MQTT; container maintenance crates absent from production | no-default/default/mqtt-only/all-normal/all-dev.log |
| Five WebPKI advisories and three maintenance entries | Current official sources agree; individual rationales accurately distinguish active paths | sources.md, source-review.log, review.md |
| cargo deny check | All four categories pass; existing duplicate-version warnings remain | deny.log |
| Negative policy controls | Baseline rejects three advisory IDs; removing wrapper rejects 0.102.8; additional unreviewed parent also rejects it | deny-base/deny-no-wrapper/deny-additional-parent.log |
| Formatting, default/all-feature Clippy, all-feature compilation | Exit 0 for each | fmt/clippy-default/clippy-all/check-all.log |
| Production/mock tests | 222 passed, 0 failed, 0 ignored | test-production.log |
| All-feature tests with real containers | 240 passed, 0 failed, 0 ignored; six DNS mappings, zero errors | test-all.log, compose-hosts.log |
| Invalid test-network values | Empty and non-Unicode values explicitly rejected | network-empty/network-nonunicode.log |

The archive preserves full commands/output, negative controls, the corrected CLI
invocation error, input hashes, toolchain/environment, source review, and the
scoped DNS launcher. `SHA256SUMS` verifies extracted files. Its metadata mutant
adds a direct old-WebPKI dependency to test policy; repository inputs were unchanged.
Persistent originals: `$SB_CARD_RUNTIME/logs/qa-20261108`.

## Reviewer rerun

Run from the review worktree with the inherited Cargo/TMP environment:

```sh
GLUCO_HUB_TEST_DOCKER_NETWORK=gluco-super-board-recovery python3 "$SB_CARD_RUNTIME/tmp/run-with-compose-hosts.py" "$(mktemp "$SB_CARD_RUNTIME/logs/compose-hosts-XXXXXX.log")" cargo test --all-features --locked -- --test-threads=1
```

Also rerun `cargo deny check`, `cargo fmt --all -- --check`,
`cargo clippy --all-targets --all-features --locked -- -D warnings`,
`cargo clippy --all-targets -- -D warnings`, `cargo check --all-features --locked`,
and `cargo test --locked --features "source-llu sink-nightscout sink-mqtt mock-source"`.
The launcher only maps new worktree-owned disposable Nightscout containers to
sibling MongoDB IPs. No assertions or service responses are replaced.

## Review and test gaps

`review.md` in the archive contains the per-AC unit/component/e2e ledger,
edge cases, security filtering, and separate Standards/Spec reviews (0 findings).
Inline Code-grounder/Historian confidence: 97/96; minimum 96, threshold 70.
No new High gap in changed behavior. Medium: unchanged PEM SEC1/PKCS1 and some
malformed/empty input cases lack individual tests. Low: this host cannot test
the unset/default Docker bridge branch. No optional project verifier map exists.
All discussion pages were read; no QA finding requires resolution. Existing
author-note-only threads are retained. No UI changes: screenshots do not apply.
