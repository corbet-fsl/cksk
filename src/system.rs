//! System board: host resources in giant type, read locally without GPU work.
//! Every source is a plain file read (`/proc`, `/sys`) plus one `df` call per poll for
//! disk usage: no perf counters, nothing that wakes the GPU. `rocm-smi`/`amdgpu_top` are
//! deliberately NOT used -- they can pull the card out of its low-power state.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tiny_skia::Pixmap;

use crate::chrome::{
    self, Fonts, Header, ACCENT, CARD, FAINT, GREEN, HEADER_H, MUTED, PAD, RED, TEXT,
};

const POLL_INTERVAL: Duration = Duration::from_secs(3);

pub struct DiskRow {
    pub mount: String,
    pub used_pct: f64,
    pub used_gib: f64,
    pub total_gib: f64,
}

pub struct GpuInfo {
    pub vram_used_gib: f64,
    pub vram_total_gib: f64,
    pub busy_pct: u64,
    pub power_w: Option<f64>,
    pub edge_c: Option<f32>,
    pub junction_c: Option<f32>,
    pub mem_c: Option<f32>,
}

pub struct SysSnapshot {
    pub cpu_pct: Option<f64>,
    pub cpu_each: Vec<f64>,
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub mem_used_gib: f64,
    pub mem_total_gib: f64,
    pub swap_used_gib: f64,
    pub swap_total_gib: f64,
    pub disks: Vec<DiskRow>,
    pub gpu: Option<GpuInfo>,
    pub cpu_temp_c: Option<f32>,
    pub uptime_s: u64,
}

#[derive(Clone, Copy, Default)]
struct CpuTimes {
    total: u64,
    idle: u64,
}

struct CpuSample {
    all: CpuTimes,
    cores: Vec<CpuTimes>,
}

#[derive(Default)]
struct CpuPrev {
    all: CpuTimes,
    cores: Vec<CpuTimes>,
}

fn read_stat() -> Option<CpuSample> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    let mut all = None;
    let mut cores = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let cpu = parts.next()?;
        let is_all = cpu == "cpu";
        let is_core =
            cpu.len() > 3 && cpu.starts_with("cpu") && cpu[3..].chars().all(|c| c.is_ascii_digit());
        if !is_all && !is_core {
            continue;
        }
        let nums: Vec<u64> = parts.filter_map(|x| x.parse().ok()).collect();
        if nums.len() < 5 {
            continue;
        }
        let times = CpuTimes {
            total: nums.iter().sum(),
            idle: nums[3] + nums[4],
        };
        if is_all {
            all = Some(times);
        } else {
            cores.push(times);
        }
    }
    all.map(|all| CpuSample { all, cores })
}

fn mem_kb(key: &str) -> f64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let mut p = line.split_whitespace();
                if p.next()? != key {
                    return None;
                }
                p.next()?.parse::<f64>().ok()
            })
        })
        .unwrap_or(0.0)
}

fn read_load() -> (f64, f64, f64) {
    let text = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let mut p = text
        .split_whitespace()
        .filter_map(|x| x.parse::<f64>().ok());
    (
        p.next().unwrap_or(0.0),
        p.next().unwrap_or(0.0),
        p.next().unwrap_or(0.0),
    )
}

fn read_uptime_s() -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or(0.0) as u64
}

/// Disk usage via one `df` call per poll for all mounts at once. A subprocess is less
/// pure than statvfs, but statvfs needs an `unsafe` FFI call and this crate forbids
/// unsafe code outright -- and `df` neither touches the GPU nor needs a new dependency.
fn df_usage(mounts: &[String]) -> std::collections::HashMap<String, (f64, f64)> {
    use std::process::Command;
    let mut map = std::collections::HashMap::new();
    if mounts.is_empty() {
        return map;
    }
    let out = Command::new("df")
        .arg("-B1")
        .arg("--output=target,used,size")
        .args(mounts)
        .output();
    let text = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return map,
    };
    let gib = 1024.0 * 1024.0 * 1024.0;
    for line in text.lines().skip(1) {
        let mut p = line.split_whitespace();
        let target = match p.next() {
            Some(t) => t,
            None => continue,
        };
        let (used, size): (f64, f64) = match (p.next(), p.next()) {
            (Some(u), Some(s)) => match (u.parse(), s.parse()) {
                (Ok(u), Ok(s)) => (u, s),
                _ => continue,
            },
            _ => continue,
        };
        if size > 0.0 {
            map.insert(target.to_string(), (used / gib, size / gib));
        }
    }
    map
}

