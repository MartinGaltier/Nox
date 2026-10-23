#!/usr/bin/env bash
# Builds dist/Nox.app (arm64, release) reproducibly from a clean checkout.
#   scripts/build-app.sh            # release build, ad-hoc signature
#   NOX_SIGN_IDENTITY="Developer ID Application: …" scripts/build-app.sh
#   NOX_PROFILE=debug scripts/build-app.sh
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
PROFILE="${NOX_PROFILE:-release}"
TARGET=aarch64-apple-darwin
APP="$ROOT/dist/Nox.app"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
BUILD="$(git rev-list --count HEAD 2>/dev/null || echo 0)"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct 2>/dev/null || date +%s)}"

flags=(--target "$TARGET" --locked)
[[ "$PROFILE" == release ]] && flags+=(--release)
echo "==> cargo build ($PROFILE, $TARGET)"
cargo build "${flags[@]}" -p nox-app -p nox-helper
# Optional cdylibs/binaries contributed by modules (built if the package exists).
for extra in nox-nowplaying; do
  if cargo metadata --no-deps --format-version 1 | grep -q "\"name\":\"$extra\""; then
    cargo build "${flags[@]}" -p "$extra"
  fi
done
OUT="$ROOT/target/$TARGET/$PROFILE"

echo "==> assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Library/LaunchServices"
sed -e "s/__VERSION__/$VERSION/" -e "s/__BUILD__/$BUILD/" packaging/Info.plist > "$APP/Contents/Info.plist"
printf 'APPL????' > "$APP/Contents/PkgInfo"
cp "$OUT/nox" "$APP/Contents/MacOS/nox"
cp "$OUT/nox-helper" "$APP/Contents/Library/LaunchServices/nox-helper"
[[ -f "$OUT/libnox_nowplaying.dylib" ]] && cp "$OUT/libnox_nowplaying.dylib" "$APP/Contents/Resources/"
if compgen -G "packaging/resources/*" > /dev/null; then cp -R packaging/resources/. "$APP/Contents/Resources/"; fi
[[ -f packaging/AppIcon.icns ]] && cp packaging/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
cp LICENSE NOTICE.md "$APP/Contents/Resources/"

IDENTITY="${NOX_SIGN_IDENTITY:--}"
echo "==> codesign (identity: $IDENTITY)"
sign_opts=(--force --timestamp=none --options runtime)
[[ "$IDENTITY" != "-" ]] && sign_opts=(--force --timestamp --options runtime)
codesign "${sign_opts[@]}" -s "$IDENTITY" -i io.github.martingaltier.nox.helper "$APP/Contents/Library/LaunchServices/nox-helper"
[[ -f "$APP/Contents/Resources/libnox_nowplaying.dylib" ]] && codesign "${sign_opts[@]}" -s "$IDENTITY" "$APP/Contents/Resources/libnox_nowplaying.dylib"
codesign "${sign_opts[@]}" -s "$IDENTITY" --entitlements packaging/entitlements.plist "$APP"
codesign --verify --strict --verbose=2 "$APP"
lipo -archs "$APP/Contents/MacOS/nox"
echo "==> done: $APP ($(du -sh "$APP" | cut -f1))"
[[ "$IDENTITY" == "-" ]] && echo "note: ad-hoc signature — not notarized; macOS may ask to re-grant permissions after each rebuild."
