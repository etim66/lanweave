//! Lanweave application library.
//!
//! The executable uses the small [`run`] facade. Implementation modules stay
//! private so their boundaries can evolve without creating an accidental API.

mod app;
mod bootstrap;
mod discovery;
mod framing;
mod pairing;
mod protocol;
mod session;
mod storage;
mod transfer;
mod transport;
mod tui;

/// Starts Lanweave and runs it until shutdown completes.
pub fn run() -> anyhow::Result<()> {
    tui::install_panic_hook();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(bootstrap::run())
}
