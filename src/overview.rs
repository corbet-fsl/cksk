//! Overview board: the whole estate at a glance in giant type.
//! One big verdict, then one row per group. Detail lives on the group boards.
//! Geometry comes from the shared grid (layout.rs): rows, type and dots match
//! every other board by construction.

use tiny_skia::Pixmap;

use crate::chrome::{self, Fonts, Header, HEADER_H, PAD};
use crate::layout::{self, D1, H2, MARGIN, T3};
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

const ROW_H: f32 = 92.0;

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

    chrome::fill_rect(pm, 0.0, 0.0, w, h, chrome::BG);
    chrome::render_header(pm, fonts, hdr);

    let grid = layout::Rect {
        x: MARGIN,
        y: 0.0,
        w: w - 2.0 * MARGIN,
        h,
    };
    let mut y = HEADER_H + 28.0;

    // ---- verdict ----
    if hdr.down == 0 {
        layout::line(pm, &fonts.sans, "ALL OK", grid, y, D1, chrome::GREEN);
        let mut sub = format!(
            "{}/{} services up   ·   {} idle",
            hdr.up, hdr.total, hdr.unknown
        );
        let mut sub_color = chrome::MUTED;
        if let Some(age) = data_age_secs {
            if age > 90 {
                // Stale green is worse than no green: say so on the screen.
                sub.push_str(&format!("   ·   data {age}s old"));
                sub_color = chrome::AMBER;
            }
        }
        layout::line(
            pm,
            &fonts.sans,
            &sub,
            grid,
            y + D1.lh + layout::XS,
            T3,
            sub_color,
        );
    } else {
        let v = format!(
            "{} OUTAGE{}",
            hdr.down,
            if hdr.down == 1 { "" } else { "S" }
        );
        layout::line(pm, &fonts.sans, &v, grid, y, D1, chrome::RED);
        let downs: Vec<&str> = eps
            .iter()
            .filter(|e| e.up() == Some(false))
            .map(|e| e.name.as_str())
            .collect();
        let joined = downs.join(", ");
        layout::line(
            pm,
            &fonts.sans,
            &joined,
            grid,
            y + D1.lh + layout::XS,
            T3,
            chrome::RED,
        );
    }
    y += D1.lh + T3.lh + 2.0 * layout::XS + layout::M;

    // ---- one row per group ----
    for (gname, idxs) in groups {
        if y + ROW_H > h - PAD {
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
        let dot = if down > 0 { chrome::RED } else { chrome::GREEN };
        let row = grid.at_y(y, ROW_H);
        // Name block: dot + title share one line box, dot on the measured cap-center.
        let name_top = y + (ROW_H - H2.lh) / 2.0;
        layout::dot(
            pm,
            PAD + 12.0,
            name_top,
            layout::Face {
                font: &fonts.sans,
                size: H2.size,
                sample: 'H',
            },
            10.0,
            dot,
        );
        layout::line(
            pm,
            &fonts.sans,
            &gname.to_uppercase(),
            layout::Rect {
                x: PAD + 36.0,
                y: row.y,
                w: row.w - 36.0,
                h: row.h,
            },
            name_top,
            H2,
            chrome::TEXT,
        );

        let stat = if down > 0 {
            format!("{down} DOWN")
        } else {
            format!("{up}/{0} OK", idxs.len())
        };
        let stat_c = if down > 0 { chrome::RED } else { chrome::MUTED };
        layout::line_right(pm, &fonts.mono, &stat, row, name_top, T3, stat_c);
        chrome::fill_rect(
            pm,
            PAD,
            y + ROW_H - 2.0,
            w - 2.0 * PAD,
            2.0,
            chrome::CARD_LINE,
        );
        y += ROW_H;
    }
}
