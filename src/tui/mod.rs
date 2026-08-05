//! Terminal user interface rendering and event capture.
//!
//! Owns the RAII terminal guard, alternate-screen lifecycle, and the basic
//! screen views (browsing, empty-device, error, shutdown).

mod event;
mod terminal;
mod view;

pub(crate) use event::run_events;
pub(crate) use terminal::TerminalSession;
pub use terminal::install_panic_hook;
