#!/usr/bin/env bash
# Build an UNSIGNED, un-notarized DMG for local testing (no Apple Developer ID
# required). For public distribution use packaging/build-signed-dmg.sh instead.
#
#   bash packaging/build-dmg.sh                 # full build, then package
#   SKIP_APP_BUILD=1 bash packaging/build-dmg.sh  # reuse existing .app (fast)
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

APP_NAME="STDF Parser"
APP_BUNDLE="target/release/bundle/macos/${APP_NAME}.app"
DMG_DIR="target/release/bundle/dmg"
DMG_NAME="STDF_Parser_0.1.1_aarch64.dmg"
SRC_DIR="target/release/bundle/dmg-unsigned-src"

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

if [[ "${SKIP_APP_BUILD:-0}" == "1" && -d "$APP_BUNDLE" ]]; then
  echo "==> Reusing existing app bundle (SKIP_APP_BUILD=1)"
else
  echo "==> Building app bundle"
  npm run tauri -- build --bundles app
fi

if [[ ! -d "$APP_BUNDLE" ]]; then
  echo "App bundle not found at $APP_BUNDLE" >&2
  echo "Run packaging/build-mac-app workflow first, or unset SKIP_APP_BUILD." >&2
  exit 1
fi

echo "==> Generating DMG background"
mkdir -p packaging/dmg
python3 packaging/dmg/install-background.py
BG="packaging/dmg/install-background.tiff"
[[ -f "$BG" ]] || BG="packaging/dmg/install-background.png"
echo "    background: $BG"

echo "==> Staging app into a clean source folder"
rm -rf "$SRC_DIR"
mkdir -p "$SRC_DIR"
cp -a "$APP_BUNDLE" "$SRC_DIR/"

echo "==> Creating unsigned DMG via bundled create-dmg script"
mkdir -p "$DMG_DIR"
rm -f "$DMG_DIR/$DMG_NAME"

env -u LC_ALL -u LC_CTYPE -u LANG LC_ALL=C LANG=C \
target/release/bundle/dmg/bundle_dmg.sh \
  --volname "$APP_NAME" \
  --background "$BG" \
  --window-size 680 440 \
  --icon-size 128 \
  --icon "$APP_NAME.app" 160 220 \
  --icon Applications 500 220 \
  --app-drop-link 500 220 \
  "$DMG_DIR/$DMG_NAME" \
  "$SRC_DIR"

echo ""
echo "✅ Unsigned DMG created:"
echo "   $ROOT_DIR/$DMG_DIR/$DMG_NAME"
echo ""
echo "ℹ️  This DMG is NOT code-signed or notarized (no Apple verification)."
echo "    • On THIS Mac it opens normally."
echo "    • On another Mac, Gatekeeper will block it. To run it there, after"
echo "      dragging the app to /Applications:"
echo "        xattr -dr com.apple.quarantine \"/Applications/${APP_NAME}.app\""
echo "      (or right-click the app → Open the first time)."
echo "    For public release, use: bash packaging/build-signed-dmg.sh"
