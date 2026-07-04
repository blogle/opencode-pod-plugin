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
      patch && \
    rm -rf /var/lib/apt/lists/*

# Install Determinate Nix (pinned version for reproducibility)
# Using version 0.16.0 for stability
RUN curl --proto '=https' --tlsv1.2 -sSf -L https://install.determinate.systems/nix | \
    sh -s -- install linux \
      --extra-conf "sandbox = false" \
      --init none \
      --no-confirm \
      --version 0.16.0

# Add nix to PATH for all users
ENV PATH="/nix/var/nix/profiles/default/bin:${PATH}"

# Set up direnv hook for bash (auto-loads dev environments on cd)
RUN echo 'eval "$(direnv hook bash)"' >> /root/.bashrc && \
    mkdir -p /etc/bash_completion.d && \
    echo 'eval "$(direnv hook bash)"' > /etc/bash_completion.d/direnv.sh

# Create workspace directory (init container clones repo here)
RUN mkdir -p /workspace

WORKDIR /workspace

