# Shared builds between open software-layer pull requests

A draft CI for `EESSI/software-layer` (the European Environment for Scientific Software Installations). It is kept here until the EESSI
maintainers want it, and nothing in this directory is wired into any EESSI
repository.

## The question

Every software-layer pull request adds lines to an easystack. What it actually
builds is everything `eb --easystack <file> --robot` would install for those
lines on the current stack: the named easyconfigs plus each dependency not yet
installed. When two open pull requests both need a module nobody has built,
both build it. Reviewers work out the order by hand today, from `--dry-run`
output pasted into opening posts.

The workflow answers it on a schedule. For each open pull request it writes a
record of what the pull request would build. It then lists the modules more
than one of them would build, for example "#101 shares `libpng/1.6.50-GCCcore-15.2.0`
with #102", so reviewers can order the builds.

## Where the build set comes from

`eb-stack pr-overlap` computes the build set itself, deterministically, from
the recipes. It does not run `eb`.

- The entries a pull request adds are the head easystack entries missing from
  `main`'s copy of the same file.
- eb-stack looks each entry's easyconfig up on the robot path: the
  easyconfigs that come with the stack's EasyBuild version, read from the
  mounted stack. For an entry pinned with `from-commit`, the easyconfigs tree
  at that commit goes ahead of the release tree, as in EasyBuild itself.
- The entries are closed over their dependencies, build dependencies and
  toolchains, using the same graph walk as `eb-stack stack order`. An
  installed module is a leaf: the walk stops there, as the robot does,
  because its own dependencies were installed with it. Entries in a
  `rebuilds/` easystack are built even when installed.
- "Installed" means a `software/<Name>/<VersionFull>` directory exists under
  one reference CPU target of the mounted stack (`x86_64/amd/zen4` by
  default). A `module_version` alias in `.modulerc.lua` counts as that
  module when the full version it names is one of those directories.

Running the real `eb --dry-run --robot` with the stack mounted
(`eessi/github-action-eessi`, as `test-software.eessi.io.yml` already does)
would be ground truth. Here it checks the closure, for three reasons:

1. It runs code from the pull request's point of view: an easystack is
   EasyBuild input, and `from-commit` fetches recipes from wherever the entry
   points. The closure only reads text.
2. It needs EasyBuild, Lmod and the stack set up per run and per EasyBuild
   version, which costs minutes per pull request every hour. The closure
   takes seconds for all of them.
3. A failed dry run gives no set at all. The closure records what it could not
   resolve (`unresolved`) and still reports the rest.

The prototype was cross-checked against real dry runs on a sample of open
pull requests. Where they disagree, the dry run wins, and the difference is a
bug to fix in the closure.

## Output: the data branch

The branch `gh_action_pr_dependency_overlap` follows the naming of
`gh_action_update_software_layer_commit_sha`. It contains:

| Path | What |
|---|---|
| `prs/<N>.toml` | One record per open pull request: number, head sha, timestamp, stack version, installed source, easystack files, `to_build` (one sorted line per module), `already_installed`, `unresolved`, and the added entries. |
| `overlap.toml` | The index: each module more than one pull request would build, mapped to those pull requests, plus the shared modules per pair. |
| `overlap.md` | The same as a report for reviewers. |
| `README.md` | Says the branch is data and is never merged. |

Every generated file starts with a never-merge header. Closed pull requests
drop out, because each run writes the full open set and deletes stale
records.

Concurrent pull requests can't conflict on this branch. One scheduled job is
the only writer, and the `concurrency` group lets one run at a time. Each
pull request has its own file, so a change in one shows up as a one-file
diff. A run commits only when a build set changed; a timestamp change alone
is not committed.

## The overlap is a diff of the build sets

Two open pull requests can add different easystack entries and still need the
same module. The entries name different easyconfigs, so a diff of the
easystack files is clean. A dependency that sits two steps down the closure
is not a line in either easystack, so that diff never mentions it.

`prs/<N>.toml` is that pull request's SBOM: the closure of the entries it
adds, minus every module the deployed stack already has. `to_build` is one
sorted line per module. The same line in two of those files is a module both
pull requests would build. Merging every open SBOM into one tree, the way a
staging merge stacks branches, and listing the duplicate `to_build` lines
(`sort | uniq -d`) is the same answer `overlap.md` prints.

Two conditions make a duplicate line mean "both would build this":

1. The SBOM lists only what is not installed on the deployed stack. The
   toolchain is installed for every pull request; if the file listed the
   whole closure, every file would share it and the duplicates would be noise.
