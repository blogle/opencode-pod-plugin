FROM rust:1.87-slim AS builder

RUN apt-get update && apt-get install -y musl-tools pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /app
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/opencode-k8s-sandbox-router /usr/local/bin/

EXPOSE 8080 9090

ENTRYPOINT ["opencode-k8s-sandbox-router"]
