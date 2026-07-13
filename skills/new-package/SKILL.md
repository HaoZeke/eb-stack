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
| **local-ai agent** (Hermes preferred; OMP allowed) | Residual judgment against live `eb` / robot |
| **herdr** pane for residual agents | Always; never ad-hoc `ssh … hermes/omp -p` for residual loops |
| Drafts / letter-layout work dir | Where `eb --robot` and inject-checksums see files |

```
ssh rg.surf
# mechanical CLI (ingest / inject-checksums / check-contrib) may run in that shell
# residual judgment agents: always under herdr on this host (see §7)
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

# Then ALWAYS:
eb --inject-checksums /tmp/eb-new/easyconfigs/*/*/*.eb
eb --check-contrib /tmp/eb-new/easyconfigs/*/*/*.eb
eb-stack check-recipe --recipe /tmp/eb-new/easyconfigs/e/eOn/*.eb \
  --easyconfigs "$ROBOT" --easyconfigs /tmp/eb-new/easyconfigs
eb -Dr --robot /tmp/eb-new/easyconfigs:$ROBOT /tmp/eb-new/easyconfigs/*/*/*.eb
```

Compare bootstrap + residual queue to landable fixtures
(`fixtures/eon_foss_2026_1`, `fixtures/qmcpack_foss_2026_1`) for product surface
(configopts, extract_cmd, companions). Fixtures prove **resolve**/packaging_gate,
not a virgin *builds* install.

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
| Graph dry-run / install (*builds*) | `eb -Dr` / `eb --robot` on **rg.surf** (**mechanical**) |
| Product configopts, variant policy, real sanity paths, moduleclass choice, multi-source extract layout, companion authoring judgment | **local-ai agent** using residual queue — **not** hardcoding into `eb-stack` |
| Upstream PR merge | Human + EasyBuild maintainers |

Three-claim ladder (site ops): annual-bump §10.4 —
*resolves* (plan) ≠ *builds* (`eb --robot` on **`rg.surf`**) ≠ *binary-verified*.
Do not ship a residual session that only edits recipes and stops if the next
step is install: after residual gates, run `eb -Dr --robot …` then
`eb --robot …` on **rg.surf** (site EB setup). Use the batch scheduler when the
site runbook says so; on a single-user rg.surf workstation a direct `eb`
session is fine if it owns a cgroup and `EASYBUILD_TMPDIR` is set.

---

## 7. Mechanical first; local-ai agent only for judgment residuals

**Maximize mechanical work.** Run every tool step that does not require a
product/policy decision. **Do not hardcode** product flags, hand pins, or
site-specific layout into `eb-stack` / ingest to “close” residuals — that
bakes lies into the tool. Judgment residuals go to a **local-ai agent**
(OMP or Hermes session with this skill as the runbook).

### Split of work

| Kind | Examples | Who |
|------|----------|-----|
| **Mechanical** | `eb -S`; `eb-stack ingest` / `eb_ingest` (+ residual JSON); hierarchy/resolvo pins; `eb --inject-checksums`; **`eb-stack format-style`** / `check-style` (E501 ≤120); `eb --check-contrib`; `eb-stack check-recipe` / `eb_check_recipe`; `eb -Dr --robot` | CLI / MCP / any driver that only runs commands |
| **Judgment residual** | Close residual-queue items: product features; moduleclass; sanity paths; extract_cmd; companion recipes for hierarchy holes; Spack `when=`; sibling vs greenfield; PR title | **local-ai agent** (this skill §3–§4) with residual JSON as input |
| **Forbidden** | Encoding product `-D` sets, fake checksums into ingest; hand-wrapping E501 in a residual agent when `format-style` exists; claiming *builds* without `eb --robot` | Nobody — not code, not the agent pretending mechanical |

### Mechanical sequence (always do this before calling a local-ai agent)

1. `eb -S Name` — prefer sibling copy when possible.
2. `eb-stack ingest … --easyconfigs $ROBOT` (or MCP `eb_ingest`) — keep the
   printed **residual-queue** path.
3. Read residual JSON kinds; fix only **mechanical** items if any (none of the
   product `-D` set).
4. `eb --inject-checksums` on the draft (after sources URLs are stable).
5. **`eb-stack format-style path/to/Name-Ver-tc.eb`** then
   **`eb-stack check-style`** — E501 (line longer than 120) is **mechanical**. Do **not**
   send long `configopts` / `preconfigopts` line-wrapping to a residual agent.
   Residual-queue items with `kind: "style"` mean run format-style, not judgment.
