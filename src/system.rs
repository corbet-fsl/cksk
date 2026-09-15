//! System board: host resources in giant type, read locally without GPU work.
//! Every source is a plain file read (`/proc`, `/sys`): no subprocesses, no perf
//! counters, nothing that wakes the GPU. `rocm-smi`/`amdgpu_top` are deliberately
//! NOT used -- they can pull the card out of its low-power state. Facts the kiosk
//! cannot see itself (host links, pool aggregates) arrive via the optional host
//! feed file instead -- see `HostFeedFile`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::chrome::{self, Fonts, Header, HEADER_H};

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
    /// "up" or "down". Down links render dimmed with no rates: a missing link
    /// must read as broken, never as absent.
    #[serde(default = "net_up")]
    pub state: String,
}

fn net_up() -> String {
    "up".to_string()
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

// ---------- render (grid system) ----------
//
// Three columns (grid cols 0-3 / 4-7 / 8-11): compute, memory, temps.
// Below: one full-span network row, one full-span pools row.
// All positions derive from layout rhythm + type scale; nothing hand-placed.

use crate::layout::{self, BODY, H1, H2, MARGIN, MICRO, SMALL, T3, XS};

fn card_title(pm: &mut tiny_skia::Pixmap, fonts: &Fonts, r: layout::Rect, title: &str) {
    layout::line(pm, &fonts.sans, title, r, r.y + XS, BODY, chrome::ACCENT);
}

fn hist_strip(pm: &mut tiny_skia::Pixmap, x: f32, y: f32, w: f32, h: f32, hist: &[f64]) {
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
            chrome::GREEN,
        );
    }
}

fn vbar(pm: &mut tiny_skia::Pixmap, x: f32, y: f32, w: f32, h: f32, frac: f64, fill: chrome::Rgb) {
    let f = frac.clamp(0.0, 1.0) as f32;
    chrome::fill_rect(pm, x, y, w, h, chrome::HIST_NONE);
    if f > 0.0 {
        chrome::fill_rect(pm, x, y + h * (1.0 - f), w, h * f, fill);
    }
}

/// Human bit-rate: 340 kb/s, 12 Mb/s, 1.2 Gb/s. Below 1 kb/s reads as idle.
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

fn fmt_temp(c: Option<f32>) -> Option<String> {
    c.map(|t| format!("{t:.0}°"))
}

/// Shared render context: seven-argument card functions collapse to five.
struct Cx<'a> {
    pm: &'a mut tiny_skia::Pixmap,
    fonts: &'a Fonts,
    sys: &'a SysSnapshot,
}

pub fn render_system(pm: &mut tiny_skia::Pixmap, sys: &SysSnapshot, fonts: &Fonts, hdr: &Header) {
    let w = pm.width() as f32;
    let h = pm.height() as f32;

    chrome::fill_rect(pm, 0.0, 0.0, w, h, chrome::BG);
    chrome::render_header(pm, fonts, hdr);

    // Vertical budget (1080p): header 112 + main 520 + net 140 + pools 170 + gaps.
    let grid = layout::Rect {
        x: MARGIN,
        y: HEADER_H + layout::S,
        w: w - 2.0 * MARGIN,
        h: h - HEADER_H - layout::S - MARGIN,
    };
    let (main, rest) = grid.take_top(520.0, layout::M);
    let (net, pools) = rest.take_top(140.0, layout::M);

    let cols = main.columns(3);
    let mut cx = Cx { pm, fonts, sys };
    cpu_gpu_column(&mut cx, cols[0]);
    mem_column(&mut cx, cols[1]);
    temps_card(&mut cx, cols[2]);
    net_row(&mut cx, net);
    pools_row(&mut cx, pools);
}

fn cpu_gpu_column(cx: &mut Cx, col: layout::Rect) {
    let (cpu, gpu) = col.take_top(320.0, layout::M);
    cpu_card(cx, cpu);
    gpu_card(cx, gpu);
}

fn mem_column(cx: &mut Cx, col: layout::Rect) {
    let (ram, vram) = col.take_top(320.0, layout::M);
    ram_card(cx, ram);
    vram_card(cx, vram);
}

