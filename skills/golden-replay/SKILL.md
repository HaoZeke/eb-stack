---
name: eb-stack-golden-replay
description: Regenerate the SeisSol 1.3.2 foss-2025a overlay unattended. Use after the package-bump packset pin, or to check that a tool-repair still emits the overlay.
---

# Replay the SeisSol golden

Do not hand-edit a `.eb` file. Fix the emitter or the package TOML instead.
See `skills/tool-repair/SKILL.md`.

Pin the packset on the laptop. Then run the one-shot script.

```sh
PACKSET_URL=http://127.0.0.1:8761
PACKSET_WORKSPACE=git:github.com/HaoZeke/eb-stack
packset pin
skills/golden-replay/replay.sh
```

`packset pin` must say `set=package-bump`. The next command is `replay.sh`.
Do not locate the binary. Do not write a plan.

`replay.sh` calls `run-on-builder.sh` for each bump and plan. Then it
runs `check-overlay.sh`. Exit 0 is the gate.

```sh
skills/golden-replay/run-on-builder.sh
```

That wrapper with no argv runs `replay.sh`. Pass an out-dir only when
the default `work/seissol-1.3.2` must change.

## What the script emits

Site generation is `foss-2025a`. ASAGI stays at 1.0. easi becomes 1.7.0.
PSpaMM becomes 0.3.1 on `gfbf-2025a`. SeisSol becomes 1.3.2.

Required facts live in `check-overlay.sh`. A missing file, leftover
ImpalaJIT, stale commit, or default `import pspamm` is a tool defect.

`HOST_ARCH=hsw` is an upstream CMake default in `seissol.toml`. Do not
retarget it to the build host.

## Standing memory

The pinned set `package-bump` is the operator contract. Atoms:
`examples/ljos-packet/package-bump.jsonl`. Instructions card:
`examples/ljos-packet/package-bump.instructions.md`.

## Claims

This replay establishes `resolves` plus `artifact-checked`. It does not
establish `builds`.

## Related

See `skills/annual-bump/SKILL.md` for the bump command.
See `skills/verify-recipe/SKILL.md` for checksum class and source claims.
See `skills/tool-repair/SKILL.md` when emit disagrees with the golden.
See `skills/site-consume/SKILL.md` for site generation and eblocalinstall.
