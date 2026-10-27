#!/bin/bash
# Builds Lavdeck.app from the LavdeckApp SwiftPM target and signs it (hardened runtime).
#   scripts/build-app.sh             release build into build/Lavdeck.app
#   scripts/build-app.sh debug       debug build
#   scripts/build-app.sh --notarize  also notarize and zip for release (keychain profile $NOTARY_PROFILE, default piedol-notary)
set -euo pipefail
cd "$(dirname "$0")/.."

CONFIG=release; NOTARIZE=0
for a in "$@"; do
  case $a in
    release|debug) CONFIG=$a ;;
    --notarize) NOTARIZE=1 ;;
    *) echo "unknown option $a"; exit 2 ;;
  esac
done
(( NOTARIZE )) && [ "$CONFIG" != release ] && { echo "--notarize needs a release build"; exit 2; }

# Keep build-machine paths (home folder, checkout) out of the binary: source paths compiled into
# it read /lavdeck/Sources/…, and the debug map is stripped from the copy in the app (the dSYM
# SwiftPM writes next to the build keeps it).
FLAGS=(-c "$CONFIG" --product LavdeckApp -Xswiftc -file-prefix-map -Xswiftc "$PWD/Sources=/lavdeck/Sources")
# The release download is for Apple silicon.
(( NOTARIZE )) && FLAGS+=(--arch arm64)
swift build "${FLAGS[@]}"
BIN="$(swift build "${FLAGS[@]}" --show-bin-path)/LavdeckApp"

APP="build/Lavdeck.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Lavdeck"
strip -S "$APP/Contents/MacOS/Lavdeck"
[ -f assets/AppIcon.icns ] || swift scripts/make-icon.swift
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
VERSION="$(git describe --tags --always 2>/dev/null || echo dev)"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>io.github.piedol.lavdeck</string>
  <key>CFBundleName</key><string>Lavdeck</string>
  <key>CFBundleDisplayName</key><string>Lavdeck</string>
  <key>CFBundleExecutable</key><string>Lavdeck</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>LSUIElement</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Lavdeck records from your DJI Mic receiver.</string>
  <key>NSCameraUsageDescription</key><string>Lavdeck records your camera alongside the mics when you switch CAMERA on.</string>
</dict></plist>
PLIST

# The hardened runtime blocks the microphone and camera unless the app is entitled to them.
ENTITLEMENTS="build/Lavdeck.entitlements"
cat > "$ENTITLEMENTS" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>com.apple.security.device.audio-input</key><true/>
  <key>com.apple.security.device.camera</key><true/>
</dict></plist>
PLIST

# Sign with a stable identity when there is one, so macOS keeps the microphone, camera and screen
# recording permissions across rebuilds (an ad-hoc signature changes with every build, and macOS
# forgets the grants). The identity is $LAVDECK_SIGN_IDENTITY, else the first line of a git-ignored
# .signing-identity file (a name or SHA-1), else the first Developer ID Application certificate in
# the keychain. Without any of them the app gets an ad-hoc signature.
IDENTITY="${LAVDECK_SIGN_IDENTITY:-}"
if [ -z "$IDENTITY" ] && [ -f .signing-identity ]; then IDENTITY="$(head -1 .signing-identity)"; fi
if [ -z "$IDENTITY" ]; then
  IDENTITY="$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)"
fi
if [ -n "$IDENTITY" ] && ! security find-identity -p codesigning | grep -qF -- "$IDENTITY"; then
  echo "warning: signing identity $IDENTITY not found, using an ad-hoc signature"
  IDENTITY=""
fi
(( NOTARIZE )) && [ -z "$IDENTITY" ] && { echo "--notarize needs a Developer ID Application certificate"; exit 1; }
# Notarization needs a secure timestamp; everyday builds skip it to stay fast and work offline.
TS=--timestamp=none; (( NOTARIZE )) && TS=--timestamp
codesign --force --options runtime "$TS" --entitlements "$ENTITLEMENTS" --sign "${IDENTITY:--}" "$APP"
codesign --verify --strict "$APP"
echo "built $APP (signed: ${IDENTITY:-ad-hoc})"

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
