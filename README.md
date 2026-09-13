# cksk

Server-monitor boards for a clck kiosk display: polls status sources, renders big-type
status screens in rotation, and streams frames to the display server. No nix required.

```console
$ cargo build --release
$ ./target/release/cksk --dump board.png --screen overview --source https://status.example.com/api/v1/endpoints/statuses
$ GATUS_URL=... CLCK_SOCKET=$XDG_RUNTIME_DIR/clck.sock ./target/release/cksk
```

Boards (`--screen`, rotation 30s each, hard cut): `overview` (verdict + groups),
`public`, `services`, `mail` (service details at double type size), `system`
(CPU/RAM/Load/disks/GPU/temps from local `/proc`/`/sys` reads + one `df`).
Header (`corbet status` + clock) is identical on every board.

Config: `$XDG_CONFIG_HOME/cksk/config.json` (`gatus_url`, `socket_path`), then
`GATUS_URL` / `CLCK_SOCKET`, then built-in defaults. Previous `nixwatch-frames`
paths and `NIXLOCK_SOCKET` keep working as migration fallbacks.

License: FSL-1.1-ALv2, see `LICENSE.md`.
