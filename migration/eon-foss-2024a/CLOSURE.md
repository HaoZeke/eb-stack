# eOn foss-2024a dependency closure

## Target repo (contribution path)

Recipes land on **HaoZeke/easybuild-easyconfigs** branch `feat/eon-2.16.0-foss-2024a`
(fork of easybuilders/easybuild-easyconfigs). **eb-stack** is the prep/validation tool only.

Local worktree: `/home/rgoswami/Git/tmp/easybuild-easyconfigs-eon`

## Direct eOn closure (`check-recipe` missing=0)

### Resolved from upstream robot (`~/.venvs/easybuild/easybuild/easyconfigs`)

| role | name | version | notes |
|------|------|---------|-------|
| runtime | Python | 3.12.3 | |
| runtime | SciPy-bundle | 2024.05 | |
| runtime | PyYAML | 6.0.2 | |
| runtime | Eigen | 3.4.0 | |
| runtime | Highway | 1.2.0 | |
| runtime | inih | 58 | |
| runtime | xtb | 6.7.1 | toolchain override gfbf-2024a |
| runtime | CapnProto | 1.1.0 | |
| runtime | PyTorch | 2.6.0 | |
| build | Ninja | 1.12.1 | |
| build | pkgconf | 2.2.0 | |
| build | CMake | 3.29.3 | |
| build | cargo-c | 0.9.32 | GCCcore-13.3.0 |

### Companions added on the easyconfigs fork (not in robot at required pin)

| path | origin |
|------|--------|
| e/eOn/eOn-2.16.0-foss-2024a.eb | generated earlier / eOn packaging fixtures |
| m/Meson/Meson-1.8.2-GCCcore-13.3.0.eb | fixture (robot has Meson-1.8.2 only on GCCcore-14.3.0) |
| r/Rust/Rust-1.88.0-GCCcore-13.3.0.eb | fixture (robot has 1.88 only on GCCcore-14.3.0) |
| q/quill/quill-11.1.0-GCCcore-13.3.0.eb | greenfield companion (no prior quill in robot) |
| m/metatensor/metatensor-0.1.17-GCCcore-13.3.0.eb | greenfield |
| m/metatensor-torch/metatensor-torch-0.10.0-foss-2024a.eb | greenfield |
| m/metatomic-torch/metatomic-torch-0.1.15-foss-2024a.eb | greenfield |

Patches already upstream and reused: `Meson-1.8.2_reenable-binutils-workaround.patch`, `Rust-1.70_sysroot-fix-interpreter.patch`.

### Human-judgment / residual gaps

1. **quill, metatensor, metatensor-torch, metatomic-torch** had **no prior EasyBuild recipe** in the robot — companions were authored (not `eb-stack bump` from an older generation). Content needs maintainer review for sources/checksums/start_dir fidelity before merge.
2. **Meson 1.8.2 / Rust 1.88.0 on GCCcore-13.3.0** are generation backports (bump-from-adjacent-toolchain style); not inventing new packages, but not a pure same-toolchain version bump either.
3. **Full `eb --robot` install** not proven here: terra session lacks Lmod (`lmod` command unavailable). Gating is `eb-stack check-recipe` missing=0 + EasyBuild `EasyConfigParser` parse of every contributed `.eb`.
4. **metatensor builddep Rust 1.83.0** (inside metatensor recipe) is already in the robot — fine for build; eOn itself needs Rust 1.88 for readcon.

## Validation evidence

- `check-recipe` robot+overlay: found=19 missing=0 (exit 0)
- EasyConfigParser: all contributed recipes OK
- `eb --dry-run-short`: failed environmentally (no Lmod), not recipe parse

## Robot path for consumers

```bash
export EASYBUILD_ROBOT_PATHS=/path/to/easybuild-easyconfigs/easybuild/easyconfigs:$EASYBUILD_ROBOT_PATHS
# or clone HaoZeke/easybuild-easyconfigs @ feat/eon-2.16.0-foss-2024a
eb eOn-2.16.0-foss-2024a.eb --robot
```

Prep tool:

```bash
eb-stack check-recipe \
  --recipe easybuild/easyconfigs/e/eOn/eOn-2.16.0-foss-2024a.eb \
  --easyconfigs /path/to/site-or-upstream-easyconfigs \
  --easyconfigs easybuild/easyconfigs
```