fn cpu_card(cx: &mut Cx, card: layout::Rect) {
    let Cx { pm, fonts, sys } = cx;
    chrome::rounded_rect(pm, card.x, card.y, card.w, card.h, 16.0, chrome::CARD);
    let inner = card.inset(layout::M);
    card_title(pm, fonts, inner, "CPU");
    let big = match sys.cpu_pct {
        Some(p) => format!("{p:.0}%"),
        None => "—".to_string(),
    };
    let head_top = inner.y + BODY.lh + layout::XS;
    layout::line(pm, &fonts.mono, &big, inner, head_top, H1, chrome::TEXT);
    let load = format!("load {:.2} {:.2} {:.2}", sys.load1, sys.load5, sys.load15);
    layout::line(
        pm,
        &fonts.mono,
        &load,
        inner,
        head_top + H1.lh + layout::XS,
        SMALL,
        chrome::MUTED,
    );
    hist_strip(
        pm,
        inner.x,
        head_top + H1.lh + SMALL.lh + 2.0 * layout::XS,
        inner.w,
        14.0,
        &sys.cpu_hist,
    );
    if !sys.cpu_each.is_empty() {
        let n = sys.cpu_each.len() as f32;
        let bw = inner.w / n;
        let bh = 56.0;
        let by = card.y + card.h - layout::M - bh;
        for (i, pct) in sys.cpu_each.iter().enumerate() {
            vbar(
                pm,
                inner.x + i as f32 * bw,
                by,
                (bw - 2.0).max(1.0),
                bh,
                pct / 100.0,
                chrome::GREEN,
            );
        }
    }
}

fn gpu_card(cx: &mut Cx, card: layout::Rect) {
    let Cx { pm, fonts, sys } = cx;
    chrome::rounded_rect(pm, card.x, card.y, card.w, card.h, 16.0, chrome::CARD);
    let inner = card.inset(layout::M);
    card_title(pm, fonts, inner, "GPU");
    match &sys.gpu {
        Some(g) => {
            let big = format!("GFX {}%", g.busy_pct);
            let head_top = inner.y + BODY.lh + layout::XS;
            layout::line(pm, &fonts.mono, &big, inner, head_top, H1, chrome::TEXT);
            let mut sub = format!("VRAM {:.1} GiB", g.vram_used_gib);
            if let Some(p) = g.power_w {
                sub.push_str(&format!("   ·   {p:.0} W"));
            }
            layout::line(
                pm,
                &fonts.mono,
                &sub,
                inner,
                head_top + H1.lh + layout::XS,
                SMALL,
                chrome::MUTED,
            );
        }
        None => {
            layout::line(
                pm,
                &fonts.sans,
                "no GPU visible",
                inner,
                inner.y + BODY.lh + layout::XS,
                SMALL,
                chrome::FAINT,
            );
        }
    }
}

fn ram_card(cx: &mut Cx, card: layout::Rect) {
    let Cx { pm, fonts, sys } = cx;
    chrome::rounded_rect(pm, card.x, card.y, card.w, card.h, 16.0, chrome::CARD);
    let inner = card.inset(layout::M);
    card_title(pm, fonts, inner, "RAM");
    let big = format!("{:.0} / {:.0} GiB", sys.mem_used_gib, sys.mem_total_gib);
    let head_top = inner.y + BODY.lh + layout::XS;
    layout::line(pm, &fonts.mono, &big, inner, head_top, H1, chrome::TEXT);
    let frac = if sys.mem_total_gib > 0.0 {
        sys.mem_used_gib / sys.mem_total_gib
    } else {
        0.0
    };
    chrome::bar(
        pm,
        inner.x,
        head_top + H1.lh + layout::XS,
        inner.w,
        16.0,
        frac,
        chrome::GREEN,
    );
    // Swap mirrors RAM one-to-one: own title, own headline, own bar.
    let stop = head_top + H1.lh + 16.0 + 2.0 * layout::XS;
    card_title(pm, fonts, layout::Rect { y: stop, ..inner }, "SWAP");
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
        chrome::RED
    } else if swap_frac > 0.7 {
        chrome::AMBER
    } else {
        chrome::GREEN
    };
    let swap_top = stop + BODY.lh + layout::XS;
    layout::line(
        pm,
        &fonts.mono,
        &swap_big,
        inner,
        swap_top,
        H1,
        chrome::TEXT,
    );
    chrome::bar(
        pm,
        inner.x,
        swap_top + H1.lh + layout::XS,
        inner.w,
        16.0,
        swap_frac,
        swap_color,
    );
}

