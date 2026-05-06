#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: scripts/release.sh <version>" >&2
  echo "example: scripts/release.sh 0.1.3" >&2
  exit 2
fi

VERSION="${1#v}"
TAG="v${VERSION}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FORMULA="${ROOT}/packaging/homebrew/fainder.rb"
TAP_DIR="${FAINDER_HOMEBREW_TAP:-/private/tmp/fainder-homebrew-tap}"
TAP_REPO="${FAINDER_HOMEBREW_TAP_REPO:-https://github.com/satelerd/homebrew-tap.git}"

cd "${ROOT}"

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "working tree must be clean before release" >&2
  exit 1
fi

if git rev-parse "${TAG}" >/dev/null 2>&1; then
  echo "tag ${TAG} already exists" >&2
  exit 1
fi

if ! [[ "${VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "version must be semver-like, for example 0.1.3" >&2
  exit 1
fi

python3 - "${VERSION}" "${FORMULA}" <<'PY'
import pathlib
import re
import sys

version = sys.argv[1]
formula = pathlib.Path(sys.argv[2])
cargo = pathlib.Path("Cargo.toml")

cargo_text = cargo.read_text()
cargo_text = re.sub(r'^version = "[^"]+"', f'version = "{version}"', cargo_text, count=1, flags=re.M)
cargo.write_text(cargo_text)

formula_text = formula.read_text()
formula_text = re.sub(r'/refs/tags/v[^/]+\.tar\.gz', f'/refs/tags/v{version}.tar.gz', formula_text)
formula_text = re.sub(r'sha256 "[^"]+"', 'sha256 "REPLACE_WITH_RELEASE_SHA256"', formula_text, count=1)
formula.write_text(formula_text)
PY

cargo check
cargo test
cargo build --release

git add Cargo.toml Cargo.lock packaging/homebrew/fainder.rb
git commit -m "Prepare ${TAG} release"
git tag -a "${TAG}" -m "Fainder ${TAG}"
git push origin main "${TAG}"

SHA="$(curl -fsSL "https://github.com/satelerd/fainder/archive/refs/tags/${TAG}.tar.gz" | shasum -a 256 | awk '{print $1}')"
python3 - "${SHA}" "${FORMULA}" <<'PY'
import pathlib
import re
import sys

sha = sys.argv[1]
formula = pathlib.Path(sys.argv[2])
text = formula.read_text()
text = re.sub(r'sha256 "[^"]+"', f'sha256 "{sha}"', text, count=1)
formula.write_text(text)
PY

git add packaging/homebrew/fainder.rb
git commit -m "Finalize Homebrew formula for ${TAG}"
git push origin main

if [[ ! -d "${TAP_DIR}/.git" ]]; then
  git clone "${TAP_REPO}" "${TAP_DIR}"
fi

git -C "${TAP_DIR}" pull --rebase
mkdir -p "${TAP_DIR}/Formula"
cp "${FORMULA}" "${TAP_DIR}/Formula/fainder.rb"
git -C "${TAP_DIR}" add Formula/fainder.rb
git -C "${TAP_DIR}" commit -m "Update fainder to ${TAG}"
git -C "${TAP_DIR}" push origin main

cat <<EOF
released ${TAG}
sha256 ${SHA}

upgrade with:
  brew update
  brew upgrade fainder
EOF
