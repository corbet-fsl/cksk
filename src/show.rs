//! The kiosk show: ordered boards, one on screen for DWELL_SECS each.
//! The header (corbet status + clock) is identical everywhere; only the body rotates.

use tiny_skia::Pixmap;

use crate::chrome::{load_fonts, Fonts, Header};
use crate::gatus::GatusSource;
use crate::groups::{self, ROWS_PER_COLUMN};
use crate::overview::{self, GROUP_ORDER};
use crate::system::{self, SysState};

/// Seconds each board stays on screen. Hard cut, no transition frames.
pub const DWELL_SECS: u64 = 30;

pub struct BoardDef {
    pub id: &'static str,
    pub label: &'static str,
}

pub const BOARDS: &[BoardDef] = &[
    BoardDef {
        id: "overview",
        label: "Overview",
    },
    BoardDef {
        id: "public",
        label: "Public",
    },
    BoardDef {
        id: "services",
        label: "Services",
    },
    BoardDef {
        id: "mail",
        label: "Mail & Backup",
    },
    BoardDef {
        id: "system",
        label: "System",
    },
];

fn board_groups(id: &str) -> Option<&'static [&'static str]> {
    match id {
        "public" => Some(&["public"]),
        "services" => Some(&["internal", "platform", "l4-databases"]),
        // clouds (1) and watchdog (1) live on the overview as counts only; their own
        // cards would be two one-row cards in a third column of air.
        "mail" => Some(&["mail", "backups"]),
        _ => None,
    }
}

/// Detail boards, in heartbeat rotation order.
const SERVICE_BOARDS: &[&str] = &["public", "services", "mail"];

/// Downs on one detail board.
fn board_downs(eps: &[crate::model::Endpoint], id: &str) -> usize {
    let want = match board_groups(id) {
        Some(w) => w,
        None => return 0,
    };
    eps.iter()
        .filter(|e| want.contains(&e.group.as_str()) && e.up() == Some(false))
        .count()
}

/// Strict-adaptive playlist for this moment:
/// - any troubled detail board: overview, then only the troubled boards, then system --
///   trouble pins the screen until it recovers;
/// - all green: a fixed 150s window of overview/system/overview/system plus one rotating
///   detail board as proof-of-life (each detail board every ~7.5min), never all three.
fn playlist(eps: &[crate::model::Endpoint], now_secs: u64) -> Vec<&'static str> {
    // No source configured (or reachable): the system board needs nothing but the
    // machine itself, so a fresh cksk shows something useful instead of an empty
    // overview. This is what makes the system board the shipped default.
    if eps.is_empty() {
        return vec!["system"];
    }
    let troubled: Vec<&'static str> = SERVICE_BOARDS
        .iter()
        .copied()
        .filter(|id| board_downs(eps, id) > 0)
        .collect();
    if !troubled.is_empty() {
        let mut plan = vec!["overview"];
        plan.extend(troubled);
        plan.push("system");
        return plan;
    }
    let detail = SERVICE_BOARDS[(now_secs / 150 % SERVICE_BOARDS.len() as u64) as usize];
    vec!["overview", "system", "overview", "system", detail]
}

/// Columns from the chunk count: every chunk wants its own column, clamped to a
/// sane 2..=3 so single-group boards never render one lost narrow column.
fn board_cols(nchunks: usize) -> usize {
    nchunks.clamp(2, 3)
}

