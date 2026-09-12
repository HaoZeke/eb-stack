#!/usr/bin/env bash
# Launch Willma Hermes inside rg-agent-sandbox --no-seat-home.
# This is the seat launcher. It is not a golden overlay script.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
out=${1:?usage: jail-hermes.sh OUT_DIR}
query=${2:?usage: jail-hermes.sh OUT_DIR QUERY}

scratch=${JAIL_SCRATCH:-$HOME/var/scratch/2026-09/eb-stack-willma-replay}
grok_home=${GROK_HOME:-$scratch/grok-home}
hermes_home=${HERMES_HOME:-$scratch/hermes-home}
ha=$HOME/.local/share/uv/tools/hermes-agent
py=$HOME/.local/share/uv/python/cpython-3.13.5-linux-x86_64-gnu

mkdir -p "$out" "$grok_home" "$HOME/.ssh/sockets" "$out/bin"
ln -sfn "$root/skills/golden-replay/run-on-builder.sh" "$out/bin/eb-stack"
cat >"$out/bin/cargo" <<'EOF'
#!/bin/sh
printf '%s\n' "cargo is not in this jail. Use eb-stack on PATH." >&2
exit 127
EOF
cat >"$out/bin/rustc" <<'EOF'
#!/bin/sh
printf '%s\n' "rustc is not in this jail. Use eb-stack on PATH." >&2
exit 127
EOF
chmod +x "$out/bin/cargo" "$out/bin/rustc"
cat >"$out/paths.env" <<EOF
ROBOT=/home/rgoswami/Git/tmp/eon-pr26480-check/easybuild/easyconfigs
SPACK=/home/rgoswami/Git/tmp/spack-packages-eon/repos/spack_repo/builtin/packages/py_pspamm/package.py
CFG=$root/examples/packages
OUT=$out
EOF
export GROK_HOME=$grok_home
export HERMES_HOME=$hermes_home
export PACKSET_URL=${PACKSET_URL:-http://127.0.0.1:8761}
export PACKSET_WORKSPACE=${PACKSET_WORKSPACE:-git:github.com/HaoZeke/eb-stack}
export PATH="$out/bin:$HOME/.local/bin:/usr/bin:/bin"
cd "$out"

exec rg-agent-sandbox --no-seat-home \
  --allow "$root" \
  --allow "$out" \
  --allow "$hermes_home" \
  --allow "$grok_home" \
  --allow "$ha" \
  --allow "$py" \
  --allow "$HOME/.ssh/config" \
  --allow "$HOME/.ssh/id_ed25519" \
  --allow "$HOME/.ssh/id_ed25519.pub" \
  --allow "$HOME/.ssh/known_hosts" \
  --allow "$HOME/.ssh/sockets" \
  -- "$ha/bin/python" "$ha/bin/hermes" chat --yolo --accept-hooks --max-turns 30 --source tool \
      -m openai/gpt-oss-120b --provider willma \
      -s golden-replay \
      -q "$query"
