#!/bin/zsh
# Build Lavdeck.app for Apple silicon and sign it, and optionally notarize it for release.
#   scripts/bundle.sh             build + sign into build/Lavdeck.app
#   scripts/bundle.sh --notarize  also notarize and zip for release into dist/Lavdeck-macOS-arm64.zip
#                                 (keychain profile $NOTARY_PROFILE, default piedol-notary)
set -euo pipefail
cd "${0:A:h}/.."

NOTARIZE=0
for a in "$@"; do
  case $a in
    --notarize) NOTARIZE=1 ;;
    *) echo "unknown option $a"; exit 2 ;;
  esac
done

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
BUILD=$(git rev-list --count HEAD 2>/dev/null || echo 1)
# Notarization needs a secure timestamp; everyday builds skip it to stay fast and work offline.
TS=--timestamp=none; (( NOTARIZE )) && TS=--timestamp

# Sign with a stable identity when there is one, so macOS keeps the microphone, camera and screen
# recording permissions across rebuilds (an ad-hoc signature changes with every build, and macOS
# forgets the grants). The identity is $LAVDECK_SIGN_IDENTITY, else the first line of a git-ignored
# .signing-identity file (a name or SHA-1), else the first Developer ID Application certificate in
# the keychain. Without any of them the app gets an ad-hoc signature.
IDENTITY=${LAVDECK_SIGN_IDENTITY:-}
if [[ -z $IDENTITY && -f .signing-identity ]]; then IDENTITY=$(head -1 .signing-identity); fi
if [[ -z $IDENTITY ]]; then
  IDENTITY=$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)
fi
if [[ -n $IDENTITY ]] && ! security find-identity -v -p codesigning | grep -qF -- "$IDENTITY"; then
  echo "warning: signing identity $IDENTITY not found"
  IDENTITY=""
fi
(( NOTARIZE )) && [[ -z $IDENTITY ]] && { echo "--notarize needs a Developer ID Application certificate"; exit 1; }

# Keep build-machine paths (home folder, checkout) out of the binary's panic messages.
# The encoded form separates flags with 0x1f, so paths with spaces work.
export CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=$HOME/.cargo=/cargo"$'\x1f'"--remap-path-prefix=$PWD=/lavdeck"
cargo build --release --target aarch64-apple-darwin -p lavdeck-app

APP=build/Lavdeck.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/aarch64-apple-darwin/release/Lavdeck "$APP/Contents/MacOS/Lavdeck"
sed -e "s/__VERSION__/$VERSION/" -e "s/__BUILD__/$BUILD/" resources/Info.plist > "$APP/Contents/Info.plist"
[[ -f assets/AppIcon.icns ]] || swift scripts/make-icon.swift
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
# Translations: locales/<lang>.json becomes <lang>.lproj/Localizable.strings (and the permission
# prompts' InfoPlist.strings). The empty en.lproj lists English among Lavdeck's languages, so
# macOS can choose it. There may be no catalogs yet.
mkdir -p "$APP/Contents/Resources/en.lproj"
printf '{}' > "$APP/Contents/Resources/en.lproj/Localizable.strings"
plutil -convert binary1 "$APP/Contents/Resources/en.lproj/Localizable.strings"
for catalog in locales/*.json(N); do
  lang=$(basename "$catalog" .json)
  mkdir -p "$APP/Contents/Resources/$lang.lproj"
  plutil -convert binary1 -o "$APP/Contents/Resources/$lang.lproj/Localizable.strings" "$catalog"
  # The permission prompts' text: each *UsageDescription in Info.plist, looked up by its English.
  python3 -I -c '
import json, plistlib, sys
info = plistlib.load(open(sys.argv[1], "rb"))
catalog = json.load(open(sys.argv[2]))
out = {k: catalog[v] for k, v in info.items() if k.endswith("UsageDescription") and v in catalog}
plistlib.dump(out, open(sys.argv[3], "wb"), fmt=plistlib.FMT_BINARY)
' "$APP/Contents/Info.plist" "$catalog" "$APP/Contents/Resources/$lang.lproj/InfoPlist.strings"
done

# The hardened runtime, with the entitlements for the microphone and camera.
if [[ -n $IDENTITY ]]; then
  codesign --force --options runtime "$TS" --entitlements resources/Lavdeck.entitlements --sign "$IDENTITY" "$APP"
else
  echo "warning: no signing identity, using an ad-hoc signature (permissions reset on every build)"
  codesign --force --options runtime --entitlements resources/Lavdeck.entitlements --sign - "$APP"
fi
codesign --verify --strict "$APP"

if (( NOTARIZE )); then
  # Notarization needs a keychain profile made once with:
  #   xcrun notarytool store-credentials <profile> --apple-id <Apple ID> --team-id <team ID>
  PROFILE=${NOTARY_PROFILE:-piedol-notary}
  mkdir -p dist
  ditto -c -k --keepParent "$APP" dist/Lavdeck-notarize.zip
  xcrun notarytool submit dist/Lavdeck-notarize.zip --keychain-profile "$PROFILE" --wait
  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"
  rm -f dist/Lavdeck-notarize.zip
  ditto -c -k --keepParent "$APP" dist/Lavdeck-macOS-arm64.zip
  echo "Notarized: dist/Lavdeck-macOS-arm64.zip"
fi

echo "Built $APP ($(du -sh "$APP" | cut -f1), binary $(du -h "$APP/Contents/MacOS/Lavdeck" | cut -f1), $(lipo -archs "$APP/Contents/MacOS/Lavdeck"))"
