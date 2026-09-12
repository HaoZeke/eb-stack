# packset set `package-bump`

Atoms are `package-bump.jsonl`. This file is the set's instructions card.

```sh
PACKSET_URL=http://127.0.0.1:8761
PACKSET_WORKSPACE=git:github.com/HaoZeke/eb-stack
packset pin
ljos search bump
```

Name the first `package bump` the parent.
Copy its flags exactly.
Exit 1 with `residual=` means continue.
Bump NAME into the same `--out-dir`.
If no `.eb` exists, run `package plan`.
Re-run the parent with the same flags.
Do not add `--allow-unresolved` on the parent.
Stop only when that parent exits 0.
A child exit 0 is not done.

`--allow-unresolved` is a real flag. Use it only for `exclude_from_solve`.

packset and ljos run on the laptop. Do not ssh them to the builder.
The job is `eb-stack package inspect|plan|bump`. Never `cargo`. Never `rustc`.
`eb-stack` on PATH already ssh's to a built CLI on the builder.

`--out-dir` is the bundle root. Companions first.
A version bump clears the old checksum and git commit.
Site generation is the site default.
Package extras belong in `--package-config`.