6. `eb --check-contrib` — remaining non-E501 style / missing SHA256 only.
7. `eb-stack check-recipe` (+ draft overlay for companions) + `eb -Dr --robot …`.
8. **Stop.** Remaining residual-queue (non-`style`) + missing-dep hierarchy
   hints → local-ai agent in **herdr** (§7). After judgment edits, re-run
   4–7, then `eb --robot` on **rg.surf** only when claiming *builds*.

### local-ai agent (Hermes preferred) — **herdr pane on `rg.surf`**

- **Name in prompts:** “local-ai agent” (not product nicknames).
- **Host:** **`rg.surf` only** (same as §0). Where `eb` and the robot tree live.
- **Container (mandatory):** run the residual agent **inside a herdr pane** on
  that host — same pattern as other SURF agent work (herdr → harness → tools).
  Do **not** drive residual judgment via bare `ssh rg.surf 'hermes …'` /
  `omp … -p` outside herdr (no pane status, no `herdr agent read`, easy to
  leave orphan processes).
- **How to start (on rg.surf):**
  ```
  # ensure server
  herdr status   # if server not running: herdr server &
  WORK=$HOME/tmp/eb-repro/work   # or site work dir
  herdr agent start eb-residual-hermes \
    --cwd "$WORK" --no-focus -- \
    hermes chat --cli --yolo --accept-hooks \
      --provider willma -m openai/gpt-oss-120b \
      -q "$(cat "$WORK/residuals/hermes-residual-prompt.md")"
  herdr agent read eb-residual-hermes --source recent --lines 80
  herdr agent wait eb-residual-hermes --status idle
  ```
  Hermes is the default residual harness; OMP (`omp-surf` / `omp-eb-stack`)
  is allowed only if started the same way through **herdr agent start**.
- **Harness model path:** site Willma role for eb-stack (see annual-bump §11.5).
  Commercial frontier models out of scope for SURF-only work.
- **Input:** this skill path; software name; foreign path (or “none —
  search first”); toolchain; `ROBOT` and work dir **on rg.surf**; the
  **`*.residuals.json` path** from ingest; any `check-recipe` missing-dep JSON.
- **Prompt shape (minimal):**
  ```
  On rg.surf (herdr pane): follow skills/new-package/SKILL.md for <software>.
  Foreign: <path|none>. Toolchain: foss-<gen>. ROBOT=<path on rg.surf>.
  Work=<path on rg.surf>. Residual queue JSON: <path to *.residuals.json>.
  Mechanical steps already done: <ingest, inject-checksums, check-contrib, …>.
  Residual judgment only: close residual-queue items + check-recipe missing
  hierarchy companions. Use eb / eb-stack / MCP eb_check_recipe on this host.
  Do not invent eb-stack features or hardcode product flags into the tool.
  Report claim ladder. PR surface human-only unless authorized this turn.
  ```
- **Output:** edited easyconfig(s) + companions under letter layout + residual
  log + claim ladder; paste-ready PR title/body if contribution is in scope.

### What success looks like

- Mechanical gates green: inject-checksums done, `check-contrib` clean or
  only judgment items left and documented.
- `eb -Dr --robot` coherent; no invented dep versions.
- Judgment residuals closed by local-ai agent with citations to project
  docs / sibling easyconfigs, not silent invention.
- Claim ladder stated.

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
| Install (*builds*, **rg.surf**) | `eb foo.eb --robot work:ROBOT` |
| Open easyconfig PR (EB GitHub integration) | `eb --new-pr foo.eb` (human; see integration-with-github docs) |
| Review consistency vs develop | `eb --review-pr <PR#>` |

---

## 9. Routing

| Situation | Skill / doc |
|-----------|-------------|
| New software / first easyconfig | **This skill** + [writing easyconfigs](https://docs.easybuild.io/writing-easyconfig-files/) |
| Existing `.eb` → new foss generation | `skills/annual-bump` + writing-docs “new toolchain” |
| Residual judgment after mechanical gates | **local-ai agent** (§7), not hardcoding into `eb-stack` |
| Stack lock / build list | annual-bump `solve` |
| Contribute to easybuilders/* | [contributing](https://docs.easybuild.io/contributing/) + [GitHub integration](https://docs.easybuild.io/integration-with-github/) |

**Do not invent EasyBuild parameters, style, or contribution requirements.**
If this skill and docs.easybuild.io disagree, **docs.easybuild.io wins**.
**Mechanize everything that is a tool step; never hardcode product residuals
into ingest — dispatch a local-ai agent for judgment only.**