//! Overview board: the whole estate at a glance in giant type.
//! One big verdict, then one row per group. Detail lives on the group boards.
//! Geometry comes from the shared grid (layout.rs): rows, type and dots match
//! every other board by construction.

use tiny_skia::Pixmap;

use crate::chrome::{self, Fonts, Header, HEADER_H, PAD};
use crate::layout::{self, D1, H2, MARGIN, T3, XS};
use crate::model::Endpoint;

/// Canonical group order: most customer-visible first, housekeeping last.
/// This is the live estate's vocabulary (the Vultr status source). A group that
/// is not listed still appears: `pages::ordered_groups` appends it after these,
/// so a new group can never vanish from the boards.
pub const GROUP_ORDER: &[&str] = &[
    "mail",
    "certificates",
    "ci",
    "nodes",
    "caches",
    "github",
    "seaweedfs",
    "backups",
    "clouds",
    "watchdog",
];

const ROW_H: f32 = 92.0;

/// Render the overview and return how many group rows were drawn.
/// Rows compact when the estate has many groups; past the readable minimum the
/// tail is summarised as "+N more" instead of drawn over itself (the detail pages
/// still carry every row).
pub fn render_overview(
    pm: &mut Pixmap,
    eps: &[Endpoint],
    fonts: &Fonts,
    hdr: &Header,
    groups: &[(String, Vec<usize>)],
    data_age_secs: Option<u64>,
) -> usize {
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
        layout::line(pm, &fonts.sans, &sub, grid, y + D1.lh + XS, T3, sub_color);
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
            y + D1.lh + XS,
            T3,
            chrome::RED,
        );
    }
    y += D1.lh + T3.lh + 2.0 * XS + layout::M;

    // ---- one row per group (compacting to keep every row inside the panel) ----
    let available = h - PAD - y;
    let n = groups.len();
    if n == 0 {
        return 0;
    }
    let min_h = H2.lh + XS;
    let mut row_h = (available / n as f32).min(ROW_H);
    let shown = if row_h < min_h {
        // Not enough height for every group at a readable size: draw as many as
        // fit and name the remainder. Never overlap type on type.
        row_h = min_h;
        (available / row_h).floor().max(0.0) as usize
    } else {
        n
    };
    for (gname, idxs) in groups.iter().take(shown) {
        let up = idxs
            .iter()
            .filter(|&&ei| eps[ei].up() == Some(true))
            .count();
        let down = idxs
            .iter()
            .filter(|&&ei| eps[ei].up() == Some(false))
            .count();
        let dot = if down > 0 { chrome::RED } else { chrome::GREEN };
        let row = grid.at_y(y, row_h);
        // Name block: dot + title share one line box, dot on the measured cap-center.
        let name_top = y + (row_h - H2.lh) / 2.0;
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
            format!("{up}/{} OK", idxs.len())
        };
        let stat_c = if down > 0 { chrome::RED } else { chrome::MUTED };
        layout::line_right(pm, &fonts.mono, &stat, row, name_top, T3, stat_c);
        chrome::fill_rect(
            pm,
            PAD,
            y + row_h - 2.0,
            w - 2.0 * PAD,
            2.0,
            chrome::CARD_LINE,
        );
        y += row_h;
    }
    if shown < n {
        layout::line(
            pm,
            &fonts.sans,
            &format!("+{} more groups on the detail boards", n - shown),
            grid,
            y + XS,
            T3,
            chrome::MUTED,
        );
    }
    shown
}
