# Foreign recipe ingest fixtures

Minimal, frozen inputs for `eb-stack ingest`:

| Path | Format | Notes |
|------|--------|--------|
| `conda_zlib/meta.yaml` | conda-forge classic `meta.yaml` | Plain YAML (no Jinja) |
| `spack_zlib/package.py` | Spack package class | Restricted parse only (no exec) |

These are not full feedstocks or Spack packages — enough shape to drive
name/version/source/deps → EasyBuild scaffold emit.
