use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::time::Duration;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::Print;
use crossterm::terminal::{
    self, Clear, ClearType, DisableLineWrap, EnableLineWrap, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};

use super::{Key, TerminalFlow};
use crate::inspection::{InspectionSession, ScanControl};

struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(io::stdout(), EnterAlternateScreen, DisableLineWrap, Hide)?;
        Ok(guard)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, EnableLineWrap, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

/// Runs the terminal adapter over the same session as the web adapter.
/// # Errors
/// Returns terminal or worker-start errors after restoring terminal state.
pub fn run(
    session: Arc<InspectionSession>,
    budget: crate::inspection::DeepBudget,
) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "terminal mode requires an interactive input and output terminal",
        ));
    }
    let guard = TerminalGuard::enter()?;
    let worker_session = Arc::clone(&session);
    let worker = std::thread::Builder::new()
        .name("terminal-inspection".into())
        .spawn(move || worker_session.scan(|_| ScanControl::Continue))?;
    let mut flow = TerminalFlow::new(session).with_deep_budget(budget);
    let result = event_loop(&mut flow);
    flow.cancel_work();
    drop(guard);
    let _ = worker.join();
    result
}

fn event_loop(flow: &mut TerminalFlow) -> io::Result<()> {
    let mut output = io::stdout();
    while !flow.quit {
        let (width, height) = terminal::size()?;
        queue!(output, MoveTo(0, 0), Clear(ClearType::All))?;
        for (row, line) in flow.screen(width, height).into_iter().enumerate() {
            queue!(
                output,
                MoveTo(0, u16::try_from(row).unwrap_or(u16::MAX)),
                Print(line)
            )?;
        }
        output.flush()?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(event) = event::read()?
        {
            if event.kind == KeyEventKind::Release {
                continue;
            }
            let key = match event.code {
                KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::CONTROL) => Key::Quit,
                KeyCode::Char(c) => Key::Char(c),
                KeyCode::Up => Key::Up,
                KeyCode::Down => Key::Down,
                KeyCode::Enter | KeyCode::Right => Key::Enter,
                KeyCode::Esc | KeyCode::Backspace | KeyCode::Left => Key::Back,
                KeyCode::PageUp => Key::PageUp,
                KeyCode::PageDown => Key::PageDown,
                _ => continue,
            };
            flow.key(key);
        }
    }
    Ok(())
}
