use std::io::{self, Stdout};
use std::panic;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::AppModel;

use super::view;

static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);
static INSTALL_PANIC_HOOK: Once = Once::new();

/// Installs process-wide best-effort terminal restoration before panic output.
pub fn install_panic_hook() {
    INSTALL_PANIC_HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            restore_after_panic();
            previous(panic_info);
        }));
    });
}

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

    pub(crate) fn draw(&mut self, model: &AppModel) -> anyhow::Result<()> {
        if !TERMINAL_ACTIVE.load(Ordering::SeqCst) {
            anyhow::bail!("terminal session is no longer active");
        }
        self.terminal.draw(|frame| view::render(frame, model))?;
        Ok(())
    }

    pub(crate) fn restore(&mut self) -> io::Result<()> {
        self.guard.restore()
    }
}

trait TerminalControl {
    fn enable_raw(&mut self) -> io::Result<()>;
    fn enter_alternate_screen(&mut self) -> io::Result<()>;
    fn hide_cursor(&mut self) -> io::Result<()>;
    fn show_cursor(&mut self) -> io::Result<()>;
    fn leave_alternate_screen(&mut self) -> io::Result<()>;
    fn disable_raw(&mut self) -> io::Result<()>;
}

struct CrosstermControl;

impl TerminalControl for CrosstermControl {
    fn enable_raw(&mut self) -> io::Result<()> {
        enable_raw_mode()
    }

    fn enter_alternate_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnterAlternateScreen)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        execute!(io::stdout(), Hide)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(io::stdout(), Show)
    }

    fn leave_alternate_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), LeaveAlternateScreen)
    }

    fn disable_raw(&mut self) -> io::Result<()> {
        disable_raw_mode()
    }
}

struct TerminalGuard<C: TerminalControl> {
    control: C,
    raw: bool,
    alternate_screen: bool,
    cursor_hidden: bool,
    marks_process_active: bool,
}

impl<C: TerminalControl> TerminalGuard<C> {
    fn start(control: C, marks_process_active: bool) -> io::Result<Self> {
        let mut guard = Self {
            control,
            raw: false,
            alternate_screen: false,
            cursor_hidden: false,
            marks_process_active,
        };

        guard.raw = true;
        guard.control.enable_raw()?;
        if marks_process_active {
            TERMINAL_ACTIVE.store(true, Ordering::SeqCst);
        }

        guard.alternate_screen = true;
        guard.control.enter_alternate_screen()?;
        guard.cursor_hidden = true;
        guard.control.hide_cursor()?;
        Ok(guard)
    }

    fn restore(&mut self) -> io::Result<()> {
        let mut first_error = None;
        if self.cursor_hidden {
            match self.control.show_cursor() {
                Ok(()) => self.cursor_hidden = false,
                Err(error) => remember_first_error(&mut first_error, error),
            }
        }
        if self.alternate_screen {
            match self.control.leave_alternate_screen() {
                Ok(()) => self.alternate_screen = false,
                Err(error) => remember_first_error(&mut first_error, error),
            }
        }
        if self.raw {
            match self.control.disable_raw() {
                Ok(()) => self.raw = false,
                Err(error) => remember_first_error(&mut first_error, error),
            }
        }

        if self.marks_process_active {
            let still_active = self.cursor_hidden || self.alternate_screen || self.raw;
            TERMINAL_ACTIVE.store(still_active, Ordering::SeqCst);
        }

        first_error.map_or(Ok(()), Err)
    }
}

impl<C: TerminalControl> Drop for TerminalGuard<C> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn remember_first_error(first_error: &mut Option<io::Error>, error: io::Error) {
    if first_error.is_none() {
        *first_error = Some(error);
    }
}

