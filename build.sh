#!/usr/bin/env bash
# Build dist/mould-maker/ (plugin.wasm + plugin.json) and dist/mould-maker-<version>.zip.
# Needs Rust with the wasm32-unknown-unknown target and wasm-tools.
# Without a prebuilt wasm32 std, set BUILD_STD=1 (uses -Zbuild-std; needs Rust's library sources).
set -euo pipefail
cd "$(dirname "$0")"
if [[ "${BUILD_STD:-0}" == "1" ]]; then
  RUSTC_BOOTSTRAP=1 cargo build --release --lib --target wasm32-unknown-unknown -Zbuild-std=std,panic_abort
else
  cargo build --release --lib --target wasm32-unknown-unknown
fi
ver=$(python3 -c "import json;print(json.load(open('plugin.json'))['version'])")
out=dist/mould-maker
rm -rf dist; mkdir -p "$out"
wasm-tools component new "${CARGO_TARGET_DIR:-target}/wasm32-unknown-unknown/release/mould_maker.wasm" -o "$out/plugin.wasm"
wasm-tools validate --features component-model "$out/plugin.wasm"
cp plugin.json README.md LICENSE "$out/"
(cd dist && zip -qr "mould-maker-$ver.zip" mould-maker && sha256sum "mould-maker-$ver.zip" > "mould-maker-$ver.zip.sha256")
echo "built dist/mould-maker-$ver.zip"