2. Each SBOM is regenerated against the live installed stack on every cron
   run. A dependency that has been deployed is on the installed list at the
   next run, so it leaves `to_build` and the overlap goes with it. A closed
   or merged pull request is not in that run, and its `prs/<N>.toml` is
   deleted.

While the line is still in both files, both pull requests would compile that
module and install it. The build that finishes second overwrites the binary
the first one installed.

## Safety

- The workflow runs from `main` on a schedule and on `workflow_dispatch`,
  never on `pull_request` or `pull_request_target`. Fork pull requests need
  no token of their own.
- Pull request heads are fetched from `refs/pull/N/head` as git objects. Their
  easystack files are read with `git show`, and nothing is checked out,
  sourced or executed.
- The compute job has `contents: read` and `pull-requests: read`. Only the
  publish job has `contents: write`. It pushes, without force, to the data
  branch only, and commits only files the compute job wrote.
- It opens no pull requests and posts no comments. The commit message names no
  `#N`, so pull request timelines stay quiet.
- The eb-stack release is pinned by version and sha256, and every action is
  pinned by commit, except `eessi/github-action-eessi@v3`, which matches the
  existing workflows.

## What a maintainer has to set up

1. Exempt `gh_action_pr_dependency_overlap` from the branch push restriction,
   the same way `gh_action_update_software_layer_commit_sha` is exempt, so
   `github-actions[bot]` can push to it with the workflow's `GITHUB_TOKEN`.
   The exemption covers that one branch, and `main` stays protected.
2. Copy `pr_dependency_overlap.yml` to `.github/workflows/` and
   `fetch-prs.sh` to `.github/workflows/scripts/fetch_pr_easystacks.sh`.
3. Set `EB_STACK_VERSION` and `EB_STACK_SHA256` to a release that carries
   `pr-overlap`.

## Running it by hand

```
source /cvmfs/software.eessi.io/versions/2026.06/init/lmod/bash
module load EESSI-extend
examples/eessi-pr-overlap/fetch-prs.sh EESSI/software-layer 2026.06 in/
eb-stack pr-overlap --pr in/prs/*.json \
  $(for d in in/commit-trees/*/; do printf -- '--commit-tree %s=%s ' "$(basename "$d")" "$d"; done) \
  $(for d in in/easyblocks/*/; do printf -- '--easyblocks-commit %s=%s ' "$(basename "$d")" "$d"; done) \
  --easyconfigs /path/to/easybuild-easyconfigs/easybuild/easyconfigs \
  --easyblocks /path/to/easybuild/easyblocks \
  --installed installed.txt --installed-modulerc /path/to/modules/all \
  --filter-deps "$EASYBUILD_FILTER_DEPS" \
  --installed-label x86_64/amd/zen4 \
  --exclude-path /accel/ --exclude-path /additional_arch/ \
  --out-dir out/
```

`installed.txt` is `find $EESSI_SOFTWARE_PATH/software -mindepth 2 -maxdepth 2 -type d -printf '%P\n'`.
`--installed-modulerc` is that prefix's `modules/all`. `--filter-deps` is
`EASYBUILD_FILTER_DEPS` after `module load EESSI-extend` on the same CPU
target. `--easyblocks` is the EasyBuild release's `easybuild/easyblocks`
package. `--easyblocks-commit` is the files one `include-easyblocks-from-commit`
touches, which `fetch-prs.sh` writes under `in/easyblocks/<commit>/`.

`overlap.md` in this directory is the zen4 report computed `2026-10-19T14:07:20Z`:
39 pull requests, 18 records, 9 shared modules.

## Known limits

- One reference CPU target. A module installed on zen4 and missing on another
  target reads as installed. A matrix over targets would run this once per
  target.
- One robot tree: the newest EasyBuild version named by the stack's
  easystacks. Each entry records its own easystack's EasyBuild version.
- Accelerator and additional-arch easystacks are excluded, because the
  installed list covers CPU modules only.
- Dependency choice follows eb-stack's nearest-toolchain rule. The dry-run
  check is there to catch any place where that differs from EasyBuild's own
  robot.
- `--filter-deps` follows EasyBuild: a filtered dependency is dropped with
  the modules reachable only through it. A bare name filters every version.
  `name=version` and `name=[low:high]` filter a version, with `[` and `]`
  inclusive and the opposite bracket exclusive.
- An alias in `.modulerc.lua` counts as installed only when the full module
  it points at is installed.
- A missing easyblock class is an unresolved requirement, and the module is
  not counted. A class defined by an `include-easyblocks-from-commit` tree
  that was passed in is counted and noted. The commit tree is the files that
  commit touches, not the whole easyblocks repository.
