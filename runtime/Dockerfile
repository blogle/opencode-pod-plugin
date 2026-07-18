# syntax=docker/dockerfile:1.7
ARG OPENCODE_IMAGE=ghcr.io/blogle/opencode-central:1.18.3-patched-127bdb3

FROM ${OPENCODE_IMAGE} AS opencode

FROM oven/bun:1.3.14-debian AS bun

FROM golang:1.24.5-bookworm AS direnv
RUN git clone --filter=blob:none https://github.com/direnv/direnv.git /src \
    && git -C /src checkout --detach 7590ee2442104060bb11eedebd7bd6daf3d88fcd
WORKDIR /src
RUN CGO_ENABLED=0 go build -trimpath -ldflags='-s -w' -o /direnv ./

FROM rust:1.88.0-bookworm AS supervisor
WORKDIR /src
COPY supervisor/Cargo.toml supervisor/Cargo.lock ./
COPY supervisor/src ./src
RUN cargo build --locked --release

FROM node:22.17.1-bookworm-slim AS plugin
WORKDIR /src
COPY runtime-plugin/package.json runtime-plugin/package-lock.json runtime-plugin/tsconfig.json ./
COPY runtime-plugin/src ./src
RUN npm ci && npm run build && npm prune --omit=dev

FROM debian:12.11-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl git openssh-client \
    && rm -rf /var/lib/apt/lists/*
RUN install -d -m 0755 /opt/opencode/bin /opt/opencode/config/plugins /opt/opencode/config/node_modules /opt/opencode/runtime-plugin
COPY --from=opencode /usr/local/bin/opencode /opt/opencode/bin/opencode
COPY --from=bun /usr/local/bin/bun /opt/opencode/bin/bun
COPY --from=direnv /direnv /opt/opencode/bin/direnv
COPY --from=supervisor /src/target/release/supervisor /opt/opencode/bin/supervisor
COPY --from=plugin /src/dist/ /opt/opencode/runtime-plugin/
COPY --from=plugin /src/node_modules/ /opt/opencode/config/node_modules/
COPY runtime/platform-runtime.js /opt/opencode/config/plugins/platform-runtime.js
COPY runtime/opencode.json /opt/opencode/config/opencode.json
RUN chmod -R a+rX /opt/opencode \
    && test -f /opt/opencode/runtime-plugin/plugin.js \
    && test -f /opt/opencode/config/plugins/platform-runtime.js \
    && /opt/opencode/bin/opencode --version
ENTRYPOINT ["/bin/sh", "-ec"]
CMD ["cp -a /opt/opencode/. /runtime/"]
