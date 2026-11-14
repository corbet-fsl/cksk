# Security policy

<!-- ccid:security:begin (rendered by ccid render; do not edit by hand) -->
## How dependencies and security fixes are handled

- Dependencies are checked by [Renovate](https://docs.renovatebot.com/), with vulnerability alerts from [OSV](https://osv.dev/).
- Updates are grouped, and a group is merged into the primary branch only when its tests are green; a red update is not merged and stays visible as an issue until it is fixed.
- Renovate's dependency dashboard issue lists every pending, blocked and merged update.

## Copies run no CI

This repository may be mirrored to other forges. The copies run no CI: everything is built and tested once, on the primary forge, so a copy never produces a different result and no compute is spent twice. Releases, where a project has them, are built there and published to the copies.

## Reporting a problem

Open an issue in this repository's issue tracker on the forge you are using. On a public GitHub copy where private vulnerability reporting is enabled, use the Security tab and choose "Report a vulnerability" instead. Describe the impact; leave out working exploit details from a public issue.
<!-- ccid:security:end -->
