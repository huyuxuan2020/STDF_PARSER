#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

APP_NAME="STDF Parser"
APP_VERSION="${APP_VERSION:-$(node -p "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json', 'utf8')).version")}"
APP_BUNDLE="${APP_BUNDLE:-target/release/bundle/macos/${APP_NAME}.app}"
DMG_ARCH="${DMG_ARCH:-aarch64}"
DMG_DIR="${DMG_DIR:-target/release/bundle/dmg}"
DMG_NAME="${DMG_NAME:-STDF_Parser_${APP_VERSION}_${DMG_ARCH}_developer_id.dmg}"
SRC_DIR="target/release/bundle/dmg-signed-notarized"
# Set these in your local environment; no identity is hardcoded in the repo.
#   export SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
#   export NOTARY_PROFILE="your-notary-profile"
SIGNING_IDENTITY="${SIGNING_IDENTITY:?set SIGNING_IDENTITY (Developer ID Application identity)}"
NOTARY_PROFILE="${NOTARY_PROFILE:?set NOTARY_PROFILE (notarytool keychain profile)}"

export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
PYTHON_BIN="${PYTHON_BIN:-python3}"

if [[ "${SKIP_APP_BUILD:-0}" == "1" && -d "$APP_BUNDLE" ]]; then
  echo "==> Reusing existing app bundle (SKIP_APP_BUILD=1)"
else
  echo "==> Building app bundle"
  npm run tauri -- build --bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}'
fi

[[ -d "$APP_BUNDLE" ]] || { echo "App bundle not found at $APP_BUNDLE" >&2; exit 1; }

CREATE_DMG_BIN="${CREATE_DMG_BIN:-target/release/bundle/dmg/bundle_dmg.sh}"
if [[ ! -x "$CREATE_DMG_BIN" ]] && command -v create-dmg >/dev/null 2>&1; then
  CREATE_DMG_BIN="$(command -v create-dmg)"
fi
[[ -x "$CREATE_DMG_BIN" ]] || { echo "create-dmg executable not found" >&2; exit 1; }

APP_NOTARY_DIR="$(mktemp -d)"
mount_point=""
cleanup() {
  if [[ -n "$mount_point" ]]; then
    hdiutil detach "$mount_point" >/dev/null 2>&1 || true
  fi
  rm -rf "$APP_NOTARY_DIR"
}
trap cleanup EXIT

echo "==> Signing app with Developer ID"
codesign --force --deep --options runtime --timestamp --sign "$SIGNING_IDENTITY" "$APP_BUNDLE"
codesign --verify --deep --strict --verbose=2 "$APP_BUNDLE"

echo "==> Notarizing and stapling app"
ditto -c -k --keepParent "$APP_BUNDLE" "$APP_NOTARY_DIR/${APP_NAME}.zip"
xcrun notarytool submit "$APP_NOTARY_DIR/${APP_NAME}.zip" --keychain-profile "$NOTARY_PROFILE" --wait
xcrun stapler staple "$APP_BUNDLE"
xcrun stapler validate "$APP_BUNDLE"

echo "==> Preparing DMG assets"
mkdir -p packaging/dmg
"$PYTHON_BIN" packaging/dmg/install-background.py
BG="packaging/dmg/install-background.tiff"
[[ -f "$BG" ]] || { echo "DMG background not found at $BG" >&2; exit 1; }

echo "==> Staging signed app into a clean source folder"
rm -rf "$SRC_DIR"
mkdir -p "$SRC_DIR"
cp -a "$APP_BUNDLE" "$SRC_DIR/"

echo "==> Creating styled DMG via bundled create-dmg script"
mkdir -p "$DMG_DIR"
rm -f "$DMG_DIR/$DMG_NAME"

env -u LC_ALL -u LC_CTYPE -u LANG LC_ALL=C LANG=C \
  "$CREATE_DMG_BIN" \
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

[[ -f "$DMG_DIR/$DMG_NAME" ]] || { echo "DMG was not created" >&2; exit 1; }

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

expected_entries="$(printf '%s\n' '.DS_Store' '.background' 'Applications' "${APP_NAME}.app" | LC_ALL=C sort)"
actual_entries="$(find "$mount_point" -mindepth 1 -maxdepth 1 -print | sed "s#^$mount_point/##" | LC_ALL=C sort)"
if [[ "$actual_entries" != "$expected_entries" ]]; then
  echo "Unexpected DMG contents:" >&2
  printf '%s\n' "$actual_entries" >&2
  exit 1
fi
[[ -L "$mount_point/Applications" ]] || { echo "Applications link missing" >&2; exit 1; }
[[ -f "$mount_point/.background/install-background.tiff" ]] || {
  echo "Fixed DMG background missing" >&2
  exit 1
}
codesign --verify --deep --strict --verbose=2 "$mount_point/${APP_NAME}.app"
xcrun stapler validate "$mount_point/${APP_NAME}.app"
spctl -a -vvv -t execute "$mount_point/${APP_NAME}.app"
hdiutil detach "$mount_point"
mount_point=""

echo "==> Signed and notarized DMG created:"
echo "$ROOT_DIR/$DMG_DIR/$DMG_NAME"
