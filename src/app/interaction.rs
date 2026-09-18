//! Terminal-independent interaction state and key handling.

use std::fmt;

use super::action::{
    DirectAddressError, DirectEndpoint, KeyInput, MAX_DIRECT_ADDRESS_CHARS, UserAction,
};
use super::command_palette::{
    CommandId, MAX_COMMAND_QUERY_CHARS, first_visible, move_selection, reconcile_selection, resolve,
};
use super::model::{AppModel, AppState};
use crate::discovery::truncate_utf8;
use crate::pairing::{CODE_DIGITS, PairingCode};
use crate::transfer::selection::{FileSelection, MAX_SELECTION_INPUT_BYTES, SelectionIssue};

/// Live query and selection for the open command palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandPalette {
    pub(crate) query: String,
    pub(crate) selected: Option<CommandId>,
}

/// Live direct-address input line and its validation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectAddressInput {
    pub(crate) text: String,
    pub(crate) error: Option<DirectAddressError>,
}

impl DirectAddressInput {
    /// Creates an empty input with no error.
    fn new() -> Self {
        Self {
            text: String::new(),
            error: None,
        }
    }

    /// Applies one edit and re-validates the whole line.
    fn edit(&mut self, apply: impl FnOnce(&mut String)) {
        apply(&mut self.text);
        self.error = DirectEndpoint::parse(&self.text).err();
    }
}

/// Live eight-digit pairing-code entry for the initiator.
///
/// The digits are redacted from `Debug` because the entered code is as
/// sensitive as the code shown on the responder's screen.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PairingCodeInput {
    pub(crate) digits: String,
}

impl fmt::Debug for PairingCodeInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PairingCodeInput")
            .field("digits", &"[REDACTED]")
            .finish()
    }
}

impl PairingCodeInput {
    /// Creates an empty input.
    fn new() -> Self {
        Self {
            digits: String::new(),
        }
    }

    /// Appends one digit, bounded to the exact code length.
    fn push(&mut self, character: char) {
        if self.digits.len() < CODE_DIGITS && character.is_ascii_digit() {
            self.digits.push(character);
        }
    }

    /// Removes the last entered digit.
    fn pop(&mut self) {
        self.digits.pop();
    }

    /// Formats the entered digits grouped as `1234 5678`.
    pub(crate) fn grouped(&self) -> String {
        let mut display = String::with_capacity(self.digits.len() + 1);
        for (index, character) in self.digits.chars().enumerate() {
            if index == CODE_DIGITS / 2 {
                display.push(' ');
            }
            display.push(character);
        }
        display
    }
}

/// A full-screen UI surface drawn above the current screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Overlay {
    CommandPalette(CommandPalette),
    DirectAddress(DirectAddressInput),
    PairingCode(PairingCodeInput),
    FileSelection(FileSelectionInput),
    Help,
}

/// Live local file review: pending text, reviewed files, issues, and cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FileSelectionInput {
    /// Pasted or typed paths awaiting review.
    pub(crate) text: String,
    /// Files already validated and reviewed.
    pub(crate) selection: FileSelection,
    /// Rejections from the last review pass.
    pub(crate) issues: Vec<SelectionIssue>,
    /// Highlighted entry for removal.
    pub(crate) selected: Option<usize>,
}

/// Terminal-independent UI state that the view renders.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UiState {
    overlay: Option<Overlay>,
    device_selection: Option<super::action::DeviceId>,
}

impl UiState {
    /// Returns the active overlay, if any.
    pub(crate) const fn overlay(&self) -> Option<&Overlay> {
        self.overlay.as_ref()
    }

    /// Builds UI state around one overlay for rendering tests.
    #[cfg(test)]
    pub(crate) const fn for_test(overlay: Overlay) -> Self {
        Self {
            overlay: Some(overlay),
            device_selection: None,
        }
    }

    /// Returns the currently selected device in the browsing list.
    pub(crate) const fn device_selection(&self) -> Option<super::action::DeviceId> {
        self.device_selection
    }

    /// Closes any open overlay.
    pub(super) fn clear(&mut self) {
        self.overlay = None;
    }
}

