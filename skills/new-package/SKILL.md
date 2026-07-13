---
name: eb-stack-new-package
description: Author a new EasyBuild easyconfig (greenfield) with eb-stack ingest. Default: Hermes/herdr campaign agent on rg.surf full-drives residual judgment through eb --robot *builds*. Mechanical format-style/check-recipe; human owns PR surface.
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

## 0. Host: `rg.surf` (mandatory for this skill)

All EasyBuild-facing work for **new packages** runs on **`rg.surf`**
(SSH host alias; SURF workstation), not on the laptop and not on
`rg.terra` (terra is the generic remote builder for cargo/heavy compile —
different role).

| On `rg.surf` | Why |
|--------------|-----|
| `eb`, EasyBuild robot tree, modules | Real SURF EasyBuild environment (this host **is** the EB machine) |
| `eb-stack` (release binary or build there) | Same machine as `eb` for ingest → check-recipe → install |
| **Install / *builds* claim** | `eb --robot` **here** when EB is set up — not `rg.terra` (terra is cargo for this repo) |
| **local-ai agent** (Hermes preferred; OMP allowed) | **Full campaign owner** on this host: residual judgment **and** `eb --robot` *builds* (see §7) |
| **herdr** pane for the campaign agent | Always; never ad-hoc `ssh … hermes/omp -p` for residual/build loops |
| Drafts / letter-layout work dir | Where `eb --robot` and inject-checksums see files |

```
ssh rg.surf
# optional preflight mechanical CLI may run in that shell
# campaign agent (residual + install): always under herdr on this host (see §7)
```

If `rg.surf` is unreachable, **stop and report** — do not fall back to
laptop-local `eb` fiction or invent a second EasyBuild install. Site
runbook may document modules init on that host; load it before `eb`.

---

## 1. What EasyBuild requires of an easyconfig

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

## 2. When to use `eb-stack ingest` (optional bootstrap)

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
# Also writes work/easyconfigs/<letter>/<Name>/<Name>-….residuals.json
# Optional: --residual-queue PATH
```

MCP (same semantics): `eb_ingest` via `eb-stack mcp` (`source`, optional
`easyconfigs[]`, `out` / `out_dir`, `residual_queue`).

What ingest **is allowed** to claim after a successful run:

- Parseable `.eb` with mandatory-ish fields filled from foreign identity
- Dependency *names* mapped toward EB; generation-native *versions* when the
  robot has hierarchy candidates (hierarchy consensus + resolvo joint pins)
- Static configure flags only when they appear as plain string literals in the
  foreign file
- A **residual queue JSON** (`{stem}.residuals.json`) with classified items
  (`dep_version`, `product_config`, `moduleclass`, `sanity`, `checksum`, …)
  and a claim ladder that marks *resolves*/*builds* as **not** established

What ingest **must not** claim:

- A landable easyconfig (style, `moduleclass`, real checksums, sanity)
- Correctness of product `configopts` / Spack `when=` / conda selectors
- That `eb --robot` will succeed
- That the residual queue is closed (it is the work list for §7)

Treat every `# WARNING:` line **and** every residual-queue `items[]` entry as a
work-queue item mapped to an EasyBuild parameter in §4.

---

## 3. End-to-end workflow (EasyBuild-native)

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

### Step E — eb-stack plan check (complements EB)

```
eb-stack check-recipe \
  --recipe work/easyconfigs/.../Name-Ver-tc.eb \
  --easyconfigs "$ROBOT" \
  --easyconfigs work/easyconfigs
```

Use for missing-dep generation hints and checksums **positional** lint.
Unpinned deps must match a **hierarchy member** of the recipe toolchain
(e.g. CapnProto on GCCcore-14.x does **not** satisfy foss-2026.1 which needs
GCCcore-15.2.0). Explicit fourth-tuple pins (cross-gen residuals like
`xtb`/gfbf-2024a) still match exactly. Missing-dep reasons include hierarchy
member labels + “available at other generations” hints — that list is the
companion-author work queue.

