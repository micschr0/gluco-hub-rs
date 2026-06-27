# Security Policy

## Supported Versions

We support only the current `main` branch. No versioned releases exist yet; security fixes land directly on `main`.

| Version | Supported |
| ------- | :-------: |
| `main`  | ✓         |

## Scope

gluco-hub-rs is a data-relay tool for self-hosting enthusiasts. It is **not** a medical device, and it handles health data only for the operator's own use — see [DISCLAIMER.md](./DISCLAIMER.md).

## Auditable Binaries

Release binaries and container images embed a dependency list via [`cargo-auditable`](https://github.com/rust-secure-code/cargo-auditable). This lets you audit the binary directly:

```bash
# With cargo-audit
cargo audit bin /usr/local/bin/gluco-hub

# With grype (container image)
grype ghcr.io/micschr0/gluco-hub:latest

# With trivy
trivy image ghcr.io/micschr0/gluco-hub:latest
```

The embedded data uses < 4 kB and contains crate names + versions only — no paths, URLs, or timestamps. It does **not** prevent supply-chain attacks; use [`cargo-vet`](https://github.com/mozilla/cargo-vet) for that.

## Reporting a Vulnerability

**For non-sensitive bugs** (wrong HTTP status, config parsing errors, unclear messages): [open a GitHub issue](https://github.com/micschr0/gluco-hub-rs/issues). Include the error code (e.g. `LLU003`) and the output of `check-config` if relevant.

**For sensitive issues** (credential leakage, auth bypass, secret exposure in logs): use GitHub's [private vulnerability reporting](https://github.com/micschr0/gluco-hub-rs/security/advisories/new). Do **not** open a public issue.

## What to Include

- `gluco-hub --version` output
- Rust version (`rustc --version`) and OS
- Steps to reproduce
- Potential impact (e.g. "token visible in structured logs")
- Any relevant config keys (redact actual secret values)

## Response Timeline

We acknowledge sensitive reports within 7 days. This small-team project has no formal CVE process, but we release fixes promptly and credit reporters in the changelog.
