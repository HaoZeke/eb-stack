---
name: eb-stack-stack-sbom
description: Write a CycloneDX SBOM for a solved EasyBuild stack. Use after stack solve, or pass --sbom-out on the solve. The SBOM reads dependency edges and checksums from the easyconfigs the lock names.
---

# Write a stack SBOM

Run this from the repository root. Solve a lock, then emit the SBOM from that lock.

```sh
eb-stack stack solve \
  --easyconfigs fixtures/gromacs_2025_to_next/easyconfigs \
  --toolchain foss/2025b \
  --root GROMACS \
  --lock-out /tmp/gromacs.lock.json

eb-stack stack sbom \
  --lock /tmp/gromacs.lock.json \
  --out /tmp/gromacs-from-lock.cdx.json
```

The solve on this fixture reports 5 packages. The SBOM command reports 5 components.

```sh
python3 -m json.tool /tmp/gromacs.lock.json >/dev/null
python3 -m json.tool /tmp/gromacs-from-lock.cdx.json >/dev/null
python3 - <<'PY'
import json
doc = json.load(open("/tmp/gromacs-from-lock.cdx.json"))
assert doc["bomFormat"] == "CycloneDX"
assert len(doc["components"]) == 5
PY
```

`--lock` is required. `--out` defaults to `stack.cdx.json` in the current directory. Pass `--sbom-out` to `stack solve` to write the same document during the solve. A path the lock names and that cannot be read contributes nothing for that component.
