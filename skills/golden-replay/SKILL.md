---
name: eb-stack-golden-replay
description: Regenerate the SeisSol 1.3.2 foss-2025a overlay from eb-stack package bump and plan. Use after the package-bump packset pin.
---

# Replay the SeisSol golden

Do not hand-edit a `.eb` file. Fix the emitter or the package TOML instead.
See `skills/tool-repair/SKILL.md`.

First two commands, before any `package bump`:

```sh
PACKSET_URL=http://127.0.0.1:8761
PACKSET_WORKSPACE=git:github.com/HaoZeke/eb-stack
packset pin
ljos search bump
```

Name the first `package bump` the parent.
Copy its flags exactly.
Exit 1 with `residual=` means continue.
Bump NAME into the same `--out-dir`.
If no `.eb` exists, run `package plan`.
Re-run the parent with the same flags.
Do not add `--allow-unresolved` on the parent.
Stop only when that parent exits 0.
A child exit 0 is not done.

Then compose `package bump` and `package plan`.
Do not run a one-shot overlay script.

`packset pin` must say `set=package-bump`.
`eb-stack` is already on PATH. It ssh's to the builder.
Do not run `cargo`. Do not run `rustc`. Do not open `run-on-builder.sh`.
Do not run `target/debug/eb-stack`. Do not `find`.
`jail-hermes.sh` is how this seat starts Willma. Do not run it as a bump.

`--out-dir` is the bundle root. Reuse one directory for all four products.
Pass that overlay as a later `--easyconfigs`.

`--contributor NAME` stamps `# updated by:` or `# contributed by:` with
the username and the eb-stack version. Leave `Authors::` in place.

## Inputs

Site generation is `foss-2025a`.
Robot on `rg.terra`:
`/home/rgoswami/Git/tmp/eon-pr26480-check/easybuild/easyconfigs`.
Spack `py_pspamm` (jail copy):
`/home/rgoswami/var/scratch/2026-09/eb-stack-willma-unattended/spack/py_pspamm/package.py`.
Package layers: `examples/packages/{asagi,easi,pspamm,seissol}.toml`.
Stack policy for a new-package plan: `examples/stacks/gfbf-2025a.toml`.

| Product | Command | Source | Toolchain | Version | Extra |
|---|---|---|---|---|---|
| ASAGI | `package bump` | `a/ASAGI/ASAGI-1.0-foss-2023a.eb` | foss 2025a | keep 1.0 | `asagi.toml` |
| easi | `package bump` | `e/easi/easi-1.3.0-foss-2023a.eb` | foss 2025a | 1.7.0 | `easi.toml` |
| PSpaMM | `package plan` | Spack `py_pspamm` | gfbf 2025a | 0.3.1 | `pspamm.toml`; checksum `7d677e1a81897f42daeb9d89cdc596f7ab97b5b829dc4afd4e86804bbaf2af6c` |
| SeisSol | `package bump` | `s/SeisSol/SeisSol-1.1.4-foss-2023a.eb` | foss 2025a | 1.3.2 | `seissol.toml`; checksum `a0dd4f33d7d72c4a5eb6f01f0a96bc410af20c95a496e5cf1532ee8a20561612` |

The command is `eb-stack package bump` or `eb-stack package plan`.
Do not run `eb-stack stack solve` for a recipe update.

`--allow-unresolved` is a real flag. Use it only for `exclude_from_solve`.
Pass `--version` when the application version changes.
Pass `--package-config` on every command.

After pin, `source paths.env` in the overlay directory. Those paths exist
on the builder. Do not `find` them. Then run these four.

```sh
eb-stack package bump --source $ROBOT/a/ASAGI/ASAGI-1.0-foss-2023a.eb \
  --toolchain-name foss --toolchain-version 2025a \
  --package-config $CFG/asagi.toml --easyconfigs $ROBOT --out-dir $OUT
eb-stack package bump --source $ROBOT/e/easi/easi-1.3.0-foss-2023a.eb \
  --toolchain-name foss --toolchain-version 2025a --version 1.7.0 \
  --package-config $CFG/easi.toml --easyconfigs $ROBOT \
  --easyconfigs $OUT/easyconfigs --out-dir $OUT
eb-stack package plan --source $SPACK \
  --toolchain-name gfbf --toolchain-version 2025a \
  --package-config $CFG/pspamm.toml --easyconfigs $ROBOT \
  --stack-policy $CFG/../stacks/gfbf-2025a.toml \
  --source-checksum 7d677e1a81897f42daeb9d89cdc596f7ab97b5b829dc4afd4e86804bbaf2af6c \
  --out-dir $OUT
eb-stack package bump --source $ROBOT/s/SeisSol/SeisSol-1.1.4-foss-2023a.eb \
  --toolchain-name foss --toolchain-version 2025a --version 1.3.2 \
  --source-checksum a0dd4f33d7d72c4a5eb6f01f0a96bc410af20c95a496e5cf1532ee8a20561612 \
  --package-config $CFG/seissol.toml --easyconfigs $ROBOT \
  --easyconfigs $OUT/easyconfigs --out-dir $OUT
```

A version bump without `--source-checksum` clears the old digest.
Fill it on the EasyBuild host with `eb --inject-checksums`.

## Expected overlay

Compare against `work/seissol-1.3.2/easyconfigs/` (PR 26851 tip `2b137e36`).
Score with `skills/golden-replay/check-overlay.sh`.

`local_host_arch = 'hsw'` is the PR default. Do not retarget it.

## Standing memory

The pinned set `package-bump` is the operator contract. Atoms:
`examples/ljos-packet/package-bump.jsonl`. Instructions card:
`examples/ljos-packet/package-bump.instructions.md`.
