#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

APP_NAME="STDF Parser"
APP_BUNDLE="target/release/bundle/macos/${APP_NAME}.app"
DMG_DIR="target/release/bundle/dmg"
DMG_NAME="STDF_Parser_0.1.1_aarch64_developer_id.dmg"
# Set these in your local environment; no identity is hardcoded in the repo.
#   export SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
#   export NOTARY_PROFILE="your-notary-profile"
SIGNING_IDENTITY="${SIGNING_IDENTITY:?set SIGNING_IDENTITY (Developer ID Application identity)}"
NOTARY_PROFILE="${NOTARY_PROFILE:?set NOTARY_PROFILE (notarytool keychain profile)}"

echo "==> Building app bundle"
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
npm run tauri -- build --bundles app

echo "==> Signing app with Developer ID"
codesign --force --deep --options runtime --timestamp --sign "$SIGNING_IDENTITY" "$APP_BUNDLE"
codesign --verify --deep --strict --verbose=2 "$APP_BUNDLE"

echo "==> Preparing DMG assets"
mkdir -p packaging/dmg
python3 packaging/dmg/install-background.py
BG="packaging/dmg/install-background.tiff"
[[ -f "$BG" ]] || BG="packaging/dmg/install-background.png"

echo "==> Staging signed app into a clean source folder"
SRC_DIR="target/release/bundle/dmg-signed-notarized"
rm -rf "$SRC_DIR"
mkdir -p "$SRC_DIR"
cp -a "$APP_BUNDLE" "$SRC_DIR/"

echo "==> Creating styled DMG via bundled create-dmg script"
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
  --codesign "$SIGNING_IDENTITY" \
  --notarize "$NOTARY_PROFILE" \
  "$DMG_DIR/$DMG_NAME" \
  "$SRC_DIR"

echo "==> Verifying final DMG"
codesign --verify --verbose=2 "$DMG_DIR/$DMG_NAME"
xcrun stapler validate "$DMG_DIR/$DMG_NAME"
spctl -a -vvv -t open --context context:primary-signature "$DMG_DIR/$DMG_NAME"

echo "==> Mounted app verification"
mount_output="$(hdiutil attach "$DMG_DIR/$DMG_NAME" -nobrowse)"
mount_point="$(printf '%s\n' "$mount_output" | sed -n 's#^/dev/.*\(/Volumes/.*\)$#\1#p' | tail -n 1)"
if [[ -z "$mount_point" || ! -d "$mount_point" ]]; then
  echo "Could not determine DMG mount point." >&2
  printf '%s\n' "$mount_output" >&2
  exit 1
fi
trap 'hdiutil detach "$mount_point" >/dev/null 2>&1 || true' EXIT
codesign --verify --deep --strict --verbose=2 "$mount_point/${APP_NAME}.app"
spctl -a -vvv -t execute "$mount_point/${APP_NAME}.app"
hdiutil detach "$mount_point"
trap - EXIT

echo "==> Signed and notarized DMG created:"
echo "$ROOT_DIR/$DMG_DIR/$DMG_NAME"
