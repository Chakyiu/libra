# ---------- frontend: static export served by the Rust binary ----------
FROM node:22-alpine AS web
WORKDIR /app/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

# ---------- backend ----------
# glibc, not musl: the Obscura release binary needs it, and it runs in this image.
# Keep this tag in step with `dtolnay/rust-toolchain@1.100.0` in
# .github/workflows/ci.yml: CI tests the toolchain this image ships. Move both together.
FROM rust:1.100-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

# ---------- runtime ----------
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
# Obscura is pinned to a release and checked against the sha256 written down here:
# upstream publishes no checksum file for v0.2.5, so the sums below were taken from
# the assets themselves. Bump the version and both sums together.
ARG OBSCURA_VERSION=v0.2.5
RUN set -eu; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      amd64) cpu=x86_64; \
             sum=d9175cf7573d0243fc303a6f1a3b68affc757af0b5549cacb018e9670988316c ;; \
      arm64) cpu=aarch64; \
             sum=e28ed5ff2dbabe8d820c7b1ef56e6005399aff15912c7774777719e66a021a32 ;; \
      *) echo "unsupported architecture: $arch" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/obscura.tar.gz \
      "https://github.com/h4ckf0r0day/obscura/releases/download/${OBSCURA_VERSION}/obscura-${cpu}-linux-stealth.tar.gz"; \
    echo "${sum}  /tmp/obscura.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/obscura.tar.gz -C /usr/local/bin; \
    chmod 755 /usr/local/bin/obscura /usr/local/bin/obscura-worker; \
    test -x /usr/local/bin/obscura; \
    rm -f /tmp/obscura.tar.gz
WORKDIR /app
# microsandbox: the Automator's command jobs start one disposable microVM per run through
# this CLI. Needs /dev/kvm from the environment at runtime (see docker-compose.yml) —
# without it those jobs record an error run, and prompt jobs are unaffected.
#
# Pinned release, not https://install.microsandbox.dev: that script always installs the
# newest release and cannot be verified before it runs. This mirrors what the installer
# does (msb into $MSB_HOME/bin, libkrunfw into $MSB_HOME/lib beside it, so msb finds the
# library through its own ../lib lookup) but downloads a fixed asset whose sha256 is in
# this file. The sums are v0.7.16's published checksums.sha256, re-checked on 2026-11-21;
# bump MSB_VERSION and both sums together.
#
# The runtime image runs as an unprivileged user (`libra`, below). microsandbox does not
# need root to run: upstream's installer works entirely out of $HOME, and the one
# privileged thing msb touches is /dev/kvm, which it opens read/write — `msb doctor`
# reports the device. If a deployment enables the `devices:` mapping in Compose, that
# user needs access to the device node: it is usually root:kvm mode 0660, so add the
# `kvm` group (`group_add`) or grant an ACL. msb's own cache lives in $HOME/.microsandbox,
# which docker-compose.yml mounts as a volume.
ARG MSB_VERSION=v0.7.16
RUN set -eu; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      amd64) cpu=x86_64; \
             sum=1f09ec7057bfcccb70e1713eb61061f15fa759b45e147759eb0b30aa7f5cb3bd ;; \
      arm64) cpu=aarch64; \
             sum=47e2e3238f31d914eba2a7e7ce328848da4673a1457ef2222f71589fe1da6f83 ;; \
      *) echo "unsupported architecture: $arch" >&2; exit 1 ;; \
    esac; \
    bundle="microsandbox-linux-${cpu}.tar.gz"; \
    curl -fsSL -o "/tmp/${bundle}" \
      "https://github.com/superradcompany/microsandbox/releases/download/${MSB_VERSION}/${bundle}"; \
    echo "${sum}  /tmp/${bundle}" | sha256sum -c -; \
    mkdir -p /opt/microsandbox/bin /opt/microsandbox/lib /tmp/msb-unpack; \
    tar -xzf "/tmp/${bundle}" -C /tmp/msb-unpack; \
    install -m 755 /tmp/msb-unpack/msb /opt/microsandbox/bin/msb; \
    ln -sf msb /opt/microsandbox/bin/microsandbox; \
    set -- /tmp/msb-unpack/libkrunfw.so.*.*.*; \
    [ "$#" -eq 1 ] || { echo "expected exactly one libkrunfw in ${bundle}" >&2; exit 1; }; \
    libkrunfw="$(basename "$1")"; \
    install -m 644 "$1" "/opt/microsandbox/lib/${libkrunfw}"; \
    abi="${libkrunfw#libkrunfw.so.}"; abi="${abi%%.*}"; \
    ln -sf "${libkrunfw}" "/opt/microsandbox/lib/libkrunfw.so.${abi}"; \
    ln -sf "libkrunfw.so.${abi}" /opt/microsandbox/lib/libkrunfw.so; \
    rm -rf "/tmp/${bundle}" /tmp/msb-unpack; \
    /opt/microsandbox/bin/msb --version
COPY --from=build /app/target/release/libra /usr/local/bin/libra
COPY --from=web /app/web/out /app/web/out

# An unprivileged account for the server, its data volume and msb's cache. /data is a
# volume; Docker initialises it from this layer, so it keeps the ownership set here.
RUN groupadd -g 10001 libra \
 && useradd -u 10001 -g libra -M -d /home/libra -s /usr/sbin/nologin libra \
 && mkdir -p /home/libra/.microsandbox /data \
 && chown -R libra:libra /home/libra /data
USER libra
ENV HOME=/home/libra

ENV PORT=3001 \
    LIBRA_DATA_DIR=/data \
    LIBRA_STATIC_DIR=/app/web/out \
    OBSCURA_BIN=/usr/local/bin/obscura \
    MSB_BIN=/opt/microsandbox/bin/msb \
    RUST_LOG=info
VOLUME /data
EXPOSE 3001
CMD ["libra"]
