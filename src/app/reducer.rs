//! Synchronous application state transitions.

use super::action::{ConnectionTarget, UserAction};
use super::event::{AppEvent, Effect};
use super::model::{AppModel, AppState};

/// The maximum work one event may request.
///
/// Keeping this explicit makes reducer output predictable before it reaches a
/// bounded effect queue.
pub const MAX_EFFECTS_PER_EVENT: usize = 1;

/// Applies one event and returns ordered work for asynchronous effect handlers.
///
/// Invalid or stale events are ignored. In particular, once shutdown starts no
/// later queued event can revive the application or request additional work.
pub fn update(model: &mut AppModel, event: AppEvent) -> Vec<Effect> {
    if matches!(event, AppEvent::ShutdownRequested) {
        return begin_shutdown(model);
    }

    if model.state() == AppState::ShuttingDown {
        return Vec::new();
    }

    match event {
        AppEvent::Discovery(event) => {
            model.apply_discovery(event);
            Vec::new()
        }
        AppEvent::User(action) => apply_user_action(model, action),
        AppEvent::KeyInput(_) => Vec::new(),
        event => apply_service_event(model, event),
    }
}

/// Applies a user action that is valid for the current state.
///
/// Returns the effect that must run for the action, or nothing when the action
/// is not valid here. Quit always starts shutdown.
fn apply_user_action(model: &mut AppModel, action: UserAction) -> Vec<Effect> {
    if action == UserAction::Quit {
        return begin_shutdown(model);
    }

    let effect = match (model.state(), action) {
        (_, UserAction::ShowHelp | UserAction::ShowDevices) => None,
        (AppState::Browsing, UserAction::SelectDevice(device)) => {
            // The device is resolved against the live store, so a removed
            // device can never start a connection through stale UI state.
            if let Some(target) = model.discovered_target(device) {
                model.set_pairing_peer(target.pairing_peer());
                model.transition_to(AppState::PairingOutbound);
                Some(Effect::Connect(target))
            } else {
                None
            }
        }
        (AppState::Browsing, UserAction::ConnectDirect(endpoint)) => {
            let target = ConnectionTarget::Direct(endpoint);
            model.set_pairing_peer(target.pairing_peer());
            model.transition_to(AppState::PairingOutbound);
            Some(Effect::Connect(target))
        }
        (AppState::PairingInbound, UserAction::AcceptPairing) => {
            model.transition_to(AppState::PairingInboundAccepted);
            Some(Effect::AcceptPairing)
        }
        (AppState::PairingInbound, UserAction::RejectPairing) => {
            model.transition_to(AppState::ClosingPairing);
            Some(Effect::RejectPairing)
        }
        (AppState::PairingOutboundAccepted, UserAction::SubmitPairingCode(code)) => {
            model.transition_to(AppState::PairingConfirming);
            Some(Effect::SubmitPairingCode(code))
        }
        (AppState::SessionIdle, UserAction::StartTransfer(selection)) => {
            model.begin_outbound(selection.clone());
            model.transition_to(AppState::OutboundProposal);
            Some(Effect::StartTransfer(selection))
        }
        (AppState::InboundProposal, UserAction::AcceptTransfer(destination)) => {
            model.set_default_destination(destination.clone());
            model.transition_to(AppState::InboundProposalAccepted);
            Some(Effect::AcceptTransfer(destination))
        }
        (AppState::InboundProposal, UserAction::RejectTransfer) => {
            model.transition_to(AppState::SessionIdle);
            Some(Effect::RejectTransfer)
        }
        (
            AppState::OutboundProposal
            | AppState::TransferringOutbound
            | AppState::TransferringInbound,
            UserAction::CancelTransfer,
        ) => Some(Effect::CancelTransfer),
        (AppState::TransferComplete, UserAction::DismissSummary) => {
            let session_closed = model
                .summary()
                .is_some_and(|summary| summary.session_closed);
            model.clear_summary();
            model.transition_to(if session_closed {
                AppState::Browsing
            } else {
                AppState::SessionIdle
            });
            None
        }
        (state, UserAction::Disconnect) if state.can_disconnect() => {
            if state.is_pairing() {
                model.transition_to(AppState::ClosingPairing);
            } else {
                model.transition_to(AppState::ClosingSession);
            }
            Some(Effect::Disconnect)
        }
        _ => None,
    };

    effect.into_iter().collect()
}

