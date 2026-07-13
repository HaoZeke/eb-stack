# Contributing to eb-stack

Thank you for your interest. eb-stack is assistive tooling for EasyBuild
toolchain-generation rebuilds: parsers, hierarchy-aware bump, resolvo
co-selection, and reviewable reports. Clear, tested, reproducible changes win.

## Development environment

Rust stable is enough for the library and CLI. Documentation uses
[pixi](https://pixi.sh):

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
then:

```bash
pixi run -e docs docbld
# HTML → docs/build/index.html
```

Do not hand-edit generated `docs/source/**/*.rst`.

## Agent drivers

If you are driving packaging work with an agent, follow `AGENTS.md` and
`skills/annual-bump/SKILL.md`. The PR surface on GitHub remains human-only
unless a maintainer says otherwise in the live conversation.

## Releasing

1. Move Unreleased notes in `CHANGELOG.md` into a versioned section.
2. Bump `version` in `Cargo.toml`, `pixi.toml`, `CITATION.cff`, and
   `docs/source/conf.py` release.
3. Tag `vX.Y.Z` and push the tag; CI publishes the GitHub Release when the
   release workflow is enabled.
