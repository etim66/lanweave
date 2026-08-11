//! Process composition, task supervision, and ordered shutdown.

use crate::app::event::{AppEvent, Effect};
use crate::app::model::AppState;
use crate::app::runtime::{AppRuntime, EffectReceiver, effect_channel, event_channel};

pub(crate) async fn run() -> anyhow::Result<()> {
    let mut terminal = crate::tui::TerminalSession::start()?;
    let (event_sender, event_receiver) = event_channel();
    let (effect_sender, effect_receiver) = effect_channel();
    let (stop_sender, stop_receiver) = tokio::sync::watch::channel(false);

    event_sender.send(AppEvent::StartupCompleted).await?;
    let input_sender = event_sender.clone();
    drop(event_sender);

    let input = tokio::spawn(crate::tui::run_events(input_sender, stop_receiver));
    let effects = tokio::spawn(dispatch_effects(effect_receiver));

    let runtime_result = AppRuntime::new(event_receiver, effect_sender)
        .run_with_observer(|model, ui| terminal.draw(model, ui))
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

async fn dispatch_effects(mut effects: EffectReceiver) -> bool {
    while let Some(effect) = effects.recv().await {
        match effect {
            Effect::Shutdown => return true,
            Effect::Connect(_)
            | Effect::AcceptPairing
            | Effect::RejectPairing
            | Effect::RejectPairingBusy
            | Effect::StartTransfer
            | Effect::AcceptTransfer
            | Effect::RejectTransfer
            | Effect::Disconnect => {}
        }
    }
    false
}
