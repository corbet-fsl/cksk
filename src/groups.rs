//! Group boards: the service lists at double type size.
//! One board shows a fixed set of groups; rows are big enough to read across a room,
//! so a large estate spans several boards instead of one dense screen.

use tiny_skia::Pixmap;

use crate::chrome::{
    self, Fonts, Header, ACCENT, CARD, CARD_LINE, FAINT, GREEN, HEADER_H, HIST_DOWN, HIST_NONE,
    MUTED, PAD, RED, TEXT,
};
use crate::model::Endpoint;

/// Double-size metrics: body type is 2x the original single-screen layout.
const ROW_H: f32 = 88.0;
const GROUP_HEAD_H: f32 = 56.0;
const CARD_PAD: f32 = 16.0;
const GROUP_GAP: f32 = 20.0;
const COL_GAP: f32 = 24.0;
const NAME_SIZE: f32 = 30.0;
const LAT_SIZE: f32 = 26.0;
const TITLE_SIZE: f32 = 26.0;
const COUNT_SIZE: f32 = 26.0;

/// Rows per column at 2x on a 1080p screen: a card is head + n*row + pad and must fit
/// 1080 - HEADER_H - 16 - PAD = 938. Nine rows: 56 + 792 + 16 = 864. Ten would be 952.
pub const ROWS_PER_COLUMN: usize = 9;

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
/// A group larger than one column is split into titled continuation chunks; a chunk that
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
    let pad = PAD;
    let cols = cols.max(1);

    chrome::fill_rect(pm, 0.0, 0.0, w, h, chrome::BG);
    chrome::render_header(pm, fonts, hdr);

    let top = HEADER_H + 16.0;
    let bottom = h - pad;
    let col_w = (w - 2.0 * pad - (cols as f32 - 1.0) * COL_GAP) / cols as f32;

    // Biggest chunks first so a large group claims room instead of packing late.
    // Groups are split into column-sized chunks: a 22-row group never fits one column
    // whole, so it becomes titled continuations (PUBLIC, PUBLIC · 2/3, PUBLIC · 3/3).
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

    let mut col_y = vec![top; cols];
    for ci in order {
        let (title, idxs) = &chunks[ci];
        let card_h = GROUP_HEAD_H + idxs.len() as f32 * ROW_H + CARD_PAD;
        // Shortest column that actually fits this chunk.
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
        let x = pad + c as f32 * (col_w + COL_GAP);
        let y = col_y[c];

        chrome::rounded_rect(pm, x, y, col_w, card_h, 16.0, CARD);

        let gx = x + CARD_PAD;
        chrome::text(pm, &fonts.sans, title, gx, y + 40.0, TITLE_SIZE, ACCENT);
        let count = format!("{}", idxs.len());
        chrome::text_right(
            pm,
            &fonts.mono,
            &count,
            x + col_w - CARD_PAD,
            y + 40.0,
            COUNT_SIZE,
            FAINT,
        );
        chrome::fill_rect(
            pm,
            gx,
            y + GROUP_HEAD_H,
            col_w - 2.0 * CARD_PAD,
            2.0,
            CARD_LINE,
        );

        let inner_w = col_w - 2.0 * CARD_PAD;
        for (ri, &ei) in idxs.iter().enumerate() {
            let e = &eps[ei];
            let ry = y + GROUP_HEAD_H + ri as f32 * ROW_H;
            let dot = match e.up() {
                Some(true) => GREEN,
                Some(false) => RED,
                None => chrome::AMBER,
            };
            chrome::circle(pm, gx + 10.0, ry + 30.0, 9.0, dot);

            let lat = match e.latency_ms() {
                Some(ms) if ms >= 1000.0 => format!("{:.2} s", ms / 1000.0),
                Some(ms) => format!("{} ms", ms.round() as i64),
                None => "—".to_string(),
            };
            let lat_w = chrome::text_width(&fonts.mono, &lat, LAT_SIZE);
            chrome::text_right(
                pm,
                &fonts.mono,
                &lat,
                gx + inner_w,
                ry + 40.0,
                LAT_SIZE,
                MUTED,
            );

            let name_x = gx + 30.0;
            let name_max = inner_w - 30.0 - lat_w - 12.0;
            let name = chrome::truncate(&fonts.sans, &e.name, NAME_SIZE, name_max);
            chrome::text(pm, &fonts.sans, &name, name_x, ry + 40.0, NAME_SIZE, TEXT);

            let strip_y = ry + 54.0;
            let strip_h = 10.0;
            let strip_w = inner_w - 30.0;
            let hist = e.history(48);
            if hist.is_empty() {
                chrome::fill_rect(pm, name_x, strip_y, strip_w, strip_h, HIST_NONE);
            } else {
                let n = hist.len() as f32;
                let seg = (strip_w / n).max(2.0);
                for (k, ok) in hist.iter().enumerate() {
                    let sx = name_x + k as f32 * seg;
                    let c = if *ok { GREEN } else { HIST_DOWN };
                    chrome::fill_rect(pm, sx, strip_y, (seg - 1.0).max(1.0), strip_h, c);
                }
            }
        }

        col_y[c] = y + card_h + GROUP_GAP;
    }
}
