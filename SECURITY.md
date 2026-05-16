# Security Policy

## Supported Versions

We support only the current `main` branch. No versioned releases exist yet; security fixes land directly on `main`.

| Version | Supported |
| ------- | :-------: |
| `main`  | ✓         |

## Scope

gluco-hub-rs is a data-relay tool for self-hosting enthusiasts. It is **not** a medical device, and it handles health data only for the operator's own use — see [DISCLAIMER.md](./DISCLAIMER.md).

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
