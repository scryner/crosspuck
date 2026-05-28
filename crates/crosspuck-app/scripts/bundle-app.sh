#!/usr/bin/env sh
set -eu

root_dir="$(cd "$(dirname "$0")/../../.." && pwd)"
echo "Deprecated: use tools/build-app.sh instead." >&2
exec "$root_dir/tools/build-app.sh" "$@"
