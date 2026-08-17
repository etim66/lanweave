//! Runtime inputs and application-requested work.

use super::action::{ConnectionTarget, KeyInput, UserAction};
use super::failure::FailureKind;
use crate::discovery::DiscoveryEvent;

/// Inputs consumed by the application runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum AppEvent {
    StartupCompleted,
    Tick,
    TerminalResized { width: u16, height: u16 },
    Discovery(DiscoveryEvent),
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    Connect(ConnectionTarget),
    AcceptPairing,
    RejectPairing,
    RejectPairingBusy,
    StartTransfer,
    AcceptTransfer,
    RejectTransfer,
    Disconnect,
    Shutdown,
}
