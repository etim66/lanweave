//! Process composition, task supervision, and ordered shutdown.

use crate::app::event::{AppEvent, Effect};
use crate::app::model::AppState;
use crate::app::runtime::{AppRuntime, EffectReceiver, EventSender, effect_channel, event_channel};
use crate::discovery::{
    DiscoveryReceiver, DiscoveryService, MdnsDiscoveryService, event_channel as discovery_channel,
};

pub(crate) async fn run() -> anyhow::Result<()> {
    let mut terminal = crate::tui::TerminalSession::start()?;
    let (event_sender, event_receiver) = event_channel();
    let (effect_sender, effect_receiver) = effect_channel();
    let (stop_sender, stop_receiver) = tokio::sync::watch::channel(false);
    let (discovery_sender, discovery_receiver) = discovery_channel();
    let mut discovery = MdnsDiscoveryService::new();
    discovery.start(discovery_sender)?;

    event_sender.send(AppEvent::StartupCompleted).await?;
    let input_sender = event_sender.clone();
    let discovery_event_sender = event_sender.clone();
    drop(event_sender);

    let input = tokio::spawn(crate::tui::run_events(input_sender, stop_receiver));
    let discovery_events =
        tokio::spawn(relay_discovery(discovery_receiver, discovery_event_sender));
    let effects = tokio::spawn(dispatch_effects(effect_receiver, discovery));

    let runtime_result = AppRuntime::new(event_receiver, effect_sender)
        .run_with_observer(|model, ui| terminal.draw(model, ui))
        .await;
    let _ = stop_sender.send(true);

    let input_result = input.await;
    let effects_result = effects.await;
    let discovery_result = discovery_events.await;
    let restore_result = terminal.restore();

    let model = runtime_result?;
    input_result??;
    let shutdown_handled = effects_result??;
    discovery_result??;
    restore_result?;

    if !shutdown_handled {
        anyhow::bail!("application effect handler stopped before shutdown");
    }
    debug_assert_eq!(model.state(), AppState::ShuttingDown);
    Ok(())
}

async fn relay_discovery(
    mut discovery: DiscoveryReceiver,
    events: EventSender,
) -> anyhow::Result<()> {
    while let Some(event) = discovery.recv().await {
        if events.send(AppEvent::Discovery(event)).await.is_err() {
            return Ok(());
        }
    }
    Ok(())
}

async fn dispatch_effects(
    mut effects: EffectReceiver,
    mut discovery: MdnsDiscoveryService,
) -> anyhow::Result<bool> {
    while let Some(effect) = effects.recv().await {
        match effect {
            Effect::Shutdown => {
                discovery.stop().await?;
                return Ok(true);
            }
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
    discovery.stop().await?;
    Ok(false)
}
