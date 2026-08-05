# Shell-stage warning fixtures (`EB_MAINT_SHELL_STAGE`)

`EB_MAINT_SHELL_STAGE` is the milder cousin of `EB_MAINT_SHELL_MONSTER`: it
fires when `preconfigopts` stages another package's build (a `cargo`
invocation or a `(cd ...)` subshell) but stays under the hard-error
thresholds (`PRECONFIG_PLUS_EQ_HARD` = 4 `+=` lines, `PRECONFIG_CHARS_HARD` =
400 chars). Before this fixture pair, the check had zero test coverage: the
only existing shell-staging fixture
(`fixtures/maintainer_reject_26435/bad_shell_monster.eb`) has six `+=` lines
and trips the hard-error branch first, so the `EB_MAINT_SHELL_STAGE`
`else if` branch was never exercised by any test.

| File | Role |
|------|------|
| `staged_below_threshold.eb` | Minimal synthetic reproducer: one `preconfigopts` assignment, a `cargo cinstall` call inside a `(cd ...)` subshell, zero `+=` lines |
| `../eon_core_rgpot/easyconfigs/r/readcon-core/readcon-core-0.13.1-GCCcore-15.2.0.eb` | Real DO-shape control (easybuild-dos-donts skill, DO #3): the actual merged recipe stages its `cargo cinstall` via `install_cmd`/`preinstallopts`, never `preconfigopts`, so it carries no shell-stage finding at all |

This is the check behind the `easybuild-dos-donts` skill's "Put staged
software in its own easyconfig" rule (DO #3) and its matching "Don't stage
companion builds in preconfigopts/postinstallcmds" (DON'T #2), distilled
from the eOn PR (#26480): readcon-core is its own GCCcore recipe rather than
an inline `cargo cinstall` stage inside eOn's own build.
