#!/bin/bash
# Load EESSI-extend and exec EasyBuild. Runs inside eessi_container.sh.
# EESSI init is not nounset-safe.
set -eo pipefail

ver=${EESSI_VERSION:-2025.06}
init=/cvmfs/software.eessi.io/versions/${ver}/init/lmod/bash
if [[ ! -e $init ]]; then
  echo "eessi-extend-eb: missing ${init}" >&2
  exit 2
fi
# shellcheck disable=SC1090
source "$init"

# Exactly one prefix variable; the module reads them at load time.
n=0
[[ -n ${EESSI_USER_INSTALL:-} ]] && n=$((n + 1))
[[ -n ${EESSI_PROJECT_INSTALL:-} ]] && n=$((n + 1))
[[ -n ${EESSI_SITE_INSTALL:-} ]] && n=$((n + 1))
[[ -n ${EESSI_CVMFS_INSTALL:-} ]] && n=$((n + 1))
if [[ $n -eq 0 ]]; then
  export EESSI_USER_INSTALL=${EESSI_USER_INSTALL:-$HOME/EESSI}
  mkdir -p "$EESSI_USER_INSTALL"
elif [[ $n -gt 1 ]]; then
  echo "eessi-extend-eb: set exactly one of EESSI_USER_INSTALL, EESSI_PROJECT_INSTALL, EESSI_SITE_INSTALL, EESSI_CVMFS_INSTALL" >&2
  exit 3
fi

export WORKING_DIR=${WORKING_DIR:-/tmp}
module load EESSI-extend
exec eb "$@"
