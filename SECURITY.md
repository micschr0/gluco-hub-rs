# Security Policy

## Supported Versions

Only the current `main` branch is supported. There are no versioned releases yet — security fixes land directly on `main`.

| Version | Supported |
| ------- | :-------: |
| `main`  | ✓         |

## Scope

gluco-hub-rs is a data-relay tool for self-hosting enthusiasts. It is **not** a medical device and does not process personally identifiable health data on behalf of third parties — see [DISCLAIMER.md](./DISCLAIMER.md).

## Reporting a Vulnerability

**For non-sensitive bugs** (wrong HTTP status, config parsing errors, etc.): [open a GitHub issue](https://github.com/micschr0/gluco-hub-rs/issues). Include the error code (e.g. `LLU003`) and the output of `check-config` if relevant.

**For sensitive issues** (credential leakage, auth bypass, secret exposure in logs): email [micschro@mailbox.org](mailto:micschro@mailbox.org) with the subject line `[gluco-hub-rs] Security report`. Do **not** open a public issue.

## What to Include

- `gluco-hub --version` output
- Rust version (`rustc --version`) and OS
- Steps to reproduce
- Potential impact (e.g. "token visible in structured logs")
- Any relevant config keys (redact actual secret values)

## Response Timeline

Sensitive reports will receive an acknowledgement within 7 days. Given the small-team nature of this project, there is no formal CVE process, but fixes will be released promptly and credited in the changelog.
