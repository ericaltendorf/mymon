//! Top-level TUI layout and per-block rendering.
//!
//! Layout (top to bottom):
//! - **CPU/GPU** block: a shared history graph (avg CPU and avg GPU as two
//!   colored lines) on the left, and on the right a bar chart of per-core CPU
//!   utilization, a blank column, then one bar per GPU.
//! - **Memory** block: a single full-width history graph.

mod braille;
mod charts;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, mean_gpu_util};
use crate::format;
use crate::metrics::ProcessMetrics;

use charts::{bar_chart, history_line, history_multi, load_color};

const CPU_COLOR: Color = Color::Cyan;
const GPU_COLOR: Color = Color::Magenta;
const MEM_COLOR: Color = Color::Blue;
const PROC_COLOR: Color = Color::Gray;

/// Render the whole UI for one frame.
pub fn render(f: &mut Frame, app: &App) {
    // The two stat blocks stay compact (up to 4 rows each); everything left
    // over flows into the process list at the bottom.
    let [top_area, mem_area, proc_area] = Layout::vertical([
        Constraint::Max(4),
        Constraint::Max(4),
        Constraint::Fill(1),
    ])
    .areas(f.area());

    render_cpu_gpu(f, top_area, app);
    render_memory(f, mem_area, app);
    render_processes(f, proc_area, app);
}

