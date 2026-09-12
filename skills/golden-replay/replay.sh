#!/usr/bin/env bash
# One-shot SeisSol 1.3.2 foss-2025a overlay. After packset pin, run this.
# Bumps run through run-on-builder.sh. check-overlay.sh is the gate.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
wrap=$root/skills/golden-replay/run-on-builder.sh
check=$root/skills/golden-replay/check-overlay.sh
cfg=$root/examples/packages
pol=$root/examples/stacks/gfbf-2025a.toml
local_out=${1:-$root/work/seissol-1.3.2}
builder=${EB_STACK_BUILDER:-rg.terra}

remote_home=$(ssh -o BatchMode=yes "$builder" 'printf %s "$HOME"')
remote_out=${EB_STACK_REPLAY_OUT:-$remote_home/var/scratch/2026-09/seissol-1mhy-replay}
remote_cfg=$remote_out/cfg
overlay=$remote_out/easyconfigs

ssh -o BatchMode=yes "$builder" "mkdir -p '$remote_cfg' '$overlay'"
rsync -az \
  "$cfg/asagi.toml" "$cfg/easi.toml" "$cfg/pspamm.toml" "$cfg/seissol.toml" \
  "$pol" \
  "$builder:$remote_cfg/"

discover=$(ssh -o BatchMode=yes "$builder" 'bash -s' <<'EOS'
set -euo pipefail
find_one() {
  local glob=$1
  local hit
  for root in "$HOME/Git/tmp" "$HOME/Git" "$HOME/var/scratch"; do
    [[ -d $root ]] || continue
    hit=$(rg --files "$root" --glob "$glob" 2>/dev/null | head -1 || true)
    if [[ -n ${hit:-} ]]; then
      printf '%s\n' "$hit"
      return 0
    fi
  done
  return 1
}

seissol=$(find_one 'SeisSol-1.1.4-foss-2023a.eb') || {
  printf 'missing SeisSol-1.1.4-foss-2023a.eb on the builder\n' >&2
  exit 1
}
robot=$(cd "$(dirname "$seissol")/../.." && pwd)
asagi=$robot/a/ASAGI/ASAGI-1.0-foss-2023a.eb
easi=$robot/e/easi/easi-1.3.0-foss-2023a.eb
pspamm=$(rg --files "$HOME/Git/tmp" --glob 'package.py' 2>/dev/null | rg 'py_pspamm/package.py$' | head -1 || true)
if [[ -z ${pspamm:-} ]]; then
  pspamm=$(rg --files "$HOME/Git" --glob 'package.py' 2>/dev/null | rg 'py_pspamm/package.py$' | head -1 || true)
fi
if [[ -z ${pspamm:-} ]]; then
  printf 'missing py_pspamm/package.py on the builder\n' >&2
  exit 1
fi
printf '%s\n' "$robot" "$asagi" "$easi" "$seissol" "$pspamm"
EOS
)

mapfile -t found <<<"$discover"
robot=${found[0]}
asagi=${found[1]}
easi=${found[2]}
seissol=${found[3]}
pspamm=${found[4]}

"$wrap" package bump \
  --source "$asagi" \
  --toolchain-name foss --toolchain-version 2025a \
  --package-config "$remote_cfg/asagi.toml" \
  --easyconfigs "$robot" \
  --out-dir "$remote_out"

"$wrap" package bump \
  --source "$easi" \
  --toolchain-name foss --toolchain-version 2025a \
  --version 1.7.0 \
  --package-config "$remote_cfg/easi.toml" \
  --easyconfigs "$robot" --easyconfigs "$overlay" \
  --out-dir "$remote_out"

"$wrap" package plan \
  --source "$pspamm" \
  --toolchain-name gfbf --toolchain-version 2025a \
  --package-config "$remote_cfg/pspamm.toml" \
  --easyconfigs "$robot" --easyconfigs "$overlay" \
  --stack-policy "$remote_cfg/gfbf-2025a.toml" \
  --source-checksum 7d677e1a81897f42daeb9d89cdc596f7ab97b5b829dc4afd4e86804bbaf2af6c \
  --out-dir "$remote_out"

"$wrap" package bump \
  --source "$seissol" \
  --toolchain-name foss --toolchain-version 2025a \
  --version 1.3.2 \
  --source-checksum a0dd4f33d7d72c4a5eb6f01f0a96bc410af20c95a496e5cf1532ee8a20561612 \
  --package-config "$remote_cfg/seissol.toml" \
  --easyconfigs "$robot" --easyconfigs "$overlay" \
  --out-dir "$remote_out"

mkdir -p "$local_out"
rsync -az "$builder:$overlay/" "$local_out/easyconfigs/"
exec "$check" "$local_out/easyconfigs"
