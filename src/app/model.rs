//! Plain application state and messages shared by the event loop and adapters.

use std::net::{IpAddr, SocketAddr, SocketAddrV6};
use std::path::{Path, PathBuf};

use super::action::{ConnectionTarget, DeviceId, PairingPeer};
use super::failure::FailureKind;
use crate::discovery::{Candidate, CandidateStore, DiscoveryEvent, escape_display};
use crate::pairing::PairingCode;
use crate::protocol::{FileEntry, TransferRequest};
use crate::transfer::selection::FileSelection;

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
    /// The file review list can be opened and kept on screen.
    pub(crate) can_review_files: bool,
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

/// A peer's immutable manifest waiting for the local decision.
///
/// The peer display name is untrusted and escaped at construction; it never
/// proves the peer's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferProposal {
    files: Vec<FileEntry>,
    total_size: u64,
    peer: Option<String>,
}

impl TransferProposal {
    /// Captures the bounded manifest and the untrusted peer name.
    pub(crate) fn new(request: &TransferRequest, peer: Option<String>) -> Self {
        Self {
            files: request.files.clone(),
            total_size: request.total_size,
            peer: peer.map(|name| escape_display(&name)),
        }
    }

    /// Returns the manifest entries in transfer order.
    pub(crate) fn files(&self) -> &[FileEntry] {
        &self.files
    }

    /// Returns the checked total size of all entries.
    pub(crate) const fn total_size(&self) -> u64 {
        self.total_size
    }

    /// Returns the escaped, untrusted display name of the requester.
    pub(crate) fn peer(&self) -> Option<&str> {
        self.peer.as_deref()
    }
}

/// Per-file progress of the active transfer.
///
/// `index` is the zero-based manifest index of the file being moved and
/// `transferred` is the byte count already handled for that file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TransferProgress {
    pub(crate) index: u16,
    pub(crate) files: u16,
    pub(crate) transferred: u64,
}

/// State owned exclusively by the application event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppModel {
    state: AppState,
    candidates: CandidateStore,
    pairing_peer: Option<PairingPeer>,
    pairing_code: Option<PairingCode>,
    transfer_proposal: Option<TransferProposal>,
    outbound_selection: Option<FileSelection>,
    deferred_selection: Option<FileSelection>,
    transfer_progress: Option<TransferProgress>,
    default_destination: PathBuf,
}

impl AppModel {
    /// Creates a model in the starting state with an empty candidate store.
    ///
    /// The default destination is the directory Lanweave was started in; the
    /// recipient can replace it for every inbound transfer.
    pub fn new() -> Self {
        Self {
            state: AppState::Starting,
            candidates: CandidateStore::new(),
            pairing_peer: None,
            pairing_code: None,
            transfer_proposal: None,
            outbound_selection: None,
            deferred_selection: None,
            transfer_progress: None,
            default_destination: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
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

    /// Returns the inbound manifest waiting for the local decision.
    pub(crate) fn transfer_proposal(&self) -> Option<&TransferProposal> {
        self.transfer_proposal.as_ref()
    }

    /// Returns the locally reviewed files of the pending outbound proposal.
    pub(crate) fn outbound_selection(&self) -> Option<&FileSelection> {
        self.outbound_selection.as_ref()
    }

    /// Returns the withdrawn local selection waiting for a later explicit send.
    pub(crate) fn deferred_selection(&self) -> Option<&FileSelection> {
        self.deferred_selection.as_ref()
    }

    /// Returns the progress of the active transfer.
    pub(crate) fn transfer_progress(&self) -> Option<TransferProgress> {
        self.transfer_progress
    }

    /// Returns the directory prefilled for the next inbound transfer.
    pub(crate) fn default_destination(&self) -> &Path {
        &self.default_destination
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
            can_review_files: matches!(state, AppState::Browsing | AppState::SessionIdle),
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

    /// Stores the inbound manifest waiting for the local decision.
    pub(super) fn set_incoming_proposal(&mut self, proposal: TransferProposal) {
        self.transfer_proposal = Some(proposal);
        self.transfer_progress = None;
    }

    /// Starts a local proposal and stores its reviewed files.
    ///
    /// A new explicit send replaces any previously queued selection.
    pub(super) fn begin_outbound(&mut self, selection: FileSelection) {
        self.deferred_selection = None;
        self.outbound_selection = Some(selection);
        self.transfer_progress = None;
    }

    /// Moves the pending local selection to the queue.
    ///
    /// The files are never sent again without a later explicit send action.
    pub(super) fn defer_outbound(&mut self) {
        if let Some(selection) = self.outbound_selection.take() {
            self.deferred_selection = Some(selection);
        }
    }

    /// Updates the active transfer progress.
    pub(super) fn set_progress(&mut self, progress: TransferProgress) {
        self.transfer_progress = Some(progress);
    }

    /// Sets the destination prefilled for the next inbound transfer.
    pub(super) fn set_default_destination(&mut self, destination: PathBuf) {
        self.default_destination = destination;
    }

    /// Clears the finished proposal or transfer, keeping any queued selection.
    pub(super) fn clear_round(&mut self) {
        self.transfer_proposal = None;
        self.outbound_selection = None;
        self.transfer_progress = None;
    }

    /// Clears every transfer value once the session is gone.
    pub(super) fn clear_transfer(&mut self) {
        self.clear_round();
        self.deferred_selection = None;
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
            transfer_proposal: None,
            outbound_selection: None,
            deferred_selection: None,
            transfer_progress: None,
            default_destination: PathBuf::from("."),
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
