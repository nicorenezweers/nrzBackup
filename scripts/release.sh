#!/usr/bin/env bash
# Release nrzBackup as a multi-arch image in the nrzGit registry:
#   100.78.107.162:8390/nicorenezweers/nrz-backup:<version>
# nrzGit then updates image (tag + digest), version and release notes in the nrzStore by itself.
#
#   scripts/release.sh 1.0.8
#
# Needs: cargo-zigbuild + zig, Dioxus CLI under ~/.cargo/bin/dx, docker with buildx.
# Registry login: $NRZGIT_REGISTRY_AUTH (base64 of user:token) or the git extraheader of nrzCode.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="${1:?Version angeben, z. B. 1.0.8}"
REGISTRY="100.78.107.162:8390"
IMAGE="${REGISTRY}/nicorenezweers/nrz-backup"
DX="${DX:-$HOME/.cargo/bin/dx}"

grep -q "^version = \"${VERSION}\"" server/Cargo.toml || { echo "server/Cargo.toml hat nicht Version ${VERSION}" >&2; exit 1; }

# ── build ──
rm -rf target/dx/nrzbackup/release/web/public dist
"${DX}" bundle --platform web --release --package nrzbackup
mkdir -p dist
cp -R target/dx/nrzbackup/release/web/public dist/public
for pair in amd64:x86_64-unknown-linux-gnu arm64:aarch64-unknown-linux-gnu; do
  arch="${pair%%:*}"; target="${pair#*:}"
  cargo zigbuild --release -p nrzbackup-server --target "${target}.2.36"
  cp "target/${target}/release/nrzbackup-server" "dist/nrzbackup-server-${arch}"
done

# ── registry login (temporary docker config, nothing is written to ~/.docker) ──
AUTH="${NRZGIT_REGISTRY_AUTH:-}"
if [ -z "${AUTH}" ]; then
  for i in $(seq 0 $(( ${GIT_CONFIG_COUNT:-0} - 1 ))); do
    k="GIT_CONFIG_KEY_${i}"; v="GIT_CONFIG_VALUE_${i}"
    case "${!k}" in *"${REGISTRY}"*[eE]xtra[hH]eader*) AUTH="${!v##*Basic }";; esac
  done
fi
[ -n "${AUTH}" ] || { echo "Keine Registry-Anmeldung (NRZGIT_REGISTRY_AUTH setzen)" >&2; exit 1; }
ORIG_CONFIG="${DOCKER_CONFIG:-$HOME/.docker}"
export DOCKER_CONFIG="$(mktemp -d)"
trap 'rm -rf "${DOCKER_CONFIG}"' EXIT
# keep context (colima), CLI plugins and buildx builders of the normal config
for d in contexts cli-plugins buildx; do
  [ -e "${ORIG_CONFIG}/${d}" ] && ln -s "${ORIG_CONFIG}/${d}" "${DOCKER_CONFIG}/${d}"
done
python3 - "${ORIG_CONFIG}/config.json" "${DOCKER_CONFIG}/config.json" "${REGISTRY}" "${AUTH}" <<'PY'
import json, sys
src, dst, reg, auth = sys.argv[1:]
try:
    cfg = json.load(open(src))
except Exception:
    cfg = {}
cfg.pop("credsStore", None)
cfg.setdefault("auths", {})[reg] = {"auth": auth}
json.dump(cfg, open(dst, "w"))
PY

# ── image ──
docker buildx inspect nrzbackup-builder >/dev/null 2>&1 \
  || docker buildx create --name nrzbackup-builder --driver docker-container --config docker/buildkitd.toml >/dev/null
docker buildx build --builder nrzbackup-builder --platform linux/amd64,linux/arm64 \
  -f docker/Dockerfile -t "${IMAGE}:${VERSION}" --push .

docker buildx imagetools inspect --builder nrzbackup-builder "${IMAGE}:${VERSION}" | head -20
echo "Fertig: ${IMAGE}:${VERSION}"
