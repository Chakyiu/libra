# Releasing Libra

One version, one tag, one image. `Cargo.toml` is the source of truth; `web/package.json` must say
the same thing, and both the script and CI refuse to publish when they disagree.

## What a release is

| Thing | Where |
| --- | --- |
| version | `Cargo.toml` `[package] version` — bump it first, in the same commit as `web/package.json` |
| git tag | `v<version>`, annotated, on the released commit |
| image | `docker.io/chakyiu/libra:<version>` and `:latest` |
| GitHub Release | created from the tag, notes generated |

## The fast path (bash: Linux, macOS, WSL, Git Bash)

The image has to be pushed once per release and there are two ways to do it — pick one, not both.

**With `DOCKERHUB_USERNAME` / `DOCKERHUB_TOKEN` repo secrets** (CI pushes):

```bash
git switch main && git pull
./scripts/release.sh 0.1.0 --no-push   # every check + the build, no push
git push origin main v0.1.0            # the tag push pushes the image and opens the Release
```

**With `docker login` on this host and no repo secrets** (the script pushes):

```bash
git switch main && git pull
./scripts/release.sh 0.1.0             # every check, then push <version> and :latest
git push origin main v0.1.0            # the tag push only creates the Release
```

Doing both in the secrets case builds and pushes the image twice; the tag push would just replace
what the script pushed with the same thing.

The script refuses to run unless all of this holds:

1. the branch is `main` and `git status --porcelain` is empty;
2. the argument (with or without the `v`) equals the `Cargo.toml` version;
3. `web/package.json` carries the same version;
4. an existing `v<version>` tag, if there is one, points at `HEAD`;
5. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`,
   `npm test` and `npm run build` (in `web/`) are all green.

Then it creates the tag if needed, builds `<registry>/<namespace>/libra:<version>` plus `:latest`
for `linux/amd64` — the only platform the release workflow publishes — and pushes both.
`REGISTRY` / `NAMESPACE` override the Docker Hub defaults, `PLATFORM` overrides the platform.

Docker Hub credentials are not handled here: `docker login` first, or let CI push (below).

## The same steps in PowerShell (Windows, no WSL)

`scripts/release.sh` is bash; on a plain PowerShell prompt do the same steps by hand. Stop at the
first one that fails — `$LASTEXITCODE` is the check.

```powershell
cd C:\path\to\libra
git switch main; git pull
if ((git status --porcelain) -or (git rev-parse --abbrev-ref HEAD) -ne 'main') { throw 'not clean main' }

# version: all three must agree
$cargo = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"$').Matches.Groups[1].Value
$web   = (Select-String -Path web\package.json -Pattern '^  "version": "(.*)",$').Matches.Groups[1].Value
$arg   = '0.1.0'
if ($cargo -ne $web -or $cargo -ne $arg) { throw "version mismatch: cargo=$cargo web=$web arg=$arg" }

# checks, exactly what CI runs
cargo fmt --check
if ($LASTEXITCODE) { throw 'fmt' }
cargo clippy --all-targets -- -D warnings
if ($LASTEXITCODE) { throw 'clippy' }
cargo test
if ($LASTEXITCODE) { throw 'test' }
Push-Location web
if (-not (Test-Path node_modules)) { npm ci }
npm test
if ($LASTEXITCODE) { Pop-Location; throw 'web test' }
npm run build
if ($LASTEXITCODE) { Pop-Location; throw 'web build' }
Pop-Location

# tag the commit, then the image (linux/amd64, like the workflow)
git tag -a "v$cargo" -m "libra $cargo"
docker build --platform linux/amd64 -t "docker.io/chakyiu/libra:$cargo" -t "docker.io/chakyiu/libra:latest" .
docker push "docker.io/chakyiu/libra:$cargo"
docker push "docker.io/chakyiu/libra:latest"
git push origin main "v$cargo"
```

`docker push` needs `docker login` first. Skip the two `docker push` lines when the repo has the
secrets below — the tag push publishes the image, and pushing here as well would just build it twice.

## Publishing from CI

`.github/workflows/release.yml` runs on a `v*` tag push, or on `workflow_dispatch` with an existing
tag (it defaults to `v<Cargo.toml version>`). It repeats the version guard — reading `Cargo.toml` and
`web/package.json` out of the **tag's** tree, so a dispatch on a branch whose version moved on is
rejected too — and then does two independent things:

1. creates the GitHub Release for the tag, which needs no credentials; then
2. if `DOCKERHUB_USERNAME` and `DOCKERHUB_TOKEN` are set, builds `linux/amd64` and pushes
   `:<version>` and `:latest`.

So the first tag push is green and gets a Release even before registry credentials exist. Add them
in Settings → Secrets → Actions when you want CI to publish the image:

- `DOCKERHUB_USERNAME`
- `DOCKERHUB_TOKEN` — a Docker Hub access token with read/write on the repository, not the account
  password

With them unset the run logs a notice and skips the push; with them wrong the login step fails and
the Release already exists.

## Checking a release

On a host with Docker and no checkout:

```bash
export LIBRA_AUTH_TOKEN="$(openssl rand -hex 24)"
docker run --rm -d --name libra-check -p 3001:3001 docker.io/chakyiu/libra:0.1.0
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:3001/health   # expect 200
docker image inspect docker.io/chakyiu/libra:0.1.0 --format '{{index .RepoDigests 0}} {{.Size}}'
docker rm -f libra-check
```

Under Compose, with the published image instead of a local build:

```bash
docker compose pull && docker compose up --no-build     # Redis + SearxNG start with it
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:3001/health
```

## Not in scope (yet)

Signing and SBOMs, an `arm64` image (the pinned Obscura and microsandbox releases decide that), and
Helm/Kubernetes packaging.
