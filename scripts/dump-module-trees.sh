#!/usr/bin/env bash
# Capture the live module inventories that `eb-stack stack gap` compares.
#
# Usage:
#   dump-module-trees.sh OUT_DIR SITE_MODULE OLDER_EESSI_MODULE TARGET_EESSI_MODULE
#
# Pass - for SITE_MODULE to capture the base site tree without loading a
# meta-module. Run this from a shell where the `module` command is initialized.
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: $0 OUT_DIR SITE_MODULE OLDER_EESSI_MODULE TARGET_EESSI_MODULE" >&2
  exit 2
fi

readonly out_dir=$1
readonly site_module=$2
readonly older_module=$3
readonly target_module=$4

if ! type module >/dev/null 2>&1; then
  echo "dump-module-trees: the module command is not initialized" >&2
  exit 2
fi

mkdir -p "$out_dir"

dump_tree() {
  local label=$1
  local selector=$2
  local output=$3
  local temporary="${output}.tmp"

  (
    module --force purge >/dev/null 2>&1
    if [[ $selector != - ]]; then
      module load "$selector" >/dev/null 2>&1
    fi
    module -t avail
  ) >"$temporary" 2>&1
  mv "$temporary" "$output"
  printf '%s\t%s\n' "$label" "$output"
}

trap 'rm -f "$out_dir"/*.tmp' EXIT
dump_tree site "$site_module" "$out_dir/site.txt"
dump_tree older "$older_module" "$out_dir/eessi-older.txt"
dump_tree target "$target_module" "$out_dir/eessi-target.txt"
