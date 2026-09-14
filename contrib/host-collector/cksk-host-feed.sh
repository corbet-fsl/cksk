#!/usr/bin/env bash
# cksk-host-feed: privileged host facts the kiosk cannot see from its own
# namespace (host link rates, pool aggregates, disk temps) as one JSON file
# cksk polls. Read-only probes; best-effort per field (a missing tool, pool or
# disk degrades that field to null, never fails the run).
#
# Output schema (cksk/src/system.rs HostFeedFile):
#   {"updated_at": <unix>, "net": [{"iface","rx_bps","tx_bps","speed_bps"|null}],
#    "pools": [{"name","health","used_pct"|null,"read_bps","write_bps",
#               "util_pct"|null,"temp_c"|null}]}
#
# Environment (all optional):
#   CKSK_FEED_OUT   destination JSON path (default: /var/lib/cksk/host.json;
#                   must be readable from the cksk session -- e.g. a shared dataset)
#   CKSK_IFACES     space-separated interface glob list (default: bond* br* wt* eth* en*)
#   CKSK_POOLS      space-separated pool names (default: all `zpool list` pools)
set -euo pipefail

OUT="${CKSK_FEED_OUT:-/var/lib/cksk/host.json}"
TMP="$OUT.tmp.$$"

have() { command -v "$1" >/dev/null 2>&1; }
jstr() { python3 -c 'import json,sys; print(json.dumps(sys.stdin.read().strip()))' <<<"$1"; }

# ---- disk temps first: drivetemp/nvme hwmon, keyed by kernel disk name ----
declare -A DISK_TEMP=()
for hw in /sys/class/hwmon/hwmon*; do
    [ -f "$hw/name" ] || continue
    devlink=$(readlink -f "$hw/device" 2>/dev/null || echo "")
    disk=$(basename "$(dirname "$devlink")" 2>/dev/null || echo "")
    case "$disk" in sd*|hd*|nvme*) : ;; *) continue ;; esac
    temp=$(cat "$hw"/temp1_input 2>/dev/null || echo "")
    [[ "$temp" =~ ^[0-9]+$ ]] || continue
    DISK_TEMP[$disk]=$(python3 -c "print(round($temp/1000, 1))")
done

# ---- network: two samples 1s apart -> bit/s rates + link speed ----
ifaces=()
# shellcheck disable=SC2206
pats=(${CKSK_IFACES:-bond* br* wt* eth* en*})
for path in /sys/class/net/*; do
    if=$(basename "$path")
    case "$if" in lo|veth*|docker*|podman*|cni*|flannel*|tunl*|bonding_masters) continue ;; esac
    [ "$(cat "$path/operstate" 2>/dev/null)" = up ] || continue
    for pat in "${pats[@]}"; do
        # shellcheck disable=SC2053
        if [[ $if == $pat ]]; then ifaces+=("$if"); break; fi
    done
done
declare -A rx0 tx0
for if in "${ifaces[@]}"; do
    rx0[$if]=$(cat "/sys/class/net/$if/statistics/rx_bytes" 2>/dev/null || echo 0)
    tx0[$if]=$(cat "/sys/class/net/$if/statistics/tx_bytes" 2>/dev/null || echo 0)
done
sleep 1
net_json=""
for if in "${ifaces[@]}"; do
    rx1=$(cat "/sys/class/net/$if/statistics/rx_bytes" 2>/dev/null || echo 0)
    tx1=$(cat "/sys/class/net/$if/statistics/tx_bytes" 2>/dev/null || echo 0)
    rx_bps=$(( (rx1 - rx0[$if]) * 8 ))
    tx_bps=$(( (tx1 - tx0[$if]) * 8 ))
    speed_mbps=$(cat "/sys/class/net/$if/speed" 2>/dev/null || echo "")
    if [[ "$speed_mbps" =~ ^[0-9]+$ ]]; then
        speed_json=$(( speed_mbps * 1000000 ))
    else
        speed_json=null
    fi
    [ -n "$net_json" ] && net_json+=","
    net_json+=$(printf '{"iface":%s,"rx_bps":%s,"tx_bps":%s,"speed_bps":%s}' \
        "$(jstr "$if")" "$rx_bps" "$tx_bps" "$speed_json")
done

# ---- pools: health + usage + IO + hottest member temp ----
pools_json=""
if have zpool; then
    if [ -n "${CKSK_POOLS:-}" ]; then
        # shellcheck disable=SC2206
        pools=($CKSK_POOLS)
    else
        mapfile -t pools < <(zpool list -H -o name 2>/dev/null || true)
    fi
    for pool in "${pools[@]}"; do
        [ -n "$pool" ] || continue
        read -r health alloc size < <(zpool list -H -p -o health,alloc,size "$pool" 2>/dev/null || echo "UNKNOWN 0 0")
        used_pct=null
        if [ "${size:-0}" -gt 0 ] 2>/dev/null; then
            used_pct=$(python3 -c "print(round($alloc / $size * 100, 1))" 2>/dev/null || echo null)
        fi
        # 1s iostat sample, second report only (first is since-boot totals).
        io=$(zpool iostat -Hp "$pool" 1 2 2>/dev/null | tail -1 || echo "")
        read_b=$(echo "$io" | awk '{print $7}')
        write_b=$(echo "$io" | awk '{print $8}')
        read_bps=$(python3 -c "print(float('${read_b:-0}')*8)" 2>/dev/null || echo 0)
        write_bps=$(python3 -c "print(float('${write_b:-0}')*8)" 2>/dev/null || echo 0)
        # Member disks -> hottest temp. -P prints stable /dev paths; basename them.
        hottest=null
        while read -r dev; do
            [ -n "$dev" ] || continue
            t=${DISK_TEMP[$dev]:-}
            [ -n "$t" ] || continue
            if [ "$hottest" = null ] || python3 -c "exit(0 if $t > $hottest else 1)" 2>/dev/null; then
                hottest=$t
            fi
        done < <(zpool status -P "$pool" 2>/dev/null | grep -oE '/dev/(sd[a-z]+|nvme[0-9]+n[0-9]+|hd[a-z]+|vd[a-z]+)[p0-9]*' | xargs -n1 basename 2>/dev/null | sed 's/[p0-9]*$//' | sort -u || true)
        [ -n "$pools_json" ] && pools_json+=","
        pools_json+=$(printf '{"name":%s,"health":%s,"used_pct":%s,"read_bps":%s,"write_bps":%s,"util_pct":null,"temp_c":%s}' \
            "$(jstr "$pool")" "$(jstr "${health:-UNKNOWN}")" "$used_pct" "$read_bps" "$write_bps" "$hottest")
    done
fi
# NOTE: util_pct stays null (needs zpool iostat capacity -v parsing per vdev;
# r/w rates above already answer "is the pool busy"). SMART health beyond the
# pool state string is a follow-up (smartctl lane on the hub).

now=$(date +%s)
mkdir -p "$(dirname "$OUT")"
printf '{"updated_at":%s,"net":[%s],"pools":[%s]}\n' \
    "$now" "$net_json" "$pools_json" > "$TMP"
mv "$TMP" "$OUT"
echo "cksk-host-feed: wrote $OUT"
