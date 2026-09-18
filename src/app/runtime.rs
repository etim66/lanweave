//! Bounded channels and the single owner of mutable application state.

use tokio::sync::mpsc;

use super::error::AppResult;
use super::event::{AppEvent, Effect};
use super::interaction::{self, UiState};
use super::model::{AppModel, AppState};
use super::reducer::{MAX_EFFECTS_PER_EVENT, update};

/// Capacity of the application event channel.
pub const APP_EVENT_CHANNEL_CAPACITY: usize = 32;
/// Capacity of the application effect channel.
pub const APP_EFFECT_CHANNEL_CAPACITY: usize = 16;

pub type EventSender = mpsc::Sender<AppEvent>;
pub type EventReceiver = mpsc::Receiver<AppEvent>;
pub type EffectSender = mpsc::Sender<Effect>;
pub type EffectReceiver = mpsc::Receiver<Effect>;

/// Creates the bounded event channel.
pub fn event_channel() -> (EventSender, EventReceiver) {
    mpsc::channel(APP_EVENT_CHANNEL_CAPACITY)
}

/// Creates the bounded effect channel.
pub fn effect_channel() -> (EffectSender, EffectReceiver) {
    mpsc::channel(APP_EFFECT_CHANNEL_CAPACITY)
}

/// Owns the event receiver and is the only runtime component that mutates the
/// application model.
pub struct AppRuntime {
    model: AppModel,
    ui: UiState,
    events: EventReceiver,
    effects: EffectSender,
}

impl AppRuntime {
    /// Creates a runtime with a fresh model and empty UI state.
    pub fn new(events: EventReceiver, effects: EffectSender) -> Self {
        Self {
            model: AppModel::new(),
            ui: UiState::default(),
            events,
            effects,
        }
    }

    /// Processes events in arrival order until shutdown begins.
    ///
    /// Effect sends are awaited so a slow handler applies backpressure instead
    /// of allowing work to grow without a bound.
    #[cfg(test)]
    pub async fn run(self) -> AppResult<AppModel> {
        self.run_with_observer(|_, _| Ok(())).await
    }

    /// Runs the event loop and exposes immutable model snapshots to a view.
    ///
    /// The observer runs before the first event and after every reduction. This
    /// guarantees that the shutdown view is drawn before terminal teardown.
    pub async fn run_with_observer<F>(mut self, mut observe: F) -> AppResult<AppModel>
    where
        F: FnMut(&AppModel, &UiState) -> AppResult<()>,
    {
        if let Err(error) = observe(&self.model, &self.ui) {
            self.shutdown_after_observer_error().await;
            return Err(error);
        }

        while let Some(event) = self.events.recv().await {
            let effects = self.reduce(event);
            if let Err(error) = observe(&self.model, &self.ui) {
                if self.model.state() == AppState::ShuttingDown {
                    let _ = self.send_effects(effects).await;
                } else {
                    self.shutdown_after_observer_error().await;
                }
                return Err(error);
            }
            self.send_effects(effects).await?;

            if self.model.state() == AppState::ShuttingDown {
                return Ok(self.model);
            }
        }

        // Losing every producer still follows the normal idempotent cleanup path.
        let effects = self.reduce(AppEvent::ShutdownRequested);
        if let Err(error) = observe(&self.model, &self.ui) {
            let _ = self.send_effects(effects).await;
            return Err(error);
        }
        self.send_effects(effects).await?;
        Ok(self.model)
    }

    /// Reduces one event against the model and reconciles the UI state.
    ///
    /// The UI overlay is cleared once shutdown starts so no stale screen
    /// survives into the shutdown view.
    fn reduce(&mut self, event: AppEvent) -> Vec<Effect> {
        let effects = match event {
            AppEvent::KeyInput(input) => {
                match interaction::apply_key_input(&self.model, &mut self.ui, input) {
                    Some(action) => self.apply_user_action(action),
                    None => Vec::new(),
                }
            }
            AppEvent::User(action) => self.apply_user_action(action),
            AppEvent::Paste(text) => {
                interaction::apply_paste(&mut self.ui, &text);
                Vec::new()
            }
            event => update(&mut self.model, event),
        };

        if self.model.state() == AppState::ShuttingDown {
            self.ui.clear();
        } else {
            interaction::reconcile(&self.model, &mut self.ui);
        }
        debug_assert!(effects.len() <= MAX_EFFECTS_PER_EVENT);
        effects
    }

