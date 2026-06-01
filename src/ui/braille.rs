//! A tiny braille-based drawing surface.
//!
//! Unicode braille (`U+2800..=U+28FF`) packs a 2x4 grid of dots into a single
//! character cell. That gives us 2x horizontal and 4x vertical resolution over
//! plain text cells, which is exactly what we want for dense graphs and for
//! "half a character wide" per-core bars.
//!
//! Dot bit layout within one cell (column, row):
//! ```text
//!   col0 col1
//!   0x01 0x08   row0
//!   0x02 0x10   row1
//!   0x04 0x20   row2
//!   0x40 0x80   row3
//! ```
//!
//! The canvas uses a top-left origin: dot `(0, 0)` is the top-left dot. Helpers
//! that fill "from the bottom" (bars) account for that themselves.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

/// Bit value for the dot at a given `[row][col]` within a braille cell.
const DOT_BITS: [[u8; 2]; 4] = [
    [0x01, 0x08],
    [0x02, 0x10],
    [0x04, 0x20],
    [0x40, 0x80],
];

const BRAILLE_BLANK: u32 = 0x2800;

/// An off-screen braille drawing surface sized in character cells. Drawing is
/// done in *dot* coordinates; [`BrailleCanvas::render_to`] blits the result into
/// a ratatui [`Buffer`] at a target [`Rect`].
pub struct BrailleCanvas {
    cells_w: u16,
    cells_h: u16,
    /// Accumulated braille bits, one byte per character cell, row-major.
    bits: Vec<u8>,
    /// Foreground color per cell (last writer wins); `None` leaves it unset.
    colors: Vec<Option<Color>>,
}

impl BrailleCanvas {
    pub fn new(cells_w: u16, cells_h: u16) -> Self {
        let n = cells_w as usize * cells_h as usize;
        BrailleCanvas {
            cells_w,
            cells_h,
            bits: vec![0; n],
            colors: vec![None; n],
        }
    }

    /// Total dot columns (`2 * cells_w`).
    pub fn dot_width(&self) -> u32 {
        self.cells_w as u32 * 2
    }

    /// Total dot rows (`4 * cells_h`).
    pub fn dot_height(&self) -> u32 {
        self.cells_h as u32 * 4
    }

    /// Light a single dot at dot-coordinate `(x, y)` (top-left origin).
    pub fn set(&mut self, x: u32, y: u32, color: Color) {
        if x >= self.dot_width() || y >= self.dot_height() {
            return;
        }
        let cx = (x / 2) as u16;
        let cy = (y / 4) as u16;
        let idx = cy as usize * self.cells_w as usize + cx as usize;
        self.bits[idx] |= DOT_BITS[(y % 4) as usize][(x % 2) as usize];
        self.colors[idx] = Some(color);
    }

    /// Fill an inclusive vertical run of dots in column `x` from `y0` to `y1`.
    pub fn set_vline(&mut self, x: u32, y0: u32, y1: u32, color: Color) {
        let (lo, hi) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
        for y in lo..=hi {
            self.set(x, y, color);
        }
    }

    /// Draw a bar in dot-column `x` of `height` dots growing up from the bottom.
    pub fn set_bar(&mut self, x: u32, height: u32, color: Color) {
        if height == 0 {
            return;
        }
        let dh = self.dot_height();
        let top = dh.saturating_sub(height);
        self.set_vline(x, top, dh - 1, color);
    }

    /// Blit the accumulated dots into `buf` at `area`. Empty cells are left
    /// untouched so a caller can draw on top of an existing background.
    pub fn render_to(&self, area: Rect, buf: &mut Buffer) {
        let w = self.cells_w.min(area.width);
        let h = self.cells_h.min(area.height);
        for cy in 0..h {
            for cx in 0..w {
                let idx = cy as usize * self.cells_w as usize + cx as usize;
                let bits = self.bits[idx];
                if bits == 0 {
                    continue;
                }
                let ch = char::from_u32(BRAILLE_BLANK + bits as u32).unwrap_or(' ');
                if let Some(cell) = buf.cell_mut((area.x + cx, area.y + cy)) {
                    cell.set_char(ch);
                    if let Some(color) = self.colors[idx] {
                        cell.fg = color;
                    }
                }
            }
        }
    }
}