/// Applies a service event that is valid for the current state.
///
/// Returns the effect that must run for the event, or nothing when the event
/// is stale for this state. Events that do not match any state are ignored.
fn apply_service_event(model: &mut AppModel, event: AppEvent) -> Vec<Effect> {
    let effect = match (model.state(), event) {
        (AppState::Starting, AppEvent::StartupCompleted) => {
            model.transition_to(AppState::Browsing);
            None
        }
        (AppState::Browsing, AppEvent::IncomingPairingRequest(peer)) => {
            model.set_pairing_peer(peer);
            model.transition_to(AppState::PairingInbound);
            None
        }
        (AppState::PairingOutbound, AppEvent::PairingAccepted) => {
            // The code arrives only after acceptance, never before it.
            model.transition_to(AppState::PairingOutboundAccepted);
            None
        }
        (AppState::PairingInboundAccepted, AppEvent::PairingCodeIssued(code)) => {
            model.set_pairing_code(code);
            None
        }
        (
            AppState::PairingOutbound
            | AppState::PairingOutboundAccepted
            | AppState::PairingConfirming
            | AppState::PairingInboundAccepted,
            AppEvent::PairingSucceeded,
        ) => {
            model.begin_session();
            model.transition_to(AppState::SessionIdle);
            None
        }
        (state, AppEvent::PairingEnded) if state.is_pairing() => {
            model.clear_pairing();
            model.transition_to(AppState::Browsing);
            None
        }
        (AppState::SessionIdle, AppEvent::IncomingTransferRequest(proposal)) => {
            model.set_incoming_proposal(proposal);
            model.transition_to(AppState::InboundProposal);
            None
        }
        // The initiator-priority collision rule: the responder withdraws its
        // own proposal and reviews the initiator's. The local files stay
        // queued for a later explicit send.
        (AppState::OutboundProposal, AppEvent::IncomingTransferRequest(proposal)) => {
            model.defer_outbound();
            model.set_incoming_proposal(proposal);
            model.transition_to(AppState::InboundProposal);
            None
        }
        (AppState::OutboundProposal, AppEvent::TransferPreparing(preparation)) => {
            model.set_preparation(preparation);
            None
        }
        (AppState::OutboundProposal, AppEvent::PreparationFailed(error)) => {
            model.defer_outbound();
            model.clear_round();
            model.set_transfer_notice(error.notice());
            model.transition_to(AppState::SessionIdle);
            None
        }
        (AppState::OutboundProposal, AppEvent::TransferStarted) => {
            model.transition_to(AppState::TransferringOutbound);
            None
        }
        (AppState::OutboundProposal, AppEvent::ProposalRejected) => {
            model.defer_outbound();
            model.clear_round();
            model.transition_to(AppState::SessionIdle);
            None
        }
        (state, AppEvent::TransferProgress(progress)) if state.is_transfer_active() => {
            model.set_progress(progress);
            None
        }
        (state, AppEvent::TransferCompleted(summary))
            if state.is_transfer_active() || state == AppState::OutboundProposal =>
        {
            // A proposal that never started keeps its files queued for `/send`.
            if state == AppState::OutboundProposal {
                model.defer_outbound();
            }
            model.set_summary(summary);
            model.transition_to(AppState::TransferComplete);
            None
        }
        (AppState::InboundProposalAccepted, AppEvent::TransferStarted) => {
            model.transition_to(AppState::TransferringInbound);
            None
        }
        (
            AppState::InboundProposal | AppState::InboundProposalAccepted,
            AppEvent::ProposalRejected,
        ) => {
            model.clear_round();
            model.transition_to(AppState::SessionIdle);
            None
        }
        // A second request cannot be shown while a connection is busy.
        (state, AppEvent::IncomingPairingRequest(_))
            if state.is_pairing() || state.has_session() =>
        {
            Some(Effect::RejectPairingBusy)
        }
        // A new peer proposal replaces a summary that is still on screen so
        // the required review prompt is never hidden behind it.
        (AppState::TransferComplete, AppEvent::IncomingTransferRequest(proposal)) => {
            model.clear_summary();
            model.set_incoming_proposal(proposal);
            model.transition_to(AppState::InboundProposal);
            None
        }
        // A session that ends while its summary is shown keeps the summary
        // until the user dismisses it.
        (AppState::TransferComplete, AppEvent::SessionClosed) => {
            model.clear_session_peer();
            if model.summary().is_some() {
                model.mark_summary_session_closed();
            } else {
                model.transition_to(AppState::Browsing);
            }
            None
        }
        (state, AppEvent::SessionClosed) if state.is_pairing() || state.has_session() => {
            model.clear_pairing();
            model.clear_session_peer();
            model.clear_transfer();
            model.transition_to(AppState::Browsing);
            None
        }
        (state, AppEvent::Failed(kind)) if !matches!(state, AppState::Error(_)) => {
            model.clear_pairing();
            model.clear_transfer();
            model.transition_to(AppState::Error(kind));
            None
        }
        _ => None,
    };

    effect.into_iter().collect()
}

