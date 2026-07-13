#!/usr/bin/env bash
set -euo pipefail
export PATH="${HOME}/.venvs/easybuild/bin:${HOME}/.local/bin:${HOME}/Git/Github/Tools/eb-stack/target/release:${PATH}"
export LMOD_CMD="${HOME}/.local/lmod/lmod/libexec/lmod"
export FPATH="${FPATH:-}"
source "${HOME}/.local/lmod/lmod/init/bash" 2>/dev/null || true
export EASYBUILD_MODULES_TOOL=Lmod
export EASYBUILD_SOURCEPATH="${HOME}/tmp/eb-sources"
export EASYBUILD_TMPDIR="${HOME}/tmp/eb-tmp"
export EASYBUILD_INSTALLPATH="${HOME}/tmp/eb-install"
WORK="${HOME}/tmp/eb-repro-v2"
REPO="${HOME}/Git/Github/Tools/eb-stack"
ROBOT="${HOME}/Git/Github/easybuilders/easybuild-easyconfigs/easybuild/easyconfigs"
EB="${REPO}/target/release/eb-stack"
mkdir -p "${EASYBUILD_SOURCEPATH}" "${EASYBUILD_TMPDIR}" "${EASYBUILD_INSTALLPATH}" "${WORK}/logs" "${WORK}/residuals"
EON="${WORK}/easyconfigs/e/eOn/eOn-2.16.0-foss-2026.1.eb"
QMC="${WORK}/easyconfigs/q/QMCPACK/QMCPACK-4.3.0-foss-2026.1.eb"
cp -a "${REPO}/fixtures/qmcpack_foss_2026_1/easyconfigs/q/QMCPACK/QMCPACK-4.3.0-foss-2026.1.eb" "${QMC}"
cp -a "${REPO}/fixtures/eon_foss_2026_1/easyconfigs/e/eOn/eOn-2.16.0-foss-2026.1.eb" "${EON}"
rsync -a --exclude "e/eOn/eOn-2.16.0-foss-2026.1.eb" \
  "${REPO}/fixtures/eon_foss_2026_1/easyconfigs/" "${WORK}/easyconfigs/"
gate() {
  local f="$1"
  echo "===== format-style $(basename "$f") ====="
  "$EB" format-style "$f" || true
  "$EB" check-style "$f"
  sed -i "s/[[:space:]]*$//" "$f"
  echo "===== check-contrib ====="
  eb --modules-tool=Lmod --check-contrib "$f"
  echo "===== check-recipe ====="
  "$EB" check-recipe --recipe "$f" --easyconfigs "$ROBOT" --easyconfigs "${WORK}/easyconfigs"
  echo "===== eb -Dr ====="
  eb --modules-tool=Lmod -Dr --robot "${WORK}/easyconfigs:${ROBOT}" "$f"
}
gate "$QMC"
gate "$EON"
echo "===== ROBOT QMCPACK ====="
set +e
eb --modules-tool=Lmod --robot "${WORK}/easyconfigs:${ROBOT}" "$QMC" 2>&1 | tee "${WORK}/logs/robot-qmcpack.log"
qmc_rc=${PIPESTATUS[0]}
set -e
echo "qmc_robot_exit=${qmc_rc}"
echo "===== ROBOT eOn ====="
set +e
eb --modules-tool=Lmod --robot "${WORK}/easyconfigs:${ROBOT}" "$EON" 2>&1 | tee "${WORK}/logs/robot-eon.log"
eon_rc=${PIPESTATUS[0]}
set -e
echo "eon_robot_exit=${eon_rc}"
{
  echo "# session-log full-drive"
  echo
  echo "## QMCPACK 4.3.0 foss-2026.1"
  echo "- resolves: yes"
  if [[ $qmc_rc -eq 0 ]]; then echo "- builds: yes (log robot-qmcpack.log)"; else echo "- builds: no (exit ${qmc_rc}, log robot-qmcpack.log)"; fi
  echo "- binary-verified: no"
  echo
  echo "## eOn 2.16.0 foss-2026.1"
  echo "- resolves: yes"
  if [[ $eon_rc -eq 0 ]]; then echo "- builds: yes (log robot-eon.log)"; else echo "- builds: no (exit ${eon_rc}, log robot-eon.log)"; fi
  echo "- binary-verified: no"
  echo
  if [[ $qmc_rc -eq 0 && $eon_rc -eq 0 ]]; then echo "DONE_FULL_DRIVE"; else echo "DONE_PARTIAL qmc=${qmc_rc} eon=${eon_rc}"; fi
} | tee "${WORK}/residuals/session-log.md"
if [[ $qmc_rc -ne 0 || $eon_rc -ne 0 ]]; then exit 1; fi
