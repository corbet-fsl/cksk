//! The kiosk show: every board the feed warrants, one on screen for DWELL_SECS.
//! The header (corbet status + clock) is identical everywhere; only the body rotates.
//!
//! Boards are derived from the feed, not from a fixed list of group names: the
//! detail pages are packed from whatever groups the source returns (canonical
//! order first, unknown groups after it), so a group can never silently vanish
//! from the show when the estate grows a new one.

use tiny_skia::Pixmap;

use crate::chrome::{load_fonts, Fonts, Header};
use crate::gatus::GatusSource;
use crate::groups::{self, ROWS_PER_COLUMN};
use crate::model::Endpoint;
use crate::overview::{self, GROUP_ORDER};
use crate::system::{self, SysState};

/// Seconds each board stays on screen. Hard cut, no transition frames.
pub const DWELL_SECS: u64 = 30;

/// Columns a detail page may use. Three 12-row columns fill a 1080p screen.
const PAGE_COLS: usize = 3;

/// One detail page: a set of groups (or group continuations) that fit one screen.
pub struct Page {
    pub label: String,
    pub groups: Vec<(String, Vec<usize>)>,
}

impl Page {
    fn downs(&self, eps: &[Endpoint]) -> usize {
        self.groups
            .iter()
            .flat_map(|(_, idxs)| idxs)
            .filter(|&&ei| eps[ei].up() == Some(false))
            .count()
    }
}

/// Display label for a page: a page dominated by one group is named after it; a
/// mixed page that carries the mail rows keeps the mail cluster's name; anything
/// else is "Services".
fn page_label(groups: &[(String, Vec<usize>)]) -> String {
    if groups.len() == 1 {
        return group_label(&groups[0].0);
    }
    let total: usize = groups.iter().map(|(_, idxs)| idxs.len()).sum();
    if let Some((g, idxs)) = groups.iter().max_by_key(|(_, idxs)| idxs.len()) {
        if idxs.len() * 5 >= total * 3 {
            return group_label(g);
        }
    }
    if groups.iter().any(|(g, _)| g == "mail") {
        return "Mail & Backup".to_string();
    }
    "Services".to_string()
}

fn group_label(group: &str) -> String {
    match group {
        "mail" => "Mail & Backup".to_string(),
        "certificates" => "Certificates".to_string(),
        "ci" => "CI".to_string(),
        "github" => "GitHub".to_string(),
        "seaweedfs" => "SeaweedFS".to_string(),
        "backups" => "Backups".to_string(),
        "caches" => "Caches".to_string(),
        "nodes" => "Nodes".to_string(),
        "clouds" => "Clouds".to_string(),
        "watchdog" => "Watchdog".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => "Services".to_string(),
            }
        }
    }
}

/// Canonical order first, then any group the source grew after it (first-seen order).
fn ordered_groups(eps: &[Endpoint]) -> Vec<(String, Vec<usize>)> {
    let mut all = crate::model::grouped(eps);
    all.sort_by_key(|(name, _)| {
        GROUP_ORDER
            .iter()
            .position(|g| g == name)
            .unwrap_or(GROUP_ORDER.len())
    });
    all
}

/// Detail pages for the current feed: chunks (≤12 rows each) are added to the open
/// page while they still fit the 1080p band, then a new page starts. A single group
/// larger than one page is split, so no group is ever unplaceable.
pub fn detail_pages(eps: &[Endpoint], height: f32) -> Vec<Page> {
    let (top, bottom) = groups::band(height);
    let mut pages: Vec<Page> = Vec::new();
    let mut cur: Vec<(String, Vec<usize>)> = Vec::new();
    for (gname, idxs) in ordered_groups(eps) {
        let rows = groups::rows_per_column(height);
        for part in groups::chunk_group(&idxs, rows) {
            let mut trial = cur.clone();
            match trial.last_mut() {
                Some((g, acc)) if *g == gname => acc.extend_from_slice(&part),
                _ => trial.push((gname.clone(), part.clone())),
            }
            let fits = groups::pack(&groups::chunks_of(&trial, rows), PAGE_COLS, top, bottom)
                .1
                .is_empty();
            if cur.is_empty() || fits {
                cur = trial;
            } else {
                pages.push(Page {
                    label: page_label(&cur),
                    groups: std::mem::take(&mut cur),
                });
                cur.push((gname.clone(), part));
            }
        }
    }
    if !cur.is_empty() {
        pages.push(Page {
            label: page_label(&cur),
            groups: cur,
        });
    }
    pages
}

