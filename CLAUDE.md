# gluco-hub

Rust service: polls LibreLink Up, exposes glucose via HTTP, pushes to Nightscout.

## Stack

Rust stable, edition 2024. Tokio + axum. reqwest with rustls (no OpenSSL). thiserror in libs, anyhow at binary boundary. tracing + tracing-subscriber (JSON logs). config + serde (TOML + ENV overrides). secrecy for secret strings. validator for boundary checks. clap for CLI.

Always latest minor versions via Renovate. No exact pins outside `Cargo.lock`.

## Architecture

Workspace, two crates:

- `gluco-hub-core` — domain types, `Source`/`Sink` traits, errors
- `gluco-hub` — binary, axum API, config, wiring

Flow: LibreLink Up → Source poller → in-memory reading cache → fan-out to Nightscout sink and HTTP API.

V1 ships one Source impl (LibreLinkUp) and one Sink impl (Nightscout v3). Trait-based design — adding sources or sinks is a new file plus a Cargo feature, not a refactor. MQTT sink lands in V2.

## Commands

- Build: `cargo build` / `cargo build --release`
- Build with all sinks: `cargo build --features "source-llu sink-nightscout sink-mqtt"`
- Test: `cargo test`
- Test all features: `cargo test --all-features`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Format: `cargo fmt --all`
- Run: `cargo run -- run`
- Run with config: `cargo run -- run -c config.toml`
- Audit: `cargo deny check`
- Container build: `docker build -t gluco-hub:dev -f Containerfile .`

## Conventions

**Errors**: `thiserror` in libraries with named fields and error codes. `anyhow` only in `main.rs`. Never `unwrap()` outside `#[cfg(test)]` — use `expect("reason")` or `?`.

**Types**: Newtypes for all IDs (`PatientId`, `SourceId`, `ApiToken`). Enums for fixed sets (`Region`, `Trend`, `AuthMode`). Never raw `String` for domain IDs, never magic strings for fixed sets.

**Files & state**: Atomic writes via `tempfile`. Paths as `PathBuf`, not `String`. No check-then-act patterns (TOCTOU).

**Logs**: `tracing` only — no `println!` outside `main`. JSON output in production with structured fields like `error_code`. Never log secrets, tokens, or PII.

**Secrets**: Wrap in `SecretString` from `secrecy` crate. TOML references ENV variable names (e.g. `password_env = "LLU_PASSWORD"`), never the secret itself.

**Config ENV overrides**: Any TOML key can be overridden at runtime with `GLUCO_HUB__<SECTION>__<KEY>` (double-underscore delimited), e.g. `GLUCO_HUB__HTTP__BIND=0.0.0.0:9090`. Useful in containers where mounting a config file is inconvenient.

**Async**: `Arc<RwLock<...>>` for read-heavy state (cache), `Arc<Mutex<...>>` for write-heavy. No blocking calls inside async functions.

**Validation**: All config and external input validated at the boundary with `validator`.

## Files map

- `gluco-hub-core/src/{model,source,sink,error,cache,mock}.rs` — domain layer
- `gluco-hub/src/{main,config,metrics}.rs` — binary entry points
- `gluco-hub/src/api/{mod,health,glucose,auth,metrics}.rs` — axum routes
- `gluco-hub/src/sources/llu/` — LibreLink Up source impl
- `gluco-hub/src/sinks/{nightscout,mqtt}/` — Nightscout and MQTT sink impls
- `gluco-hub/src/e2e_tests.rs` — integration tests (wiremock)
- `config.example.toml` — config schema reference
- `docs/ARCHITECTURE.md` — Mermaid data-flow and sequence diagrams

## Don'ts

- No `unwrap()` outside tests
- No `println!` for output — use `tracing`
- No OpenSSL — rustls everywhere
- No new dependencies without `cargo deny check` passing
- No secrets in TOML — only ENV variable names referenced
- No PII in logs — IDs only

## Roadmap

- **V1** ✓: LLU source + Nightscout sink + HTTP API + optional Bearer
- **V2** ✓: MQTT sink (v5, LWT, schema `v: 1`, topics `_health` and `_stats`)
- **V3**: TUI, DLQ, backfill, HA discovery, webhook sink
- **V4**: NS-Socket source, multi-source routing, NS v1 fallback
- **V5**: tailscale-rs embedded, mTLS for MQTT, JWT-as-password

## Agent rules

- Small, focused changes. One concern per task.
- Test error paths explicitly, not just happy paths.
- Run `cargo clippy --all-targets -- -D warnings` before finishing any task.
- New `Source` or `Sink`: own module plus Cargo feature, register in binary wiring.
- Verify external-API claims with the latest official docs — APIs (LibreLink Up, Nightscout v3) evolve.

# Skills

## Claude Code setup

`.claude/` is not committed to this repo. It lives in a separate private
repo (`github.com/micschr0/claude-configs`) and is wired in as a symlink:

```bash
git clone git@github.com:micschr0/claude-configs.git ~/projects/claude-configs
ln -s ~/projects/claude-configs/gluco-hub-rs .claude
```

@.claude/skills/rust-developer/SKILL.md
