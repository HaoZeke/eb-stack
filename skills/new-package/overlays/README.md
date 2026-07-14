# Host-OS overlays (optional)

Optional easyconfig/header stubs for **host** EasyBuild when the robot runs
on bare metal. Prefer the **Rocky 9 Podman backend** (`container/rocky9/`)
for *builds* so these are unnecessary.

Copy a required layout under `overlays/<os-id>/` into the campaign bundle and
place that directory first in the target's `robot_paths`. Record the overlay
as target repair evidence in campaign state.

Do not expand the main skill narrative with per-distro war stories; put facts
here if a host backend is required.
