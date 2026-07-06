FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
      curl \
      git \
      bash \
      direnv \
      ca-certificates \
      openssh-client \
      patch \
      ripgrep && \
    rm -rf /var/lib/apt/lists/*

# Create non-root user matching Kubernetes runAsUser
RUN useradd --create-home --uid 1000 --shell /bin/bash sandbox

# Install Determinate Nix (pinned version for reproducibility)
# Using version 0.16.0 for stability
RUN curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix/tag/v0.16.0 | \
    sh -s -- install linux \
      --extra-conf "sandbox = false" \
      --init none \
      --no-confirm

# Add nix to PATH for all users
ENV PATH="/nix/var/nix/profiles/default/bin:${PATH}"

# Set up direnv hook for bash (auto-loads dev environments on cd)
RUN echo 'eval "$(direnv hook bash)"' >> /home/sandbox/.bashrc && \
    mkdir -p /etc/bash_completion.d && \
    echo 'eval "$(direnv hook bash)"' > /etc/bash_completion.d/direnv.sh && \
    chown -R 1000:1000 /home/sandbox

# Install attic client for Nix binary cache push/pull
# ponytail: using a dedicated profile to avoid polluting the default nix profile;
# if attic-client is missing from nixpkgs, switch to github:zhaofengli/attic#attic-client
RUN nix profile install --profile /nix/var/nix/profiles/sandbox nixpkgs#attic-client
ENV PATH="/nix/var/nix/profiles/sandbox/bin:${PATH}"

# Create workspace directory (init container clones repo here)
RUN mkdir -p /workspace

WORKDIR /workspace

