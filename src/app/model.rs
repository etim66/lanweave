//! Plain application state and messages shared by the event loop and adapters.

/// Identifies a discovery candidate without exposing adapter-specific data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId(u64);

impl DeviceId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Safe error categories suitable for application state and user-facing views.
///
/// Detailed errors remain local to the failing adapter so paths, peer input, and
/// other sensitive diagnostics do not accidentally reach the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Startup,
    Connection,
    Pairing,
    Session,
    Transfer,
    Internal,
}

/// The authoritative top-level application state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Starting,
    Browsing,
    PairingOutbound,
    PairingInbound,
    PairingInboundAccepted,
    ClosingPairing,
    SessionIdle,
    OutboundProposal,
    InboundProposal,
    InboundProposalAccepted,
    TransferringOutbound,
    TransferringInbound,
    ClosingSession,
    Error(FailureKind),
    ShuttingDown,
}

impl AppState {
    pub const fn is_pairing(self) -> bool {
        matches!(
            self,
            Self::PairingOutbound
                | Self::PairingInbound
                | Self::PairingInboundAccepted
                | Self::ClosingPairing
        )
    }

    pub const fn has_session(self) -> bool {
        matches!(
            self,
            Self::SessionIdle
                | Self::OutboundProposal
                | Self::InboundProposal
                | Self::InboundProposalAccepted
                | Self::TransferringOutbound
                | Self::TransferringInbound
                | Self::ClosingSession
        )
    }

    pub const fn is_transfer_active(self) -> bool {
        matches!(self, Self::TransferringOutbound | Self::TransferringInbound)
    }
}

/// Renderable screen derived from [`AppState`].
///
/// It is intentionally not stored in [`AppModel`], which prevents the view and
/// the application state machine from drifting apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Starting,
    Browsing,
    Pairing,
    Session,
    Transfer,
    Error,
    Shutdown,
}

impl From<AppState> for Screen {
    fn from(state: AppState) -> Self {
        match state {
            AppState::Starting => Self::Starting,
            AppState::Browsing => Self::Browsing,
            AppState::PairingOutbound
            | AppState::PairingInbound
            | AppState::PairingInboundAccepted
            | AppState::ClosingPairing => Self::Pairing,
            AppState::SessionIdle | AppState::ClosingSession => Self::Session,
            AppState::OutboundProposal
            | AppState::InboundProposal
            | AppState::InboundProposalAccepted
            | AppState::TransferringOutbound
            | AppState::TransferringInbound => Self::Transfer,
            AppState::Error(_) => Self::Error,
            AppState::ShuttingDown => Self::Shutdown,
        }
    }
}

/// State owned exclusively by the application event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppModel {
    state: AppState,
}

impl AppModel {
    pub const fn new() -> Self {
        Self {
            state: AppState::Starting,
        }
    }

    pub const fn state(&self) -> AppState {
        self.state
    }

    pub fn screen(&self) -> Screen {
        self.state.into()
    }

    pub(super) fn transition_to(&mut self, state: AppState) {
        self.state = state;
    }
}

impl Default for AppModel {
    fn default() -> Self {
        Self::new()
    }
}

/// User intent produced by the TUI or command registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAction {
    SelectDevice(DeviceId),
    AcceptPairing,
    RejectPairing,
    StartTransfer,
    AcceptTransfer,
    RejectTransfer,
    Disconnect,
    Quit,
}

/// Inputs consumed by the application reducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    StartupCompleted,
    Tick,
    TerminalResized { width: u16, height: u16 },
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
pub enum Effect {
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

#[cfg(test)]
mod tests {
    use super::{AppModel, AppState, FailureKind, Screen};

    #[test]
    fn model_starts_on_starting_screen() {
        let model = AppModel::new();

        assert_eq!(model.state(), AppState::Starting);
        assert_eq!(model.screen(), Screen::Starting);
    }

    #[test]
    fn screens_are_derived_from_every_state() {
        let cases = [
            (AppState::Starting, Screen::Starting),
            (AppState::Browsing, Screen::Browsing),
            (AppState::PairingOutbound, Screen::Pairing),
            (AppState::PairingInbound, Screen::Pairing),
            (AppState::PairingInboundAccepted, Screen::Pairing),
            (AppState::ClosingPairing, Screen::Pairing),
            (AppState::SessionIdle, Screen::Session),
            (AppState::OutboundProposal, Screen::Transfer),
            (AppState::InboundProposal, Screen::Transfer),
            (AppState::InboundProposalAccepted, Screen::Transfer),
            (AppState::TransferringOutbound, Screen::Transfer),
            (AppState::TransferringInbound, Screen::Transfer),
            (AppState::ClosingSession, Screen::Session),
            (AppState::Error(FailureKind::Internal), Screen::Error),
            (AppState::ShuttingDown, Screen::Shutdown),
        ];

        for (state, expected) in cases {
            assert_eq!(Screen::from(state), expected, "state: {state:?}");
        }
    }
}
