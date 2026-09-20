# Kura daemon image (Rust).
# Build context is the REPO ROOT (needs the crates/ workspace):
#   docker build -f deploy/docker/Dockerfile -t kura-agent .
# or just use deploy/docker/docker-compose.yml.
#
# Runtime base is debian-slim, NOT alpine: the build stage produces a
# glibc-linked binary (the *-unknown-linux-gnu targets that release.yml also
# ships). Copying that binary into a musl image makes the container exit
# immediately with a loader error. Keep both stages on the same libc.

FROM rust:1.85-bookworm AS build
WORKDIR /src/crates
COPY crates/ ./
ARG KURA_VERSION=docker
RUN cargo build --release -p kura-cli

FROM debian:bookworm-slim
ARG KURA_VERSION=docker
# ca-certificates: outbound HTTPS to LLM/providers. wget: container HEALTHCHECK.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates wget \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --create-home --home-dir /home/kura kura \
 && mkdir -p /data && chown kura:kura /data
COPY --from=build /src/crates/target/release/kura /usr/local/bin/kura

ENV KURA_ENV=prod \
    KURA_DATA_DIR=/data \
    KURA_BIND_ADDR=0.0.0.0:19191 \
    KURA_LOG_LEVEL=info \
    KURA_VERSION=${KURA_VERSION}
VOLUME ["/data"]
EXPOSE 19191
USER kura

HEALTHCHECK --interval=30s --timeout=3s --start-period=20s --retries=3 \
  CMD wget -qO- "http://127.0.0.1:19191/healthz" >/dev/null 2>&1 || exit 1

ENTRYPOINT ["/usr/local/bin/kura"]
