#!/usr/bin/env bash
# Publish libra: guard the release, run every check CI runs, tag the image, push.
#
#   ./scripts/release.sh 0.1.0              # release 0.1.0 (or v0.1.0) and push
#   ./scripts/release.sh 0.1.0 --no-push    # same checks + build, stop before the push
#
# Registry defaults to Docker Hub under chakyiu. Override with REGISTRY / NAMESPACE:
#
#   REGISTRY=ghcr.io NAMESPACE=someone ./scripts/release.sh 0.1.0
#
# Docker Hub credentials are not this script's business: log in first (`docker login`),
# or let .github/workflows/release.yml push from CI with repo secrets.
#
# The image is built for linux/amd64 only, the platform the release workflow publishes;
# PLATFORM overrides it (for a local multi-arch experiment, say).
#
# Windows: there is no PowerShell port. Run this under WSL or Git Bash — both give you
# `bash`, and the script only calls git, cargo, npm and docker.
set -euo pipefail

cd "$(dirname "$0")/.."

REGISTRY="${REGISTRY:-docker.io}"
NAMESPACE="${NAMESPACE:-chakyiu}"
IMAGE="libra"

die() { echo "release: $*" >&2; exit 1; }
step() { printf '\n== %s\n' "$*"; }

VERSION_ARG="${1:-}"
PUSH=1
for arg in "$@"; do
  case "$arg" in
    --no-push) PUSH=0 ;;
    -h|--help) awk 'NR>1 && /^#/ {sub(/^# ?/,""); print; next} NR>1 {exit}' "$0"; exit 0 ;;
    -*) die "unknown flag: $arg" ;;
  esac
done
[ -n "$VERSION_ARG" ] || die "usage: $0 <version> [--no-push]"

# --- the version is the source of truth, and there is exactly one of it -------------
step "versions"
CARGO_VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n1)"
WEB_VERSION="$(sed -n 's/^  "version": "\(.*\)",$/\1/p' web/package.json | head -n1)"
[ -n "$CARGO_VERSION" ] || die "no [package] version in Cargo.toml"
[ -n "$WEB_VERSION" ] || die "no version in web/package.json"
[ "$CARGO_VERSION" = "$WEB_VERSION" ] \
  || die "Cargo.toml is $CARGO_VERSION but web/package.json is $WEB_VERSION — bump both"

# Accept 0.1.0 and v0.1.0; anything else (including a tag that disagrees with
# Cargo.toml) is refused, the same guard notia's release uses.
REQUESTED="${VERSION_ARG#v}"
[ "$REQUESTED" = "$CARGO_VERSION" ] \
  || die "requested $VERSION_ARG but Cargo.toml says $CARGO_VERSION — bump Cargo.toml first"
VERSION="$CARGO_VERSION"
TAG="v${VERSION}"
echo "version $VERSION (tag $TAG)"

# --- refuse to publish anything but a clean main ------------------------------------
step "repository state"
BRANCH="$(git rev-parse --abbrev-ref HEAD)"
[ "$BRANCH" = "main" ] || die "on branch '$BRANCH', releases come from main"
[ -z "$(git status --porcelain)" ] || die "working tree is dirty — commit or discard first"
git fetch --quiet --tags origin
if git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null; then
  # Already tagged: it must be the commit we are about to publish.
  [ "$(git rev-parse "refs/tags/${TAG}^{commit}")" = "$(git rev-parse HEAD)" ] \
    || die "tag ${TAG} already exists on another commit"
  echo "tag ${TAG} exists at HEAD"
else
  echo "tag ${TAG} does not exist yet; it is created after the checks pass"
fi

# --- the same gate as ci.yml, in this working copy --------------------------------
step "cargo fmt --check"
cargo fmt --check
step "cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets -- -D warnings
step "cargo test"
cargo test
step "web tests + static export"
if [ ! -d web/node_modules ]; then
  (cd web && npm ci)
fi
(cd web && npm test)
(cd web && npm run build)

# --- tag the commit, build, publish ------------------------------------------------
if ! git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null; then
  step "git tag ${TAG}"
  git tag -a "$TAG" -m "libra ${VERSION}"
fi

REF="${REGISTRY}/${NAMESPACE}/${IMAGE}"
# Same platform the release workflow publishes. Without it an arm64 host would push
# arm64 under the linux/amd64 tag, and the next tag push would quietly replace it.
PLATFORM="${PLATFORM:-linux/amd64}"
step "docker build --platform ${PLATFORM} ${REF}:${VERSION}"
docker build --platform "${PLATFORM}" -t "${REF}:${VERSION}" -t "${REF}:latest" .

if [ "$PUSH" -eq 1 ]; then
  step "docker push"
  docker push "${REF}:${VERSION}"
  docker push "${REF}:latest"
fi

cat <<EOF

release: ${REF}:${VERSION} --platform ${PLATFORM} ($PUSH: $([ "$PUSH" -eq 1 ] && echo pushed || echo not pushed))
next:
  git push origin main "$TAG"    # also starts .github/workflows/release.yml
  That workflow pushes the image when DOCKERHUB_* repo secrets are set, and only
  creates the GitHub Release when they are not — so this script's push is for a host
  with \`docker login\` and no secrets. With secrets, run --no-push and let CI push:
    ./scripts/release.sh ${VERSION} --no-push && git push origin main "$TAG"
  Then check the image, on a clean Docker host:
    LIBRA_AUTH_TOKEN=\$(openssl rand -hex 24) docker run --rm -p 3001:3001 ${REF}:${VERSION}
    curl -s -o /dev/null -w '%{http_code}\\n' http://127.0.0.1:3001/health
    docker image inspect ${REF}:${VERSION} --format '{{index .RepoDigests 0}} {{.Size}}'
EOF
