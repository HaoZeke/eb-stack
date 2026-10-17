# Negative lint cases

`negative.eb` is a synthetic easyconfig of must-not-flag cases: `${PATH}`, `{{`, `.so.1`, `.sock`,
`%(build_dir)s`, `%(arch)s`, `python -E -s -m`, templates inside f-strings, and comprehensions in
sanity paths. `tests/lint_fixtures.rs` requires eb-stack lint to report no findings at all for it.
