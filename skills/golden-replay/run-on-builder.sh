#!/usr/bin/env bash
# Run argv as the current checkout's eb-stack on the configured builder.
# With no argv, run replay.sh. Paths in argv are evaluated on the builder.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
if [[ $# -eq 0 ]]; then
  exec "$here/replay.sh"
fi

builder=${EB_STACK_BUILDER:-rg.terra}

if [[ -z ${EB_STACK_BIN:-} ]]; then
  EB_STACK_BIN=$(ssh -o BatchMode=yes "$builder" 'bash -s' <<'EOS'
set -euo pipefail
if [[ -x $HOME/Git/Github/Tools/eb-stack/target/debug/eb-stack ]]; then
  printf '%s\n' "$HOME/Git/Github/Tools/eb-stack/target/debug/eb-stack"
  exit 0
fi
found=$(rg --files "$HOME/Git" --glob 'target/debug/eb-stack' 2>/dev/null | head -1 || true)
if [[ -n ${found:-} && -x $found ]]; then
  printf '%s\n' "$found"
  exit 0
fi
printf 'no eb-stack debug binary on the builder\n' >&2
exit 1
EOS
)
fi

exec ssh -o BatchMode=yes "$builder" -- "$EB_STACK_BIN" "$@"
