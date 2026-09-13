//! Runtime inputs and application-requested work.

use super::action::{ConnectionTarget, KeyInput, PairingPeer, UserAction};
use super::failure::FailureKind;
use crate::discovery::DiscoveryEvent;
use crate::pairing::PairingCode;

/// Inputs consumed by the application runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum AppEvent {
    StartupCompleted,
    Tick,
    TerminalResized {
        width: u16,
        height: u16,
    },
    Discovery(DiscoveryEvent),
    KeyInput(KeyInput),
    User(UserAction),
    /// An inbound `pair_request` is waiting for the local user's decision.
    IncomingPairingRequest(PairingPeer),
    /// The peer accepted our `pair_request`; the initiator may enter the code.
    PairingAccepted,
    /// The responder created and displays a one-time code.
    PairingCodeIssued(PairingCode),
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
    SubmitPairingCode(PairingCode),
    StartTransfer,
    AcceptTransfer,
    RejectTransfer,
    Disconnect,
    Shutdown,
}
