#!/bin/sh
set -eu
root_dir="$(cd "$(dirname "$0")/../.." && pwd)"
app_dir="$root_dir/target/audio-route-poc/CrossPuckAudioPoC.app"
mkdir -p "$app_dir/Contents/MacOS" "$root_dir/target/audio-route-poc/results"
xcrun clang -fobjc-arc -std=gnu11 -O2 -Wall -Wextra -Werror \
  -mmacosx-version-min=14.2 \
  "$root_dir/tools/audio-route-poc/main.m" \
  -framework Foundation -framework CoreAudio \
  -o "$app_dir/Contents/MacOS/CrossPuckAudioPoC"
cat > "$app_dir/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.github.scryner.crosspuck.audio-poc</string>
<key>CFBundleExecutable</key><string>CrossPuckAudioPoC</string>
<key>CFBundleName</key><string>CrossPuck Audio PoC</string>
<key>CFBundleDisplayName</key><string>CrossPuck Audio PoC</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>14.2</string>
<key>LSUIElement</key><true/>
<key>NSAudioCaptureUsageDescription</key><string>CrossPuck Audio PoC captures only CrossOver audio to test routing game sound to the macOS output device. Capture samples stay on this Mac.</string>
</dict></plist>
PLIST
codesign --force --sign - --identifier com.github.scryner.crosspuck.audio-poc "$app_dir"
echo "$app_dir"
