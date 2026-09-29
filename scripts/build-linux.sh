#!/usr/bin/env bash
# Builds Linux release binaries in Docker and packs them into dist/:
#   dist/fainder-<arch>-unknown-linux-gnu.tar.gz (+ .sha256)
# Asset names carry no version so releases/latest/download/<name> always works.
# Built on Debian bullseye (glibc 2.31), so they run on Ubuntu 20.04+ and
# Debian 11+. amd64 runs under emulation on Apple Silicon: slower, same result.
#
# usage: scripts/build-linux.sh [--upload vX.Y.Z]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${FAINDER_LINUX_IMAGE:-rust:1-bullseye}"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "${ROOT}/Cargo.toml" | head -1)"
DIST="${ROOT}/dist"
mkdir -p "${DIST}"

for platform in linux/amd64 linux/arm64; do
  case "${platform}" in
    linux/amd64) target="x86_64-unknown-linux-gnu" ;;
    linux/arm64) target="aarch64-unknown-linux-gnu" ;;
  esac
  name="fainder-${target}"
  echo "building ${name}"
  docker run --rm --platform "${platform}" \
    -v "${ROOT}":/src:ro \
    -v "fainder-cargo-${platform#linux/}":/usr/local/cargo/registry \
    -v "fainder-target-${platform#linux/}":/target \
    -v "${DIST}":/dist \
    -e CARGO_TARGET_DIR=/target \
    -w /src "${IMAGE}" \
    bash -euc "
      cargo build --release --locked
      /target/release/fainder --version
      tar -C /target/release -czf /dist/${name}.tar.gz fainder
    "
  (cd "${DIST}" && shasum -a 256 "${name}.tar.gz" > "${name}.tar.gz.sha256")
done

ls -1 "${DIST}"/fainder-*-linux-gnu.tar.gz*

if [[ "${1:-}" == "--upload" ]]; then
  tag="${2:?usage: scripts/build-linux.sh --upload vX.Y.Z}"
  if [[ "${tag#v}" != "${VERSION}" ]]; then
    echo "Cargo.toml says ${VERSION}, refusing to upload to ${tag}" >&2
    exit 1
  fi
  gh release upload "${tag}" --clobber "${DIST}"/fainder-*-linux-gnu.tar.gz*
fi
