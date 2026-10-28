# syntax=docker/dockerfile:1
#
# lethora-server in a container (see docs/dev/server.md). Build from the
# repository root (the whole Cargo workspace is needed to resolve it):
#
#   docker build -f deploy/Dockerfile -t lethora-server .
#
# Run it; LETHORA_PUBLIC_URL (the address the apps use) is required:
#
#   docker run -d --name lethora -p 8740:8740 -v lethora-data:/data \
#     -e LETHORA_PUBLIC_URL=https://sync.example.org lethora-server
#
# The image sets LETHORA_LISTEN and LETHORA_DATA_DIR; environment variables
# win over the config file (/etc/lethora/lethora-server.toml, optional).

FROM rust:1-slim-trixie AS build
WORKDIR /src
# Only what Cargo needs (see Dockerfile.dockerignore): the manifests and
# sources of the workspace members and vendor/loro-internal for
# [patch.crates-io]. Only lethora-server and its dependencies are built.
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p lethora-server \
    && cp target/release/lethora-server /usr/local/bin/lethora-server

FROM debian:trixie-slim
RUN useradd --uid 10001 --user-group --home-dir /data --no-create-home lethora \
    && mkdir -p /data /etc/lethora \
    && chown lethora:lethora /data
COPY --from=build /usr/local/bin/lethora-server /usr/local/bin/lethora-server
ENV LETHORA_LISTEN=0.0.0.0:8740 \
    LETHORA_DATA_DIR=/data \
    RUST_LOG=info
USER lethora
WORKDIR /data
VOLUME /data
EXPOSE 8740
STOPSIGNAL SIGTERM
ENTRYPOINT ["lethora-server"]
CMD ["--config", "/etc/lethora/lethora-server.toml"]
