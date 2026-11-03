#!/usr/bin/env bash
# Capture the live module inventories that `eb-stack stack gap` compares.
#
# Usage:
#   dump-module-trees.sh OUT_DIR SITE_MODULE OLDER_EESSI_MODULE TARGET_EESSI_MODULE
#
# Pass - for SITE_MODULE to capture the base site tree without loading a
# meta-module. When the shell has no `module` function, the helper sources
# $LMOD_PKG/init/bash, then $MODULESHOME/init/bash. Each tree's purge and
# load output goes to OUT_DIR/<name>.module.log. A failed purge or load, or a
# meta-module that is not loaded afterwards, exits without writing that tree.
set -euo pipefail

readonly exit_failure=1
readonly exit_usage=2

if [[ $# -ne 4 ]]; then
  echo "usage: $0 OUT_DIR SITE_MODULE OLDER_EESSI_MODULE TARGET_EESSI_MODULE" >&2
  exit "$exit_usage"
fi

readonly out_dir=$1
readonly site_module=$2
readonly older_module=$3
readonly target_module=$4

module_is_function() {
  [[ $(type -t module) == function ]]
}

# A non-interactive bash does not read the profile that defines `module`.
initialize_module() {
  local init
  module_is_function && return 0
  for init in "${LMOD_PKG:+$LMOD_PKG/init/bash}" \
    "${MODULESHOME:+$MODULESHOME/init/bash}"; do
    [[ -n $init && -e $init ]] || continue
    # Lmod init is not nounset-safe, and a site's init may run a bare command
    # that fails. Check for the function afterwards instead.
    set +eu
    # shellcheck disable=SC1090
    source "$init"
    set -eu
    module_is_function && return 0
  done
  echo "dump-module-trees: the module command is not initialized," \
    "and neither \$LMOD_PKG/init/bash nor \$MODULESHOME/init/bash exists" >&2
  exit "$exit_usage"
}

initialize_module
mkdir -p "$out_dir"

fail_tree() {
  local label=$1
  local message=$2
  local log=$3
  echo "dump-module-trees: ${label} tree: ${message}" >&2
  cat "$log" >&2
  exit "$exit_failure"
}

dump_tree() {
  local label=$1
  local selector=$2
  local output=$3
  local temporary="${output}.tmp"
  local log="${output%.txt}.module.log"

  current_temporary=$temporary
  (
    module --force purge >"$log" 2>&1 ||
      fail_tree "$label" "module --force purge failed" "$log"
    if [[ $selector != - ]]; then
      module load "$selector" >>"$log" 2>&1 ||
        fail_tree "$label" "module load ${selector} failed" "$log"
      module is-loaded "$selector" >>"$log" 2>&1 ||
        fail_tree "$label" "${selector} is not loaded after module load" "$log"
    fi
    module -t avail >"$temporary" 2>&1
  )
  mv "$temporary" "$output"
  current_temporary=
  printf '%s\t%s\n' "$label" "$output"
}

# The inventory being written, removed on exit if it was never moved into place.
current_temporary=
trap '[[ -z $current_temporary ]] || rm -f -- "$current_temporary"' EXIT
readonly site_output="$out_dir/site.txt"
readonly older_output="$out_dir/eessi-older.txt"
readonly target_output="$out_dir/eessi-target.txt"

# A failed run must not leave an earlier run's inventory behind, including the
# trees after the one that failed.
rm -f -- "$site_output" "$older_output" "$target_output"
dump_tree site "$site_module" "$site_output"
dump_tree older "$older_module" "$older_output"
dump_tree target "$target_module" "$target_output"
