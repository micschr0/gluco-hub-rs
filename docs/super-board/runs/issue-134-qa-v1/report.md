# Issue 134 QA evidence

Product head: `9cf94d754e046de1ecd793702814a7fa67fec30b`. Clean before/after checks.
QA host: Rust 1.98; inherited workspace-backed Cargo cache, target, install root and temporary directory.

## Acceptance ledger

| AC | Unit | Component | End-to-end | Result |
|---|---|---|---|---|
| Explicit, revalidated, compatible selections | n/a: install configuration | Dockerfile:44; Taskfile.yml:12,86,96,102; Renovate regex finds all eight installs | complete container job and local release task below | pass |
| Container uses chef/auditable; release dry run uses selected release without publishing | n/a: external build/release commands | CI workflow:74 and Taskfile.yml:87 | image export succeeds; release dry run exit 0 | pass |

Fresh crates.io API responses in `crates.json`: all selected versions exist and are not yanked. Chef/auditable declare no Rust minimum; actual successful compilation and invocation on the Dockerfile's Rust 1.98 establish compatibility. Release 1.1.7 requires Rust 1.97, below the executing 1.98. These are build tools, not application MSRV dependencies.
[Cargo install documentation](https://doc.rust-lang.org/cargo/commands/cargo-install.html) confirms full versions select exact releases and `--locked` uses packaged dependencies.

## Independent checks

Local tests: cargo fmt --all -- --check && cargo clippy --all-targets --all-features --locked -- -D warnings && cargo check --all-features --locked && XDG_CONFIG_HOME="$SB_CARD_RUNTIME/config" PATH="$CARGO_INSTALL_ROOT/bin:$PATH" task release:dry

All commands exited 0. `local-tests.log.gz` preserves full output: selected cargo-release 1.1.7 is already installed; release executes the lint hook and 208 binary + 9 core tests, then aborts as a dry run. No --execute was passed. Tracked files and HEAD remained unchanged. Runtime-local native CA trust from Builder is retained for proxy trust. Warnings about missing local remote-tracking refs do not cause writes.

[Complete container build](https://github.com/micschr0/gluco-hub-rs/actions/runs/42418317045/job/128541106879) has successful conclusion at the product head. QA retrieved full logs anew: `container.log.gz` lines 819/888 install chef 0.1.78/auditable 0.7.7; lines 906/912 invoke chef prepare/cook; line 1392 invokes auditable build; final image exports successfully. This is actual CI execution, not a local Docker rerun.

## Gap and independent review

Infrastructure seams are real install/build/release commands; both ACs have observable checks. No new unit test or forced mutation is appropriate for this configuration-only change. No High test gaps. Medium: setup installation-failure propagation is inspected as fail-fast Task commands, without inducing a registry outage. Boundary/input/auth/race/UI classes are n/a: no new application inputs or state. Actual tool execution, test results and image export cannot pass by asserting on their own mock. Optional project verifier map absent; ordinary AC checks used.

Standards: 0 findings against repository instructions and CONTRIBUTING; no new application dependencies or secrets. Explicit tool pins are installation selections outside Cargo.toml, covered by Renovate.
Spec: 0 findings. Complete container CI directly serves AC2; setup install failures now propagate.
Security review: 0 findings after tracing the complete six-file diff. No untrusted shell interpolation, permission escalation, credential exposure or new application attack surface. Read-only PR permissions and no image push. False-positive pass excludes trusted version literals and documentation.
Truth code-grounder: No issue, 98/100; checked source lines and actual full logs against both ACs.
Truth historian: No issue, 95/100; blame shows previously unversioned locked installs and existing dry-run/release commands; no ADR or contrary incident found. Minimum 95 exceeds threshold 70. Duties performed inline per Codex lane instruction.
Prior review threads: eight author explanations, no reviewer/QA findings or replies; no QA threads require resolution.
Screenshots do not apply: no UI or visual AC. Compressed logs retain complete output without inflating the code review.
