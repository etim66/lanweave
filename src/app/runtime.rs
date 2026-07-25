//! Bounded channels and the single owner of mutable application state.

use tokio::sync::mpsc;

use super::error::AppResult;
use super::model::{AppEvent, AppModel, AppState, Effect};
use super::reducer::{MAX_EFFECTS_PER_EVENT, update};

/// Accommodates short input and discovery bursts without hiding sustained load.
/// High-frequency replaceable events, such as ticks, should be coalesced by
/// their producers rather than increasing this capacity.
pub const APP_EVENT_CHANNEL_CAPACITY: usize = 32;

/// Keeps effect handlers close enough to the reducer for backpressure to reach
/// event producers before substantial work accumulates.
pub const APP_EFFECT_CHANNEL_CAPACITY: usize = 16;

pub type EventSender = mpsc::Sender<AppEvent>;
pub type EventReceiver = mpsc::Receiver<AppEvent>;
pub type EffectSender = mpsc::Sender<Effect>;
pub type EffectReceiver = mpsc::Receiver<Effect>;

pub fn event_channel() -> (EventSender, EventReceiver) {
    mpsc::channel(APP_EVENT_CHANNEL_CAPACITY)
}

pub fn effect_channel() -> (EffectSender, EffectReceiver) {
    mpsc::channel(APP_EFFECT_CHANNEL_CAPACITY)
}

/// Owns the event receiver and is the only runtime component that mutates the
/// application model.
pub struct AppRuntime {
    model: AppModel,
    events: EventReceiver,
    effects: EffectSender,
}

impl AppRuntime {
    pub fn new(events: EventReceiver, effects: EffectSender) -> Self {
        Self {
            model: AppModel::new(),
            events,
            effects,
        }
    }

    /// Processes events in arrival order until shutdown begins.
    ///
    /// Effect sends are awaited so a slow handler applies backpressure instead
    /// of allowing work to grow without a bound.
    pub async fn run(mut self) -> AppResult<AppModel> {
        while let Some(event) = self.events.recv().await {
            self.apply(event).await?;

            if self.model.state() == AppState::ShuttingDown {
                return Ok(self.model);
            }
        }

        // Losing every producer still follows the normal idempotent cleanup path.
        self.apply(AppEvent::ShutdownRequested).await?;
        Ok(self.model)
    }

    async fn apply(&mut self, event: AppEvent) -> AppResult<()> {
        let effects = update(&mut self.model, event);
        debug_assert!(effects.len() <= MAX_EFFECTS_PER_EVENT);

        for effect in effects {
            self.effects
                .send(effect)
                .await
                .map_err(|_| anyhow::anyhow!("application effect handler stopped"))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::error::TrySendError;

    use super::{
        APP_EFFECT_CHANNEL_CAPACITY, APP_EVENT_CHANNEL_CAPACITY, AppRuntime, effect_channel,
        event_channel,
    };
    use crate::app::model::{AppEvent, AppState, DeviceId, Effect, UserAction};

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
        let device = DeviceId::new(11);

        for event in [
            AppEvent::StartupCompleted,
            AppEvent::User(UserAction::SelectDevice(device)),
            AppEvent::PairingSucceeded,
            AppEvent::User(UserAction::StartTransfer),
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
        assert_eq!(effect_receiver.recv().await, Some(Effect::Connect(device)));
        assert_eq!(effect_receiver.recv().await, Some(Effect::StartTransfer));
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
}
