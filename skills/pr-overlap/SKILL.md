---
name: eb-stack-pr-overlap
description: Report modules that more than one open pull request would build. Use the recorded software-layer inputs in this repository, or the same flags with other pull-request JSON. Nothing from a pull request is executed.
---

# Report shared pull-request builds

Run this from the repository root. The inputs are recorded pull requests 1585 and 1728.

```sh
eb-stack pr-overlap \
  --pr tests/pr_overlap_layer/prs.json \
  --easyconfigs tests/pr_overlap_layer \
  --installed tests/pr_overlap_layer/installed.txt \
  --installed-label none \
  --generated-at 2026-10-24T00:00:00Z \
  --repo-url https://github.com/EESSI/software-layer \
  --out-dir /tmp/pr-overlap-out
```

The command prints `prs=2` and `shared_modules=91` for this recording. It writes:

```sh
test -s /tmp/pr-overlap-out/overlap.md
test -s /tmp/pr-overlap-out/overlap.toml
test -d /tmp/pr-overlap-out/prs
```

`--kind` defaults to `software-layer`, which reads easystack additions and requires `--installed`. `--kind easyconfigs` reads changed `.eb` files. Repeat `--pr`, or pass `--open OWNER/NAME` with `--kind easyconfigs` to read open pull requests over HTTPS. `--impact N` writes `merge-impact.toml` for pull request N. `--merge-order` writes `merge-order.toml`.

`--bot-build-set` closes every head entry of each changed easystack, not only added lines. `--arch-installed ARCH=FILE` repeats, once per architecture, and implies that flag. It replaces `--installed`. Two or more architectures write `build_sets` and per-architecture merge order and impact. The recorded shape is `tests/pr_overlap_arch`.

Exit status is 2 when the command line is rejected, including a `--merged` or `--impact` number that has no overlap record.
