#!/usr/bin/env sh
set -eu

profile="${1:-debug}"
root_dir="$(cd "$(dirname "$0")/.." && pwd)"
driver_target="x86_64-pc-windows-gnu"
driver_profile="release"
app_features="${CROSSPUCK_APP_FEATURES:-}"

ensure_driver_target_installed() {
  if ! command -v rustup >/dev/null 2>&1; then
    cat >&2 <<EOF
rustup is required to verify the Windows target before building the guest driver.

Install rustup first, then add the driver target:
  rustup target add $driver_target
EOF
    exit 1
  fi

  if ! rustup target list --installed | grep -Fxq "$driver_target"; then
    cat >&2 <<EOF
Required Rust target is not installed:
  $driver_target

Install it first:
  rustup target add $driver_target
EOF
    exit 1
  fi
}

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

set_bundle_version() {
  plist="$1"
  version="$2"

  if [ ! -x /usr/libexec/PlistBuddy ]; then
    echo "PlistBuddy is required to update the app bundle version." >&2
    exit 1
  fi

  /usr/libexec/PlistBuddy \
    -c "Set :CFBundleShortVersionString $version" \
    "$plist"
}

sha256_file() {
  shasum -a 256 "$1" | awk '{print $1}'
}

file_size() {
  wc -c < "$1" | tr -d '[:space:]'
}

case "$profile" in
  debug)
    app_cargo_args=""
    ;;
  release)
    app_cargo_args="--release"
    ;;
  *)
    echo "usage: $0 [debug|release]" >&2
    exit 2
    ;;
esac

app_version="$(workspace_version)"
if [ -z "$app_version" ]; then
  echo "Could not read workspace package version from Cargo.toml" >&2
  exit 1
fi

ensure_driver_target_installed

cargo build \
  --manifest-path "$root_dir/Cargo.toml" \
  -p crosspuck-driver \
  --release \
  --target "$driver_target"

if [ -n "$app_features" ]; then
  # shellcheck disable=SC2086
  cargo build --manifest-path "$root_dir/Cargo.toml" -p crosspuck-app $app_cargo_args --features "$app_features"
else
  # shellcheck disable=SC2086
  cargo build --manifest-path "$root_dir/Cargo.toml" -p crosspuck-app $app_cargo_args
fi

driver_dll="$root_dir/target/$driver_target/$driver_profile/hid.dll"
if [ ! -f "$driver_dll" ]; then
  echo "driver DLL not found after build: $driver_dll" >&2
  exit 1
fi

app_dir="$root_dir/target/$profile/CrossPuck.app"
contents_dir="$app_dir/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"
guest_driver_dir="$resources_dir/GuestDriver"

rm -rf "$app_dir"
mkdir -p "$macos_dir" "$resources_dir" "$guest_driver_dir"
cp "$root_dir/target/$profile/CrossPuck" "$macos_dir/CrossPuck"
cp "$root_dir/crates/crosspuck-app/Info.plist" "$contents_dir/Info.plist"
set_bundle_version "$contents_dir/Info.plist" "$app_version"
cp -R "$root_dir/crates/crosspuck-app/Resources/." "$resources_dir/"
cp "$root_dir/LICENSE" "$resources_dir/LICENSE"
cp "$root_dir/THIRD-PARTY-NOTICES.md" "$resources_dir/THIRD-PARTY-NOTICES.md"
cp "$driver_dll" "$guest_driver_dir/hid.dll"

driver_sha256="$(sha256_file "$guest_driver_dir/hid.dll")"
driver_size="$(file_size "$guest_driver_dir/hid.dll")"
cat > "$guest_driver_dir/manifest.json" <<EOF
{
  "name": "crosspuck-driver",
  "dll_name": "hid.dll",
  "target": "$driver_target",
  "profile": "$driver_profile",
  "sha256": "$driver_sha256",
  "size": $driver_size
}
EOF

echo "$app_dir"