/// Interprets one key against the current UI state.
///
/// Returns a user action when the key completes one, and otherwise updates
/// `ui` in place. Typing in the palette is capped at
/// [`MAX_COMMAND_QUERY_CHARS`].
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
        Some(Overlay::DirectAddress(mut address)) => {
            match input {
                KeyInput::Character(character) => {
                    if address.text.chars().count() < MAX_DIRECT_ADDRESS_CHARS {
                        address.edit(|text| text.push(character));
                    }
                }
                KeyInput::Backspace => address.edit(|text| {
                    text.pop();
                }),
                KeyInput::Enter => match DirectEndpoint::parse(&address.text) {
                    Ok(endpoint) => return Some(UserAction::ConnectDirect(endpoint)),
                    Err(error) => address.error = Some(error),
                },
                KeyInput::Escape => return None,
                _ => {}
            }
            ui.overlay = Some(Overlay::DirectAddress(address));
            None
        }
        Some(Overlay::PairingCode(mut code_input)) => {
            match input {
                KeyInput::Character(character) if character.is_ascii_digit() => {
                    code_input.push(character);
                }
                KeyInput::Backspace => code_input.pop(),
                KeyInput::Enter => {
                    if let Some(code) = PairingCode::parse(&code_input.digits) {
                        return Some(UserAction::SubmitPairingCode(code));
                    }
                }
                // Cancelling code entry closes the provisional connection.
                KeyInput::Escape => return Some(UserAction::Disconnect),
                _ => {}
            }
            ui.overlay = Some(Overlay::PairingCode(code_input));
            None
        }
        Some(Overlay::FileSelection(mut files)) => {
            match input {
                KeyInput::Character(character) => {
                    if files.text.len() < MAX_SELECTION_INPUT_BYTES {
                        files.text.push(character);
                    }
                }
                KeyInput::Backspace => {
                    if files.text.pop().is_none() {
                        remove_selected_file(&mut files);
                    }
                }
                KeyInput::Enter => {
                    if files.text.is_empty() {
                        // An empty input sends the reviewed selection when the
                        // session is authorized and idle.
                        if model.capabilities().can_start_transfer
                            && files.selection.request().is_some()
                        {
                            return Some(UserAction::StartTransfer(std::mem::take(
                                &mut files.selection,
                            )));
                        }
                    } else {
                        files.issues = files.selection.add_text(&files.text);
                        files.text.clear();
                        if files.selected.is_none() && !files.selection.is_empty() {
                            files.selected = Some(0);
                        }
                    }
                }
                KeyInput::Up => move_file_selection(&mut files, false),
                KeyInput::Down => move_file_selection(&mut files, true),
                KeyInput::Escape => return None,
            }
            ui.overlay = Some(Overlay::FileSelection(files));
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
            // The in-person pairing prompt is decided with Enter and Escape.
            KeyInput::Enter if model.state() == AppState::PairingInbound => {
                Some(UserAction::AcceptPairing)
            }
            KeyInput::Escape if model.state() == AppState::PairingInbound => {
                Some(UserAction::RejectPairing)
            }
            KeyInput::Up | KeyInput::Down if model.capabilities().can_show_devices => {
                move_device_selection(model, ui, input == KeyInput::Down);
                None
            }
            KeyInput::Enter if model.capabilities().can_show_devices => {
                resolve_selected_device(model, ui)
            }
            _ => None,
        },
    }
}

/// Appends pasted text to the open file review, bounded by the input limit.
///
/// Paste is accepted only where a reviewed path buffer is open; other screens
/// ignore it.
pub(crate) fn apply_paste(ui: &mut UiState, text: &str) {
    let Some(Overlay::FileSelection(files)) = ui.overlay.as_mut() else {
        return;
    };
    let remaining = MAX_SELECTION_INPUT_BYTES.saturating_sub(files.text.len());
    if remaining > 0 {
        files.text.push_str(&truncate_utf8(text, remaining));
    }
}

/// Applies an already resolved action to the UI state.
///
/// Returns `true` when the action changed the UI and `false` when it must be
/// forwarded to the application model instead.
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
        UserAction::OpenDirectAddress => {
            ui.overlay = Some(Overlay::DirectAddress(DirectAddressInput::new()));
            true
        }
        UserAction::OpenFileSelection => {
            ui.overlay = Some(Overlay::FileSelection(FileSelectionInput::default()));
            true
        }
        _ => false,
    }
}

