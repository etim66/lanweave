//! Plain application state and messages shared by the event loop and adapters.

use super::failure::FailureKind;
use crate::discovery::{Candidate, CandidateStore, DiscoveryEvent};

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
    /// Every state, for exhaustive test coverage.
    #[cfg(test)]
    pub const ALL: [Self; 15] = [
        Self::Starting,
        Self::Browsing,
        Self::PairingOutbound,
        Self::PairingInbound,
        Self::PairingInboundAccepted,
        Self::ClosingPairing,
        Self::SessionIdle,
        Self::OutboundProposal,
        Self::InboundProposal,
        Self::InboundProposalAccepted,
        Self::TransferringOutbound,
        Self::TransferringInbound,
        Self::ClosingSession,
        Self::Error(FailureKind::Internal),
        Self::ShuttingDown,
    ];

    /// Returns whether the app is in a pairing state.
    pub const fn is_pairing(self) -> bool {
        matches!(
            self,
            Self::PairingOutbound
                | Self::PairingInbound
                | Self::PairingInboundAccepted
                | Self::ClosingPairing
        )
    }

    /// Returns whether the app has an established session.
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

    /// Returns whether a transfer is actively running.
    pub const fn is_transfer_active(self) -> bool {
        matches!(self, Self::TransferringOutbound | Self::TransferringInbound)
    }

    /// Returns whether the user may start a disconnect from this state.
    pub const fn can_disconnect(self) -> bool {
        (self.is_pairing() && !matches!(self, Self::ClosingPairing))
            || (self.has_session() && !matches!(self, Self::ClosingSession))
    }
}

/// Narrow application capabilities consumed by interaction adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AppCapabilities {
    pub(crate) accepts_commands: bool,
    pub(crate) can_show_devices: bool,
    pub(crate) can_start_transfer: bool,
    pub(crate) transfer_unavailable: bool,
    pub(crate) session_closing: bool,
    pub(crate) can_disconnect: bool,
    pub(crate) disconnecting: bool,
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
    /// Maps each application state to the screen that renders it.
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
    candidates: CandidateStore,
}

impl AppModel {
    /// Creates a model in the starting state with an empty candidate store.
    pub const fn new() -> Self {
        Self {
            state: AppState::Starting,
            candidates: CandidateStore::new(),
        }
    }

    /// Returns the current application state.
    pub const fn state(&self) -> AppState {
        self.state
    }

    /// Returns the screen the current state should render.
    pub fn screen(&self) -> Screen {
        self.state.into()
    }

    /// Returns the currently discovered candidates.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn candidates(&self) -> &[Candidate] {
        self.candidates.candidates()
    }

    /// Returns candidates in deterministic display order.
    pub(crate) fn sorted_candidates(&self) -> Vec<&Candidate> {
        self.candidates.sorted_candidates()
    }

    /// Returns the interaction capabilities of the current state.
    pub(crate) fn capabilities(&self) -> AppCapabilities {
        let state = self.state;
        AppCapabilities {
            accepts_commands: state != AppState::ShuttingDown,
            can_show_devices: state == AppState::Browsing,
            can_start_transfer: state == AppState::SessionIdle,
            transfer_unavailable: state.has_session() && state != AppState::SessionIdle,
            session_closing: state == AppState::ClosingSession,
            can_disconnect: state.can_disconnect(),
            disconnecting: matches!(state, AppState::ClosingPairing | AppState::ClosingSession),
        }
    }

    /// Moves the model into `state`, discarding the previous one.
    pub(super) fn transition_to(&mut self, state: AppState) {
        self.state = state;
    }

    /// Applies one discovery event to the candidate store.
    pub(super) fn apply_discovery(&mut self, event: DiscoveryEvent) {
        self.candidates.apply(event);
    }

    /// Builds a model in `state` for tests.
    #[cfg(test)]
    pub(crate) fn for_test(state: AppState) -> Self {
        Self {
            state,
            candidates: CandidateStore::new(),
        }
    }
}

impl Default for AppModel {
    /// Creates a model in the starting state.
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{AppModel, AppState, Screen};
    use crate::app::failure::FailureKind;

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
