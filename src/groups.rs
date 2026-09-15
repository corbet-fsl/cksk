//! Group boards: the service lists at room-readable type, laid out on the
//! shared grid. One board shows a fixed set of groups; rows are big enough to
//! read across a room, so a large estate spans several boards instead of one
//! dense screen.

use tiny_skia::Pixmap;

use crate::chrome::{self, Fonts, Header, HEADER_H};
use crate::layout::{self, BODY, MARGIN, SMALL, XS};
use crate::model::Endpoint;

/// Twelve rows fill a 1080p column: head 56 + 12*70 + pad 16 = 912 of 938.
const ROW_H: f32 = 70.0;
const GROUP_HEAD_H: f32 = 56.0;
const CARD_PAD: f32 = 16.0;
const GROUP_GAP: f32 = 20.0;
pub const ROWS_PER_COLUMN: usize = 12;

/// Filter `eps` down to the `want` groups, in `want` order. Missing or empty groups
/// vanish silently, so a board never renders an empty card.
pub fn select_groups(eps: &[Endpoint], want: &[&str]) -> Vec<(String, Vec<usize>)> {
    let mut out = Vec::new();
    for name in want {
        let idxs: Vec<usize> = eps
            .iter()
            .enumerate()
            .filter(|(_, e)| e.group == *name)
            .map(|(i, _)| i)
            .collect();
        if !idxs.is_empty() {
            out.push((name.to_string(), idxs));
        }
    }
    out
}

/// Render pre-selected `groups` into `cols` columns, greediest-shortest-column first.
/// A group larger than one column splits into titled continuation chunks; a chunk that
/// fits no column is skipped gracefully -- a board shows what fits, never a clipped card.
pub fn render_groups(
    pm: &mut Pixmap,
    eps: &[Endpoint],
    fonts: &Fonts,
    hdr: &Header,
    groups: &[(String, Vec<usize>)],
    cols: usize,
) {
    let w = pm.width() as f32;
    let h = pm.height() as f32;
    let cols = cols.max(1);

    chrome::fill_rect(pm, 0.0, 0.0, w, h, chrome::BG);
    chrome::render_header(pm, fonts, hdr);

    let band = layout::Rect {
        x: MARGIN,
        y: HEADER_H + XS,
        w: w - 2.0 * MARGIN,
        h: h - HEADER_H - XS - MARGIN,
    };
    let bottom = band.y + band.h;
    let columns = band.columns(cols);
    let col_w = columns[0].w;

    // Biggest chunks first so a large group claims room instead of packing late.
    // Groups split into column-sized chunks with titled continuations.
    let mut chunks: Vec<(String, Vec<usize>)> = Vec::new();
    for (gname, idxs) in groups {
        let parts: Vec<&[usize]> = idxs.chunks(ROWS_PER_COLUMN).collect();
        let n = parts.len().max(1);
        for (k, part) in parts.into_iter().enumerate() {
            let title = if n == 1 {
                gname.to_uppercase()
            } else {
                format!("{} · {}/{}", gname.to_uppercase(), k + 1, n)
            };
            chunks.push((title, part.to_vec()));
        }
    }
    let mut order: Vec<usize> = (0..chunks.len()).collect();
    order.sort_by_key(|&ci| std::cmp::Reverse(chunks[ci].1.len()));

    // Column cursors: greedy shortest column that actually fits the chunk.
    let mut col_y: Vec<f32> = columns.iter().map(|c| c.y).collect();
    for ci in order {
        let (title, idxs) = &chunks[ci];
        let card_h = GROUP_HEAD_H + idxs.len() as f32 * ROW_H + CARD_PAD;
        let mut best: Option<usize> = None;
        for (c, y) in col_y.iter().enumerate() {
            if y + card_h > bottom {
                continue;
            }
            if best.is_none_or(|b| *y < col_y[b]) {
                best = Some(c);
            }
        }
        let c = match best {
            Some(c) => c,
            None => continue, // fits no column; skip it, never clip
        };
        let x = columns[c].x;
        let y = col_y[c];

        chrome::rounded_rect(pm, x, y, col_w, card_h, 16.0, chrome::CARD);

        // Card header: title + count share one SMALL line top.
        let head_top = y + 12.0;
        layout::line(
            pm,
            &fonts.sans,
            title,
            layout::Rect {
                x: x + CARD_PAD,
                y,
                w: col_w - 2.0 * CARD_PAD,
                h: GROUP_HEAD_H,
            },
            head_top,
            SMALL,
            chrome::ACCENT,
        );
        layout::line_right(
            pm,
            &fonts.mono,
            &idxs.len().to_string(),
            layout::Rect {
                x: x + CARD_PAD,
                y,
                w: col_w - 2.0 * CARD_PAD,
                h: GROUP_HEAD_H,
            },
            head_top,
            SMALL,
            chrome::FAINT,
        );
        chrome::fill_rect(
            pm,
            x + CARD_PAD,
            y + GROUP_HEAD_H,
            col_w - 2.0 * CARD_PAD,
            2.0,
            chrome::CARD_LINE,
        );

        // Rows: name + latency share one line top; history strip below.
        let inner = layout::Rect {
            x: x + CARD_PAD,
            y,
            w: col_w - 2.0 * CARD_PAD,
            h: card_h,
        };
        for (ri, &ei) in idxs.iter().enumerate() {
            let e = &eps[ei];
            let ry = y + GROUP_HEAD_H + ri as f32 * ROW_H;
            let name_top = ry + 12.0;
            let dot = match e.up() {
                Some(true) => chrome::GREEN,
                Some(false) => chrome::RED,
                None => chrome::AMBER,
            };
            layout::dot(
                pm,
                inner.x + 10.0,
                name_top,
                layout::Face {
                    font: &fonts.sans,
                    size: BODY.size,
                    sample: 'H',
                },
                8.0,
                dot,
            );

            let lat = match e.latency_ms() {
                Some(ms) if ms >= 1000.0 => format!("{:.2} s", ms / 1000.0),
                Some(ms) => format!("{} ms", ms.round() as i64),
                None => "—".to_string(),
            };
            layout::line_right(pm, &fonts.mono, &lat, inner, name_top, SMALL, chrome::MUTED);

            let name_x = inner.x + 30.0;
            let lat_w = chrome::text_width(&fonts.mono, &lat, SMALL.size);
            let name_max = inner.w - 30.0 - lat_w - 12.0;
            let name = chrome::truncate(&fonts.sans, &e.name, BODY.size, name_max);
            chrome::text(
                pm,
                &fonts.sans,
                &name,
                name_x,
                layout::baseline(&fonts.sans, name_top, BODY.size),
                BODY.size,
                chrome::TEXT,
            );

            let strip = layout::Rect {
                x: name_x,
                y: name_top + BODY.lh + XS,
                w: inner.w - 30.0,
                h: 8.0,
            };
            let hist = e.history(48);
            if hist.is_empty() {
                chrome::fill_rect(pm, strip.x, strip.y, strip.w, strip.h, chrome::HIST_NONE);
            } else {
                let n = hist.len() as f32;
                let seg = (strip.w / n).max(2.0);
                for (k, ok) in hist.iter().enumerate() {
                    let sx = strip.x + k as f32 * seg;
                    let c = if *ok {
                        chrome::GREEN
                    } else {
                        chrome::HIST_DOWN
                    };
                    chrome::fill_rect(pm, sx, strip.y, (seg - 1.0).max(1.0), strip.h, c);
                }
            }
        }

        col_y[c] = y + card_h + GROUP_GAP;
    }
}
