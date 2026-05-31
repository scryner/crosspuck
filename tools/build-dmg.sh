#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  tools/build-dmg.sh [--profile debug|release] [--app PATH] [--output PATH] [--volume-name NAME] [--volume-icon PATH] [--no-build] [--dmg-sign-identity ID]

Builds a drag-and-drop macOS DMG containing CrossPuck.app and an Applications
symlink in the conventional Finder layout. The default flow builds
target/release/CrossPuck.app first, then creates
target/dmg/CrossPuck-<version>.dmg.

Options:
  --profile NAME             App build profile when --app is not provided. Defaults to release.
  --app PATH                 Prebuilt CrossPuck.app to package. Useful after CI codesigning.
  --output PATH              DMG output path. Defaults to target/dmg/CrossPuck-<version>.dmg.
  --volume-name NAME         Mounted DMG volume name. Defaults to "CrossPuck <version>".
  --volume-icon PATH         ICNS icon for the mounted DMG volume. Defaults to the app icon.
  --no-build                 Package target/<profile>/CrossPuck.app without rebuilding it.
  --dmg-sign-identity ID     Optional codesign identity for signing the generated DMG.
  --help                     Show this help.

This script does not notarize. In CI, notarize and staple either the signed app
before packaging, or the generated signed DMG after this script finishes.
USAGE
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root_dir="$(cd "$script_dir/.." && pwd)"

profile="release"
app_path=""
output_path=""
volume_name=""
volume_icon_path=""
no_build="0"
dmg_sign_identity="${CROSSPUCK_DMG_SIGN_IDENTITY:-}"

workspace_version() {
  awk '
    /^\[workspace.package\]/ { in_workspace_package = 1; next }
    /^\[/ { in_workspace_package = 0 }
    in_workspace_package && /^[[:space:]]*version[[:space:]]*=/ {
      value = $0
      sub(/^[^=]*=[[:space:]]*"/, "", value)
      sub(/".*$/, "", value)
      print value
      exit
    }
  ' "$root_dir/Cargo.toml"
}

absolute_path() {
  local path="$1"
  local dir
  local base

  dir="$(dirname "$path")"
  base="$(basename "$path")"
  mkdir -p "$dir"
  dir="$(cd "$dir" && pwd)"
  printf '%s/%s\n' "$dir" "$base"
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Required command not found: $1" >&2
    exit 1
  fi
}

find_setfile() {
  if command -v SetFile >/dev/null 2>&1; then
    command -v SetFile
    return 0
  fi

  xcrun -find SetFile 2>/dev/null || true
}

set_custom_icon_flag() {
  local target="$1"
  local setfile

  setfile="$(find_setfile)"
  if [[ -z "$setfile" ]]; then
    echo "Warning: SetFile not found; the DMG volume icon may not be marked as custom." >&2
    return 0
  fi

  "$setfile" -a C "$target"
}

generate_background_image() {
  local output="$1"
  local generator="$tmp_root/generate-dmg-background.swift"

  cat > "$generator" <<'SWIFT'
import AppKit

let output = CommandLine.arguments[1]
let size = NSSize(width: 760, height: 420)
var mediaBox = CGRect(x: 0, y: 0, width: size.width, height: size.height)
guard let consumer = CGDataConsumer(url: URL(fileURLWithPath: output) as CFURL),
      let pdf = CGContext(consumer: consumer, mediaBox: &mediaBox, nil) else {
    fputs("failed to create DMG background PDF\n", stderr)
    exit(1)
}

pdf.beginPDFPage(nil)
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(cgContext: pdf, flipped: false)

NSColor(calibratedRed: 0.33, green: 0.46, blue: 0.58, alpha: 1.0).setFill()
NSBezierPath(rect: NSRect(origin: .zero, size: size)).fill()

let arrow = NSBezierPath()
arrow.move(to: NSPoint(x: 323, y: 208))
arrow.line(to: NSPoint(x: 410, y: 208))
arrow.line(to: NSPoint(x: 410, y: 232))
arrow.line(to: NSPoint(x: 448, y: 196))
arrow.line(to: NSPoint(x: 410, y: 160))
arrow.line(to: NSPoint(x: 410, y: 184))
arrow.line(to: NSPoint(x: 323, y: 184))
arrow.close()
NSColor(calibratedWhite: 1.0, alpha: 0.30).setFill()
arrow.fill()
NSColor(calibratedWhite: 1.0, alpha: 0.42).setStroke()
arrow.lineWidth = 1.25
arrow.stroke()

let paragraph = NSMutableParagraphStyle()
paragraph.alignment = .center
let instructionAttrs: [NSAttributedString.Key: Any] = [
    .font: NSFont.systemFont(ofSize: 18, weight: .semibold),
    .foregroundColor: NSColor(calibratedWhite: 1.0, alpha: 0.76),
    .paragraphStyle: paragraph
]
"DRAG TO INSTALL".draw(
    in: NSRect(x: 285, y: 116, width: 200, height: 28),
    withAttributes: instructionAttrs
)

NSGraphicsContext.restoreGraphicsState()
pdf.endPDFPage()
pdf.closePDF()
SWIFT

  swift "$generator" "$output"
}

apply_finder_layout() {
  local mounted_volume="$1"
  local mounted_name

  mounted_name="$(basename "$mounted_volume")"

  osascript <<APPLESCRIPT
tell application "Finder"
  tell disk "$mounted_name"
    open
    delay 1
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set bounds of container window to {160, 100, 920, 520}
    set theViewOptions to icon view options of container window
    set backgroundImage to POSIX file "$mounted_volume/.background/background.pdf" as alias
    set arrangement of theViewOptions to not arranged
    set icon size of theViewOptions to 128
    set text size of theViewOptions to 10
    set background picture of theViewOptions to backgroundImage
    set position of item "CrossPuck.app" of container window to {190, 220}
    set position of item "Applications" of container window to {570, 220}
    update without registering applications
    delay 1
    close
  end tell
end tell
APPLESCRIPT

  if command -v bless >/dev/null 2>&1; then
    bless --folder "$mounted_volume" --openfolder "$mounted_volume" >/dev/null 2>&1 || true
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile)
      profile="${2:?missing value for --profile}"
      shift 2
      ;;
    --app)
      app_path="${2:?missing value for --app}"
      shift 2
      ;;
    --output)
      output_path="${2:?missing value for --output}"
      shift 2
      ;;
    --volume-name)
      volume_name="${2:?missing value for --volume-name}"
      shift 2
      ;;
    --volume-icon)
      volume_icon_path="${2:?missing value for --volume-icon}"
      shift 2
      ;;
    --no-build)
      no_build="1"
      shift
      ;;
    --dmg-sign-identity)
      dmg_sign_identity="${2:?missing value for --dmg-sign-identity}"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$profile" in
  debug|release)
    ;;
  *)
    echo "Invalid profile: $profile" >&2
    echo "Expected debug or release." >&2
    exit 2
    ;;
