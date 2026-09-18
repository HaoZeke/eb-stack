# packset set `package-bump`

Pin with `PACKSET_WORKSPACE=git:github.com/HaoZeke/eb-stack packset pin package-bump`.
Atoms are `package-bump.jsonl`. This file is the set's instructions card.

packset and ljos run on the laptop. Do not ssh them to the builder.

Read `generation_retarget=` and `version_only=` on the first lines of
`package bump` before a second run.

A same-generation version bump (`version_only=true`) is one parent
command. Expect exit 0 and `companion_count=0`. Do not start a companion
campaign. Do not hand-write the `.eb`. PLUMED 2.9.2 → 2.9.3 on foss-2024a
is that case.

For the SeisSol 1.3.2 foss-2025a overlay, the next command after pin is
`eb-stack package bump` of the parent. That overlay is a generation
retarget: follow `companion=` and `re_run=` only when
`generation_retarget=true`. Do not write a helper script. Do not run
`replay.sh`.

On the builder the robot is local. packset and ljos stay on the laptop.

Version-one CLI is `eb-stack package inspect|plan|bump|mutate`. A binary whose
help lists solve and check-recipe is stale.

Robot trees and `--out-dir` live on the builder. `--out-dir` is the
bundle root. The tool writes `easyconfigs/` under it.

On a generation retarget, bump companions first. Reuse one `--out-dir`.
Pass the overlay as a later `--easyconfigs`.

No EasyBuild recipe: `package plan` the foreign source with
`--package-config`. Do not write a `.eb` by hand.

A version bump clears the old checksum and git commit. Fill with
`eb --inject-checksums`. Do not copy hashes across artifact classes.
Site generation is the site default. Package extras belong in
`--package-config`.
