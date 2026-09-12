---
name: eb-stack-golden-replay
description: Regenerate the SeisSol 1.3.2 foss-2025a overlay using only eb-stack commands plus EasyBuild inject-checksums. Use when replaying the golden, teaching a small model the bump path, or checking that a tool-repair still emits the overlay.
---

# Replay the SeisSol golden from eb-stack only

Do not hand-edit a `.eb` file. If the emitted file is wrong, fix the emitter
or the package TOML (`skills/tool-repair/SKILL.md`).

Recall the pinned packset first. It is the operator contract for any bump,
not this overlay:

```sh
PACKSET_URL=http://127.0.0.1:8761
PACKSET_WORKSPACE=git:github.com/HaoZeke/eb-stack
packset pin            # must say set=package-bump
ljos search bump
```

Then `skills/annual-bump/SKILL.md` for which `eb-stack` binary. Robot trees
live on the configured builder. Discover them there. Do not `test -d` those
paths on this machine.

## Inputs (this overlay)

Site generation is `foss-2025a`. Confirm on the builder before changing it.

On the configured builder:

- Robot: a tree that contains `SeisSol-1.1.4-foss-2023a.eb`
  (`rg --files "$ROBOT" | rg 'SeisSol-1\\.1\\.4'`).
- Spack tree if PSpaMM has no `.eb`:
  `rg --files ~ | rg 'py_pspamm/package.py'`.
- Package layers in this repo: `examples/packages/{asagi,easi,pspamm,seissol}.toml`.
- Unconstrained stack policy for a new-package plan:
  `examples/stacks/gfbf-2025a.toml`.

Discover each source on the builder:

```sh
rg --files "$ROBOT" | rg 'ASAGI-1\\.0-|easi-|PSpaMM-|SeisSol-1\\.1\\.4'
```

Use the newest recipe on the previous site generation. Do not invent a
basename. ASAGI is a same-version toolchain bump. easi → `1.7.0`. SeisSol →
`1.3.2` with `--source-checksum a0dd4f33d7d72c4a5eb6f01f0a96bc410af20c95a496e5cf1532ee8a20561612`
(the recursive `git_config` archive, already injected). PSpaMM → `0.3.1` on
`gfbf-2025a`. If no PSpaMM `.eb` exists, `package plan` the Spack
`py_pspamm` recipe with `--package-config examples/packages/pspamm.toml`
and `--source-checksum 7d677e1a81897f42daeb9d89cdc596f7ab97b5b829dc4afd4e86804bbaf2af6c`.

Pass `--package-config` on every bump or plan. Bump companions first. Reuse
**one** `--out-dir` so `easyconfigs/<letter>/<name>/` accumulates. Pass that
overlay as a later `--easyconfigs`.

A version bump without `--source-checksum` clears the old digest to `''`.
That is required. Fill it on the EasyBuild host:

```sh
eb --inject-checksums <emitted.eb>
```

Do not copy a Spack or conda-forge hash onto these recipes. SeisSol uses
`git_config` recursive; that archive is not the GitHub tag tarball. After
inject, run:

```sh
eb-stack recipe check --verify-sources \
  --recipe <emitted.eb> \
  --easyconfigs "$ROBOT" --easyconfigs work/seissol-1.3.2/easyconfigs
```

## Expected overlay

Compare against `work/seissol-1.3.2/easyconfigs/` in this checkout (the
golden). Required assignments:

| File | Must contain |
|---|---|
| `a/ASAGI/ASAGI-1.0-foss-2025a.eb` | foss 2025a; `ASAGI-1.0_fix-level.h.patch` beside it |
| `e/easi/easi-1.7.0-foss-2025a.eb` | no ImpalaJIT; Lua 5.4.x; yaml-cpp |
| `p/PSpaMM/PSpaMM-0.3.1-gfbf-2025a.eb` | `options = {'modulename': 'pypspamm'}` or the equivalent dict |
| `s/SeisSol/SeisSol-1.3.2-foss-2025a.eb` | `local_commit_id` `20334d02af9c539a9cf7cc4cd03c3e2622104aea`; checksum `a0dd4f33…`; Eigen 3.4.x; PSpaMM; numactl; HOST_ARCH=hsw |

Whitespace, comment text, and dependency tuple order may differ. A missing
assignment, a leftover ImpalaJIT tuple, a stale commit/hash, or a default
`import pspamm` is a tool defect: go to `skills/tool-repair/SKILL.md`.

```sh
skills/golden-replay/check-overlay.sh work/seissol-1.3.2/easyconfigs
```

`HOST_ARCH=hsw` is an upstream CMake default encoded in `seissol.toml` because
the sanity paths name `SeisSol_Release_dhsw_6_elastic`. Do not retarget it to
the build host's zen/skylake string.

## Standing memory packet

The pinned set `package-bump` is the operator contract. Atoms:
`examples/ljos-packet/package-bump.jsonl`. Instructions card:
`examples/ljos-packet/package-bump.instructions.md`. Product names,
commits, and hashes stay in the package TOML and this skill.

## Claims

This replay establishes `resolves` plus `artifact-checked` after inject and
`recipe check --verify-sources`. It does not establish `builds`. The Snellius
SUCCESS on PR 26851 is separate `builds` evidence.

## Related

- `skills/annual-bump/SKILL.md` — the bump command
- `skills/verify-recipe/SKILL.md` — checksum class and source claims
- `skills/tool-repair/SKILL.md` — when emit disagrees with the golden
- `skills/site-consume/SKILL.md` — site generation and eblocalinstall
