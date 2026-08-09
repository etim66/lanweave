//! Terminal-independent interaction state and key handling.

use super::action::{KeyInput, UserAction};
use super::command_palette::{
    CommandId, MAX_COMMAND_QUERY_CHARS, first_visible, move_selection, reconcile_selection, resolve,
};
use super::model::AppModel;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandPalette {
    pub(crate) query: String,
    pub(crate) selected: Option<CommandId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Overlay {
    CommandPalette(CommandPalette),
    Help,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UiState {
    overlay: Option<Overlay>,
}

impl UiState {
    pub(crate) const fn overlay(&self) -> Option<&Overlay> {
        self.overlay.as_ref()
    }

    pub(super) fn clear(&mut self) {
        self.overlay = None;
    }
}

pub(crate) fn apply_key_input(
    model: &AppModel,
    ui: &mut UiState,
    input: KeyInput,
) -> Option<UserAction> {
    match ui.overlay.take() {
        Some(Overlay::CommandPalette(mut palette)) => {
            match input {
                KeyInput::Character(character) => {
                    if palette.query.chars().count() < MAX_COMMAND_QUERY_CHARS {
                        palette.query.push(character);
                    }
                    palette.selected = first_visible(model.capabilities(), &palette.query);
                }
                KeyInput::Backspace => {
                    palette.query.pop();
                    palette.selected = first_visible(model.capabilities(), &palette.query);
                }
                KeyInput::Up => {
                    palette.selected = move_selection(
                        model.capabilities(),
                        &palette.query,
                        palette.selected,
                        false,
                    );
                }
                KeyInput::Down => {
                    palette.selected = move_selection(
                        model.capabilities(),
                        &palette.query,
                        palette.selected,
                        true,
                    );
                }
                KeyInput::Enter => {
                    if let Some(action) =
                        resolve(model.capabilities(), &palette.query, palette.selected)
                    {
                        return Some(action);
                    }
                }
                KeyInput::Escape => return None,
            }
            ui.overlay = Some(Overlay::CommandPalette(palette));
            None
        }
        Some(Overlay::Help) => match input {
            KeyInput::Escape => None,
            KeyInput::Character('/') => {
                open_palette(model, ui);
                None
            }
            KeyInput::Character(character) if character.eq_ignore_ascii_case(&'q') => {
                Some(UserAction::Quit)
            }
            _ => {
                ui.overlay = Some(Overlay::Help);
                None
            }
        },
        None => match input {
            KeyInput::Character('/') => {
                open_palette(model, ui);
                None
            }
            KeyInput::Character(character) if character.eq_ignore_ascii_case(&'q') => {
                Some(UserAction::Quit)
            }
            _ => None,
        },
    }
}

pub(crate) fn apply_user_action(ui: &mut UiState, action: UserAction) -> bool {
    match action {
        UserAction::ShowHelp => {
            ui.overlay = Some(Overlay::Help);
            true
        }
        UserAction::ShowDevices => {
            ui.overlay = None;
            true
        }
        _ => false,
    }
}

pub(super) fn reconcile(model: &AppModel, ui: &mut UiState) {
    let Some(Overlay::CommandPalette(palette)) = ui.overlay.as_mut() else {
        return;
    };
    palette.selected = reconcile_selection(model.capabilities(), &palette.query, palette.selected);
}

fn open_palette(model: &AppModel, ui: &mut UiState) {
    let query = String::new();
    let selected = first_visible(model.capabilities(), &query);
    ui.overlay = Some(Overlay::CommandPalette(CommandPalette { query, selected }));
}

#[cfg(test)]
mod tests {
    use super::{Overlay, UiState, apply_key_input, apply_user_action, reconcile};
    use crate::app::action::{KeyInput, UserAction};
    use crate::app::model::{AppModel, AppState};

    #[test]
    fn palette_query_is_bounded_and_q_is_contextual() {
        let model = AppModel::for_test(AppState::Browsing);
        let mut ui = UiState::default();
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Character('/')),
            None
        );
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Character('q')),
            None
        );

        for _ in 0..super::MAX_COMMAND_QUERY_CHARS + 10 {
            apply_key_input(&model, &mut ui, KeyInput::Character('x'));
        }

        let Some(Overlay::CommandPalette(palette)) = ui.overlay() else {
            panic!("palette should remain open");
        };
        assert_eq!(
            palette.query.chars().count(),
            super::MAX_COMMAND_QUERY_CHARS
        );

        apply_key_input(&model, &mut ui, KeyInput::Escape);
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Character('q')),
            Some(UserAction::Quit)
        );
    }

    #[test]
    fn palette_resolves_actions_and_reconciles_state_changes() {
        let session = AppModel::for_test(AppState::SessionIdle);
        let mut ui = UiState::default();
        apply_key_input(&session, &mut ui, KeyInput::Character('/'));
        for character in "send".chars() {
            apply_key_input(&session, &mut ui, KeyInput::Character(character));
        }
        assert_eq!(
            apply_key_input(&session, &mut ui, KeyInput::Enter),
            Some(UserAction::StartTransfer)
        );
        assert_eq!(ui.overlay(), None);

        apply_key_input(&session, &mut ui, KeyInput::Character('/'));
        for character in "send".chars() {
            apply_key_input(&session, &mut ui, KeyInput::Character(character));
        }
        let busy = AppModel::for_test(AppState::OutboundProposal);
        reconcile(&busy, &mut ui);
        assert_eq!(apply_key_input(&busy, &mut ui, KeyInput::Enter), None);
        assert!(matches!(ui.overlay(), Some(Overlay::CommandPalette(_))));
    }

    #[test]
    fn help_and_devices_only_change_ui_state() {
        let mut ui = UiState::default();
        assert!(apply_user_action(&mut ui, UserAction::ShowHelp));
        assert_eq!(ui.overlay(), Some(&Overlay::Help));
        assert!(apply_user_action(&mut ui, UserAction::ShowDevices));
        assert_eq!(ui.overlay(), None);
        assert!(!apply_user_action(&mut ui, UserAction::Quit));
    }
}
