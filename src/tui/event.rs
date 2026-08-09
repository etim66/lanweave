use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};

use crate::app::action::KeyInput;
use crate::app::event::AppEvent;
use crate::app::failure::FailureKind;
use crate::app::runtime::EventSender;

const TICK_RATE: Duration = Duration::from_millis(250);

pub(crate) async fn run_events(
    events: EventSender,
    mut stop: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let mut terminal_events = EventStream::new();
    let mut ticks = tokio::time::interval_at(Instant::now() + TICK_RATE, TICK_RATE);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);

    loop {
        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    return Ok(());
                }
            }
            result = &mut interrupt => {
                result?;
                send_shutdown(&events).await;
                return Ok(());
            }
            event = terminal_events.next() => {
                match event {
                    Some(Ok(event)) => dispatch_terminal_event(&events, event).await,
                    Some(Err(error)) => {
                        // Show the safe error category before taking the normal shutdown path.
                        let _ = events.send(AppEvent::Failed(FailureKind::Internal)).await;
                        send_shutdown(&events).await;
                        return Err(error.into());
                    }
                    None => {
                        send_shutdown(&events).await;
                        return Ok(());
                    }
                }
            }
            _ = ticks.tick() => {
                send_tick(&events);
            }
        }
    }
}

fn send_tick(events: &EventSender) {
    // Other events also redraw the view, so retain at most one standalone tick.
    if events.capacity() == events.max_capacity() {
        let _ = events.try_send(AppEvent::Tick);
    }
}

async fn dispatch_terminal_event(events: &EventSender, event: Event) {
    let app_event = map_terminal_event(event);

    if let Some(event) = app_event {
        if matches!(event, AppEvent::Tick | AppEvent::TerminalResized { .. }) {
            let _ = events.try_send(event);
        } else {
            let _ = events.send(event).await;
        }
    }
}

fn map_terminal_event(event: Event) -> Option<AppEvent> {
    match event {
        Event::Key(key) => map_key(key),
        Event::Resize(width, height) => Some(AppEvent::TerminalResized { width, height }),
        _ => None,
    }
}

fn map_key(key: KeyEvent) -> Option<AppEvent> {
    if key.kind != KeyEventKind::Press {
        return None;
    }

    match key.code {
        KeyCode::Char(character)
            if character.eq_ignore_ascii_case(&'c')
                && key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            Some(AppEvent::ShutdownRequested)
        }
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            Some(AppEvent::KeyInput(KeyInput::Character(character)))
        }
        KeyCode::Backspace if has_no_command_modifier(key.modifiers) => {
            Some(AppEvent::KeyInput(KeyInput::Backspace))
        }
        KeyCode::Enter if has_no_command_modifier(key.modifiers) => {
            Some(AppEvent::KeyInput(KeyInput::Enter))
        }
        KeyCode::Esc if has_no_command_modifier(key.modifiers) => {
            Some(AppEvent::KeyInput(KeyInput::Escape))
        }
        KeyCode::Up if has_no_command_modifier(key.modifiers) => {
            Some(AppEvent::KeyInput(KeyInput::Up))
        }
        KeyCode::Down if has_no_command_modifier(key.modifiers) => {
            Some(AppEvent::KeyInput(KeyInput::Down))
        }
        _ => None,
    }
}

fn has_no_command_modifier(modifiers: KeyModifiers) -> bool {
    !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

async fn send_shutdown(events: &EventSender) {
    let _ = events.send(AppEvent::ShutdownRequested).await;
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use super::{map_key, map_terminal_event, send_tick};
    use crate::app::action::KeyInput;
    use crate::app::event::AppEvent;
    use crate::app::runtime::event_channel;

    fn key(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn text_keys_map_to_context_neutral_input() {
        let cases = [
            (
                key(KeyCode::Char('q'), KeyModifiers::NONE, KeyEventKind::Press),
                AppEvent::KeyInput(KeyInput::Character('q')),
            ),
            (
                key(KeyCode::Char('Q'), KeyModifiers::SHIFT, KeyEventKind::Press),
                AppEvent::KeyInput(KeyInput::Character('Q')),
            ),
            (
                key(KeyCode::Char('/'), KeyModifiers::NONE, KeyEventKind::Press),
                AppEvent::KeyInput(KeyInput::Character('/')),
            ),
        ];

        for (key, expected) in cases {
            assert_eq!(map_key(key), Some(expected));
        }
    }

    #[test]
    fn editing_and_navigation_keys_are_forwarded() {
        let cases = [
            (KeyCode::Backspace, KeyInput::Backspace),
            (KeyCode::Enter, KeyInput::Enter),
            (KeyCode::Esc, KeyInput::Escape),
            (KeyCode::Up, KeyInput::Up),
            (KeyCode::Down, KeyInput::Down),
        ];

        for (code, input) in cases {
            assert_eq!(
                map_key(key(code, KeyModifiers::NONE, KeyEventKind::Press)),
                Some(AppEvent::KeyInput(input))
            );
        }
    }

    #[test]
    fn control_c_maps_to_shutdown_intent() {
        assert_eq!(
            map_key(key(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press,
            )),
            Some(AppEvent::ShutdownRequested)
        );
    }

    #[test]
    fn unrelated_and_non_press_keys_are_ignored() {
        let cases = [
            key(
                KeyCode::Char('q'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press,
            ),
            key(KeyCode::Char('x'), KeyModifiers::ALT, KeyEventKind::Press),
            key(KeyCode::Enter, KeyModifiers::CONTROL, KeyEventKind::Press),
            key(KeyCode::Down, KeyModifiers::ALT, KeyEventKind::Press),
            key(KeyCode::Char('q'), KeyModifiers::NONE, KeyEventKind::Repeat),
            key(
                KeyCode::Char('q'),
                KeyModifiers::NONE,
                KeyEventKind::Release,
            ),
        ];

        for key in cases {
            assert_eq!(map_key(key), None);
        }
    }

    #[test]
    fn resize_is_forwarded_and_unrelated_terminal_events_are_ignored() {
        assert_eq!(
            map_terminal_event(crossterm::event::Event::Resize(120, 40)),
            Some(AppEvent::TerminalResized {
                width: 120,
                height: 40,
            })
        );
        assert_eq!(
            map_terminal_event(crossterm::event::Event::FocusGained),
            None
        );
    }

    #[test]
    fn ticks_are_coalesced_behind_pending_events() {
        let (events, mut receiver) = event_channel();

        send_tick(&events);
        send_tick(&events);

        assert_eq!(receiver.try_recv(), Ok(AppEvent::Tick));
        assert!(receiver.try_recv().is_err());

        events.try_send(AppEvent::StartupCompleted).unwrap();
        send_tick(&events);

        assert_eq!(receiver.try_recv(), Ok(AppEvent::StartupCompleted));
        assert!(receiver.try_recv().is_err());
    }
}
