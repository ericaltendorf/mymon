//! Reusable braille chart primitives: a scrolling history line graph and a
//! vertical bar chart. Both are plain functions that draw into a [`Buffer`]
//! region, built on [`BrailleCanvas`].

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::braille::BrailleCanvas;

/// Orange used for the 75% bar-chart band.
const ORANGE: Color = Color::Rgb(255, 140, 0);

/// Map a fraction in `0.0..=1.0` to a green -> yellow -> red load color.
/// Used for non-bar contexts (e.g. the CPU% column in the process list).
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

/// Color for a bar dot, banded by the cell row it sits in. The bottom half of
/// the bar's cell rows always shows the indicator `base` color; cells in the
/// 50-75% band turn yellow; cells above 75% turn orange, or red once the bar's
/// value crosses 90%. Each braille cell is 4 dot rows tall, so for the standard
/// 4-cell-tall overview the bottom two rows are always `base`.
fn cell_band_color(cell_idx: u32, n_cells: u32, frac: f64, base: Color) -> Color {
    if n_cells == 0 {
        return base;
    }
    let cell_top_frac = (cell_idx + 1) as f64 / n_cells as f64;
    if cell_top_frac <= 0.5 + 1e-9 {
        base
    } else if cell_top_frac <= 0.75 + 1e-9 {
        Color::Yellow
    } else if frac >= 0.9 {
        Color::Red
    } else {
        ORANGE
    }
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
fn draw_series(canvas: &mut BrailleCanvas, samples: &[f64], max: f64, color: Color) {
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
        draw_series(&mut canvas, samples, max, *color);
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
/// Bars grow up from the bottom; each lit dot is colored by its vertical
/// position via [`threshold_color`], so the upper bands flash yellow/orange/red
/// while the rest of the bar carries the metric's `base` indicator color.
pub fn bar_chart(
    area: Rect,
    buf: &mut Buffer,
    values: &[f64],
    bar_dots: u32,
    gap_dots: u32,
    base: Color,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut canvas = BrailleCanvas::new(area.width, area.height);
    let dot_w = canvas.dot_width();
    let dot_h = canvas.dot_height();
    let n_cells = dot_h / 4;
    let stride = (bar_dots + gap_dots).max(1);

    let mut x = 0u32;
    for &v in values {
        if x >= dot_w {
            break;
        }
        let frac = v.clamp(0.0, 1.0);
        let height = bar_height(frac, dot_h);
        // Color each lit dot by which braille cell row it sits in, so whole
        // cell rows take on the indicator color or the upper-band color
        // together rather than smearing across thresholds.
        for h in 0..height {
            let y = dot_h - 1 - h;
            let cell_idx = h / 4;
            let color = cell_band_color(cell_idx, n_cells, frac, base);
            for dx in 0..bar_dots {
                if x + dx < dot_w {
                    canvas.set(x + dx, y, color);
                }
            }
        }
        x += stride;
    }

    canvas.render_to(area, buf);
}