fn read_disks() -> Vec<DiskRow> {
    let text = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let mut mounts = Vec::new();
    for line in text.lines() {
        let mut sides = line.splitn(2, " - ");
        let left = sides.next().unwrap_or("");
        let right = sides.next().unwrap_or("");
        let fields: Vec<&str> = left.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }
        let fstype = right.split_whitespace().next().unwrap_or("");
        if fstype != "zfs" {
            continue;
        }
        let mount = fields[4].replace("\\040", " ");
        if mount.starts_with("/nix/store") {
            continue; // read-only bind of the same pool, no own signal
        }
        mounts.push(mount);
    }
    mounts.sort();
    mounts.dedup();
    let usage = df_usage(&mounts);
    let mut rows: Vec<DiskRow> = mounts
        .iter()
        .filter_map(|m| {
            usage.get(m).map(|(used, total)| DiskRow {
                mount: m.clone(),
                used_pct: used / total * 100.0,
                used_gib: *used,
                total_gib: *total,
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        b.used_gib
            .partial_cmp(&a.used_gib)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows.truncate(4);
    rows
}

fn read_sysfs(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

fn read_hwmon_temps() -> (Option<f32>, Option<f32>, Option<f32>, Option<f32>) {
    // (cpu_tctl, gpu_edge, gpu_junction, gpu_mem)
    let mut out = (None, None, None, None);
    let dir = match std::fs::read_dir("/sys/class/hwmon") {
        Ok(d) => d,
        Err(_) => return out,
    };
    for entry in dir.filter_map(|e| e.ok()) {
        let base = entry.path();
        let name = base
            .join("name")
            .to_str()
            .and_then(|_| read_sysfs(base.join("name").to_str()?));
        let temp = |n: u32| -> Option<f32> {
            let p = base.join(format!("temp{n}_input"));
            read_sysfs(p.to_str()?).and_then(|v| v.parse::<f32>().ok().map(|t| t / 1000.0))
        };
        let label = |n: u32| -> Option<String> {
            let p = base.join(format!("temp{n}_label"));
            read_sysfs(p.to_str()?)
        };
        match name.as_deref() {
            Some("k10temp") => {
                if out.0.is_none() {
                    out.0 = temp(1);
                }
            }
            Some("amdgpu") => {
                for n in 1..=8u32 {
                    match label(n).as_deref() {
                        Some("edge") if out.1.is_none() => out.1 = temp(n),
                        Some("junction") if out.2.is_none() => out.2 = temp(n),
                        Some("mem") if out.3.is_none() => out.3 = temp(n),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn read_gpu() -> Option<GpuInfo> {
    let mut cards: Vec<String> = std::fs::read_dir("/sys/class/drm")
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("card") && n[4..].chars().all(|c| c.is_ascii_digit()))
        .collect();
    cards.sort();
    for card in cards {
        let dev = format!("/sys/class/drm/{card}/device");
        // No VRAM counters (ASPEED, evdi): not the compute GPU, try the next card.
        let total: u64 =
            match read_sysfs(&format!("{dev}/mem_info_vram_total")).and_then(|v| v.parse().ok()) {
                Some(t) => t,
                None => continue,
            };
        let used: u64 = read_sysfs(&format!("{dev}/mem_info_vram_used"))?
            .parse()
            .unwrap_or(0);
        let busy: u64 = read_sysfs(&format!("{dev}/gpu_busy_percent"))?
            .parse()
            .unwrap_or(0);
        let gib = 1024.0 * 1024.0 * 1024.0;
        // Power + temps from the card's own hwmon (microwatt/millidegree sysfs units).
        let mut power_w = None;
        let (mut edge, mut junction, mut mem) = (None, None, None);
        if let Ok(hw) = std::fs::read_dir(format!("{dev}/hwmon")) {
            for h in hw.filter_map(|e| e.ok()) {
                let b = h.path();
                let s = |f: &str| b.join(f).to_str().and_then(read_sysfs);
                if power_w.is_none() {
                    power_w = s("power1_average")
                        .and_then(|v| v.parse::<f64>().ok().map(|p| p / 1_000_000.0));
                }
                for n in 1..=8u32 {
                    let lab = b
                        .join(format!("temp{n}_label"))
                        .to_str()
                        .and_then(read_sysfs);
                    let val = b
                        .join(format!("temp{n}_input"))
                        .to_str()
                        .and_then(read_sysfs)
                        .and_then(|v| v.parse::<f32>().ok().map(|t| t / 1000.0));
                    match lab.as_deref() {
                        Some("edge") => edge = edge.or(val),
                        Some("junction") => junction = junction.or(val),
                        Some("mem") => mem = mem.or(val),
                        _ => {}
                    }
                }
            }
        }
        // First card exposing VRAM counters is the compute GPU (ASPEED/evdi have none).
        return Some(GpuInfo {
            vram_used_gib: used as f64 / gib,
            vram_total_gib: total as f64 / gib,
            busy_pct: busy,
            power_w,
            edge_c: edge,
            junction_c: junction,
            mem_c: mem,
        });
    }
    None
}

fn cpu_pct_since(prev: &CpuTimes, cur: &CpuTimes) -> Option<f64> {
    if prev.total > 0 && cur.total > prev.total {
        let dt = (cur.total - prev.total) as f64;
        let idle = (cur.idle.saturating_sub(prev.idle)) as f64;
        Some((1.0 - idle / dt).clamp(0.0, 1.0) * 100.0)
    } else {
        None // no baseline yet: report honestly as unknown
    }
}

fn read_snapshot(prev: &mut CpuPrev) -> SysSnapshot {
    let (cpu_pct, cpu_each) = match read_stat() {
        Some(sample) => {
            let pct = cpu_pct_since(&prev.all, &sample.all);
            let mut each = Vec::with_capacity(sample.cores.len());
            for (i, cur) in sample.cores.iter().enumerate() {
                let v = match prev.cores.get(i) {
                    Some(p) => cpu_pct_since(p, cur).unwrap_or(0.0),
                    None => 0.0,
                };
                each.push(v);
            }
            prev.all = sample.all;
            prev.cores = sample.cores;
            (pct, each)
        }
        None => (None, Vec::new()),
    };

    let mem_total = mem_kb("MemTotal:") / 1024.0 / 1024.0;
    let mem_avail = mem_kb("MemAvailable:") / 1024.0 / 1024.0;
    let swap_total = mem_kb("SwapTotal:") / 1024.0 / 1024.0;
    let swap_free = mem_kb("SwapFree:") / 1024.0 / 1024.0;
    let (load1, load5, load15) = read_load();
    let (cpu_temp_c, gpu_edge, gpu_junction, gpu_mem) = read_hwmon_temps();
    let mut gpu = read_gpu();
    // hwmon scan and card hwmon read the same sensors; prefer whichever answered.
    if let Some(g) = gpu.as_mut() {
        g.edge_c = g.edge_c.or(gpu_edge);
        g.junction_c = g.junction_c.or(gpu_junction);
        g.mem_c = g.mem_c.or(gpu_mem);
    }

    SysSnapshot {
        cpu_pct,
        cpu_each,
        load1,
        load5,
        load15,
        mem_used_gib: (mem_total - mem_avail).max(0.0),
        mem_total_gib: mem_total,
        swap_used_gib: (swap_total - swap_free).max(0.0),
        swap_total_gib: swap_total,
        disks: read_disks(),
        gpu,
        cpu_temp_c,
        uptime_s: read_uptime_s(),
    }
}

pub struct SysState {
    data: Arc<Mutex<SysSnapshot>>,
}

impl SysState {
    pub fn new() -> Self {
        // Two reads one second apart for the initial snapshot, so even the very first
        // frame (and every --dump preview) reports a real CPU percentage instead of "—".
        // One second of startup cost, once; the poller below keeps it fresh after that.
        let mut prev = CpuPrev::default();
        read_snapshot(&mut prev);
        std::thread::sleep(Duration::from_secs(1));
        let initial = read_snapshot(&mut prev);
        let data = Arc::new(Mutex::new(initial));
        let poll_data = Arc::clone(&data);
        std::thread::spawn(move || loop {
            std::thread::sleep(POLL_INTERVAL);
            *poll_data.lock().unwrap() = read_snapshot(&mut prev);
        });
        SysState { data }
    }

    pub fn snapshot(&self) -> std::sync::MutexGuard<'_, SysSnapshot> {
        self.data.lock().unwrap()
    }
}

impl Default for SysState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn format_uptime(s: u64) -> String {
    let (d, h, m) = (s / 86400, s % 86400 / 3600, s % 3600 / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

// ---------- render ----------

const TITLE_SIZE: f32 = 26.0;
const HEADLINE_SIZE: f32 = 64.0;
const SUB_SIZE: f32 = 26.0;
const FOOT_SIZE: f32 = 26.0;

fn card_title(pm: &mut Pixmap, fonts: &Fonts, x: f32, y: f32, title: &str) {
    chrome::text(pm, &fonts.sans, title, x, y + 34.0, TITLE_SIZE, ACCENT);
}

fn vbar(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, frac: f64, fill: chrome::Rgb) {
    let f = frac.clamp(0.0, 1.0) as f32;
    chrome::fill_rect(pm, x, y, w, h, chrome::HIST_NONE);
    if f > 0.0 {
        chrome::fill_rect(pm, x, y + h * (1.0 - f), w, h * f, fill);
    }
}

pub fn render_system(pm: &mut Pixmap, sys: &SysSnapshot, fonts: &Fonts, hdr: &Header) {
    let w = pm.width() as f32;
    let h = pm.height() as f32;
    let pad = PAD;

    chrome::fill_rect(pm, 0.0, 0.0, w, h, chrome::BG);
    chrome::render_header(pm, fonts, hdr);

    let gap = 20.0;
    let foot_h = 52.0;
    let top = HEADER_H + 16.0;
    let area_h = h - top - pad - foot_h - gap;
    let card_w = (w - 2.0 * pad - gap) / 2.0;
    let card_h = (area_h - gap) / 2.0;
    let xs = [pad, pad + card_w + gap];
    let ys = [top, top + card_h + gap];
    let inpad = 24.0;

    // ---- CPU ----
    {
        let (x, y) = (xs[0], ys[0]);
        chrome::rounded_rect(pm, x, y, card_w, card_h, 16.0, CARD);
        card_title(pm, fonts, x + inpad, y + 8.0, "CPU");
        let big = match sys.cpu_pct {
            Some(p) => format!("{p:.0}%"),
            None => "—".to_string(),
        };
        chrome::text(
            pm,
            &fonts.mono,
            &big,
            x + inpad,
            y + 118.0,
            HEADLINE_SIZE,
            TEXT,
        );
        let load = format!("load {:.2} {:.2} {:.2}", sys.load1, sys.load5, sys.load15);
        chrome::text(
            pm,
            &fonts.mono,
            &load,
            x + inpad,
            y + 158.0,
            SUB_SIZE,
            MUTED,
        );
        // per-core strip along the card bottom
        if !sys.cpu_each.is_empty() {
            let n = sys.cpu_each.len() as f32;
            let bw = (card_w - 2.0 * inpad) / n;
            let bh = 64.0;
            let by = y + card_h - inpad - bh;
            for (i, pct) in sys.cpu_each.iter().enumerate() {
                vbar(
                    pm,
                    x + inpad + i as f32 * bw,
                    by,
                    (bw - 2.0).max(1.0),
                    bh,
                    pct / 100.0,
                    GREEN,
                );
            }
        }
    }

    // ---- MEM ----
    {
        let (x, y) = (xs[1], ys[0]);
        chrome::rounded_rect(pm, x, y, card_w, card_h, 16.0, CARD);
        card_title(pm, fonts, x + inpad, y + 8.0, "RAM");
        let big = format!("{:.0} / {:.0} GiB", sys.mem_used_gib, sys.mem_total_gib);
        chrome::text(
            pm,
            &fonts.mono,
            &big,
            x + inpad,
            y + 118.0,
            HEADLINE_SIZE,
            TEXT,
        );
        let frac = if sys.mem_total_gib > 0.0 {
            sys.mem_used_gib / sys.mem_total_gib
        } else {
            0.0
        };
        chrome::bar(
            pm,
            x + inpad,
            y + 142.0,
            card_w - 2.0 * inpad,
            16.0,
            frac,
            GREEN,
        );
        let swap = if sys.swap_total_gib > 0.0 {
            format!(
                "swap {:.0} / {:.0} GiB",
                sys.swap_used_gib, sys.swap_total_gib
            )
        } else {
            "kein Swap".to_string()
        };
        chrome::text(
            pm,
            &fonts.mono,
            &swap,
            x + inpad,
            y + 196.0,
            SUB_SIZE,
            MUTED,
        );
    }

    // ---- DISK ----
    {
        let (x, y) = (xs[0], ys[1]);
        chrome::rounded_rect(pm, x, y, card_w, card_h, 16.0, CARD);
        card_title(pm, fonts, x + inpad, y + 8.0, "DISKS");
        let mut ry = y + 66.0;
        for d in sys.disks.iter().take(4) {
            let pct_s = format!("{:.0}%", d.used_pct);
            let pct_w = chrome::text_width(&fonts.mono, &pct_s, SUB_SIZE);
            chrome::text_right(
                pm,
                &fonts.mono,
                &pct_s,
                x + card_w - inpad,
                ry + 28.0,
                SUB_SIZE,
                if d.used_pct > 90.0 { RED } else { MUTED },
            );
            let name_max = card_w - 2.0 * inpad - pct_w - 16.0;
            let name = chrome::truncate(&fonts.sans, &d.mount, SUB_SIZE, name_max);
            chrome::text(pm, &fonts.sans, &name, x + inpad, ry + 28.0, SUB_SIZE, TEXT);
            chrome::bar(
                pm,
                x + inpad,
                ry + 40.0,
                card_w - 2.0 * inpad,
                12.0,
                d.used_pct / 100.0,
                if d.used_pct > 90.0 { RED } else { GREEN },
            );
            ry += 84.0;
        }
    }

    // ---- GPU ----
    {
        let (x, y) = (xs[1], ys[1]);
        chrome::rounded_rect(pm, x, y, card_w, card_h, 16.0, CARD);
        card_title(pm, fonts, x + inpad, y + 8.0, "GPU");
        match &sys.gpu {
            Some(g) => {
                let big = format!("{:.1} / {:.0} GiB", g.vram_used_gib, g.vram_total_gib);
                chrome::text(
                    pm,
                    &fonts.mono,
                    &big,
                    x + inpad,
                    y + 118.0,
                    HEADLINE_SIZE,
                    TEXT,
                );
                let frac = if g.vram_total_gib > 0.0 {
                    g.vram_used_gib / g.vram_total_gib
                } else {
                    0.0
                };
                chrome::bar(
                    pm,
                    x + inpad,
                    y + 142.0,
                    card_w - 2.0 * inpad,
                    16.0,
                    frac,
                    GREEN,
                );
                let mut sub = format!("GFX {}%", g.busy_pct);
                if let Some(p) = g.power_w {
                    sub.push_str(&format!("   ·   {p:.0} W"));
                }
                chrome::text(pm, &fonts.mono, &sub, x + inpad, y + 196.0, SUB_SIZE, MUTED);
            }
            None => {
                chrome::text(
                    pm,
                    &fonts.sans,
                    "keine GPU sichtbar",
                    x + inpad,
                    y + 118.0,
                    SUB_SIZE,
                    FAINT,
                );
            }
        }
    }

    // ---- footer: temps + uptime ----
    {
        let mut parts = Vec::new();
        if let Some(t) = sys.cpu_temp_c {
            parts.push(format!("CPU {t:.0}°"));
        }
        if let Some(g) = &sys.gpu {
            let mut gt = Vec::new();
            if let Some(t) = g.edge_c {
                gt.push(format!("{t:.0}°"));
            }
            if let Some(t) = g.junction_c {
                gt.push(format!("{t:.0}°"));
            }
            if let Some(t) = g.mem_c {
                gt.push(format!("{t:.0}°"));
            }
            if !gt.is_empty() {
                parts.push(format!("GPU {}", gt.join(" / ")));
            }
        }
        parts.push(format!("up {}", format_uptime(sys.uptime_s)));
        let foot = parts.join("      ·      ");
        chrome::text(pm, &fonts.sans, &foot, pad, h - pad - 8.0, FOOT_SIZE, MUTED);
    }
}
