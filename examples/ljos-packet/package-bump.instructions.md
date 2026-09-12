# packset set `package-bump`

Pin with `PACKSET_WORKSPACE=git:github.com/HaoZeke/eb-stack packset pin package-bump`.
Atoms are `package-bump.jsonl`. This file is the set's instructions card.

Version-one CLI is `eb-stack package inspect|plan|bump`. If `eb-stack package --help` fails or `--help` lists solve/bump/check-recipe, that binary is stale: run the current checkout on the configured builder.

Robot trees and `--out-dir` live on that builder. Do not test those paths on the laptop. Discover sources on the builder with `rg --files`.

Bump companions first. Reuse one `--out-dir` so letter/name trees accumulate. Pass the overlay as a later `--easyconfigs`.

No EasyBuild recipe for a companion: `package plan` the foreign source (Spack/conda) with `--package-config`. Do not write a `.eb` by hand.

A version bump clears the old checksum and git commit. Fill with `eb --inject-checksums`. Do not copy hashes across artifact classes.

Site generation is the site default, not newest-on-develop. Package extras belong in `--package-config`.