/// Moves the model into the shutting-down state and requests the shutdown effect.
///
/// A second shutdown request is ignored, so no effect is ever emitted twice.
fn begin_shutdown(model: &mut AppModel) -> Vec<Effect> {
    if model.state() == AppState::ShuttingDown {
        return Vec::new();
    }

    model.transition_to(AppState::ShuttingDown);
    vec![Effect::Shutdown]
}

#[cfg(test)]
mod tests {
    use tokio::time::Instant;

    use super::{MAX_EFFECTS_PER_EVENT, update};
    use crate::app::action::{ConnectionTarget, DeviceId, DirectEndpoint, PairingPeer, UserAction};
    use crate::app::event::{AppEvent, Effect};
    use crate::app::failure::FailureKind;
    use crate::app::model::{
        AppModel, AppState, TransferDirection, TransferProposal, TransferSummary,
    };
    use crate::discovery::{DiscoveredService, DiscoveryEvent};
    use crate::pairing::PairingCode;
    use crate::protocol::{FileEntry, TransferRequest};
    use crate::transfer::selection::FileSelection;

    const DEVICE: DeviceId = DeviceId::new(7);

    /// A local destination directory for accept actions.
    fn destination() -> std::path::PathBuf {
        std::path::PathBuf::from("/incoming")
    }

    /// A bounded inbound manifest for transfer events.
    fn proposal() -> TransferProposal {
        TransferProposal::new(
            &TransferRequest::new(vec![FileEntry::new("report.txt".to_owned(), 64)]).unwrap(),
            Some("peer".to_owned()),
        )
    }

    /// A completed-transfer summary for reducer tests.
    fn summary() -> TransferSummary {
        TransferSummary::new(
            TransferDirection::Sent,
            vec![FileEntry::new("report.txt".to_owned(), 64)],
            None,
            Some("peer".to_owned()),
            std::time::Duration::from_secs(1),
        )
    }

    fn peer() -> PairingPeer {
        PairingPeer::new(Some("peer".to_owned()), "127.0.0.1:4242".to_owned())
    }

    fn code(digits: &str) -> PairingCode {
        PairingCode::parse(digits).expect("test code is eight digits")
    }

    fn model_in(state: AppState) -> AppModel {
        let mut model = AppModel::new();
        model.transition_to(state);
        model
    }

    /// A browsing model with one live discovered candidate.
    fn browsing_with_device() -> AppModel {
        let mut model = model_in(AppState::Browsing);
        model.apply_discovery(DiscoveryEvent::Resolved(DiscoveredService::for_test(
            "peer",
            Instant::now(),
        )));
        model
    }

