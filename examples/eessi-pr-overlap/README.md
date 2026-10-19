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
  mounted stack. For an entry pinned with `from-commit`, the files that commit
  touched go ahead of the release tree, as in EasyBuild itself.
- The entries are closed over their dependencies, build dependencies and
  toolchains, using the same graph walk as `eb-stack stack order`. An
  installed module is a leaf: the walk stops there, as the robot does,
  because its own dependencies were installed with it. Entries in a
  `rebuilds/` easystack are built even when installed.
- "Installed" means a `software/<Name>/<VersionFull>` directory exists under
  one reference CPU target of the mounted stack (`x86_64/amd/zen4` by
  default).

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
| `prs/<N>.toml` | One record per open pull request: number, head sha, timestamp, stack version, installed source, easystack files, `to_build`, `already_installed`, `unresolved`, and the added entries. |
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
examples/eessi-pr-overlap/fetch-prs.sh EESSI/software-layer 2026.06 in/
eb-stack pr-overlap --pr in/prs/*.json \
  $(for d in in/commit-trees/*/; do printf -- '--commit-tree %s=%s ' "$(basename "$d")" "$d"; done) \
  --easyconfigs /path/to/easybuild-easyconfigs/easybuild/easyconfigs \
  --installed installed.txt --installed-label x86_64/amd/zen4 \
  --exclude-path /accel/ --exclude-path /additional_arch/ \
  --out-dir out/
```

`installed.txt` is `find $EESSI_SOFTWARE_PATH/software -mindepth 2 -maxdepth 2 -type d -printf '%P\n'`.

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
