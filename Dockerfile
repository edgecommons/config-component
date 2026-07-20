# Container image for the EdgeCommons ConfigComponent, for the KUBERNETES (and HOST/Docker) platform.
# Builds from the repo root against the pinned edgecommons git dependency in Cargo.toml — no umbrella
# checkout or sibling COPY is needed:
#   docker build -t <image> .
#
# The cargo git dep needs network + git auth to fetch the private edgecommons repo — pass a GITHUB_TOKEN
# or mount an SSH agent. Then push to your registry (or `kind load docker-image <image>` for a local
# cluster) and set `image:` in k8s/deployment.yaml.

# ---- stage 1: build -------------------------------------------------------------------------
FROM rust:1.85-slim AS build

# Resolve the private edgecommons git dependency using the system git (honours GITHUB_TOKEN / SSH).
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy the crate manifest (+ committed lockfile) and sources, then build the release binary.
COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release --bin config-component

# ---- stage 2: runtime -----------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

COPY --from=build /build/target/release/config-component /usr/local/bin/config-component

USER 65532:65532

ENTRYPOINT ["/usr/local/bin/config-component"]
