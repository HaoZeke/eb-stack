#!/usr/bin/env bash
set -euo pipefail

build_dir="${1:-docs/build}"

if [[ ! -d "$build_dir" ]]; then
  echo "documentation build directory does not exist: $build_dir" >&2
  exit 2
fi

search() {
  local pattern="$1"
  if command -v rg >/dev/null 2>&1; then
    rg -n "$pattern" "$build_dir" -g '*.html'
  else
    grep -R -n --include='*.html' -E "$pattern" "$build_dir"
  fi
}

if search 'href="[^"]+\.rst([?#][^"]*)?"'; then
  echo "rendered documentation contains links to generated RST files" >&2
  exit 1
fi

if search 'github\.com/HaoZeke/eb-stack/blob/[^"]+/docs/orgmode/[^"]+\.rst'; then
  echo "rendered documentation edit links target generated RST instead of org sources" >&2
  exit 1
fi

if search '(chatgpt\.com|claude\.ai|perplexity\.ai)'; then
  echo "rendered documentation contains AI-service links" >&2
  exit 1
fi
