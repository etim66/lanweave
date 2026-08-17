//! Process composition, task supervision, and ordered shutdown.

use crate::app::event::{AppEvent, Effect};
use crate::app::model::AppState;
use crate::app::runtime::{AppRuntime, EffectReceiver, EventSender, effect_channel, event_channel};
use crate::discovery::{
    DiscoveryReceiver, DiscoveryService, LocalListener, MdnsDiscoveryService,
    event_channel as discovery_channel,
};

pub(crate) async fn run() -> anyhow::Result<()> {
    let mut terminal = crate::tui::TerminalSession::start()?;
    let (event_sender, event_receiver) = event_channel();
    let (effect_sender, effect_receiver) = effect_channel();
    let (stop_sender, stop_receiver) = tokio::sync::watch::channel(false);
    let (discovery_sender, discovery_receiver) = discovery_channel();
    let mut listener = LocalListener::bind()
        .map_err(|error| anyhow::anyhow!("failed to bind the local TCP listener: {error}"))?;
    listener.start(event_sender.clone())?;
    let mut discovery = MdnsDiscoveryService::new();
    if let Err(error) = discovery.start(discovery_sender, listener.port()) {
        let _ = listener.stop().await;
        return Err(error);
    }

    event_sender.send(AppEvent::StartupCompleted).await?;
    let input_sender = event_sender.clone();
    let discovery_event_sender = event_sender.clone();
    drop(event_sender);

    let input = tokio::spawn(crate::tui::run_events(input_sender, stop_receiver));
    let discovery_events =
        tokio::spawn(relay_discovery(discovery_receiver, discovery_event_sender));
    let effects = tokio::spawn(dispatch_effects(effect_receiver, discovery, listener));

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
    mut listener: LocalListener,
) -> anyhow::Result<bool> {
    while let Some(effect) = effects.recv().await {
        match effect {
            Effect::Shutdown => {
                stop_network_services(&mut discovery, &mut listener).await?;
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
    stop_network_services(&mut discovery, &mut listener).await?;
    Ok(false)
}

async fn stop_network_services(
    discovery: &mut MdnsDiscoveryService,
    listener: &mut LocalListener,
) -> anyhow::Result<()> {
    // Stop advertising first so new peers no longer receive a usable route.
    let discovery_result = discovery.stop().await;
    let listener_result = listener.stop().await;
    discovery_result.and(listener_result)
}
