//! Synchronous application state transitions.

use super::model::{AppEvent, AppModel, AppState, Effect, UserAction};

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
    if matches!(
        event,
        AppEvent::ShutdownRequested | AppEvent::User(UserAction::Quit)
    ) {
        return begin_shutdown(model);
    }

    if model.state() == AppState::ShuttingDown {
        return Vec::new();
    }

    let effect = match (model.state(), event) {
        (AppState::Starting, AppEvent::StartupCompleted) => {
            model.transition_to(AppState::Browsing);
            None
        }
        (AppState::Browsing, AppEvent::User(UserAction::SelectDevice(device))) => {
            model.transition_to(AppState::PairingOutbound);
            Some(Effect::Connect(device))
        }
        (AppState::Browsing, AppEvent::IncomingPairingRequest) => {
            model.transition_to(AppState::PairingInbound);
            None
        }
        (AppState::PairingInbound, AppEvent::User(UserAction::AcceptPairing)) => {
            model.transition_to(AppState::PairingInboundAccepted);
            Some(Effect::AcceptPairing)
        }
        (AppState::PairingInbound, AppEvent::User(UserAction::RejectPairing)) => {
            model.transition_to(AppState::ClosingPairing);
            Some(Effect::RejectPairing)
        }
        (
            AppState::PairingOutbound | AppState::PairingInboundAccepted,
            AppEvent::PairingSucceeded,
        ) => {
            model.transition_to(AppState::SessionIdle);
            None
        }
        (state, AppEvent::PairingEnded) if state.is_pairing() => {
            model.transition_to(AppState::Browsing);
            None
        }
        (AppState::SessionIdle, AppEvent::User(UserAction::StartTransfer)) => {
            model.transition_to(AppState::OutboundProposal);
            Some(Effect::StartTransfer)
        }
        (AppState::SessionIdle, AppEvent::IncomingTransferRequest) => {
            model.transition_to(AppState::InboundProposal);
            None
        }
        (AppState::OutboundProposal, AppEvent::TransferStarted) => {
            model.transition_to(AppState::TransferringOutbound);
            None
        }
        (AppState::OutboundProposal, AppEvent::ProposalRejected) => {
            model.transition_to(AppState::SessionIdle);
            None
        }
        (AppState::InboundProposal, AppEvent::User(UserAction::AcceptTransfer)) => {
            model.transition_to(AppState::InboundProposalAccepted);
            Some(Effect::AcceptTransfer)
        }
        (AppState::InboundProposal, AppEvent::User(UserAction::RejectTransfer)) => {
            model.transition_to(AppState::SessionIdle);
            Some(Effect::RejectTransfer)
        }
        (state, AppEvent::TransferFinished) if state.is_transfer_active() => {
            model.transition_to(AppState::SessionIdle);
            None
        }
        (AppState::InboundProposalAccepted, AppEvent::TransferStarted) => {
            model.transition_to(AppState::TransferringInbound);
            None
        }
        (AppState::InboundProposalAccepted, AppEvent::ProposalRejected) => {
            model.transition_to(AppState::SessionIdle);
            None
        }
        (state, AppEvent::IncomingPairingRequest) if state.is_pairing() || state.has_session() => {
            Some(Effect::RejectPairingBusy)
        }
        (state, AppEvent::User(UserAction::Disconnect))
            if state.is_pairing() && state != AppState::ClosingPairing =>
        {
            model.transition_to(AppState::ClosingPairing);
            Some(Effect::Disconnect)
        }
        (state, AppEvent::User(UserAction::Disconnect))
            if state.has_session() && state != AppState::ClosingSession =>
        {
            model.transition_to(AppState::ClosingSession);
            Some(Effect::Disconnect)
        }
        (state, AppEvent::SessionClosed) if state.is_pairing() || state.has_session() => {
            model.transition_to(AppState::Browsing);
            None
        }
        (state, AppEvent::Failed(kind)) if !matches!(state, AppState::Error(_)) => {
            model.transition_to(AppState::Error(kind));
            None
        }
        _ => None,
    };

    effect.into_iter().collect()
}

fn begin_shutdown(model: &mut AppModel) -> Vec<Effect> {
    if model.state() == AppState::ShuttingDown {
        return Vec::new();
    }

    model.transition_to(AppState::ShuttingDown);
    vec![Effect::Shutdown]
}

