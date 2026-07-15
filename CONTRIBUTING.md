# Contributing to eb-stack

Thank you for your interest. eb-stack is assistive tooling for EasyBuild
toolchain-generation rebuilds: parsers, hierarchy-aware bump, resolvo
co-selection, and reviewable reports. Clear, tested, reproducible changes win.

## Development environment

Rust 1.88 is the minimum supported toolchain; current stable is used for the
main test and lint gates. Documentation uses [pixi](https://pixi.sh):

```bash
cargo test --locked --lib
cargo test --locked --test reproduce_real_prs --test bump_emit
pixi run -e docs docbld   # orgmode → rst → Sphinx (Shibuya)
```

Build heavy work on a remote builder when local compiles thrash the machine.

## Code style and gates

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings` when you touch Rust
- All tests relevant to your change must pass
- New public CLI surface needs a flag in `docs/orgmode/reference/cli.org` and
  either a unit test or an integration test under `tests/`
- Known maintainer-bump regressions live in `tests/reproduce_real_prs.rs` and
  must stay green (library **and** CLI auto-bump)

## Documentation

Primary docs live in orgmode under `docs/orgmode/` (Diataxis). Edit there,
then build:

```bash
# once: system-cargo venv for sphinxcontrib-rust (conda mold cannot link it)
python3 -m venv .venv-docs
export RUSTFLAGS='-C linker=cc'
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc
.venv-docs/bin/pip install 'sphinx>=9,<10' shibuya sphinx-sitemap \
  sphinx-copybutton sphinx-design 'sphinxcontrib-rust>=1,<2' \
  'sphinx-rustdoc-postprocess>=0.1,<0.2'
cargo install sphinx-rustdocgen   # or use an existing ~/.cargo/bin copy

export EB_STACK_DOCS_PYTHON=$PWD/.venv-docs/bin/python
pixi run -e docs docbld
# HTML → docs/build/index.html (includes crates/eb_stack/* Rust API)
```

Do not hand-edit generated `docs/source/**/*.rst` or `docs/source/crates/`.

## Packaging campaigns

Follow `AGENTS.md`, `skills/annual-bump/SKILL.md` (generation rebuild), and
`skills/new-package/SKILL.md` (conda-forge/Spack → new EasyBuild recipe). Keep
site hostnames, paths, credentials, scheduler sizing, and private module names
in the site target layer. The public files contain reusable package intent and
target topology.

## Releasing

Version source of truth: `version` in root `Cargo.toml` (currently **0.3.0**).
Keep these in lockstep on every release:

1. `Cargo.toml` / `pixi.toml` `version`
2. `CITATION.cff` `version`
3. `docs/source/conf.py` `release`
4. `CHANGELOG.md` — move Unreleased notes into `## [X.Y.Z] - YYYY-MM-DD`
5. `CITATION.cff` — add `date-released` when the version is published

Then:

```bash
git tag -a v0.3.0 -m "eb-stack 0.3.0"
git push origin v0.3.0
# GitHub Actions ci_release.yml builds linux x86_64 tarball + checksum
```

Verify the binary surface:

```bash
cargo build --release
./target/release/eb-stack --version
# → eb-stack 0.3.0
```
