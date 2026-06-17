# syntax=docker/dockerfile:1.24@sha256:87999aa3d42bdc6bea60565083ee17e86d1f3339802f543c0d03998580f9cb89
#
# Multi-stage build for gluco-hub.
#
# Stage 1 ("chef")    installs cargo-chef on the Rust toolchain image.
# Stage 2 ("planner") computes the dependency recipe from the workspace manifests.
# Stage 3 ("build")   cooks dependencies first (cached layer), then builds the binary.
# Stage 4 ("runtime") is a distroless `cc` base (libc + ca-certificates, no shell,
#                     no package manager) running as the bundled non-root user.
#
# Build:
#   docker build -t gluco-hub:dev \
#     --build-arg GLUCO_HUB_GIT_SHA=$(git rev-parse HEAD) \
#     --build-arg BUILD_DATE=$(date -u +%Y-%m-%dT%H:%M:%SZ) .
#
# Run (env-only — config.toml is optional, see compose.example.yml):
#   docker run --rm -p 127.0.0.1:8080:8080 \
#     -e GLUCO_HUB__HTTP__BIND=0.0.0.0:8080 \
#     -e GLUCO_HUB__SOURCE__LLU__EMAIL=you@example.com \
#     -e GLUCO_HUB__SOURCE__LLU__PASSWORD=… \
#     -e GLUCO_HUB__SOURCE__LLU__REGION=EU \
#     -e GLUCO_HUB__SINK__NIGHTSCOUT__BASE_URL=https://ns.example.com \
#     -e GLUCO_HUB__SINK__NIGHTSCOUT__API_SECRET=… \
#     gluco-hub:dev
#
# Or with a file-based config bind-mounted in:
#   docker run --rm -p 127.0.0.1:8080:8080 \
#     -v "$PWD/config.toml:/etc/gluco-hub/config.toml:ro" \
#     -e GLUCO_HUB__SOURCE__LLU__PASSWORD=… \
#     gluco-hub:dev run -c /etc/gluco-hub/config.toml
#
# The image carries no shell — `docker exec` is intentionally limited.
# Health is observable via GET /healthz on port 8080.

# ── Stage 1: chef ─────────────────────────────────────────────────────────────
# renovate: datasource=docker depName=docker.io/library/rust
FROM docker.io/library/rust:1.96.0-bookworm@sha256:19817ead3289c8c631c73df281e18b59b172f6a31f4f563290f69cddd06c30e9 AS chef

# `aws-lc-sys` (a transitive dep of rustls) needs cmake at build time.
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    apt-get update && apt-get install -y --no-install-recommends cmake

RUN cargo install cargo-chef --locked

WORKDIR /src

# ── Stage 2: planner ───────────────────────────────────────────────────────────
FROM chef AS planner

# Copy only the manifests and create minimal stub sources so cargo-chef
# can resolve the full dependency graph without touching real source files.
COPY Cargo.toml Cargo.lock ./
COPY gluco-hub-core/Cargo.toml ./gluco-hub-core/
COPY gluco-hub/Cargo.toml ./gluco-hub/

RUN mkdir -p gluco-hub-core/src gluco-hub/src \
    && echo "pub fn _stub() {}" > gluco-hub-core/src/lib.rs \
    && echo "fn main() {}" > gluco-hub/src/main.rs

RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3: build ─────────────────────────────────────────────────────────────
FROM chef AS build

ARG TARGETPLATFORM
ARG BUILDPLATFORM

# Cook dependencies from the recipe — this layer is only invalidated when
# Cargo.lock / Cargo.toml files change, not on source edits.
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked \
    --features "source-llu sink-nightscout sink-mqtt" \
    --recipe-path recipe.json

# Copy real workspace sources and build the production binary.
COPY Cargo.toml Cargo.lock ./
COPY gluco-hub-core ./gluco-hub-core
COPY gluco-hub ./gluco-hub

# GLUCO_HUB_GIT_SHA is read by option_env!() at compile time.
# Passing it inline in the RUN command avoids an ENV layer in the image.
ARG GLUCO_HUB_GIT_SHA=unknown
RUN GLUCO_HUB_GIT_SHA="${GLUCO_HUB_GIT_SHA}" \
    cargo build --release --locked \
    --features "source-llu sink-nightscout sink-mqtt" \
    --bin gluco-hub

# ── Stage 4: runtime ───────────────────────────────────────────────────────────
# Distroless `cc` provides glibc + ca-certificates; no shell, no package manager.
# renovate: datasource=docker depName=gcr.io/distroless/cc-debian12
FROM gcr.io/distroless/cc-debian12:nonroot

WORKDIR /app

ARG GLUCO_HUB_GIT_SHA=unknown
ARG BUILD_DATE
LABEL org.opencontainers.image.title="gluco-hub" \
      org.opencontainers.image.description="LibreLink Up → HTTP / Nightscout bridge" \
      org.opencontainers.image.source="https://github.com/micschr0/gluco-hub-rs" \
      org.opencontainers.image.url="https://github.com/micschr0/gluco-hub-rs" \
      org.opencontainers.image.revision="${GLUCO_HUB_GIT_SHA}" \
      org.opencontainers.image.created="${BUILD_DATE}" \
      org.opencontainers.image.licenses="AGPL-3.0-or-later"

COPY --from=build /src/target/release/gluco-hub /usr/local/bin/gluco-hub

# `:nonroot` user is uid 65532; documented for orchestrators that
# need to set fsGroup on mounted secrets / config volumes.
USER nonroot:nonroot

EXPOSE 8080

# Exec-form ENTRYPOINT: PID 1 receives SIGTERM directly (no shell wrapper).
# The binary registers tokio SIGTERM + ctrl_c handlers for graceful shutdown.
ENTRYPOINT ["/usr/local/bin/gluco-hub"]
CMD ["run"]

# ── Alpine / musl alternative ─────────────────────────────────────────────────
# Alpine is possible but requires two changes:
#
# 1. Musl target: in the build stage add `musl-tools`, run
#    `rustup target add x86_64-unknown-linux-musl`, and build with
#    `--target x86_64-unknown-linux-musl`.
#
# 2. TLS crypto: aws-lc-sys (default rustls provider) has a complex C build
#    that requires go + perl for musl. Switch to the `ring` backend instead —
#    pure Rust, zero C deps, musl-compatible. In Cargo.toml override the
#    rustls crypto-provider feature; see https://docs.rs/rustls for details.
#
# Once both changes land the runtime stage becomes:
#   FROM alpine:3.21
#   RUN apk add --no-cache ca-certificates
#   COPY --from=build /src/target/x86_64-unknown-linux-musl/release/gluco-hub …
#   USER 65532:65532
