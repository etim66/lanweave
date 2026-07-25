//! Application state, reducer, and event-loop ownership.

pub mod error;
mod model;
mod reducer;
mod runtime;

pub use model::{AppEvent, AppModel, AppState, DeviceId, Effect, FailureKind, Screen, UserAction};
pub use reducer::{MAX_EFFECTS_PER_EVENT, update};
pub use runtime::{
    APP_EFFECT_CHANNEL_CAPACITY, APP_EVENT_CHANNEL_CAPACITY, AppRuntime, EffectReceiver,
    EffectSender, EventReceiver, EventSender, effect_channel, event_channel,
};

pub async fn run() -> anyhow::Result<()> {
    let (event_sender, event_receiver) = event_channel();
    let (effect_sender, mut effect_receiver) = effect_channel();

    // PR 03 will replace these bootstrap events with terminal and signal input.
    event_sender.send(AppEvent::StartupCompleted).await?;
    event_sender.send(AppEvent::ShutdownRequested).await?;
    drop(event_sender);

    let runtime = AppRuntime::new(event_receiver, effect_sender).run();
    let effects = async move {
        while let Some(effect) = effect_receiver.recv().await {
            if effect == Effect::Shutdown {
                return true;
            }
        }
        false
    };
    let (model, shutdown_handled) = tokio::join!(runtime, effects);
    let model = model?;
    if !shutdown_handled {
        anyhow::bail!("application effect handler stopped before shutdown");
    }

    debug_assert_eq!(model.state(), AppState::ShuttingDown);

    println!("lanweave dev build: no TUI yet");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::run;

    #[tokio::test]
    async fn run_returns_ok() {
        assert!(run().await.is_ok());
    }
}
