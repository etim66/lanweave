//! Lanweave application library.
//!
//! The executable uses the small [`run`] facade. Implementation modules stay
//! private so their boundaries can evolve without creating an accidental API.

mod app;
mod bootstrap;
mod discovery;
mod framing;
mod hostname;
mod pairing;
mod protocol;
mod session;
mod storage;
mod transfer;
mod transport;
mod tui;
mod update;

/// Entry points for the `fuzz/` harness build.
///
/// The wire codecs stay private to the crate; this shim is compiled only with
/// the `fuzz` feature so fuzz targets can reach them without widening the
/// library API for normal builds.
#[cfg(feature = "fuzz")]
pub mod fuzzing {
    pub use crate::framing::{Frame, FrameError, decode};
}

/// Starts Lanweave and runs it until shutdown completes.
pub fn run() -> anyhow::Result<()> {
    tui::install_panic_hook();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(bootstrap::run())
}
