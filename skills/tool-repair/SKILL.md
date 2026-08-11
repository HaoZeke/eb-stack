---
name: eb-stack-tool-repair
description: Treat eb-stack output as a draft, catch where the emitter is wrong rather than working around it by hand, and close the loop by fixing eb-stack with a regression test, rebuilding on the remote builder, regenerating, and diffing byte-for-byte against the hand-corrected file. Use whenever a generated recipe needs a manual correction, whenever a solve fails on a constraint the package never stated, and before shipping a hand-edited file that the tool was supposed to produce.
---

# When the emitter is wrong, fix the emitter

A hand correction to generated output is a silent fork: the next run of the
same command reproduces the defect, and nobody knows the tool is wrong. Every
manual fix to an emitted recipe is a bug report with a reproducer already
attached.

The rule: hand-correct to unblock, then fix the tool, then regenerate and prove
the tool now produces what you wrote by hand.

## The loop

1. **Correct by hand and keep the file.** It is the expected output of the test
   you are about to write.
2. **Reduce it to a fixture.** Synthetic names, the smallest recipe and robot
   tree that reproduce it. A test named after a package is a test that rots and,
   in this repository, one the package-identity guard rejects outright: the
   production sources may not carry a package name, so put the real case in the
   commit message and the doc comment's reasoning in general terms.
3. **Fix, with the test failing first.** If the test passes before the fix, it
   is testing something else.
4. **Build and test on the remote builder**, never locally:

```sh
rsync -az --delete --exclude 'target/' --exclude '.git/' ./ <builder>:~/tmp/eb-stack-fix/
ssh <builder> 'cd ~/tmp/eb-stack-fix && cargo test 2>&1 | rg "^test result|FAILED"'
ssh <builder> 'cd ~/tmp/eb-stack-fix && cargo fmt --check && cargo clippy --all-targets'
```

5. **Regenerate with the built binary and diff against the hand-corrected file.**

```sh
scp <builder>:~/tmp/eb-stack-fix/target/release/eb-stack /tmp/eb-stack-fixed
/tmp/eb-stack-fixed package bump --source <same args as the failing run> --out-dir /tmp/regen
diff -u /tmp/regen/easyconfigs/<letter>/<Name>/<file>.eb <hand-corrected>.eb
```

Identical is the proof. A remaining difference is either a second defect or a
judgement call the tool cannot make; say which.

6. **Explain any difference that survives.** Regenerating against a tree you
   have since changed can legitimately change the output (a new sibling recipe
   becomes evidence for a patch decision, for instance). That is not a failure,
   but it must be named rather than waved through.

## Where the emitter tends to be wrong

Not a bug list to memorise, a set of places to look before trusting output:

- **Nested structures.** A rewrite that walks the top level and stops there:
  `exts_list` entries carry their own `checksums`, `patches` and `source_tmpl`,
  and they must move with the parent.
- **Derived names.** An emitted filename is derived from name, version,
  toolchain *and* versionsuffix. Anything template-shaped in the suffix is a
  place where a resolved value and a literal template can diverge, and two
  recipes can collide on one path.
- **Constraints the tool synthesised.** A version the tool read from an
  easyconfig is a pin for one generation, not a requirement. When a solve fails
  on a bound the package never stated, the bound is the bug.
- **Tolerant parsing.** A parser that skips what it cannot model will produce
  plausible output from a partly-read file. Run the strict mode; then check
  whether a "skipped statement" is a real defect or valid syntax the parser does
  not cover yet, because both happen.

## Report it even when you cannot fix it

If the fix is out of scope, the reproducer still belongs somewhere durable:
the command, the input, the wrong output, the expected output, and what would
close it. A defect found and not written down is found again by the next person
at the same cost.

## Claims

A tool fix is `builds`-rung evidence for the tool, not for any recipe. Passing
tests and a byte-identical regeneration establish that the emitter is right;
they say nothing about whether the recipe compiles.

## Related

- `skills/verify-recipe/SKILL.md` — the checks that surface these defects
- `skills/annual-bump/SKILL.md` — the bump path most of them live in
- `AGENTS.md` — the claim ladder these rungs come from