This is **not** a substitute for `eb --check-contrib` or a real install.
MCP: `eb_check_recipe` with the same trees.

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

## 4. Residual map (foreign bootstrap → EasyBuild parameters)

Primary input: **`{stem}.residuals.json`** from ingest (kinds below). Also map
`# WARNING:` lines and `check-recipe` missing-dep reasons.

| Residual kind / symptom | EasyBuild action |
|-------------------------|------------------|
| `checksum` / zero checksum | `eb --inject-checksums`; fix `sources` to a stable URL first |
| Hardcoded version in `sources` | Use `%(version)s` / `SOURCE_*` templates |
| `moduleclass` default `lib` wrong | Set known class; `eb --show-default-moduleclasses` |
| `sanity` / empty paths | Real files under install prefix |
| `dep_version` / residual foreign pin | Pin from robot / subtoolchain easyconfig for target gen |
| check-recipe missing + hierarchy note | Author or bump companion for **this** generation (e.g. CapnProto-GCCcore-15.2.0), put under work overlay first in `--robot` |
| `product_config` / flags missing | From project build docs / sibling EB recipe — **not** inventing `-D` in `eb-stack` |
| `style` / E501 line too long | **`eb-stack format-style`** then `check-style` / `eb --check-contrib` — **not** residual judgment |
| `variant` / Spack `when=` / selectors | Encode variants as separate easyconfigs or toolchainopts |
| Multi-source without extract layout | Hand-write `sources` dicts / `extract_cmd` per EB multi-source docs |

---

## 5. Worked fixtures (in-repo foreign inputs only)

These **bootstrap** only; landable PRs live under `fixtures/eon_foss_2026_1`
and `fixtures/qmcpack_foss_2026_1` and were hand-finished to EasyBuild rules.

```
# On rg.surf:
REPO=<eb-stack-checkout-on-rg.surf>
ROBOT=$HOME/.venvs/easybuild/easybuild/easyconfigs   # or easybuild-easyconfigs checkout on rg.surf

# Bootstrap from conda-forge eOn (foss-2026.1 landable gen)
eb-stack ingest \
  --source $REPO/fixtures/foreign_ingest/conda_eon/recipe.yaml \
  --toolchain-name foss --toolchain-version 2026.1 \
  --easyconfigs "$ROBOT" --keep-old-deps \
  --out-dir /tmp/eb-new/easyconfigs
# → …/eOn-….eb and …/eOn-….residuals.json

# Bootstrap from Spack QMCPACK
eb-stack ingest \
  --source $REPO/fixtures/foreign_ingest/spack_qmcpack/package.py \
  --format spack \
  --toolchain-name foss --toolchain-version 2026.1 \
  --easyconfigs "$ROBOT" --keep-old-deps \
  --out-dir /tmp/eb-new/easyconfigs

# Overlay landable companions when robot holes exist (e.g. CapnProto GCCcore-15.2.0):
# rsync $REPO/fixtures/eon_foss_2026_1/easyconfigs/ /tmp/eb-new/easyconfigs/

# Then ALWAYS (mechanical preflight; campaign agent §7 re-runs and continues to install):
eb --inject-checksums /tmp/eb-new/easyconfigs/*/*/*.eb
eb-stack format-style /tmp/eb-new/easyconfigs/*/*/*.eb
eb --check-contrib /tmp/eb-new/easyconfigs/*/*/*.eb
eb-stack check-recipe --recipe /tmp/eb-new/easyconfigs/e/eOn/*.eb \
  --easyconfigs "$ROBOT" --easyconfigs /tmp/eb-new/easyconfigs
eb -Dr --robot /tmp/eb-new/easyconfigs:$ROBOT /tmp/eb-new/easyconfigs/*/*/*.eb
# *builds* (required for DONE_FULL_DRIVE / *builds* claim):
# eb --robot /tmp/eb-new/easyconfigs:$ROBOT /tmp/eb-new/easyconfigs/*/*/*.eb
```

