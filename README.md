# cksk

Server-monitor boards for a clck kiosk display: polls status sources, renders big-type
status screens in rotation, and streams frames to the display server. No nix required.

```console
$ cargo build --release
$ ./target/release/cksk --dump board.png --screen overview --source https://status.example.com/api/v1/endpoints/statuses
$ GATUS_URL=... CLCK_SOCKET=$XDG_RUNTIME_DIR/clck.sock ./target/release/cksk
```

Boards (`--screen`, 30s each, hard cut; mailbox 60s): `overview` (verdict + groups),
`public`, `services`, `mail` (service details at room-readable type), `system`
(compute/memory/temps columns plus network + pool rows).
Header (`corbet status` + clock) is identical on every board.
Live rotation is strictly adaptive: overview/system core loop, troubled boards
pin until recovery, healthy detail boards appear as proof-of-life every fifth
cycle. Gatus data older than 90s is labeled stale instead of shown green.

Config (`$XDG_CONFIG_HOME/cksk/config.json`): `gatus_url`, `socket_path`,
optional `host_feed` (path to the host collector JSON with link rates and pool
aggregates the kiosk cannot see itself; missing/stale reads as pending rows).
See `contrib/host-collector/` for the privileged feed script.

Config: `$XDG_CONFIG_HOME/cksk/config.json` (`gatus_url`, `socket_path`), then
`GATUS_URL` / `CLCK_SOCKET`, then built-in defaults. Previous `nixwatch-frames`
paths and `NIXLOCK_SOCKET` keep working as migration fallbacks.

License: FSL-1.1-ALv2, see `LICENSE.md`.
