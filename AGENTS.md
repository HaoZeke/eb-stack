# Agent driver contract for eb-stack

Any agent working in this repository follows these rules. They are the
distilled, machine-checked form of real campaign incidents; the tool is
built to make the right thing the easy thing.

## The procedure

Pick the skill that matches the work, then follow it end to end. The tool
does the mechanical majority and fails loudly; you handle only the bounded
residual cases it names. Your site should pair these with a site runbook
(init paths, module names, scheduler sizing); ask for it if you were not
given one.

| Work | Skill | Host |
|------|--------|------|
| Existing recipes → new toolchain generation (annual rebuild) | `skills/annual-bump/SKILL.md` | SURF EasyBuild work: **`rg.surf`** |
| **New package** from conda-forge / Spack (greenfield) | `skills/new-package/SKILL.md` | **`rg.surf`** (mandatory; see skill §0) |

Build/PR ops and three-claim ladder live in annual-bump §10. **SURF
EasyBuild** (authoring, residual agents in herdr, and `eb --robot` *builds*)
**runs on `rg.surf`**, not the laptop. **`rg.terra` is only the remote cargo
builder** for this repo’s Rust compile when required — do not route EasyBuild
installs there unless a site runbook explicitly says so.

## Non-negotiables

1. **Run the real CLI** (`eb-stack check-recipe | bump | solve | ingest |
   check-style | format-style`). Never guess dependency versions, checksums,
   or hierarchy relationships in prose — the tool resolves them or tells you
   exactly what is missing. If your harness speaks MCP, prefer the typed
   tool surface: `eb-stack mcp` serves `eb_check_recipe` / `eb_bump` /
   `eb_solve` / `eb_ingest` over stdio, with the reporting ladder and next
   actions embedded in every result. Ingest also writes
   `{stem}.residuals.json` — that file is the residual work queue (not a
   closed landable PR).
2. **Tool output is instructions.** A missing-dep hint ("available at
   other generations: ...") and hierarchy-member notes are your work queue.
   A `[packaging]` checksum finding means fix the recipe (checksums are
   positional: all sources first, then patches) — bypassing or deleting a
   check is never the fix. Residual-queue `kind: "style"` / pycodestyle
   E501 means run **`eb-stack format-style`** (mechanical); do not spend a
   residual agent turn hand-wrapping lines.
3. **Report on the three-claim ladder** (skill section 10.4): *resolves* /
   *builds* / *binary-verified* are different claims; state which rung you
   actually established and which you did not. Ingest alone is never
   *resolves* or *builds*.
4. **EasyBuild *builds* on `rg.surf`** (or site scheduler/cgroup when the
   runbook says so), with `EASYBUILD_TMPDIR` on durable storage. Residual
   agents run in **herdr** on that host. Not the laptop login session; not
   `rg.terra` (cargo only for this repo).
5. **The PR surface belongs to the human.** Branch pushes to a fork you
   were told to use are plumbing; opening, editing, or commenting on PRs
   and issues is not yours. Prepare paste-ready drafts instead. One PR per
   recipe set — check for an existing open PR before drafting another.

## Tests

`cargo test --lib` is the fast suite. CI (`.github/workflows/ci_test.yml`) also
runs the **known-bump** regression (`--test reproduce_real_prs --test bump_emit`:
frozen `foss-2023b` → `foss-2024a` maintainer pairs under
`tests/repro_fixtures/`, library and CLI), the packaging fixture suites
(`--test eon_foss_2026_1 --test qmcpack_foss_2026_1 --test eon_packaging`),
and foreign ingest (`--test foreign_ingest`). Robot-overlay check-recipe cases
skip when no easyconfigs tree is present; resolve and packaging_gate always
run. Build/test on a build machine when the repository owner's rules say the
local machine must not compile.
