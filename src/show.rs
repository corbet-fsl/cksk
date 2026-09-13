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
        label: "Übersicht",
    },
    BoardDef {
        id: "public",
        label: "Public",
    },
    BoardDef {
        id: "services",
        label: "Dienste",
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
        "mail" => Some(&["mail", "backups", "clouds", "watchdog"]),
        _ => None,
    }
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
        Show {
            gatus: GatusSource::new(gatus_source),
            sys: SysState::new(),
            fonts: load_fonts(),
        }
    }

    pub fn version(&self) -> u64 {
        self.gatus.version()
    }

    pub fn index_for(now_secs: u64) -> usize {
        (now_secs / DWELL_SECS % BOARDS.len() as u64) as usize
    }

    pub fn render_screen(&self, id: &str, width: u32, height: u32) -> Option<Pixmap> {
        let pos = BOARDS.iter().position(|b| b.id == id)?;
        let eps = self.gatus.snapshot();
        let sys = self.sys.snapshot();
        let (up, down, unknown) = GatusSource::counts(&eps);
        let (clock, date) = now_strings();
        let indicator = format!("{}/{} · {}", pos + 1, BOARDS.len(), BOARDS[pos].label);
        let hdr = Header {
            clock: &clock,
            date: &date,
            total: eps.len(),
            up,
            down,
            unknown,
            indicator: &indicator,
        };

        let mut pm = Pixmap::new(width, height)?;
        match id {
            "overview" => {
                let groups = groups::select_groups(&eps, GROUP_ORDER);
                overview::render_overview(&mut pm, &eps, &self.fonts, &hdr, &groups);
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
        let id = BOARDS[Self::index_for(now_secs)].id;
        self.render_screen(id, width, height)
    }
}
