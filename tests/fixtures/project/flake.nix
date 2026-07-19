{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/a47c123a609287a012dfc44d281de2dd4ed13394";

  outputs = { nixpkgs, ... }: {
    devShells.x86_64-linux.default = nixpkgs.legacyPackages.x86_64-linux.mkShell {
      NIX_FIXTURE_VERSION = "one";
    };
  };
}
