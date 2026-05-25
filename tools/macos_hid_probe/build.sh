#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"
mkdir -p build

cc \
  -dynamiclib \
  -O2 \
  -Wall \
  -Wextra \
  -framework IOKit \
  -framework CoreFoundation \
  -o build/libcrosspuck_host_hid_probe.dylib \
  host_hid_probe.c

codesign --force --sign - build/libcrosspuck_host_hid_probe.dylib >/dev/null 2>&1 || true

echo "$(pwd)/build/libcrosspuck_host_hid_probe.dylib"
