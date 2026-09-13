//! Plain application state and messages shared by the event loop and adapters.

use std::net::{IpAddr, SocketAddr, SocketAddrV6};

use super::action::{ConnectionTarget, DeviceId, PairingPeer};
use super::failure::FailureKind;
use crate::discovery::{Candidate, CandidateStore, DiscoveryEvent};
use crate::pairing::PairingCode;

/// The authoritative top-level application state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Starting,
    Browsing,
    /// A connection is being established or a `pair_request` awaits response.
    PairingOutbound,
    /// The request was accepted; the initiator enters the displayed code.
    PairingOutboundAccepted,
    /// An inbound request awaits the local accept or reject decision.
    PairingInbound,
    /// The local user accepted; the responder displays the one-time code.
    PairingInboundAccepted,
    /// The code was submitted and the mutual confirmation is in progress.
    PairingConfirming,
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
    pub const ALL: [Self; 17] = [
        Self::Starting,
        Self::Browsing,
        Self::PairingOutbound,
        Self::PairingOutboundAccepted,
        Self::PairingInbound,
        Self::PairingInboundAccepted,
        Self::PairingConfirming,
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
                | Self::PairingOutboundAccepted
                | Self::PairingInbound
                | Self::PairingInboundAccepted
                | Self::PairingConfirming
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
            | AppState::PairingOutboundAccepted
            | AppState::PairingInbound
            | AppState::PairingInboundAccepted
            | AppState::PairingConfirming
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
    pairing_peer: Option<PairingPeer>,
    pairing_code: Option<PairingCode>,
}

impl AppModel {
    /// Creates a model in the starting state with an empty candidate store.
    pub const fn new() -> Self {
        Self {
            state: AppState::Starting,
            candidates: CandidateStore::new(),
            pairing_peer: None,
            pairing_code: None,
        }
    }

    /// Returns the current application state.
    pub const fn state(&self) -> AppState {
        self.state
    }

    /// Returns the peer shown on a pairing prompt or code screen.
    pub(crate) fn pairing_peer(&self) -> Option<&PairingPeer> {
        self.pairing_peer.as_ref()
    }

    /// Returns the responder's one-time code while it is displayed.
    pub(crate) fn pairing_code(&self) -> Option<&PairingCode> {
        self.pairing_code.as_ref()
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

    /// Stores the untrusted peer shown during pairing.
    pub(super) fn set_pairing_peer(&mut self, peer: PairingPeer) {
        self.pairing_peer = Some(peer);
    }

    /// Stores the responder's displayed one-time code.
    pub(super) fn set_pairing_code(&mut self, code: PairingCode) {
        self.pairing_code = Some(code);
    }

    /// Drops the peer and code as soon as pairing or the session ends.
    pub(super) fn clear_pairing(&mut self) {
        self.pairing_peer = None;
        self.pairing_code = None;
    }

    /// Resolves a discovered device to one route with display context.
    ///
    /// Addresses are already sorted by the candidate store, so the first one
    /// is the deterministic choice. Returning `None` means the device is gone
    /// and the selection must not start a connection.
    pub(super) fn discovered_target(&self, device: DeviceId) -> Option<ConnectionTarget> {
        let candidate = self.candidates.candidate(device)?;
        let address = candidate.addresses().first()?;
        Some(ConnectionTarget::Discovered {
            address: socket_address(
                address.address(),
                address.interface_index(),
                candidate.port(),
            ),
            display_name: candidate.display_name().to_owned(),
        })
    }

    /// Builds a model in `state` for tests.
    #[cfg(test)]
    pub(crate) fn for_test(state: AppState) -> Self {
        Self {
            state,
            candidates: CandidateStore::new(),
            pairing_peer: None,
            pairing_code: None,
        }
    }
}

/// Builds a routable socket address, keeping IPv6 interface scope.
fn socket_address(ip: IpAddr, scope: u32, port: u16) -> SocketAddr {
    match ip {
        IpAddr::V4(v4) => SocketAddr::new(IpAddr::V4(v4), port),
        IpAddr::V6(v6) => SocketAddr::V6(SocketAddrV6::new(v6, port, 0, scope)),
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
            (AppState::PairingOutboundAccepted, Screen::Pairing),
            (AppState::PairingInbound, Screen::Pairing),
            (AppState::PairingInboundAccepted, Screen::Pairing),
            (AppState::PairingConfirming, Screen::Pairing),
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
