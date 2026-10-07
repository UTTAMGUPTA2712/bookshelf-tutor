#!/usr/bin/env bash
# Downloads the prebuilt pdfium library for this OS into src-tauri/resources/pdfium/.
# Version must match the pdfium-render feature in src-tauri/Cargo.toml (pdfium_latest = 7543).
set -euo pipefail

VERSION="7543"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/src-tauri/resources/pdfium"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)  ASSET="pdfium-linux-x64.tgz";   LIB="lib/libpdfium.so" ;;
  Linux-aarch64) ASSET="pdfium-linux-arm64.tgz"; LIB="lib/libpdfium.so" ;;
  Darwin-arm64)  ASSET="pdfium-mac-arm64.tgz";   LIB="lib/libpdfium.dylib" ;;
  Darwin-x86_64) ASSET="pdfium-mac-x64.tgz";     LIB="lib/libpdfium.dylib" ;;
  *) echo "Unsupported platform: $(uname -s)-$(uname -m) (on Windows use scripts/fetch-pdfium.ps1)"; exit 1 ;;
esac

if [ -f "$DEST/$(basename "$LIB")" ]; then
  echo "pdfium already present: $DEST/$(basename "$LIB")"
  exit 0
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
URL="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F$VERSION/$ASSET"
echo "Downloading $URL"
curl -fsSL "$URL" -o "$TMP/pdfium.tgz"
tar -xzf "$TMP/pdfium.tgz" -C "$TMP"
mkdir -p "$DEST"
cp "$TMP/$LIB" "$DEST/"
echo "Installed $DEST/$(basename "$LIB")"
