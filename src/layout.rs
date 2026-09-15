//! Grid + rhythm + type system: every board is laid out on the same 12-column
//! grid with 8px rhythm and one type scale, so spacing and alignment match
//! across screens by construction instead of by hand-placed coordinates.
//!
//! Two rules everything below enforces:
//! 1. Every y position is `MARGIN`-relative and a multiple of the 8px rhythm.
//! 2. Every text baseline derives from a line TOP plus the font's REAL ascent,
//!    and every optical center from a MEASURED cap (`H`/`0`), never a heuristic.

use fontdue::Font;

use crate::chrome::{self, Rgb};

// ---------- grid ----------

/// Twelve columns across the full 1920px width.
pub const COLS: usize = 12;
pub const MARGIN: f32 = 30.0;
pub const GUTTER: f32 = 24.0;

/// Rhythm steps. Every padding, gap and row pitch is one of these.
pub const XS: f32 = 8.0;
pub const S: f32 = 16.0;
pub const M: f32 = 24.0;
pub const L: f32 = 32.0;
pub const XL: f32 = 48.0;

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn full(w: f32, h: f32) -> Self {
        Rect {
            x: MARGIN,
            y: 0.0,
            w: w - 2.0 * MARGIN,
            h,
        }
    }

    /// Column `i` of `COLS`, spanning `span` columns (gutters shared).
    pub fn col(&self, i: usize, span: usize) -> Rect {
        let total_gutter = (COLS as f32 - 1.0) * GUTTER;
        let unit = (self.w - total_gutter) / COLS as f32;
        Rect {
            x: self.x + i as f32 * (unit + GUTTER),
            y: self.y,
            w: unit * span as f32 + GUTTER * (span as f32 - 1.0),
            h: self.h,
        }
    }

    pub fn at_y(&self, y: f32, h: f32) -> Rect {
        Rect {
            x: self.x,
            y,
            w: self.w,
            h,
        }
    }

    pub fn inset(&self, pad: f32) -> Rect {
        Rect {
            x: self.x + pad,
            y: self.y + pad,
            w: (self.w - 2.0 * pad).max(0.0),
            h: (self.h - 2.0 * pad).max(0.0),
        }
    }

    /// Split off a top band of height `h`, returning (band, rest).
    pub fn take_top(&self, h: f32, gap: f32) -> (Rect, Rect) {
        (
            Rect {
                x: self.x,
                y: self.y,
                w: self.w,
                h,
            },
            Rect {
                x: self.x,
                y: self.y + h + gap,
                w: self.w,
                h: (self.h - h - gap).max(0.0),
            },
        )
    }

    /// n equal columns with gutters.
    pub fn columns(&self, n: usize) -> Vec<Rect> {
        let n = n.max(1);
        let cw = (self.w - (n as f32 - 1.0) * GUTTER) / n as f32;
        (0..n)
            .map(|i| Rect {
                x: self.x + i as f32 * (cw + GUTTER),
                y: self.y,
                w: cw,
                h: self.h,
            })
            .collect()
    }
}

// ---------- type scale (size, line height) ----------

#[derive(Clone, Copy, Debug)]
pub struct Type {
    pub size: f32,
    pub lh: f32,
}

pub const D1: Type = Type {
    size: 100.0,
    lh: 112.0,
}; // verdict
pub const H1: Type = Type {
    size: 64.0,
    lh: 76.0,
}; // headlines
pub const H2: Type = Type {
    size: 40.0,
    lh: 48.0,
}; // header title, clock
pub const H3: Type = Type {
    size: 34.0,
    lh: 44.0,
}; // pool names
pub const T3: Type = Type {
    size: 30.0,
    lh: 40.0,
}; // temps, section values
pub const BODY: Type = Type {
    size: 26.0,
    lh: 34.0,
}; // names, labels
pub const SMALL: Type = Type {
    size: 22.0,
    lh: 30.0,
}; // latencies, sub info
pub const TINY: Type = Type {
    size: 16.0,
    lh: 24.0,
}; // header meta
pub const MICRO: Type = Type {
    size: 20.0,
    lh: 28.0,
}; // feed age

// ---------- measured metrics (never heuristics) ----------

fn ascent(font: &Font, size: f32) -> f32 {
    font.horizontal_line_metrics(size)
        .map(|m| m.ascent)
        .unwrap_or(size * 0.8)
}

/// Baseline for a line whose TOP is `top`: text hangs from real ascent.
pub fn baseline(font: &Font, top: f32, size: f32) -> f32 {
    top + ascent(font, size)
}

/// Vertical center of a capital-height run starting at `top`, measured from a
/// reference cap (`H` for sans, `0` for mono) instead of guessed.
pub fn cap_center(font: &Font, top: f32, size: f32, sample: char) -> f32 {
    let m = font.metrics(sample, size);
    top + (m.height as f32 + m.ymin as f32) / 2.0
}

/// Baseline that centers `size` type on the cap-center of `ref_size` type sharing
/// `ref_baseline`. Replaces every hand-tuned secondary-size offset.
pub fn mate_baseline(
    font: &Font,
    ref_font: &Font,
    ref_baseline: f32,
    ref_size: f32,
    size: f32,
    sample: char,
) -> f32 {
    let ref_center = ref_baseline - cap_offset(ref_font, ref_size, sample);
    ref_center + cap_offset(font, size, sample)
}

fn cap_offset(font: &Font, size: f32, sample: char) -> f32 {
    let m = font.metrics(sample, size);
    (m.height as f32 + m.ymin as f32) / 2.0
}

/// Baseline that centers `size` caps on the vertical middle `mid`.
pub fn mid_baseline(font: &Font, mid: f32, size: f32, sample: char) -> f32 {
    mid + cap_offset(font, size, sample)
}

// ---------- line primitives on the grid ----------

/// Left-anchored line in a rect top: returns nothing, geometry is the contract.
pub fn line(
    pm: &mut tiny_skia::Pixmap,
    font: &Font,
    text: &str,
    rect: Rect,
    top: f32,
    t: Type,
    color: Rgb,
) {
    let s = chrome::truncate(font, text, t.size, rect.w);
    chrome::text(
        pm,
        font,
        &s,
        rect.x,
        baseline(font, top, t.size),
        t.size,
        color,
    );
}

/// Right-anchored line in a rect top.
pub fn line_right(
    pm: &mut tiny_skia::Pixmap,
    font: &Font,
    text: &str,
    rect: Rect,
    top: f32,
    t: Type,
    color: Rgb,
) {
    let s = chrome::truncate(font, text, t.size, rect.w);
    let w = chrome::text_width(font, &s, t.size);
    chrome::text(
        pm,
        font,
        &s,
        rect.x + rect.w - w,
        baseline(font, top, t.size),
        t.size,
        color,
    );
}

/// Dot centered on the cap-center of a line starting at `top`.
/// A typeface at a size plus its optical reference cap: the unit dots center on.
#[derive(Clone, Copy)]
pub struct Face<'a> {
    pub font: &'a Font,
    pub size: f32,
    pub sample: char,
}

/// Dot centered on the measured cap-center of a line starting at `top`.
pub fn dot(pm: &mut tiny_skia::Pixmap, cx: f32, top: f32, face: Face, r: f32, color: Rgb) {
    chrome::circle(
        pm,
        cx,
        cap_center(face.font, top, face.size, face.sample),
        r,
        color,
    );
}