fn vram_card(cx: &mut Cx, card: layout::Rect) {
    let Cx { pm, fonts, sys } = cx;
    chrome::rounded_rect(pm, card.x, card.y, card.w, card.h, 16.0, chrome::CARD);
    let inner = card.inset(layout::M);
    card_title(pm, fonts, inner, "VRAM");
    match &sys.gpu {
        Some(g) => {
            let big = format!("{:.1} / {:.0} GiB", g.vram_used_gib, g.vram_total_gib);
            let head_top = inner.y + BODY.lh + layout::XS;
            layout::line(pm, &fonts.mono, &big, inner, head_top, H1, chrome::TEXT);
            let frac = if g.vram_total_gib > 0.0 {
                g.vram_used_gib / g.vram_total_gib
            } else {
                0.0
            };
            chrome::bar(
                pm,
                inner.x,
                head_top + H1.lh + layout::XS,
                inner.w,
                16.0,
                frac,
                chrome::GREEN,
            );
        }
        None => {
            layout::line(
                pm,
                &fonts.sans,
                "no GPU visible",
                inner,
                inner.y + BODY.lh + layout::XS,
                SMALL,
                chrome::FAINT,
            );
        }
    }
}

fn temps_card(cx: &mut Cx, card: layout::Rect) {
    let Cx { pm, fonts, sys } = cx;
    chrome::rounded_rect(pm, card.x, card.y, card.w, card.h, 16.0, chrome::CARD);
    let inner = card.inset(layout::M);
    card_title(pm, fonts, inner, "TEMPS");
    let mut top = inner.y + BODY.lh + layout::XS;
    let step = T3.lh + layout::XS;
    let mut row = |label: &str, value: Option<String>| {
        layout::line(pm, &fonts.sans, label, inner, top, T3, chrome::MUTED);
        layout::line_right(
            pm,
            &fonts.mono,
            &value.unwrap_or_else(|| "—".to_string()),
            inner,
            top,
            T3,
            chrome::TEXT,
        );
        top += step;
    };
    row("CPU", fmt_temp(sys.cpu_temp_c));
    for (i, t) in sys.cpu_ccd.iter().enumerate() {
        row(&format!("CPU CCD{}", i + 1), fmt_temp(Some(*t)));
    }
    if let Some(g) = &sys.gpu {
        row("GPU edge", fmt_temp(g.edge_c));
        row("GPU junction", fmt_temp(g.junction_c));
        row("GPU mem", fmt_temp(g.mem_c));
    }
    // Pool temps live in-lane on the pools row (used % · temp); duplicating them
    // here would overflow the card once a third lane exists.
    layout::line(
        pm,
        &fonts.mono,
        &format!("up {}", format_uptime(sys.uptime_s)),
        inner,
        card.y + card.h - layout::M - SMALL.lh,
        SMALL,
        chrome::MUTED,
    );
}

fn net_row(cx: &mut Cx, row: layout::Rect) {
    let Cx { pm, fonts, sys } = cx;
    chrome::rounded_rect(pm, row.x, row.y, row.w, row.h, 16.0, chrome::CARD);
    let inner = row.inset(layout::M);
    // Side label gutter: title vertically centered, feed age beneath it.
    // Blocks take the remaining width at full inner height.
    let gutter = 140.0;
    let mid = inner.y + inner.h / 2.0;
    layout::line(
        pm,
        &fonts.sans,
        "NET",
        inner,
        layout::mid_baseline(&fonts.sans, mid - MICRO.lh, BODY.size, 'H'),
        BODY,
        chrome::ACCENT,
    );
    if let Some(age) = sys.feed_age_secs {
        // Gutter-owned, never over block content (br0's pct lives top-right).
        layout::line(
            pm,
            &fonts.mono,
            &format!("feed {age}s"),
            inner,
            mid + layout::XS,
            MICRO,
            chrome::FAINT,
        );
    }
    let blocks = layout::Rect {
        x: inner.x + gutter,
        w: inner.w - gutter,
        ..inner
    }
    .columns(sys.net.len().max(1));
    if sys.net.is_empty() {
        layout::line(
            pm,
            &fonts.sans,
            "network: waiting for host feed",
            blocks[0],
            layout::mid_baseline(&fonts.sans, mid, BODY.size, 'H'),
            BODY,
            chrome::FAINT,
        );
        return;
    }
    for (nic, b) in sys.net.iter().zip(blocks.iter()) {
        if nic.state != "up" {
            // Down is data: dimmed name plus the word, same geometry as a live block.
            layout::line(pm, &fonts.sans, &nic.iface, *b, b.y, BODY, chrome::FAINT);
            layout::line(
                pm,
                &fonts.mono,
                "down",
                *b,
                b.y + BODY.lh + layout::XS,
                SMALL,
                chrome::RED,
            );
            continue;
        }
        layout::line(pm, &fonts.sans, &nic.iface, *b, b.y, BODY, chrome::ACCENT);
        let rates = format!("↓ {}   ↑ {}", human_bps(nic.rx_bps), human_bps(nic.tx_bps));
        layout::line(
            pm,
            &fonts.mono,
            &rates,
            *b,
            b.y + BODY.lh + layout::XS,
            SMALL,
            chrome::MUTED,
        );
        let peak = nic.rx_bps.max(nic.tx_bps);
        if let Some(speed) = nic.speed_bps {
            if speed > 0.0 {
                chrome::bar(
                    pm,
                    b.x,
                    b.y + b.h - 10.0,
                    b.w - layout::S,
                    10.0,
                    peak / speed,
                    chrome::GREEN,
                );
                layout::line_right(
                    pm,
                    &fonts.mono,
                    &format!("{:.1}%", peak / speed * 100.0),
                    *b,
                    b.y,
                    SMALL,
                    chrome::MUTED,
                );
            }
        }
    }
}

