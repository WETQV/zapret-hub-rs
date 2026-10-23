#!/usr/bin/env bash
# Cross-builds the Windows release on Linux and packs a portable zip.
#
#   packaging/build-windows.sh              # latest upstream bundle
#   BUNDLE_TAG=1.10.4 packaging/build-windows.sh
#
# Needs: rustup target x86_64-pc-windows-gnu, mingw-w64 (gcc + windres), python3.
# The Inno Setup installer is built on Windows with packaging/build-installer.ps1
# (or here, if ISCC is reachable through wine as $ISCC).
set -euo pipefail

cd "$(dirname "$0")/.."
TARGET=x86_64-pc-windows-gnu
STAGE=dist/stage
VERSION=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
ZIP="dist/portable/zapret-hub-${VERSION}-portable.zip"

echo "==> Building Zapret Hub ${VERSION} for ${TARGET}"
cargo build --release --target "${TARGET}"

echo "==> Staging bundle (same code path as the in-app updater)"
rm -rf "${STAGE}"
mkdir -p "${STAGE}"
cargo run --quiet -- --stage-bundle "${STAGE}/bundle" ${BUNDLE_TAG:+--bundle-tag "${BUNDLE_TAG}"}
cp "target/${TARGET}/release/zapret-hub-rs.exe" "${STAGE}/Zapret Hub.exe"
cp assets/builtin-whitelist.txt "${STAGE}/"

echo "==> Packing ${ZIP}"
mkdir -p dist/portable
rm -f "${ZIP}"
python3 - "${STAGE}" "${ZIP}" <<'PY'
import os, sys, zipfile
stage, out = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
    for root, _, files in os.walk(stage):
        for name in files:
            path = os.path.join(root, name)
            archive.write(path, os.path.join("Zapret Hub", os.path.relpath(path, stage)))
PY

if [[ -n "${ISCC:-}" ]]; then
    echo "==> Building installer with ${ISCC}"
    mkdir -p dist/installer
    ${ISCC} "/DAppVersion=${VERSION}" "/DSourceDir=..\\dist\\stage" installer/zapret-hub.iss
fi

echo "Done:"
echo "  ${STAGE}/Zapret Hub.exe"
echo "  ${ZIP}"
