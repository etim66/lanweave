use ratatui::style::Color;

use crate::app::failure::FailureKind;
use crate::app::model::{AppModel, AppState, Screen};

use super::theme::{ACCENT, ERROR, WARNING};

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

pub(super) fn screen_content(model: &AppModel) -> (&'static str, &'static str, Color) {
    match model.screen() {
        Screen::Starting => ("Starting Lanweave", "Preparing the terminal...", WARNING),
        Screen::Browsing => (
            "No devices found",
            "Searching the local network. Devices will appear automatically.",
            ACCENT,
        ),
        Screen::Error => (
            "Something went wrong",
            failure_message(model.state()),
            ERROR,
        ),
        Screen::Shutdown => ("Closing Lanweave", "Shutting down safely...", WARNING),
        Screen::Pairing => (
            "Pairing",
            "Pairing controls are not available in this build.",
            ACCENT,
        ),
        Screen::Session => (
            "Session active",
            "Session controls are not available in this build.",
            ACCENT,
        ),
        Screen::Transfer => (
            "Transfer",
            "Transfer controls are not available in this build.",
            ACCENT,
        ),
    }
}

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