fn pools_row(cx: &mut Cx, row: layout::Rect) {
    let Cx { pm, fonts, sys } = cx;
    chrome::rounded_rect(pm, row.x, row.y, row.w, row.h, 16.0, chrome::CARD);
    let inner = row.inset(layout::M);
    // Side label gutter like the network row; halves take the rest at full height.
    let gutter = 140.0;
    let mid = inner.y + inner.h / 2.0;
    layout::line(
        pm,
        &fonts.sans,
        "POOLS",
        inner,
        layout::mid_baseline(&fonts.sans, mid, BODY.size, 'H'),
        BODY,
        chrome::ACCENT,
    );
    if sys.pools.is_empty() {
        layout::line(
            pm,
            &fonts.sans,
            "pools: waiting for host feed",
            layout::Rect {
                x: inner.x + gutter,
                w: inner.w - gutter,
                ..inner
            },
            layout::mid_baseline(&fonts.sans, mid, BODY.size, 'H'),
            BODY,
            chrome::FAINT,
        );
        return;
    }
    let halves = layout::Rect {
        x: inner.x + gutter,
        w: inner.w - gutter,
        ..inner
    }
    .columns(sys.pools.len().max(1));
    for (p, b) in sys.pools.iter().zip(halves.iter()) {
        let dot = match p.health.as_str() {
            "ONLINE" => chrome::GREEN,
            // UNKNOWN is not failure: amber, never red. Only a stated bad
            // state (DEGRADED excepted as caution, everything else red) alarms.
            "DEGRADED" | "UNKNOWN" => chrome::AMBER,
            _ => chrome::RED,
        };
        layout::dot(
            pm,
            b.x + 10.0,
            b.y,
            layout::Face {
                font: &fonts.sans,
                size: H2.size,
                sample: 'H',
            },
            9.0,
            dot,
        );
        let name_rect = layout::Rect {
            x: b.x + 30.0,
            y: b.y,
            w: b.w - 30.0,
            h: b.h,
        };
        layout::line(pm, &fonts.sans, &p.name, name_rect, b.y, H2, chrome::TEXT);
        layout::line_right(pm, &fonts.mono, &p.health, *b, b.y, SMALL, chrome::MUTED);
        let io = format!(
            "↓ {}   ↑ {}",
            human_bps(p.read_bps * 8.0),
            human_bps(p.write_bps * 8.0)
        );
        layout::line(
            pm,
            &fonts.mono,
            &io,
            *b,
            b.y + H2.lh + layout::XS,
            SMALL,
            chrome::MUTED,
        );
        let frac = p.util_pct.map(|u| u / 100.0).unwrap_or(0.0);
        chrome::bar(
            pm,
            b.x,
            b.y + H2.lh + SMALL.lh + 2.0 * layout::XS,
            b.w - layout::M,
            12.0,
            frac,
            chrome::GREEN,
        );
        let mut sub = match p.used_pct {
            Some(u) => format!("used {u:.0}%"),
            None => "used —".to_string(),
        };
        if let Some(t) = p.temp_c {
            sub.push_str(&format!("   ·   {t:.0}°"));
        }
        layout::line(
            pm,
            &fonts.mono,
            &sub,
            *b,
            b.y + b.h - SMALL.lh,
            SMALL,
            chrome::MUTED,
        );
    }
}
