# ---------- frontend: static export served by the Rust binary ----------
FROM node:22-alpine AS web
WORKDIR /app/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

# ---------- backend ----------
# glibc, not musl: the Obscura release binary needs it, and it runs in this image.
FROM rust:1.95-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# ---------- runtime ----------
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
ARG OBSCURA_VERSION=v0.2.5
RUN set -eu; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      amd64) cpu=x86_64 ;; \
      arm64) cpu=aarch64 ;; \
      *) echo "unsupported architecture: $arch" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/obscura.tar.gz \
      "https://github.com/h4ckf0r0day/obscura/releases/download/${OBSCURA_VERSION}/obscura-${cpu}-linux-stealth.tar.gz"; \
    tar -xzf /tmp/obscura.tar.gz -C /usr/local/bin; \
    chmod 755 /usr/local/bin/obscura /usr/local/bin/obscura-worker; \
    test -x /usr/local/bin/obscura; \
    rm -f /tmp/obscura.tar.gz
WORKDIR /app
COPY --from=build /app/target/release/libra /usr/local/bin/libra
COPY --from=web /app/web/out /app/web/out

ENV PORT=3001 \
    LIBRA_DATA_DIR=/data \
    LIBRA_STATIC_DIR=/app/web/out \
    DATABASE_URL=/data/libra.db \
    OBSCURA_BIN=/usr/local/bin/obscura \
    RUST_LOG=info
VOLUME /data
EXPOSE 3001
CMD ["libra"]
