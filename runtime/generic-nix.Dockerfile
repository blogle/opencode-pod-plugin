# syntax=docker/dockerfile:1.7
FROM nixos/nix:2.30.1
ENV NIX_CONFIG="experimental-features = nix-command flakes"
RUN nix profile install \
      github:NixOS/nixpkgs/a47c123a609287a012dfc44d281de2dd4ed13394#bash \
      github:NixOS/nixpkgs/a47c123a609287a012dfc44d281de2dd4ed13394#cacert \
      github:NixOS/nixpkgs/a47c123a609287a012dfc44d281de2dd4ed13394#coreutils \
      github:NixOS/nixpkgs/a47c123a609287a012dfc44d281de2dd4ed13394#git
RUN printf 'experimental-features = nix-command flakes\nbuild-users-group =\nsandbox = false\n' \
      > /etc/nix/nix.conf
RUN install -d -m 0755 /opt/opencode-nix/bin \
    && printf '%s\n' \
      '#!/bin/sh' \
      'unset LD_LIBRARY_PATH' \
      'exec /nix/var/nix/profiles/default/bin/nix "$@"' \
      > /opt/opencode-nix/bin/nix \
    && chmod 0755 /opt/opencode-nix/bin/nix
ENV PATH="/opt/opencode-nix/bin:${PATH}"
RUN for file in passwd group shadow; do \
      source=$(readlink -f "/etc/$file"); \
      cp --remove-destination "$source" "/etc/$file"; \
    done
RUN glibc=$(nix build --no-link --print-out-paths \
      github:NixOS/nixpkgs/a47c123a609287a012dfc44d281de2dd4ed13394#glibc^out) \
    && gcc=$(nix build --no-link --print-out-paths \
      github:NixOS/nixpkgs/a47c123a609287a012dfc44d281de2dd4ed13394#gcc.cc.lib) \
    && mkdir -p /lib64 /lib/x86_64-linux-gnu \
    && ln -s "$glibc/lib/ld-linux-x86-64.so.2" /lib64/ld-linux-x86-64.so.2 \
    && for library in "$glibc"/lib/*.so* "$gcc"/lib/*.so*; do \
         ln -s "$library" "/lib/x86_64-linux-gnu/$(basename "$library")"; \
       done
WORKDIR /workspace
CMD ["/bin/sh"]
