use std::io;
use std::panic;
use std::sync::Once;
use std::sync::atomic::Ordering;

use crossterm::cursor::Show;
use crossterm::execute;
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};

use super::TERMINAL_ACTIVE;

/// Guards against installing the panic hook more than once.
static INSTALL_PANIC_HOOK: Once = Once::new();

/// Installs process-wide best-effort terminal restoration before panic output.
pub(crate) fn install_panic_hook() {
    INSTALL_PANIC_HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            restore_after_panic();
            previous(panic_info);
        }));
    });
}

/// Restores the terminal after a panic, if the session was still active.
fn restore_after_panic() {
    if !TERMINAL_ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }

    // Each step is independent so one failed write cannot skip raw-mode cleanup.
    let _ = execute!(io::stdout(), Show);
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    let _ = disable_raw_mode();
}
