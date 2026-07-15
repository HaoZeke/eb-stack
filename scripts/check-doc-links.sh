#!/usr/bin/env bash
set -euo pipefail

build_dir="${1:-docs/build}"

if [[ ! -d "$build_dir" ]]; then
  echo "documentation build directory does not exist: $build_dir" >&2
  exit 2
fi

if rg -n 'href="[^"]+\.rst([?#][^"]*)?"' "$build_dir" -g '*.html'; then
  echo "rendered documentation contains links to generated RST files" >&2
  exit 1
fi

if rg -n 'github\.com/HaoZeke/eb-stack/blob/[^"]+/docs/orgmode/[^"]+\.rst' \
  "$build_dir" -g '*.html'; then
  echo "rendered documentation edit links target generated RST instead of org sources" >&2
  exit 1
fi
