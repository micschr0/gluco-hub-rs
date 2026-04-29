# Multi-stage container build for cgm-bridge.
#
# Stage 1 ("build") produces the release binary against the standard
# Rust toolchain image. Stage 2 ("runtime") is a distroless `cc` base
# (libc + ca-certificates, no shell, no package manager) running as
# the bundled non-root user.
#
# Build:
#   docker build -t cgm-bridge:dev -f Containerfile .
# Run (bind config + secrets):
#   docker run --rm -p 8080:8080 \
#     -v "$PWD/config.toml:/etc/cgm-bridge/config.toml:ro" \
#     -e LLU_PASSWORD=… \
#     -e NIGHTSCOUT_API_SECRET=… \
#     cgm-bridge:dev run -c /etc/cgm-bridge/config.toml
#
# The image carries no shell — `docker exec` is intentionally
# limited. Health is observable via Kubernetes-style probes against
# /healthz on port 8080.

FROM docker.io/library/rust:1-bookworm AS build

WORKDIR /src

# `aws-lc-sys` (a transitive dep of rustls) needs cmake at build time.
RUN apt-get update && apt-get install -y --no-install-recommends \
        cmake \
    && rm -rf /var/lib/apt/lists/*

# Copy the workspace manifest first so layer caching does not break
# on every source edit.
COPY Cargo.toml Cargo.lock ./
COPY cgm-bridge-core/Cargo.toml ./cgm-bridge-core/
COPY cgm-bridge/Cargo.toml ./cgm-bridge/

# Pre-fetch dependencies into the layer cache. `cargo fetch` resolves
# the dep graph from Cargo.lock without running build scripts.
RUN cargo fetch --locked

COPY cgm-bridge-core ./cgm-bridge-core
COPY cgm-bridge ./cgm-bridge

# Build the production binary with the canonical V1 feature set.
ARG CGM_BRIDGE_GIT_SHA=unknown
ENV CGM_BRIDGE_GIT_SHA=${CGM_BRIDGE_GIT_SHA}
RUN cargo build --release --locked \
    --features "source-llu sink-nightscout" \
    --bin cgm-bridge

# ----------------------------------------------------------------------
# Runtime: distroless `cc` for glibc + ca-certificates, no shell.
FROM gcr.io/distroless/cc-debian12:nonroot

WORKDIR /
COPY --from=build /src/target/release/cgm-bridge /usr/local/bin/cgm-bridge

# `:nonroot` user is uid 65532; documented for orchestrators that
# need to set fsGroup on mounted secrets / config volumes.
USER nonroot:nonroot

EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/cgm-bridge"]
CMD ["run"]
