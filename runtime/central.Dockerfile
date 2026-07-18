# The central binary is rebuilt at the exact pin to carry the narrow upstream
# WebSocket target-auth compatibility patch. Child OpenCode remains unmodified.
FROM oven/bun:1.3.14-debian AS build
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates g++ git make python3 \
    && rm -rf /var/lib/apt/lists/*
RUN git clone --filter=blob:none https://github.com/anomalyco/opencode.git /src \
    && git -C /src checkout --detach 127bdb30784d508cc556c71a0f32b508a3061517
WORKDIR /src
COPY runtime/upstream-websocket-auth.patch /tmp/upstream-websocket-auth.patch
COPY runtime/upstream-workspace-warp.patch /tmp/upstream-workspace-warp.patch
COPY runtime/upstream-workspace-timeout.patch /tmp/upstream-workspace-timeout.patch
COPY runtime/upstream-child-session-directory.patch /tmp/upstream-child-session-directory.patch
RUN git apply --check /tmp/upstream-websocket-auth.patch \
    && git apply --check /tmp/upstream-workspace-warp.patch \
    && git apply --check /tmp/upstream-workspace-timeout.patch \
    && git apply --check /tmp/upstream-child-session-directory.patch \
    && git apply /tmp/upstream-websocket-auth.patch \
    && git apply /tmp/upstream-workspace-warp.patch \
    && git apply /tmp/upstream-workspace-timeout.patch \
    && git apply /tmp/upstream-child-session-directory.patch
RUN bun install --frozen-lockfile \
    && OPENCODE_VERSION=1.18.3 bun run --cwd packages/opencode build --single --skip-embed-web-ui --skip-install

FROM node:22.17.1-bookworm-slim AS plugin
WORKDIR /plugin
COPY plugin/package.json plugin/package-lock.json plugin/tsconfig.json ./
COPY plugin/src ./src
RUN npm ci && npm run build && npm prune --omit=dev

FROM debian:12.11-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates git python3 python3-yaml \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/packages/opencode/dist/opencode-linux-x64/bin/opencode /usr/local/bin/opencode
COPY --from=plugin /plugin/dist/ /opt/opencode-plugin/
COPY --from=plugin /plugin/node_modules/ /opt/node_modules/
COPY runtime/central-opencode.json /etc/opencode/opencode.json
RUN test -f /opt/opencode-plugin/plugin.js
ENV OPENCODE_CONFIG=/etc/opencode/opencode.json
ENTRYPOINT ["opencode"]
