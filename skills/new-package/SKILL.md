---
name: eb-stack-new-package
description: Author a new EasyBuild easyconfig (greenfield) using EasyBuild's own contribution workflow, with eb-stack ingest from conda-forge/Spack as an optional bootstrap. Follows docs.easybuild.io writing-easyconfig-files + contributing (mandatory params, templates, inject-checksums, check-contrib, review-pr, new-pr).
---

# New EasyBuild package (greenfield)

You are adding **software that is not yet in** (or not yet on this
generation of) [easybuild-easyconfigs](https://github.com/easybuilders/easybuild-easyconfigs)
`develop`. The ground truth is **EasyBuild documentation and `eb` itself**,
not this tool's inventiveness.

**Authoritative EasyBuild docs (read these; do not improvise around them):**

| Topic | URL |
|-------|-----|
| Writing easyconfigs (mandatory params, sources, deps, sanity, easyblock, moduleclass) | https://docs.easybuild.io/writing-easyconfig-files/ |
| Updating for a new toolchain (hierarchy, dep versions, inject-checksums) | same page, “Updating existing easyconfigs for a new toolchain” |
| Templates (`%(version)s`, `SOURCE_TAR_GZ`, `GITHUB_SOURCE`, …) | https://docs.easybuild.io/version-specific/easyconfig-templates/ |
| Generic easyblocks (`ConfigureMake`, `CMakeMake`, `CMakeNinja`, `MesonNinja`, …) | https://docs.easybuild.io/version-specific/generic-easyblocks/ |
| Contributing easyconfigs + PR requirements | https://docs.easybuild.io/contributing/ |
| `eb --new-pr` / `--update-pr` / GitHub integration | https://docs.easybuild.io/integration-with-github/ |
| Code / easyconfig style | https://docs.easybuild.io/code-style/ |
| Common toolchains (`foss`, hierarchy diagram) | https://docs.easybuild.io/common-toolchains/ |
| Typical install workflow (`-S`, `-Dr`, `--robot`) | https://docs.easybuild.io/typical-workflow-example/ |

**What `eb-stack` is for in this skill:** a *bootstrap* when you have a foreign
recipe (conda-forge / Spack). It is **not** a replacement for EasyBuild's
easyconfig model, style, or contribution gates. Landable recipes must satisfy
`eb --check-contrib` and (for upstream PRs) EasyBuild's review process.

**Sister skill:** retargeting *existing* easyconfigs to a new generation is
`skills/annual-bump/SKILL.md` (`eb-stack bump`), which mirrors the “update for
new toolchain” section of the writing-easyconfigs page.

---

## 0. What EasyBuild requires of an easyconfig

From [Writing easyconfig files](https://docs.easybuild.io/writing-easyconfig-files/):

### Mandatory parameters

- `name`, `version`
- `homepage`, `description` (module help metadata)
- `toolchain` as `{'name': '…', 'version': '…'}` (or `SYSTEM`)

### Filename scheme (robot resolution)

`<name>-<version>[-<toolchain>][<versionsuffix>].eb`  
Toolchain label omitted for system toolchain; empty versionsuffix omitted.
Filename matters for `--robot` dependency resolution.

### Common parameters you must get right

| Parameter | EasyBuild rule (summary) |
|-----------|--------------------------|
| `easyblock` | Explicit generic easyblock (`ConfigureMake`, `CMakeMake`, `CMakeNinja`, `MesonNinja`, …) or omit to use software-specific `EB_*` if it exists. Prefer a **generic** easyblock when the build is Autotools/CMake/Meson/copy. List with `eb --list-easyblocks`. |
| `source_urls` / `sources` | Filenames in `sources`; URLs in `source_urls`. Prefer **templates** (`%(version)s`, `SOURCE_TAR_GZ`, `GITHUB_SOURCE`) over hardcoded names. Prefer **downloadable release tarballs** over `git_config` clones (checksums must be stable). |
| `checksums` | Highly recommended for **all** sources and patches. SHA256 (64-char) preferred. Order: consume sources first, then patches. Use **`eb --inject-checksums`** rather than hand-typing. Multi-source often uses dict form. |
| `patches` | Unified diffs (`diff -ruN`), paths relative to unpacked sources. |
| `dependencies` / `builddependencies` | Tuples `(name, version[, versionsuffix[, toolchain]])`. Modules must exist (or be installable via `--robot`). Default resolve uses same toolchain, then **compatible subtoolchain** (EB ≥ 3.0). Use `SYSTEM` as 4th element for system-toolchain deps. |
| `configopts` / `buildopts` / … | Easyblock-specific; consult `eb -a -e <EasyBlock>`. |
| `sanity_check_paths` | Dict with `files` / `dirs` only (relative to install prefix). Default if omitted: non-empty `bin` and `lib`/`lib64`. Prefer explicit binaries. |
| `moduleclass` | **Required to be a known class** (not free text). Default `base` is wrong for real software — replace. List defaults: `eb --show-default-moduleclasses`. |

Parameter order / grouping in the file is part of EasyBuild **style** (see
contributing + code-style docs). `eb --inject-checksums` reorders
`source_urls` → `sources` → `patches` → `checksums` for you.

### Official “new software / new toolchain” procedure (EasyBuild)

From the writing-easyconfigs page (updating for a new toolchain — same
discipline for greenfield deps):

1. Start from a closely related easyconfig if one exists (`eb -S Name`).
2. Set `version` / `toolchain`; remove stale checksums, then
   `eb --inject-checksums`.
3. For each dependency, find an easyconfig for the **target toolchain or a
   subtoolchain** (foss → gompi/gfbf/GCC/GCCcore). Read the toolchain
   easyconfig (e.g. `foss-2023b.eb`) for member versions.
4. Recurse for missing deps into a working folder; install with
   `eb --robot <that-folder> …`.
5. Build, test, then contribute.

`eb-stack ingest` only automates *parts of steps 1–3* when a foreign recipe
exists. You still own inject-checksums, style, sanity, moduleclass, and
contribution gates.

---

## 1. When to use `eb-stack ingest` (optional bootstrap)

Use ingest when you have a **real** foreign recipe and want a first draft:

| Source | Path | Notes |
|--------|------|--------|
| conda-forge classic | `meta.yaml` | Jinja `{% set %}` / limited expand |
| rattler / v1 | `recipe.yaml` | `context:` + multi-source |
| Spack | `package.py` | Static parse only — no Python exec, `when=` recorded as residual |

Do **not** use ingest as the final authoring path when:

- An easyconfig already exists in easybuild-easyconfigs for a nearby version —
  **copy and tweak** that (EasyBuild's preferred approach) with `eb -S` and
  optional `eb-stack bump`.
- You need a software-specific easyblock (`EB_Name`) — implement or use the
  existing one per EasyBuild docs.

### Bootstrap command

```
ROBOT=/path/to/easybuild-easyconfigs/easybuild/easyconfigs

eb-stack ingest \
  --source path/to/recipe.yaml \    # or meta.yaml / package.py
  --format auto \                   # or conda-forge | spack
  --toolchain-name foss \
  --toolchain-version 2024a \       # must match a real toolchain in ROBOT / develop
  --easyconfigs "$ROBOT" \
  --keep-old-deps \
  --out-dir work/easyconfigs
```

What ingest **is allowed** to claim after a successful run:

- Parseable `.eb` with mandatory-ish fields filled from foreign identity
- Dependency *names* mapped toward EB; generation-native *versions* when the
  robot has hierarchy candidates (hierarchy consensus + resolvo joint pins)
- Static configure flags only when they appear as plain string literals in the
  foreign file

What ingest **must not** claim:

- A landable easyconfig (style, `moduleclass`, real checksums, sanity)
- Correctness of product `configopts` / Spack `when=` / conda selectors
- That `eb --robot` will succeed

Treat every `# WARNING:` line as a work-queue item mapped to an EasyBuild
parameter below.

---

## 2. End-to-end workflow (EasyBuild-native)

### Step A — Search first

```
eb -S SoftwareName
```

If a close easyconfig exists, **copy it** and update version/toolchain (writing
docs “update for new toolchain”). Prefer that over foreign ingest.

### Step B — Bootstrap or hand-draft

- **Foreign available:** `eb-stack ingest … --easyconfigs $ROBOT`
- **Else:** draft by hand from a sibling easyconfig or a generic-easyblock
  template. Use `eb -a -e CMakeNinja` (etc.) for parameters.

Place drafts under a letter layout directory EasyBuild expects, e.g.
`work/easyconfigs/q/QMCPACK/QMCPACK-4.3.0-foss-2024a.eb`.

### Step C — Align to EasyBuild easyconfig rules (mandatory cleanup)

Do these **before** calling the recipe “done”:

1. **Templates:** rewrite hardcoded tarball names to `%(version)s` /
   `SOURCE_TAR_GZ` / `GITHUB_SOURCE` where applicable
   ([templates](https://docs.easybuild.io/version-specific/easyconfig-templates/)).
2. **Checksums:** delete placeholders; run
   ```
   eb --inject-checksums path/to/Name-Ver-tc.eb
   ```
   Prefer release assets over git-archive URLs (EB docs: git-created tarballs
   are not checksum-stable across hosts).
3. **Dependencies:** only packages that exist as easyconfigs/modules; use
   subtoolchain versions from `foss-*.eb` / hierarchy. Drop fake conda
   packages (`pip`, `setuptools` as modules) and language virtuals.
4. **`moduleclass`:** set a **known** class (`tools`, `chem`, `lib`, `math`,
   `devel`, …) via `eb --show-default-moduleclasses`. Never leave wrong
   defaults that mis-classify.
5. **`sanity_check_paths`:** real install relative paths (binaries), not empty
   placeholder dirs that always “pass”.
6. **Style / order:** run EasyBuild's contributor check:
   ```
   eb --check-contrib path/to/Name-Ver-tc.eb
   ```
   This checks style + SHA256 presence (required for easyconfig PRs).

### Step D — Resolve graph and dry-run install plan

```
eb Name-Ver-foss-YYYY.eb -Dr --robot work/easyconfigs:$ROBOT
```

Then install (build machine / scheduler — see annual-bump §10):

```
eb Name-Ver-foss-YYYY.eb --robot work/easyconfigs:$ROBOT
```

### Step E — eb-stack plan check (optional, complements EB)

```
eb-stack check-recipe \
  --recipe work/easyconfigs/.../Name-Ver-tc.eb \
  --easyconfigs "$ROBOT" \
  --easyconfigs work/easyconfigs
```

Use for missing-dep generation hints and checksums **positional** lint.
This is **not** a substitute for `eb --check-contrib` or a real install.

### Step F — Contribute (upstream easyconfigs)

Per [Contributing](https://docs.easybuild.io/contributing/):

- Target **`develop`** of easybuild-easyconfigs.
- Prefer **`eb --new-pr`** / **`eb --update-pr`** for opening/updating PRs.
- PR title pattern for new easyconfigs:
  `{<moduleclass>}[<toolchain>] <software name> <software version> <extra>`
  e.g. `{chem}[foss/2024a] QMCPACK 4.3.0`
- PR requirements (do not skip):
  - green CI on the easyconfigs repo
  - style consistency (`--check-contrib`)
  - consistency vs related easyconfigs: `eb --review-pr <PR#>`
  - **successful test reports** for the easyconfig PR
  - maintainer approval; author does not merge own PR

Site/fork PRs: still human-owned surface; prepare paste-ready text if your
org forbids agents on GitHub PR APIs.

---

## 3. Residual map (foreign bootstrap → EasyBuild parameters)

| Symptom from ingest / check | EasyBuild action |
|-----------------------------|------------------|
| Zero / missing checksum | `eb --inject-checksums`; fix `sources` to a stable URL first |
| Hardcoded version in `sources` | Use `%(version)s` / `SOURCE_*` templates |
| `moduleclass = 'lib'` default wrong | Set known class; `eb --show-default-moduleclasses` |
| Empty / fake `sanity_check_paths` | Real files under install prefix |
| Residual foreign dep version | Pin from robot / subtoolchain easyconfig for target gen |
| Missing dep + generation hint | Author or bump that dep easyconfig (recurse) |
| Product flags missing | From project build docs / sibling EB recipe — not from inventing `-D` |
| Spack `when=` / conda selectors | Residual; encode variants as separate easyconfigs or toolchainopts |
| Multi-source without extract layout | Hand-write `sources` dicts / `extract_cmd` per EB multi-source docs |

---

## 4. Worked fixtures (in-repo foreign inputs only)

These **bootstrap** only; landable PRs live under `fixtures/eon_foss_2026_1`
and `fixtures/qmcpack_foss_2026_1` and were hand-finished to EasyBuild rules.

```
REPO=<eb-stack>
ROBOT=$HOME/.venvs/easybuild/easybuild/easyconfigs   # or easybuild-easyconfigs checkout

# Bootstrap from conda-forge eOn
eb-stack ingest \
  --source $REPO/fixtures/foreign_ingest/conda_eon/recipe.yaml \
  --toolchain-name foss --toolchain-version 2024a \
  --easyconfigs "$ROBOT" --keep-old-deps \
  --out-dir /tmp/eb-new/easyconfigs

# Bootstrap from Spack QMCPACK
eb-stack ingest \
  --source $REPO/fixtures/foreign_ingest/spack_qmcpack/package.py \
  --format spack \
  --toolchain-name foss --toolchain-version 2024a \
  --easyconfigs "$ROBOT" --keep-old-deps \
  --out-dir /tmp/eb-new/easyconfigs

# Then ALWAYS:
eb --inject-checksums /tmp/eb-new/easyconfigs/*/*/*.eb
eb --check-contrib /tmp/eb-new/easyconfigs/*/*/*.eb
eb -Dr --robot /tmp/eb-new/easyconfigs:$ROBOT /tmp/eb-new/easyconfigs/*/*/*.eb
```

Compare bootstrap output to landable fixtures to see residual product surface
(configopts, extract_cmd, real checksums, toolchainopts).

Regression for the bootstrap path:

```
cargo test --locked --test foreign_ingest
```

---

## 5. Guarantees vs non-guarantees

| Claim | Who |
|-------|-----|
| Foreign identity → parseable draft | `eb-stack ingest` |
| Generation-native dep versions when robot has candidates | `eb-stack` hierarchy + resolvo |
| Stable checksums, templates, style, product flags | **Driver agent** running *this skill* + `eb` |
| Modules exist / install works | **`eb --robot` on a build host** |
| Upstream-ready | **EasyBuild PR process** (`--new-pr`, test reports, maintainers) |

Three-claim ladder (site ops): annual-bump §10.4 —
*resolves* (plan) ≠ *builds* (`eb`) ≠ *binary-verified*.

---

## 6. Closing residuals: Willma (OMP / Hermes) — not more eb-stack code

**Do not patch `eb-stack` to invent product flags, checksums, or landable
style.** Ingest deliberately leaves residuals. Closing them is **agent work
against EasyBuild docs and `eb`**, driven by this skill.

### Who runs what

| Layer | Role |
|-------|------|
| `eb-stack ingest` / `check-recipe` | Mechanical bootstrap + plan lint |
| **This skill** | Full runbook (EasyBuild-native finish steps) |
| **SURF Willma** (`surf-ai-hub/openai/gpt-oss-120b`) via **OMP** or **Hermes** | Execute the residual loop: templates, inject-checksums, moduleclass, sanity, `check-contrib`, `-Dr`/`--robot`, paste-ready PR notes |
| Human | PR surface, maintainer review, site policy |

### Driver contract (paste as system / task context)

1. Open and follow **`skills/new-package/SKILL.md` end to end** — do not
   re-summarize EasyBuild into a private procedure.
2. Prefer **`eb -S` → copy sibling** over foreign invent when a close
   easyconfig exists.
3. If bootstrapping: run **`eb-stack ingest`** with robot, then treat every
   WARNING as a work-queue item in §3.
4. Finish with **EasyBuild tools only** for landability:
   `eb --inject-checksums`, `eb --check-contrib`, `eb -Dr --robot …`,
   optional `eb --new-pr` when authorized.
5. Report on the **three-claim ladder** (annual-bump §10.4). Never say
   “works” when only *resolves*.
6. **Do not** open/edit GitHub PRs unless the human explicitly authorized
   it in this turn. Prepare paste-ready PR title/body instead.
7. **Do not** change stock robot recipes to silence host errors (annual-bump
   §10.6).

### OMP (SURF)

Same path as annual-bump §11.5:

- Role / model: **`eb-stack` → `surf-ai-hub/openai/gpt-oss-120b`** (Willma).
- Give the agent: path to this skill, foreign recipe path (or “search first”),
  target toolchain generation, `ROBOT` path, output work dir.
- Prompt shape (minimal):  
  `Follow skills/new-package/SKILL.md for <software>. Foreign: <path or none>.
   Toolchain foss-<gen>. ROBOT=<path>. Work dir=<path>. Close residuals with
   eb, not by inventing eb-stack features. Stop at paste-ready recipe + claim
   ladder unless told to open a PR.`

### Hermes

- Start a Willma/gpt-oss session with the same skill path and task prompt as
  above (Hermes role equivalent to OMP `eb-stack` if configured).
- Point `cwd` at the easyconfigs work tree; keep `eb` and `eb-stack` on PATH.
- Do not embed EasyBuild parameter encyclopedias in the prompt — the skill
  and docs.easybuild.io are the encyclopedias.

### What success looks like for the driver

- `eb --check-contrib` clean (or listed, justified residuals only).
- `eb -Dr --robot` shows a coherent graph with no invented dep versions.
- Claim ladder stated; build only if site runbook + scheduler allow.
- Paste-ready PR title matching `{moduleclass}[toolchain] Name version`.

---

## 7. Quick reference

| Task | Command |
|------|---------|
| Search existing EB recipes | `eb -S Name` |
| Bootstrap from foreign | `eb-stack ingest --source … --easyconfigs ROBOT --out-dir DIR` |
| Inject SHA256 | `eb --inject-checksums foo.eb` |
| Contributor gate | `eb --check-contrib foo.eb` |
| Dry-run graph | `eb foo.eb -Dr --robot DIR:ROBOT` |
| Install | `eb foo.eb --robot DIR:ROBOT` |
| Plan check (eb-stack) | `eb-stack check-recipe --recipe foo.eb --easyconfigs ROBOT` |
| Open easyconfig PR (EB GitHub integration) | `eb --new-pr foo.eb` (see integration-with-github docs) |
| Review consistency vs develop | `eb --review-pr <PR#>` |

---

## 8. Routing

| Situation | Skill / doc |
|-----------|-------------|
| New software / first easyconfig | **This skill** + [writing easyconfigs](https://docs.easybuild.io/writing-easyconfig-files/) |
| Existing `.eb` → new foss generation | `skills/annual-bump` + writing-docs “new toolchain” |
| Residual finish / landability | **Willma via OMP or Hermes** (§6), not more `eb-stack` code |
| Stack lock / build list | annual-bump `solve` |
| Contribute to easybuilders/* | [contributing](https://docs.easybuild.io/contributing/) + [GitHub integration](https://docs.easybuild.io/integration-with-github/) |

**Do not invent EasyBuild parameters, style, or contribution requirements.**
If this skill and docs.easybuild.io disagree, **docs.easybuild.io wins**.
**Do not “fix” residual product gaps by coding them into ingest** — run Willma
on this skill.