Compare bootstrap + residual queue to landable fixtures
(`fixtures/eon_foss_2026_1`, `fixtures/qmcpack_foss_2026_1`) for product surface
(configopts, extract_cmd, companions). Fixtures prove **resolve**/packaging_gate;
a *builds* claim still needs `eb --robot` on **rg.surf** (campaign agent §7).


Regression (code + validation):

```
cargo test --locked --lib unpinned_dep_requires_hierarchy
cargo test --locked --lib residual_queue_classifies
cargo test --locked --test foreign_ingest
cargo test --locked --test eon_foss_2026_1 --test qmcpack_foss_2026_1
```

---

## 6. Guarantees vs non-guarantees

| Claim | Who |
|-------|-----|
| Foreign identity → parseable draft + residual JSON | `eb-stack ingest` / MCP `eb_ingest` (**mechanical**) |
| Generation-native dep versions when robot has candidates | `eb-stack` hierarchy + resolvo (**mechanical**) |
| Hierarchy-aware missing deps (no older-GCCcore false pass) | `eb-stack check-recipe` (**mechanical**) |
| SHA256 for fetched sources | `eb --inject-checksums` (**mechanical**) |
| Style + checksum presence gate | `eb --check-contrib` (**mechanical**) |
| E501 line length (≤120) lint / wrap | `eb-stack check-style` / `format-style` (**mechanical**) |
| Graph dry-run | `eb -Dr --robot` on **rg.surf** (**mechanical**; campaign agent re-runs) |
| Real install (*builds*) | `eb --robot` on **rg.surf** (**mechanical command**, run by the **campaign agent** §7 — not optional if goal is PR-ready / *builds*) |
| Product configopts, variant policy, real sanity paths, moduleclass choice, multi-source extract layout, companion authoring judgment | **campaign agent** using residual queue — **not** hardcoding into `eb-stack` |
| Upstream PR merge | Human + EasyBuild maintainers |

Three-claim ladder (site ops): annual-bump §10.4 —
*resolves* (plan) ≠ *builds* (`eb --robot` on **`rg.surf`**) ≠ *binary-verified*.

**Default campaign goal when the human asks for PR-ready / landable / “do the
packages”:** establish *resolves* **and** *builds* for every target recipe.
Stopping after recipe polish + `eb -Dr` without `eb --robot` is **not done**.
Use the batch scheduler when the site runbook says so; on a single-user
rg.surf workstation a direct `eb --robot` session is fine if it owns a cgroup
and `EASYBUILD_TMPDIR` is set.

---

## 7. Full-drive campaign agent (default)

The **local-ai agent (Hermes preferred)** is the **process owner** of a greenfield
campaign on **rg.surf**: it runs mechanical CLI steps, closes judgment residuals,
**and** drives `eb --robot` until *builds* is established or a real block is
documented. It is **not** a “residual-only chat that stops before install.”

**Maximize mechanical tools** (do not invent product `-D` into `eb-stack`). Use
`format-style` for E501; use residual judgment for product/moduleclass/companions.
**Do not** end the herdr session after check-contrib / check-recipe / `eb -Dr`
when the goal includes *builds*.

### Split of work

| Kind | Examples | Who |
|------|----------|-----|
| **Mechanical commands** | `eb -S`; `eb-stack ingest` / `eb_ingest`; inject-checksums; **format-style** / check-style; check-contrib; check-recipe; `eb -Dr`; **`eb --robot`** | Run by shell **or** by the campaign agent; same commands either way |
| **Judgment** | product features; moduleclass; sanity paths; extract_cmd; hierarchy companions; Spack `when=`; sibling vs greenfield | **campaign agent** with residual JSON + oracles/docs |
| **Forbidden** | Hardcoding product `-D` into ingest; hand-wrapping E501 when format-style exists; claiming *builds* without `eb --robot` success; opening GitHub PRs without human authorization | Nobody |

