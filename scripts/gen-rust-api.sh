#!/usr/bin/env bash
# Generate Sphinx RST for the eb_stack crate via sphinx-rustdocgen.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

GEN="${SPHINX_RUSTDOCGEN:-}"
if [[ -z "$GEN" ]]; then
  if command -v sphinx-rustdocgen >/dev/null 2>&1; then
    GEN=$(command -v sphinx-rustdocgen)
  elif [[ -x "$HOME/.cargo/bin/sphinx-rustdocgen" ]]; then
    GEN="$HOME/.cargo/bin/sphinx-rustdocgen"
  else
    echo "sphinx-rustdocgen not found; install with:" >&2
    echo "  cargo install sphinx-rustdocgen" >&2
    exit 1
  fi
fi

rm -rf docs/source/crates/eb_stack
mkdir -p docs/source/crates
echo "using $GEN"
"$GEN" '{"crate_name":"eb_stack","crate_dir":".","doc_dir":"docs/source/crates","force":true,"strip_src":true}'
test -f docs/source/crates/eb_stack/lib.rst
echo "generated docs/source/crates/eb_stack/"