/// Re-validates the UI state after the application model changed.
///
/// The palette selection is kept on the first still-visible command, and a
/// device selection is cleared when its device disappeared from discovery.
/// The initiator's code input opens when the request is accepted and closes
/// as soon as the flow leaves that state.
pub(crate) fn reconcile(model: &AppModel, ui: &mut UiState) {
    if let Some(Overlay::CommandPalette(palette)) = ui.overlay.as_mut() {
        palette.selected =
            reconcile_selection(model.capabilities(), &palette.query, palette.selected);
    }

    if model.state() == AppState::PairingOutboundAccepted {
        if !matches!(ui.overlay, Some(Overlay::PairingCode(_))) {
            ui.overlay = Some(Overlay::PairingCode(PairingCodeInput::new()));
        }
    } else if matches!(ui.overlay, Some(Overlay::PairingCode(_))) {
        ui.overlay = None;
    }

    // The review list is local, but it must never hide a required prompt or a
    // running transfer.
    if matches!(ui.overlay, Some(Overlay::FileSelection(_)))
        && !model.capabilities().can_review_files
    {
        ui.overlay = None;
    }

    if let Some(selected) = ui.device_selection {
        let present = model
            .candidates()
            .iter()
            .any(|candidate| candidate.id() == selected);
        if !present {
            ui.device_selection = None;
        }
    }
}

/// Moves the browsing selection by one sorted candidate, wrapping at the ends.
fn move_device_selection(model: &AppModel, ui: &mut UiState, forward: bool) {
    let candidates = model.sorted_candidates();
    if candidates.is_empty() {
        ui.device_selection = None;
        return;
    }

    let current = ui.device_selection.and_then(|selected| {
        candidates
            .iter()
            .position(|candidate| candidate.id() == selected)
    });
    let next = match (current, forward) {
        (Some(index), true) => (index + 1) % candidates.len(),
        (Some(0), false) | (None, false) => candidates.len() - 1,
        (None, true) => 0,
        (Some(index), false) => index - 1,
    };
    ui.device_selection = Some(candidates[next].id());
}

/// Resolves the selected device against the live candidate store.
///
/// A stale selection (the device disappeared) resolves to nothing and is
/// cleared, so a removed device can never be connected through the UI.
fn resolve_selected_device(model: &AppModel, ui: &mut UiState) -> Option<UserAction> {
    let selected = ui.device_selection?;
    if model
        .candidates()
        .iter()
        .any(|candidate| candidate.id() == selected)
    {
        Some(UserAction::SelectDevice(selected))
    } else {
        ui.device_selection = None;
        None
    }
}

/// Moves the review selection by one entry, wrapping at the ends.
fn move_file_selection(files: &mut FileSelectionInput, forward: bool) {
    let count = files.selection.len();
    if count == 0 {
        files.selected = None;
        return;
    }

    let next = match (files.selected, forward) {
        (Some(index), true) => (index + 1) % count,
        (Some(0), false) | (None, false) => count - 1,
        (None, true) => 0,
        (Some(index), false) => index - 1,
    };
    files.selected = Some(next);
}

/// Removes the highlighted review entry and keeps the cursor valid.
fn remove_selected_file(files: &mut FileSelectionInput) {
    let Some(index) = files.selected else {
        return;
    };
    if files.selection.remove(index) {
        let remaining = files.selection.len();
        files.selected = (remaining > 0).then(|| index.min(remaining - 1));
    }
}

/// Opens the command palette with an empty query and the first command selected.
fn open_palette(model: &AppModel, ui: &mut UiState) {
    let query = String::new();
    let selected = first_visible(model.capabilities(), &query);
    ui.overlay = Some(Overlay::CommandPalette(CommandPalette { query, selected }));
}

#[cfg(test)]
mod tests {
    use tokio::time::Instant;

    use super::{Overlay, UiState, apply_key_input, apply_paste, apply_user_action, reconcile};
    use crate::app::action::{DirectAddressError, DirectEndpoint, KeyInput, UserAction};
    use crate::app::failure::FailureKind;
    use crate::app::model::{AppModel, AppState};
    use crate::discovery::{DiscoveredService, DiscoveryEvent};
    use crate::transfer::selection::MAX_SELECTION_INPUT_BYTES;

