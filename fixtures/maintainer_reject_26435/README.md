# Maintainer-reject fixtures (easybuild-easyconfigs #26435)

Frozen surfaces for mechanical maintainer-acceptability checks.

| File | Role |
|------|------|
| `eOn-2.16.0-foss-2026.1.eb` | Real rejected PR #26435 head (review: cross-gen + unreadable staged shell) |
| `bad_cross_gen.eb` | Minimal cross-generation pin hard error |
| `bad_shell_monster.eb` | Minimal staged `preconfigopts` / `patchelf` hard warning path |
| `good_single_gen.eb` | Clean single-generation control |

Review findings (PR #26435), as rules:

- One recipe must not mix toolchain generations.
- Long, uncommented staged shell in build options is not accepted; move staged
  builds to their own easyconfig and comment every deviation.
