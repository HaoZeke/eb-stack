# Host-OS overlays (optional)

Optional easyconfig/header stubs for **host** EasyBuild when the robot runs
on bare metal. Prefer the **Rocky 9 Podman backend** (`container/rocky9/`)
for *builds* so these are unnecessary.

Layouts under `overlays/<os-id>/` are rsynced into `WORK/easyconfigs` only when
`full-drive` runs with `--build-backend host` on a matching host.

Do not expand the main skill narrative with per-distro war stories; put facts
here if a host backend is required.
