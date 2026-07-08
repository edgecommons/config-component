# Build from the EdgeCommons umbrella directory so the sibling core path dependency
# is available:
#   docker build -f config-component/Dockerfile .
FROM rust:1.96-bookworm AS build

WORKDIR /src
COPY core/libs/rust /src/core/libs/rust
COPY core/proto /src/core/proto
COPY config-component /src/config-component
WORKDIR /src/config-component
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=build /src/config-component/target/release/config-component /usr/local/bin/config-component

USER 65532:65532

ENTRYPOINT ["/usr/local/bin/config-component"]
