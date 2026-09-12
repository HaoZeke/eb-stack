#!/usr/bin/env bash
# Compare an emitted overlay to the SeisSol golden required assignments.
# Exit 0 only when every required fact is present. Whitespace and
# dependency order are ignored.
set -euo pipefail

overlay=${1:?usage: check-overlay.sh OVERLAY_EASYCONFIGS_DIR}
fail=0

need() {
  local file=$1
  local pattern=$2
  if ! rg -q --fixed-strings "$pattern" "$file"; then
    printf 'missing in %s: %s\n' "$file" "$pattern" >&2
    fail=1
  fi
}

asagi=$overlay/a/ASAGI/ASAGI-1.0-foss-2025a.eb
easi=$overlay/e/easi/easi-1.7.0-foss-2025a.eb
pspamm=$overlay/p/PSpaMM/PSpaMM-0.3.1-gfbf-2025a.eb
seissol=$overlay/s/SeisSol/SeisSol-1.3.2-foss-2025a.eb

for f in "$asagi" "$easi" "$pspamm" "$seissol"; do
  if [[ ! -f $f ]]; then
    printf 'missing file: %s\n' "$f" >&2
    fail=1
  fi
done

if [[ -f $asagi ]]; then
  need "$asagi" "foss"
  need "$asagi" "2025a"
  need "$asagi" "ASAGI-1.0_fix-level.h.patch"
  if [[ ! -f $overlay/a/ASAGI/ASAGI-1.0_fix-level.h.patch ]]; then
    printf 'missing patch beside ASAGI recipe\n' >&2
    fail=1
  fi
fi

if [[ -f $easi ]]; then
  if rg -q "^\s*\(['\"]ImpalaJIT['\"]" "$easi"; then
    printf 'easi still declares ImpalaJIT\n' >&2
    fail=1
  fi
  need "$easi" "Lua"
  need "$easi" "yaml-cpp"
fi

if [[ -f $pspamm ]]; then
  need "$pspamm" "modulename"
  need "$pspamm" "pypspamm"
fi

if [[ -f $seissol ]]; then
  need "$seissol" "20334d02af9c539a9cf7cc4cd03c3e2622104aea"
  need "$seissol" "a0dd4f33d7d72c4a5eb6f01f0a96bc410af20c95a496e5cf1532ee8a20561612"
  need "$seissol" "Eigen"
  need "$seissol" "PSpaMM"
  need "$seissol" "numactl"
  need "$seissol" "HOST_ARCH=hsw"
fi

exit "$fail"
