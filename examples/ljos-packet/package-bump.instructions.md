# packset set `package-bump`

Pin with `PACKSET_WORKSPACE=git:github.com/HaoZeke/eb-stack packset pin package-bump`.
Atoms are `package-bump.jsonl`. This file is the set's instructions card.

packset and ljos run on the laptop. Do not ssh them to the builder.

For the SeisSol 1.3.2 foss-2025a overlay, the next command after pin is
`skills/golden-replay/replay.sh`. Do not locate the binary. Do not write a plan.

Version-one CLI is `eb-stack package inspect|plan|bump`. A binary whose
help lists solve and check-recipe is stale. `replay.sh` calls
`run-on-builder.sh` so a golden replay never needs that path by hand.

Robot trees and `--out-dir` live on the builder. `--out-dir` is the
bundle root. The tool writes `easyconfigs/` under it.

Bump companions first. Reuse one `--out-dir`. Pass the overlay as a
later `--easyconfigs`.

No EasyBuild recipe: `package plan` the foreign source with
`--package-config`. Do not write a `.eb` by hand.

A version bump clears the old checksum and git commit. Fill with
`eb --inject-checksums`. Do not copy hashes across artifact classes.
Site generation is the site default. Package extras belong in
`--package-config`.