### Preflight (optional outer driver)

An outer driver **may** run steps 1–7 below before starting herdr to save agent
turns. If skipped, the campaign agent runs them itself. **Never** treat preflight
as the end of the campaign when *builds* is in scope.

1. `eb -S Name` — prefer sibling copy when possible.
2. `eb-stack ingest … --easyconfigs $ROBOT` — keep residual-queue path.
3. Mechanical residual items only (not product `-D`).
4. `eb --inject-checksums` after sources are stable.
5. `eb-stack format-style` then `check-style` (E501 is mechanical).
6. `eb --check-contrib`.
7. `eb-stack check-recipe` + `eb -Dr --robot …`.
8. **Start campaign agent (§7 full-drive)** — judgment + re-gates + **`eb --robot`**.

### Full-drive sequence (campaign agent MUST run)

Inside herdr on **rg.surf**, for each target recipe (and companions as needed):

1. Close residual-queue judgment (oracles / project docs / sibling EB). Prefer
   `cp` from a landable fixture when residual is “match landable fixture.”
2. Re-run: inject-checksums (if needed) → **format-style** → check-style →
   check-contrib → check-recipe → `eb -Dr --robot WORK:ROBOT` until green.
3. **REAL BUILD (required for *builds*):**
   ```
   eb --modules-tool=Lmod --robot "$WORK/easyconfigs:$ROBOT" path/to/Name-Ver-tc.eb \
     2>&1 | tee "$WORK/logs/robot-<name>.log"
   ```
   Prefer smaller packages first when batching. On failure: read the log, fix
   recipe/companion with justified edits, re-run from step 2 for that package.
   **Do not** disable sanity checks or invent looser settings.
4. Write `$WORK/residuals/session-log.md` with the three-claim ladder **per package**:
   *resolves* / *builds* / *binary-verified* (last only if real binary checks ran).
5. Print **`DONE_FULL_DRIVE`** when every target has *resolves* and *builds*, else
   **`DONE_PARTIAL`** with exact failures and log paths.

### How to start (on rg.surf)

```
herdr status   # if server not running: herdr server &
WORK=$HOME/tmp/eb-repro/work   # or site work dir
# Write $WORK/residuals/hermes-full-drive.md from the prompt template below
# (or skills/new-package/hermes-full-drive.prompt.md).

herdr agent start eb-full-drive-hermes \
  --cwd "$WORK" --no-focus -- \
  hermes chat --cli --yolo --accept-hooks \
    --provider willma -m openai/gpt-oss-120b \
    -q "$(cat "$WORK/residuals/hermes-full-drive.md")"
herdr agent read eb-full-drive-hermes --source recent --lines 80
# Monitor: herdr agent read / wait; outer driver uses monitor tool (no sleep-poll).
```

Hermes is the default harness; OMP only via the same **herdr agent start** path.
Harness model: site Willma role for eb-stack (annual-bump §11.5). Commercial
frontier models out of scope for SURF-only work.

### Prompt template (full-drive — default)

Copy and fill paths. Ship as `$WORK/residuals/hermes-full-drive.md`.

