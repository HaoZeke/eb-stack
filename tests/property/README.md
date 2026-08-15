# Property backtests

These drive the built binary against a real `easybuild-easyconfigs` checkout.
Every example is a recipe upstream actually ships, so the corpus exercises the
shapes that exist rather than the shapes someone imagined.

```console
$ cargo build --release
$ uv venv .venv && uv pip install --python .venv/bin/python hypothesis pytest
$ EB_STACK_BIN=target/release/eb-stack \
  EB_EASYCONFIGS=~/src/easybuild-easyconfigs/easybuild/easyconfigs \
  .venv/bin/python -m pytest tests/property -q
```

Four properties, each a thing that must hold of any correct answer:

1. a dependency is built before whatever needs it
2. nothing is built twice
3. the same question gets the same answer
4. a root's own dependencies are all present in the order

`reference.py` reads an easyconfig by executing it, the way EasyBuild does,
and is deliberately not a second parser of the same design: two parsers written
the same way share their blind spots, and a differential test is only worth
running when the two sides fail differently.

An example that cannot be ordered is *rejected*, not passed. An early return
would let a corpus that orders nothing report four green properties, which is
the exact failure this harness exists to catch.
