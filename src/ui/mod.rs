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

use crate::app::{App, ProcSort, mean_gpu_mem, mean_gpu_util};
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
    // The overview block stays compact (up to 4 content rows); the status bar
    // at the bottom is always 1 row; everything left over flows into the
    // process list in the middle.
    let [top_area, proc_area, status_area] = Layout::vertical([
        Constraint::Max(STAT_BLOCK_ROWS),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(f.area());

    render_overview(f, top_area, app);
    render_processes(f, proc_area, app);
    render_status_bar(f, status_area, app);
}

/// Bottom status row: either a kill-confirm prompt or terse key hints.
fn render_status_bar(f: &mut Frame, area: Rect, app: &App) {
    let line = if let Some((pid, name)) = &app.kill_prompt {
        Line::from(Span::styled(
            format!(" kill pid {pid} ({name}) — send SIGTERM? [y/N] "),
            Style::new()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        ))
    } else {
        Line::from(Span::styled(
            " ↑/↓ select · tab pane · k kill · q quit ",
            Style::new().fg(Color::DarkGray),
        ))
    };
    f.render_widget(Paragraph::new(line), area);
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

    // Title: hostname, CPU model (in CPU color), total RAM (in MEM color),
    // GPU summary (in GPU color), uptime. The accent colors echo each
    // metric's bar/graph color.
    let host = snap.host.hostname.as_deref().unwrap_or("mymon");
    let cpu_model = format::model_number(&cpu.brand);
    let gpu_summary = format::gpu_summary(&snap.gpus);
    let uptime = format::duration(snap.host.uptime);
    let (ram_num, ram_unit) = format::bytes_short(mem.total);
    let ram_str = format!("{ram_num}{ram_unit}");
    let mut title_spans = vec![
        bold(format!(" {host} · "), FRAME_COLOR),
        bold(cpu_model, CPU_COLOR),
        bold(" · ", FRAME_COLOR),
        bold(ram_str, MEM_COLOR),
    ];
    if !gpu_summary.is_empty() {
        title_spans.push(bold(" · ", FRAME_COLOR));
        title_spans.push(bold(gpu_summary, GPU_COLOR));
    }
    title_spans.push(bold(
        format!(" · {} procs · up {uptime} ", snap.process_count),
        FRAME_COLOR,
    ));

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
    // CPU cores at 1 dot per core; system memory at 2 dots (1 char). GPU util
    // and GPU memory are 1 dot per device when there are multiple GPUs (so 4
    // GPUs collapse into 2 text cells per group), but bump to 2 dots when
    // there's only one GPU so a single device still reads cleanly.
    let cpu_chars = (cpu_bars.len() as u16).div_ceil(2);
    let mem_chars = 1u16;
    let gpu_bar_dots: u32 = if gpu_bars.len() <= 1 { 2 } else { 1 };
    let gpu_chars = ((gpu_bars.len() as u32 * gpu_bar_dots).div_ceil(2)) as u16;
    let gpu_mem_chars = ((gpu_mem_bars.len() as u32 * gpu_bar_dots).div_ceil(2)) as u16;
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
        bar_chart(segs[4], f.buffer_mut(), &gpu_bars, gpu_bar_dots, 0, GPU_COLOR);
        bar_chart(
            segs[6],
            f.buffer_mut(),
            &gpu_mem_bars,
            gpu_bar_dots,
            0,
            VRAM_COLOR,
        );
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

/// Below this width we collapse to a single CPU-sorted pane; at or above we
/// split into side-by-side CPU- and MEM-sorted panes.
const PROC_DUAL_MIN_WIDTH: u16 = 110;

fn render_processes(f: &mut Frame, area: Rect, app: &App) {
    if area.width >= PROC_DUAL_MIN_WIDTH {
        let [left, right] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(area);
        render_process_pane(f, left, app, ProcSort::Cpu);
        render_process_pane(f, right, app, ProcSort::Mem);
    } else {
        render_process_pane(f, area, app, ProcSort::Cpu);
    }
}

fn render_process_pane(f: &mut Frame, area: Rect, app: &App, sort: ProcSort) {
    let snap = &app.snapshot;
    let show_gpu = !snap.gpus.is_empty();

    // Column headers ride the top frame as the block title. Spaces between
    // and around the labels become `─` so the frame appears to flow through
    // the gaps, with the label words sitting on top of it.
    let header_text: String = process_header_text(show_gpu)
        .chars()
        .map(|c| if c == ' ' { '─' } else { c })
        .collect();
    let header_title = Line::from(Span::styled(
        header_text,
        Style::new().fg(PROC_COLOR).add_modifier(Modifier::BOLD),
    ));

    let block = Block::bordered()
        .border_style(Style::new().fg(PROC_COLOR))
        .title(header_title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if !app.ready || inner.height == 0 {
        return;
    }

    let mut procs: Vec<&ProcessMetrics> = snap.processes.iter().collect();
    match sort {
        ProcSort::Cpu => procs.sort_by(|a, b| {
            b.cpu_usage
                .partial_cmp(&a.cpu_usage)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        ProcSort::Mem => procs.sort_by(|a, b| b.memory.cmp(&a.memory)),
    }

    let rows = inner.height as usize;
    let total_ram = snap.memory.total;
    let pane_is_active = app.active_pane == sort;
    let lines: Vec<Line> = procs
        .into_iter()
        .take(rows)
        .enumerate()
        .map(|(i, p)| {
            let selected = pane_is_active && i == app.selected_index;
            process_row(p, show_gpu, inner.width, sort, total_ram, selected)
        })
        .collect();

    f.render_widget(Paragraph::new(lines), inner);
}

/// Width of one memory cell — 4 chars for the number, 1 for the unit suffix.
const MEM_CELL_W: usize = 5;

/// Column-header text, matching the column widths used by [`process_row`] so
/// it aligns with the data when rendered on the top frame as a title.
fn process_header_text(show_gpu: bool) -> String {
    let gpu = if show_gpu {
        format!(" {:>w$}", "VRAM", w = MEM_CELL_W)
    } else {
        String::new()
    };
    format!(
        "{:>7} {:<8} {:>5} {:>w$}{} {}",
        "PID",
        "USER",
        "CPU%",
        "MEM",
        gpu,
        "COMMAND",
        w = MEM_CELL_W
    )
}

/// One process row. The "active" column for the pane's sort dimension is
/// colored by load — CPU% by per-core saturation in the CPU pane, MEM by
/// fraction of total system RAM in the MEM pane. Memory cells split the
/// number and the unit suffix into separate spans so the suffix can be
/// dimmed. The command is truncated to the remaining width.
fn process_row(
    p: &ProcessMetrics,
    show_gpu: bool,
    width: u16,
    sort: ProcSort,
    total_ram: u64,
    selected: bool,
) -> Line<'static> {
    let user = truncate(p.user.as_deref().unwrap_or("?"), 8);
    let prefix = format!("{:>7} {:<8} ", p.pid, user);
    // Drop the decimal once a process is using a full ten cores or more so
    // a maxed-out 64-core box reads as " 6400" rather than overflowing the
    // 5-char column.
    let cpu_s = if p.cpu_usage >= 1000.0 {
        format!("{:>5.0}", p.cpu_usage)
    } else {
        format!("{:>5.1}", p.cpu_usage)
    };

    let (mem_num, mem_unit) = format::bytes_short(p.memory);
    let mem_num_str = format!(" {mem_num:>4}");

    // gpu memory: either "<num><unit>" or "    -" with no unit
    let (gpu_num_str, gpu_unit) = if show_gpu {
        if let Some(g) = p.gpu_memory {
            let (n, u) = format::bytes_short(g);
            (format!(" {n:>4}"), u)
        } else {
            (format!(" {:>w$}", "-", w = MEM_CELL_W), "")
        }
    } else {
        (String::new(), "")
    };

    let used = prefix.chars().count()
        + cpu_s.chars().count()
        + mem_num_str.chars().count()
        + mem_unit.len()
        + gpu_num_str.chars().count()
        + gpu_unit.len()
        + 1; // separating space before the command
    let cmd_w = (width as usize).saturating_sub(used);
    let cmd = truncate(&p.name, cmd_w);

    let unit_style = Style::new().fg(Color::DarkGray);
    let (cpu_style, mem_style) = match sort {
        ProcSort::Cpu => {
            let cpu_frac = (p.cpu_usage as f64 / 100.0).clamp(0.0, 1.0);
            (Style::new().fg(load_color(cpu_frac)), Style::new())
        }
        ProcSort::Mem => {
            let mem_frac = if total_ram == 0 {
                0.0
            } else {
                p.memory as f64 / total_ram as f64
            };
            (Style::new(), Style::new().fg(mem_load_color(mem_frac)))
        }
    };
    let line = Line::from(vec![
        Span::raw(prefix),
        Span::styled(cpu_s, cpu_style),
        Span::styled(mem_num_str, mem_style),
        Span::styled(mem_unit, unit_style),
        Span::raw(gpu_num_str),
        Span::styled(gpu_unit, unit_style),
        Span::raw(format!(" {cmd}")),
    ]);
    if selected {
        line.patch_style(Style::new().bg(Color::DarkGray))
    } else {
        line
    }
}

/// Banded color for a process's share of total system memory: green at idle,
/// yellow at >=10%, orange at >=25%, red at >=50%.
fn mem_load_color(frac: f64) -> Color {
    if frac >= 0.50 {
        Color::Red
    } else if frac >= 0.25 {
        Color::Rgb(255, 140, 0)
    } else if frac >= 0.10 {
        Color::Yellow
    } else {
        Color::Green
    }
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