    #[test]
    fn legal_transitions_match_the_application_state_machine() {
        let cases = [
            (
                AppState::Starting,
                AppEvent::StartupCompleted,
                AppState::Browsing,
                None,
            ),
            (
                AppState::Browsing,
                AppEvent::IncomingPairingRequest(peer()),
                AppState::PairingInbound,
                None,
            ),
            (
                AppState::PairingOutbound,
                AppEvent::PairingAccepted,
                AppState::PairingOutboundAccepted,
                None,
            ),
            (
                AppState::PairingOutboundAccepted,
                AppEvent::User(UserAction::SubmitPairingCode(code("12345678"))),
                AppState::PairingConfirming,
                Some(Effect::SubmitPairingCode(code("12345678"))),
            ),
            (
                AppState::PairingInbound,
                AppEvent::User(UserAction::AcceptPairing),
                AppState::PairingInboundAccepted,
                Some(Effect::AcceptPairing),
            ),
            (
                AppState::PairingInbound,
                AppEvent::User(UserAction::RejectPairing),
                AppState::ClosingPairing,
                Some(Effect::RejectPairing),
            ),
            (
                AppState::PairingOutbound,
                AppEvent::PairingSucceeded,
                AppState::SessionIdle,
                None,
            ),
            (
                AppState::PairingInboundAccepted,
                AppEvent::PairingSucceeded,
                AppState::SessionIdle,
                None,
            ),
            (
                AppState::PairingOutbound,
                AppEvent::PairingEnded,
                AppState::Browsing,
                None,
            ),
            (
                AppState::SessionIdle,
                AppEvent::IncomingPairingRequest(peer()),
                AppState::SessionIdle,
                Some(Effect::RejectPairingBusy),
            ),
            (
                AppState::SessionIdle,
                AppEvent::User(UserAction::StartTransfer(FileSelection::for_test(&[(
                    "report.txt",
                    64,
                )]))),
                AppState::OutboundProposal,
                Some(Effect::StartTransfer(FileSelection::for_test(&[(
                    "report.txt",
                    64,
                )]))),
            ),
            (
                AppState::SessionIdle,
                AppEvent::IncomingTransferRequest(proposal()),
                AppState::InboundProposal,
                None,
            ),
            (
                AppState::OutboundProposal,
                AppEvent::TransferStarted,
                AppState::TransferringOutbound,
                None,
            ),
            (
                AppState::OutboundProposal,
                AppEvent::ProposalRejected,
                AppState::SessionIdle,
                None,
            ),
            (
                AppState::InboundProposal,
                AppEvent::User(UserAction::AcceptTransfer(destination())),
                AppState::InboundProposalAccepted,
                Some(Effect::AcceptTransfer(destination())),
            ),
            (
                AppState::InboundProposalAccepted,
                AppEvent::TransferStarted,
                AppState::TransferringInbound,
                None,
            ),
            (
                AppState::InboundProposal,
                AppEvent::User(UserAction::RejectTransfer),
                AppState::SessionIdle,
                Some(Effect::RejectTransfer),
            ),
            (
                AppState::TransferringOutbound,
                AppEvent::TransferCompleted(summary()),
                AppState::TransferComplete,
                None,
            ),
            (
                AppState::TransferringInbound,
                AppEvent::TransferCompleted(summary()),
                AppState::TransferComplete,
                None,
            ),
            (
                AppState::OutboundProposal,
                AppEvent::User(UserAction::CancelTransfer),
                AppState::OutboundProposal,
                Some(Effect::CancelTransfer),
            ),
            (
                AppState::TransferringOutbound,
                AppEvent::User(UserAction::CancelTransfer),
                AppState::TransferringOutbound,
                Some(Effect::CancelTransfer),
            ),
            (
                AppState::TransferComplete,
                AppEvent::User(UserAction::DismissSummary),
                AppState::SessionIdle,
                None,
            ),
            (
                AppState::SessionIdle,
                AppEvent::User(UserAction::Disconnect),
                AppState::ClosingSession,
                Some(Effect::Disconnect),
            ),
            (
                AppState::ClosingSession,
                AppEvent::SessionClosed,
                AppState::Browsing,
                None,
            ),
            (
                AppState::Browsing,
                AppEvent::Failed(FailureKind::Internal),
                AppState::Error(FailureKind::Internal),
                None,
            ),
        ];

        for (initial, event, expected_state, expected_effect) in cases {
            let mut model = model_in(initial);
            let effects = update(&mut model, event.clone());

            assert_eq!(model.state(), expected_state, "event: {event:?}");
            assert_eq!(effects, expected_effect.into_iter().collect::<Vec<_>>());
            assert!(effects.len() <= MAX_EFFECTS_PER_EVENT);
        }
    }

