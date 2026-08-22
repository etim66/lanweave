use ratatui::style::Color;

use crate::app::failure::FailureKind;
use crate::app::model::{AppModel, AppState, Screen};

use super::theme::{ACCENT, ERROR, WARNING};

/// Returns the short status label shown for an application state.
pub(super) fn status_text(state: AppState) -> &'static str {
    match state {
        AppState::Starting => "Starting",
        AppState::Browsing => "Browsing for devices",
        AppState::PairingOutbound
        | AppState::PairingInbound
        | AppState::PairingInboundAccepted
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
        Screen::Pairing => (
            "Pairing".to_owned(),
            "Pairing controls are not available in this build.".to_owned(),
            ACCENT,
        ),
        Screen::Session => (
            "Session active".to_owned(),
            "Session controls are not available in this build.".to_owned(),
            ACCENT,
        ),
        Screen::Transfer => (
            "Transfer".to_owned(),
            "Transfer controls are not available in this build.".to_owned(),
            ACCENT,
        ),
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
