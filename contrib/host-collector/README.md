# cksk host collector (contrib)

Privileged host facts the kiosk cannot see from its own namespace: host link
rates, pool aggregates, disk temps. Read-only probes; best-effort per field.

## Install (operator)

1. Copy `cksk-host-feed.sh` to `/usr/local/bin/` (or package it), the unit and
   timer to `/etc/systemd/system/`, `systemctl enable --now cksk-host-feed.timer`.
2. Point the destination at a path the cksk session can read
   (`CKSK_FEED_OUT`, e.g. a shared dataset) and set the same path as cksk's
   `host_feed` config value.
3. No secrets involved: every probe is read-only (`/sys`, `zpool list/iostat`,
   hwmon). Nothing leaves the host except through the JSON file itself.

## Schema

`{"updated_at": <unix>, "net": [{"iface","rx_bps","tx_bps","speed_bps"|null}],
"pools": [{"name","health","used_pct"|null,"read_bps","write_bps",
"util_pct"|null,"temp_c"|null}]}`. cksk treats a missing file, bad JSON or a
feed older than 120s as pending rows -- never as an error screen.
