# Security Policy

eb-stack is a command-line tool for EasyBuild stack planning. It reads local
easyconfig trees and writes lock files, recipes, and reports. It does not
execute builds and does not contact remote package registries by default.

## Supported versions

Only the latest release on the `main` branch is supported.

## Reporting a vulnerability

Email the maintainer (rgoswami@ieee.org) with details. Do not open a public
issue for security matters.

We acknowledge within 72 hours and aim for a fix plus coordinated disclosure
for issues that could lead to arbitrary code execution when processing
untrusted easyconfig input, path traversal when writing outputs, or
supply-chain problems in release artifacts.

Treat `Cargo.lock` and the release binary provenance as part of the security
surface.
