ARG GENERIC_NIX_IMAGE=ocws-e2e-generic-nix:rev2
FROM ${GENERIC_NIX_IMAGE}
COPY tests/fixtures/project/flake.nix tests/fixtures/project/flake.lock /tmp/ocws-nix-fixture/
RUN nix develop path:/tmp/ocws-nix-fixture --command true \
    && rm -rf /tmp/ocws-nix-fixture
