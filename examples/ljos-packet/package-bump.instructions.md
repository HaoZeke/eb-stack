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

Three review-fix lessons in `package-bump.jsonl` are pack atoms, the same
store as the bump lessons above. They are not rows in
`$XDG_CONFIG_HOME/ljos/user-rules.json`. eb-stack does not call `ljos prefer`
or `ljos rule`. A person who wants one of them on every seat promotes it
with `ljos prefer --user --promote LESSON_ID --why REASON` (leidarljos/ljos
PR #128). `ljos rule --user PATTERN --verdict deny|ask --why REASON` writes
an argv law into that same user-rule file. This repository does not ship
that client.

- EasyBuild values stay `%(name)s`, `%(version)s`, and `%(builddir)s`
  templates. Use an f-string only for a Python constant such as `SHLIB_EXT`,
  and never without a placeholder.
- Apply a review fix to every occurrence in the easyconfig. Search the whole
  file before editing.
- Run `recipe lint` and leave it clean before calling a review fix done.
