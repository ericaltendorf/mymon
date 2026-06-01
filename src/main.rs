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
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => app.should_quit = true,
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
