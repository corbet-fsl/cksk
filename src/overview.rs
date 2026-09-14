//! Overview board: the whole estate at a glance in giant type.
//! One big verdict, then one row per group. Detail lives on the group boards.

use tiny_skia::Pixmap;

use crate::chrome::{self, Fonts, Header, GREEN, HEADER_H, MUTED, PAD, RED, TEXT};
use crate::model::Endpoint;

/// Canonical group order: most customer-visible first, housekeeping last.
pub const GROUP_ORDER: &[&str] = &[
    "public",
    "internal",
    "platform",
    "mail",
    "backups",
    "l4-databases",
    "clouds",
    "watchdog",
];

const VERDICT_SIZE: f32 = 100.0;
const SUBLINE_SIZE: f32 = 30.0;
// Tall rows fill the 1080p screen: verdict block ~158 + 8 rows fill to the bottom pad.
const ROW_H: f32 = 92.0;
const ROW_NAME_SIZE: f32 = 38.0;
const ROW_COUNT_SIZE: f32 = 32.0;

pub fn render_overview(
    pm: &mut Pixmap,
    eps: &[Endpoint],
    fonts: &Fonts,
    hdr: &Header,
    groups: &[(String, Vec<usize>)],
    data_age_secs: Option<u64>,
) {
    let w = pm.width() as f32;
    let h = pm.height() as f32;
    let pad = PAD;

    chrome::fill_rect(pm, 0.0, 0.0, w, h, chrome::BG);
    chrome::render_header(pm, fonts, hdr);

    let mut y = HEADER_H + 28.0;

    // ---- verdict ----
    if hdr.down == 0 {
        chrome::text(
            pm,
            &fonts.sans,
            "ALL OK",
            pad,
            y + 78.0,
            VERDICT_SIZE,
            GREEN,
        );
        let mut sub = format!(
            "{}/{} services up   ·   {} idle",
            hdr.up, hdr.total, hdr.unknown
        );
        let mut sub_color = MUTED;
        if let Some(age) = data_age_secs {
            if age > 90 {
                // Stale green is worse than no green: say so on the screen.
                sub.push_str(&format!("   ·   data {age}s old"));
                sub_color = chrome::AMBER;
            }
        }
        chrome::text(
            pm,
            &fonts.sans,
            &sub,
            pad,
            y + 122.0,
            SUBLINE_SIZE,
            sub_color,
        );
    } else {
        let v = format!(
            "{} OUTAGE{}",
            hdr.down,
            if hdr.down == 1 { "" } else { "S" }
        );
        chrome::text(pm, &fonts.sans, &v, pad, y + 78.0, VERDICT_SIZE, RED);
        let downs: Vec<&str> = eps
            .iter()
            .filter(|e| e.up() == Some(false))
            .map(|e| e.name.as_str())
            .collect();
        let joined = downs.join(", ");
        let max_w = w - 2.0 * pad;
        let names = chrome::truncate(&fonts.sans, &joined, SUBLINE_SIZE, max_w);
        chrome::text(pm, &fonts.sans, &names, pad, y + 122.0, SUBLINE_SIZE, RED);
    }
    y += 158.0;

    // ---- one row per group ----
    for (gname, idxs) in groups {
        if y + ROW_H > h - pad {
            break;
        }
        let up = idxs
            .iter()
            .filter(|&&ei| eps[ei].up() == Some(true))
            .count();
        let down = idxs
            .iter()
            .filter(|&&ei| eps[ei].up() == Some(false))
            .count();
        let dot = if down > 0 { RED } else { GREEN };
        chrome::circle(pm, pad + 12.0, y + 40.0, 10.0, dot);

        let title = gname.to_uppercase();
        chrome::text(
            pm,
            &fonts.sans,
            &title,
            pad + 36.0,
            y + 58.0,
            ROW_NAME_SIZE,
            TEXT,
        );

        let stat = if down > 0 {
            format!("{down} DOWN")
        } else {
            format!("{up}/{0} OK", idxs.len())
        };
        let stat_c = if down > 0 { RED } else { MUTED };
        chrome::text_right(
            pm,
            &fonts.mono,
            &stat,
            w - pad,
            y + 58.0,
            ROW_COUNT_SIZE,
            stat_c,
        );
        chrome::fill_rect(
            pm,
            pad,
            y + ROW_H - 2.0,
            w - 2.0 * pad,
            2.0,
            chrome::CARD_LINE,
        );
        y += ROW_H;
    }
}
