use lanweave::app::{
    AppEvent, AppRuntime, AppState, Effect, UserAction, effect_channel, event_channel,
};

#[tokio::test]
async fn user_quit_produces_one_clean_shutdown_effect() {
    let (events, event_receiver) = event_channel();
    let (effect_sender, mut effects) = effect_channel();

    events.send(AppEvent::StartupCompleted).await.unwrap();
    events.send(AppEvent::User(UserAction::Quit)).await.unwrap();
    events.send(AppEvent::ShutdownRequested).await.unwrap();
    drop(events);

    let model = AppRuntime::new(event_receiver, effect_sender)
        .run()
        .await
        .unwrap();

    assert_eq!(model.state(), AppState::ShuttingDown);
    assert_eq!(effects.recv().await, Some(Effect::Shutdown));
    assert_eq!(effects.recv().await, None);
}
