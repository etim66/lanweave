mod guard;
mod panic;

use std::io::{self, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::interaction::UiState;
use crate::app::model::AppModel;

use self::guard::{CrosstermControl, TerminalGuard};
use super::view;

pub(crate) use self::panic::install_panic_hook;

/// Whether the terminal session currently owns the alternate screen.
static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Owns the ratatui terminal and the RAII guard that restores the terminal.
pub(crate) struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    guard: TerminalGuard<CrosstermControl>,
}

impl TerminalSession {
    /// Enters the alternate screen, enables raw mode, and creates the terminal.
    pub(crate) fn start() -> io::Result<Self> {
        let guard = TerminalGuard::start(CrosstermControl, true)?;
        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        Ok(Self { terminal, guard })
    }

    /// Renders the current model and UI state to the terminal.
    ///
    /// Fails when the terminal session was already torn down.
    pub(crate) fn draw(&mut self, model: &AppModel, ui: &UiState) -> anyhow::Result<()> {
        if !TERMINAL_ACTIVE.load(Ordering::SeqCst) {
            anyhow::bail!("terminal session is no longer active");
        }
        self.terminal.draw(|frame| view::render(frame, model, ui))?;
        Ok(())
    }

    /// Restores the terminal and releases the alternate screen.
    pub(crate) fn restore(&mut self) -> io::Result<()> {
        self.guard.restore()
    }
}
