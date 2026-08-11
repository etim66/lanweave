//! Runtime inputs and application-requested work.

use super::action::{DeviceId, KeyInput, UserAction};
use super::failure::FailureKind;

/// Inputs consumed by the application runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum AppEvent {
    StartupCompleted,
    Tick,
    TerminalResized { width: u16, height: u16 },
    KeyInput(KeyInput),
    User(UserAction),
    IncomingPairingRequest,
    PairingSucceeded,
    PairingEnded,
    IncomingTransferRequest,
    TransferStarted,
    ProposalRejected,
    TransferFinished,
    SessionClosed,
    Failed(FailureKind),
    ShutdownRequested,
}

/// Side effects requested by the reducer and executed outside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Effect {
    Connect(DeviceId),
    AcceptPairing,
    RejectPairing,
    RejectPairingBusy,
    StartTransfer,
    AcceptTransfer,
    RejectTransfer,
    Disconnect,
    Shutdown,
}