esac

require_command hdiutil
require_command ditto
require_command osascript
require_command swift

version="$(workspace_version)"
if [[ -z "$version" ]]; then
  echo "Could not read workspace package version from Cargo.toml" >&2
  exit 1
fi

if [[ -z "$volume_name" ]]; then
  volume_name="CrossPuck $version"
fi

if [[ -z "$app_path" ]]; then
  app_path="$root_dir/target/$profile/CrossPuck.app"
  if [[ "$no_build" != "1" ]]; then
    "$root_dir/tools/build-app.sh" "$profile"
  fi
else
  no_build="1"
fi

if [[ ! -d "$app_path" ]]; then
  echo "App bundle not found: $app_path" >&2
  if [[ "$no_build" == "1" ]]; then
    echo "Build it first, or pass --app PATH." >&2
  fi
  exit 1
fi

if [[ -z "$volume_icon_path" ]]; then
  default_app_icon="$app_path/Contents/Resources/CrossPuck.icns"
  if [[ -f "$default_app_icon" ]]; then
    volume_icon_path="$default_app_icon"
  elif [[ -f "$root_dir/crates/crosspuck-app/Resources/CrossPuck.icns" ]]; then
    volume_icon_path="$root_dir/crates/crosspuck-app/Resources/CrossPuck.icns"
  fi
fi

if [[ -n "$volume_icon_path" && ! -f "$volume_icon_path" ]]; then
  echo "Volume icon not found: $volume_icon_path" >&2
  exit 1
fi

if [[ -z "$output_path" ]]; then
  output_path="$root_dir/target/dmg/CrossPuck-$version.dmg"
fi
output_path="$(absolute_path "$output_path")"
mkdir -p "$(dirname "$output_path")"

tmp_root="$(mktemp -d "${TMPDIR:-/tmp}/crosspuck-dmg.XXXXXX")"
cleanup() {
  rm -rf "$tmp_root"
}
trap cleanup EXIT

staging_dir="$tmp_root/staging"
mkdir -p "$staging_dir"

app_name="$(basename "$app_path")"
ditto --rsrc --extattr --acl "$app_path" "$staging_dir/$app_name"
ln -s /Applications "$staging_dir/Applications"

if [[ -n "$volume_icon_path" ]]; then
  cp "$volume_icon_path" "$staging_dir/.VolumeIcon.icns"
  set_custom_icon_flag "$staging_dir"
fi

background_dir="$staging_dir/.background"
mkdir -p "$background_dir"
generate_background_image "$background_dir/background.pdf"

rm -f "$output_path"
rw_image="$tmp_root/CrossPuck-rw.dmg"
hdiutil create \
  -volname "$volume_name" \
  -srcfolder "$staging_dir" \
  -format UDRW \
  -ov \
  "$rw_image"

mount_output="$(hdiutil attach -nobrowse -readwrite "$rw_image")"
mount_point="$(printf '%s\n' "$mount_output" | awk -F'\t' 'NF >= 3 {print $3}' | tail -n 1)"
if [[ -z "$mount_point" ]]; then
  echo "Could not determine mounted DMG path for icon customization" >&2
  printf '%s\n' "$mount_output" >&2
  exit 1
fi

detach_mounted_dmg() {
  hdiutil detach "$mount_point" >/dev/null || true
}
trap 'detach_mounted_dmg; cleanup' EXIT

if [[ -n "$volume_icon_path" ]]; then
  cp "$volume_icon_path" "$mount_point/.VolumeIcon.icns"
  set_custom_icon_flag "$mount_point"
fi
apply_finder_layout "$mount_point"
if [[ -n "$volume_icon_path" ]]; then
  cp "$volume_icon_path" "$mount_point/.VolumeIcon.icns"
  set_custom_icon_flag "$mount_point"
fi

hdiutil detach "$mount_point" >/dev/null
trap cleanup EXIT

hdiutil convert "$rw_image" \
  -format UDZO \
  -o \
  "$output_path"

if [[ -n "$dmg_sign_identity" ]]; then
  require_command codesign
  codesign --force --sign "$dmg_sign_identity" "$output_path"
fi

cat <<EOF
Created CrossPuck DMG:
  $output_path

Volume name:
  $volume_name

Packaged app:
  $app_path
EOF

if [[ -n "$volume_icon_path" ]]; then
  cat <<EOF

Volume icon:
  $volume_icon_path
EOF
fi