    #[test]
    fn device_selection_resolves_once_and_stores_the_peer() {
        let mut model = browsing_with_device();
        let device = model.sorted_candidates()[0].id();

        assert_eq!(
            update(&mut model, AppEvent::User(UserAction::SelectDevice(device))),
            vec![Effect::Connect(ConnectionTarget::Discovered {
                address: "127.0.0.1:4242".parse().unwrap(),
                display_name: "peer".to_owned(),
            })]
        );
        assert_eq!(model.state(), AppState::PairingOutbound);
        assert_eq!(model.pairing_peer().unwrap().display_name(), Some("peer"));

        for state in all_states() {
            if state == AppState::Browsing {
                continue;
            }

            let mut model = model_in(state);
            let effects = update(&mut model, AppEvent::User(UserAction::SelectDevice(DEVICE)));

            assert_eq!(model.state(), state, "state: {state:?}");
            assert!(effects.is_empty(), "state: {state:?}");
        }
    }

    #[test]
    fn removed_devices_cannot_start_a_connection() {
        let mut model = browsing_with_device();
        let device = model.sorted_candidates()[0].id();
        model.apply_discovery(DiscoveryEvent::Removed {
            service_instance: "peer._lanweave._tcp.local.".to_owned(),
        });

        assert!(update(&mut model, AppEvent::User(UserAction::SelectDevice(device))).is_empty());
        assert_eq!(model.state(), AppState::Browsing);
    }

    #[test]
    fn direct_address_uses_the_same_connection_entry_point() {
        let endpoint = DirectEndpoint::new("peer.local".to_owned(), 4242).unwrap();
        assert_eq!(endpoint.host(), "peer.local");
        assert_eq!(endpoint.port(), 4242);
        let mut model = model_in(AppState::Browsing);

        assert_eq!(
            update(
                &mut model,
                AppEvent::User(UserAction::ConnectDirect(endpoint.clone())),
            ),
            vec![Effect::Connect(ConnectionTarget::Direct(endpoint))]
        );
        assert_eq!(model.state(), AppState::PairingOutbound);

        assert!(DirectEndpoint::new(String::new(), 4242).is_none());
        assert!(DirectEndpoint::new("peer".to_owned(), 0).is_none());
        assert!(
            DirectEndpoint::new(
                "x".repeat(crate::app::action::MAX_DIRECT_HOST_BYTES + 1),
                4242,
            )
            .is_none()
        );
    }

    #[test]
    fn transfer_only_starts_from_an_idle_session() {
        for state in all_states() {
            if state == AppState::SessionIdle {
                continue;
            }

            let mut model = model_in(state);
            let effects = update(
                &mut model,
                AppEvent::User(UserAction::StartTransfer(FileSelection::default())),
            );

            assert_eq!(model.state(), state, "state: {state:?}");
            assert!(effects.is_empty(), "state: {state:?}");
        }
    }

    #[test]
    fn decision_actions_are_ignored_outside_their_prompt() {
        let cases = [
            (UserAction::AcceptPairing, AppState::PairingInbound),
            (UserAction::RejectPairing, AppState::PairingInbound),
            (
                UserAction::SubmitPairingCode(code("12345678")),
                AppState::PairingOutboundAccepted,
            ),
            (
                UserAction::AcceptTransfer(destination()),
                AppState::InboundProposal,
            ),
            (UserAction::RejectTransfer, AppState::InboundProposal),
            (UserAction::DismissSummary, AppState::TransferComplete),
        ];

        for (action, valid_state) in cases {
            for state in all_states() {
                if state == valid_state {
                    continue;
                }

                let mut model = model_in(state);
                let effects = update(&mut model, AppEvent::User(action.clone()));

                assert_eq!(model.state(), state, "action: {action:?}, state: {state:?}");
                assert!(effects.is_empty(), "action: {action:?}, state: {state:?}");
            }
        }
    }

    #[test]
    fn stale_service_events_do_not_change_unrelated_states() {
        let cases = [
            (AppState::Browsing, AppEvent::PairingSucceeded),
            (AppState::PairingInbound, AppEvent::PairingSucceeded),
            (
                AppState::PairingOutbound,
                AppEvent::TransferCompleted(summary()),
            ),
            (AppState::SessionIdle, AppEvent::TransferStarted),
            (AppState::Browsing, AppEvent::SessionClosed),
            (AppState::Browsing, AppEvent::PairingAccepted),
        ];

        for (state, event) in cases {
            let mut model = model_in(state);

            assert!(update(&mut model, event).is_empty());
            assert_eq!(model.state(), state);
        }
    }

