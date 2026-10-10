# ---------- frontend: static export served by the Rust binary ----------
FROM node:22-alpine AS web
WORKDIR /app/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

# ---------- backend ----------
FROM rust:1.95-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# ---------- runtime ----------
FROM alpine:3.21
RUN apk add --no-cache ca-certificates
WORKDIR /app
COPY --from=build /app/target/release/vane-next /usr/local/bin/vane-next
COPY --from=web /app/web/out /app/web/out

ENV PORT=3001 \
    VANE_DATA_DIR=/data \
    VANE_STATIC_DIR=/app/web/out \
    DATABASE_URL=/data/vane.db \
    RUST_LOG=info
VOLUME /data
EXPOSE 3001
CMD ["vane-next"]