    fn browsing_with(names: &[&str]) -> AppModel {
        let mut model = AppModel::for_test(AppState::Browsing);
        for name in names {
            model.apply_discovery(DiscoveryEvent::Resolved(DiscoveredService::for_test(
                name,
                Instant::now(),
            )));
        }
        model
    }

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
            Some(UserAction::OpenFileSelection)
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
        assert!(apply_user_action(&mut ui, UserAction::OpenFileSelection));
        assert!(matches!(ui.overlay(), Some(Overlay::FileSelection(_))));
        assert!(!apply_user_action(&mut ui, UserAction::Quit));
    }

    #[test]
    fn paste_is_bounded_and_only_reaches_the_review_input() {
        let mut ui = UiState::default();

        apply_paste(&mut ui, "ignored");
        assert_eq!(ui.overlay(), None);

        assert!(apply_user_action(&mut ui, UserAction::OpenFileSelection));
        apply_paste(&mut ui, &"x".repeat(MAX_SELECTION_INPUT_BYTES + 10));
        let Some(Overlay::FileSelection(files)) = ui.overlay() else {
            panic!("the review overlay should remain open");
        };
        assert_eq!(files.text.len(), MAX_SELECTION_INPUT_BYTES);
    }

    #[test]
    fn file_review_adds_removes_and_gates_send() {
        let root = std::env::temp_dir().join(format!("lanweave-review-{:016x}", fastrand::u64(..)));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first.txt");
        let second = root.join("second.txt");
        std::fs::write(&first, b"one").unwrap();
        std::fs::write(&second, b"two").unwrap();

        let mut ui = UiState::default();
        assert!(apply_user_action(&mut ui, UserAction::OpenFileSelection));

        // An empty review cannot send, even in an idle session.
        let session = AppModel::for_test(AppState::SessionIdle);
        assert_eq!(apply_key_input(&session, &mut ui, KeyInput::Enter), None);

        apply_paste(
            &mut ui,
            &format!("{}\n{}\n", first.display(), second.display()),
        );

        // Enter reviews the pasted paths; browsing cannot send them yet.
        let browsing = AppModel::for_test(AppState::Browsing);
        assert_eq!(apply_key_input(&browsing, &mut ui, KeyInput::Enter), None);
        let Some(Overlay::FileSelection(files)) = ui.overlay() else {
            panic!("the review overlay should remain open");
        };
        assert_eq!(files.selection.len(), 2);
        assert_eq!(apply_key_input(&browsing, &mut ui, KeyInput::Enter), None);

        // Down and backspace remove the highlighted entry.
        assert_eq!(apply_key_input(&browsing, &mut ui, KeyInput::Down), None);
        apply_key_input(&browsing, &mut ui, KeyInput::Backspace);
        let Some(Overlay::FileSelection(files)) = ui.overlay() else {
            panic!("the review overlay should remain open");
        };
        assert_eq!(files.selection.len(), 1);

        // An authorized idle session sends the remaining reviewed file.
        assert!(matches!(
            apply_key_input(&session, &mut ui, KeyInput::Enter),
            Some(UserAction::StartTransfer(_))
        ));
        assert_eq!(ui.overlay(), None);

        // Escape closes the review without sending.
        assert!(apply_user_action(&mut ui, UserAction::OpenFileSelection));
        assert_eq!(apply_key_input(&session, &mut ui, KeyInput::Escape), None);
        assert_eq!(ui.overlay(), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn review_overlay_closes_when_the_state_cannot_host_it() {
        let mut ui = UiState::default();
        assert!(apply_user_action(&mut ui, UserAction::OpenFileSelection));
        assert!(matches!(ui.overlay(), Some(Overlay::FileSelection(_))));

        let pairing = AppModel::for_test(AppState::PairingInbound);
        reconcile(&pairing, &mut ui);
        assert_eq!(ui.overlay(), None);
    }

    #[test]
    fn device_list_navigation_selects_wraps_and_connects() {
        let model = browsing_with(&["zeta", "alpha"]);
        let mut ui = UiState::default();

        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Down), None);
        let alpha = model.sorted_candidates()[0].id();
        assert_eq!(ui.device_selection(), Some(alpha));

        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Down), None);
        let zeta = model.sorted_candidates()[1].id();
        assert_eq!(ui.device_selection(), Some(zeta));

        // Navigation wraps at both ends.
        apply_key_input(&model, &mut ui, KeyInput::Down);
        assert_eq!(ui.device_selection(), Some(alpha));
        apply_key_input(&model, &mut ui, KeyInput::Up);
        assert_eq!(ui.device_selection(), Some(zeta));

        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Enter),
            Some(UserAction::SelectDevice(zeta))
        );

        // An empty list ignores navigation and cannot dispatch.
        let empty = browsing_with(&[]);
        let mut empty_ui = UiState::default();
        assert_eq!(apply_key_input(&empty, &mut empty_ui, KeyInput::Down), None);
        assert_eq!(empty_ui.device_selection(), None);
        assert_eq!(
            apply_key_input(&empty, &mut empty_ui, KeyInput::Enter),
            None
        );
    }

    #[test]
    fn removed_device_clears_selection_and_cannot_connect() {
        let mut model = browsing_with(&["alpha", "beta"]);
        let mut ui = UiState::default();
        apply_key_input(&model, &mut ui, KeyInput::Down);
        assert!(ui.device_selection().is_some());

        model.apply_discovery(DiscoveryEvent::Removed {
            service_instance: "alpha._lanweave._tcp.local.".to_owned(),
        });
        reconcile(&model, &mut ui);

        assert_eq!(ui.device_selection(), None);
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Enter), None);

        // A remaining device can still be selected and connected.
        apply_key_input(&model, &mut ui, KeyInput::Down);
        assert_eq!(
            ui.device_selection(),
            Some(model.sorted_candidates()[0].id())
        );
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Enter),
            Some(UserAction::SelectDevice(model.sorted_candidates()[0].id()))
        );
    }

    #[test]
    fn selection_survives_unrelated_updates_and_list_keys_are_ignored_outside_browsing() {
        let mut model = browsing_with(&["alpha"]);
        let mut ui = UiState::default();
        apply_key_input(&model, &mut ui, KeyInput::Down);
        let alpha = ui.device_selection().unwrap();

        model.apply_discovery(DiscoveryEvent::Resolved(DiscoveredService::for_test(
            "beta",
            Instant::now(),
        )));
        reconcile(&model, &mut ui);
        assert_eq!(ui.device_selection(), Some(alpha));

        for state in [
            AppState::Starting,
            AppState::PairingOutbound,
            AppState::SessionIdle,
            AppState::Error(FailureKind::Internal),
        ] {
            let mut busy = AppModel::for_test(state);
            busy.apply_discovery(DiscoveryEvent::Resolved(DiscoveredService::for_test(
                "peer",
                Instant::now(),
            )));
            let mut busy_ui = UiState::default();

            assert_eq!(apply_key_input(&busy, &mut busy_ui, KeyInput::Down), None);
            assert_eq!(busy_ui.device_selection(), None, "state: {state:?}");
            assert_eq!(
                apply_key_input(&busy, &mut busy_ui, KeyInput::Enter),
                None,
                "state: {state:?}"
            );
        }
    }

    #[test]
    fn direct_address_input_bounds_validates_and_dispatches() {
        let model = browsing_with(&[]);
        let mut ui = UiState::default();
        assert!(apply_user_action(&mut ui, UserAction::OpenDirectAddress));

        // Input is bounded.
        for _ in 0..super::MAX_DIRECT_ADDRESS_CHARS + 20 {
            apply_key_input(&model, &mut ui, KeyInput::Character('x'));
        }
        let Some(Overlay::DirectAddress(input)) = ui.overlay() else {
            panic!("the input overlay should remain open");
        };
        assert_eq!(input.text.chars().count(), super::MAX_DIRECT_ADDRESS_CHARS);

        // Empty input reports a missing host and never dispatches.
        for _ in 0..super::MAX_DIRECT_ADDRESS_CHARS {
            apply_key_input(&model, &mut ui, KeyInput::Backspace);
        }
        apply_key_input(&model, &mut ui, KeyInput::Enter);
        let Some(Overlay::DirectAddress(input)) = ui.overlay() else {
            panic!("the input overlay should remain open");
        };
        assert_eq!(input.error, Some(DirectAddressError::MissingHost));

        // A host without a port reports a missing port.
        for character in "peer.local".chars() {
            apply_key_input(&model, &mut ui, KeyInput::Character(character));
        }
        let Some(Overlay::DirectAddress(input)) = ui.overlay() else {
            panic!("the input overlay should remain open");
        };
        assert_eq!(input.error, Some(DirectAddressError::MissingPort));

        // An invalid port blocks Enter.
        for character in ":abc".chars() {
            apply_key_input(&model, &mut ui, KeyInput::Character(character));
        }
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Enter), None);
        let Some(Overlay::DirectAddress(input)) = ui.overlay() else {
            panic!("the input overlay should remain open");
        };
        assert_eq!(input.error, Some(DirectAddressError::InvalidPort));

        // A valid address dispatches and closes the overlay.
        for _ in 0..3 {
            apply_key_input(&model, &mut ui, KeyInput::Backspace);
        }
        for character in "4242".chars() {
            apply_key_input(&model, &mut ui, KeyInput::Character(character));
        }
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Enter),
            Some(UserAction::ConnectDirect(
                DirectEndpoint::parse("peer.local:4242").unwrap()
            ))
        );
        assert_eq!(ui.overlay(), None);

        // Escape closes the overlay without dispatching.
        apply_user_action(&mut ui, UserAction::OpenDirectAddress);
        assert_eq!(apply_key_input(&model, &mut ui, KeyInput::Escape), None);
        assert_eq!(ui.overlay(), None);
    }

    #[test]
    fn pairing_prompt_accepts_and_rejects_with_enter_and_escape() {
        let model = AppModel::for_test(AppState::PairingInbound);
        let mut ui = UiState::default();

        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Enter),
            Some(UserAction::AcceptPairing)
        );
        assert_eq!(
            apply_key_input(&model, &mut ui, KeyInput::Escape),
            Some(UserAction::RejectPairing)
        );

        // The same keys do nothing on other pairing screens.
        let outbound = AppModel::for_test(AppState::PairingOutbound);
        assert_eq!(apply_key_input(&outbound, &mut ui, KeyInput::Enter), None);
        assert_eq!(apply_key_input(&outbound, &mut ui, KeyInput::Escape), None);
    }

    #[test]
    fn code_entry_opens_accepts_only_digits_and_dispatches_exactly_eight() {
        let accepted = AppModel::for_test(AppState::PairingOutboundAccepted);
        let mut ui = UiState::default();
        reconcile(&accepted, &mut ui);
        assert!(matches!(ui.overlay(), Some(Overlay::PairingCode(_))));

        // Text keys are ignored; only digits edit the code.
        for character in "12ab34 56".chars() {
            apply_key_input(&accepted, &mut ui, KeyInput::Character(character));
        }
        let Some(Overlay::PairingCode(input)) = ui.overlay() else {
            panic!("the code input should remain open");
        };
        assert_eq!(input.grouped(), "1234 56");
        assert_eq!(apply_key_input(&accepted, &mut ui, KeyInput::Enter), None);

        // The ninth digit is dropped, and Enter dispatches the first eight.
        apply_key_input(&accepted, &mut ui, KeyInput::Character('7'));
        apply_key_input(&accepted, &mut ui, KeyInput::Character('8'));
        apply_key_input(&accepted, &mut ui, KeyInput::Character('9'));
        let Some(Overlay::PairingCode(input)) = ui.overlay() else {
            panic!("the code input should remain open");
        };
        assert_eq!(input.grouped(), "1234 5678");
        assert_eq!(
            apply_key_input(&accepted, &mut ui, KeyInput::Enter),
            Some(UserAction::SubmitPairingCode(
                crate::pairing::PairingCode::parse("12345678").unwrap()
            ))
        );

        // Backspace edits, and Escape cancels the pairing connection.
        let mut ui = UiState::default();
        reconcile(&accepted, &mut ui);
        apply_key_input(&accepted, &mut ui, KeyInput::Character('1'));
        apply_key_input(&accepted, &mut ui, KeyInput::Backspace);
        assert_eq!(
            apply_key_input(&accepted, &mut ui, KeyInput::Escape),
            Some(UserAction::Disconnect)
        );

        // Leaving the code-entry state closes the overlay.
        let confirming = AppModel::for_test(AppState::PairingConfirming);
        reconcile(&confirming, &mut ui);
        assert_eq!(ui.overlay(), None);
    }
}
