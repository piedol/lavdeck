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
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSUIElement</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Lavdeck records from your DJI Mic receiver.</string>
</dict></plist>
PLIST
codesign --force --sign - --timestamp=none "$APP"
echo "built $APP"
