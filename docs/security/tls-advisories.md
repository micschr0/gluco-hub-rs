# TLS advisory assessment

Reviewed 2026-11-08 for issue #128, using `Cargo.lock` from base `428b919`,
Cargo 1.98.1, cargo-deny 0.20.2 and RustSec database commit
`309392b18f6b17022f82c701f300433126901068`. No dependency upgrade or client
migration is needed for the active verifier in this lockfile.

## Resolved paths and actual use

| Feature selection | Production TLS dependencies |
|---|---|
| `--no-default-features` | HTTP dependencies include rustls 0.23.46 and WebPKI 0.103.16; no rumqttc or WebPKI 0.102.8 |
| default (`source-llu sink-nightscout`) | Same patched verifier; reqwest handles LLU/Nightscout HTTPS |
| `--no-default-features --features sink-mqtt` | Adds rumqttc 0.25.1; tokio-rustls 0.26.6 uses the same rustls 0.23.46 / WebPKI 0.103.16 |
| `--all-features` | Same TLS versions; dev edges additionally bring testcontainers 0.27.3 and its dependencies |

rumqttc also **directly** depends on WebPKI 0.102.8. It is not pulled in by
rustls 0.23.46. Inspecting the published
[rumqttc 0.25.1 TLS source](https://docs.rs/crate/rumqttc/0.25.1/source/src/tls.rs)
finds its sole `webpki::` reference in `Error::WebPki(#[from] webpki::Error)`.
Neither certificate verification nor CRL parsing calls that old crate.
`tls_connect` delegates to tokio-rustls's `TlsConnector`; the rustls verifier
uses WebPKI 0.103.16.

In `gluco-hub/src/sinks/mqtt/sink.rs`, `build_client` uses
`TlsConfiguration::default()` without mTLS and a custom `ClientConfig` with
mTLS. Both select `TlsConfiguration::Rustls`, native trust roots and the
standard verifier. No custom permissive verifier or certificate pinning exists.
Neither configuration supplies CRLs. TLS disabled means plaintext MQTT;
it does not change what Cargo resolves for the enabled MQTT feature.

## Five WebPKI advisories

All five match the retained 0.102.8 package, but none executes through that
package in rumqttc 0.25.1. The active 0.103.16 verifier is patched for all five.
The `deny.toml` exceptions rely on this source-level distinction, not a claim
that outbound connections, trusted operators or DNS names eliminate exposure.

| Advisory | Stable patched version | Assessment of the active path |
|---|---|---|
| [RUSTSEC-2026-0049](https://rustsec.org/advisories/RUSTSEC-2026-0049.html) | 0.103.10 | CRL distribution-point matching; no CRLs are configured, and the active library is patched |
| [RUSTSEC-2026-0098](https://rustsec.org/advisories/RUSTSEC-2026-0098.html) | 0.103.12 | URI constraints must be rejected even without an API asserting URI names; DNS-only use was not a valid waiver |
| [RUSTSEC-2026-0099](https://rustsec.org/advisories/RUSTSEC-2026-0099.html) | 0.103.12 | Wildcard DNS constraints can affect broker certificates; an operator-controlled hostname is not certificate pinning |
| [RUSTSEC-2026-0104](https://rustsec.org/advisories/RUSTSEC-2026-0104.html) | 0.103.13 | CRL parsing panic precedes signature validation; no CRLs are configured, and the active library is patched |
| [RUSTSEC-2026-0382](https://rustsec.org/advisories/RUSTSEC-2026-0382.html) | 0.103.16 | Distrusted intermediate keys can affect ordinary certificate verification; the active library contains the rejection fix |

The `bans.deny` rule rejects WebPKI below 0.103.16 when any direct parent
other than rumqttc depends on it. This prevents the advisory-ID exceptions
from hiding a downgrade of the active rustls verifier. It does not prove
rumqttc's future source remains unchanged: review that source on every update.
Remove the five exceptions and the wrapper allowance when rumqttc removes or
updates its direct dependency. Reassess if TLS configuration or CRL use changes.

## PEM and container-test maintenance debt

- [RUSTSEC-2025-0134](https://rustsec.org/advisories/RUSTSEC-2025-0134.html):
  rustls-pemfile 2.2.0 is both a direct optional MQTT dependency and a rumqttc
  dependency. Our mTLS path parses **operator-supplied certificate and private
  key PEM**, including SEC1, PKCS#1 and PKCS#8 keys. The old platform-CA-only
  explanation was false. The parser delegates to maintained rustls-pki-types;
  this is a temporary maintenance-risk exception, not an input-safety claim.
  Issue #127 owns direct migration; transitive removal also needs rumqttc work.
- [RUSTSEC-2026-0440](https://rustsec.org/advisories/RUSTSEC-2026-0440.html):
  astral-tokio-tar 0.6.4 comes from testcontainers 0.27.3, including the
  testcontainers-modules 0.15.0 path. It implements container archive copying.
  Our Mosquitto fixture bind-mounts its configuration; no explicit archive-copy
  call was found in our tests. The crate is still compiled for tests, absent
  from the production dependency graph and
  has no maintained patched release. Retain a specific maintenance exception
  until testcontainers replaces it; do not treat container archive input as safe.
- [RUSTSEC-2024-0449](https://rustsec.org/advisories/RUSTSEC-2024-0449.html):
  unsafe-libyaml 0.2.11 comes through serde_yaml 0.9.34+deprecated and
  docker-compose-types 0.23.0, enabled by testcontainers' `docker-compose`
  feature. Nightscout tests parse the checked-in compose fixture. It is
  reachable in tests, absent from production, and has no patched release.
  Retain a separate maintenance exception until the compose parser is replaced.

Dev dependencies are resolved for ordinary `cargo test` even when the
`integration-tests` code is disabled. That feature gates container execution,
not dependency resolution. They also enable rustls's ring provider alongside
production's AWS-LC; the isolated TLS tests explicitly select AWS-LC.
No category-wide advisory suppression is used.
Reassess both test exceptions on testcontainers/compose changes or any new
runtime use; remove them when those packages leave the graph. Their risks remain
accepted maintenance debt, subject to the security-label human merge gate.

## Reproduce the evidence

Run from the repository root; keep `--locked` so the assessed graph cannot drift:

```sh
cargo tree --locked --no-default-features -e normal -i rustls-webpki@0.103.16
cargo tree --locked -e normal -i rustls-webpki@0.103.16
cargo tree --locked --no-default-features --features sink-mqtt -e normal -i rustls-webpki@0.102.8
cargo tree --locked --all-features -i rustls-webpki@0.103.16
cargo tree --locked --all-features -i astral-tokio-tar
cargo tree --locked --all-features -i unsafe-libyaml
cargo tree --locked --all-features -e normal
cargo deny check
cargo test --locked --features sink-mqtt tls_
```

The TLS tests reject an untrusted local broker as `MQTT002`, load an operator
mTLS identity, and reject missing/malformed certificate input and a key file
containing only a certificate. The fixture key is a public test identity, never
a deployment credential. These tests exercise configuration and handshake/error
paths; they are not exploit reproductions for the five upstream advisories.

## Container runner compatibility

The Mosquitto fixture mounts its temporary config **directory**, keeping
`mosquitto.conf` beneath the mount. A single-file mount fails on this gVisor
runner: `O_DIRECTORY` incorrectly succeeds on the file, so Mosquitto/musl
reports it as a directory. The file is visible and readable; the earlier
fixture-visibility diagnosis was incorrect. The existing publish test fails
before this change and passes with the directory mount and named network.

For a runner without Docker's default bridge, select an existing test network:

```sh
GLUCO_HUB_TEST_DOCKER_NETWORK=gluco-super-board-recovery \
  cargo test --all-features --locked -- --test-threads=1
```

This optional variable affects only the Mosquitto test fixture through
[testcontainers 0.27.3 `ImageExt::with_network`](https://docs.rs/crate/testcontainers/0.27.3/source/src/core/image/image_ext.rs).
When unset, Docker's normal network selection is unchanged. Empty/non-Unicode
values fail explicitly. Nightscout retains its isolated Compose network.
No production networking or image selection changes.

This runner also fails Docker service-name lookup (`mongo` gives `EAI_AGAIN`),
while direct sibling-IP TCP succeeds. Its local test launcher maps each new
Nightscout container's `mongo` hostname to the actual MongoDB sibling IP in
`/etc/hosts`, scoped to this worktree's Compose file and project. It changes
only name resolution in disposable containers, without replacing service
responses or assertions. Normal CI needs neither this launcher nor the
network override. The PR handoff records its exact command and retained log.

## Verification status (2026-11-08)

The locked all-feature graph passes cargo-deny 0.20.2 at the RustSec revision
above. The baseline policy fails on 2026-0382, 2026-0440 and 2024-0449; removing
the rumqttc wrapper allowance rejects WebPKI 0.102.8. Formatting, all-feature
Clippy and compilation pass on Rust 1.98.1. The full suite passes 240 tests (231 binary, 9 core), with no failures or
ignored tests, using the network selection and scoped hosts launcher above.
All six Nightscout stacks received the sibling-IP mapping without errors.
The production/mock suite passes 222 tests; the HTTP smoke check also passes.
Earlier interrupted runs remain failure evidence, not reused passing results.
