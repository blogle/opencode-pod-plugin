FROM rust:1.88.0-bookworm AS build
WORKDIR /src
COPY gateway/Cargo.toml gateway/Cargo.lock ./
COPY gateway/src ./src
RUN cargo build --locked --release

FROM debian:12.11-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/opencode-gateway /usr/local/bin/opencode-gateway
USER 65532:65532
ENTRYPOINT ["opencode-gateway"]
