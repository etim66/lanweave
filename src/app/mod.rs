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
    let mut terminal = crate::tui::TerminalSession::start()?;
    let (event_sender, event_receiver) = event_channel();
    let (effect_sender, effect_receiver) = effect_channel();
    let (stop_sender, stop_receiver) = tokio::sync::watch::channel(false);

    event_sender.send(AppEvent::StartupCompleted).await?;
    let input_sender = event_sender.clone();
    drop(event_sender);

    let input = tokio::spawn(crate::tui::run_events(input_sender, stop_receiver));
    let effects = tokio::spawn(handle_effects(effect_receiver));

    let runtime_result = AppRuntime::new(event_receiver, effect_sender)
        .run_with_observer(|model| terminal.draw(model))
        .await;
    let _ = stop_sender.send(true);

    let input_result = input.await?;
    let shutdown_handled = effects.await?;
    let restore_result = terminal.restore();

    let model = runtime_result?;
    input_result?;
    restore_result?;

    if !shutdown_handled {
        anyhow::bail!("application effect handler stopped before shutdown");
    }
    debug_assert_eq!(model.state(), AppState::ShuttingDown);
    Ok(())
}

async fn handle_effects(mut effects: EffectReceiver) -> bool {
    while let Some(effect) = effects.recv().await {
        if effect == Effect::Shutdown {
            return true;
        }
    }
    false
}
