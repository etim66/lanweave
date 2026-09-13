//! Process composition, task supervision, and ordered shutdown.

use crate::app::event::{AppEvent, Effect};
use crate::app::model::AppState;
use crate::app::runtime::{AppRuntime, EffectReceiver, EventSender, effect_channel, event_channel};
use crate::discovery::{
    DiscoveryReceiver, DiscoveryService, LocalListener, MdnsDiscoveryService,
    event_channel as discovery_channel,
};
use crate::session::{SessionCommand, SessionService, accepted_channel};

/// Runs Lanweave until shutdown completes.
///
/// Owns every long-lived resource: the terminal session, the TCP listener, the
/// mDNS advertiser, the channels, and the background tasks. On return, all
/// resources are stopped and the terminal is restored.
pub(crate) async fn run() -> anyhow::Result<()> {
    // --- Setup ---------------------------------------------------------------
    // Terminal, channels, and network services must be in place before any
    // task starts, so no task can race against an uninitialized resource.
    let mut terminal = crate::tui::TerminalSession::start()?;
    let (event_sender, event_receiver) = event_channel();
    let (effect_sender, effect_receiver) = effect_channel();
    let (stop_sender, stop_receiver) = tokio::sync::watch::channel(false);
    let (discovery_sender, discovery_receiver) = discovery_channel();

    let mut listener = LocalListener::bind()
        .map_err(|error| anyhow::anyhow!("failed to bind the local TCP listener: {error}"))?;
    let (accepted_sender, accepted_receiver) = accepted_channel();
    listener.start(event_sender.clone(), accepted_sender)?;

    let mut discovery = MdnsDiscoveryService::new();
    if let Err(error) = discovery.start(discovery_sender, listener.port()) {
        let _ = listener.stop().await;
        return Err(error);
    }

    // The session owner receives accepted sockets from the listener and the
    // pairing commands produced by the application effects.
    let session = SessionService::start(event_sender.clone(), accepted_receiver);

    // --- Startup signal -----------------------------------------------------
    // Announce that bootstrap is complete, then split the sender among the
    // input and discovery tasks. Dropping the original sender guarantees the
    // event loop ends once every producer task has finished.
    event_sender.send(AppEvent::StartupCompleted).await?;
    let input_sender = event_sender.clone();
    let discovery_event_sender = event_sender.clone();
    drop(event_sender);

    // --- Background tasks ---------------------------------------------------
    // Each task owns one side of the runtime:
    // - the TUI input loop, which stops on the shutdown signal;
    // - the discovery relay, which forwards mDNS events into the app;
    // - the effect dispatcher, which drives the network services.
    let input = tokio::spawn(crate::tui::run_events(input_sender, stop_receiver));
    let discovery_events =
        tokio::spawn(relay_discovery(discovery_receiver, discovery_event_sender));
    let effects = tokio::spawn(dispatch_effects(
        effect_receiver,
        session,
        discovery,
        listener,
    ));

    // --- Event loop ---------------------------------------------------------
    // The runtime is the main process: it consumes events, reduces the model,
    // and redraws the terminal. Everything else works around it.
    let runtime_result = AppRuntime::new(event_receiver, effect_sender)
        .run_with_observer(|model, ui| terminal.draw(model, ui))
        .await;

    // --- Shutdown -----------------------------------------------------------
    // Stop the input loop first, then join every task so resources are
    // released in order before the terminal is restored.
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

/// Relays discovery events from the mDNS service into the application event loop.
///
/// Stops when the discovery channel closes or the application no longer
/// accepts events, which only happens once the runtime has shut down.
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

/// Executes the side effects requested by the application reducer.
///
/// A shutdown effect stops the session owner and the network services, then
/// reports that shutdown was handled. If the channel closes first, everything
/// is stopped anyway and `Ok(false)` is returned. Transfer effects stay
/// no-ops until the transfer feature lands.
async fn dispatch_effects(
    mut effects: EffectReceiver,
    session: SessionService,
    mut discovery: MdnsDiscoveryService,
    mut listener: LocalListener,
) -> anyhow::Result<bool> {
    while let Some(effect) = effects.recv().await {
        match effect {
            Effect::Shutdown => {
                session.stop().await;
                stop_network_services(&mut discovery, &mut listener).await?;
                return Ok(true);
            }
            Effect::Connect(target) => {
                session.send(SessionCommand::Connect(target)).await?;
            }
            Effect::AcceptPairing => {
                session.send(SessionCommand::AcceptPairing).await?;
            }
            Effect::RejectPairing => {
                session.send(SessionCommand::RejectPairing).await?;
            }
            Effect::RejectPairingBusy => {
                session.send(SessionCommand::RejectPairingBusy).await?;
            }
            Effect::SubmitPairingCode(code) => {
                session
                    .send(SessionCommand::SubmitPairingCode(code))
                    .await?;
            }
            Effect::Disconnect => {
                session.send(SessionCommand::Disconnect).await?;
            }
            Effect::StartTransfer | Effect::AcceptTransfer | Effect::RejectTransfer => {}
        }
    }

    session.stop().await;
    stop_network_services(&mut discovery, &mut listener).await?;
    Ok(false)
}

/// Stops advertising and listening so peers can no longer reach this device.
async fn stop_network_services(
    discovery: &mut MdnsDiscoveryService,
    listener: &mut LocalListener,
) -> anyhow::Result<()> {
    // Stop advertising first so new peers no longer receive a usable route.
    let discovery_result = discovery.stop().await;
    let listener_result = listener.stop().await;
    discovery_result.and(listener_result)
}