fn now_strings() -> (String, String) {
    let now = chrono::Local::now();
    (
        now.format("%H:%M:%S").to_string(),
        now.format("%a %d %b %Y").to_string(),
    )
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

    pub fn render_screen(&self, id: &str, width: u32, height: u32) -> Option<Pixmap> {
        // Deterministic position for previews: --dump always addresses the full board list.
        let pos = BOARDS.iter().position(|b| b.id == id)?;
        let indicator = format!("{}/{} · {}", pos + 1, BOARDS.len(), BOARDS[pos].label);
        self.render_at(id, width, height, &indicator)
    }

    fn render_at(&self, id: &str, width: u32, height: u32, indicator: &str) -> Option<Pixmap> {
        if !BOARDS.iter().any(|b| b.id == id) {
            return None;
        }
        let eps = self.gatus.snapshot();
        let sys = self.sys.snapshot();
        let (up, down, unknown) = GatusSource::counts(&eps);
        let (clock, date) = now_strings();
        let hdr = Header {
            clock: &clock,
            date: &date,
            total: eps.len(),
            up,
            down,
            unknown,
            indicator,
        };

        let mut pm = Pixmap::new(width, height)?;
        match id {
            "overview" => {
                let groups = groups::select_groups(&eps, GROUP_ORDER);
                let age = self.gatus.data_age_secs();
                overview::render_overview(&mut pm, &eps, &self.fonts, &hdr, &groups, age);
            }
            "system" => {
                system::render_system(&mut pm, &sys, &self.fonts, &hdr);
            }
            _ => {
                let want = board_groups(id)?;
                let groups = groups::select_groups(&eps, want);
                let nchunks: usize = groups
                    .iter()
                    .map(|(_, idxs)| idxs.len().div_ceil(ROWS_PER_COLUMN).max(1))
                    .sum();
                groups::render_groups(
                    &mut pm,
                    &eps,
                    &self.fonts,
                    &hdr,
                    &groups,
                    board_cols(nchunks),
                );
            }
        }
        Some(pm)
    }

    pub fn render_current(&self, width: u32, height: u32, now_secs: u64) -> Option<Pixmap> {
        // Snapshot once for the playlist decision; render_at snapshots again for
        // the frame itself. Two cheap clones per frame, no shared-borrow puzzles.
        // The indicator names the live position: adaptive playlists skip healthy
        // boards, so a static N/5 would lie about where the show is.
        let plan = playlist(&self.gatus.snapshot(), now_secs);
        let pos = (now_secs / DWELL_SECS % plan.len() as u64) as usize;
        let id = plan[pos];
        let label = BOARDS.iter().find(|b| b.id == id).map(|b| b.label)?;
        let indicator = format!("{}/{} · {}", pos + 1, plan.len(), label);
        self.render_at(id, width, height, &indicator)
    }
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

    #[test]
    fn all_green_rotates_core_plus_heartbeat() {
        let eps = vec![
            ep("a", "public", Some(true)),
            ep("b", "internal", Some(true)),
            ep("c", "mail", Some(true)),
        ];
        // Window 0 (now 0..150): heartbeat detail is public.
        assert_eq!(
            playlist(&eps, 0),
            vec!["overview", "system", "overview", "system", "public"]
        );
        assert_eq!(
            playlist(&eps, 120),
            vec!["overview", "system", "overview", "system", "public"]
        );
        // Window 1 (150..300): heartbeat moves to services; window 2: mail.
        assert_eq!(playlist(&eps, 150)[4], "services");
        assert_eq!(playlist(&eps, 300)[4], "mail");
        assert_eq!(playlist(&eps, 450)[4], "public");
    }

    #[test]
    fn troubled_board_pins_until_recovered() {
        let eps = vec![
            ep("a", "public", Some(false)),
            ep("b", "internal", Some(true)),
        ];
        assert_eq!(playlist(&eps, 0), vec!["overview", "public", "system"]);
        // Healthy detail boards vanish entirely while trouble persists.
        assert!(!playlist(&eps, 9999).contains(&"services"));
        assert!(!playlist(&eps, 9999).contains(&"mail"));
    }

    #[test]
    fn empty_source_shows_system_only() {
        let eps: Vec<crate::model::Endpoint> = Vec::new();
        assert_eq!(playlist(&eps, 0), vec!["system"]);
        assert_eq!(playlist(&eps, 9999), vec!["system"]);
    }
}
