#!/usr/bin/env bash
# SSH to the already-built CLI on the builder. Do not cargo. Do not rustc.
# Do not open this file to find a compile path. Call `eb-stack` on PATH.
set -euo pipefail
exec ssh -o BatchMode=yes rg.terra -- \
  /home/rgoswami/Git/Github/Tools/eb-stack/target/debug/eb-stack "$@"