fn restore_after_panic() {
    if !TERMINAL_ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }

    // Each step is independent so one failed write cannot skip raw-mode cleanup.
    let _ = execute!(io::stdout(), Show);
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io;
    use std::rc::Rc;

    use super::{TerminalControl, TerminalGuard};

    #[derive(Clone)]
    struct FakeControl {
        calls: Rc<RefCell<Vec<&'static str>>>,
        fail_on: Option<&'static str>,
        failures_remaining: usize,
    }

    impl FakeControl {
        fn new(fail_on: Option<&'static str>) -> (Self, Rc<RefCell<Vec<&'static str>>>) {
            let calls = Rc::new(RefCell::new(Vec::new()));
            (
                Self {
                    calls: Rc::clone(&calls),
                    fail_on,
                    failures_remaining: usize::from(fail_on.is_some()),
                },
                calls,
            )
        }

        fn call(&mut self, name: &'static str) -> io::Result<()> {
            self.calls.borrow_mut().push(name);
            if self.fail_on == Some(name) && self.failures_remaining > 0 {
                self.failures_remaining -= 1;
                Err(io::Error::other("injected terminal failure"))
            } else {
                Ok(())
            }
        }
    }

    impl TerminalControl for FakeControl {
        fn enable_raw(&mut self) -> io::Result<()> {
            self.call("enable_raw")
        }

        fn enter_alternate_screen(&mut self) -> io::Result<()> {
            self.call("enter_alternate")
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            self.call("hide_cursor")
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            self.call("show_cursor")
        }

        fn leave_alternate_screen(&mut self) -> io::Result<()> {
            self.call("leave_alternate")
        }

        fn disable_raw(&mut self) -> io::Result<()> {
            self.call("disable_raw")
        }
    }

    #[test]
    fn restore_reverses_setup_and_is_idempotent() {
        let (control, calls) = FakeControl::new(None);
        let mut guard = TerminalGuard::start(control, false).unwrap();

        guard.restore().unwrap();
        guard.restore().unwrap();
        drop(guard);

        assert_eq!(
            *calls.borrow(),
            [
                "enable_raw",
                "enter_alternate",
                "hide_cursor",
                "show_cursor",
                "leave_alternate",
                "disable_raw",
            ]
        );
    }

    #[test]
    fn every_partial_setup_failure_is_rolled_back() {
        let cases = [
            ("enable_raw", vec!["enable_raw", "disable_raw"]),
            (
                "enter_alternate",
                vec![
                    "enable_raw",
                    "enter_alternate",
                    "leave_alternate",
                    "disable_raw",
                ],
            ),
            (
                "hide_cursor",
                vec![
                    "enable_raw",
                    "enter_alternate",
                    "hide_cursor",
                    "show_cursor",
                    "leave_alternate",
                    "disable_raw",
                ],
            ),
        ];

        for (failure, expected) in cases {
            let (control, calls) = FakeControl::new(Some(failure));
            assert!(TerminalGuard::start(control, false).is_err());
            assert_eq!(*calls.borrow(), expected, "failure: {failure}");
        }
    }

    #[test]
    fn cleanup_continues_after_an_error() {
        let (control, calls) = FakeControl::new(Some("show_cursor"));
        let mut guard = TerminalGuard::start(control, false).unwrap();

        assert!(guard.restore().is_err());
        drop(guard);
        assert_eq!(
            *calls.borrow(),
            [
                "enable_raw",
                "enter_alternate",
                "hide_cursor",
                "show_cursor",
                "leave_alternate",
                "disable_raw",
                "show_cursor",
            ]
        );
    }

    #[test]
    fn drop_retries_each_failed_cleanup_step() {
        for failure in ["show_cursor", "leave_alternate", "disable_raw"] {
            let (control, calls) = FakeControl::new(Some(failure));
            let mut guard = TerminalGuard::start(control, false).unwrap();

            assert!(guard.restore().is_err());
            drop(guard);

            assert_eq!(
                calls
                    .borrow()
                    .iter()
                    .filter(|call| **call == failure)
                    .count(),
                2,
                "failure: {failure}"
            );
        }
    }

    #[test]
    fn unwinding_drops_the_guard_and_restores_terminal() {
        let (control, calls) = FakeControl::new(None);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = TerminalGuard::start(control, false).unwrap();
            panic!("injected panic");
        }));

        assert!(result.is_err());
        assert_eq!(calls.borrow().last(), Some(&"disable_raw"));
    }
}
