#!/usr/bin/env bash
# Builds the macOS (Apple Silicon) distribution: the .app, as a drag-to-install
# .dmg and as a .zip, and the fallback folder bundle (bare binary + double-click
# launcher) as a .tar.gz.
#
# Signed ad hoc, so the app and image names carry `-local`, the convention the
# release guide uses for builds Gatekeeper has not been shown by Apple
# (docs/обновление-antigravity.md, «Выпуск для Mac»).
#
#   macos/build.sh                 build, then assemble into release/
#   macos/build.sh --out DIR       assemble into DIR instead
#   macos/build.sh --skip-build    reuse the last release binary
#
# Used by CI (.github/workflows/macos.yml) and by hand, so the two cannot drift.
# Everything it ships comes from this folder (README.md, Info.plist,
# AppIcon.icns, Запустить.command) and the cargo build.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT=$(pwd)
OUT="$ROOT/release"
BUILD=1
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT=$(cd "$2" && pwd); shift 2 ;;
    --skip-build) BUILD=0; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

TARGET=aarch64-apple-darwin
VER=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
# A 4th-digit build 2.28.2.1 is named with an underscore before the 4th number,
# matching the other platforms' assets.
ASSET_VER=$(printf '%s' "$VER" | sed -E 's/^([0-9]+\.[0-9]+\.[0-9]+)\.([0-9]+)$/\1_\2/')
NAME="AG_${ASSET_VER}_macos"
APP_NAME="Antigravity Unlocker.app"
VOLUME="Antigravity Unlocker"
APP_ZIP="AG_${ASSET_VER}_macOS_arm64_app-local.zip"
DMG="AG_${ASSET_VER}_macOS_arm64-local.dmg"
BIN_ID="local.antigravity.unlocker"
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}
BIN="$TARGET_DIR/$TARGET/release/ag_unlocker"

if [ "$BUILD" = 1 ]; then
  cargo build --release --bin ag_unlocker --target "$TARGET"
fi
[ -x "$BIN" ] || { echo "no release binary at $BIN" >&2; exit 1; }

# Smoke: the binary starts and knows its version. `--version` touches nothing.
"$BIN" --version >/dev/null

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

# ---- The folder bundle: binary + launcher + README.
mkdir -p "$STAGE/$NAME"
cp "$BIN" "$STAGE/$NAME/ag_unlocker"
chmod +x "$STAGE/$NAME/ag_unlocker"
# Ad hoc, so Gatekeeper does not kill our own download before the patcher can
# re-sign anything. Identifier kept the same as the .app's.
codesign --force --sign - --identifier "$BIN_ID" "$STAGE/$NAME/ag_unlocker"
cp macos/README.md "$STAGE/$NAME/README.md"
cp "macos/Запустить.command" "$STAGE/$NAME/Запустить.command"
chmod +x "$STAGE/$NAME/Запустить.command"

# ---- The .app: the same binary, signed as a bundle. A bundle is what System
# Settings → App Management can be granted to; a bare binary started from
# Finder inherits no grant at all.
APP="$STAGE/$APP_NAME"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/ag_unlocker"
chmod +x "$APP/Contents/MacOS/ag_unlocker"
sed "s/__VERSION__/$VER/g" macos/Info.plist > "$APP/Contents/Info.plist"
cp macos/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
codesign --force --sign - --options runtime --identifier "$BIN_ID" "$APP"
codesign --verify --strict --verbose=1 "$APP"
"$APP/Contents/MacOS/ag_unlocker" --version >/dev/null

# ---- Archives. tar keeps the execute bits; ditto keeps the bundle's signature
# and resource layout the way Finder's own "Compress" would.
mkdir -p "$OUT"
rm -rf "$OUT/$NAME" "$OUT/$APP_NAME"
cp -R "$STAGE/$NAME" "$OUT/$NAME"
ditto "$APP" "$OUT/$APP_NAME"
tar --no-mac-metadata -C "$STAGE" -czf "$OUT/$NAME.tar.gz" "$NAME"
ditto -c -k --norsrc --noextattr --keepParent "$APP" "$OUT/$APP_ZIP"

# ---- The disk image: the app beside a link to /Applications, so installing is
# one drag, plus the README. Compressed (UDZO), read-only.
DMG_ROOT="$STAGE/dmg"
mkdir -p "$DMG_ROOT"
ditto --norsrc --noextattr "$APP" "$DMG_ROOT/$APP_NAME"
ln -s /Applications "$DMG_ROOT/Applications"
cp macos/README.md "$DMG_ROOT/README.md"
rm -f "$OUT/$DMG"
hdiutil create -quiet -volname "$VOLUME" -srcfolder "$DMG_ROOT" -fs HFS+ \
  -format UDZO -ov "$OUT/$DMG"
hdiutil verify -quiet "$OUT/$DMG"

echo "version=$VER"
echo "name=$NAME"
echo "tgz=$OUT/$NAME.tar.gz"
echo "app_zip=$OUT/$APP_ZIP"
echo "dmg=$OUT/$DMG"
