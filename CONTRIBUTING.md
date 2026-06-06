# Contributing to gluco-hub-rs

## Before you open a PR

Read [DISCLAIMER.md](./DISCLAIMER.md) first. We decline PRs that add clinical features, hosted services, or medical decision logic — those uses are out of scope.

For security-sensitive reports (credential leaks, auth bypass), use [SECURITY.md](./SECURITY.md); do not open a public issue.

## Development setup

You need only Rust ≥ 1.95 (edition 2024) and Cargo.

```bash
git clone https://github.com/micschr0/gluco-hub-rs.git
cd gluco-hub-rs
cargo build --all-features
```

Optional: [go-task](https://taskfile.dev) — run `task` to list the shortcuts.

Run `task setup` once to install the git hooks (`core.hooksPath` → `.githooks`):
a **pre-commit** hook rejects unformatted Rust (`cargo fmt --all --check`) and a
**pre-push** hook runs clippy with warnings-as-errors. They catch the same
failures CI does, but on your machine before the push. Editors that honour
`.editorconfig` / `.vscode/settings.json` also format Rust on save.

## Smoke-test before submitting

No LibreLink Up credentials needed:

```bash
bash scripts/smoke.sh
```

It starts the binary with a mock source, hits all three endpoints, verifies the 503-on-empty-cache path, and confirms graceful shutdown.

## Pre-PR checklist

```bash
cargo fmt --all                                     # format
cargo clippy --all-targets --all-features -- -D warnings  # lint (must be clean)
cargo test --all-features                           # tests (must pass)
cargo deny check                                    # supply-chain gate
bash scripts/smoke.sh                               # end-to-end binary test
```

All five must pass; CI runs the same checks on every PR.

## Commit style

[Conventional Commits](https://www.conventionalcommits.org/) — `type(scope): subject`.

Types used here: `feat`, `fix`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`.  
Scopes match the changed area: `llu`, `mqtt`, `nightscout`, `api`, `config`, `metrics`, `core`.

