---
name: eb-stack-easyblock-fix
description: Fix an EasyBuild easyblock (a Python module under easybuild/easyblocks/) from an issue report, prove the fix with the commands EasyBuild generates and with a build where builds are allowed, and describe it hunk by hunk with the merged easyblock each hunk follows. Use when a build, configure option, link line or test of a package is wrong because of what its easyblock does, not because of its easyconfig.
---

# Fix an easyblock

An easyconfig fix changes values. An easyblock fix changes code that every
recipe of that software runs, so it is proven by what EasyBuild does with the
code, not by the file looking right.

## Steps

1. **Read the report and find the module.** A software-specific easyblock lives
   at `easybuild/easyblocks/<first letter>/<name in lower case>.py`; the class is
   `EB_<name>`. Read it whole, then read the generic easyblock it derives from
   (`easybuild/easyblocks/generic/...`): which step methods the parent defines,
   what each one runs, and which easyconfig parameters it reads. Most fixes are
   a few lines in one step method.

2. **Find the precedent before writing code.** Search the easyblocks tree with
   `eb_recipe_grep` for another easyblock that already does what you need (the
   same toolchain value, the same kind of step override). Search the framework
   (`easybuild/tools/toolchain/`, `easybuild/framework/easyblock.py`) for how
   the value you use is defined, so you know its exact form (a list of names, a
   comma-separated string of file names, flags with `-l`, ...). Write the fix
   the way the merged easyblock does it, and keep the file name: you will cite it.

3. **Edit, then lint.** Edit with `eb_recipe_edit`. Run `eb_module_lint` on the
   module until `clean` is true. Do not run `eb_recipe_lint` on a `.py` file.

4. **Dry-run every case you were asked about.** `eb_easyblock_dry_run` with the
   recipe and your module in `easyblocks`. Do it once per recipe, toolchain and
   toolchain option the task names; a recipe with different `toolchainopts` is a
   separate call. Read the generated commands: the option values must have the
   form the build system expects, and the step you meant to add or change must
   show the command you expect. A Python error in your module shows up here.

5. **Build where builds are allowed.** `eb_build_test` runs the real build and
   reports the test step's commands and output. If it is refused, say that the
   build is still owed; do not claim it passed. On a failure, read the failing
   step with `eb_build_log` and fix the cause the log line shows, not a guess.

6. **Stay in scope.** Change only the easyblock. Do not hardcode library names,
   versions or paths that the toolchain or dependencies provide. Keep the other
   branches of the code working (for example both the static and the shared
   variant when the easyblock handles both), and let an easyconfig still
   override any default you add.

7. **Describe the diff.** Call `eb_recipe_diff` on the module last. Describe each
   hunk by its `scope` (the method it is in), what it changes, why the report
   needs it, and the merged easyblock you followed.
