#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
echo "Deprecated: use tools/crosspuck/smoke-check.sh instead." >&2
exec "$repo_root/tools/crosspuck/smoke-check.sh" "$@"
