# syntax=docker/dockerfile:1
# Production image: React SPA (web/dist) embedded into the single Rust binary.
# Build: docker build -t maxi-loop:latest .

# --- 1. Frontend -------------------------------------------------------------
FROM node:22-bookworm-slim AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

# --- 2. Rust binary ----------------------------------------------------------
FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY migrations migrations
# rust-embed reads ../../web/dist at compile time.
COPY --from=web /src/web/dist web/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p maxi-loop \
    && cp target/release/maxi-loop /usr/local/bin/maxi-loop

# --- 3. Runtime --------------------------------------------------------------
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 maxiloop \
    && useradd --system --uid 10001 --gid maxiloop --home-dir /var/lib/maxi-loop --shell /usr/sbin/nologin maxiloop \
    # Spool briefly holds raw, unscrubbed payloads: owner-only (0700).
    && install -d -m 0700 -o maxiloop -g maxiloop /var/lib/maxi-loop /var/lib/maxi-loop/spool
COPY --from=build /usr/local/bin/maxi-loop /usr/local/bin/maxi-loop

USER maxiloop:maxiloop
ENV APP_HOST=0.0.0.0 \
    APP_PORT=8080 \
    LOG_FORMAT=json
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/maxi-loop"]
CMD ["serve"]
