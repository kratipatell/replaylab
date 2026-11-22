FROM rust:1-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY engine/Cargo.toml ./engine/Cargo.toml
RUN mkdir -p engine/src \
    && printf 'fn main() {}\n' > engine/src/main.rs \
    && cargo build --release -p engine

COPY engine/src ./engine/src
COPY engine/templates.json ./engine/templates.json
RUN touch engine/src/main.rs && cargo build --release -p engine

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 replaylab \
    && useradd --system --uid 10001 --gid replaylab --home-dir /nonexistent replaylab \
    && mkdir -p /data/cache \
    && chown -R replaylab:replaylab /data

COPY --from=builder --chown=replaylab:replaylab /app/target/release/engine /usr/local/bin/engine

ENV HOST=0.0.0.0 \
    PORT=8080 \
    CACHE_DIR=/data/cache \
    DB_PATH=/data/runs.db

VOLUME ["/data"]
EXPOSE 8080
USER replaylab:replaylab

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl --fail --silent http://127.0.0.1:8080/health || exit 1

ENTRYPOINT ["/usr/local/bin/engine"]
