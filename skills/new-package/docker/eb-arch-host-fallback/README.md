# Docker fallback for EasyBuild OS-dep name checks

## Prefer host first (Arch)

On Arch, install:

```bash
sudo pacman -S --needed rdma-core
test -f /usr/include/infiniband/verbs.h
```

The campaign `full-drive.sh` (from `render-full-drive`) auto-adds
`--ignore-osdeps` when those headers exist. That is the default fix.

## When to use this image

Only if you need a Debian userspace where EasyBuild’s `libibverbs-dev` /
`rdma-core-devel` package-name probes succeed via `dpkg`, or you want an
isolated build root.

Build and run are documented in the Dockerfile comments. Bind-mount work
tree, robot easyconfigs, and sourcepath; do not bake site hostnames into
images or public skills.