    /// Applies an action to the UI first, forwarding it to the model when the
    /// UI does not consume it.
    fn apply_user_action(&mut self, action: super::action::UserAction) -> Vec<Effect> {
        if interaction::apply_user_action(&mut self.ui, action.clone()) {
            Vec::new()
        } else {
            update(&mut self.model, AppEvent::User(action))
        }
    }

    /// Sends every effect to the handler, failing if the handler is gone.
    async fn send_effects(&self, effects: Vec<Effect>) -> AppResult<()> {
        for effect in effects {
            self.effects
                .send(effect)
                .await
                .map_err(|_| anyhow::anyhow!("application effect handler stopped"))?;
        }

        Ok(())
    }

    /// Starts shutdown after a rendering failure so the app still stops cleanly.
    async fn shutdown_after_observer_error(&mut self) {
        let effects = self.reduce(AppEvent::ShutdownRequested);
        let _ = self.send_effects(effects).await;
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::error::TrySendError;
    use tokio::time::Instant;

    use super::{
        APP_EFFECT_CHANNEL_CAPACITY, APP_EVENT_CHANNEL_CAPACITY, AppRuntime, effect_channel,
        event_channel,
    };
    use crate::app::action::{DeviceId, KeyInput, UserAction};
    use crate::app::event::{AppEvent, Effect};
    use crate::app::model::AppState;
    use crate::discovery::{DiscoveredService, DiscoveryEvent};
    use crate::transfer::selection::FileSelection;

    #[test]
    fn event_channel_is_bounded() {
        let (sender, mut receiver) = event_channel();

        for _ in 0..APP_EVENT_CHANNEL_CAPACITY {
            sender.try_send(AppEvent::StartupCompleted).unwrap();
        }
        assert!(matches!(
            sender.try_send(AppEvent::StartupCompleted),
            Err(TrySendError::Full(_))
        ));

        assert_eq!(receiver.try_recv(), Ok(AppEvent::StartupCompleted));
        sender.try_send(AppEvent::StartupCompleted).unwrap();
    }

    #[test]
    fn effect_channel_is_bounded() {
        let (sender, mut receiver) = effect_channel();

        for _ in 0..APP_EFFECT_CHANNEL_CAPACITY {
            sender.try_send(Effect::Disconnect).unwrap();
        }
        assert!(matches!(
            sender.try_send(Effect::Disconnect),
            Err(TrySendError::Full(_))
        ));

        assert_eq!(receiver.try_recv(), Ok(Effect::Disconnect));
        sender.try_send(Effect::Disconnect).unwrap();
    }

    #[tokio::test]
    async fn runtime_processes_events_and_effects_in_order() {
        let (event_sender, event_receiver) = event_channel();
        let (effect_sender, mut effect_receiver) = effect_channel();
        for event in [
            AppEvent::StartupCompleted,
            AppEvent::Discovery(DiscoveryEvent::Resolved(DiscoveredService::for_test(
                "peer",
                Instant::now(),
            ))),
            AppEvent::User(UserAction::SelectDevice(DeviceId::new(1))),
            AppEvent::PairingSucceeded,
            AppEvent::User(UserAction::StartTransfer(FileSelection::default())),
            AppEvent::ProposalRejected,
            AppEvent::ShutdownRequested,
        ] {
            event_sender.send(event).await.unwrap();
        }
        drop(event_sender);

        let model = AppRuntime::new(event_receiver, effect_sender)
            .run()
            .await
            .unwrap();

        assert_eq!(model.state(), AppState::ShuttingDown);
        assert_eq!(
            effect_receiver.recv().await,
            Some(Effect::Connect(
                crate::app::action::ConnectionTarget::Discovered {
                    address: "127.0.0.1:4242".parse().unwrap(),
                    display_name: "peer".to_owned(),
                }
            ))
        );
        assert_eq!(
            effect_receiver.recv().await,
            Some(Effect::StartTransfer(FileSelection::default()))
        );
        assert_eq!(effect_receiver.recv().await, Some(Effect::Shutdown));
        assert_eq!(effect_receiver.recv().await, None);
    }

    #[tokio::test]
    async fn channel_closure_requests_shutdown_once() {
        let (event_sender, event_receiver) = event_channel();
        let (effect_sender, mut effect_receiver) = effect_channel();
        drop(event_sender);

        let model = AppRuntime::new(event_receiver, effect_sender)
            .run()
            .await
            .unwrap();

        assert_eq!(model.state(), AppState::ShuttingDown);
        assert_eq!(effect_receiver.recv().await, Some(Effect::Shutdown));
        assert_eq!(effect_receiver.recv().await, None);
    }

    #[tokio::test]
    async fn palette_quit_uses_the_application_shutdown_path() {
        let (event_sender, event_receiver) = event_channel();
        let (effect_sender, mut effect_receiver) = effect_channel();

        event_sender.send(AppEvent::StartupCompleted).await.unwrap();
        for input in [
            KeyInput::Character('/'),
            KeyInput::Character('q'),
            KeyInput::Character('u'),
            KeyInput::Character('i'),
            KeyInput::Character('t'),
            KeyInput::Enter,
        ] {
            event_sender.send(AppEvent::KeyInput(input)).await.unwrap();
        }
        drop(event_sender);

        let model = AppRuntime::new(event_receiver, effect_sender)
            .run()
            .await
            .unwrap();

        assert_eq!(model.state(), AppState::ShuttingDown);
        assert_eq!(effect_receiver.recv().await, Some(Effect::Shutdown));
        assert_eq!(effect_receiver.recv().await, None);
    }

    #[tokio::test]
    async fn queued_work_after_shutdown_is_not_processed() {
        let (event_sender, event_receiver) = event_channel();
        let (effect_sender, mut effect_receiver) = effect_channel();

        event_sender.send(AppEvent::StartupCompleted).await.unwrap();
        event_sender
            .send(AppEvent::ShutdownRequested)
            .await
            .unwrap();
        event_sender
            .send(AppEvent::User(UserAction::SelectDevice(DeviceId::new(3))))
            .await
            .unwrap();

        let model = AppRuntime::new(event_receiver, effect_sender)
            .run()
            .await
            .unwrap();

        assert_eq!(model.state(), AppState::ShuttingDown);
        assert_eq!(effect_receiver.recv().await, Some(Effect::Shutdown));
        assert_eq!(effect_receiver.recv().await, None);
    }

    #[tokio::test]
    async fn full_effect_queue_applies_backpressure() {
        let (event_sender, event_receiver) = event_channel();
        let (effect_sender, mut effect_receiver) = effect_channel();

        for _ in 0..APP_EFFECT_CHANNEL_CAPACITY {
            effect_sender.try_send(Effect::Disconnect).unwrap();
        }
        event_sender
            .send(AppEvent::ShutdownRequested)
            .await
            .unwrap();
        drop(event_sender);

        let runtime = tokio::spawn(AppRuntime::new(event_receiver, effect_sender).run());
        tokio::task::yield_now().await;
        assert!(!runtime.is_finished());

        assert_eq!(effect_receiver.recv().await, Some(Effect::Disconnect));
        let model = runtime.await.unwrap().unwrap();
        assert_eq!(model.state(), AppState::ShuttingDown);

        for _ in 1..APP_EFFECT_CHANNEL_CAPACITY {
            assert_eq!(effect_receiver.recv().await, Some(Effect::Disconnect));
        }
        assert_eq!(effect_receiver.recv().await, Some(Effect::Shutdown));
        assert_eq!(effect_receiver.recv().await, None);
    }

    #[tokio::test]
    async fn observer_sees_initial_browsing_and_shutdown_states() {
        let (event_sender, event_receiver) = event_channel();
        let (effect_sender, _effect_receiver) = effect_channel();
        let mut observed = Vec::new();

        event_sender.send(AppEvent::StartupCompleted).await.unwrap();
        event_sender
            .send(AppEvent::ShutdownRequested)
            .await
            .unwrap();

        AppRuntime::new(event_receiver, effect_sender)
            .run_with_observer(|model, _| {
                observed.push(model.state());
                Ok(())
            })
            .await
            .unwrap();

        assert_eq!(
            observed,
            [
                AppState::Starting,
                AppState::Browsing,
                AppState::ShuttingDown
            ]
        );
    }

    #[tokio::test]
    async fn observer_sees_discovery_candidate_updates() {
        let (event_sender, event_receiver) = event_channel();
        let (effect_sender, _effect_receiver) = effect_channel();
        let mut observed_counts = Vec::new();

        event_sender
            .send(AppEvent::Discovery(DiscoveryEvent::Resolved(
                DiscoveredService::for_test("peer", Instant::now()),
            )))
            .await
            .unwrap();
        event_sender
            .send(AppEvent::ShutdownRequested)
            .await
            .unwrap();

        AppRuntime::new(event_receiver, effect_sender)
            .run_with_observer(|model, _| {
                observed_counts.push(model.candidates().len());
                Ok(())
            })
            .await
            .unwrap();

        assert_eq!(observed_counts, [0, 1, 1]);
    }

    #[tokio::test]
    async fn device_list_connects_once_and_stale_selection_never_does() {
        let (event_sender, event_receiver) = event_channel();
        let (effect_sender, mut effect_receiver) = effect_channel();

        event_sender.send(AppEvent::StartupCompleted).await.unwrap();
        event_sender
            .send(AppEvent::Discovery(DiscoveryEvent::Resolved(
                DiscoveredService::for_test("peer", Instant::now()),
            )))
            .await
            .unwrap();
        event_sender
            .send(AppEvent::KeyInput(KeyInput::Down))
            .await
            .unwrap();
        event_sender
            .send(AppEvent::Discovery(DiscoveryEvent::Removed {
                service_instance: "peer._lanweave._tcp.local.".to_owned(),
            }))
            .await
            .unwrap();
        event_sender
            .send(AppEvent::KeyInput(KeyInput::Enter))
            .await
            .unwrap();

        // The removed device cannot connect; a fresh record can, exactly once.
        event_sender
            .send(AppEvent::Discovery(DiscoveryEvent::Resolved(
                DiscoveredService::for_test("peer", Instant::now()),
            )))
            .await
            .unwrap();
        for input in [KeyInput::Down, KeyInput::Enter] {
            event_sender.send(AppEvent::KeyInput(input)).await.unwrap();
        }
        drop(event_sender);

        let model = AppRuntime::new(event_receiver, effect_sender)
            .run()
            .await
            .unwrap();

        // The reappeared device gets a fresh id (2) and connects once.
        assert_eq!(model.state(), AppState::ShuttingDown);
        assert_eq!(
            effect_receiver.recv().await,
            Some(Effect::Connect(
                crate::app::action::ConnectionTarget::Discovered {
                    address: "127.0.0.1:4242".parse().unwrap(),
                    display_name: "peer".to_owned(),
                }
            ))
        );
        assert_eq!(effect_receiver.recv().await, Some(Effect::Shutdown));
        assert_eq!(effect_receiver.recv().await, None);
    }

    #[tokio::test]
    async fn pasted_paths_review_and_send_through_the_runtime() {
        let root =
            std::env::temp_dir().join(format!("lanweave-runtime-{:016x}", fastrand::u64(..)));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("payload.txt");
        std::fs::write(&file, b"data").unwrap();

        let (event_sender, event_receiver) = event_channel();
        let (effect_sender, mut effect_receiver) = effect_channel();

        for event in [
            AppEvent::StartupCompleted,
            AppEvent::Discovery(DiscoveryEvent::Resolved(DiscoveredService::for_test(
                "peer",
                Instant::now(),
            ))),
            AppEvent::KeyInput(KeyInput::Down),
            AppEvent::KeyInput(KeyInput::Enter),
            AppEvent::PairingSucceeded,
            AppEvent::User(UserAction::OpenFileSelection),
            AppEvent::Paste(file.display().to_string()),
            AppEvent::KeyInput(KeyInput::Enter),
            AppEvent::KeyInput(KeyInput::Enter),
        ] {
            event_sender.send(event).await.unwrap();
        }
        drop(event_sender);

        let model = AppRuntime::new(event_receiver, effect_sender)
            .run()
            .await
            .unwrap();

        assert_eq!(model.state(), AppState::ShuttingDown);
        assert!(matches!(
            effect_receiver.recv().await,
            Some(Effect::Connect(_))
        ));
        match effect_receiver.recv().await {
            Some(Effect::StartTransfer(selection)) => {
                assert_eq!(selection.len(), 1);
                assert_eq!(selection.files()[0].name(), "payload.txt");
                assert_eq!(selection.files()[0].size(), 4);
            }
            other => panic!("unexpected effect: {other:?}"),
        }
        assert_eq!(effect_receiver.recv().await, Some(Effect::Shutdown));
        assert_eq!(effect_receiver.recv().await, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn observer_error_stops_the_runtime() {
        let (_event_sender, event_receiver) = event_channel();
        let (effect_sender, mut effect_receiver) = effect_channel();

        let error = AppRuntime::new(event_receiver, effect_sender)
            .run_with_observer(|_, _| Err(anyhow::anyhow!("injected render failure")))
            .await
            .unwrap_err();

        assert_eq!(error.to_string(), "injected render failure");
        assert_eq!(effect_receiver.recv().await, Some(Effect::Shutdown));
        assert_eq!(effect_receiver.recv().await, None);
    }
}