```
On rg.surf (herdr pane): follow skills/new-package/SKILL.md end to end.
You OWN the full campaign through REAL installs (*builds*). Do not stop at recipe polish.

WORK=<path> REPO=<eb-stack> ROBOT=<easyconfigs tree>
export PATH=$HOME/.venvs/easybuild/bin:$HOME/.local/bin:$REPO/target/release:$PATH
export LMOD_CMD=$HOME/.local/lmod/lmod/libexec/lmod
source $HOME/.local/lmod/lmod/init/bash 2>/dev/null || true
export EASYBUILD_MODULES_TOOL=Lmod EASYBUILD_SOURCEPATH=… EASYBUILD_TMPDIR=… EASYBUILD_INSTALLPATH=…

Recipes: <list .eb paths>
Residual queues: <*.residuals.json>
Oracles (if any): <fixture .eb paths — copy product judgment, do not invent -D>
Companions: under WORK/easyconfigs as needed.

FORBIDDEN: edit REPO/src; open GitHub/GitLab PRs; invent product flags not in
oracle/docs; claim builds without eb --robot success.

Sequence: (1) residual/oracle align (2) inject-checksums (3) format-style +
check-style (4) check-contrib (5) check-recipe (6) eb -Dr (7) eb --robot with
tee to WORK/logs/robot-*.log for EACH package (8) session-log.md claim ladder
(9) DONE_FULL_DRIVE or DONE_PARTIAL.

Start now. Run commands; do not only plan.
```

Optional **residual-only** prompt is allowed only when the human explicitly
scopes “no install / recipe gates only.” Default is full-drive.

### What success looks like

- Gates green: format-style, check-contrib, check-recipe, `eb -Dr`.
- **`eb --robot` exit 0** for each target (or documented real failure + log).
- session-log.md states *resolves* / *builds* / *binary-verified* honestly.
- `DONE_FULL_DRIVE` or `DONE_PARTIAL` printed.
- PR surface: paste-ready only unless human authorized remote PR this turn.

---

## 8. Quick reference

| Task | Command |
|------|---------|
| Search existing EB recipes | `eb -S Name` |
| Bootstrap from foreign | `eb-stack ingest --source … --easyconfigs ROBOT --out-dir DIR` |
| Residual work queue | `{stem}.residuals.json` (or `--residual-queue PATH`) |
| MCP bootstrap | `eb-stack mcp` → `eb_ingest` |
| Inject SHA256 | `eb --inject-checksums foo.eb` |
| Contributor gate | `eb --check-contrib foo.eb` |
| E501 lint (≤120) | `eb-stack check-style foo.eb` |
| E501 auto-wrap (mechanical) | `eb-stack format-style foo.eb` |
| Plan check (hierarchy-aware) | `eb-stack check-recipe --recipe foo.eb --easyconfigs ROBOT --easyconfigs work` |
| MCP plan check | `eb_check_recipe` |
| Dry-run graph | `eb foo.eb -Dr --robot work:ROBOT` |
| Install (*builds*, **rg.surf**) | `eb foo.eb --robot work:ROBOT` (campaign agent §7) |
| Full-drive agent (default) | herdr + Hermes with `hermes-full-drive.md` (§7) |
| Open easyconfig PR (EB GitHub integration) | `eb --new-pr foo.eb` (human; see integration-with-github docs) |
| Review consistency vs develop | `eb --review-pr <PR#>` |

---

## 9. Routing

| Situation | Skill / doc |
|-----------|-------------|
| New software / first easyconfig | **This skill** + [writing easyconfigs](https://docs.easybuild.io/writing-easyconfig-files/) |
| Existing `.eb` → new foss generation | `skills/annual-bump` + writing-docs “new toolchain” |
| PR-ready / landable / “do the packages” | **Campaign agent full-drive** (§7) through `eb --robot` |
| Residual-only (explicit human scope) | Campaign agent residual-only prompt — no *builds* claim |
| Stack lock / build list | annual-bump `solve` |
| Contribute to easybuilders/* | [contributing](https://docs.easybuild.io/contributing/) + [GitHub integration](https://docs.easybuild.io/integration-with-github/) |

**Do not invent EasyBuild parameters, style, or contribution requirements.**
If this skill and docs.easybuild.io disagree, **docs.easybuild.io wins**.
**Mechanize every tool step (including format-style and `eb --robot`); never
hardcode product residuals into ingest. The campaign agent owns the loop
through *builds* unless the human explicitly scoped residual-only.**
