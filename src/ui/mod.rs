//! Top-level TUI layout and per-block rendering.
//!
//! Layout (top to bottom):
//! - **Overview** block (compact): a left stats gutter (CPU/MEM/GPU/VRAM)
//!   that doubles as the graph legend, a bar group — per-core CPU bars, then
//!   the memory bar, then per-GPU utilization and per-GPU memory bars — and
//!   on the right a shared history graph plotting the four series. White ▴
//!   tick marks sit on the bottom frame at one-minute intervals.
//! - **Processes** block: the column headers live on the top frame so all
//!   inner rows can be filled with process entries.

mod braille;
mod charts;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, mean_gpu_mem, mean_gpu_util};
use crate::format;
use crate::metrics::{ProcessMetrics, Snapshot};

use charts::{bar_chart, history_multi, load_color};

const CPU_COLOR: Color = Color::Cyan;
const GPU_COLOR: Color = Color::Magenta;
const VRAM_COLOR: Color = Color::LightMagenta;
const MEM_COLOR: Color = Color::Blue;
const PROC_COLOR: Color = Color::Gray;
const FRAME_COLOR: Color = Color::Gray;

/// Graph content rows for the compact overview block, excluding the border.
const STAT_INNER_ROWS: u16 = 4;
/// Total block height: inner content rows plus the top/bottom frame rows.
const STAT_BLOCK_ROWS: u16 = STAT_INNER_ROWS + 2;
/// Width of the left stats/legend gutter (fits e.g. "CPU 100%").
const GUTTER_W: u16 = 9;

/// Render the whole UI for one frame.
pub fn render(f: &mut Frame, app: &App) {
    // The overview block stays compact (up to 4 content rows); everything left
    // over flows into the process list below.
    let [top_area, proc_area] =
        Layout::vertical([Constraint::Max(STAT_BLOCK_ROWS), Constraint::Fill(1)]).areas(f.area());

    render_overview(f, top_area, app);
    render_processes(f, proc_area, app);
}

