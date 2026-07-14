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

Configure the same image through a target runtime layer:

```toml
[targets.runtime]
kind = "podman"
image = "eb-stack-rocky9"
mounts = ["/shared/work:/work", "/shared/robot:/robot:ro"]
workdir = "/work"
```

`eb-stack campaign run` routes EasyBuild builds and profile verification
through this runtime.

## Capabilities

`eb-in-podman` adds `CAP_NET_RAW` so Perl’s Net-Ping ICMP tests (and similar)
can open raw ICMP sockets. Without it, rootless Podman fails those tests with
`icmp socket error - Operation not permitted`.
