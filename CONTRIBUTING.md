# Contributing to gluco-hub-rs

## Before you open a PR

Read [DISCLAIMER.md](./DISCLAIMER.md) first. PRs that add clinical features, hosted services, or medical decision logic will be declined regardless of code quality — those uses are explicitly out of scope.

For security-sensitive reports (credential leaks, auth bypass), see [SECURITY.md](./SECURITY.md) instead of opening a public issue.

## Development setup

Rust ≥ 1.95 (edition 2024) and Cargo are the only hard requirements.

```bash
git clone https://github.com/micschr0/gluco-hub-rs.git
cd gluco-hub-rs
cargo build --all-features
```

Optional: [go-task](https://taskfile.dev) — run `task` to list all shortcuts.

## Smoke-test before submitting

No LibreLink Up credentials needed:

```bash
bash scripts/smoke.sh
```

This starts the binary with a mock source, hits all three endpoints, verifies the 503-on-empty-cache path, and checks graceful shutdown.

## Pre-PR checklist

```bash
cargo fmt --all                                     # format
cargo clippy --all-targets --all-features -- -D warnings  # lint (must be clean)
cargo test --all-features                           # tests (must pass)
cargo deny check                                    # supply-chain gate
bash scripts/smoke.sh                               # end-to-end binary test
```

All five must pass. CI will run the same checks automatically once installed.

## Commit style

[Conventional Commits](https://www.conventionalcommits.org/) — `type(scope): subject`.

Types used here: `feat`, `fix`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`.  
Scopes match the changed area: `llu`, `mqtt`, `nightscout`, `api`, `config`, `metrics`, `core`.

