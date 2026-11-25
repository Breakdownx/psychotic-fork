#!/usr/bin/env bash
# Builds the macOS distribution: the .app, as a drag-to-install .dmg and as a
# .zip, and the fallback folder bundle (bare binary + double-click launcher) as
# a .tar.gz. Universal (Apple Silicon + Intel) unless --arm64-only.
#
#   macos/build.sh                 build, then assemble into release/
#   macos/build.sh --out DIR       assemble into DIR instead
#   macos/build.sh --arm64-only    one slice, for a quicker local build
#   macos/build.sh --skip-build    reuse the last release binaries
#
# Signed ad hoc: Gatekeeper asks once on first launch of a downloaded copy.
# Developer ID signing and notarization are the manual steps in
# docs/обновление-antigravity.md («Выпуск для Mac»).
#
# Used by CI (.github/workflows/macos.yml) and by hand, so the two cannot drift.
# Everything it ships comes from this folder (README.md, Info.plist,
# AppIcon.icns, Запустить.command) and the cargo build.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT=$(pwd)
OUT="$ROOT/release"
BUILD=1
TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT=$(cd "$2" && pwd); shift 2 ;;
    --skip-build) BUILD=0; shift ;;
    --arm64-only) TARGETS=(aarch64-apple-darwin); shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

VER=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
# A 4th-digit build 2.28.2.1 is named with an underscore before the 4th number,
# matching the other platforms' assets.
ASSET_VER=$(printf '%s' "$VER" | sed -E 's/^([0-9]+\.[0-9]+\.[0-9]+)\.([0-9]+)$/\1_\2/')
if [ ${#TARGETS[@]} -gt 1 ]; then ARCH_TAG=universal; else ARCH_TAG=arm64; fi
NAME="AG_${ASSET_VER}_macos"
APP_NAME="Antigravity Unlocker.app"
VOLUME="Antigravity Unlocker"
APP_ZIP="AG_${ASSET_VER}_macOS_${ARCH_TAG}_app-local.zip"
# No version in the name: every release carries the same file, so
# .../releases/latest/download/Psychotic_AG_Fork.dmg always fetches the newest.
DMG="Psychotic_AG_Fork.dmg"
BIN_ID="local.antigravity.unlocker"
TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target}

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

# ---- The binary: one cargo build per slice, then one fat Mach-O.
SLICES=()
for target in "${TARGETS[@]}"; do
  if [ "$BUILD" = 1 ]; then
    cargo build --release --bin ag_unlocker --target "$target"
  fi
  slice="$TARGET_DIR/$target/release/ag_unlocker"
  [ -x "$slice" ] || { echo "no release binary at $slice" >&2; exit 1; }
  SLICES+=("$slice")
done
BIN="$STAGE/ag_unlocker"
lipo -create -output "$BIN" "${SLICES[@]}"
if [ "$ARCH_TAG" = universal ]; then
  lipo "$BIN" -verify_arch arm64 x86_64
fi

# Ad hoc, with the hardened runtime the window needs no entitlement against.
# The identifier is the app's, for the bare binary too.
sign() {
  codesign --force --sign - --options runtime --identifier "$BIN_ID" "$1"
}

# Smoke: the binary starts and knows its version. `--version` touches nothing.
# The Intel slice runs under Rosetta when it is installed.
sign "$BIN"
"$BIN" --version >/dev/null
if [ "$ARCH_TAG" = universal ] && arch -x86_64 /usr/bin/true 2>/dev/null; then
  arch -x86_64 "$BIN" --version >/dev/null
fi

# ---- The folder bundle: binary + launcher + README.
mkdir -p "$STAGE/$NAME"
cp "$BIN" "$STAGE/$NAME/ag_unlocker"
chmod +x "$STAGE/$NAME/ag_unlocker"
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
sign "$APP"
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