    #[test]
    fn pairing_data_is_stored_and_cleared_with_the_flow() {
        let mut model = browsing_with_device();
        let device = model.sorted_candidates()[0].id();
        update(&mut model, AppEvent::User(UserAction::SelectDevice(device)));
        update(&mut model, AppEvent::PairingSucceeded);
        assert_eq!(model.state(), AppState::SessionIdle);
        assert!(model.pairing_peer().is_none());

        // The responder keeps the generated code until the flow ends.
        let mut model = model_in(AppState::PairingInboundAccepted);
        update(&mut model, AppEvent::PairingCodeIssued(code("00000042")));
        assert_eq!(
            model.pairing_code().unwrap().grouped(),
            "0000 0042".to_owned()
        );
        update(&mut model, AppEvent::SessionClosed);
        assert_eq!(model.state(), AppState::Browsing);
        assert!(model.pairing_code().is_none());

        // A failure clears pairing state before showing the error screen.
        let mut model = model_in(AppState::PairingConfirming);
        update(&mut model, AppEvent::Failed(FailureKind::Pairing));
        assert_eq!(model.state(), AppState::Error(FailureKind::Pairing));
        assert!(model.pairing_peer().is_none() && model.pairing_code().is_none());
    }

    #[test]
    fn terminal_redraw_events_do_not_mutate_application_state() {
        let mut model = model_in(AppState::Browsing);

        assert!(update(&mut model, AppEvent::Tick).is_empty());
        assert!(
            update(
                &mut model,
                AppEvent::TerminalResized {
                    width: 120,
                    height: 40,
                },
            )
            .is_empty()
        );
        assert_eq!(model.state(), AppState::Browsing);
    }

    #[test]
    fn discovery_updates_candidates_without_changing_application_state() {
        for state in all_states() {
            if state == AppState::ShuttingDown {
                continue;
            }

            let mut model = model_in(state);
            let effects = update(
                &mut model,
                AppEvent::Discovery(DiscoveryEvent::Resolved(DiscoveredService::for_test(
                    "peer",
                    Instant::now(),
                ))),
            );

            assert!(effects.is_empty(), "state: {state:?}");
            assert_eq!(model.state(), state);
            assert_eq!(model.candidates().len(), 1);
        }
    }

    #[test]
    fn discovery_removal_does_not_close_an_active_session() {
        let mut model = model_in(AppState::SessionIdle);
        let service = DiscoveredService::for_test("peer", Instant::now());
        update(
            &mut model,
            AppEvent::Discovery(DiscoveryEvent::Resolved(service)),
        );

        let effects = update(
            &mut model,
            AppEvent::Discovery(DiscoveryEvent::Removed {
                service_instance: "peer._lanweave._tcp.local.".to_owned(),
            }),
        );

        assert!(effects.is_empty());
        assert_eq!(model.state(), AppState::SessionIdle);
        assert!(model.candidates().is_empty());
    }

    #[test]
    fn shutdown_ignores_queued_discovery_updates() {
        let mut model = model_in(AppState::ShuttingDown);

        assert!(
            update(
                &mut model,
                AppEvent::Discovery(DiscoveryEvent::Resolved(DiscoveredService::for_test(
                    "peer",
                    Instant::now(),
                ))),
            )
            .is_empty()
        );
        assert!(model.candidates().is_empty());
    }

    #[test]
    fn disconnect_uses_the_correct_closing_state_and_is_emitted_once() {
        let cases = [
            (AppState::PairingOutbound, AppState::ClosingPairing),
            (AppState::SessionIdle, AppState::ClosingSession),
        ];

        for (initial, closing) in cases {
            let mut model = model_in(initial);

            assert_eq!(
                update(&mut model, AppEvent::User(UserAction::Disconnect)),
                vec![Effect::Disconnect]
            );
            assert_eq!(model.state(), closing);
            assert!(update(&mut model, AppEvent::User(UserAction::Disconnect)).is_empty());
            assert_eq!(model.state(), closing);
        }
    }

