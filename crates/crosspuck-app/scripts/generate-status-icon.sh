#!/usr/bin/env sh
set -eu

root_dir="$(cd "$(dirname "$0")/../../.." && pwd)"
resources_dir="$root_dir/crates/crosspuck-app/Resources"

swift "$root_dir/crates/crosspuck-app/scripts/generate-status-icon.swift" \
  "$resources_dir/CrossPuckStatusTemplate.pdf"

draw_icon() {
  size="$1"
  output="$2"
  magick -size 44x44 xc:none \
    -fill none -stroke black -strokewidth 3.6 \
    -draw 'roundrectangle 3.5,8.5 40.5,35.5 7.5,7.5' \
    -strokewidth 2.8 \
    -draw 'roundrectangle 12,12.6 32,22 4.7,4.7' \
    -fill black -stroke none \
    -draw 'circle 17.2,17.3 17.2,18.9 circle 22,17.3 22,18.9 circle 26.8,17.3 26.8,18.9' \
    -resize "${size}x${size}" "$output"
}

draw_icon 22 "$resources_dir/CrossPuckStatusTemplate.png"
draw_icon 44 "$resources_dir/CrossPuckStatusTemplate@2x.png"