fn bold(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(
        text.into(),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn render_overview(f: &mut Frame, area: Rect, app: &App) {
    if !app.ready {
        let placeholder = Block::bordered()
            .border_style(Style::new().fg(FRAME_COLOR))
            .title(Line::from(bold(" mymon ", FRAME_COLOR)));
        f.render_widget(placeholder, area);
        return;
    }
    let snap = &app.snapshot;
    let cpu = &snap.cpu;
    let mem = &snap.memory;
    let has_gpu = !snap.gpus.is_empty();

    // Title: hostname, CPU model (in CPU color), GPU summary (in GPU color),
    // uptime. The accent colors echo each metric's bar/graph color.
    let host = snap.host.hostname.as_deref().unwrap_or("mymon");
    let cpu_model = format::model_number(&cpu.brand);
    let gpu_summary = format::gpu_summary(&snap.gpus);
    let uptime = format::duration(snap.host.uptime);
    let mut title_spans = vec![
        bold(format!(" {host} · "), FRAME_COLOR),
        bold(cpu_model, CPU_COLOR),
    ];
    if !gpu_summary.is_empty() {
        title_spans.push(bold(" · ", FRAME_COLOR));
        title_spans.push(bold(gpu_summary, GPU_COLOR));
    }
    title_spans.push(bold(format!(" · up {uptime} "), FRAME_COLOR));

    let block = Block::bordered()
        .border_style(Style::new().fg(FRAME_COLOR))
        .title(Line::from(title_spans));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // --- Bar values, each sorted high -> low for a descending staircase. ---
    let mut cpu_bars: Vec<f64> = cpu.per_core.iter().map(|c| c.usage as f64 / 100.0).collect();
    cpu_bars.sort_by(desc);
    let mut gpu_bars: Vec<f64> = snap
        .gpus
        .iter()
        .map(|g| g.utilization_gpu.unwrap_or(0) as f64 / 100.0)
        .collect();
    gpu_bars.sort_by(desc);
    let mut gpu_mem_bars: Vec<f64> = snap.gpus.iter().map(|g| g.memory_used_fraction()).collect();
    gpu_mem_bars.sort_by(desc);
    let mem_frac = mem.used as f64 / mem.total.max(1) as f64;

    // --- Bar-panel width budget. ---
    // CPU cores at 1 dot per core; system memory at 2 dots (1 char); GPU util
    // at 2 dots per device; GPU memory at 1 dot per device (so 4 GPUs = 2
    // text cells).
    let cpu_chars = (cpu_bars.len() as u16).div_ceil(2);
    let mem_chars = 1u16;
    let gpu_chars = gpu_bars.len() as u16;
    let gpu_mem_chars = (gpu_mem_bars.len() as u16).div_ceil(2);
    let bars_w = cpu_chars
        + 1
        + mem_chars
        + if has_gpu {
            1 + gpu_chars + 1 + gpu_mem_chars
        } else {
            0
        };

    // --- Inner layout: gutter | bars | gap | graph (graph on the right). ---
    let max_bars = inner.width.saturating_sub(GUTTER_W + 6);
    let [gutter, bars, _gap, graph] = Layout::horizontal([
        Constraint::Length(GUTTER_W),
        Constraint::Length(bars_w.min(max_bars)),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);

    render_gutter(f, gutter, snap);

    // --- Bars: CPU cores | gap | memory | [gap | GPU util | gap | GPU mem]. ---
    let mut cons = vec![
        Constraint::Length(cpu_chars),
        Constraint::Length(1),
        Constraint::Length(mem_chars),
    ];
    if has_gpu {
        cons.push(Constraint::Length(1));
        cons.push(Constraint::Length(gpu_chars));
        cons.push(Constraint::Length(1));
        cons.push(Constraint::Length(gpu_mem_chars));
    }
    let segs = Layout::horizontal(cons).split(bars);

    bar_chart(segs[0], f.buffer_mut(), &cpu_bars, 1, 0, CPU_COLOR);
    bar_chart(segs[2], f.buffer_mut(), &[mem_frac], 2, 0, MEM_COLOR);
    if has_gpu {
        bar_chart(segs[4], f.buffer_mut(), &gpu_bars, 2, 0, GPU_COLOR);
        bar_chart(segs[6], f.buffer_mut(), &gpu_mem_bars, 1, 0, VRAM_COLOR);
    }

    // --- Graph: lines sharing one axis. Draw the flatter/quieter ones first so
    // the lines you watch most (CPU/GPU) win the cell color on overlap. ---
    let cpu_hist = app.cpu_history.to_vec();
    let gpu_hist = app.gpu_history.to_vec();
    let gpu_mem_hist = app.gpu_mem_history.to_vec();
    let mem_hist = app.mem_history.to_vec();
    let mut series: Vec<(&[f64], Color)> = vec![(&mem_hist, MEM_COLOR)];
    if has_gpu {
        series.push((&gpu_mem_hist, VRAM_COLOR));
        series.push((&gpu_hist, GPU_COLOR));
    }
    series.push((&cpu_hist, CPU_COLOR));
    history_multi(graph, f.buffer_mut(), &series, 100.0);

    // White ▴ tick marks on the bottom frame at one-minute intervals, counted
    // back from the right edge (newest sample).
    draw_time_ticks(f, graph, app.stats_interval.as_secs_f64());
}

/// Place a ▴ on the bottom border row at one-minute boundaries within the
/// graph's horizontal span. Each character cell holds two samples, so the
/// character period is `60s / (interval * 2)`.
fn draw_time_ticks(f: &mut Frame, graph: Rect, secs_per_sample: f64) {
    if graph.width == 0 {
        return;
    }
    let chars_per_tick = (60.0 / (secs_per_sample.max(0.001) * 2.0)).round() as u16;
    if chars_per_tick == 0 {
        return;
    }
    let tick_y = graph.bottom(); // the row just below `inner` is the bottom frame
    let right_x = graph.right() - 1;
    let mut k: u16 = 1;
    loop {
        let Some(off) = chars_per_tick.checked_mul(k) else {
            break;
        };
        let Some(x) = right_x.checked_sub(off) else {
            break;
        };
        if x < graph.left() {
            break;
        }
        if let Some(cell) = f.buffer_mut().cell_mut((x, tick_y)) {
            cell.set_char('▴');
            cell.fg = Color::White;
        }
        k += 1;
    }
}

/// The left gutter: one color-coded readout per row, doubling as the legend
/// for the graph lines (CPU / memory / GPU util / GPU mem).
fn render_gutter(f: &mut Frame, area: Rect, snap: &Snapshot) {
    let has_gpu = !snap.gpus.is_empty();
    let (gpu_line, vram_line) = if has_gpu {
        (
            Line::from(bold(format!("GPU {:>3.0}%", mean_gpu_util(snap)), GPU_COLOR)),
            Line::from(bold(
                format!("VRM {:>3.0}%", mean_gpu_mem(snap) * 100.0),
                VRAM_COLOR,
            )),
        )
    } else {
        (
            Line::from(Span::styled("GPU   --", Style::new().fg(Color::DarkGray))),
            Line::from(Span::styled("VRM   --", Style::new().fg(Color::DarkGray))),
        )
    };
    let lines = vec![
        Line::from(bold(format!("CPU {:>3.0}%", snap.cpu.global_usage), CPU_COLOR)),
        Line::from(bold(
            format!("MEM {:>3.0}%", snap.memory.used_fraction() * 100.0),
            MEM_COLOR,
        )),
        gpu_line,
        vram_line,
    ];
    f.render_widget(Paragraph::new(lines), area);
}

/// Descending sort comparator for `f64` (NaN treated as equal).
fn desc(a: &f64, b: &f64) -> std::cmp::Ordering {
    b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal)
}

fn render_processes(f: &mut Frame, area: Rect, app: &App) {
    let snap = &app.snapshot;
    let show_gpu = !snap.gpus.is_empty();

    // Column headers ride the top frame as a left-aligned title; the process
    // count sits on the right. Headers use the reversed style of the previous
    // in-content header row so they read as a header bar.
    let headers = process_header_text(show_gpu);
    let header_title = Line::from(Span::styled(
        headers,
        Style::new()
            .fg(PROC_COLOR)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED),
    ));
    let count_title = Line::from(bold(format!(" {} procs ", snap.process_count), PROC_COLOR))
        .right_aligned();

    let block = Block::bordered()
        .border_style(Style::new().fg(PROC_COLOR))
        .title(header_title)
        .title(count_title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if !app.ready || inner.height == 0 {
        return;
    }

    // Highest CPU first.
    let mut procs: Vec<&ProcessMetrics> = snap.processes.iter().collect();
    procs.sort_by(|a, b| {
        b.cpu_usage
            .partial_cmp(&a.cpu_usage)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let rows = inner.height as usize;
    let lines: Vec<Line> = procs
        .into_iter()
        .take(rows)
        .map(|p| process_row(p, show_gpu, inner.width))
        .collect();

    f.render_widget(Paragraph::new(lines), inner);
}

/// Column-header text, matching the column widths used by [`process_row`] so
/// it aligns with the data when rendered on the top frame as a title.
fn process_header_text(show_gpu: bool) -> String {
    let gpu = if show_gpu {
        format!(" {:>9}", "GPU MEM")
    } else {
        String::new()
    };
    format!(
        "{:>7} {:<8} {:>5} {:>9}{} {}",
        "PID", "USER", "CPU%", "MEM", gpu, "COMMAND"
    )
}

/// One process row. The CPU% cell is colored by load (a process at >=100% — a
/// full core — reads as hot). The command is truncated to the remaining width.
fn process_row(p: &ProcessMetrics, show_gpu: bool, width: u16) -> Line<'static> {
    let user = truncate(p.user.as_deref().unwrap_or("?"), 8);
    let prefix = format!("{:>7} {:<8} ", p.pid, user);
    let cpu_s = format!("{:>5.1}", p.cpu_usage);
    let mem_s = format!(" {:>9}", format::bytes(p.memory));
    let gpu_s = if show_gpu {
        let g = p
            .gpu_memory
            .map(format::bytes)
            .unwrap_or_else(|| "-".to_string());
        format!(" {g:>9}")
    } else {
        String::new()
    };

    let used = prefix.chars().count()
        + cpu_s.chars().count()
        + mem_s.chars().count()
        + gpu_s.chars().count()
        + 1; // separating space before the command
    let cmd_w = (width as usize).saturating_sub(used);
    let cmd = truncate(&p.name, cmd_w);

    let cpu_frac = (p.cpu_usage as f64 / 100.0).clamp(0.0, 1.0);
    Line::from(vec![
        Span::raw(prefix),
        Span::styled(cpu_s, Style::new().fg(load_color(cpu_frac))),
        Span::raw(format!("{mem_s}{gpu_s} {cmd}")),
    ])
}

/// Truncate `s` to at most `max` characters, using an ellipsis when cut.
fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
