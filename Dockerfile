# Container image for the EdgeCommons ConfigComponent, for the KUBERNETES (and HOST/Docker) platform.
# Builds from the repo root against the pinned edgecommons git dependency in Cargo.toml — no umbrella
# checkout or sibling COPY is needed.
#
# The cargo git dep fetches the private edgecommons repo, so the build needs a read token. Pass it as a
# BuildKit secret (never baked into an image layer):
#   DOCKER_BUILDKIT=1 docker build --secret id=gh_token,env=GITHUB_TOKEN -t <image> .
# (or mount an SSH agent with `--ssh default` and an SSH git remote). Then push to your registry (or
# `kind load docker-image <image>` for a local cluster) and set `image:` in k8s/deployment.yaml.

# ---- stage 1: build -------------------------------------------------------------------------
# rustc floor: the pinned edgecommons rev's locked dependency tree needs >= 1.86 (the icu_*@2.2.0
# transitive crates reject 1.85), so this base must stay >= 1.86. Pinned at 1.96 to match the sibling
# Rust components' Docker base; bump in lockstep when the pinned rev raises its toolchain floor.
FROM rust:1.96-slim AS build

# Resolve the private edgecommons git dependency using the system git (honours the token / SSH).
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy the crate manifest (+ committed lockfile) and sources, then build the release binary.
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Fetch + build. When a `gh_token` BuildKit secret is present it authenticates the private git fetch;
# the token is read only inside this layer and is never persisted into the image.
RUN --mount=type=secret,id=gh_token \
    sh -c 'if [ -f /run/secrets/gh_token ]; then \
      git config --global url."https://x-access-token:$(cat /run/secrets/gh_token)@github.com/".insteadOf "https://github.com/"; \
    fi' \
    && cargo build --release --bin config-component

# ---- stage 2: runtime -----------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

COPY --from=build /build/target/release/config-component /usr/local/bin/config-component

USER 65532:65532

ENTRYPOINT ["/usr/local/bin/config-component"]