    #[test]
    fn error_state_ignores_non_shutdown_events() {
        let mut model = model_in(AppState::Error(FailureKind::Startup));

        for event in [
            AppEvent::StartupCompleted,
            AppEvent::IncomingPairingRequest(peer()),
            AppEvent::PairingAccepted,
            AppEvent::PairingCodeIssued(code("12345678")),
            AppEvent::PairingSucceeded,
            AppEvent::IncomingTransferRequest(proposal()),
            AppEvent::TransferStarted,
            AppEvent::TransferCompleted(summary()),
            AppEvent::SessionClosed,
            AppEvent::Failed(FailureKind::Internal),
            AppEvent::User(UserAction::SelectDevice(DEVICE)),
            AppEvent::User(UserAction::Disconnect),
        ] {
            assert!(update(&mut model, event).is_empty());
            assert_eq!(model.state(), AppState::Error(FailureKind::Startup));
        }
    }

    #[test]
    fn incoming_pairing_is_rejected_while_a_connection_is_active() {
        for state in all_states() {
            if !state.is_pairing() && !state.has_session() {
                continue;
            }

            let mut model = model_in(state);

            assert_eq!(
                update(&mut model, AppEvent::IncomingPairingRequest(peer())),
                vec![Effect::RejectPairingBusy],
                "state: {state:?}"
            );
            assert_eq!(model.state(), state);
        }
    }

    #[test]
    fn session_close_returns_every_connected_state_to_browsing() {
        for state in all_states() {
            if !state.is_pairing() && !state.has_session() {
                continue;
            }
            // A summary that is still on screen survives the close.
            if state == AppState::TransferComplete {
                continue;
            }

            let mut model = model_in(state);

            assert!(update(&mut model, AppEvent::SessionClosed).is_empty());
            assert_eq!(model.state(), AppState::Browsing, "state: {state:?}");
        }
    }

    #[test]
    fn a_cancelled_proposal_summary_returns_to_idle_when_dismissed() {
        let selection = FileSelection::for_test(&[("report.txt", 64)]);
        let mut model = model_in(AppState::SessionIdle);
        update(
            &mut model,
            AppEvent::User(UserAction::StartTransfer(selection.clone())),
        );
        let cancelled = TransferSummary::new(
            TransferDirection::Sent,
            vec![FileEntry::new("report.txt".to_owned(), 64)],
            None,
            Some("peer".to_owned()),
            std::time::Duration::ZERO,
        )
        .cancelled(false);

        update(&mut model, AppEvent::TransferCompleted(cancelled));
        assert_eq!(model.state(), AppState::TransferComplete);
        assert!(model.summary().unwrap().cancelled);
        // Files that never went out stay queued for another `/send`.
        assert_eq!(model.deferred_selection(), Some(&selection));

        update(&mut model, AppEvent::User(UserAction::DismissSummary));
        assert_eq!(model.state(), AppState::SessionIdle);
        assert!(model.summary().is_none());
    }

    #[test]
    fn a_summary_survives_a_session_close_until_dismissed() {
        let mut model = model_in(AppState::TransferringInbound);
        update(&mut model, AppEvent::TransferCompleted(summary()));
        assert_eq!(model.state(), AppState::TransferComplete);

        update(&mut model, AppEvent::SessionClosed);
        assert_eq!(model.state(), AppState::TransferComplete);
        assert!(model.summary().unwrap().session_closed);

        update(&mut model, AppEvent::User(UserAction::DismissSummary));
        assert_eq!(model.state(), AppState::Browsing);
        assert!(model.summary().is_none());
    }

    #[test]
    fn shutdown_from_every_state_emits_once_and_wins_later_races() {
        for state in all_states() {
            let mut model = model_in(state);
            let first = update(&mut model, AppEvent::ShutdownRequested);
            let second = update(&mut model, AppEvent::User(UserAction::Quit));
            let stale = update(&mut model, AppEvent::User(UserAction::SelectDevice(DEVICE)));

            if state == AppState::ShuttingDown {
                assert!(first.is_empty());
            } else {
                assert_eq!(first, vec![Effect::Shutdown]);
            }
            assert_eq!(model.state(), AppState::ShuttingDown);
            assert!(second.is_empty());
            assert!(stale.is_empty());
        }
    }

