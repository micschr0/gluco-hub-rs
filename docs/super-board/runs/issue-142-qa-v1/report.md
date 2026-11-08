# Issue #142 — independent QA evidence

Product head: `276ce03ca80fec78303f18935f92c0c4ea034a73`.
PR #146 targets `staging`. This QA commit adds evidence documentation only.

## Acceptance proof

| AC | Result | Evidence |
|---|---|---|
| Compatible, non-yanked uuid patch; no exact manifest pin | Pass | `gluco-hub/Cargo.toml:85` keeps the dev-dependency requirement at `1`. `Cargo.lock` resolves `1.29.1` with the current official crates.io sparse-index checksum. The live index lists it as non-yanked, Rust 1.89.0 minimum, and the latest non-yanked stable uuid 1.x. Check script: `docs/super-board/runs/issue-142-qa-v1/check-uuid-registry.py`; official source: https://index.crates.io/uu/id/uuid. |
| Relevant tests, MSRV and cargo-deny pass | Pass | The production-feature test suite passed (213 binary + 9 core tests); `cargo +1.95 check --all-features --locked` passed; `cargo deny check` passed all advisory, ban, license and source checks. The required repository Clippy gate also passed. |

The PR diff at the tested product head changes only the uuid lock entry version and checksum. All other lock entries and the manifest requirement remain unchanged. No application API or runtime behavior changed.

## Fresh checks

| Exact command | Result |
|---|---|
| `cargo test --locked --features 'source-llu sink-nightscout sink-mqtt mock-source'` | 222 passed, 0 failed; exit 0 |
| `cargo +1.95 check --all-features --locked` | exit 0 |
| `cargo deny check` | advisories, bans, licenses and sources OK; exit 0 |
| `cargo clippy --all-targets --all-features --locked -- -D warnings` | exit 0 |
| `python3 docs/super-board/runs/issue-142-qa-v1/check-uuid-registry.py` | latest stable 1.x, non-yanked, Rust 1.89.0, checksum match; exit 0 |

Local tests: `cargo test --locked --features 'source-llu sink-nightscout sink-mqtt mock-source'`

Complete command outputs are retained under `.claude/super-board/run-artifacts/executions/`:

- Registry: see the execution logs recorded in `.claude/super-board/runtime/issue-142/checkpoint.json` and `.claude/super-board/run-artifacts/executions/`; the committed script is rerunnable from the repository root.
- Production-feature suite: `c8da741063954de88699325b2484670a.log`.
- Rust 1.95 check: `e3539bb8dd224159a4b5cc6f4995e5f5.log`.
- cargo-deny: `eaa6740aefec4471b4ed400cd7648eb7.log`.
- Clippy: `a57703e8bd6d4651b0334b28bee97611.log`.

The first MSRV invocation could not create a rustup temp file in the read-only host home. Rerunning with the card runtime's writable `RUSTUP_HOME` passed. The initial registry script attempts looked in the wrong manifest section; the corrected script checks `gluco-hub`'s dev-dependency and passed. These setup attempts and their complete logs remain retained; neither is an acceptance failure.

Screenshot evidence does not apply: this is a lockfile and dependency-policy change with no UI or visual acceptance criteria.

## Test-gap ledger

| AC | Unit | Component | E2E / operational |
|---|---|---|---|
| AC1: compatible lock resolution | Direct manifest/lock/index check; no application behavior assertion applies | n/a: Cargo dependency resolution is the boundary | n/a: no runtime behavior changed |
| AC2: tests, MSRV and cargo-deny | Full production-feature suite: 213 binary + 9 core tests | `cargo +1.95 check --all-features --locked` and `cargo deny check` | n/a: dependency-only change; full suite completed without failures |

`--locked` makes the test and MSRV commands fail if Cargo needs to alter the committed resolution. The suite does not assert the uuid registry yanked flag or checksum; the independent official-index check covers those values directly. No High gaps were found, and no new application tests are needed for a version/checksum-only lockfile change. Input, auth, date, volume, race, UI and accessibility cases are not applicable to this diff.

## Independent review

**Standards: 0 findings.** The manifest requirement remains a compatible range, with the selected resolution recorded in `Cargo.lock`. The change follows the lockfile-only scope; no changelog or behavior update applies.

**Spec: 0 findings.** The issue asks for a refreshed compatible uuid patch and passing dependency/build checks. The live registry, manifest and lockfile agree, and every requested check passed. No unrelated lock entries changed.

**Security pass:** `pr-security-review: 0 findings`. The PR adds no executable code, input path, authorization behavior, cryptography or data exposure. The dependency patch is the current non-yanked release, and its index checksum matches the lockfile.

**Code-grounder pass:** confirmed the full diff, manifest requirement, registry record and check outputs against the ACs; score 98/100.

**Historian pass:** issue sources #123 and #119 establish this maintenance scope. The earlier cargo-deny blocker was tied to advisories on the old base; prerequisite #128 is merged, and the current base and branch both pass cargo-deny. Current discussion contains no contradictory request; score 96/100.

Truth-check minimum score: 96/100, above configured threshold 70. Review threads contain one unresolved Builder file author note on `Cargo.lock`; it is not a finding. There are no open `[qa]` findings or human requests. No prior Reviewer findings require revalidation.
