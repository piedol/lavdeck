#!/bin/bash
# Builds "Lavdeck.app" into build/ from the LavdeckApp SwiftPM target (ad-hoc signed, for this Mac).
set -euo pipefail
cd "$(dirname "$0")/.."
CONFIG="${1:-release}"
swift build -c "$CONFIG" --product LavdeckApp
BIN="$(swift build -c "$CONFIG" --show-bin-path)/LavdeckApp"
APP="build/Lavdeck.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/Lavdeck"
VERSION="$(git describe --tags --always 2>/dev/null || echo dev)"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>io.github.piedol.lavdeck</string>
  <key>CFBundleName</key><string>Lavdeck</string>
  <key>CFBundleDisplayName</key><string>Lavdeck</string>
  <key>CFBundleExecutable</key><string>Lavdeck</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>LSUIElement</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Lavdeck records from your DJI Mic receiver.</string>
  <key>NSCameraUsageDescription</key><string>Capture plugins you switch on can record your camera alongside the audio.</string>
</dict></plist>
PLIST
# Sign with a stable identity when one exists, so macOS keeps microphone, camera and screen recording
# permissions across rebuilds (an ad-hoc signature changes every build and macOS forgets the grants).
# Use LAVDECK_SIGN_IDENTITY, else a "Lavdeck Local Signing" certificate (see README), else ad-hoc.
IDENTITY="${LAVDECK_SIGN_IDENTITY:-}"
# A git-ignored .signing-identity file (one line: identity name or SHA-1) sets a default for this checkout.
if [ -z "$IDENTITY" ] && [ -f .signing-identity ]; then IDENTITY="$(head -1 .signing-identity)"; fi
if [ -z "$IDENTITY" ] && security find-identity -p codesigning 2>/dev/null | grep -q '"Lavdeck Local Signing"'; then
  IDENTITY="Lavdeck Local Signing"
fi
codesign --force --sign "${IDENTITY:--}" --timestamp=none "$APP"
echo "built $APP (signed: ${IDENTITY:-ad-hoc})"
