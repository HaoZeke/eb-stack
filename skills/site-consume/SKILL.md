---
name: eb-stack-site-consume
description: Get an easyconfig built at a site without forking it into the site's own repository, by keeping the recipe upstream and consuming it with --from-pr or --from-commit, checking the target generation is actually populated before choosing it, and driving the site's CI through whatever trigger protocol it parses. Use when a site needs a package that is still an open upstream PR, when deciding between copying a recipe locally and referencing it, and when a site build list or test pipeline has to be made to run a specific recipe.
---

# Consume an upstream recipe at a site

The default is upstream. A recipe copied into a site repository is a fork with
no upstream review, no upstream fixes, and a divergence nobody tracks. Copy
only when the recipe is genuinely site-specific (site license handling, site
paths, a site-only toolchain), and say why in the commit.

## Reference, do not copy

EasyBuild can build a recipe that exists only in an open PR:

```sh
eb QMCPACK-4.3.0-foss-2025a.eb --from-pr=26437 --robot
eb SomePkg-1.2.3-foss-2025b.eb --from-commit=<sha> --robot
```

`--from-pr` takes the PR's files as they stand now and needs nothing but the
number. `--from-commit` pins one revision, which is what you want when the PR
is still moving under you and the site needs a reproducible input.

**`--from-commit` resolves against the upstream repository, not your fork.** A
commit that exists only on a fork branch with no open PR cannot be fetched, and
the failure is a download error that reads like a network problem. Once a PR is
open its head commit is reachable in the base repository, so either open the PR
first or use `--from-pr`.

Check what the referenced commit actually carries. Upstream copies of a
dependency are written against the generation they were added for: an upstream
`HDF5` at the newest generation may pin a `CMake`, `Perl` and `Python` your site
has never built, so referencing it pulls a second dependency generation into
the build. Diff the upstream file against what your site already has before
choosing it over a local one.

## Pick a generation the site has actually built out

A recipe resolving is not the same as a site being able to build it cheaply.
Before choosing the target generation, count what is missing:

```sh
# what the site builds, per generation
rg -n '^[A-Za-z0-9_.+-]+-.*(2025a|2025b)\.eb' <site-repo>/buildlists/<list>

# what the recipe needs
eb-stack recipe check --recipe <recipe.eb> --easyconfigs <upstream-tree>
```

A generation that carries only compilers, MPI and the toolchain meta-packages
will build every dependency from scratch under `--robot`, and a language
runtime is usually the long pole. Against a CI walltime that is often the
difference between a result and a timeout. Prefer the generation where the
dependencies already exist, and say which ones decided it.

## Drive the site CI on its own terms

Site pipelines parse a trigger, and the parser is the contract. Read it in the
site's own CI definition rather than assuming, and check your trigger against
it before pushing. The pattern that matters, whatever the syntax:

- **Find what is parsed.** Usually a commit subject, a build-list file, or a
  branch name. If it is the commit message, find out whether the body is read
  at all: often only the subject is, which makes the body free for explanation
  and makes everything the pipeline needs fit on one line.
- **Find which commit is read.** If the pipeline reads the tip of a branch, a
  merge commit whose subject is "Merge branch ..." carries none of your tags and
  the run aborts. Fast-forward, set the merge subject, or push an empty commit
  carrying the trigger.
- **Find what the parser can express.** A regex that splits on commas will
  accept several recipes in one trigger; one that matches a single bracket will
  not. Test it before spending a run:

```python
import re
subject = "<the exact subject you are about to push>"
m = re.search(r"<the pipeline's own regex>", subject)
print([e for e in m.group(1).split(',')])          # the build list it will run
```

- **Know which flags the pipeline consumes and which it forwards.** A flag the
  pipeline preprocesses (architecture scoping, force lists) is meaningless on a
  path that does not parse it and is handed to `eb` as an unknown option. Flags
  `eb` itself takes (`--from-pr`, `--from-commit`, `--robot`) forward fine.
- **Scope the architectures.** A GPU build queued on CPU-only nodes compiles,
  installs into the wrong module tree, and holds a partition slot for as long as
  the walltime allows. If the recipe skips its sanity check, nothing catches it.

An empty commit is a legitimate trigger when the recipe lives upstream and the
site repository has nothing to change:

```sh
git commit --allow-empty -m "<trigger subject the pipeline parses>"
```

Put the reasoning in the body: which generation, why that one, what will be
built as a side effect, and what the run is expected to prove.

## After the run

Report which rung the run reached, and separate the recipe from the site. A
build that failed because the site lacks a dependency says nothing about the
recipe; a build that failed on a patch or a configure option is upstream's
problem and belongs in the PR. Send the recipe fix upstream rather than
patching the site copy, which is the whole point of not having a site copy.

## Related

- `skills/verify-recipe/SKILL.md` — prove the recipe before spending the run
- `skills/upstream-pr/SKILL.md` — getting the recipe upstream in the first place
- `skills/eessi-extend/SKILL.md` — the EESSI equivalent of this path
