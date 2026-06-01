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

fn main() -> Result<()> {
    // Default sample cadence; never faster than what sysinfo needs for accurate
    // CPU figures.
    let tick_rate = Duration::from_secs(1).max(monitor::MIN_REFRESH_INTERVAL);

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, App::new(tick_rate));
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, mut app: App) -> Result<()> {
    // Prime the first sample so the graphs start populating immediately.
    while !app.should_quit {
        terminal.draw(|f| ui::render(f, &app))?;

        // Wait for input until the next tick is due.
        let timeout = app
            .tick_rate
            .checked_sub(app.last_tick.elapsed())
            .unwrap_or_default();

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(&mut app, key.code, key.modifiers);
                }
            }
        }

        if app.last_tick.elapsed() >= app.tick_rate {
            app.on_tick();
        }
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
        let mut app = App::new(Duration::from_millis(50));
        app.on_tick();
        sleep(monitor::MIN_REFRESH_INTERVAL);
        app.on_tick();

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
}
