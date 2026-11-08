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

/// Where one chunk (a group, or one continuation chunk of a large group) lands.
pub struct Placement {
    /// Index into the chunk list that was packed.
    pub chunk: usize,
    pub col: usize,
    pub y: f32,
    pub h: f32,
}

/// Rows a column holds at this pixmap height. Twelve at 1080p; a shorter kiosk
/// (or a preview at another size) gets fewer, so no page silently drops a card
/// that cannot fit the band.
pub fn rows_per_column(height: f32) -> usize {
    let (top, bottom) = band(height);
    let usable = bottom - top - GROUP_HEAD_H - CARD_PAD + GROUP_GAP;
    ((usable / ROW_H).floor() as usize).clamp(1, ROWS_PER_COLUMN)
}

/// Split `idxs` of one group into column-sized chunks (the last one may be shorter).
pub fn chunk_group(idxs: &[usize], rows: usize) -> Vec<Vec<usize>> {
    idxs.chunks(rows.max(1)).map(|c| c.to_vec()).collect()
}

/// All chunks of `groups` with their titled continuations, biggest first (the order
/// the packer consumes them in so a large group claims room instead of packing late).
pub fn chunks_of(groups: &[(String, Vec<usize>)], rows: usize) -> Vec<(String, Vec<usize>)> {
    let mut chunks: Vec<(String, Vec<usize>)> = Vec::new();
    for (gname, idxs) in groups {
        let parts = chunk_group(idxs, rows);
        let n = parts.len().max(1);
        for (k, part) in parts.into_iter().enumerate() {
            let title = if n == 1 {
                gname.to_uppercase()
            } else {
                format!("{} · {}/{}", gname.to_uppercase(), k + 1, n)
            };
            chunks.push((title, part));
        }
    }
    chunks.sort_by_key(|(_, idxs)| std::cmp::Reverse(idxs.len()));
    chunks
}

/// Greedy shortest-column packing of `chunks` between `top` and `bottom`.
/// Returns the placements plus every chunk that fit no column (dropped, never clipped).
pub fn pack(
    chunks: &[(String, Vec<usize>)],
    cols: usize,
    top: f32,
    bottom: f32,
) -> (Vec<Placement>, Vec<usize>) {
    let cols = cols.max(1);
    let mut col_y = vec![top; cols];
    let mut placements = Vec::new();
    let mut dropped = Vec::new();
    for (ci, (_, idxs)) in chunks.iter().enumerate() {
        let h = card_height(idxs.len());
        let mut best: Option<usize> = None;
        for (c, y) in col_y.iter().enumerate() {
            if y + h > bottom {
                continue;
            }
            if best.is_none_or(|b| *y < col_y[b]) {
                best = Some(c);
            }
        }
        match best {
            Some(c) => {
                placements.push(Placement {
                    chunk: ci,
                    col: c,
                    y: col_y[c],
                    h,
                });
                col_y[c] += h + GROUP_GAP;
            }
            None => dropped.push(ci),
        }
    }
    (placements, dropped)
}

fn card_height(rows: usize) -> f32 {
    GROUP_HEAD_H + rows as f32 * ROW_H + CARD_PAD
}

/// The vertical band every group board draws into, for a given pixmap height.
pub fn band(height: f32) -> (f32, f32) {
    (HEADER_H + XS, height - MARGIN)
}

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
/// Returns the indices of every endpoint actually drawn, so callers can prove coverage.
pub fn render_groups(
    pm: &mut Pixmap,
    eps: &[Endpoint],
    fonts: &Fonts,
    hdr: &Header,
    groups: &[(String, Vec<usize>)],
    cols: usize,
) -> Vec<usize> {
    let w = pm.width() as f32;
    let h = pm.height() as f32;
    if cols == 0 {
        return Vec::new();
    }

    chrome::fill_rect(pm, 0.0, 0.0, w, h, chrome::BG);
    chrome::render_header(pm, fonts, hdr);

    let (top, bottom) = band(h);
    let band_rect = layout::Rect {
        x: MARGIN,
        y: top,
        w: w - 2.0 * MARGIN,
        h: bottom - top,
    };
    let columns = band_rect.columns(cols);
    let col_w = columns[0].w;

    let chunks = chunks_of(groups, rows_per_column(h));
    let (placements, dropped) = pack(&chunks, cols, top, bottom);
    if !dropped.is_empty() {
        eprintln!(
            "cksk: {} chunk(s) do not fit this board at {w}x{h}; they are dropped. \
             The page planner must leave more room.",
            dropped.len()
        );
    }

    let mut drawn = Vec::new();
    for p in placements {
        let (title, idxs) = &chunks[p.chunk];
        let x = columns[p.col].x;
        let y = p.y;
        let card_h = p.h;

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
            drawn.push(ei);
        }
    }
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups(n: &[usize]) -> Vec<(String, Vec<usize>)> {
        n.iter()
            .enumerate()
            .map(|(g, &rows)| (format!("g{g}"), (0..rows).collect()))
            .collect()
    }

    #[test]
    fn pack_drops_nothing_when_the_page_was_planned_for_the_band() {
        let (top, bottom) = band(1080.0);
        // 30 + 24 + 25 rows: three pages worth of whole groups across three columns.
        for page in [
            groups(&[30]),
            groups(&[9, 13, 1, 2]),
            groups(&[3, 2, 2, 10, 7]),
        ] {
            let chunks = chunks_of(&page, rows_per_column(1080.0));
            let (placements, dropped) = pack(&chunks, 3, top, bottom);
            assert!(dropped.is_empty(), "dropped {dropped:?} for {page:?}");
            assert_eq!(placements.len(), chunks.len());
        }
    }

    #[test]
    fn pack_reports_a_chunk_that_fits_no_column() {
        let chunks = chunks_of(&groups(&[12, 12, 12, 12]), 12);
        let (placements, dropped) = pack(&chunks, 3, 0.0, 1000.0);
        assert_eq!(placements.len(), 3);
        assert_eq!(dropped.len(), 1);
    }
}