fn now_strings() -> (String, String) {
    let now = chrono::Local::now();
    (
        now.format("%H:%M:%S").to_string(),
        now.format("%a %d %b %Y").to_string(),
    )
}

fn header<'a>(eps: &[Endpoint], indicator: &'a str, clock: &'a str, date: &'a str) -> Header<'a> {
    let (up, down, unknown) = GatusSource::counts(eps);
    Header {
        clock,
        date,
        total: eps.len(),
        up,
        down,
        unknown,
        indicator,
    }
}

/// What render_current picked for this moment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Screen {
    Overview,
    Detail(usize),
    System,
}

/// Strict-adaptive playlist for this moment:
/// - any troubled detail page: overview, then only the troubled pages, then system --
///   trouble pins the screen until it recovers;
/// - all green: a fixed 150s window of overview/system/overview/system plus one rotating
///   detail page as proof-of-life, so every page is seen at least every few minutes.
fn playlist(pages: &[Page], eps: &[Endpoint], now_secs: u64) -> Vec<Screen> {
    // No source configured (or reachable): the system board needs nothing but the
    // machine itself, so a fresh cksk shows something useful instead of an empty
    // overview. This is what makes the system board the shipped default.
    if eps.is_empty() {
        return vec![Screen::System];
    }
    let troubled: Vec<usize> = (0..pages.len())
        .filter(|&i| pages[i].downs(eps) > 0)
        .collect();
    if !troubled.is_empty() {
        let mut plan = vec![Screen::Overview];
        plan.extend(troubled.into_iter().map(Screen::Detail));
        plan.push(Screen::System);
        return plan;
    }
    if pages.is_empty() {
        return vec![Screen::Overview, Screen::System];
    }
    let detail = (now_secs / 150 % pages.len() as u64) as usize;
    vec![
        Screen::Overview,
        Screen::System,
        Screen::Overview,
        Screen::System,
        Screen::Detail(detail),
    ]
}

pub struct Show {
    gatus: GatusSource,
    sys: SysState,
    fonts: Fonts,
}

impl Show {
    pub fn new(gatus_source: String) -> Self {
        Self::with_feed(gatus_source, None)
    }

    pub fn with_feed(gatus_source: String, host_feed: Option<String>) -> Self {
        Show {
            gatus: GatusSource::new(gatus_source),
            sys: SysState::with_feed(host_feed),
            fonts: load_fonts(),
        }
    }

    pub fn version(&self) -> u64 {
        self.gatus.version()
    }

    /// Detail pages for the current feed at this geometry (used by --dump and tests).
    pub fn pages(&self, height: u32) -> Vec<Page> {
        detail_pages(&self.gatus.snapshot(), height as f32)
    }

    /// Render one named board: `overview`, `system`, or `page-<n>` (1-based).
    /// Every page is addressable so a preview can prove what the show contains.
    pub fn render_named(&self, id: &str, width: u32, height: u32) -> Result<Pixmap, String> {
        if id == "overview" {
            return self
                .render_overview(width, height, "1/1 · Overview")
                .ok_or_else(|| "empty geometry".to_string());
        }
        if id == "system" {
            return self
                .render_system(width, height, "1/1 · System")
                .ok_or_else(|| "empty geometry".to_string());
        }
        let idx = id
            .strip_prefix("page-")
            .and_then(|n| n.parse::<usize>().ok())
            .ok_or_else(|| format!("unknown board {id}"))?;
        let pages = detail_pages(&self.gatus.snapshot(), height as f32);
        let page = pages
            .get(idx.saturating_sub(1))
            .ok_or_else(|| format!("page {idx} does not exist ({}/{})", idx, pages.len()))?;
        let indicator = format!("{}/{} · {}", idx, pages.len(), page.label);
        self.render_detail(page, width, height, &indicator)
            .ok_or_else(|| "empty geometry".to_string())
    }

