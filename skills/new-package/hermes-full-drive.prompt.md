# Hermes full-drive prompt template (new-package skill §7)

Fill `WORK`, `REPO`, `ROBOT`, recipe paths, then:

```
herdr agent start eb-full-drive-hermes --cwd "$WORK" --no-focus -- \
  hermes chat --cli --yolo --accept-hooks \
    --provider willma -m openai/gpt-oss-120b \
    -q "$(cat path/to/this-filled-prompt.md)"
```

---

On rg.surf (herdr pane): follow skills/new-package/SKILL.md end to end.
You OWN the full campaign through REAL installs (*builds*). Do not stop at recipe polish.

WORK=<path>
REPO=<eb-stack checkout on rg.surf>
ROBOT=<easybuild-easyconfigs .../easyconfigs>
export PATH=$HOME/.venvs/easybuild/bin:$HOME/.local/bin:$REPO/target/release:$PATH
export LMOD_CMD=$HOME/.local/lmod/lmod/libexec/lmod
source $HOME/.local/lmod/lmod/init/bash 2>/dev/null || true
export EASYBUILD_MODULES_TOOL=Lmod
export EASYBUILD_SOURCEPATH=$HOME/tmp/eb-sources
export EASYBUILD_TMPDIR=$HOME/tmp/eb-tmp
export EASYBUILD_INSTALLPATH=$HOME/tmp/eb-install
mkdir -p $EASYBUILD_SOURCEPATH $EASYBUILD_TMPDIR $EASYBUILD_INSTALLPATH $WORK/logs $WORK/residuals

Recipes:
- <WORK/.../Name-Ver-tc.eb>
Residual queues: <*.residuals.json if any>
Oracles (product surface; copy judgment, do not invent -D):
- <REPO/fixtures/... if any>
Companions under WORK/easyconfigs as needed.

FORBIDDEN: edit REPO/src; open GitHub/GitLab PRs; invent product flags not in
oracle/docs; claim builds without eb --robot success.

Sequence (run every step):
1. Align recipes to oracles / residual queue (cp oracle when match-landable is OK).
2. eb --inject-checksums if needed.
3. eb-stack format-style; eb-stack check-style.
4. eb --modules-tool=Lmod --check-contrib until PASS (W299: strip trailing space).
5. eb-stack check-recipe --recipe … --easyconfigs ROBOT --easyconfigs WORK/easyconfigs.
6. eb -Dr --robot WORK/easyconfigs:ROBOT until exit 0.
7. REAL BUILD: for each package, tee logs:
   eb --modules-tool=Lmod --robot WORK/easyconfigs:ROBOT path.eb 2>&1 | tee WORK/logs/robot-<name>.log
   On failure: fix from log, re-run from step 3. Do not disable sanity.
8. Write WORK/residuals/session-log.md with claim ladder per package
   (resolves / builds / binary-verified).
9. Print DONE_FULL_DRIVE or DONE_PARTIAL with exact failures.

Start now. Run commands; do not only plan.
