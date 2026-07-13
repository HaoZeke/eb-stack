#!/usr/bin/env bash
# Example: render full-drive for eOn + QMCPACK landable fixtures (not a hard-coded drive body).
set -euo pipefail
SKILL="$(cd "$(dirname "$0")/.." && pwd)"
REPO="${REPO:-$(cd "$SKILL/../.." && pwd)}"
WORK="${WORK:-$HOME/tmp/eb-repro-v2}"
ROBOT="${ROBOT:-$HOME/Git/Github/easybuilders/easybuild-easyconfigs/easybuild/easyconfigs}"

"$SKILL/render-full-drive" \
  --work "$WORK" \
  --repo "$REPO" \
  --robot "$ROBOT" \
  --overlay "$REPO/fixtures/eon_foss_2026_1/easyconfigs" \
  --recipe "$WORK/easyconfigs/q/QMCPACK/QMCPACK-4.3.0-foss-2026.1.eb" \
  --oracle "fixtures/qmcpack_foss_2026_1/easyconfigs/q/QMCPACK/QMCPACK-4.3.0-foss-2026.1.eb" \
  --stem "qmcpack" \
  --recipe "$WORK/easyconfigs/e/eOn/eOn-2.16.0-foss-2026.1.eb" \
  --oracle "fixtures/eon_foss_2026_1/easyconfigs/e/eOn/eOn-2.16.0-foss-2026.1.eb" \
  --stem "eon"