    fn all_states() -> [AppState; 18] {
        [
            AppState::Starting,
            AppState::Browsing,
            AppState::PairingOutbound,
            AppState::PairingOutboundAccepted,
            AppState::PairingInbound,
            AppState::PairingInboundAccepted,
            AppState::PairingConfirming,
            AppState::ClosingPairing,
            AppState::SessionIdle,
            AppState::OutboundProposal,
            AppState::InboundProposal,
            AppState::InboundProposalAccepted,
            AppState::TransferringOutbound,
            AppState::TransferringInbound,
            AppState::TransferComplete,
            AppState::ClosingSession,
            AppState::Error(FailureKind::Internal),
            AppState::ShuttingDown,
        ]
    }

    #[test]
    fn a_withdrawn_local_proposal_stays_queued_for_an_explicit_send() {
        let selection = FileSelection::for_test(&[("report.txt", 64)]);
        let mut model = model_in(AppState::SessionIdle);
        update(
            &mut model,
            AppEvent::User(UserAction::StartTransfer(selection.clone())),
        );
        assert_eq!(model.state(), AppState::OutboundProposal);

        // The initiator-priority collision moves the local files to the queue
        // while the peer's request is reviewed.
        update(&mut model, AppEvent::IncomingTransferRequest(proposal()));
        assert_eq!(model.state(), AppState::InboundProposal);
        assert_eq!(model.deferred_selection(), Some(&selection));
        assert!(model.outbound_selection().is_none());
        assert!(model.transfer_proposal().is_some());

        // Ending the peer's proposal returns to idle with the files still queued.
        update(&mut model, AppEvent::ProposalRejected);
        assert_eq!(model.state(), AppState::SessionIdle);
        assert_eq!(model.deferred_selection(), Some(&selection));

        // A new explicit send moves the queued files back into the proposal.
        update(
            &mut model,
            AppEvent::User(UserAction::StartTransfer(selection.clone())),
        );
        assert_eq!(model.state(), AppState::OutboundProposal);
        assert!(model.deferred_selection().is_none());
        assert_eq!(model.outbound_selection(), Some(&selection));
    }

    #[test]
    fn a_failed_folder_preparation_defers_the_selection_and_shows_a_notice() {
        use crate::transfer::selection::PrepareError;

        let selection = FileSelection::for_test(&[("docs.zip", 1_234)]);
        let mut model = model_in(AppState::SessionIdle);
        update(
            &mut model,
            AppEvent::User(UserAction::StartTransfer(selection.clone())),
        );
        assert_eq!(model.state(), AppState::OutboundProposal);

        update(
            &mut model,
            AppEvent::PreparationFailed(PrepareError::TooLarge),
        );
        assert_eq!(model.state(), AppState::SessionIdle);
        assert_eq!(model.deferred_selection(), Some(&selection));
        assert_eq!(
            model.transfer_notice(),
            Some(PrepareError::TooLarge.notice())
        );

        // A new explicit send clears the previous notice.
        update(
            &mut model,
            AppEvent::User(UserAction::StartTransfer(selection)),
        );
        assert_eq!(model.state(), AppState::OutboundProposal);
        assert!(model.transfer_notice().is_none());
    }

    #[test]
    fn progress_is_tracked_only_while_a_transfer_runs() {
        let mut model = model_in(AppState::SessionIdle);
        let progress = crate::app::model::TransferProgress {
            index: 1,
            files: 3,
            file_size: 256,
            transferred: 128,
            total_size: 1_024,
            total_transferred: 384,
            elapsed: std::time::Duration::from_secs(1),
        };

        assert!(update(&mut model, AppEvent::TransferProgress(progress)).is_empty());
        assert!(model.transfer_progress().is_none());

        let mut model = model_in(AppState::TransferringOutbound);
        update(&mut model, AppEvent::TransferProgress(progress));
        assert_eq!(model.transfer_progress(), Some(progress));
        update(&mut model, AppEvent::TransferCompleted(summary()));
        assert_eq!(model.state(), AppState::TransferComplete);
        assert!(model.transfer_progress().is_none());
        assert!(model.summary().is_some());

        update(&mut model, AppEvent::User(UserAction::DismissSummary));
        assert_eq!(model.state(), AppState::SessionIdle);
        assert!(model.summary().is_none());
    }
}
