---
name: eb-stack-eessi-extend
description: Test easyconfigs inside the EESSI build environment with EESSI-extend, and contribute them to the EESSI/software-layer easystacks. Separate from the EasyBuild easyconfigs contribution path. Use after an easyconfig builds on a normal target, when the goal is an EESSI software-layer PR, or when a recipe must be proven against the EESSI compatibility layer and CPU targets.
---

# EESSI: test with EESSI-extend, contribute via software-layer

This skill covers the **EESSI** side only. The upstream **EasyBuild** easyconfigs
PR path lives in `skills/upstream-pr/SKILL.md` and stays separate: an easyconfig
is contributed to EasyBuild, and *separately* an entry is contributed to the
EESSI software layer. Do not merge the two flows, and do not put EESSI wording
in an easyconfigs PR or easyconfigs wording in a software-layer PR.

Order of operations: an easyconfig must exist in a released EasyBuild or in an
open easyconfigs PR **before** the software-layer entry can deploy.

## Claim ladder addition

EESSI adds one rung above `binary-verified`:

- **eessi-verified** — the recipe installs on top of the EESSI compatibility
  layer through `EESSI-extend`, on at least the host CPU target.

An `eb --from-pr` SUCCESS on a normal builder does **not** establish it: EESSI
supplies a different sysroot and compatibility layer. Report it separately.

## Initialize EESSI and install with EESSI-extend

`EESSI-extend` is a module EESSI ships that loads and pre-configures EasyBuild
for installing on top of the stack. The EESSI build-deploy bots use the same
module, so this path matches what the bot will do.

```sh
source /cvmfs/software.eessi.io/versions/2025.06/init/lmod/bash

export EESSI_USER_INSTALL=$HOME/EESSI-2025.06
mkdir -p $EESSI_USER_INSTALL

module load EESSI-extend
eb --show-config                     # confirm the prefix EESSI-extend chose
eb <easyconfig>.eb --robot
```

Pick exactly one install-prefix variable before loading the module; the module
reads it at load time, so set it first:

| Variable | Meaning |
|---|---|
| `$EESSI_USER_INSTALL` | user-private prefix (umask 077, readable by you only) |
| `$EESSI_PROJECT_INSTALL` | project prefix (GID bit, group-writable, umask 002) |
| `$EESSI_SITE_INSTALL` | site-wide installations on top of EESSI |
| `$EESSI_CVMFS_INSTALL` | installation into the CernVM-FS repo itself |

Testing an easyconfig that is still an open EasyBuild PR uses the same module
plus the usual PR flags:

```sh
source /cvmfs/software.eessi.io/versions/2025.06/init/lmod/bash
export EESSI_USER_INSTALL=$HOME/EESSI-2025.06
module load EESSI-extend

eb --from-pr <N> --robot
# or, to pin the exact PR head:
eb --from-commit <sha> --robot
```

Always use the newest EasyBuild EESSI provides (`module avail EasyBuild`).

## Building without EasyBuild: buildenv

When something must be built by hand on top of EESSI, load the toolchain's
`buildenv` module rather than improvising. It sets `$CC` / `$CFLAGS` the way
EasyBuild would and injects the RPATH wrappers for `gcc`, `g++`, `gfortran`,
`ld`:

```sh
source /cvmfs/software.eessi.io/versions/2025.06/init/lmod/bash
module load buildenv/default-foss-2025b
module load <dependency modules>
make
readelf -d <binary>   # confirm RUNPATH
ldd <binary>          # confirm nothing resolves outside the stack
```

Hand builds must either use RPATH linking or set `$LD_LIBRARY_PATH`, and must
respect the compatibility-layer sysroot.

## Contributing to the software layer

1. Find or write a working easyconfig.
2. Test it in the EESSI build environment with `EESSI-extend` (above).
3. Contribute the easyconfig to EasyBuild if it is not there yet
   (`skills/upstream-pr/SKILL.md`).
4. Add an entry to the easystack file for the target EESSI version and
   toolchain generation, for example
   `easystacks/software.eessi.io/2025.06/eessi-2025.06-eb-5.3.0-2025a.yml`.
   When the easyconfig is still an open PR, use the `from-commit` option in the
   easystack entry so EasyBuild takes the file from that PR.
5. Open a pull request to `EESSI/software-layer`.
6. A member of the EESSI builders team triggers the bot to build on every
   supported CPU target (x86_64 generic/Haswell/Skylake/Cascadelake/Icelake/
   Sapphire Rapids/Zen2/Zen3/Zen4, aarch64 generic/Neoverse-N1/Neoverse-V1/
   A64FX/NVIDIA Grace).
7. On green, installations are deployed as signed tarballs and picked up into
   the CernVM-FS repo.

A software-layer PR may be opened while the easyconfigs PR is still open, but
**deployment only happens after the EasyBuild PR is merged**.

## Contribution policy gates

Per <https://eessi.io/docs/adding_software/contribution_policy>, before opening
a software-layer PR confirm the software:

- is redistributable (open source or equivalently redistributable);
- works on **all** supported CPU targets, not just the host;
- can be built by the EESSI bot without interactive steps or manual fetches;
- is a recent version on a recent toolchain generation.

A package that only builds on one microarchitecture is not ready for the
software layer, however green it is locally.

## Do not

1. Do not treat a normal `eb --from-pr` SUCCESS as EESSI evidence; the sysroot
   differs.
2. Do not set more than one `$EESSI_*_INSTALL` variable, and do not set them
   after loading `EESSI-extend`.
3. Do not write into `/cvmfs` directly; deployment is the bot's and the Stratum 0
   server's job.
4. Do not open or comment on `EESSI/software-layer` PRs from an agent; prepare
   paste-ready material and let the operator post (same rule as easyconfigs).
5. Do not mix easyconfig content changes into a software-layer PR; the easystack
   entry references the easyconfig, it does not restate it.
6. Do not claim all-CPU-target coverage from a single host build; that claim
   belongs to the bot run.

## Related

- `skills/upstream-pr/SKILL.md` — EasyBuild easyconfigs contribution (separate track)
- `skills/easybuild-dos-donts/SKILL.md` — recipe-shape rules that apply to both
- <https://eessi.io/docs/using_eessi/building_on_eessi>
- <https://eessi.io/docs/adding_software/overview>
- <https://www.eessi.io/docs/adding_software/opening_pr>
