# cksk

Server-monitor boards for a clck kiosk display: polls status sources, renders big-type
status screens in rotation, and streams frames to the display server. No nix required.

```console
$ cargo build --release --config .cargo/release-profile.toml
$ ./target/release/cksk --dump board.png --screen overview --source https://status.example.com/api/v1/endpoints/statuses
$ ./target/release/cksk --dump-pages /tmp/boards --source https://status.example.com/api/v1/endpoints/statuses
$ GATUS_URL=... CLCK_SOCKET=$XDG_RUNTIME_DIR/clck.sock ./target/release/cksk
```

Boards (30s each, hard cut): `overview` (verdict + one row per group), the detail
pages (service details at room-readable type), `system` (compute/memory/temps
columns plus network + pool rows). Header (`corbet status` + clock) is identical
on every board.

Detail pages are derived from the feed, not from a fixed group list: groups are
packed three columns at a time in canonical order (mail, certificates, ci, nodes,
caches, github, seaweedfs, backups, clouds, watchdog) and any group the source
grows after that list is appended, so every row the source returns appears on
some page. `--dump-pages DIR` writes `overview`, every `page-N`, and `system` as
PNGs -- the proof of what the show contains without watching it rotate.
Live rotation is strictly adaptive: overview/system core loop, troubled pages
pin until recovery, healthy detail pages appear as proof-of-life every fifth
cycle. Gatus data older than 90s is labeled stale instead of shown green.

Config (`$XDG_CONFIG_HOME/cksk/config.json`): `gatus_url`, `socket_path`,
optional `host_feed` (path to the host collector JSON with link rates and pool
aggregates the kiosk cannot see itself; missing/stale reads as pending rows).
See `contrib/host-collector/` for the privileged feed script.

Config: `$XDG_CONFIG_HOME/cksk/config.json` (`gatus_url`, `socket_path`), then
`GATUS_URL` / `CLCK_SOCKET`, then built-in defaults. Previous `nixwatch-frames`
paths and `NIXLOCK_SOCKET` keep working as migration fallbacks.

License: FSL-1.1-ALv2, see `LICENSE.md`.
