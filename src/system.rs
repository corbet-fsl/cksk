//! System board: host resources in giant type, read locally without GPU work.
//! Every source is a plain file read (`/proc`, `/sys`) plus one `df` call per poll for
//! disk usage: no perf counters, nothing that wakes the GPU. `rocm-smi`/`amdgpu_top` are
//! deliberately NOT used -- they can pull the card out of its low-power state.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tiny_skia::Pixmap;

use crate::chrome::{
    self, Fonts, Header, ACCENT, CARD, FAINT, GREEN, HEADER_H, MUTED, PAD, RED, TEXT,
};

const FAST_INTERVAL: Duration = Duration::from_secs(1);
const SLOW_EVERY_NTH_TICK: u64 = 5;

pub struct GpuInfo {
    pub vram_used_gib: f64,
    pub vram_total_gib: f64,
    pub busy_pct: u64,
    pub power_w: Option<f64>,
    pub edge_c: Option<f32>,
    pub junction_c: Option<f32>,
    pub mem_c: Option<f32>,
}

/// One network interface as the host collector saw it (rates already per second).
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct NetIf {
    pub iface: String,
    pub rx_bps: f64,
    pub tx_bps: f64,
    /// Link speed in bit/s, when known. Without it only absolute rates render.
    pub speed_bps: Option<f64>,
}

