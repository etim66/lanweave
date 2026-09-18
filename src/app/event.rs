//! Runtime inputs and application-requested work.

use std::path::PathBuf;

use super::action::{ConnectionTarget, KeyInput, PairingPeer, UserAction};
use super::failure::FailureKind;
use super::model::{TransferProgress, TransferProposal};
use crate::discovery::DiscoveryEvent;
use crate::pairing::PairingCode;
use crate::transfer::selection::FileSelection;

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
    /// Bracketed-paste text from the terminal.
    Paste(String),
    User(UserAction),
    /// An inbound `pair_request` is waiting for the local user's decision.
    IncomingPairingRequest(PairingPeer),
    /// The peer accepted our `pair_request`; the initiator may enter the code.
    PairingAccepted,
    /// The responder created and displays a one-time code.
    PairingCodeIssued(PairingCode),
    PairingSucceeded,
    PairingEnded,
    /// An inbound `transfer_request` is waiting for the local decision.
    IncomingTransferRequest(TransferProposal),
    TransferStarted,
    /// Per-file progress of the active transfer.
    TransferProgress(TransferProgress),
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
    StartTransfer(FileSelection),
    /// Accepts the inbound manifest and stores files under the chosen directory.
    AcceptTransfer(PathBuf),
    RejectTransfer,
    Disconnect,
    Shutdown,
}
