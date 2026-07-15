#!/usr/bin/env bash
# Sphinx build with an optional system venv that has sphinxcontrib-rust.
# Falls back to pixi env if the extension is already importable there.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

pick_python() {
  if [[ -n "${EB_STACK_DOCS_PYTHON:-}" && -x "${EB_STACK_DOCS_PYTHON}" ]]; then
    echo "$EB_STACK_DOCS_PYTHON"
    return
  fi
  # Prefer a venv whose native extension uses the host Rust/C toolchain.
  for cand in \
    "$ROOT/.venv-docs/bin/python" \
    "$HOME/.local/share/eb-stack-docs-venv/bin/python"; do
    if [[ -x "$cand" ]] && "$cand" -c 'import sphinxcontrib_rust' 2>/dev/null; then
      echo "$cand"
      return
    fi
  done
  if python3 -c 'import sphinxcontrib_rust' 2>/dev/null; then
    echo python3
    return
  fi
  # Last resort: pixi docs env (may lack sphinxcontrib-rust)
  if command -v pixi >/dev/null 2>&1; then
    # shellcheck disable=SC2016
    echo "pixi"
    return
  fi
  echo "python3"
}

PY=$(pick_python)
echo "sphinx-build via: $PY"
if [[ "$PY" == "pixi" ]]; then
  pixi run -e docs -- python -c 'import sphinxcontrib_rust' 2>/dev/null || {
    echo "ERROR: sphinxcontrib_rust not importable." >&2
    echo "Create a docs venv using the host Rust/C linker:" >&2
    echo "  python3 -m venv .venv-docs" >&2
    echo "  .venv-docs/bin/pip install 'sphinx>=9,<10' shibuya sphinx-sitemap \\" >&2
    echo "    sphinx-copybutton sphinx-design 'sphinxcontrib-rust>=1,<2' \\" >&2
    echo "    'sphinx-rustdoc-postprocess>=0.1,<0.2'" >&2
    echo "  # with: export RUSTFLAGS='-C linker=cc' CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc" >&2
    exit 1
  }
  pixi run -e docs -- sphinx-build -W --keep-going docs/source/ docs/build
else
  "$PY" -c 'import sphinxcontrib_rust' || {
    echo "ERROR: $PY cannot import sphinxcontrib_rust" >&2
    exit 1
  }
  "$PY" -m sphinx -W --keep-going -b html docs/source/ docs/build
fi
