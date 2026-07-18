# syntax=docker/dockerfile:1.7
FROM nixos/nix:2.30.1
ENV NIX_CONFIG="experimental-features = nix-command flakes"
RUN nix profile install \
      github:NixOS/nixpkgs/a47c123a609287a012dfc44d281de2dd4ed13394#bash \
      github:NixOS/nixpkgs/a47c123a609287a012dfc44d281de2dd4ed13394#cacert \
      github:NixOS/nixpkgs/a47c123a609287a012dfc44d281de2dd4ed13394#coreutils \
      github:NixOS/nixpkgs/a47c123a609287a012dfc44d281de2dd4ed13394#git
WORKDIR /workspace
CMD ["/bin/sh"]
