use ratatui::style::Color;

use crate::app::action::PairingPeer;
use crate::app::failure::FailureKind;
use crate::app::model::{AppModel, AppState, Screen};

use super::theme::{ACCENT, ERROR, WARNING};

/// Returns the short status label shown for an application state.
pub(super) fn status_text(state: AppState) -> &'static str {
    match state {
        AppState::Starting => "Starting",
        AppState::Browsing => "Browsing for devices",
        AppState::PairingOutbound
        | AppState::PairingOutboundAccepted
        | AppState::PairingInbound
        | AppState::PairingInboundAccepted
        | AppState::PairingConfirming
        | AppState::ClosingPairing => "Pairing",
        AppState::SessionIdle | AppState::ClosingSession => "Session active",
        AppState::OutboundProposal
        | AppState::InboundProposal
        | AppState::InboundProposalAccepted
        | AppState::TransferringOutbound
        | AppState::TransferringInbound => "Transfer",
        AppState::Error(_) => "Error",
        AppState::ShuttingDown => "Shutting down",
    }
}

/// Returns the title, message, and accent color for the current screen.
pub(super) fn screen_content(model: &AppModel) -> (String, String, Color) {
    match model.screen() {
        Screen::Starting => (
            "Starting Lanweave".to_owned(),
            "Preparing the terminal...".to_owned(),
            WARNING,
        ),
        Screen::Browsing => {
            if model.sorted_candidates().is_empty() {
                (
                    "No devices found".to_owned(),
                    "Searching the local network. Devices will appear automatically.".to_owned(),
                    ACCENT,
                )
            } else {
                (
                    "Devices".to_owned(),
                    "Use up/down to select a device and enter to connect.".to_owned(),
                    ACCENT,
                )
            }
        }
        Screen::Error => (
            "Something went wrong".to_owned(),
            failure_message(model.state()).to_owned(),
            ERROR,
        ),
        Screen::Shutdown => (
            "Closing Lanweave".to_owned(),
            "Shutting down safely...".to_owned(),
            WARNING,
        ),
        Screen::Pairing => pairing_content(model),
        Screen::Session => (
            "Authorized session".to_owned(),
            "The code was confirmed on the live connection. File transfer is not available in this build."
                .to_owned(),
            ACCENT,
        ),
        Screen::Transfer => (
            "Transfer".to_owned(),
            "Transfer controls are not available in this build.".to_owned(),
            ACCENT,
        ),
    }
}

/// Returns the pairing title and message for the current pairing substate.
///
/// Peer names come from untrusted `hello` text; the wording keeps that limit
/// visible on every prompt until confirmation succeeds.
fn pairing_content(model: &AppModel) -> (String, String, Color) {
    let peer = model
        .pairing_peer()
        .map(describe_peer)
        .unwrap_or_else(|| "the other device".to_owned());

    match model.state() {
        AppState::PairingOutbound => (
            "Waiting for response".to_owned(),
            format!(
                "Pairing request sent to {peer}. The name is untrusted until pairing confirms the device."
            ),
            ACCENT,
        ),
        AppState::PairingOutboundAccepted => (
            "Enter the pairing code".to_owned(),
            "Type the eight-digit code shown on the other device, then press enter.".to_owned(),
            ACCENT,
        ),
        AppState::PairingConfirming => (
            "Checking the code".to_owned(),
            "Confirming the pairing over the encrypted connection...".to_owned(),
            ACCENT,
        ),
        AppState::PairingInbound => (
            "Pairing request".to_owned(),
            format!(
                "{peer} wants to pair. The name and address are untrusted; accept only if the person is present."
            ),
            ACCENT,
        ),
        AppState::PairingInboundAccepted => {
            let code = model
                .pairing_code()
                .map(|code| code.grouped())
                .unwrap_or_else(|| "........".to_owned());
            (
                "Pairing code".to_owned(),
                format!(
                    "Tell the other device: {code}\nThe code expires after about two minutes and is never sent over the connection."
                ),
                ACCENT,
            )
        }
        AppState::ClosingPairing => (
            "Closing pairing".to_owned(),
            "Closing the provisional connection...".to_owned(),
            WARNING,
        ),
        _ => (
            "Pairing".to_owned(),
            "Pairing is in progress.".to_owned(),
            ACCENT,
        ),
    }
}

/// Describes a peer for a pairing prompt without presenting it as verified.
fn describe_peer(peer: &PairingPeer) -> String {
    match peer.display_name() {
        Some(name) => format!("{name} ({})", peer.endpoint()),
        None => peer.endpoint().to_owned(),
    }
}

/// Returns the user-facing message for an error state.
fn failure_message(state: AppState) -> &'static str {
    match state {
        AppState::Error(FailureKind::Startup) => "Lanweave could not start.",
        AppState::Error(FailureKind::Connection) => "The connection failed.",
        AppState::Error(FailureKind::Pairing) => "Pairing failed.",
        AppState::Error(FailureKind::Session) => "The session failed.",
        AppState::Error(FailureKind::Transfer) => "The transfer failed.",
        AppState::Error(FailureKind::Internal) => "Lanweave encountered an internal error.",
        _ => "Lanweave encountered an error.",
    }
}
