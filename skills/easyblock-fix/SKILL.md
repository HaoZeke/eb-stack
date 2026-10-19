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
   When a toolchain option (`openmp`, `usempi`, ...) decides which value applies,
   grep how merged easyblocks read that option (`self.toolchain.options.get(...)`)
   and choose the value from it the same way.

3. **Edit, then lint.** Edit with `eb_recipe_edit`; `new` is written exactly as
   given, so never paste diff markers (`+`, `-`) or line numbers into it. An edit
   that would leave the module invalid Python is refused and the file stays as
   it was: re-read the lines and send the edit again. Run `eb_module_lint` on the
   module until `clean` is true. Do not run `eb_recipe_lint` on a `.py` file.
   `clean` ignores warnings, so read them: `EBM_CFG_OVERRIDE` (a parameter set
   to a fixed value without checking that it is unset),
   `EBM_TEST_STEP_OWN_CMD` (a test step that ignores `runtest`) and
   `EBM_THREADED_VARIANT` (a plain toolchain library variable read with no
   choice of its threaded variant) are bugs in a fix unless the task asks for
   that behaviour.

4. **Dry-run every case you were asked about.** `eb_easyblock_dry_run` with the
   recipe and your module in `easyblocks`. Do it once per recipe, toolchain and
   toolchain option the task names; a recipe with different `toolchainopts` is a
   separate call. Read the generated commands: the option values must have the
   form the build system expects, and the step you meant to add or change must
   show the command you expect. A Python error in your module shows up here.
   The result also has `toolchain_options` (the recipe's `toolchainopts`),
   `threaded_pairs` (variables with a threaded variant) and `notes`: plain and
   threaded variables are both defined whatever the option says, so compare the
   commands of the recipe with and without the option, not the environment.
   When `threaded_findings` is not empty, a command uses a plain value although
   the recipe turns the threaded option on: do what `next` says (edit the
   easyblock to choose the threaded variable from the option) before building.
   When `failure_kind` is `environment`, the recipe cannot be set up where the
   server runs and no easyblock code ran: say which recipe could not be checked
   and go on with the others, without changing the code for it. When it is
   `setup`, EasyBuild itself did not start (the first error line is the module
   system's message): no check ran, so do not change the code for it; quote the
   error and say which checks are still owed.

5. **Build where builds are allowed.** `eb_build_test` runs the real build and
   reports the test step's commands and output. Its description and the dry
   run's `next` say whether builds are on for this server; when they are, build
   each recipe you were asked about before calling the change done, since only
   a build shows link errors and test failures. Say that a build is still owed
   only when `eb_build_test` returned an error, and never claim it passed. On a
   failure, read the failing step with `eb_build_log` and fix the cause the log
   line shows, not a guess. A configure failure is explained in config.log, not
   in the EasyBuild log: read `config_log` in the result (the failed check and
   its compiler or linker output, with the path of the whole file for
   `eb_recipe_read`), fix what it shows, and build again. A failed build is not
   the end of the task: keep fixing and building until it passes or you can
   name, from a log line, a cause outside the easyblock.

6. **Stay in scope.** Change only the easyblock. Do not hardcode library names,
   versions or paths that the toolchain or dependencies provide. Keep the other
   branches of the code working (for example both the static and the shared
   variant when the easyblock handles both), and let an easyconfig still
   override any default you add: set it only when the parameter is unset
   (`if self.cfg['name'] is None:`), and have a test step of your own read
   `runtest` or call `super().test_step()`. When a value the code needs from the
   toolchain or a dependency is missing, raise `EasyBuildError` naming it rather
   than skip it, so the build stops where the cause is.

7. **Describe the diff.** Call `eb_recipe_diff` on the module last. Describe each
   hunk by its `scope` (the method it is in), what it changes, why the report
   needs it, and the merged easyblock you followed.
