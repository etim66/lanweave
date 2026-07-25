//! Lanweave library crate. The binary in `src/main.rs` is a thin wrapper
//! around [`app::run`]. Modules are split per `docs/ARCHITECTURE.md` and
//! `IMPLEMENTATION_PLAN.txt`. Each module starts as a placeholder and
//! gains behavior as features are added.

pub mod app;
pub mod command;
pub mod discovery;
pub mod framing;
pub mod pairing;
pub mod protocol;
pub mod session;
pub mod storage;
pub mod transfer;
pub mod transport;
pub mod tui;
