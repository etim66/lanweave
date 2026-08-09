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

static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);

pub(crate) struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    guard: TerminalGuard<CrosstermControl>,
}

impl TerminalSession {
    pub(crate) fn start() -> io::Result<Self> {
        let guard = TerminalGuard::start(CrosstermControl, true)?;
        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        Ok(Self { terminal, guard })
    }

    pub(crate) fn draw(&mut self, model: &AppModel, ui: &UiState) -> anyhow::Result<()> {
        if !TERMINAL_ACTIVE.load(Ordering::SeqCst) {
            anyhow::bail!("terminal session is no longer active");
        }
        self.terminal.draw(|frame| view::render(frame, model, ui))?;
        Ok(())
    }

    pub(crate) fn restore(&mut self) -> io::Result<()> {
        self.guard.restore()
    }
}