/// A bordered block with a composed title; returns the inner drawing area.
fn bordered(f: &mut Frame, area: Rect, title: Line<'_>, accent: Color) -> Rect {
    let block = Block::bordered()
        .border_style(Style::new().fg(accent))
        .title(title);
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

fn bold(text: impl Into<String>, color: Color) -> Span<'static> {
    Span::styled(
        text.into(),
        Style::new().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn render_cpu_gpu(f: &mut Frame, area: Rect, app: &App) {
    let Some(snap) = &app.snapshot else {
        let _ = bordered(f, area, Line::from(bold(" CPU / GPU ", CPU_COLOR)), CPU_COLOR);
        return;
    };
    let cpu = &snap.cpu;
    let has_gpu = !snap.gpus.is_empty();

    // --- Title: CPU summary (cyan), then GPU summary (magenta) if present. ---
    let mut spans = vec![bold(
        format!(
            " CPU {:.0}%  {}c/{}p  load {:.2} {:.2} {:.2} ",
            cpu.global_usage,
            cpu.logical_cores(),
            cpu.physical_cores.unwrap_or(0),
            cpu.load_average.one,
            cpu.load_average.five,
            cpu.load_average.fifteen,
        ),
        CPU_COLOR,
    )];
    if has_gpu {
        spans.push(Span::raw("│"));
        spans.push(bold(
            format!(" GPU {:.0}%  {} ", mean_gpu_util(snap), snap.gpus[0].name),
            GPU_COLOR,
        ));
    }
    let inner = bordered(f, area, Line::from(spans), CPU_COLOR);

    // --- Bar values, each sorted high -> low for a descending staircase. ---
    let mut cpu_bars: Vec<f64> = cpu.per_core.iter().map(|c| c.usage as f64 / 100.0).collect();
    cpu_bars.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    let mut gpu_bars: Vec<f64> = snap
        .gpus
        .iter()
        .map(|g| g.utilization_gpu.unwrap_or(0) as f64 / 100.0)
        .collect();
    gpu_bars.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    // --- Width budget for the right (bars) side. ---
    // CPU: one dot-column per core => ceil(n/2) cells.
    let cpu_chars = (cpu_bars.len() as u16).div_ceil(2);
    // GPU: each device is a full character (2 dot-columns) wide.
    let gpu_chars = gpu_bars.len() as u16;
    // +1 blank column separating the two groups when both are present.
    let bars_w = if has_gpu && gpu_chars > 0 {
        cpu_chars + 1 + gpu_chars
    } else {
        cpu_chars
    };

    let (graph_area, bars_area) = split_left_right(inner, bars_w);

    // --- Left: shared history plot (CPU cyan, GPU magenta). ---
    let cpu_hist = app.cpu_history.to_vec();
    let gpu_hist = app.gpu_history.to_vec();
    let mut series: Vec<(&[f64], Color)> = vec![(&cpu_hist, CPU_COLOR)];
    if has_gpu {
        series.push((&gpu_hist, GPU_COLOR));
    }
    history_multi(graph_area, f.buffer_mut(), &series, 100.0);

    // --- Right: CPU bars, blank column, then GPU bars. ---
    if has_gpu && gpu_chars > 0 {
        let [cpu_area, _blank, gpu_area] = Layout::horizontal([
            Constraint::Length(cpu_chars),
            Constraint::Length(1),
            Constraint::Length(gpu_chars),
        ])
        .areas(bars_area);
        bar_chart(cpu_area, f.buffer_mut(), &cpu_bars, 1, 0);
        bar_chart(gpu_area, f.buffer_mut(), &gpu_bars, 2, 0);
    } else {
        bar_chart(bars_area, f.buffer_mut(), &cpu_bars, 1, 0);
    }
}

fn render_memory(f: &mut Frame, area: Rect, app: &App) {
    let title = match &app.snapshot {
        Some(snap) => {
            let m = &snap.memory;
            format!(
                " MEM {} / {} ({:.0}%)   SWAP {} / {} ({:.0}%) ",
                format::bytes(m.used),
                format::bytes(m.total),
                m.used_fraction() * 100.0,
                format::bytes(m.swap_used),
                format::bytes(m.swap_total),
                m.swap_used_fraction() * 100.0,
            )
        }
        None => " MEM ".into(),
    };
    let inner = bordered(f, area, Line::from(bold(title, MEM_COLOR)), MEM_COLOR);
    history_line(inner, f.buffer_mut(), &app.mem_history.to_vec(), 100.0);
}

fn render_processes(f: &mut Frame, area: Rect, app: &App) {
    let count = app.snapshot.as_ref().map_or(0, |s| s.processes.len());
    let title = format!(" PROCESSES  {count} ");
    let inner = bordered(f, area, Line::from(bold(title, PROC_COLOR)), PROC_COLOR);

    let Some(snap) = &app.snapshot else { return };
    if inner.height == 0 {
        return;
    }

    let show_gpu = !snap.gpus.is_empty();

    // Highest CPU first.
    let mut procs: Vec<&ProcessMetrics> = snap.processes.iter().collect();
    procs.sort_by(|a, b| {
        b.cpu_usage
            .partial_cmp(&a.cpu_usage)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut lines: Vec<Line> = Vec::with_capacity(inner.height as usize);
    lines.push(process_header(show_gpu));
    let rows = (inner.height as usize).saturating_sub(1);
    for p in procs.into_iter().take(rows) {
        lines.push(process_row(p, show_gpu, inner.width));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

/// Header row, matching the column widths used by [`process_row`].
fn process_header(show_gpu: bool) -> Line<'static> {
    let gpu = if show_gpu {
        format!(" {:>9}", "GPU MEM")
    } else {
        String::new()
    };
    let text = format!(
        "{:>7} {:<8} {:>5} {:>9}{} {}",
        "PID", "USER", "CPU%", "MEM", gpu, "COMMAND"
    );
    Line::from(Span::styled(
        text,
        Style::new()
            .fg(PROC_COLOR)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED),
    ))
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

/// Split `inner` into `(graph, bars)` with a one-column gap, reserving
/// `bars_w` columns for the bar panel (clamped so the graph keeps room).
fn split_left_right(inner: Rect, bars_w: u16) -> (Rect, Rect) {
    let bars = bars_w.min(inner.width.saturating_sub(6));
    let [graph, _gap, bars] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(bars),
    ])
    .areas(inner);
    (graph, bars)
}
