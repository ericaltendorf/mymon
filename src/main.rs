//! mymon — a Linux system monitor with a custom braille TUI.

mod app;
mod format;
mod metrics;
mod monitor;
mod ui;

use std::time::Duration;

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use app::App;

/// Read a millisecond duration from an env var, falling back to `default`.
fn interval_from_env(var: &str, default: Duration) -> Duration {
    std::env::var(var)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(default)
        .max(monitor::MIN_REFRESH_INTERVAL)
}

fn main() -> Result<()> {
    // Cheap stats drive the graphs; the expensive process scan runs less often.
    // Both are overridable for big machines where the /proc scan is pricey.
    let stats_interval = interval_from_env("MYMON_INTERVAL_MS", Duration::from_secs(1));
    let process_interval = interval_from_env("MYMON_PROC_INTERVAL_MS", Duration::from_secs(2));

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, App::new(stats_interval, process_interval));
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, mut app: App) -> Result<()> {
    while !app.should_quit {
        terminal.draw(|f| ui::render(f, &app))?;

        // Block until the next tick is due (or an input event arrives), so the
        // process is asleep the rest of the time.
        if event::poll(app.time_until_next_tick())? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(&mut app, key.code, key.modifiers);
                }
            }
        }

        app.update();
    }
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    // Ctrl-C always quits, even mid-prompt.
    if let KeyCode::Char('c') = code {
        if mods.contains(KeyModifiers::CONTROL) {
            app.should_quit = true;
            return;
        }
    }

    // A pending kill-confirm prompt swallows the next keystroke: 'y' commits,
    // anything else (including q/Esc) cancels without killing or quitting.
    if app.kill_prompt.is_some() {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_kill(),
            _ => app.cancel_kill(),
        }
        return;
    }

    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Up => app.move_selection(-1),
        KeyCode::Down => app.move_selection(1),
        KeyCode::Tab => app.toggle_pane(),
        KeyCode::Char('k') => app.request_kill_selected(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::thread::sleep;

    /// End-to-end render smoke test: sample real metrics twice, draw into an
    /// in-memory backend, and confirm braille glyphs were emitted.
    #[test]
    fn renders_braille_into_buffer() {
        let mut app = App::new(Duration::from_millis(50), Duration::from_millis(50));
        sleep(monitor::MIN_REFRESH_INTERVAL);
        app.on_stats_tick();

        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        terminal.draw(|f| ui::render(f, &app)).unwrap();

        let buf = terminal.backend().buffer();
        let braille = buf
            .content
            .iter()
            .filter(|c| {
                c.symbol()
                    .chars()
                    .next()
                    .is_some_and(|ch| ('\u{2800}'..='\u{28ff}').contains(&ch))
            })
            .count();

        assert!(braille > 0, "expected braille glyphs to be rendered");
    }

    /// Print a synthetic render to stdout so the README sample can be lifted
     /// verbatim. Hidden behind `--nocapture --ignored` so it doesn't run by
     /// default. Invoke with:
     ///   cargo test --release dump_sample_render -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_sample_render() {
        use crate::metrics::*;
        let mut app = App::new(Duration::from_secs(1), Duration::from_secs(2));
        sleep(monitor::MIN_REFRESH_INTERVAL);
        app.on_stats_tick();

        // Overwrite with synthetic, deterministic data.
        let mut snap = Snapshot::default();
        snap.host.hostname = Some("devbox".to_string());
        snap.host.uptime = Duration::from_secs(2 * 86_400 + 14 * 3_600 + 23 * 60 + 11);
        snap.cpu.brand = "AMD Ryzen 9 5950X 16-Core Processor".to_string();
        snap.cpu.global_usage = 47.0;
        snap.cpu.per_core = (0..16)
            .map(|i| {
                let pattern = [62, 18, 41, 9, 84, 23, 55, 12, 71, 28, 47, 14, 33, 22, 8, 5];
                CoreMetrics {
                    name: format!("cpu{i}"),
                    usage: pattern[i] as f32,
                    frequency_mhz: 4200,
                }
            })
            .collect();
        snap.memory = MemoryMetrics {
            total: 64 * 1024 * 1024 * 1024,
            used: 38 * 1024 * 1024 * 1024,
            available: 26 * 1024 * 1024 * 1024,
            ..MemoryMetrics::default()
        };
        snap.gpus = (0..4)
            .map(|i| {
                let utils = [82, 77, 31, 71];
                let mem_used: [u64; 4] = [42, 18, 9, 35];
                GpuMetrics {
                    index: i as u32,
                    name: "NVIDIA RTX A6000".to_string(),
                    utilization_gpu: Some(utils[i]),
                    memory_total: 48 * 1024 * 1024 * 1024,
                    memory_used: mem_used[i] * 1024 * 1024 * 1024,
                    ..GpuMetrics::default()
                }
            })
            .collect();
        snap.processes = vec![
            ("train.py", "alice", 312.4, 18.2),
            ("rustc", "alice", 198.7, 4.6),
            ("cargo", "alice", 96.3, 1.1),
            ("clangd", "bob", 47.8, 2.4),
            ("firefox", "alice", 22.1, 3.8),
            ("zsh", "alice", 0.4, 0.012),
            ("vim", "bob", 0.3, 0.015),
            ("tmux", "alice", 0.1, 0.008),
            ("systemd", "root", 0.1, 0.020),
            ("dbus-daemon", "root", 0.0, 0.009),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (name, user, cpu, mem_gib))| ProcessMetrics {
            pid: 1000 + i as u32 * 137,
            name: name.to_string(),
            user: Some(user.to_string()),
            cpu_usage: cpu,
            memory: (mem_gib * (1024.0 * 1024.0 * 1024.0)) as u64,
            ..ProcessMetrics::default()
        })
        .collect();
        snap.process_count = 423;
        app.snapshot = snap;

        // Fabricate plausible-looking histories: enough samples to fill the
        // graph width with a gentle oscillation.
        app.cpu_history = crate::app::History::default();
        app.mem_history = crate::app::History::default();
        app.gpu_history = crate::app::History::default();
        app.gpu_mem_history = crate::app::History::default();
        for i in 0..400 {
            let t = i as f64 * 0.05;
            app.cpu_history.push(40.0 + 22.0 * t.sin() + 8.0 * (t * 0.7).cos());
            app.mem_history.push(55.0 + 6.0 * (t * 0.3).sin());
            app.gpu_history.push(60.0 + 18.0 * (t * 0.4).cos());
            app.gpu_mem_history.push(48.0 + 4.0 * (t * 0.2).sin());
        }

        let mut terminal = Terminal::new(TestBackend::new(120, 22)).unwrap();
        terminal.draw(|f| ui::render(f, &app)).unwrap();

        let buf = terminal.backend().buffer();
        for y in 0..buf.area.height {
            let mut row = String::new();
            for x in 0..buf.area.width {
                row.push_str(buf[(x, y)].symbol());
            }
            // Trim trailing spaces so README diffs are clean.
            let trimmed = row.trim_end();
            println!("{trimmed}");
        }
    }

    #[test]
    fn model_number_examples() {
        assert_eq!(
            format::model_number("13th Gen Intel(R) Core(TM) i7-1370P"),
            "i7-1370P"
        );
        assert_eq!(
            format::model_number("AMD Ryzen 9 5950X 16-Core Processor"),
            "5950X"
        );
        assert_eq!(
            format::model_number("Intel(R) Xeon(R) Gold 6248 CPU @ 2.50GHz"),
            "6248"
        );
        assert_eq!(format::model_number("NVIDIA RTX A6000"), "A6000");
        assert_eq!(format::model_number("NVIDIA GeForce RTX 4090"), "4090");
    }
}
