//! Reusable braille chart primitives: a scrolling history line graph and a
//! vertical bar chart. Both are plain functions that draw into a [`Buffer`]
//! region, built on [`BrailleCanvas`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::braille::BrailleCanvas;

/// Map a fraction in `0.0..=1.0` to a green -> yellow -> red load color.
pub fn load_color(frac: f64) -> Color {
    let f = frac.clamp(0.0, 1.0);
    // Two-segment linear gradient: green->yellow for the first half, then
    // yellow->red for the second half.
    let (r, g) = if f < 0.5 {
        let t = f / 0.5;
        (lerp(40.0, 220.0, t), 200.0)
    } else {
        let t = (f - 0.5) / 0.5;
        (220.0, lerp(200.0, 40.0, t))
    };
    Color::Rgb(r as u8, g as u8, 40)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Map a value to a braille dot row (0 = top), given the value range and the
/// total number of dot rows. Higher values sit nearer the top.
fn value_row(value: f64, max: f64, dot_h: u32) -> u32 {
    if dot_h == 0 {
        return 0;
    }
    let frac = (value / max).clamp(0.0, 1.0);
    let row = ((1.0 - frac) * (dot_h - 1) as f64).round();
    row as u32
}

/// Plot one connected line series onto `canvas`, right-aligned (newest last).
/// `color_at` resolves the color for a column given its raw value.
fn draw_series(
    canvas: &mut BrailleCanvas,
    samples: &[f64],
    max: f64,
    color_at: impl Fn(f64) -> Color,
) {
    let dot_w = canvas.dot_width();
    let dot_h = canvas.dot_height();
    let n = samples.len().min(dot_w as usize);
    if n == 0 {
        return;
    }
    let recent = &samples[samples.len() - n..];
    let x_offset = dot_w - n as u32;

    let mut prev_y: Option<u32> = None;
    for (i, &v) in recent.iter().enumerate() {
        let x = x_offset + i as u32;
        let y = value_row(v, max, dot_h);
        let color = color_at(v);
        match prev_y {
            // Connect consecutive samples with a vertical run so the line is
            // continuous even across steep changes.
            Some(py) => canvas.set_vline(x, py, y, color),
            None => canvas.set(x, y, color),
        }
        prev_y = Some(y);
    }
}

/// Draw several line series sharing one set of axes into `area`. Each series is
/// drawn in a single fixed color; later series win color on overlapping cells.
pub fn history_multi(area: Rect, buf: &mut Buffer, series: &[(&[f64], Color)], max: f64) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut canvas = BrailleCanvas::new(area.width, area.height);
    for (samples, color) in series {
        let color = *color;
        draw_series(&mut canvas, samples, max, move |_| color);
    }
    canvas.render_to(area, buf);
}

/// Bar height in dots for a fill fraction, with a persistent baseline: a value
/// of 0 still lights the bottom dot row (level 1), and any non-zero value
/// reaches at least level 2. This keeps every bar visible so idle cores/GPUs
/// don't vanish.
fn bar_height(frac: f64, dot_h: u32) -> u32 {
    if dot_h == 0 {
        return 0;
    }
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.0 {
        1
    } else {
        let h = 2.0 + frac * dot_h.saturating_sub(2) as f64;
        (h.round() as u32).clamp(2, dot_h)
    }
}

/// Draw vertical bars for `values` (each in `0.0..=1.0`) into `area`.
///
/// Each value occupies `bar_dots` dot-columns (use `1` for the "half a
/// character wide" per-core look) followed by `gap_dots` empty dot-columns.
/// Bars grow up from the bottom and are colored by value.
pub fn bar_chart(area: Rect, buf: &mut Buffer, values: &[f64], bar_dots: u32, gap_dots: u32) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut canvas = BrailleCanvas::new(area.width, area.height);
    let dot_w = canvas.dot_width();
    let dot_h = canvas.dot_height();
    let stride = (bar_dots + gap_dots).max(1);

    let mut x = 0u32;
    for &v in values {
        if x >= dot_w {
            break;
        }
        let frac = v.clamp(0.0, 1.0);
        let height = bar_height(frac, dot_h);
        let color = load_color(frac);
        for dx in 0..bar_dots {
            if x + dx >= dot_w {
                break;
            }
            canvas.set_bar(x + dx, height, color);
        }
        x += stride;
    }

    canvas.render_to(area, buf);
}

/// Draw a single vertical bar made of stacked `segments`, each
/// `(fraction_of_full_height, color)`, stacked from the bottom up. The bar is
/// `bar_dots` dot-columns wide and left-aligned in `area`. Cumulative height is
/// clamped to the top, so overflowing segments are simply cut off.
pub fn stacked_bar(area: Rect, buf: &mut Buffer, segments: &[(f64, Color)], bar_dots: u32) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut canvas = BrailleCanvas::new(area.width, area.height);
    let dot_w = canvas.dot_width();
    let dot_h = canvas.dot_height();
    let cols = bar_dots.min(dot_w);

    let mut filled = 0u32; // dots already filled, measured from the bottom
    for (frac, color) in segments {
        if filled >= dot_h {
            break;
        }
        let h = (frac.clamp(0.0, 1.0) * dot_h as f64).round() as u32;
        let top = (filled + h).min(dot_h);
        for b in filled..top {
            let y = dot_h - 1 - b;
            for dx in 0..cols {
                canvas.set(dx, y, *color);
            }
        }
        filled = top;
    }

    // Persistent baseline: keep the bar visible even when everything is ~0.
    if filled == 0 {
        let color = segments.first().map(|(_, c)| *c).unwrap_or(Color::Gray);
        for dx in 0..cols {
            canvas.set(dx, dot_h - 1, color);
        }
    }

    canvas.render_to(area, buf);
}
