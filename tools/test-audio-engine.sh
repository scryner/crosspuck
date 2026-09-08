#!/usr/bin/env sh
set -eu
root_dir="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$root_dir/target/audio-tests"
xcrun clang -fobjc-arc -std=gnu11 -O1 -g -Wall -Wextra -Werror \
  -fsanitize=address,undefined -fno-omit-frame-pointer \
  -mmacosx-version-min=14.2 -framework Foundation -framework CoreAudio \
  "$root_dir/tools/audio-route-poc/engine-tests.m" \
  -o "$root_dir/target/audio-tests/engine-tests"
"$root_dir/target/audio-tests/engine-tests"
