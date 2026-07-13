# Rocky 9 Podman backend for EasyBuild *builds*

Default recommendation for greenfield install campaigns: run **`eb --robot`
inside this image**, not on the laptop/host OS. Host OS quirks (package-name
probes, kernel UAPI churn, bleeding-edge system GCC) stay out of the skill
narrative.

## Build image

```bash
podman build -t eb-stack-rocky9 \
  -f skills/new-package/container/rocky9/Containerfile \
  skills/new-package/container/rocky9
```

## Run one recipe

```bash
skills/new-package/container/rocky9/eb-in-podman \
  --work "$WORK" --robot "$ROBOT" -- \
  --robot /work/easyconfigs:/robot Name-Ver-tc.eb
```

`render-full-drive --build-backend podman-rocky9` wires this into `full-drive.sh`.
