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

## Pre-flight: is your generation even in EESSI?

Run this **before** anything else. EESSI's compatibility layer lags the newest
EasyBuild toolchain generation, and a gap ends the attempt.

```sh
ls /cvmfs/software.eessi.io/versions/          # list only; do not descend yet
```

**Never descend into a version speculatively.** A listed version can be an
unpopulated placeholder with no `init/` at all, and one stray `ls` into it wedges
`/cvmfs` for every user on the host (see below) — the lookup neither resolves nor
errors. List, then confirm a version is real before using it:

```sh
ls /cvmfs/software.eessi.io/versions/<ver>/init/lmod/bash   # must exist
```

Do that check from a container client if the host mount is at all suspect.

Then check the target generation against that version's supported top-level
toolchains. `EESSI-extend` refuses an unsupported one and prints the opt-in:

```sh
export EESSI_SITE_TOP_LEVEL_TOOLCHAINS_2025_06='[{"name": "GCCcore", "version": "15.2.0"}]'
```

Setting that variable makes EasyBuild *plan* the build, but if the generation is
not in the compatibility layer every dependency is `[SKIPPED]` and the run starts
by bootstrapping the compiler itself (`gcc-<ver>.tar.gz`, ~180 MB). **Do not do
that**: it duplicates what the EESSI bot does centrally and contradicts the
don't-bootstrap-from-scratch rule. A generation gap means the honest ceiling is
`builds` / `binary-verified` on a normal target, and the EESSI entry waits for a
version whose layer carries the generation.

Also check whether EESSI's bundled EasyBuild has every easyconfig in the closure
(`eb --version` inside EESSI, then resolve). Anything newer than that release
must come from a PR via `from-commit`.

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

`installpath` resolves to a microarchitecture-qualified path under your prefix
(`.../versions/<ver>/software/linux/x86_64/amd/zen5`), and `robot-paths` points at
EESSI's own bundled EasyBuild easyconfigs. The module also suggests setting
`$EASYBUILD_SOURCEPATH` to reuse sources and `$WORKING_DIR` (commonly `/dev/shm`)
to move the build directory.

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

## When the host CVMFS client is unusable

A host CVMFS mount can wedge: `cvmfs_config probe` reports OK, `cvmfs_talk`
answers, and the Stratum-1 returns HTTP 200 to `curl` in milliseconds, yet every
`ls` into a nested path blocks in uninterruptible `D` state and poisons the shell
and any tmux session started from it. `timeout` cannot kill a `D`-state reader.

Free the wedge first — this works as an ordinary user via polkit, and does not
need the root password:

```sh
systemctl kill --signal=SIGKILL cvmfs-<repo>.mount   # clears D-state readers
systemctl restart cvmfs-<repo>.mount
```

Recover from a terminal that did **not** run the hanging command — a root shell
that did is itself stuck behind its own FUSE operation. For one-shot root
commands where `sudo` wants a password, try `systemd-run --pipe --quiet <cmd>`
before driving a `sudo su` tmux pane; escaped quotes in `send-keys` silently
leave that shell at a continuation prompt swallowing every later keystroke.

Diagnosing further is often not worth it. On one host, root-catalog fetches
succeeded while nested traversal stalled, and none of these changed anything:
wiping `/var/lib/cvmfs/shared`, adding a second Stratum-1, disabling
`CVMFS_LOW_SPEED_LIMIT` / `CVMFS_TIMEOUT*` / `CVMFS_MAX_RETRIES` /
`CVMFS_IPFAMILY_PREFER`, or adding `cvmfs-config.cern.ch` to
`CVMFS_REPOSITORIES`. Meanwhile the container mounted CVMFS itself on the same
host and network and traversed everything instantly. Revert your experiments and
use the container.

Do not fight it if you lack root. Apptainer mounts CVMFS itself, needing only
unprivileged user namespaces, and is immune to the host client's state:

```sh
git clone --depth 1 https://github.com/EESSI/software-layer-scripts.git
export APPTAINER_CACHEDIR=$PWD/storage/apptainer-cache
./software-layer-scripts/eessi_container.sh \
  --mode exec --access ro --storage $PWD/storage \
  --extra-bind-paths "$PWD/ecs:/ecs:ro" \
  -- /bin/bash /ecs/run-extend.sh
```

Four things that bite:

- `eessi_container.sh` lives in **`EESSI/software-layer-scripts`**, not in
  `EESSI/software-layer` (it moved; the latter no longer has it).
- `--mode run` warns and behaves as `exec`; pass `exec` directly.
- The script you run must sit **inside** a bound path. `/ecs/../run-extend.sh`
  resolves outside the bind mount and fails with "No such file or directory".
- Set `APPTAINER_CACHEDIR`, not `SINGULARITY_CACHEDIR`; with both set the latter
  wins and the former is silently ignored.

Use `--access rw` only when the run must write into the repository overlay.

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
7. Do not bootstrap a whole toolchain generation into a user prefix to force a
   too-new recipe onto an older EESSI; report the generation gap instead.

## Related

- `skills/upstream-pr/SKILL.md` — EasyBuild easyconfigs contribution (separate track)
- `skills/easybuild-dos-donts/SKILL.md` — recipe-shape rules that apply to both
- <https://eessi.io/docs/using_eessi/building_on_eessi>
- <https://eessi.io/docs/adding_software/overview>
- <https://www.eessi.io/docs/adding_software/opening_pr>
- Internal run log: `Software/eb-stack/eessi-extend-verification-2026-07-23.org`
