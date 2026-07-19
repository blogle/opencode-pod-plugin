FROM debian:12.11-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends git python3 \
    && rm -rf /var/lib/apt/lists/*
COPY tests/fixtures/project/ /tmp/project/
COPY tests/fixtures/git-server/server.py /usr/local/bin/git-http-server
RUN git init --initial-branch=main /tmp/project \
    && git -C /tmp/project config user.name "Fixture Author" \
    && git -C /tmp/project config user.email fixture@example.test \
    && git -C /tmp/project add --all \
    && git -C /tmp/project commit -m "deterministic fixture" \
    && git clone --bare /tmp/project /srv/git/fixture.git \
    && git -C /srv/git/fixture.git config http.receivepack false \
    && rm -rf /tmp/project
EXPOSE 8000
ENTRYPOINT ["python3", "/usr/local/bin/git-http-server"]
