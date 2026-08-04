# RPATH hard-error fixtures (easybuild-easyconfigs #26480)

`EB_MAINT_PATCHELF_RPATH` is grouped with the #26435 hard-error classes
(`easybuild-dos-donts` skill) but its real precedent is #26480, not #26435,
and until now had no fixture or test at all.

| File | Role |
|------|------|
| `readcon-core-0.13.1-draft-patchelf.eb` | Real PR #26480 first-commit content (`df310a91`): `patchelf --force-rpath` in `postinstallcmds` |
| `../eon_core_rgpot/easyconfigs/r/readcon-core/readcon-core-0.13.1-GCCcore-15.2.0.eb` | Real PR #26480 post-review content (`cd9f7f61`): `check_readelf_rpath = False`, no patchelf |

Both files are the *same* recipe (`readcon-core` 0.13.1, GCCcore-15.2.0) at
two real commits of the same PR; only the RPATH handling changed between
them. The draft never shipped upstream: review replaced the `patchelf
--force-rpath` postinstall step before merge.

Reviewer/skill context: "use `check_readelf_rpath = False` when cargo-c
installs lack RPATH, do not invent `$ORIGIN`" (`easybuild-dos-donts` skill,
DON'T #3).

Code: `EB_MAINT_PATCHELF_RPATH` (hard error).
