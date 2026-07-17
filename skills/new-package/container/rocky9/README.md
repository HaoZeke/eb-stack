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
args = ["--security-opt", "label=disable", "--cap-add=NET_RAW"]
mounts = ["/shared/work:/work", "/shared/robot:/robot:ro"]
workdir = "/work"

[targets.easybuild]
work_root = "/shared/eb-stack/targets/rocky9/campaigns"
tmp_root = "/shared/eb-stack/targets/rocky9/tmp"

[targets.easybuild.environment]
EASYBUILD_INSTALLPATH = "/shared/eb-stack/targets/rocky9/easybuild"
EASYBUILD_SOURCEPATH = "/shared/easybuild/sources"
```

Keep the install, build, and temporary roots specific to the container ABI.
Reusing modules compiled on the host or in another image can load binaries
that require an unavailable glibc or system library. Source archives are
architecture-neutral and may use a shared cache.

Size `EASYBUILD_PARALLEL` against memory as well as CPU count. Several C++
compiler processes can each consume more than a gigabyte; the runnable local
target therefore starts at two jobs. A kernel-killed compiler or exhausted
virtual memory is a resource-allocation failure. Lower parallelism or increase
the scheduler memory request before changing the recipe or dependency choice.

Cargo searches parent directories for `.cargo/config.toml`. Place `work_root`
in a target-owned namespace rather than below a bind-mounted home directory
with personal compiler-wrapper or linker settings. Rust-backed recipes that
disable wrappers must `unset RUSTC_WRAPPER CARGO_BUILD_RUSTC_WRAPPER`. Empty
values remain environment entries and Rust bootstrap can treat the empty
string as a wrapper executable. The target-owned work root prevents `unset`
from exposing an inherited Cargo wrapper.

Sites that store target state below a user's home directory can expose the
same host campaign directory at a neutral container path:

```toml
[targets.runtime]
mounts = [
  "/home/operator:/home/operator",
  "/home/operator/.local/share/eb-stack/targets/rocky9/campaigns:/eb-stack-campaigns",
]
workdir = "/eb-stack-campaigns"

[targets.easybuild]
work_root = "/eb-stack-campaigns"
```

Builds then run below `/eb-stack-campaigns/build`, so Cargo cannot merge a
personal `/home/operator/.cargo/config.toml` into a Rust-backed EasyBuild
recipe.

For rootful container execution, set EasyBuild's explicit acceptance switch in
the workload environment:

```toml
[targets.easybuild.environment]
EASYBUILD_ALLOW_USE_AS_ROOT_AND_ACCEPT_CONSEQUENCES = "1"
```

`eb-stack campaign run` routes EasyBuild builds and profile verification
through this runtime.

## Capabilities

`eb-in-podman` adds `CAP_NET_RAW` so Perl’s Net-Ping ICMP tests (and similar)
can open raw ICMP sockets. Without it, rootless Podman fails those tests with
`icmp socket error - Operation not permitted`.