    fn render_overview(&self, width: u32, height: u32, indicator: &str) -> Option<Pixmap> {
        let eps = self.gatus.snapshot();
        let (clock, date) = now_strings();
        let hdr = header(&eps, indicator, &clock, &date);
        let groups = ordered_groups(&eps);
        let mut pm = Pixmap::new(width, height)?;
        let age = self.gatus.data_age_secs();
        overview::render_overview(&mut pm, &eps, &self.fonts, &hdr, &groups, age);
        Some(pm)
    }

    fn render_system(&self, width: u32, height: u32, indicator: &str) -> Option<Pixmap> {
        let eps = self.gatus.snapshot();
        let (clock, date) = now_strings();
        let hdr = header(&eps, indicator, &clock, &date);
        let sys = self.sys.snapshot();
        let mut pm = Pixmap::new(width, height)?;
        system::render_system(&mut pm, &sys, &self.fonts, &hdr);
        Some(pm)
    }

    fn render_detail(
        &self,
        page: &Page,
        width: u32,
        height: u32,
        indicator: &str,
    ) -> Option<Pixmap> {
        let eps = self.gatus.snapshot();
        let (clock, date) = now_strings();
        let hdr = header(&eps, indicator, &clock, &date);
        let mut pm = Pixmap::new(width, height)?;
        let nchunks: usize = page
            .groups
            .iter()
            .map(|(_, idxs)| idxs.len().div_ceil(ROWS_PER_COLUMN).max(1))
            .sum();
        groups::render_groups(
            &mut pm,
            &eps,
            &self.fonts,
            &hdr,
            &page.groups,
            board_cols(nchunks),
        );
        Some(pm)
    }

    pub fn render_current(&self, width: u32, height: u32, now_secs: u64) -> Option<Pixmap> {
        let eps = self.gatus.snapshot();
        let pages = detail_pages(&eps, height as f32);
        let plan = playlist(&pages, &eps, now_secs);
        let pos = (now_secs / DWELL_SECS % plan.len() as u64) as usize;
        let indicator = format!("{}/{}", pos + 1, plan.len());
        match plan[pos] {
            Screen::Overview => {
                self.render_overview(width, height, &format!("{indicator} · Overview"))
            }
            Screen::System => self.render_system(width, height, &format!("{indicator} · System")),
            Screen::Detail(i) => {
                let page = pages.get(i)?;
                let label = format!("{indicator} · {}", page.label);
                self.render_detail(page, width, height, &label)
            }
        }
    }
}

