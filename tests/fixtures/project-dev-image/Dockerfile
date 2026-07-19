FROM debian:12.11-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends git python3 procps ripgrep \
    && rm -rf /var/lib/apt/lists/*
RUN for binary in opencode bun direnv; do ! command -v "$binary"; done
WORKDIR /workspace
CMD ["python3", "--version"]
