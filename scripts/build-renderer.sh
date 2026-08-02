#!/usr/bin/env bash
# Builds the unmodified upstream renderer (Vue UI) from the vendored
# awakened-poe-trade submodule. Run this whenever the submodule is updated.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../vendor/awakened-poe-trade/renderer"

npm ci
npm run make-index-files
npm run build

echo "renderer built at vendor/awakened-poe-trade/renderer/dist/"
