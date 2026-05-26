#!/usr/bin/env sh
set -eu

profile="${1:-debug}"
root_dir="$(cd "$(dirname "$0")/../.." && pwd)"

case "$profile" in
  debug)
    cargo build -p crosspuck-app
    ;;
  release)
    cargo build -p crosspuck-app --release
    ;;
  *)
    echo "usage: $0 [debug|release]" >&2
    exit 2
    ;;
esac

app_dir="$root_dir/target/$profile/CrossPuck.app"
contents_dir="$app_dir/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"

rm -rf "$app_dir"
mkdir -p "$macos_dir" "$resources_dir"
cp "$root_dir/target/$profile/CrossPuck" "$macos_dir/CrossPuck"
cp "$root_dir/crates/crosspuck-app/Info.plist" "$contents_dir/Info.plist"
cp -R "$root_dir/crates/crosspuck-app/Resources/." "$resources_dir/"

echo "$app_dir"
