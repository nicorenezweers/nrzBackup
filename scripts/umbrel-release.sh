#!/usr/bin/env bash
# Baut nrzBackup fuer umbrelOS und legt den Release-Stand auf dem Zweig `umbrel` ab:
#   nrzbackup-server-amd64.gz, nrzbackup-server-arm64.gz, public.tar.gz, SHA256SUMS
# Danach: git push origin umbrel umbrel-<version>   (nrzGit aktualisiert den nrzStore)
#
# Voraussetzungen: cargo-zigbuild + zig, Dioxus-CLI 0.7 unter ~/.cargo/bin/dx
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="${1:?Version angeben, z. B. 1.0.1}"
TAG="umbrel-${VERSION}"
NOTES="${2:-nrzBackup ${VERSION}}"
DX="${DX:-$HOME/.cargo/bin/dx}"
OUT="$(mktemp -d)"
trap 'rm -rf "${OUT}"' EXIT

git diff --quiet && git diff --cached --quiet || { echo "Arbeitsverzeichnis nicht sauber" >&2; exit 1; }
grep -q "^version = \"${VERSION}\"" server/Cargo.toml || { echo "server/Cargo.toml hat nicht Version ${VERSION}" >&2; exit 1; }

"${DX}" bundle --platform web --release --package nrzbackup
tar -czf "${OUT}/public.tar.gz" -C target/dx/nrzbackup/release/web public

for pair in amd64:x86_64-unknown-linux-gnu aarch64:aarch64-unknown-linux-gnu; do
  arch="${pair%%:*}"; target="${pair#*:}"
  [ "${arch}" = aarch64 ] && arch=arm64
  cargo zigbuild --release -p nrzbackup-server --target "${target}.2.36"
  gzip -9 -c "target/${target}/release/nrzbackup-server" > "${OUT}/nrzbackup-server-${arch}.gz"
done
(cd "${OUT}" && shasum -a 256 nrzbackup-server-amd64.gz nrzbackup-server-arm64.gz public.tar.gz > SHA256SUMS)

SRC="$(git rev-parse --short HEAD)"
WT="$(mktemp -d)"
if git show-ref --quiet refs/heads/umbrel; then
  git worktree add -q "${WT}" umbrel
else
  git worktree add -q --detach "${WT}"
  (cd "${WT}" && git checkout -q --orphan umbrel && git rm -rqf . )
fi
cp "${OUT}"/* "${WT}/"
cat > "${WT}/README.md" <<README
# nrzBackup – Umbrel-Dateien

Nur fertige Dateien fuer die App \`nrz-backup\` im nrzStore (geladen von \`hooks/pre-start\`).
Quellcode: Zweig \`main\`. Stand: ${SRC}, Version ${VERSION}.
README
(cd "${WT}" && git add -A && git commit -q -m "nrzBackup ${VERSION}: Umbrel-Dateien (aus ${SRC})" && git tag -a "${TAG}" -m "${NOTES}")
git worktree remove --force "${WT}"
echo "Fertig: Zweig umbrel und Tag ${TAG}. Jetzt: git push origin umbrel ${TAG}"
