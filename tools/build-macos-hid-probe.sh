#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
probe_dir="$script_dir/macos_hid_probe"
build_dir="$probe_dir/build"

mkdir -p "$build_dir"

cc \
  -dynamiclib \
  -O2 \
  -Wall \
  -Wextra \
  -framework IOKit \
  -framework CoreFoundation \
  -o "$build_dir/libcrosspuck_host_hid_probe.dylib" \
  "$probe_dir/host_hid_probe.c"

codesign --force --sign - "$build_dir/libcrosspuck_host_hid_probe.dylib" >/dev/null 2>&1 || true

echo "$build_dir/libcrosspuck_host_hid_probe.dylib"