/// One pool, aggregated by the host collector (cksk never maps disks itself).
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct PoolHealth {
    pub name: String,
    /// e.g. ONLINE / DEGRADED / FAULTED / UNKNOWN (collector degrades gracefully).
    pub health: String,
    pub used_pct: Option<f64>,
    pub read_bps: f64,
    pub write_bps: f64,
    pub util_pct: Option<f64>,
    pub temp_c: Option<f32>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct HostFeedFile {
    updated_at: u64,
    #[serde(default)]
    net: Vec<NetIf>,
    #[serde(default)]
    pools: Vec<PoolHealth>,
}

/// Refresh the cached host rows. A failed read keeps the previous rows; their age
/// keeps growing from the last good read instead of flapping to pending and back.
fn refresh_feed(feed: &mut FeedState) {
    if let Some((net, pools)) = read_host_feed(feed.path.as_deref()) {
        feed.net = net;
        feed.pools = pools;
        feed.last_ok = Some(Instant::now());
    }
}

/// Read the host feed if configured. Never fails the board: missing file, bad JSON
/// or stale data all degrade to pending rows, never to an error screen.
fn read_host_feed(path: Option<&str>) -> Option<(Vec<NetIf>, Vec<PoolHealth>)> {
    let bytes = std::fs::read(path?).ok()?;
    let feed: HostFeedFile = serde_json::from_slice(&bytes)
        .map_err(|e| {
            eprintln!("cksk: host feed parse failed: {e}");
        })
        .ok()?;
    let age = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().saturating_sub(feed.updated_at))
        .unwrap_or(0);
    // Stale feed, wrongly fresh timestamps and all: older than 2 minutes reads as absent.
    if age > 120 {
        return None;
    }
    Some((feed.net, feed.pools))
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
    pub gpu: Option<GpuInfo>,
    pub cpu_temp_c: Option<f32>,
    pub cpu_ccd: Vec<f32>,
    pub uptime_s: u64,
    /// Last 60 fast-tick samples (one per second) for the history strips.
    pub cpu_hist: Vec<f64>,
    /// Host-collected rows (network interfaces, pool aggregates). Empty until the
    /// host collector lands; the board renders pending placeholders instead.
    pub net: Vec<NetIf>,
    pub pools: Vec<PoolHealth>,
    /// Seconds since the host feed was written, if a feed was ever read.
    pub feed_age_secs: Option<u64>,
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

fn read_sysfs(path: &str) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

#[derive(Default)]
struct HwTemps {
    cpu: Option<f32>,
    ccd: Vec<f32>,
    edge: Option<f32>,
    junction: Option<f32>,
    mem: Option<f32>,
}

fn read_hwmon_temps() -> HwTemps {
    let mut out = HwTemps::default();
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
                for n in 1..=8u32 {
                    let lab = label(n).unwrap_or_default().to_lowercase();
                    if n == 1 && out.cpu.is_none() {
                        out.cpu = temp(n);
                    } else if lab.contains("ccd") && out.ccd.len() < 4 {
                        if let Some(t) = temp(n) {
                            out.ccd.push(t);
                        }
                    }
                }
            }
            Some("amdgpu") => {
                for n in 1..=8u32 {
                    match label(n).as_deref() {
                        Some("edge") if out.edge.is_none() => out.edge = temp(n),
                        Some("junction") if out.junction.is_none() => out.junction = temp(n),
                        Some("mem") if out.mem.is_none() => out.mem = temp(n),
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

/// Rolling history for the sparkline strips: one sample per fast tick, capped.
#[derive(Default)]
struct Hist {
    cpu: Vec<f64>,
}

/// Host-feed cache: path from config plus the last good rows, so a slow tick that
/// finds no file (collector restarting, first boot) keeps showing the previous
/// rows with a growing age instead of flapping to pending and back.
#[derive(Default)]
struct FeedState {
    path: Option<String>,
    net: Vec<NetIf>,
    pools: Vec<PoolHealth>,
    last_ok: Option<Instant>,
}

impl Hist {
    fn push(&mut self, cpu_pct: Option<f64>) {
        if let Some(p) = cpu_pct {
            self.cpu.push(p);
        }
        let excess = self.cpu.len().saturating_sub(60);
        self.cpu.drain(..excess);
    }
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

/// Fast tick (1s): counters, load, memory, GPU, temps -- all plain file reads.
/// The host feed refreshes on the slow lane (every 5th tick) in the poller.
fn read_snapshot(prev: &mut CpuPrev, hist: &mut Hist, feed: &mut FeedState) -> SysSnapshot {
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
    let mem_used = (mem_total - mem_avail).max(0.0);
    hist.push(cpu_pct);
    let swap_total = mem_kb("SwapTotal:") / 1024.0 / 1024.0;
    let swap_free = mem_kb("SwapFree:") / 1024.0 / 1024.0;
    let (load1, load5, load15) = read_load();
    let temps = read_hwmon_temps();
    let mut gpu = read_gpu();
    // hwmon scan and card hwmon read the same sensors; prefer whichever answered.
    if let Some(g) = gpu.as_mut() {
        g.edge_c = g.edge_c.or(temps.edge);
        g.junction_c = g.junction_c.or(temps.junction);
        g.mem_c = g.mem_c.or(temps.mem);
    }

    SysSnapshot {
        cpu_pct,
        cpu_each,
        load1,
        load5,
        load15,
        mem_used_gib: mem_used,
        mem_total_gib: mem_total,
        swap_used_gib: (swap_total - swap_free).max(0.0),
        swap_total_gib: swap_total,
        gpu,
        cpu_temp_c: temps.cpu,
        cpu_ccd: temps.ccd,
        uptime_s: read_uptime_s(),
        cpu_hist: hist.cpu.clone(),
        net: feed.net.clone(),
        pools: feed.pools.clone(),
        feed_age_secs: feed.last_ok.map(|t| t.elapsed().as_secs()),
    }
}

pub struct SysState {
    data: Arc<Mutex<SysSnapshot>>,
}

impl SysState {
    pub fn new() -> Self {
        Self::with_feed(None)
    }

    pub fn with_feed(feed_path: Option<String>) -> Self {
        // Two reads one second apart for the initial snapshot, so even the very first
        // frame (and every --dump preview) reports a real CPU percentage instead of "—".
        // One second of startup cost, once; the poller below keeps it fresh after that.
        let mut prev = CpuPrev::default();
        let mut hist = Hist::default();
        let mut feed = FeedState {
            path: feed_path,
            ..Default::default()
        };
        refresh_feed(&mut feed);
        read_snapshot(&mut prev, &mut hist, &mut feed);
        std::thread::sleep(Duration::from_secs(1));
        let initial = read_snapshot(&mut prev, &mut hist, &mut feed);
        let data = Arc::new(Mutex::new(initial));
        let poll_data = Arc::clone(&data);
        std::thread::spawn(move || {
            let mut tick: u64 = 0;
            loop {
                std::thread::sleep(FAST_INTERVAL);
                tick += 1;
                // Slow lane every 5th tick: the host feed joins at 5s cadence.
                if tick.is_multiple_of(SLOW_EVERY_NTH_TICK) {
                    refresh_feed(&mut feed);
                }
                *poll_data.lock().unwrap() = read_snapshot(&mut prev, &mut hist, &mut feed);
            }
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
const SUB_SIZE: f32 = 26.0;
const INPAD: f32 = 24.0;

/// Shared render context: seven-argument card functions collapse to five.
struct Cx<'a> {
    pm: &'a mut Pixmap,
    fonts: &'a Fonts,
    sys: &'a SysSnapshot,
}

fn card_title(pm: &mut Pixmap, fonts: &Fonts, x: f32, y: f32, title: &str) {
    chrome::text(pm, &fonts.sans, title, x, y + 34.0, TITLE_SIZE, ACCENT);
}

fn hist_strip(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, hist: &[f64]) {
    if hist.is_empty() {
        chrome::fill_rect(pm, x, y, w, h, chrome::HIST_NONE);
        return;
    }
    let n = hist.len() as f32;
    let seg = (w / n).max(2.0);
    for (k, v) in hist.iter().enumerate() {
        vbar(
            pm,
            x + k as f32 * seg,
            y,
            (seg - 1.0).max(1.0),
            h,
            v / 100.0,
            GREEN,
        );
    }
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

    // Vertical budget (1080p): header 112 + main 520 + net 140 + pools 170 + gaps.
    let gap = 20.0;
    let top = HEADER_H + 16.0;
    let main_h = 520.0;
    let net_y = top + main_h + gap;
    let net_h = 140.0;
    let pool_y = net_y + net_h + gap;
    let pool_h = h - pool_y - pad;

    let col_gap = 24.0;
    let col_w = (w - 2.0 * pad - 2.0 * col_gap) / 3.0;
    let colx = [pad, pad + col_w + col_gap, pad + 2.0 * (col_w + col_gap)];
    let mut cx = Cx { pm, fonts, sys };

    // ---- column 1: COMPUTE (CPU over GPU) ----
    {
        let cpu_h = 320.0;
        let gpu_h = main_h - cpu_h - gap;
        cpu_card(&mut cx, colx[0], top, col_w, cpu_h);
        gpu_card(&mut cx, colx[0], top + cpu_h + gap, col_w, gpu_h);
    }

    // ---- column 2: MEMORY (RAM over VRAM) ----
    {
        let ram_h = 320.0;
        let vram_h = main_h - ram_h - gap;
        ram_card(&mut cx, colx[1], top, col_w, ram_h);
        vram_card(&mut cx, colx[1], top + ram_h + gap, col_w, vram_h);
    }

    // ---- column 3: TEMPS ----
    temps_card(&mut cx, colx[2], top, col_w, main_h);

    // ---- bottom row: NETWORK, one line of interfaces ----
    net_row(&mut cx, pad, net_y, w - 2.0 * pad, net_h);

    // ---- bottom row: POOLS, aggregates only, never datasets ----
    pools_row(&mut cx, pad, pool_y, w - 2.0 * pad, pool_h);
}

fn cpu_card(cx: &mut Cx, x: f32, y: f32, w: f32, h: f32) {
    let Cx { pm, fonts, sys } = cx;
    chrome::rounded_rect(pm, x, y, w, h, 16.0, CARD);
    card_title(pm, fonts, x + INPAD, y + 8.0, "CPU");
    let big = match sys.cpu_pct {
        Some(p) => format!("{p:.0}%"),
        None => "—".to_string(),
    };
    chrome::text(pm, &fonts.mono, &big, x + INPAD, y + 108.0, 64.0, TEXT);
    let load = format!("load {:.2} {:.2} {:.2}", sys.load1, sys.load5, sys.load15);
    chrome::text(
        pm,
        &fonts.mono,
        &load,
        x + INPAD,
        y + 144.0,
        SUB_SIZE,
        MUTED,
    );
    hist_strip(
        pm,
        x + INPAD,
        y + 160.0,
        w - 2.0 * INPAD,
        14.0,
        &sys.cpu_hist,
    );
    if !sys.cpu_each.is_empty() {
        let n = sys.cpu_each.len() as f32;
        let bw = (w - 2.0 * INPAD) / n;
        let bh = 56.0;
        let by = y + h - INPAD - bh;
        for (i, pct) in sys.cpu_each.iter().enumerate() {
            vbar(
                pm,
                x + INPAD + i as f32 * bw,
                by,
                (bw - 2.0).max(1.0),
                bh,
                pct / 100.0,
                GREEN,
            );
        }
    }
}

fn gpu_card(cx: &mut Cx, x: f32, y: f32, w: f32, h: f32) {
    let Cx { pm, fonts, sys } = cx;
    chrome::rounded_rect(pm, x, y, w, h, 16.0, CARD);
    card_title(pm, fonts, x + INPAD, y + 8.0, "GPU");
    match &sys.gpu {
        Some(g) => {
            let big = format!("GFX {}%", g.busy_pct);
            chrome::text(pm, &fonts.mono, &big, x + INPAD, y + 96.0, 56.0, TEXT);
            let mut sub = format!("VRAM {:.1} GiB", g.vram_used_gib);
            if let Some(p) = g.power_w {
                sub.push_str(&format!("   ·   {p:.0} W"));
            }
            chrome::text(pm, &fonts.mono, &sub, x + INPAD, y + 132.0, SUB_SIZE, MUTED);
        }
        None => {
            chrome::text(
                pm,
                &fonts.sans,
                "no GPU visible",
                x + INPAD,
                y + 96.0,
                SUB_SIZE,
                FAINT,
            );
        }
    }
    let _ = h;
}

fn ram_card(cx: &mut Cx, x: f32, y: f32, w: f32, h: f32) {
    let Cx { pm, fonts, sys } = cx;
    chrome::rounded_rect(pm, x, y, w, h, 16.0, CARD);
    card_title(pm, fonts, x + INPAD, y + 8.0, "RAM");
    let big = format!("{:.0} / {:.0} GiB", sys.mem_used_gib, sys.mem_total_gib);
    chrome::text(pm, &fonts.mono, &big, x + INPAD, y + 96.0, 60.0, TEXT);
    let frac = if sys.mem_total_gib > 0.0 {
        sys.mem_used_gib / sys.mem_total_gib
    } else {
        0.0
    };
    chrome::bar(pm, x + INPAD, y + 118.0, w - 2.0 * INPAD, 16.0, frac, GREEN);
    // Swap mirrors RAM one-to-one: own title, own headline, own bar -- never a footnote.
    card_title(pm, fonts, x + INPAD, y + 156.0, "SWAP");
    let swap_big = if sys.swap_total_gib > 0.0 {
        format!("{:.0} / {:.0} GiB", sys.swap_used_gib, sys.swap_total_gib)
    } else {
        "no swap".to_string()
    };
    let swap_frac = if sys.swap_total_gib > 0.0 {
        sys.swap_used_gib / sys.swap_total_gib
    } else {
        0.0
    };
    let swap_color = if swap_frac > 0.9 {
        RED
    } else if swap_frac > 0.7 {
        chrome::AMBER
    } else {
        GREEN
    };
    chrome::text(pm, &fonts.mono, &swap_big, x + INPAD, y + 220.0, 60.0, TEXT);
    chrome::bar(
        pm,
        x + INPAD,
        y + 242.0,
        w - 2.0 * INPAD,
        16.0,
        swap_frac,
        swap_color,
    );
    let _ = h;
}

fn vram_card(cx: &mut Cx, x: f32, y: f32, w: f32, h: f32) {
    let Cx { pm, fonts, sys } = cx;
    chrome::rounded_rect(pm, x, y, w, h, 16.0, CARD);
    card_title(pm, fonts, x + INPAD, y + 8.0, "VRAM");
    match &sys.gpu {
        Some(g) => {
            let big = format!("{:.1} / {:.0} GiB", g.vram_used_gib, g.vram_total_gib);
            chrome::text(pm, &fonts.mono, &big, x + INPAD, y + 108.0, 64.0, TEXT);
            let frac = if g.vram_total_gib > 0.0 {
                g.vram_used_gib / g.vram_total_gib
            } else {
                0.0
            };
            chrome::bar(pm, x + INPAD, y + 132.0, w - 2.0 * INPAD, 16.0, frac, GREEN);
        }
        None => {
            chrome::text(
                pm,
                &fonts.sans,
                "no GPU visible",
                x + INPAD,
                y + 108.0,
                SUB_SIZE,
                FAINT,
            );
        }
    }
    let _ = h;
}

fn temp_row(
    pm: &mut Pixmap,
    fonts: &Fonts,
    x: f32,
    y: f32,
    w: f32,
    label: &str,
    value: Option<String>,
) {
    chrome::text(pm, &fonts.sans, label, x, y, 30.0, MUTED);
    match value {
        Some(v) => chrome::text_right(pm, &fonts.mono, &v, x + w, y, 32.0, TEXT),
        None => chrome::text_right(pm, &fonts.mono, "—", x, y, 32.0, FAINT),
    }
}

fn fmt_temp(c: Option<f32>) -> Option<String> {
    c.map(|t| format!("{t:.0}°"))
}

fn temps_card(cx: &mut Cx, x: f32, y: f32, w: f32, h: f32) {
    let Cx { pm, fonts, sys } = cx;
    chrome::rounded_rect(pm, x, y, w, h, 16.0, CARD);
    card_title(pm, fonts, x + INPAD, y + 8.0, "TEMPS");
    let inner = w - 2.0 * INPAD;
    let mut ry = y + 78.0;
    let step = 52.0;
    temp_row(
        pm,
        fonts,
        x + INPAD,
        ry,
        inner,
        "CPU",
        fmt_temp(sys.cpu_temp_c),
    );
    ry += step;
    for (i, t) in sys.cpu_ccd.iter().enumerate() {
        temp_row(
            pm,
            fonts,
            x + INPAD,
            ry,
            inner,
            &format!("CPU CCD{}", i + 1),
            fmt_temp(Some(*t)),
        );
        ry += step;
    }
    if let Some(g) = &sys.gpu {
        for (label, t) in [
            ("GPU edge", g.edge_c),
            ("GPU junction", g.junction_c),
            ("GPU mem", g.mem_c),
        ] {
            temp_row(pm, fonts, x + INPAD, ry, inner, label, fmt_temp(t));
            ry += step;
        }
    }
    for p in sys.pools.iter().take(2) {
        temp_row(
            pm,
            fonts,
            x + INPAD,
            ry,
            inner,
            &format!("pool {}", p.name),
            fmt_temp(p.temp_c),
        );
        ry += step;
    }
    chrome::text(
        pm,
        &fonts.mono,
        &format!("up {}", format_uptime(sys.uptime_s)),
        x + INPAD,
        y + h - INPAD,
        SUB_SIZE,
        MUTED,
    );
}

/// Human bit-rate: 340 kb/s, 12 Mb/s, 1.2 Gb/s. Below 1 kb/s reads as idle dots.
fn human_bps(bps: f64) -> String {
    if bps < 1_000.0 {
        "—".to_string()
    } else if bps < 1_000_000.0 {
        format!("{:.0} kb/s", bps / 1_000.0)
    } else if bps < 1_000_000_000.0 {
        format!("{:.0} Mb/s", bps / 1_000_000.0)
    } else {
        format!("{:.1} Gb/s", bps / 1_000_000_000.0)
    }
}

fn net_row(cx: &mut Cx, x: f32, y: f32, w: f32, h: f32) {
    let Cx { pm, fonts, sys } = cx;
    chrome::rounded_rect(pm, x, y, w, h, 16.0, CARD);
    card_title(pm, fonts, x + INPAD, y + 4.0, "NET");
    if sys.net.is_empty() {
        pending_strip(pm, fonts, x + INPAD, y + 52.0, w - 2.0 * INPAD, "network");
        return;
    }
    let n = sys.net.len().max(1) as f32;
    let bw = (w - 2.0 * INPAD) / n;
    for (i, nic) in sys.net.iter().enumerate() {
        let bx = x + INPAD + i as f32 * bw;
        chrome::text(pm, &fonts.sans, &nic.iface, bx, y + 74.0, 24.0, ACCENT);
        let rates = format!("↓ {}   ↑ {}", human_bps(nic.rx_bps), human_bps(nic.tx_bps));
        chrome::text(pm, &fonts.mono, &rates, bx, y + 102.0, 22.0, MUTED);
        let peak = nic.rx_bps.max(nic.tx_bps);
        if let Some(speed) = nic.speed_bps {
            if speed > 0.0 {
                chrome::bar(
                    pm,
                    bx,
                    y + h - INPAD - 10.0,
                    bw - 16.0,
                    10.0,
                    peak / speed,
                    GREEN,
                );
                let pct = format!("{:.1}%", peak / speed * 100.0);
                chrome::text_right(pm, &fonts.mono, &pct, bx + bw - 16.0, y + 56.0, 22.0, MUTED);
            }
        }
    }
    if let Some(age) = sys.feed_age_secs {
        chrome::text_right(
            pm,
            &fonts.mono,
            &format!("feed {age}s"),
            x + w - INPAD,
            y + 30.0,
            20.0,
            FAINT,
        );
    }
}

fn pending_strip(pm: &mut Pixmap, fonts: &Fonts, x: f32, y: f32, w: f32, what: &str) {
    chrome::text(
        pm,
        &fonts.sans,
        &format!("{what}: waiting for host feed"),
        x,
        y,
        SUB_SIZE,
        FAINT,
    );
    let _ = w;
}

fn pools_row(cx: &mut Cx, x: f32, y: f32, w: f32, h: f32) {
    let Cx { pm, fonts, sys } = cx;
    chrome::rounded_rect(pm, x, y, w, h, 16.0, CARD);
    card_title(pm, fonts, x + INPAD, y + 4.0, "POOLS");
    if sys.pools.is_empty() {
        pending_strip(pm, fonts, x + INPAD, y + 62.0, w - 2.0 * INPAD, "pools");
        return;
    }
    let n = sys.pools.len().max(1) as f32;
    let bw = (w - 2.0 * INPAD) / n;
    for (i, p) in sys.pools.iter().enumerate() {
        let bx = x + INPAD + i as f32 * bw;
        let inner = bw - 24.0;
        let dot = match p.health.as_str() {
            "ONLINE" => GREEN,
            "DEGRADED" => chrome::AMBER,
            _ => RED,
        };
        chrome::circle(pm, bx + 10.0, y + 66.0, 9.0, dot);
        chrome::text(pm, &fonts.sans, &p.name, bx + 30.0, y + 76.0, 34.0, TEXT);
        chrome::text_right(
            pm,
            &fonts.mono,
            &p.health,
            bx + inner,
            y + 76.0,
            24.0,
            MUTED,
        );
        let io = format!(
            "↓ {}   ↑ {}",
            human_bps(p.read_bps * 8.0),
            human_bps(p.write_bps * 8.0)
        );
        chrome::text(pm, &fonts.mono, &io, bx, y + 112.0, 24.0, MUTED);
        let frac = p.util_pct.map(|u| u / 100.0).unwrap_or(0.0);
        chrome::bar(pm, bx, y + 124.0, inner, 12.0, frac, GREEN);
        let mut sub = match p.used_pct {
            Some(u) => format!("used {u:.0}%"),
            None => "used —".to_string(),
        };
        if let Some(t) = p.temp_c {
            sub.push_str(&format!("   ·   {t:.0}°"));
        }
        chrome::text(pm, &fonts.mono, &sub, bx, y + h - INPAD, SUB_SIZE, MUTED);
    }
}
