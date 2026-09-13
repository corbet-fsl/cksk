//! Shared kiosk chrome: palette, CPU primitives, fonts, and the persistent header.
//! Every board renders through this so `corbet status` + clock read identically on all
//! screens. Pure `tiny-skia`/`fontdue`: no GPU work, no Wayland, no display server.

use fontdue::Font;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

pub type Rgb = (u8, u8, u8);

// Palette — dark, violet-accented, calm. Never cream.
pub const BG: Rgb = (0x0d, 0x0d, 0x12);
pub const CARD: Rgb = (0x16, 0x16, 0x1f);
pub const CARD_LINE: Rgb = (0x26, 0x26, 0x33);
pub const TEXT: Rgb = (0xea, 0xea, 0xf2);
pub const MUTED: Rgb = (0x8a, 0x8a, 0x9c);
pub const FAINT: Rgb = (0x55, 0x55, 0x66);
pub const ACCENT: Rgb = (0xa7, 0x8b, 0xfa);
pub const GREEN: Rgb = (0x3f, 0xd0, 0x68);
pub const RED: Rgb = (0xf4, 0x53, 0x53);
pub const AMBER: Rgb = (0xf5, 0x9e, 0x0b);
pub const HIST_DOWN: Rgb = (0x7a, 0x2a, 0x2a);
pub const HIST_NONE: Rgb = (0x2a, 0x2a, 0x36);

pub struct Fonts {
    pub sans: Font,
    pub mono: Font,
}

pub fn load_fonts() -> Fonts {
    let settings = fontdue::FontSettings::default();
    let sans = fontdue::Font::from_bytes(&include_bytes!("../assets/Inter.ttf")[..], settings)
        .expect("load Inter");
    let mono = fontdue::Font::from_bytes(&include_bytes!("../assets/Mono.ttf")[..], settings)
        .expect("load Mono");
    Fonts { sans, mono }
}

// ---------- primitives ----------

fn solid(c: Rgb, a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.0, c.1, c.2, a);
    p.anti_alias = true;
    p
}

pub fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: Rgb) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pm.as_mut()
            .fill_rect(r, &solid(c, 255), Transform::identity(), None);
    }
}

pub fn rounded_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, rad: f32, c: Rgb) {
    let r = rad.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    if let Some(path) = pb.finish() {
        pm.as_mut().fill_path(
            &path,
            &solid(c, 255),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

pub fn circle(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, c: Rgb) {
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, r);
    if let Some(path) = pb.finish() {
        pm.as_mut().fill_path(
            &path,
            &solid(c, 255),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// Horizontal bar: `frac` (0.0..=1.0, clamped) of `w` in `fill` on a sunk track.
pub fn bar(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, frac: f64, fill: Rgb) {
    let f = frac.clamp(0.0, 1.0) as f32;
    fill_rect(pm, x, y, w, h, HIST_NONE);
    if f > 0.0 {
        fill_rect(pm, x, y, w * f, h, fill);
    }
}

#[inline]
fn blend(data: &mut [u8], i: usize, c: Rgb, cov: u8) {
    if cov == 0 {
        return;
    }
    let sa = cov as f32 / 255.0;
    let ia = 1.0 - sa;
    // Pixmap is premultiplied RGBA; text colour is opaque so src_premul = c*sa.
    data[i] = (c.0 as f32 * sa + data[i] as f32 * ia).round() as u8;
    data[i + 1] = (c.1 as f32 * sa + data[i + 1] as f32 * ia).round() as u8;
    data[i + 2] = (c.2 as f32 * sa + data[i + 2] as f32 * ia).round() as u8;
    data[i + 3] = (sa * 255.0 + data[i + 3] as f32 * ia).round() as u8;
}

pub fn text_width(font: &Font, text: &str, size: f32) -> f32 {
    text.chars()
        .map(|ch| font.metrics(ch, size).advance_width)
        .sum()
}

/// Draw a left-anchored string at baseline `by`. Returns advance width.
pub fn text(pm: &mut Pixmap, font: &Font, s: &str, x: f32, by: f32, size: f32, c: Rgb) -> f32 {
    let w = pm.width() as i32;
    let h = pm.height() as i32;
    let data = pm.data_mut();
    let mut pen = x;
    for ch in s.chars() {
        let (m, bitmap) = font.rasterize(ch, size);
        let gx = (pen + m.xmin as f32).round() as i32;
        let gy = (by - (m.height as f32 + m.ymin as f32)).round() as i32;
        for row in 0..m.height {
            for col in 0..m.width {
                let cov = bitmap[row * m.width + col];
                let px = gx + col as i32;
                let py = gy + row as i32;
                if px < 0 || py < 0 || px >= w || py >= h {
                    continue;
                }
                blend(data, ((py * w + px) * 4) as usize, c, cov);
            }
        }
        pen += m.advance_width;
    }
    pen - x
}

pub fn text_right(pm: &mut Pixmap, font: &Font, s: &str, right: f32, by: f32, size: f32, c: Rgb) {
    let w = text_width(font, s, size);
    text(pm, font, s, right - w, by, size, c);
}

pub fn truncate(font: &Font, s: &str, size: f32, max_w: f32) -> String {
    if text_width(font, s, size) <= max_w {
        return s.to_string();
    }
    let ell = "…";
    let ell_w = text_width(font, ell, size);
    let mut out = String::new();
    let mut w = 0.0;
    for ch in s.chars() {
        let cw = font.metrics(ch, size).advance_width;
        if w + cw + ell_w > max_w {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

// ---------- header ----------

pub struct Header<'a> {
    pub clock: &'a str,
    pub date: &'a str,
    pub total: usize,
    pub up: usize,
    pub down: usize,
    pub unknown: usize,
    /// Rotation position, e.g. `"2/5 · Public"`. Reads after the date, same line.
    pub indicator: &'a str,
}

pub const HEADER_H: f32 = 96.0;
pub const PAD: f32 = 30.0;

/// Persistent header: `corbet status` + summary left, clock/date + rotation
/// indicator right, accent divider below. Identical on every board.
pub fn render_header(pm: &mut Pixmap, fonts: &Fonts, hdr: &Header) {
    let w = pm.width() as f32;
    let pad = PAD;

    text(pm, &fonts.sans, "corbet", pad, 58.0, 40.0, TEXT);
    let cw = text_width(&fonts.sans, "corbet", 40.0);
    text(
        pm,
        &fonts.sans,
        "status",
        pad + cw + 14.0,
        58.0,
        40.0,
        ACCENT,
    );
    // summary line
    let summary = format!(
        "{} services   ·   {} up   ·   {} down   ·   {} idle",
        hdr.total, hdr.up, hdr.down, hdr.unknown
    );
    text(pm, &fonts.sans, &summary, pad, 82.0, 16.0, MUTED);
    // clock (right)
    text_right(pm, &fonts.mono, hdr.clock, w - pad, 56.0, 42.0, TEXT);
    let dateline = format!("{}   ·   {}", hdr.date, hdr.indicator);
    text_right(pm, &fonts.sans, &dateline, w - pad, 82.0, 16.0, MUTED);
    // accent divider
    fill_rect(pm, pad, HEADER_H, w - 2.0 * pad, 2.0, ACCENT);
}
