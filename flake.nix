{
  description = "opencode-k8s-sandbox dev environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        rustToolchain = pkgs.rust-bin.stable."1.88.0".default.override {
          targets = [ "x86_64-unknown-linux-musl" ];
        };
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = [
            rustToolchain
            pkgs.nodejs
            pkgs.kind
            pkgs.kubectl
            pkgs.kustomize
            pkgs.websocat
            pkgs.just
            pkgs.jq
            pkgs.git
            pkgs.python3
            pkgs.docker-client
            pkgs.pkg-config
            pkgs.openssl
          ];

          shellHook = ''
            echo "opencode-k8s-sandbox dev shell loaded"
            echo "  Rust: $(rustc --version)"
            echo "  Node: $(node --version)"
            echo "  kind: $(kind version 2>/dev/null || echo 'not found')"
          '';
        };
      });
}