#[cfg(test)]
mod tests {
    use super::{MAX_EFFECTS_PER_EVENT, update};
    use crate::app::model::{
        AppEvent, AppModel, AppState, DeviceId, Effect, FailureKind, UserAction,
    };

    const DEVICE: DeviceId = DeviceId::new(7);

    fn model_in(state: AppState) -> AppModel {
        let mut model = AppModel::new();
        model.transition_to(state);
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
                AppEvent::User(UserAction::SelectDevice(DEVICE)),
                AppState::PairingOutbound,
                Some(Effect::Connect(DEVICE)),
            ),
            (
                AppState::Browsing,
                AppEvent::IncomingPairingRequest,
                AppState::PairingInbound,
                None,
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
                AppEvent::IncomingPairingRequest,
                AppState::SessionIdle,
                Some(Effect::RejectPairingBusy),
            ),
            (
                AppState::SessionIdle,
                AppEvent::User(UserAction::StartTransfer),
                AppState::OutboundProposal,
                Some(Effect::StartTransfer),
            ),
            (
                AppState::SessionIdle,
                AppEvent::IncomingTransferRequest,
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
                AppEvent::User(UserAction::AcceptTransfer),
                AppState::InboundProposalAccepted,
                Some(Effect::AcceptTransfer),
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
                AppEvent::TransferFinished,
                AppState::SessionIdle,
                None,
            ),
            (
                AppState::TransferringInbound,
                AppEvent::TransferFinished,
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
            let effects = update(&mut model, event);

            assert_eq!(model.state(), expected_state, "event: {event:?}");
            assert_eq!(effects, expected_effect.into_iter().collect::<Vec<_>>());
            assert!(effects.len() <= MAX_EFFECTS_PER_EVENT);
        }
    }

    #[test]
    fn device_selection_only_starts_a_connection_while_browsing() {
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
    fn transfer_only_starts_from_an_idle_session() {
        for state in all_states() {
            if state == AppState::SessionIdle {
                continue;
            }

            let mut model = model_in(state);
            let effects = update(&mut model, AppEvent::User(UserAction::StartTransfer));

            assert_eq!(model.state(), state, "state: {state:?}");
            assert!(effects.is_empty(), "state: {state:?}");
        }
    }

    #[test]
    fn decision_actions_are_ignored_outside_their_prompt() {
        let cases = [
            (UserAction::AcceptPairing, AppState::PairingInbound),
            (UserAction::RejectPairing, AppState::PairingInbound),
            (UserAction::AcceptTransfer, AppState::InboundProposal),
            (UserAction::RejectTransfer, AppState::InboundProposal),
        ];

        for (action, valid_state) in cases {
            for state in all_states() {
                if state == valid_state {
                    continue;
                }

                let mut model = model_in(state);
                let effects = update(&mut model, AppEvent::User(action));

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
            (AppState::PairingOutbound, AppEvent::TransferFinished),
            (AppState::SessionIdle, AppEvent::TransferStarted),
            (AppState::Browsing, AppEvent::SessionClosed),
        ];

        for (state, event) in cases {
            let mut model = model_in(state);

            assert!(update(&mut model, event).is_empty());
            assert_eq!(model.state(), state);
        }
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
            AppEvent::IncomingPairingRequest,
            AppEvent::PairingSucceeded,
            AppEvent::IncomingTransferRequest,
            AppEvent::TransferStarted,
            AppEvent::TransferFinished,
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
                update(&mut model, AppEvent::IncomingPairingRequest),
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

            let mut model = model_in(state);

            assert!(update(&mut model, AppEvent::SessionClosed).is_empty());
            assert_eq!(model.state(), AppState::Browsing, "state: {state:?}");
        }
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

    fn all_states() -> [AppState; 15] {
        [
            AppState::Starting,
            AppState::Browsing,
            AppState::PairingOutbound,
            AppState::PairingInbound,
            AppState::PairingInboundAccepted,
            AppState::ClosingPairing,
            AppState::SessionIdle,
            AppState::OutboundProposal,
            AppState::InboundProposal,
            AppState::InboundProposalAccepted,
            AppState::TransferringOutbound,
            AppState::TransferringInbound,
            AppState::ClosingSession,
            AppState::Error(FailureKind::Internal),
            AppState::ShuttingDown,
        ]
    }
}