/// Columns from the chunk count: every chunk wants its own column, clamped to a
/// sane 2..=3 so single-group boards never render one lost narrow column.
fn board_cols(nchunks: usize) -> usize {
    nchunks.clamp(2, PAGE_COLS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Endpoint, GResult};

    fn ep(name: &str, group: &str, ok: Option<bool>) -> Endpoint {
        Endpoint {
            name: name.to_string(),
            group: group.to_string(),
            key: format!("{group}/{name}"),
            results: match ok {
                None => Vec::new(),
                Some(ok) => vec![GResult {
                    success: ok,
                    duration: 1_000_000,
                    status: Some(200),
                }],
            },
        }
    }

    /// The live source's groups and row counts on 2026-11-08 (79 rows, 10 groups).
    fn live_estate() -> Vec<Endpoint> {
        let counts = [
            ("mail", 9),
            ("certificates", 30),
            ("ci", 3),
            ("nodes", 2),
            ("caches", 2),
            ("github", 10),
            ("seaweedfs", 7),
            ("backups", 13),
            ("clouds", 1),
            ("watchdog", 2),
        ];
        let mut eps = Vec::new();
        for (group, n) in counts {
            for i in 0..n {
                eps.push(ep(&format!("{group}-{i}"), group, Some(true)));
            }
        }
        eps
    }

    #[test]
    fn every_live_endpoint_appears_on_a_detail_page() {
        let eps = live_estate();
        assert_eq!(eps.len(), 79);
        let pages = detail_pages(&eps, 1080.0);
        let mut seen = vec![false; eps.len()];
        for page in &pages {
            for (_, idxs) in &page.groups {
                for &i in idxs {
                    seen[i] = true;
                }
            }
        }
        assert!(
            seen.iter().all(|&s| s),
            "{} endpoint(s) are on no page",
            seen.iter().filter(|&&s| !s).count()
        );
        // And each page is renderable without dropping a chunk.
        let (top, bottom) = groups::band(1080.0);
        for page in &pages {
            let chunks = groups::chunks_of(&page.groups, groups::rows_per_column(1080.0));
            let (_, dropped) = groups::pack(&chunks, PAGE_COLS, top, bottom);
            assert!(dropped.is_empty(), "page {} drops a chunk", page.label);
        }
    }

    #[test]
    fn a_new_group_still_reaches_a_page() {
        let mut eps = live_estate();
        eps.push(ep("brandnew", "brandnew-group", None));
        let pages = detail_pages(&eps, 1080.0);
        let covered = pages
            .iter()
            .flat_map(|p| p.groups.iter())
            .any(|(g, _)| g == "brandnew-group");
        assert!(covered, "unknown group vanished from the show");
    }

    #[test]
    fn a_shorter_kiosk_gets_more_pages_not_dropped_rows() {
        let eps = live_estate();
        let pages = detail_pages(&eps, 720.0);
        let covered: usize = pages
            .iter()
            .map(|p| p.groups.iter().map(|(_, i)| i.len()).sum::<usize>())
            .sum();
        assert_eq!(covered, eps.len(), "720p lost rows instead of adding pages");
        let (top, bottom) = groups::band(720.0);
        for page in &pages {
            let chunks = groups::chunks_of(&page.groups, groups::rows_per_column(720.0));
            let (_, dropped) = groups::pack(&chunks, PAGE_COLS, top, bottom);
            assert!(dropped.is_empty(), "720p page {} drops a chunk", page.label);
        }
    }

    #[test]
    fn all_green_rotates_core_plus_heartbeat() {
        let eps = live_estate();
        let pages = detail_pages(&eps, 1080.0);
        assert_eq!(pages.len(), 3);
        assert_eq!(
            playlist(&pages, &eps, 0),
            vec![
                Screen::Overview,
                Screen::System,
                Screen::Overview,
                Screen::System,
                Screen::Detail(0)
            ]
        );
        assert_eq!(playlist(&pages, &eps, 150)[4], Screen::Detail(1));
        assert_eq!(playlist(&pages, &eps, 300)[4], Screen::Detail(2));
        assert_eq!(playlist(&pages, &eps, 450)[4], Screen::Detail(0));
    }

    #[test]
    fn troubled_page_pins_until_recovered() {
        let mut eps = live_estate();
        // a down certificate lives on the certificates page
        let i = eps.iter().position(|e| e.name == "certificates-3").unwrap();
        eps[i].results[0].success = false;
        let pages = detail_pages(&eps, 1080.0);
        let plan = playlist(&pages, &eps, 0);
        assert_eq!(plan[0], Screen::Overview);
        assert_eq!(plan.last(), Some(&Screen::System));
        assert_eq!(plan.len(), 3);
        assert!(matches!(plan[1], Screen::Detail(_)));
    }

    #[test]
    fn empty_source_shows_system_only() {
        let eps: Vec<Endpoint> = Vec::new();
        assert_eq!(playlist(&[], &eps, 0), vec![Screen::System]);
    }
}
