# Maintainer-reject fixtures (easybuild-easyconfigs #26435)

Frozen surfaces for mechanical maintainer-acceptability checks.

| File | Role |
|------|------|
| `eOn-2.16.0-foss-2026.1.eb` | Real rejected PR #26435 head (ocaisa: cross-gen + incomprehensible shell) |
| `bad_cross_gen.eb` | Minimal cross-generation pin hard error |
| `bad_shell_monster.eb` | Minimal staged `preconfigopts` / `patchelf` hard warning path |
| `good_single_gen.eb` | Clean single-generation control |

Reviewer quotes (PR #26435):

- "This is mixing two different toolchain generations, it shouldn't be done"
- "Sorry, but we can never accept this. It's incomprehensible and uncommented"
